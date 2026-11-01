//! A minimal DNS wire codec for mDNS browsing (RFC 1035 / RFC 6762).
//!
//! Only what the mDNS source needs: one-question queries for PTR/A/SRV/TXT,
//! and response parsing that handles label compression, multiple answers and
//! unknown record types. Everything else (`AAAA`, DNSSEC, TSIG, ...) is skipped
//! — this is a LAN browser, not a resolver.

use std::net::Ipv4Addr;

use crate::engine::error::{Error, Result};

/// DNS record types this codec understands.
pub mod qtype {
    pub const A: u16 = 1;
    pub const PTR: u16 = 12;
    pub const TXT: u16 = 16;
    pub const SRV: u16 = 33;
}

/// Question class. mDNS sets the top bit (`QU`) to request a unicast reply,
/// which lets a browser on an ephemeral port avoid the well-known 5353 and any
/// conflict with the host's own avahi/mDNSResponder.
pub const CLASS_IN: u16 = 1;
pub const CLASS_IN_QU: u16 = 0x8001;

/// One parsed answer record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// Owner name, fully expanded (`_airplay._tcp.local`).
    pub name: String,
    pub qtype: u16,
    pub ttl: u32,
    pub data: AnswerData,
}

/// Interpreted record data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerData {
    /// A record.
    A(Ipv4Addr),
    /// PTR record — target name (a service instance or a hostname).
    Ptr(String),
    /// TXT record — each string is one `key=value` (possibly without `=`).
    Txt(Vec<String>),
    /// SRV record.
    Srv { port: u16, target: String },
    /// Anything else (AAAA, NSEC, ...): carried but not interpreted.
    Unknown { class: u16, rdata: Vec<u8> },
}

/// Build a DNS query for one name/type pair.
///
/// `id = 0` is correct for mDNS (the ID is ignored there); `single_shot` writes
/// no header, which lets several questions share one datagram.
pub fn encode_question(name: &str, ot: u16, class: u16) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(32 + name.len());
    bytes.extend_from_slice(&[0, 0]); // ID — ignored by mDNS
    bytes.extend_from_slice(&[0, 0]); // flags — standard query
    bytes.extend_from_slice(&[0, 1]); // QDCOUNT
    bytes.extend_from_slice(&[0, 0]); // ANCOUNT
    bytes.extend_from_slice(&[0, 0]); // NSCOUNT
    bytes.extend_from_slice(&[0, 0]); // ARCOUNT
    write_name(&mut bytes, name);
    bytes.extend_from_slice(&ot.to_be_bytes());
    bytes.extend_from_slice(&class.to_be_bytes());
    bytes
}

/// Raw `label` list, suitable for concatenating several questions.
pub fn encode_question_body(name: &str, ot: u16, class: u16) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(16 + name.len());
    write_name(&mut bytes, name);
    bytes.extend_from_slice(&ot.to_be_bytes());
    bytes.extend_from_slice(&class.to_be_bytes());
    bytes
}

/// Parse the answer section of a response (questions are skipped).
///
/// Malformed records stop the walk and return what was parsed so far — a
/// partially readable mDNS packet is still useful.
pub fn parse_answers(message: &[u8]) -> Result<Vec<Answer>> {
    if message.len() < 12 {
        return Err(Error::parse(
            "mDNS response",
            "packet shorter than the header",
        ));
    }

    let qd = u16::from_be_bytes([message[4], message[5]]) as usize;
    let mut an = u16::from_be_bytes([message[6], message[7]]) as usize;

    let mut cursor = 12usize;
    for _ in 0..qd {
        cursor = skip_name(message, cursor)?;
        cursor += 4; // qtype + qclass
        if cursor > message.len() {
            return Err(Error::parse(
                "mDNS response",
                "question runs past the packet",
            ));
        }
    }

    let mut answers = Vec::new();
    while an > 0 && cursor < message.len() {
        let (name, _) = read_name(message, cursor)?;
        cursor = skip_name(message, cursor)?;
        if cursor + 10 > message.len() {
            break;
        }
        let ot = u16::from_be_bytes([message[cursor], message[cursor + 1]]);
        let class = u16::from_be_bytes([message[cursor + 2], message[cursor + 3]]);
        let ttl = u32::from_be_bytes([
            message[cursor + 4],
            message[cursor + 5],
            message[cursor + 6],
            message[cursor + 7],
        ]);
        let rdlen = u16::from_be_bytes([message[cursor + 8], message[cursor + 9]]) as usize;
        let rdata = cursor + 10;
        if rdata + rdlen > message.len() {
            break;
        }

        let data = match ot {
            qtype::A if rdlen == 4 => AnswerData::A(Ipv4Addr::new(
                message[rdata],
                message[rdata + 1],
                message[rdata + 2],
                message[rdata + 3],
            )),
            qtype::PTR => match read_name(message, rdata) {
                Ok((target, _)) => AnswerData::Ptr(target),
                Err(_) => AnswerData::Unknown {
                    class,
                    rdata: message[rdata..rdata + rdlen].to_vec(),
                },
            },
            qtype::TXT => AnswerData::Txt(parse_txt(&message[rdata..rdata + rdlen])),
            qtype::SRV if rdlen >= 7 => {
                // priority(2) weight(2) port(2) target(labels)
                let port = u16::from_be_bytes([message[rdata + 4], message[rdata + 5]]);
                match read_name(message, rdata + 6) {
                    Ok((target, _)) => AnswerData::Srv { port, target },
                    Err(_) => AnswerData::Unknown {
                        class,
                        rdata: message[rdata..rdata + rdlen].to_vec(),
                    },
                }
            }
            _ => AnswerData::Unknown {
                class,
                rdata: message[rdata..rdata + rdlen].to_vec(),
            },
        };

        answers.push(Answer {
            name,
            qtype: ot,
            ttl,
            data,
        });
        an -= 1;
        cursor = rdata + rdlen;
    }

    Ok(answers)
}

/// Split a TXT rdlength block into its strings.
fn parse_txt(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let len = bytes[i] as usize;
        i += 1;
        if i + len > bytes.len() {
            break;
        }
        if let Ok(text) = std::str::from_utf8(&bytes[i..i + len]) {
            out.push(text.to_string());
        }
        i += len;
    }
    out
}

fn write_name(out: &mut Vec<u8>, name: &str) {
    let name = name.trim_end_matches('.');
    if name.is_empty() {
        out.push(0);
        return;
    }
    for label in name.split('.') {
        let label = label.as_bytes();
        // A label longer than 63 bytes cannot be encoded; split at the byte
        // boundary as best we can (never happens for .local names in practice).
        for chunk in label.chunks(63) {
            out.push(chunk.len() as u8);
            out.extend_from_slice(chunk);
        }
    }
    out.push(0);
}

/// Expand the name at `start`, returning `(name, bytes_consumed)`.
fn read_name(message: &[u8], start: usize) -> Result<(String, usize)> {
    let mut labels = Vec::new();
    let mut cursor = start;
    // Bytes consumed at the original position. A compression pointer ends the
    // name early, so the terminator's position is not the answer once we jump.
    let mut consumed = 0;
    for _ in 0..32 {
        if cursor >= message.len() {
            return Err(Error::parse("mDNS name", "name runs past the packet"));
        }
        let len = message[cursor] as usize;
        match len & 0xC0 {
            0 => {
                if len == 0 {
                    if consumed == 0 {
                        consumed = cursor + 1 - start;
                    }
                    return Ok((labels.join("."), consumed));
                }
                if cursor + 1 + len > message.len() {
                    return Err(Error::parse("mDNS name", "label runs past the packet"));
                }
                let label = &message[cursor + 1..cursor + 1 + len];
                let text = std::str::from_utf8(label)
                    .map_err(|_| Error::parse("mDNS name", "label is not UTF-8"))?;
                labels.push(text.to_string());
                cursor += 1 + len;
            }
            0xC0 => {
                // Compression pointer: 14-bit offset within the message.
                if cursor + 1 >= message.len() {
                    return Err(Error::parse("mDNS name", "pointer runs past the packet"));
                }
                let offset = ((len & 0x3F) << 8) | message[cursor + 1] as usize;
                if offset >= message.len() {
                    return Err(Error::parse("mDNS name", "pointer offset is out of range"));
                }
                if consumed == 0 {
                    consumed = cursor + 2 - start;
                }
                cursor = offset;
                continue;
            }
            _ => {
                return Err(Error::parse(
                    "mDNS name",
                    "unsupported label type (0b10 is reserved)",
                ));
            }
        }
    }
    Err(Error::parse("mDNS name", "too many labels (loop?)"))
}

/// Advance `cursor` past the name at that position without expanding it.
fn skip_name(message: &[u8], start: usize) -> Result<usize> {
    let mut cursor = start;
    for _ in 0..64 {
        if cursor >= message.len() {
            return Err(Error::parse("mDNS name", "name runs past the packet"));
        }
        let len = message[cursor] as usize;
        match len & 0xC0 {
            0 if len == 0 => return Ok(cursor + 1),
            0 => cursor += 1 + len,
            0xC0 => return Ok(cursor + 2),
            _ => return Err(Error::parse("mDNS name", "unsupported label type")),
        }
    }
    Err(Error::parse("mDNS name", "too many labels (loop?)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(answers: &[&[u8]]) -> Vec<u8> {
        let mut bytes = vec![0u8; 12];
        bytes[6] = answers.len() as u8;
        for a in answers {
            bytes.extend_from_slice(a);
        }
        bytes
    }

    fn name(label: &[&str], pointer: Option<u16>) -> Vec<u8> {
        match pointer {
            Some(p) => vec![0xC0, p as u8],
            None => {
                let mut out = Vec::new();
                for l in label {
                    out.push(l.len() as u8);
                    out.extend_from_slice(l.as_bytes());
                }
                out.push(0);
                out
            }
        }
    }

    #[test]
    fn encodes_a_question() {
        let bytes = encode_question("_services._dns.local", qtype::PTR, CLASS_IN);
        assert_eq!(
            &bytes[0..6],
            &[0, 0, 0, 0, 0, 1],
            "standard query, one question"
        );
        assert_eq!(bytes[6..12], [0, 0, 0, 0, 0, 0]);
        assert_eq!(&bytes[12..34], b"\x09_services\x04_dns\x05local\x00");
        assert_eq!(&bytes[34..38], &[0, qtype::PTR as u8, 0, CLASS_IN as u8]);
    }

    #[test]
    fn parses_an_a_record() {
        let mut bytes = message(&[]);
        bytes[6] = 2;
        // Record 1: owner `_x._tcp.local`, PTR -> `host.local`.
        bytes.extend_from_slice(&name(&["_x", "_tcp", "local"], None));
        bytes.extend_from_slice(&[0, qtype::PTR as u8, 0, 1, 0, 0, 0, 120, 0, 12]);
        bytes.extend_from_slice(&name(&["host", "local"], None));
        // Record 2: owner pointer back to record 1, A 10.0.0.1.
        bytes.extend_from_slice(&[0xC0, 0x0C, 0, 1, 0, 1, 0, 0, 0, 120, 0, 4, 10, 0, 0, 1]);

        let answers = parse_answers(&bytes).unwrap();
        assert_eq!(answers.len(), 2);
        assert_eq!(answers[0].name, "_x._tcp.local");
        assert_eq!(answers[0].data, AnswerData::Ptr("host.local".to_string()));
        assert_eq!(answers[1].data, AnswerData::A("10.0.0.1".parse().unwrap()));
    }

    #[test]
    fn parses_txt_records() {
        let dns_txt = [
            3, b'm', b'd', b'=', 10, b'A', b'p', b'p', b'l', b'e', b'T', b'V', b'6', b',', b'1', 0,
        ];
        let mut bytes = message(&[]);
        bytes[6] = 1;
        bytes.extend_from_slice(&name(&["_x", "_tcp", "local"], None));
        bytes.extend_from_slice(&[
            0,
            qtype::TXT as u8,
            0,
            1,
            0,
            0,
            0,
            120,
            0,
            dns_txt.len() as u8,
        ]);
        bytes.extend_from_slice(&dns_txt);

        let answers = parse_answers(&bytes).unwrap();
        match &answers[0].data {
            AnswerData::Txt(entries) => {
                assert_eq!(
                    entries,
                    &["md=".to_string(), "AppleTV6,1".to_string(), "".to_string()]
                );
            }
            other => panic!("expected TXT, got {other:?}"),
        }
    }

    #[test]
    fn parses_srv_records() {
        // SRV: priority=0 weight=0 port=7000 target=host.local (pointer)
        let mut bytes = message(&[]);
        bytes[6] = 1;
        bytes.extend_from_slice(&name(&["_x", "_tcp", "local"], None));
        bytes.extend_from_slice(&[0, qtype::SRV as u8, 0, 1, 0, 0, 0, 120, 0, 8]);
        bytes.extend_from_slice(&[0, 0, 0, 0, 0x1B, 0x58, 0xC0, 0x0C]);

        let answers = parse_answers(&bytes).unwrap();
        assert_eq!(
            answers[0].data,
            AnswerData::Srv {
                port: 7000,
                target: "_x._tcp.local".to_string()
            }
        );
    }

    #[test]
    fn skips_unknown_record_types() {
        let mut bytes = message(&[]);
        bytes[6] = 1;
        bytes.extend_from_slice(&name(&["_x", "_tcp", "local"], None));
        bytes.extend_from_slice(&[0, 255, 0, 1, 0, 0, 0, 120, 0, 2, 0xAA, 0xBB]);

        let answers = parse_answers(&bytes).unwrap();
        assert!(matches!(answers[0].data, AnswerData::Unknown { .. }));
    }

    #[test]
    fn truncated_packets_return_what_was_parsed() {
        let mut bytes = message(&[]);
        bytes[6] = 1;
        bytes.extend_from_slice(&name(&["_x", "_tcp", "local"], None));
        bytes.extend_from_slice(&[0, qtype::A as u8, 0, 1, 0, 0, 0, 120, 0, 4, 10]);

        let answers = parse_answers(&bytes).unwrap();
        assert!(answers.is_empty(), "no complete records");
    }

    #[test]
    fn header_guard_rejects_tiny_packets() {
        assert!(parse_answers(&[0; 8]).is_err());
    }
}
