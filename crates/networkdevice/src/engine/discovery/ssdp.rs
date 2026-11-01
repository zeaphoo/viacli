//! SSDP / UPnP discovery (the `M-SEARCH` half of UPnP).
//!
//! Cheap, unprivileged and chatty: an `M-SEARCH` datagram to the SSDP group
//! makes routers, smart TVs, speakers and NAS boxes answer with their device
//! type and a URL pointing at an XML description. Fetching that description
//! yields the `friendlyName` and `modelName` — the same "what is this thing"
//! payload mDNS provides.
//!
//! Like the mDNS source, one scan is one bounded poll: send, listen for a
//! moment, optionally fetch descriptions, and emit what was learned.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

use crate::engine::discovery::arpjoin::ArpJoin;
use crate::engine::discovery::{Discovery, ScanOutcome};
use crate::engine::error::Result;
use crate::engine::model::{DiscoverySource, ObservationInfo};

const SSDP_GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
const SSDP_PORT: u16 = 1900;
const RECV_BUF: usize = 4096;
const MAX_DEVICES: usize = 64;
const MAX_DESCRIPTION_BYTES: usize = 64 * 1024;

const SEARCH_REQUEST: &str = "M-SEARCH * HTTP/1.1\r\n\
    HOST: 239.255.255.250:1900\r\n\
    MAN: \"ssdp:discover\"\r\n\
    MX: 2\r\n\
    ST: ssdp:all\r\n\
    \r\n";

/// One device as assembled from its SSDP answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsdpDevice {
    pub ip: Ipv4Addr,
    pub iface: Option<String>,
    /// Announced service/device types (`urn:schemas-upnp-org:device:...`).
    pub types: Vec<String>,
    /// UPnP `friendlyName`, when the description could be fetched.
    pub hostname: Option<String>,
    /// UPnP `modelName`.
    pub model: Option<String>,
}

/// SSDP / UPnP discovery source.
#[derive(Debug, Clone)]
pub struct SsdpDiscovery {
    ifaces: Vec<String>,
    /// How long to listen for answers after the search.
    listen: Duration,
    /// Fetch the UPnP description XML (one small HTTP GET per device).
    fetch_descriptions: bool,
}

impl Default for SsdpDiscovery {
    fn default() -> Self {
        SsdpDiscovery {
            ifaces: Vec::new(),
            listen: Duration::from_millis(2500),
            fetch_descriptions: true,
        }
    }
}

impl SsdpDiscovery {
    pub fn new(ifaces: Vec<String>) -> Self {
        SsdpDiscovery {
            ifaces,
            ..Default::default()
        }
    }

    pub fn with_listen(mut self, listen: Duration) -> Self {
        self.listen = listen;
        self
    }

    pub fn with_descriptions(mut self, fetch: bool) -> Self {
        self.fetch_descriptions = fetch;
        self
    }

    fn interfaces(&self) -> Vec<(String, Ipv4Addr)> {
        let Ok(subnets) = crate::engine::netinfo::scan_subnets(&self.ifaces) else {
            return Vec::new();
        };
        subnets.into_iter().map(|s| (s.iface, s.addr)).collect()
    }
}

/// One parsed answer, before devices are assembled.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SsdpAnswer {
    ip: Ipv4Addr,
    iface: Option<String>,
    st: Option<String>,
    location: Option<String>,
}

impl Discovery for SsdpDiscovery {
    fn source(&self) -> DiscoverySource {
        DiscoverySource::Ssdp
    }

    fn describe(&self) -> String {
        format!(
            "SSDP/UPnP M-SEARCH ({}ms listen, descriptions {})",
            self.listen.as_millis(),
            if self.fetch_descriptions { "on" } else { "off" }
        )
    }

    fn scan(&self) -> Result<ScanOutcome> {
        let interfaces = self.interfaces();
        if interfaces.is_empty() {
            return Ok(ScanOutcome::warning(
                Vec::new(),
                "SSDP: no usable IPv4 interface",
            ));
        }

        let mut warnings = Vec::new();
        let mut answers: Vec<SsdpAnswer> = Vec::new();

        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for (iface, addr) in &interfaces {
                let listen = self.listen;
                let iface_name = iface.clone();
                handles.push((
                    iface.clone(),
                    scope.spawn(move || search_iface(&iface_name, addr, listen)),
                ));
            }
            for (iface, handle) in handles {
                match handle.join() {
                    Ok(Ok(found)) => answers.extend(found),
                    Ok(Err(e)) => warnings.push(format!("SSDP on {iface}: {e}")),
                    Err(_) => warnings.push(format!("SSDP on {iface}: search thread panicked")),
                }
            }
        });

        // One device per source address, union of announced types.
        let mut by_ip: BTreeMap<Ipv4Addr, SsdpDevice> = BTreeMap::new();
        let mut locations: BTreeMap<Ipv4Addr, String> = BTreeMap::new();
        for answer in answers {
            let device = by_ip.entry(answer.ip).or_insert_with(|| SsdpDevice {
                ip: answer.ip,
                iface: answer.iface.clone(),
                types: Vec::new(),
                hostname: None,
                model: None,
            });
            if device.iface.is_none() {
                device.iface = answer.iface.clone();
            }
            if let Some(st) = answer.st.filter(|s| !s.is_empty()) {
                if !device.types.contains(&st) {
                    device.types.push(st);
                }
            }
            if let Some(location) = answer.location {
                locations.entry(answer.ip).or_insert(location);
            }
        }

        if self.fetch_descriptions {
            let mut requests: Vec<(Ipv4Addr, String)> = locations.into_iter().collect();
            requests.truncate(MAX_DEVICES);
            let fetched = fetch_descriptions(&requests, &mut warnings);
            for (ip, (hostname, model)) in fetched {
                if let Some(device) = by_ip.get_mut(&ip) {
                    device.hostname = hostname;
                    device.model = model;
                }
            }
        }

        let join = ArpJoin::from_proc();
        let observations = by_ip
            .into_values()
            .map(|device| {
                let info = ObservationInfo {
                    hostname: device.hostname,
                    model: device.model,
                    services: device.types,
                    ..Default::default()
                };
                join.observation(
                    device.ip,
                    device.iface.as_deref(),
                    DiscoverySource::Ssdp,
                    Some(info),
                )
            })
            .collect();

        Ok(ScanOutcome {
            observations,
            warnings,
        })
    }
}

/// Run one M-SEARCH on an interface and collect the answers.
fn search_iface(iface: &str, addr: &Ipv4Addr, listen: Duration) -> Result<Vec<SsdpAnswer>> {
    // Bind to the interface address so the search goes out that interface and
    // the answers come back to this socket.
    let socket = UdpSocket::bind((*addr, 0))?;
    socket.set_read_timeout(Some(Duration::from_millis(150)))?;

    let group = SocketAddr::from((SSDP_GROUP, SSDP_PORT));
    socket.send_to(SEARCH_REQUEST.as_bytes(), group)?;

    let deadline = Instant::now() + listen;
    let mut buf = [0u8; RECV_BUF];
    let mut answers = Vec::new();

    while Instant::now() < deadline {
        let Ok((len, peer)) = socket.recv_from(&mut buf) else {
            continue;
        };
        let Ok(text) = std::str::from_utf8(&buf[..len]) else {
            continue;
        };
        if let Some(mut answer) = parse_answer(text, peer) {
            answer.iface = Some(iface.to_string());
            answers.push(answer);
        }
    }

    Ok(answers)
}

/// Parse one datagram into an answer, or `None` for a byebye/unrelated message.
fn parse_answer(text: &str, peer: SocketAddr) -> Option<SsdpAnswer> {
    let mut lines = text.lines();
    let status = lines.next()?.trim();
    let is_search_reply = status.starts_with("HTTP/1.1 200") || status.starts_with("HTTP/1.0 200");
    let is_notify = status.starts_with("NOTIFY");
    if !is_search_reply && !is_notify {
        return None;
    }

    let mut st = None;
    let mut location = None;
    let mut nt = None;
    let mut alive = true;

    for line in lines {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim().to_ascii_lowercase().as_str() {
            "st" => st = Some(value.to_string()),
            "nt" => nt = Some(value.to_string()),
            "location" => location = Some(value.to_string()),
            "nts" => alive = !value.eq_ignore_ascii_case("ssdp:byebye"),
            _ => {}
        }
    }

    if !alive {
        return None;
    }
    // Search replies carry `ST`; NOTIFY carries `NT`. Normalise to one field.
    let st = st.or(nt);

    let ip = match peer.ip() {
        std::net::IpAddr::V4(ip) => ip,
        std::net::IpAddr::V6(_) => return None,
    };

    Some(SsdpAnswer {
        ip,
        iface: None,
        st,
        location,
    })
}

/// Identity fields pulled out of a UPnP description.
type DescriptionFields = (Option<String>, Option<String>);

/// Fetch every description in parallel, bounded by one thread per device.
fn fetch_descriptions(
    requests: &[(Ipv4Addr, String)],
    warnings: &mut Vec<String>,
) -> BTreeMap<Ipv4Addr, DescriptionFields> {
    let mut out = BTreeMap::new();

    let results: Vec<(Ipv4Addr, std::result::Result<DescriptionFields, String>)> =
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for (ip, location) in requests {
                let ip = *ip;
                let location = location.clone();
                handles.push((
                    ip,
                    location.clone(),
                    scope.spawn(move || fetch_description(&location)),
                ));
            }
            handles
                .into_iter()
                .map(|(ip, location, handle)| {
                    let result = handle
                        .join()
                        .unwrap_or_else(|_| Err("description fetch panicked".to_string()));
                    (ip, result.map_err(|e| format!("{location}: {e}")))
                })
                .collect()
        });

    for (ip, result) in results {
        match result {
            Ok(fields) => {
                out.insert(ip, fields);
            }
            Err(e) => warnings.push(format!("SSDP description: {e}")),
        }
    }

    out
}

/// GET a UPnP description URL and pull the identity fields out of the XML.
fn fetch_description(location: &str) -> std::result::Result<DescriptionFields, String> {
    let (host, port, path) = parse_location(location)?;
    let timeout = Duration::from_millis(900);

    let mut stream = TcpStream::connect_timeout(&SocketAddr::from((host, port)), timeout)
        .map_err(|e| e.to_string())?;
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let request =
        format!("GET {path} HTTP/1.0\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;

    let mut body = Vec::new();
    let mut chunk = [0u8; 4096];
    while body.len() < MAX_DESCRIPTION_BYTES {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }

    Ok((
        extract_tag(&body, "friendlyName"),
        extract_tag(&body, "modelName"),
    ))
}

/// Split `http://host:port/path` into its parts.
fn parse_location(location: &str) -> std::result::Result<(Ipv4Addr, u16, String), String> {
    let rest = location
        .strip_prefix("http://")
        .ok_or_else(|| format!("unsupported scheme in `{location}`"))?;

    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (
            host,
            port.parse::<u16>()
                .map_err(|_| format!("bad port in `{location}`"))?,
        ),
        None => (authority, 80),
    };

    let host: Ipv4Addr = host
        .parse()
        .map_err(|_| format!("non-IPv4 host in `{location}` (mDNS names are not fetched)"))?;

    Ok((host, port, path))
}

/// Case-insensitive, whitespace-tolerant `<tag>value</tag>` extraction.
fn extract_tag(body: &[u8], tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let owned = String::from_utf8_lossy(body);
    let text: &str = owned.as_ref();

    let start = find_ignore_ascii_case(text, &open)? + open.len();
    let end = start + find_ignore_ascii_case(&text[start..], &close)?;
    let value = decode_entities(text[start..end].trim());
    (!value.is_empty()).then_some(value)
}

/// Byte offset of `needle` in `haystack`, ASCII-case-insensitively.
fn find_ignore_ascii_case(haystack: &str, needle: &str) -> Option<usize> {
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len())
        .find(|&i| haystack[i..i + needle.len()].eq_ignore_ascii_case(needle))
}

/// Decode the handful of XML entities that show up in device names.
fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(ip: &str) -> SocketAddr {
        SocketAddr::from((ip.parse::<Ipv4Addr>().unwrap(), 1900))
    }

    #[test]
    fn parses_a_search_reply() {
        let text = "HTTP/1.1 200 OK\r\n\
            CACHE-CONTROL: max-age=1800\r\n\
            LOCATION: http://192.168.3.70:5000/description.xml\r\n\
            ST: urn:schemas-upnp-org:device:MediaServer:1\r\n\
            USN: uuid:abc::urn:schemas-upnp-org:device:MediaServer:1\r\n\
            SERVER: Linux/4.4 UPnP/1.0 NAS/1.0\r\n\r\n";
        let answer = parse_answer(text, peer("192.168.3.70")).unwrap();
        assert_eq!(answer.ip, "192.168.3.70".parse::<Ipv4Addr>().unwrap());
        assert_eq!(
            answer.st.as_deref(),
            Some("urn:schemas-upnp-org:device:MediaServer:1")
        );
        assert_eq!(
            answer.location.as_deref(),
            Some("http://192.168.3.70:5000/description.xml")
        );
    }

    #[test]
    fn parses_notify_alive_and_ignores_byebye() {
        let alive = "NOTIFY * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nNT: upnp:rootdevice\r\nNTS: ssdp:alive\r\nLOCATION: http://10.0.0.5:80/d.xml\r\n\r\n";
        assert!(parse_answer(alive, peer("10.0.0.5")).is_some());

        let byebye = "NOTIFY * HTTP/1.1\r\nNT: upnp:rootdevice\r\nNTS: ssdp:byebye\r\n\r\n";
        assert!(parse_answer(byebye, peer("10.0.0.5")).is_none());
    }

    #[test]
    fn ignores_unrelated_datagrams() {
        assert!(parse_answer("some garbage", peer("10.0.0.5")).is_none());
    }

    #[test]
    fn extracts_and_decodes_xml_tags() {
        let xml = b"<?xml version=\"1.0\"?><root><device>\
            <friendlyName>Living Room &amp; TV</friendlyName>\
            <modelName>BRAVIA XR</modelName></device></root>";
        assert_eq!(
            extract_tag(xml, "friendlyName").as_deref(),
            Some("Living Room & TV")
        );
        assert_eq!(extract_tag(xml, "modelName").as_deref(), Some("BRAVIA XR"));
        assert_eq!(extract_tag(xml, "manufacturer"), None);
    }

    #[test]
    fn parses_location_urls() {
        assert_eq!(
            parse_location("http://10.0.0.5:5000/d.xml").unwrap(),
            ("10.0.0.5".parse().unwrap(), 5000, "/d.xml".to_string())
        );
        assert_eq!(
            parse_location("http://10.0.0.5/d.xml").unwrap(),
            ("10.0.0.5".parse().unwrap(), 80, "/d.xml".to_string())
        );
        assert!(parse_location("https://10.0.0.5/d.xml").is_err());
        assert!(parse_location("http://nas.local/d.xml").is_err());
    }

    #[test]
    fn describe_reports_the_configuration() {
        let discovery = SsdpDiscovery::new(vec!["eth0".into()]).with_descriptions(false);
        assert!(discovery.describe().contains("SSDP"));
        assert!(discovery.describe().contains("descriptions off"));
        assert_eq!(discovery.source(), DiscoverySource::Ssdp);
    }
}
