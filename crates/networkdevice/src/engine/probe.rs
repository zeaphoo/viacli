//! The probe: describe the addresses the presence sources already found.
//!
//! Every other source scans a range it was told about. This one takes the
//! *inventory* as its target list — the addresses seen alive, and only those the
//! probe has not described yet (or described longer ago than the configured TTL).
//! That is what makes the second phase of the scan possible without a second
//! scheduler: the probe is a source like any other, it just reads the snapshot
//! first.
//!
//! What it does per address:
//!
//! * reverse DNS and the OUI vendor lookup (`identify.rs`);
//! * a TCP port probe — opt-in via the existing `port_probe` setting, which is
//!   also the consent switch for everything below, because those touch hosts;
//! * an HTTP request on the ports that look like a web interface, for the
//!   `Server` header, the page title and the favicon, all of which go through
//!   the web rule table in `fingerprint::web` — that is what turns "some page"
//!   into "RouterOS", "Synology DSM" or "Hikvision";
//! * one SNMP `GetRequest` for `sysDescr` — the single OID that identifies
//!   switches, printers and UPSs, which answer nothing else;
//! * a TLS handshake on the ports that look encrypted, for the certificate's
//!   CN/SAN — the cheapest way to learn a device's real name;
//! * a Modbus "Read Device Identification" request, which returns the vendor,
//!   product and version the device itself declares;
//! * a BACnet `Who-Is`, whose `I-Am` carries the device instance and vendor id;
//! * a NetBIOS `NBSTAT` query, which returns the Windows machine and domain
//!   names that nothing else on a LAN will tell you.
//!
//! Results leave as an ordinary [`Observation`] from
//! [`DiscoverySource::Probe`], so the inventory keeps exactly one write path.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream, UdpSocket};
use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme};

use crate::engine::discovery::{Discovery, ScanOutcome};
use crate::engine::error::{Error, Result};
use crate::engine::fingerprint;
use crate::engine::fingerprint::tcp::{Engine, ServiceMatch};
use crate::engine::identify::Identifier;
use crate::engine::model::{DiscoverySource, Observation, ObservationInfo, PortInfo, ProbeTarget};

/// Ports the nmap-style probe table may spend a connection on, per address.
const MAX_TCP_PROBES: usize = 24;
/// The port SNMP answers on, which `enrich_snmp` already covers.
const SNMP_PORT: u16 = 161;
/// Cap on the HTTP response we keep: enough for headers and `<title>`.
const HTTP_READ_LIMIT: usize = 8 * 1024;
/// Web interfaces are tried in this order, at most this many per address.
const MAX_WEB_PROBES: usize = 3;
/// `1.3.6.1.2.1.1.1.0` — the description a device gives of itself.
const OID_SYS_DESCR: [u8; 8] = [0x2B, 0x06, 0x01, 0x02, 0x01, 0x01, 0x01, 0x00];
/// `1.3.6.1.2.1.1.2.0` — the vendor's own object identifier for this device.
const OID_SYS_OBJECT_ID: [u8; 8] = [0x2B, 0x06, 0x01, 0x02, 0x01, 0x01, 0x02, 0x00];
/// `1.3.6.1.2.1.1.5.0` — the name the device calls itself.
const OID_SYS_NAME: [u8; 8] = [0x2B, 0x06, 0x01, 0x02, 0x01, 0x01, 0x05, 0x00];
/// The three of them, asked for in one request.
const SNMP_OIDS: [[u8; 8]; 3] = [OID_SYS_DESCR, OID_SYS_OBJECT_ID, OID_SYS_NAME];

pub struct ProbeDiscovery {
    targets: Vec<ProbeTarget>,
    identifier: Identifier,
    timeout: Duration,
    /// The TCP probe table: the built-in one, plus whatever probe file the
    /// settings point at.
    engine: Arc<Engine>,
}

impl ProbeDiscovery {
    pub fn new(
        targets: Vec<ProbeTarget>,
        identifier: Identifier,
        timeout: Duration,
        engine: Arc<Engine>,
    ) -> Self {
        ProbeDiscovery {
            targets,
            identifier,
            timeout,
            engine,
        }
    }

    /// The extra probes are part of the port probe's bargain: `port_probe`
    /// decides whether this source touches a host at all.
    fn touches_hosts(&self) -> bool {
        self.identifier.probe.is_some()
    }
}

impl Discovery for ProbeDiscovery {
    fn source(&self) -> DiscoverySource {
        DiscoverySource::Probe
    }

    fn describe(&self) -> String {
        let count = self.targets.len();
        if !self.touches_hosts() {
            return format!("reverse DNS only ({count} target(s))");
        }
        let ports = self
            .identifier
            .probe
            .as_ref()
            .map(|probe| probe.ports.len())
            .unwrap_or(0);
        format!(
            "{count} target(s): {ports} TCP port(s), {} service probe(s), web, SNMP, TLS, Modbus, BACnet, NetBIOS",
            self.engine.len()
        )
    }

    fn scan(&self) -> Result<ScanOutcome> {
        let mut observations = Vec::with_capacity(self.targets.len());
        let mut warnings = Vec::new();

        for target in &self.targets {
            let identification = self.identifier.identify(target.ip);

            let mut info = ObservationInfo {
                hostname: identification.hostname,
                ports: identification.open_ports,
                ..Default::default()
            };

            if self.touches_hosts() {
                enrich_tls(&mut info, target.ip, self.timeout, &mut warnings);
                enrich_modbus(&mut info, target.ip, self.timeout, &mut warnings);
                enrich_web(&mut info, target.ip, self.timeout, &mut warnings);
                enrich_snmp(&mut info, target.ip, self.timeout);
                enrich_bacnet(&mut info, target.ip, self.timeout);
                enrich_netbios(&mut info, target.ip, self.timeout);
                // Last, so the ports the probes above claimed are known and each
                // of the rest gets the best probe available rather than two.
                enrich_tcp(&mut info, &self.engine, target.ip, self.timeout);
            }

            observations.push(Observation {
                ip: target.ip,
                mac: target.mac,
                iface: target.iface.clone(),
                source: DiscoverySource::Probe,
                info: Some(info),
            });
        }

        Ok(ScanOutcome {
            observations,
            warnings,
        })
    }
}

// ------------------------------------------------------- service probing

/// The ports a more specific probe has already claimed: the web, TLS, SNMP,
/// Modbus, BACnet and NetBIOS ones. Each of those knows more about its port than
/// a banner probe could, and re-connecting would only be wasted traffic.
fn claimed_ports(info: &ObservationInfo) -> Vec<u16> {
    let mut ports = web_ports(&info.ports);
    ports.extend(
        info.ports
            .iter()
            .filter(|entry| is_tls_service(&entry.service))
            .map(|entry| entry.port),
    );
    ports.extend([SNMP_PORT, MODBUS_PORT, BACNET_PORT, NETBIOS_PORT]);
    ports
}

/// Run the nmap-style probe table over the ports nothing else covers.
fn enrich_tcp(info: &mut ObservationInfo, engine: &Engine, ip: Ipv4Addr, timeout: Duration) {
    let claimed = claimed_ports(info);
    let ports: Vec<u16> = info
        .ports
        .iter()
        .map(|entry| entry.port)
        .filter(|port| !claimed.contains(port))
        .take(MAX_TCP_PROBES)
        .collect();
    if ports.is_empty() {
        return;
    }

    for (port, found) in engine.identify(ip, &ports, timeout) {
        apply_service_match(info, port, &found);
    }
}

/// Fold one identified service into the observation.
fn apply_service_match(info: &mut ObservationInfo, port: u16, found: &ServiceMatch) {
    let description = found.describe();

    if let Some(entry) = info.ports.iter_mut().find(|entry| entry.port == port) {
        // The probe's verdict beats the port hint: it is evidence, not a guess.
        entry.service = Some(found.service.clone());
        entry.banner = Some(match (description.as_deref(), entry.banner.take()) {
            (Some(description), Some(banner)) if !banner.contains(description) => {
                format!("{description} | {banner}")
            }
            (Some(description), _) => description.to_string(),
            (None, Some(banner)) => banner,
            (None, None) => found.response.clone(),
        });
    }

    let mut services = vec![format!("tcp:{}", found.service)];
    if let Some(product) = found.product.as_deref() {
        services.push(format!("tcp:{}", slug(product)));
    }
    if let Some(os) = found.os.as_deref() {
        services.push(format!("os:{}", slug(os)));
    }
    for service in services {
        if !info.services.contains(&service) {
            info.services.push(service);
        }
    }

    if info.kind.is_none() {
        info.kind = found
            .device_type
            .as_deref()
            .and_then(fingerprint::device_type_kind);
    }
    if info.hostname.is_none() {
        info.hostname = found.hostname.clone();
    }
}

// ------------------------------------------------------------------ web

/// The open ports that look like a web interface, best guess first.
fn web_ports(ports: &[PortInfo]) -> Vec<u16> {
    // IPP is HTTP with a printer's name on it, and the page it serves is a
    // printer's web interface — worth the same request.
    let looks_web = |service: &Option<String>| {
        service
            .as_deref()
            .is_some_and(|service| service.starts_with("http") || service == "ipp")
    };
    ports
        .iter()
        .filter(|port| looks_web(&port.service))
        .map(|port| port.port)
        .take(MAX_WEB_PROBES)
        .collect()
}

/// The largest favicon worth fetching and hashing.
const FAVICON_LIMIT: usize = 32 * 1024;

/// A parsed HTTP/1.x response. The body stays as bytes because a favicon is not
/// text; the decoded text is kept beside it, so the rule engine can borrow it
/// instead of rebuilding it for every rule.
#[derive(Debug, Default)]
struct HttpResponse {
    status: Option<u16>,
    headers: Vec<(String, String)>,
    cookies: Vec<String>,
    body: Vec<u8>,
    text: String,
}

impl HttpResponse {
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .and_then(|(_, value)| clean_value(value))
    }

    /// A favicon hash, only when the body really is an icon: hashing the body of
    /// a 404 page would be a confident wrong answer, and the public databases it
    /// is meant to be looked up in are built from real icons.
    fn icon_hash(&self) -> Option<u32> {
        let is_icon =
            self.status == Some(200) && !self.body.is_empty() && !looks_like_html(&self.body);
        is_icon.then(|| fingerprint::web::favicon_hash(&self.body))
    }
}

/// The front page and, when it was worth asking for, its icon.
#[derive(Debug, Default)]
struct FrontPage {
    page: HttpResponse,
    favicon: Option<HttpResponse>,
}

impl FrontPage {
    fn server(&self) -> Option<String> {
        self.page.header("server")
    }

    fn title(&self) -> Option<String> {
        parse_title(&self.page.text)
    }

    fn favicon_hash(&self) -> Option<u32> {
        self.favicon.as_ref().and_then(HttpResponse::icon_hash)
    }

    /// The page as the rule engine wants it.
    fn rules(&self) -> fingerprint::web::Page<'_> {
        fingerprint::web::Page {
            headers: &self.page.headers,
            cookies: &self.page.cookies,
            text: &self.page.text,
            favicon_hash: self.favicon_hash(),
        }
    }
}

/// Ask for the front page and keep what identifies the device.
fn enrich_web(
    info: &mut ObservationInfo,
    ip: Ipv4Addr,
    timeout: Duration,
    warnings: &mut Vec<String>,
) {
    for (index, port) in web_ports(&info.ports).into_iter().enumerate() {
        // The favicon costs one more request, so only the most likely web
        // interface gets one.
        match http_front_page(ip, port, timeout, index == 0) {
            Ok(Some(page)) => apply_front_page(info, port, &page),
            Ok(None) => {}
            Err(e) => warnings.push(format!("web probe on {ip}:{port}: {e}")),
        }
    }
}

/// Fold a front page into the observation: the port banner, and everything the
/// rules recognised.
fn apply_front_page(info: &mut ObservationInfo, port: u16, page: &FrontPage) {
    let found = fingerprint::web::evaluate(&page.rules());
    let rule = found.as_ref().map(|found| found.rule);
    let title = page.title();
    let favicon = page.favicon_hash();

    if let Some(entry) = info.ports.iter_mut().find(|entry| entry.port == port) {
        let mut parts = Vec::new();
        if let Some(server) = page.server() {
            parts.push(format!("Server: {server}"));
        }
        if let Some(rule) = rule {
            parts.push(rule.name.to_string());
        }
        if let Some(title) = title.clone() {
            parts.push(title);
        }
        if let Some(hash) = favicon {
            parts.push(format!("favicon={hash}"));
        }
        entry.banner = Some(if parts.is_empty() {
            "HTTP".to_string()
        } else {
            parts.join(" | ")
        });
    }

    if let Some(rule) = rule {
        // A rule that named the product is better evidence than the page title,
        // which is often just "Login".
        if info.vendor.is_none() {
            info.vendor = rule.vendor.map(str::to_string);
        }
        if info.kind.is_none() {
            info.kind = rule.kind;
        }
        let service = format!("web:{}", slug(rule.name));
        if !info.services.contains(&service) {
            info.services.push(service);
        }
    }
    if info.model.is_none() {
        info.model = title.filter(|title| !is_generic_title(title));
    }
    if !info.services.iter().any(|service| service == "http-ui") {
        info.services.push("http-ui".to_string());
    }
}

/// `GET /`, plus `/favicon.ico` when the caller asks for one.
fn http_front_page(
    ip: Ipv4Addr,
    port: u16,
    timeout: Duration,
    with_favicon: bool,
) -> Result<Option<FrontPage>> {
    let Some(raw) = http_get(ip, port, "/", timeout, HTTP_READ_LIMIT)? else {
        return Ok(None);
    };
    let page = parse_http_response(&raw);
    if page.status.is_none() && page.headers.is_empty() {
        return Ok(None);
    }

    let mut front = FrontPage {
        page,
        favicon: None,
    };
    if with_favicon
        && let Ok(Some(raw)) = http_get(ip, port, "/favicon.ico", timeout, FAVICON_LIMIT)
    {
        front.favicon = Some(parse_http_response(&raw));
    }
    Ok(Some(front))
}

/// One bounded `GET`. `Ok(None)` when the peer accepted the connection and then
/// said nothing at all.
fn http_get(
    ip: Ipv4Addr,
    port: u16,
    path: &str,
    timeout: Duration,
    limit: usize,
) -> Result<Option<Vec<u8>>> {
    let address = SocketAddr::from((ip, port));
    let mut stream = TcpStream::connect_timeout(&address, timeout).map_err(Error::PlainIo)?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();

    let request = format!("GET {path} HTTP/1.0\r\nHost: {ip}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(Error::PlainIo)?;

    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    while buffer.len() < limit {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
            Err(_) => break, // a slow or silent service is not an error
        }
    }
    if buffer.is_empty() {
        return Ok(None);
    }
    Ok(Some(buffer))
}

/// Split a response into status, headers, cookies and body. Pure, so it is
/// testable against bytes no server would ever send.
fn parse_http_response(raw: &[u8]) -> HttpResponse {
    let (head, body) = split_head(raw);
    let head = String::from_utf8_lossy(head);
    let mut lines = head.split('\n').map(|line| line.trim_end_matches('\r'));

    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok());

    let mut headers = Vec::new();
    let mut cookies = Vec::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        if name.is_empty() {
            continue;
        }
        if name.eq_ignore_ascii_case("set-cookie")
            && let Some(cookie) = value.split('=').next().map(str::trim)
            && !cookie.is_empty()
        {
            cookies.push(cookie.to_string());
        }
        headers.push((name.to_string(), value.to_string()));
    }

    HttpResponse {
        status,
        headers,
        cookies,
        text: String::from_utf8_lossy(body).into_owned(),
        body: body.to_vec(),
    }
}

/// The head and the body of an HTTP response, at the first blank line. A
/// response with no blank line is all head, which is what a bodyless one looks
/// like.
fn split_head(raw: &[u8]) -> (&[u8], &[u8]) {
    for separator in [b"\r\n\r\n".as_slice(), b"\n\n".as_slice()] {
        if let Some(at) = find_bytes(raw, separator) {
            return (&raw[..at], &raw[at + separator.len()..]);
        }
    }
    (raw, &[])
}

/// `haystack.find(needle)` for bytes, which `[u8]` does not provide.
fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Is this an HTML page rather than, say, an icon?
fn looks_like_html(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
    ["<html", "<!doctype", "<head", "<body"]
        .iter()
        .any(|marker| head.contains(marker))
}

/// The `<title>` of a page, cleaned up. Pure, so it is testable.
fn parse_title(text: &str) -> Option<String> {
    let lowered = text.to_ascii_lowercase();
    let start = lowered.find("<title")?;
    let open = lowered[start..].find('>')? + start + 1;
    let close = lowered[open..].find("</title")? + open;
    clean_value(&text[open..close])
}

/// Titles that say nothing about the device. A router's page is very often
/// titled "Login", and putting that in the model column would be worse than
/// leaving it empty.
fn is_generic_title(title: &str) -> bool {
    const GENERIC: &[&str] = &[
        "login",
        "log in",
        "sign in",
        "login page",
        "web ui",
        "webui",
        "web interface",
        "administration",
        "admin",
        "index",
        "home",
        "welcome",
        "document",
        "document moved",
        "redirect",
        "redirecting",
        "loading",
        "device",
        "router",
        "untitled",
        // A captive portal or an API stub answers "Success" to everything.
        "success",
        "ok",
        "it works!",
    ];
    let title = title.trim().to_ascii_lowercase();
    GENERIC.contains(&title.as_str())
}

/// A rule name as a service token: `HP Embedded Web Server` →
/// `hp-embedded-web-server`.
fn slug(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Trim, collapse whitespace and cap a header or title value.
fn clean_value(value: &str) -> Option<String> {
    let cleaned: String = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() {
        return None;
    }
    Some(cleaned.chars().take(120).collect())
}

// ------------------------------------------------------------------ snmp

/// Add `sysDescr` when the host answers SNMP.
///
/// Switches, printers and UPSs rarely answer a port probe but almost always
/// answer this, so it runs for every target rather than only for an open port.
fn enrich_snmp(info: &mut ObservationInfo, ip: Ipv4Addr, timeout: Duration) {
    let Some(answer) = snmp_identity(ip, timeout) else {
        return;
    };

    if !info.services.iter().any(|service| service == "snmp") {
        info.services.push("snmp".to_string());
    }
    if !info.ports.iter().any(|entry| entry.port == 161) {
        info.ports.push(PortInfo {
            port: 161,
            service: Some("snmp".into()),
            banner: Some(answer.describe()),
        });
    }
    if info.model.is_none() {
        info.model = answer.descr.clone();
    }
    if info.hostname.is_none() {
        info.hostname = answer.name.clone();
    }
}

fn snmp_identity(ip: Ipv4Addr, timeout: Duration) -> Option<SnmpIdentity> {
    let response = udp_exchange(ip, 161, &snmp_get_request("public", &SNMP_OIDS), timeout)?;
    let identity = parse_snmp_identity(&response);
    (identity != SnmpIdentity::default()).then_some(identity)
}

/// What one SNMP request can tell us about a device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SnmpIdentity {
    descr: Option<String>,
    object_id: Option<String>,
    name: Option<String>,
}

impl SnmpIdentity {
    fn describe(&self) -> String {
        let mut parts = vec!["SNMP".to_string()];
        for (label, value) in [
            ("descr", &self.descr),
            ("oid", &self.object_id),
            ("name", &self.name),
        ] {
            if let Some(value) = value {
                parts.push(format!("{label}={value}"));
            }
        }
        parts.join(" ")
    }
}

/// An SNMP v2c `GetRequest` for one OID, encoded by hand: the whole protocol is
/// BER, and one request is cheaper than a dependency.
fn snmp_get_request(community: &str, oids: &[[u8; 8]]) -> Vec<u8> {
    fn tlv(tag: u8, body: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        if body.len() < 0x80 {
            out.push(body.len() as u8);
        } else {
            out.push(0x81);
            out.push(body.len() as u8);
        }
        out.extend_from_slice(body);
        out
    }
    fn integer(value: i64) -> Vec<u8> {
        let bytes = value.to_be_bytes();
        let start = bytes
            .iter()
            .position(|byte| *byte != 0 || *byte >= 0x80)
            .unwrap_or(7);
        tlv(0x02, &bytes[start..])
    }

    let mut list = Vec::new();
    for oid in oids {
        list.extend_from_slice(&tlv(0x30, &tlv(0x06, oid)));
    }
    let varbinds = tlv(0x30, &list);
    // [0] = context-specific, constructed: the GetRequest PDU.
    let pdu = tlv(
        0xA0,
        &[integer(1), integer(0), integer(0), varbinds].concat(),
    );
    tlv(
        0x30,
        &[integer(1), tlv(0x04, community.as_bytes()), pdu].concat(),
    )
}

/// Read the three values out of a response. Pure, so it is testable.
fn parse_snmp_identity(response: &[u8]) -> SnmpIdentity {
    SnmpIdentity {
        descr: snmp_string(response, &OID_SYS_DESCR),
        object_id: snmp_value(response, &OID_SYS_OBJECT_ID)
            .filter(|(tag, _)| *tag == 0x06)
            .and_then(|(_, value)| decode_oid(value)),
        name: snmp_string(response, &OID_SYS_NAME),
    }
}

/// The text of an OCTET STRING varbind.
fn snmp_string(response: &[u8], oid: &[u8]) -> Option<String> {
    let (tag, value) = snmp_value(response, oid)?;
    (tag == 0x04).then(|| clean_value(&String::from_utf8_lossy(value)))?
}

/// The value TLV that follows `oid` in a response: `(tag, bytes)`.
fn snmp_value<'a>(response: &'a [u8], oid: &[u8]) -> Option<(u8, &'a [u8])> {
    let at = response
        .windows(oid.len())
        .position(|window| window == oid)?;
    let rest = &response[at + oid.len()..];
    if rest.first() != Some(&0x04) && rest.first() != Some(&0x06) {
        return None;
    }
    let (length, offset) = match rest.get(1)? {
        length if *length < 0x80 => (*length as usize, 2),
        0x81 => (*rest.get(2)? as usize, 3),
        _ => return None,
    };
    Some((*rest.first()?, rest.get(offset..offset + length)?))
}

/// Decode an OBJECT IDENTIFIER's contents into dotted notation.
///
/// The first byte packs the first two components; the rest are base-128, where a
/// byte with the high bit set continues into the next one — which is why
/// `1.3.6.1.4.1.8072` (Net-SNMP) needs it.
fn decode_oid(bytes: &[u8]) -> Option<String> {
    let (&first, rest) = bytes.split_first()?;
    let mut parts = vec![u32::from(first / 40), u32::from(first % 40)];

    let mut current: u64 = 0;
    let mut pending = false;
    for byte in rest {
        current = (current << 7) | u64::from(byte & 0x7F);
        pending = byte & 0x80 != 0;
        if !pending {
            parts.push(u32::try_from(current).ok()?);
            current = 0;
        }
    }
    if pending || parts.len() < 2 {
        return None;
    }

    Some(
        parts
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join("."),
    )
}

// ------------------------------------------------------------ transports

/// One UDP request/response exchange. `None` means silence, which is an answer.
fn udp_exchange(ip: Ipv4Addr, port: u16, payload: &[u8], timeout: Duration) -> Option<Vec<u8>> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.set_read_timeout(Some(timeout)).ok();
    socket.send_to(payload, SocketAddr::from((ip, port))).ok()?;
    let mut buffer = [0u8; 4096];
    let (read, _) = socket.recv_from(&mut buffer).ok()?;
    Some(buffer[..read].to_vec())
}

/// Connect, send, and read until the peer stops or the cap is reached.
fn tcp_exchange(
    ip: Ipv4Addr,
    port: u16,
    request: &[u8],
    timeout: Duration,
    limit: usize,
) -> Result<Vec<u8>> {
    let address = SocketAddr::from((ip, port));
    let mut stream = TcpStream::connect_timeout(&address, timeout).map_err(Error::PlainIo)?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    stream.write_all(request).map_err(Error::PlainIo)?;

    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    while buffer.len() < limit {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
            Err(_) => break,
        }
    }
    Ok(buffer)
}

// ------------------------------------------------------------------ tls

/// Service hints that mean "this port speaks TLS".
fn is_tls_service(service: &Option<String>) -> bool {
    let Some(service) = service.as_deref() else {
        return false;
    };
    // Ports the identify pass labelled as an encrypted variant of something.
    [
        "https", "ssl", "tls", "imaps", "smtps", "pop3s", "ldaps", "ftps", "wss",
    ]
    .iter()
    .any(|prefix| service.starts_with(prefix))
}

/// Read the certificate of every port that looks encrypted.
///
/// Done with an in-process `rustls` handshake rather than an `openssl`
/// subprocess: no process to spawn per port, no binary to install, and the
/// socket's own timeouts bound the handshake. The certificate is only wanted
/// for the names inside it, so verification is deliberately skipped.
fn enrich_tls(
    info: &mut ObservationInfo,
    ip: Ipv4Addr,
    timeout: Duration,
    warnings: &mut Vec<String>,
) {
    let ports: Vec<u16> = info
        .ports
        .iter()
        .filter(|port| is_tls_service(&port.service))
        .map(|port| port.port)
        .take(MAX_TLS_PROBES)
        .collect();
    if ports.is_empty() {
        return;
    }

    // Owned, because the certificate may become this address's hostname.
    let sni = info
        .hostname
        .as_deref()
        .filter(|name| is_dns_name(name))
        .map(str::to_string);

    for port in ports {
        match certificate(ip, port, sni.as_deref(), timeout) {
            Ok(Some(facts)) => {
                if let Some(entry) = info.ports.iter_mut().find(|entry| entry.port == port) {
                    entry.banner = Some(facts.describe());
                }
                if !info.services.iter().any(|service| service == "tls") {
                    info.services.push("tls".to_string());
                }
                if info.hostname.is_none() {
                    info.hostname = facts.name();
                }
            }
            Ok(None) => {}
            Err(e) => warnings.push(format!("tls probe on {ip}:{port}: {e}")),
        }
    }
}

const MAX_TLS_PROBES: usize = 2;

/// What a certificate says, when it says anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CertificateFacts {
    subject: Option<String>,
    issuer: Option<String>,
    san: Vec<String>,
}

impl CertificateFacts {
    /// The first name the certificate claims, preferring a subjectAltName.
    fn name(&self) -> Option<String> {
        self.san
            .iter()
            .find(|name| is_dns_name(name))
            .cloned()
            .or_else(|| self.subject.clone())
    }

    fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(subject) = &self.subject {
            parts.push(format!("subject={subject}"));
        }
        if let Some(issuer) = &self.issuer {
            parts.push(format!("issuer={issuer}"));
        }
        if !self.san.is_empty() {
            parts.push(format!("san={}", self.san.join(",")));
        }
        if self.subject.is_some() && self.subject == self.issuer {
            parts.push("self-signed".to_string());
        }
        format!("TLS {}", parts.join(" "))
    }
}

fn certificate(
    ip: Ipv4Addr,
    port: u16,
    sni: Option<&str>,
    timeout: Duration,
) -> Result<Option<CertificateFacts>> {
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate))
        .with_no_client_auth();

    // A DNS name from reverse DNS is the SNI; without one, the address itself.
    let server_name = sni
        .and_then(|name| ServerName::try_from(name.to_string()).ok())
        .unwrap_or_else(|| ServerName::IpAddress(std::net::IpAddr::V4(ip).into()));

    let connection = ClientConnection::new(Arc::new(config), server_name)
        .map_err(|e| Error::failed("tls", e.to_string()))?;

    let socket = TcpStream::connect_timeout(&SocketAddr::from((ip, port)), timeout)
        .map_err(Error::PlainIo)?;
    socket.set_read_timeout(Some(timeout)).map_err(Error::PlainIo)?;
    socket.set_write_timeout(Some(timeout)).map_err(Error::PlainIo)?;

    // The blocking stream drives the handshake. The socket timeouts bound it,
    // so a port that never completes TLS cannot hang the probe.
    let mut stream = rustls::StreamOwned::new(connection, socket);
    stream
        .conn
        .complete_io(&mut stream.sock)
        .map_err(Error::PlainIo)?;

    let Some(leaf) = stream
        .conn
        .peer_certificates()
        .and_then(|certs| certs.first())
        .map(|cert| cert.to_vec())
    else {
        return Ok(None);
    };

    Ok(parse_der_certificate(&leaf))
}

/// Accept any certificate: this is a scanner, not a client. A LAN device serves
/// a self-signed or private-CA certificate, and the point is the handshake and
/// the names inside, not trust.
#[derive(Debug)]
struct AcceptAnyCertificate;

impl ServerCertVerifier for AcceptAnyCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Pull the subject CN, the issuer CN and the DNS names out of a DER
/// certificate. `None` when it says nothing useful.
fn parse_der_certificate(der: &[u8]) -> Option<CertificateFacts> {
    let (_, parsed) = x509_parser::parse_x509_certificate(der).ok()?;
    let subject = parsed
        .subject()
        .iter_common_name()
        .next()
        .and_then(|attribute| attribute.as_str().ok())
        .map(str::to_string);
    let issuer = parsed
        .issuer()
        .iter_common_name()
        .next()
        .and_then(|attribute| attribute.as_str().ok())
        .map(str::to_string);

    let mut san = Vec::new();
    if let Ok(Some(extension)) = parsed.subject_alternative_name() {
        for name in &extension.value.general_names {
            if let x509_parser::extensions::GeneralName::DNSName(name) = name
                && is_dns_name(name)
            {
                san.push((*name).to_string());
            }
        }
    }

    (subject.is_some() || issuer.is_some() || !san.is_empty()).then_some(CertificateFacts {
        subject,
        issuer,
        san,
    })
}

/// A name that is safe to store and to send as an SNI value.
fn is_dns_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && name.len() <= 253
        && !name.starts_with('-')
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-' || byte == b'_'
        })
}

// --------------------------------------------------------------- modbus

/// Modbus has exactly one well-known port, and only there can this run.
const MODBUS_PORT: u16 = 502;

/// Ask the device to identify itself: vendor, product, version, by its own word.
///
/// Function 43 / MEI 14 ("Read Device Identification") is what every Modbus
/// device implements, and the answer is a small list of `[id, length, text]`.
fn enrich_modbus(
    info: &mut ObservationInfo,
    ip: Ipv4Addr,
    timeout: Duration,
    warnings: &mut Vec<String>,
) {
    if !info.ports.iter().any(|port| port.port == MODBUS_PORT) {
        return;
    }
    match modbus_identity(ip, timeout) {
        Ok(Some(identity)) => {
            if let Some(entry) = info
                .ports
                .iter_mut()
                .find(|entry| entry.port == MODBUS_PORT)
            {
                entry.service = Some("modbus".to_string());
                entry.banner = Some(identity.describe());
            }
            if info.model.is_none() {
                info.model = identity.model();
            }
            if !info.services.iter().any(|service| service == "modbus") {
                info.services.push("modbus".to_string());
            }
        }
        Ok(None) => {}
        Err(e) => warnings.push(format!("modbus probe on {ip}:{MODBUS_PORT}: {e}")),
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ModbusIdentity {
    vendor: Option<String>,
    product: Option<String>,
    version: Option<String>,
}

impl ModbusIdentity {
    fn model(&self) -> Option<String> {
        match (&self.vendor, &self.product) {
            (Some(vendor), Some(product)) => Some(format!("{vendor} {product}")),
            (Some(vendor), None) => Some(vendor.clone()),
            (None, Some(product)) => Some(product.clone()),
            (None, None) => None,
        }
    }

    fn describe(&self) -> String {
        let mut parts = vec!["Modbus".to_string()];
        for (label, value) in [
            ("vendor", &self.vendor),
            ("product", &self.product),
            ("version", &self.version),
        ] {
            if let Some(value) = value {
                parts.push(format!("{label}={value}"));
            }
        }
        parts.join(" ")
    }
}

fn modbus_identity(ip: Ipv4Addr, timeout: Duration) -> Result<Option<ModbusIdentity>> {
    // Transaction id, protocol 0, length 5, unit 0xff, function 43 (0x2b),
    // MEI type 14 (0x0e), read device id code 1 (basic), first object 0.
    let request = [
        0x00, 0x01, 0x00, 0x00, 0x00, 0x05, 0xFF, 0x2B, 0x0E, 0x01, 0x00,
    ];
    let response = tcp_exchange(ip, MODBUS_PORT, &request, timeout, 1024)?;
    Ok(parse_modbus_identity(&response))
}

/// Read the object list out of a 43/14 response. Pure, so it is testable.
fn parse_modbus_identity(response: &[u8]) -> Option<ModbusIdentity> {
    if response.len() < 8 || response[7] != 0x2B {
        return None;
    }
    // MBAP(7) + function(1) + MEI type(1) + code(1) + conformity(1)
    //       + more follows(1) + next object(1) + object count(1)
    let mut at = 8;
    if response.get(at) != Some(&0x0E) {
        return None;
    }
    at += 3; // MEI type, read device id code, conformity level
    at += 1; // more follows
    at += 1; // next object id
    let count = *response.get(at)? as usize;
    at += 1;

    let mut identity = ModbusIdentity::default();
    for _ in 0..count {
        let id = *response.get(at)?;
        let length = *response.get(at + 1)? as usize;
        let value = response.get(at + 2..at + 2 + length)?;
        let text = clean_value(&String::from_utf8_lossy(value));
        match id {
            0x00 => identity.vendor = text,
            0x01 => identity.product = text,
            0x02 => identity.version = text,
            // 0x03 vendor url, 0x04 product name, 0x05 model name, 0x06 app name
            0x05 if identity.product.is_none() => identity.product = text,
            _ => {}
        }
        at += 2 + length;
    }

    (identity != ModbusIdentity::default()).then_some(identity)
}

// --------------------------------------------------------------- bacnet

const BACNET_PORT: u16 = 47808;

/// A directed `Who-Is`: BVLC original-unicast, NPDU version 1, APDU `Who-Is`.
fn bacnet_who_is() -> [u8; 8] {
    [0x81, 0x0B, 0x00, 0x04, 0x01, 0x00, 0x10, 0x08]
}

/// BACnet devices answer a `Who-Is` with an `I-Am` carrying their instance and
/// vendor id — two stable identifiers that no other probe on this list gives.
fn enrich_bacnet(info: &mut ObservationInfo, ip: Ipv4Addr, timeout: Duration) {
    let Some(response) = udp_exchange(ip, BACNET_PORT, &bacnet_who_is(), timeout) else {
        return;
    };
    let Some(device) = parse_bacnet_i_am(&response) else {
        return;
    };

    info.ports.push(PortInfo {
        port: BACNET_PORT,
        service: Some("bacnet".into()),
        banner: Some(device.describe()),
    });
    if !info.services.iter().any(|service| service == "bacnet") {
        info.services.push("bacnet".to_string());
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BacnetDevice {
    instance: u32,
    vendor_id: u16,
}

impl BacnetDevice {
    fn describe(&self) -> String {
        format!("BACnet device {} vendor {}", self.instance, self.vendor_id)
    }
}

/// Pull the device instance and vendor id out of an `I-Am`. Pure.
///
/// The APDU is `10 00 68` followed by application-tagged fields in a fixed order:
/// object identifier, max APDU, segmentation, vendor id.
fn parse_bacnet_i_am(response: &[u8]) -> Option<BacnetDevice> {
    if response.len() < 8 || response[0] != 0x81 {
        return None;
    }
    // The BVLC length counts the NPDU and APDU — everything after the 4-byte
    // BVLC header. A few stacks count the header too, so both readings pass.
    let declared = u16::from_be_bytes([response[2], response[3]]) as usize;
    if declared + 4 != response.len() && declared != response.len() {
        return None;
    }

    let at = response.windows(3).position(|w| w == [0x10, 0x00, 0x68])? + 3;
    let (tag, length, mut index) = read_tag(response, at)?;

    // Application tag 12 is the object identifier: 10 bits of type in the high
    // bits, 22 bits of instance below it.
    if tag != 12 || length != 4 {
        return None;
    }
    let raw = response.get(index..index + 4)?;
    let instance = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) & 0x003F_FFFF;
    index += 4;

    // max APDU length, then segmentation supported.
    for _ in 0..2 {
        let (_, length, next) = read_tag(response, index)?;
        index = next + length;
    }

    let (tag, length, value_at) = read_tag(response, index)?;
    if tag != 2 || !(1..=2).contains(&length) {
        return None;
    }
    let mut vendor_id = 0u16;
    for byte in response.get(value_at..value_at + length)? {
        vendor_id = vendor_id << 8 | u16::from(*byte);
    }

    Some(BacnetDevice {
        instance,
        vendor_id,
    })
}

/// Read one BACnet application/context tag: `(number, length, value offset)`.
fn read_tag(data: &[u8], at: usize) -> Option<(u8, usize, usize)> {
    let first = *data.get(at)?;
    let tag = first >> 4;
    let mut length = (first & 0x0F) as usize;
    let mut index = at + 1;
    if length == 5 {
        let extra = *data.get(index)? as usize;
        length = extra;
        index += 1;
    }
    if length == 6 || length == 7 {
        return None; // extended/opening tags: not needed for an I-Am
    }
    Some((tag, length, index))
}

// -------------------------------------------------------------- netbios

const NETBIOS_PORT: u16 = 137;

/// A NetBIOS node status (`NBSTAT`) query for the wildcard name.
///
/// The 34-byte question is the wildcard `*` in NetBIOS's half-ASCII encoding,
/// padded to 16 characters. Layout follows fscan's `netbios.go` (MIT).
fn netbios_nbstat_query() -> [u8; 50] {
    [
        0x66, 0x66, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
        0x20, 0x43, 0x4B, 0x41, 0x41, 0x41, 0x41, 0x41, 0x41, 0x41, 0x41, 0x41, //
        0x41, 0x41, 0x41, 0x41, 0x41, 0x41, 0x41, 0x41, 0x41, 0x41, 0x41, 0x41, //
        0x41, 0x41, 0x41, 0x41, 0x41, 0x41, 0x41, 0x41, 0x41, 0x00, //
        0x00, 0x21, 0x00, 0x01,
    ]
}

/// Windows keeps its machine and domain names in NetBIOS, and answers this
/// without any credentials.
fn enrich_netbios(info: &mut ObservationInfo, ip: Ipv4Addr, timeout: Duration) {
    let Some(response) = udp_exchange(ip, NETBIOS_PORT, &netbios_nbstat_query(), timeout) else {
        return;
    };
    let Some(names) = parse_nbstat(&response) else {
        return;
    };

    info.ports.push(PortInfo {
        port: NETBIOS_PORT,
        service: Some("netbios".into()),
        banner: Some(names.describe()),
    });
    if !info.services.iter().any(|service| service == "netbios") {
        info.services.push("netbios".to_string());
    }
    if info.hostname.is_none() {
        info.hostname = names.workstation.clone();
    }
    if info.model.is_none() {
        info.model = names.server.clone();
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct NbstatNames {
    workstation: Option<String>,
    server: Option<String>,
    domain: Option<String>,
}

impl NbstatNames {
    fn describe(&self) -> String {
        let mut parts = vec!["NetBIOS".to_string()];
        for (label, value) in [
            ("name", &self.workstation),
            ("server", &self.server),
            ("domain", &self.domain),
        ] {
            if let Some(value) = value {
                parts.push(format!("{label}={value}"));
            }
        }
        parts.join(" ")
    }
}

/// Names out of an `NBSTAT` response: a count, then 18-byte records of
/// (15 characters, suffix, flags). Pure.
///
/// The record offset is *found* rather than assumed: the response is a DNS
/// message, so the question section is walked (following compression pointers)
/// and the answer's RDLENGTH is read. A fixed offset works on one layout only,
/// and stacks differ in whether they echo the question.
fn parse_nbstat(response: &[u8]) -> Option<NbstatNames> {
    const RECORD: usize = 18;

    if response.len() < 12 {
        return None;
    }
    let questions = u16::from_be_bytes([response[4], response[5]]) as usize;
    let answers = u16::from_be_bytes([response[6], response[7]]) as usize;
    if answers == 0 {
        return None;
    }

    let mut at = 12;
    for _ in 0..questions {
        at = skip_name(response, at)?.checked_add(4)?; // QTYPE + QCLASS
    }

    // First answer: name, type(2), class(2), ttl(4), rdlength(2), rdata.
    at = skip_name(response, at)?.checked_add(8)?;
    let rdata_length = u16::from_be_bytes([*response.get(at)?, *response.get(at + 1)?]) as usize;
    let rdata = response.get(at + 2..at + 2 + rdata_length)?;

    let count = *rdata.first()? as usize;
    if count == 0 {
        return None;
    }
    let records = rdata.get(1..)?;

    let mut names = NbstatNames::default();
    for index in 0..count {
        let at = index * RECORD;
        let record = records.get(at..at + RECORD)?;
        let Some(name) = clean_value(&String::from_utf8_lossy(&record[..15])) else {
            continue;
        };
        let suffix = record[15];
        let group = record[16] >= 0x80;

        match (suffix, group) {
            (0x00, false) => names.workstation.get_or_insert(name),
            (0x00, true) => names.domain.get_or_insert(name),
            (0x20, false) => names.server.get_or_insert(name),
            _ => continue,
        };
    }

    (names != NbstatNames::default()).then_some(names)
}

/// Step over a DNS name at `at`: either a compression pointer or labels
/// terminated by a zero length.
fn skip_name(data: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let length = *data.get(at)? as usize;
        if length == 0 {
            return Some(at + 1);
        }
        if length & 0xC0 == 0xC0 {
            return Some(at + 2);
        }
        at += 1 + length;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    // ---------------------------------------------------------- tls

    /// A self-signed certificate (CN=diskstation.lan, SANs diskstation.lan and
    /// nas.lan), generated once and embedded so the parser needs no openssl.
    const TEST_CERT_DER: &[u8] = &[
        0x30, 0x82, 0x01, 0xad, 0x30, 0x82, 0x01, 0x54, 0xa0, 0x03, 0x02, 0x01,
        0x02, 0x02, 0x14, 0x7e, 0x1c, 0x28, 0x22, 0x4c, 0x80, 0x0a, 0xd2, 0xe2,
        0x6c, 0x02, 0x0f, 0xc5, 0xce, 0xcb, 0x53, 0x4a, 0x9a, 0x0c, 0xda, 0x30,
        0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02, 0x30,
        0x1a, 0x31, 0x18, 0x30, 0x16, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x0f,
        0x64, 0x69, 0x73, 0x6b, 0x73, 0x74, 0x61, 0x74, 0x69, 0x6f, 0x6e, 0x2e,
        0x6c, 0x61, 0x6e, 0x30, 0x1e, 0x17, 0x0d, 0x32, 0x36, 0x30, 0x39, 0x31,
        0x35, 0x30, 0x38, 0x31, 0x30, 0x35, 0x35, 0x5a, 0x17, 0x0d, 0x33, 0x36,
        0x30, 0x39, 0x31, 0x32, 0x30, 0x38, 0x31, 0x30, 0x35, 0x35, 0x5a, 0x30,
        0x1a, 0x31, 0x18, 0x30, 0x16, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0c, 0x0f,
        0x64, 0x69, 0x73, 0x6b, 0x73, 0x74, 0x61, 0x74, 0x69, 0x6f, 0x6e, 0x2e,
        0x6c, 0x61, 0x6e, 0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48,
        0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03,
        0x01, 0x07, 0x03, 0x42, 0x00, 0x04, 0x32, 0x71, 0x32, 0x1b, 0xb4, 0x63,
        0x28, 0xf3, 0x1d, 0xc8, 0xfd, 0x13, 0xdd, 0xec, 0xfc, 0x7b, 0xe0, 0xb7,
        0x3c, 0x46, 0xc9, 0x7c, 0x65, 0x4d, 0x1b, 0xcb, 0x07, 0xdd, 0x6a, 0x36,
        0x60, 0xcf, 0x62, 0x29, 0x3b, 0x41, 0xf8, 0xc9, 0xcb, 0x29, 0x30, 0x40,
        0x9a, 0x9c, 0x18, 0x2e, 0x4e, 0x7e, 0x62, 0x81, 0x5f, 0x91, 0xdd, 0x1e,
        0xf8, 0xa8, 0x07, 0xd2, 0xf0, 0xe1, 0xc4, 0x62, 0x7c, 0xfa, 0xa3, 0x78,
        0x30, 0x76, 0x30, 0x1d, 0x06, 0x03, 0x55, 0x1d, 0x0e, 0x04, 0x16, 0x04,
        0x14, 0x56, 0x50, 0x67, 0xc7, 0x55, 0x72, 0xe5, 0x88, 0x91, 0xd8, 0x99,
        0xf5, 0x09, 0x39, 0x74, 0x13, 0x7d, 0x0c, 0xa3, 0x5a, 0x30, 0x1f, 0x06,
        0x03, 0x55, 0x1d, 0x23, 0x04, 0x18, 0x30, 0x16, 0x80, 0x14, 0x56, 0x50,
        0x67, 0xc7, 0x55, 0x72, 0xe5, 0x88, 0x91, 0xd8, 0x99, 0xf5, 0x09, 0x39,
        0x74, 0x13, 0x7d, 0x0c, 0xa3, 0x5a, 0x30, 0x0f, 0x06, 0x03, 0x55, 0x1d,
        0x13, 0x01, 0x01, 0xff, 0x04, 0x05, 0x30, 0x03, 0x01, 0x01, 0xff, 0x30,
        0x23, 0x06, 0x03, 0x55, 0x1d, 0x11, 0x04, 0x1c, 0x30, 0x1a, 0x82, 0x0f,
        0x64, 0x69, 0x73, 0x6b, 0x73, 0x74, 0x61, 0x74, 0x69, 0x6f, 0x6e, 0x2e,
        0x6c, 0x61, 0x6e, 0x82, 0x07, 0x6e, 0x61, 0x73, 0x2e, 0x6c, 0x61, 0x6e,
        0x30, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02,
        0x03, 0x47, 0x00, 0x30, 0x44, 0x02, 0x20, 0x40, 0x4c, 0x47, 0x1f, 0xcd,
        0x68, 0x59, 0x25, 0xfd, 0x17, 0xec, 0x7b, 0x01, 0x45, 0x0f, 0x2a, 0x30,
        0xfd, 0xe9, 0xad, 0x5e, 0x09, 0xe7, 0x90, 0xb2, 0xf8, 0x5f, 0xca, 0x77,
        0x80, 0xdd, 0xb8, 0x02, 0x20, 0x53, 0x61, 0x52, 0x0c, 0x45, 0xb8, 0x26,
        0x5a, 0xe6, 0x7f, 0x99, 0x92, 0x40, 0x09, 0x55, 0xb3, 0xf9, 0xf9, 0xec,
        0x3f, 0xa4, 0xf7, 0x15, 0x41, 0xd5, 0x15, 0x04, 0xfe, 0x9b, 0x3d, 0x73,
        0x22,
    ];

    #[test]
    fn the_der_parser_reads_subject_issuer_and_san() {
        let facts = parse_der_certificate(TEST_CERT_DER).expect("facts");
        assert_eq!(facts.subject.as_deref(), Some("diskstation.lan"));
        assert_eq!(facts.issuer.as_deref(), Some("diskstation.lan"));
        assert_eq!(facts.san, vec!["diskstation.lan", "nas.lan"]);
        assert_eq!(facts.name().as_deref(), Some("diskstation.lan"));
        assert!(
            facts.describe().contains("self-signed"),
            "{}",
            facts.describe()
        );
    }

    #[test]
    fn garbage_is_not_a_certificate() {
        assert!(parse_der_certificate(b"not a certificate").is_none());
    }

    /// The TLS path with a real handshake: `openssl s_server` serves a
    /// self-signed certificate and the probe's rustls client reads it back.
    /// `openssl` is only needed to stand up the test server; the probe itself no
    /// longer calls it. Skipped when `openssl` is missing.
    #[test]
    fn the_tls_probe_reads_a_certificate_over_a_real_handshake() {
        use std::net::TcpListener;

        if Command::new("openssl").arg("version").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("toposight-tls-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let key = dir.join("key.pem");
        let cert = dir.join("cert.pem");

        let generated = Command::new("openssl")
            .args([
                "req",
                "-x509",
                "-newkey",
                "ec",
                "-pkeyopt",
                "ec_paramgen_curve:prime256v1",
                "-nodes",
                "-keyout",
            ])
            .arg(&key)
            .arg("-out")
            .arg(&cert)
            .args(["-subj", "/CN=test-router", "-days", "1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("openssl req");
        assert!(
            generated.success(),
            "the test certificate was not generated"
        );

        // A port that was free a moment ago; s_server takes it immediately.
        let port = {
            let probe = TcpListener::bind("127.0.0.1:0").expect("bind");
            probe.local_addr().unwrap().port()
        };
        let mut server = Command::new("openssl")
            .arg("s_server")
            .arg("-accept")
            .arg(port.to_string())
            .arg("-cert")
            .arg(&cert)
            .arg("-key")
            .arg(&key)
            .args(["-www", "-naccept", "1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("openssl s_server");

        let mut info = ObservationInfo {
            ports: vec![PortInfo {
                port,
                service: Some("https".into()),
                banner: None,
            }],
            ..Default::default()
        };
        let mut warnings = Vec::new();
        for _ in 0..20 {
            enrich_tls(
                &mut info,
                "127.0.0.1".parse().unwrap(),
                Duration::from_millis(700),
                &mut warnings,
            );
            if info.hostname.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        let _ = server.kill();
        let _ = server.wait();
        std::fs::remove_dir_all(&dir).ok();

        assert!(
            info.services.iter().any(|service| service == "tls"),
            "{warnings:?}"
        );
        assert_eq!(info.hostname.as_deref(), Some("test-router"));
        let banner = info.ports[0].banner.clone().expect("banner");
        assert!(banner.contains("test-router"), "{banner}");
        assert!(banner.contains("self-signed"), "{banner}");
    }

    /// The BACnet path over a real socket. Skipped when 47808 is taken.
    #[test]
    fn the_bacnet_probe_reads_an_i_am_over_udp() {
        let Ok(socket) = UdpSocket::bind((Ipv4Addr::from([127, 0, 0, 1]), BACNET_PORT)) else {
            return;
        };
        let response = bacnet_i_am(4321, 8);
        let server = std::thread::spawn(move || {
            let mut buffer = [0u8; 512];
            if let Ok((_, from)) = socket.recv_from(&mut buffer) {
                let _ = socket.send_to(&response, from);
            }
        });

        let mut info = ObservationInfo::default();
        enrich_bacnet(
            &mut info,
            "127.0.0.1".parse().unwrap(),
            Duration::from_millis(800),
        );
        server.join().ok();

        assert!(
            info.services.iter().any(|service| service == "bacnet"),
            "{info:?}"
        );
        let port = info
            .ports
            .iter()
            .find(|entry| entry.port == BACNET_PORT)
            .expect("port entry");
        assert!(
            port.banner
                .as_deref()
                .unwrap_or_default()
                .contains("device 4321"),
            "{:?}",
            port.banner
        );
    }

    #[test]
    fn only_encrypted_looking_ports_get_a_handshake_and_only_sane_names_are_passed_on() {
        assert!(is_tls_service(&Some("https".into())));
        assert!(is_tls_service(&Some("ldaps".into())));
        assert!(!is_tls_service(&Some("http".into())));
        assert!(!is_tls_service(&None));

        assert!(is_dns_name("diskstation.lan"));
        assert!(is_dns_name("_ipp._tcp.local"));
        assert!(!is_dns_name("-verify"), "an argument, not a name");
        assert!(!is_dns_name("has space"));
        assert!(!is_dns_name(""));
        assert!(!is_dns_name("semi;colon"));
    }

    // ------------------------------------------------------- modbus

    /// A 43/14 response with the objects the device declares.
    fn modbus_device_id_response(objects: &[(u8, &str)]) -> Vec<u8> {
        let mut pdu = vec![0x2B, 0x0E, 0x01, 0x01, 0x00, 0x00, objects.len() as u8];
        for (id, text) in objects {
            pdu.push(*id);
            pdu.push(text.len() as u8);
            pdu.extend_from_slice(text.as_bytes());
        }
        let mut packet = vec![0x00, 0x01, 0x00, 0x00];
        packet.extend_from_slice(&((pdu.len() + 1) as u16).to_be_bytes());
        packet.push(0xFF);
        packet.extend_from_slice(&pdu);
        packet
    }

    #[test]
    fn modbus_object_list_becomes_vendor_product_and_version() {
        let response =
            modbus_device_id_response(&[(0x00, "Vendor Oy"), (0x01, "Widget-1"), (0x02, "V1.4.2")]);
        let identity = parse_modbus_identity(&response).expect("identity");
        assert_eq!(identity.vendor.as_deref(), Some("Vendor Oy"));
        assert_eq!(identity.product.as_deref(), Some("Widget-1"));
        assert_eq!(identity.version.as_deref(), Some("V1.4.2"));
        assert_eq!(identity.model().as_deref(), Some("Vendor Oy Widget-1"));
        assert!(identity.describe().contains("version=V1.4.2"));

        assert!(parse_modbus_identity(&[]).is_none());
        assert!(
            parse_modbus_identity(&[0; 8]).is_none(),
            "not a 43/14 answer"
        );
    }

    /// The Modbus path against a socket we control.
    #[test]
    fn the_modbus_probe_reads_identity_over_a_real_socket() {
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let response = modbus_device_id_response(&[(0x00, "Siemens"), (0x01, "S7-1200")]);

        let server = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut request = [0u8; 64];
                let _ = stream.read(&mut request);
                let _ = stream.write_all(&response);
            }
        });

        let received = tcp_exchange(
            "127.0.0.1".parse().unwrap(),
            port,
            &[
                0x00, 0x01, 0x00, 0x00, 0x00, 0x05, 0xFF, 0x2B, 0x0E, 0x01, 0x00,
            ],
            Duration::from_millis(800),
            1024,
        )
        .expect("exchange");
        server.join().ok();
        let identity = parse_modbus_identity(&received).expect("identity");
        assert_eq!(identity.model().as_deref(), Some("Siemens S7-1200"));
    }

    // ------------------------------------------------------- bacnet

    /// An `I-Am`: BVLC, NPDU, then an APDU with instance 1234 and vendor 5.
    fn bacnet_i_am(instance: u32, vendor: u16) -> Vec<u8> {
        let object_id = (8u32 << 22) | (instance & 0x003F_FFFF);
        let mut apdu = vec![0x10, 0x00, 0x68, 0xC4];
        apdu.extend_from_slice(&object_id.to_be_bytes());
        apdu.extend_from_slice(&[0x21, 0x05, 0x91, 0x03]); // max apdu, segmentation
        apdu.push(0x22); // vendor id, unsigned, 2 bytes
        apdu.extend_from_slice(&vendor.to_be_bytes());

        let mut npdu = vec![0x01, 0x00];
        npdu.extend_from_slice(&apdu);

        let mut packet = vec![0x81, 0x0B];
        packet.extend_from_slice(&(npdu.len() as u16).to_be_bytes());
        packet.extend_from_slice(&npdu);
        packet
    }

    #[test]
    fn a_who_is_is_a_directed_unicast_request() {
        assert_eq!(
            bacnet_who_is(),
            [0x81, 0x0B, 0x00, 0x04, 0x01, 0x00, 0x10, 0x08]
        );
    }

    #[test]
    fn the_i_am_carries_the_device_instance_and_vendor() {
        let device = parse_bacnet_i_am(&bacnet_i_am(1234, 5)).expect("device");
        assert_eq!(device.instance, 1234);
        assert_eq!(device.vendor_id, 5);
        assert_eq!(device.describe(), "BACnet device 1234 vendor 5");

        assert!(parse_bacnet_i_am(&[]).is_none());
        assert!(
            parse_bacnet_i_am(&[0x81, 0x0B, 0x00, 0x99, 0x01]).is_none(),
            "bad length"
        );
        assert!(
            parse_bacnet_i_am(&[0x81, 0x0B, 0x00, 0x04, 0x01, 0x00, 0xAB, 0xCD]).is_none(),
            "not an I-Am"
        );
    }

    // ------------------------------------------------------ netbios

    /// A full `NBSTAT` response, built the way a Windows stack writes one:
    /// header, echoed question, answer RR with a compression pointer, RDATA.
    fn nbstat_response(entries: &[(&str, u8, bool)]) -> Vec<u8> {
        let mut packet = vec![0x66, 0x66, 0x84, 0x00, 0x00, 0x01, 0x00, 0x01, 0, 0, 0, 0];
        packet.push(0x20);
        packet.extend_from_slice(b"CK");
        packet.extend_from_slice(&[b'A'; 30]);
        packet.push(0x00);
        packet.extend_from_slice(&[0x00, 0x21, 0x00, 0x01]); // QTYPE, QCLASS

        let mut rdata = vec![entries.len() as u8];
        for (name, suffix, group) in entries {
            let mut field = [b' '; 15];
            for (slot, byte) in field.iter_mut().zip(name.bytes()) {
                *slot = byte;
            }
            rdata.extend_from_slice(&field);
            rdata.push(*suffix);
            rdata.push(if *group { 0x80 } else { 0x00 });
            rdata.push(0x00);
        }

        packet.extend_from_slice(&[0xC0, 0x0C, 0x00, 0x21, 0x00, 0x01]); // name, type, class
        packet.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // TTL
        packet.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        packet.extend_from_slice(&rdata);
        packet
    }

    #[test]
    fn nbstat_names_become_the_hostname_server_and_domain() {
        let response = nbstat_response(&[
            ("DESKTOP-8JT4A", 0x00, false),
            ("SRV2019", 0x20, false),
            ("WORKGROUP", 0x00, true),
            ("__MSBROWSE__", 0x01, true),
        ]);
        let names = parse_nbstat(&response).expect("names");
        assert_eq!(names.workstation.as_deref(), Some("DESKTOP-8JT4A"));
        assert_eq!(names.server.as_deref(), Some("SRV2019"));
        assert_eq!(names.domain.as_deref(), Some("WORKGROUP"));
        assert!(names.describe().contains("name=DESKTOP-8JT4A"));

        assert!(parse_nbstat(&[]).is_none());
        assert!(parse_nbstat(&[0; 12]).is_none(), "no answers");
        assert!(
            parse_nbstat(&nbstat_response(&[("X", 0x1F, false)])).is_none(),
            "a suffix nobody asked for"
        );
    }

    #[test]
    fn the_nbstat_query_asks_for_the_wildcard_name() {
        let query = netbios_nbstat_query();
        assert_eq!(query.len(), 50);
        assert_eq!(&query[12..15], &[0x20, 0x43, 0x4B], "length + `CK`");
        assert_eq!(
            &query[46..],
            &[0x00, 0x21, 0x00, 0x01],
            "QTYPE NBSTAT + QCLASS IN, right after the name terminator"
        );
    }

    fn response(raw: &str) -> HttpResponse {
        parse_http_response(raw.as_bytes())
    }

    #[test]
    fn the_response_parser_takes_the_status_the_headers_and_the_title() {
        let raw = "HTTP/1.0 200 OK\r\nServer: lighttpd/1.4.63\r\n\
                   Set-Cookie: grafana_session=abc; Path=/\r\n\
                   Content-Type: text/html\r\n\r\n<html><head><title>\n  RouterOS  \n</title></head></html>";
        let parsed = response(raw);

        assert_eq!(parsed.status, Some(200));
        assert_eq!(parsed.header("server").as_deref(), Some("lighttpd/1.4.63"));
        assert_eq!(parsed.header("SERVER").as_deref(), Some("lighttpd/1.4.63"));
        assert_eq!(parsed.cookies, vec!["grafana_session".to_string()]);
        assert_eq!(parse_title(&parsed.text).as_deref(), Some("RouterOS"));
        assert!(parsed.text.contains("<title>"), "the body is kept");
    }

    #[test]
    fn the_response_parser_survives_junk() {
        // No blank line at all: all head, no body.
        let bare = response("HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic");
        assert_eq!(bare.status, Some(401));
        assert!(bare.body.is_empty());
        assert_eq!(bare.header("server"), None);
        assert_eq!(bare.header("www-authenticate").as_deref(), Some("Basic"));

        // Not HTTP at all.
        let ssh = response("SSH-2.0-OpenSSH_9.6");
        assert_eq!(ssh.status, None);
        assert!(ssh.headers.is_empty());

        // A title without the closing tag, and case-insensitivity.
        assert_eq!(
            parse_title("<HTML><TITLE>DiskStation</TITLE>").as_deref(),
            Some("DiskStation")
        );
        assert_eq!(parse_title("no title here"), None);
        assert_eq!(parse_title("<title></title>"), None, "empty title");
        assert_eq!(
            response("Server:   ").header("server"),
            None,
            "empty header value"
        );
    }

    #[test]
    fn a_favicon_is_only_hashed_when_it_really_is_one() {
        // A real icon.
        let icon = HttpResponse {
            status: Some(200),
            body: vec![0x00, 0x00, 0x01, 0x00, 0x01, 0x00],
            text: String::new(),
            ..Default::default()
        };
        assert_eq!(
            icon.icon_hash(),
            Some(fingerprint::web::favicon_hash(&icon.body))
        );

        // A 404: the body is an error page, and hashing it would give a
        // confident wrong answer.
        let missing = HttpResponse {
            status: Some(404),
            body: b"<html><body>Not found</body></html>".to_vec(),
            ..Default::default()
        };
        assert_eq!(missing.icon_hash(), None);

        // A 200 that is an HTML page rather than an icon.
        let page = HttpResponse {
            status: Some(200),
            body: b"<!DOCTYPE html><html>welcome</html>".to_vec(),
            ..Default::default()
        };
        assert_eq!(page.icon_hash(), None);

        // Empty body: nothing to hash.
        let empty = HttpResponse {
            status: Some(200),
            ..Default::default()
        };
        assert_eq!(empty.icon_hash(), None);
    }

    #[test]
    fn generic_titles_are_not_models() {
        for title in [
            "Login",
            "login page",
            "  Web UI ",
            "Administration",
            "index",
            "Success",
        ] {
            assert!(is_generic_title(title), "{title:?} should be ignored");
        }
        for title in [
            "RouterOS",
            "HP LaserJet 1020",
            "Synology DiskStation",
            "OpenWrt",
        ] {
            assert!(!is_generic_title(title), "{title:?} is informative");
        }
    }

    #[test]
    fn rule_names_become_service_tokens() {
        assert_eq!(slug("HP Embedded Web Server"), "hp-embedded-web-server");
        assert_eq!(slug("OpenWrt (LuCI)"), "openwrt-luci");
        assert_eq!(slug("FRITZ!Box"), "fritz-box");
        assert_eq!(slug("QNAP QTS"), "qnap-qts");
    }

    #[test]
    fn only_web_looking_ports_are_probed_and_at_most_three() {
        let ports = vec![
            PortInfo {
                port: 22,
                service: Some("ssh".into()),
                banner: None,
            },
            PortInfo {
                port: 80,
                service: Some("http".into()),
                banner: None,
            },
            PortInfo {
                port: 443,
                service: Some("https".into()),
                banner: None,
            },
            PortInfo {
                port: 5000,
                service: Some("http".into()),
                banner: None,
            },
            PortInfo {
                port: 8080,
                service: Some("http".into()),
                banner: None,
            },
            PortInfo {
                port: 9100,
                service: Some("printer".into()),
                banner: None,
            },
        ];
        assert_eq!(web_ports(&ports), vec![80, 443, 5000]);
        assert!(web_ports(&ports[..1]).is_empty());
    }

    #[test]
    fn the_snmp_request_is_well_formed() {
        let request = snmp_get_request("public", &SNMP_OIDS);
        assert_eq!(request[0], 0x30, "outer SEQUENCE");
        assert_eq!(
            request[1] as usize,
            request.len() - 2,
            "the declared length covers the whole body"
        );
        let community = b"\x04\x06public";
        assert!(
            request.windows(community.len()).any(|w| w == community),
            "the community string is carried as an OCTET STRING"
        );
        assert!(request.contains(&0xA0), "GetRequest PDU tag");
        // One packet, three questions.
        for oid in SNMP_OIDS {
            assert!(
                request.windows(oid.len()).any(|w| w == oid),
                "{oid:02X?} is missing from the request"
            );
        }
    }

    /// A response the parser can walk: it searches for each OID, so the envelope
    /// around them does not have to be a faithful GetResponse.
    fn snmp_response(varbinds: &[(&[u8], u8, &[u8])]) -> Vec<u8> {
        let mut out = vec![0x30, 0x00];
        for (oid, tag, value) in varbinds {
            out.extend_from_slice(oid);
            out.push(*tag);
            out.push(value.len() as u8);
            out.extend_from_slice(value);
        }
        out
    }

    #[test]
    fn the_three_snmp_values_are_read_from_one_response() {
        // 1.3.6.1.4.1.8072 (Net-SNMP): the last component needs two base-128 bytes.
        let object_id: &[u8] = &[0x2B, 0x06, 0x01, 0x04, 0x01, 0xBF, 0x08];
        let response = snmp_response(&[
            (&OID_SYS_DESCR, 0x04, b"Cisco IOS C2960-24"),
            (&OID_SYS_OBJECT_ID, 0x06, object_id),
            (&OID_SYS_NAME, 0x04, b"switch-1"),
        ]);

        let identity = parse_snmp_identity(&response);
        assert_eq!(identity.descr.as_deref(), Some("Cisco IOS C2960-24"));
        assert_eq!(identity.name.as_deref(), Some("switch-1"));
        assert_eq!(identity.object_id.as_deref(), Some("1.3.6.1.4.1.8072"));
        assert!(identity.describe().contains("name=switch-1"));

        // Silence, and a response carrying none of the three, both mean nothing.
        assert_eq!(
            parse_snmp_identity(b"nothing useful"),
            SnmpIdentity::default()
        );
        assert_eq!(
            snmp_value(&response, &OID_SYS_DESCR)
                .filter(|(tag, _)| *tag == 0x06)
                .and_then(|(_, value)| decode_oid(value)),
            None,
            "an OCTET STRING is not an OID"
        );
    }

    #[test]
    fn an_oid_with_a_truncated_component_is_rejected() {
        assert_eq!(decode_oid(&[0x2B, 0x06]), Some("1.3.6".to_string()));
        assert_eq!(
            decode_oid(&[0x2B, 0x86]),
            None,
            "continuation bit with no continuation"
        );
        assert_eq!(decode_oid(&[]), None);
    }

    #[test]
    fn a_value_that_is_not_an_octet_string_is_not_a_name() {
        let response = snmp_response(&[(&OID_SYS_DESCR, 0x02, &[0x00])]);
        assert_eq!(parse_snmp_identity(&response).descr, None);
    }

    /// The whole probe path against a socket we control: a tiny HTTP server on
    /// loopback that answers the front page and then the favicon. No network, no
    /// external process — and the only test that runs the rules over a real
    /// response.
    #[test]
    fn the_web_probe_turns_a_front_page_into_a_verdict() {
        use crate::engine::model::DeviceKind;
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();

        // An icon big enough to be split across base64 lines by the hasher.
        let icon: Vec<u8> = (0..120).map(|byte| byte as u8).collect();
        let served = icon.clone();
        let server = std::thread::spawn(move || {
            // One request for the page, one for the favicon.
            for _ in 0..2 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut request = [0u8; 512];
                let _ = stream.read(&mut request);
                let response = if String::from_utf8_lossy(&request).contains("favicon.ico") {
                    let mut raw = b"HTTP/1.0 200 OK\r\nContent-Type: image/x-icon\r\n\r\n".to_vec();
                    raw.extend_from_slice(&served);
                    raw
                } else {
                    b"HTTP/1.0 200 OK\r\nServer: nginx\r\nContent-Type: text/html\r\n\r\n\
                      <html><head><title>HP LaserJet 1020</title></head><body>ok</body></html>"
                        .to_vec()
                };
                let _ = stream.write_all(&response);
            }
        });

        // The port has to look web-ish for the probe to consider it.
        let ports = vec![PortInfo {
            port,
            service: Some("http".into()),
            banner: None,
        }];
        let mut info = ObservationInfo {
            ports,
            ..Default::default()
        };
        let mut warnings = Vec::new();
        enrich_web(
            &mut info,
            "127.0.0.1".parse().unwrap(),
            Duration::from_millis(800),
            &mut warnings,
        );

        server.join().ok();
        assert!(warnings.is_empty(), "{warnings:?}");

        // The device's own title is the model, and the rule adds the make and
        // the kind the title alone could not tell us.
        assert_eq!(info.model.as_deref(), Some("HP LaserJet 1020"));
        assert_eq!(info.vendor.as_deref(), Some("HP"));
        assert_eq!(info.kind, Some(DeviceKind::Printer));
        assert!(
            info.services
                .iter()
                .any(|s| s == "web:hp-embedded-web-server"),
            "{:?}",
            info.services
        );
        assert!(info.services.iter().any(|service| service == "http-ui"));

        let banner = info.ports[0].banner.clone().expect("banner");
        assert!(banner.contains("nginx"), "{banner}");
        assert!(banner.contains("HP Embedded Web Server"), "{banner}");
        assert!(banner.contains("HP LaserJet 1020"), "{banner}");
        assert!(
            banner.contains(&format!(
                "favicon={}",
                fingerprint::web::favicon_hash(&icon)
            )),
            "{banner}"
        );
    }

    #[test]
    fn ports_another_probe_owns_are_left_to_it() {
        let ports = vec![
            PortInfo {
                port: 80,
                service: Some("http".into()),
                banner: None,
            },
            PortInfo {
                port: 443,
                service: Some("https".into()),
                banner: None,
            },
            PortInfo {
                port: 22,
                service: Some("ssh".into()),
                banner: None,
            },
            PortInfo {
                port: 161,
                service: Some("snmp".into()),
                banner: None,
            },
        ];
        let claimed = claimed_ports(&ObservationInfo {
            ports,
            ..Default::default()
        });
        assert!(
            claimed.contains(&80) && claimed.contains(&443),
            "web and TLS"
        );
        assert!(claimed.contains(&161), "SNMP");
        assert!(!claimed.contains(&22), "SSH is the probe table's business");
    }

    /// The probe table against a socket we control: an SSH banner on a port
    /// nothing else claims, so the engine has to name it.
    #[test]
    fn the_service_probe_table_reads_a_banner() {
        use std::net::TcpListener;
        use std::sync::atomic::{AtomicBool, Ordering};

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).expect("non-blocking");

        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let server = std::thread::spawn(move || {
            while !flag.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // SSH greets first, like the real thing.
                        let _ = stream.write_all(b"SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.5\r\n");
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(2)),
                }
            }
        });

        let mut info = ObservationInfo {
            ports: vec![PortInfo {
                port,
                service: None,
                banner: None,
            }],
            ..Default::default()
        };
        enrich_tcp(
            &mut info,
            &Engine::embedded(),
            "127.0.0.1".parse().unwrap(),
            Duration::from_millis(400),
        );
        stop.store(true, Ordering::Relaxed);
        server.join().ok();

        assert_eq!(info.ports[0].service.as_deref(), Some("ssh"));
        let banner = info.ports[0].banner.clone().expect("banner");
        assert!(banner.starts_with("OpenSSH 9.6p1"), "{banner}");
        assert!(
            info.services.iter().any(|service| service == "tcp:ssh"),
            "{:?}",
            info.services
        );
        assert!(
            info.services.iter().any(|service| service == "tcp:openssh"),
            "{:?}",
            info.services
        );
    }

    #[test]
    fn the_probe_leaves_observations_from_its_own_source() {
        let probe = ProbeDiscovery::new(
            vec![ProbeTarget {
                ip: "127.0.0.1".parse().unwrap(),
                mac: Some("aa:bb:cc:00:00:01".parse().unwrap()),
                iface: Some("lo".into()),
            }],
            Identifier::passive(false),
            Duration::from_millis(200),
            Arc::new(Engine::embedded()),
        );
        assert_eq!(probe.source(), DiscoverySource::Probe);
        assert!(
            probe.describe().contains("reverse DNS only"),
            "{}",
            probe.describe()
        );

        let outcome = probe.scan().expect("a passive probe cannot fail");
        assert_eq!(outcome.observations.len(), 1);
        assert_eq!(outcome.observations[0].source, DiscoverySource::Probe);
        assert_eq!(outcome.observations[0].ip.to_string(), "127.0.0.1");
    }
}
