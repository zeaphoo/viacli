//! Device identification: what *is* this thing?
//!
//! Three independent signals are combined, in decreasing order of reliability:
//!
//! 1. **Open ports** (opt-in). An open 9100 or 631 is a printer; 62078 is an
//!    iPhone; 5000+5001 is a Synology NAS. Slow but very specific.
//! 2. **Vendor** from the IEEE OUI registry. Good for "this is an Espressif
//!    chip" (→ IoT) or "this is VMware" (→ virtual machine), weak for vendors
//!    that span every category (Apple, Samsung, Huawei).
//! 3. **Hostname** from reverse DNS. Often the most human-meaningful signal
//!    (`router.lan`, `iphone-von-anna`), but only when the network publishes it.
//!
//! Classification is intentionally conservative: when the signals disagree the
//! result stays [`DeviceKind::Unknown`] rather than guessing.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

use crate::engine::model::{DeviceKind, MacAddr, ObservationInfo, PortInfo};

/// Ports probed by default. Chosen for *classification value*, not coverage.
pub const DEFAULT_PROBE_PORTS: [u16; 23] = [
    22,    // ssh          -> server / computer
    23,    // telnet       -> network gear / iot
    53,    // dns          -> router
    80,    // http         -> web ui on almost anything
    139,   // netbios      -> windows
    443,   // https        -> web ui
    445,   // smb          -> windows / nas
    515,   // lpd          -> printer
    548,   // afp          -> apple nas
    554,   // rtsp         -> camera
    631,   // ipp          -> printer
    1883,  // mqtt         -> iot
    2049,  // nfs          -> nas / server
    3389,  // rdp          -> windows
    5000,  // upnp/synology
    5001,  // synology https
    5900,  // vnc          -> computer
    8080,  // http-alt
    8443,  // https-alt
    9100,  // raw printing -> printer
    32400, // plex         -> server
    62078, // iphone sync  -> phone
    161,   // snmp (udp technically, but many stacks answer on tcp too)
];

/// TCP connect probe used to refine classification.
#[derive(Debug, Clone)]
pub struct PortProbe {
    pub ports: Vec<u16>,
    pub timeout: Duration,
    pub concurrency: usize,
}

impl Default for PortProbe {
    fn default() -> Self {
        PortProbe {
            ports: DEFAULT_PROBE_PORTS.to_vec(),
            timeout: Duration::from_millis(250),
            concurrency: crate::engine::connect::DEFAULT_CONCURRENCY,
        }
    }
}

impl PortProbe {
    pub fn new(ports: Vec<u16>) -> Self {
        PortProbe {
            ports,
            ..Default::default()
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency.max(1);
        self
    }

    /// Probe every configured port: connect, grab what the service says, and
    /// name it.
    ///
    /// The connect half runs asynchronously with `concurrency` connections in
    /// flight, so a full list against one host costs about one timeout rather
    /// than one timeout per port. Only the ports that answered are reconnected
    /// to, for the banner. Nothing here is authoritative: a service name is a
    /// guess from the banner or the port.
    pub fn probe(&self, ip: Ipv4Addr) -> Vec<PortInfo> {
        let targets: Vec<(Ipv4Addr, u16)> =
            self.ports.iter().map(|port| (ip, *port)).collect();
        let mut connected =
            crate::engine::connect::connect_and_read(&targets, self.timeout, self.concurrency);
        connected.sort_by_key(|found| found.port);

        connected
            .into_iter()
            .map(|found| {
                let mut banner = found.banner;
                // A web server waits for a request; ask for one on a fresh
                // connection, which is harmless because it accepts many.
                if banner.is_empty() && is_http_port(found.port) {
                    banner = http_banner(ip, found.port, self.timeout);
                }
                let text = String::from_utf8_lossy(&banner).into_owned();
                PortInfo {
                    port: found.port,
                    service: guess_service(found.port, &text),
                    banner: clean_banner(&text),
                }
            })
            .collect()
    }
}

/// Everything identification could work out about one device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identification {
    /// Reverse-DNS name, when one is asked for and one exists.
    pub hostname: Option<String>,
    pub open_ports: Vec<PortInfo>,
}

/// Drives the identification passes.
#[derive(Debug, Clone, Default)]
pub struct Identifier {
    /// Resolve PTR records. Costs a DNS round trip per host.
    pub resolve_dns: bool,
    /// Optional port probe. `None` disables it entirely.
    pub probe: Option<PortProbe>,
}

impl Identifier {
    /// Cheap identification: vendor + hostname + heuristics, no network I/O
    /// beyond an optional reverse lookup.
    pub fn passive(resolve_dns: bool) -> Self {
        Identifier {
            resolve_dns,
            probe: None,
        }
    }

    /// Identification including a TCP port probe.
    pub fn with_probe(resolve_dns: bool, probe: PortProbe) -> Self {
        Identifier {
            resolve_dns,
            probe: Some(probe),
        }
    }

    /// Resolve the name and, when configured, probe the ports.
    ///
    /// Deliberately does *not* judge the kind or look up the vendor: the
    /// inventory does that once, from everything it holds — a vendor needs only
    /// the MAC, and a kind needs the vendor. Split this way, a device's vendor
    /// no longer depends on this pass running at all.
    pub fn identify(&self, ip: Ipv4Addr) -> Identification {
        let hostname = if self.resolve_dns {
            reverse_dns(ip)
        } else {
            None
        };
        let open_ports = match self.probe.as_ref() {
            Some(probe) => probe.probe(ip),
            None => Vec::new(),
        };

        Identification {
            hostname,
            open_ports,
        }
    }
}

/// Reverse DNS, returning `None` for missing or unhelpful answers.
///
/// Many stacks answer a PTR query with the literal IP address; that is treated
/// as "no hostname".
pub fn reverse_dns(ip: Ipv4Addr) -> Option<String> {
    let addr = IpAddr::V4(ip);
    let name = dns_lookup::lookup_addr(&addr).ok()?;
    let name = name.trim_end_matches('.').trim();
    if name.is_empty() || name == ip.to_string() {
        return None;
    }
    Some(name.to_string())
}

/// The longest banner kept: enough to name a service, short enough to store.
const BANNER_LIMIT: usize = 200;

/// Ask a web server for its front page and keep the response. Only the ports
/// where a plain-text `GET /` is worth trying are sent here.
fn http_banner(ip: Ipv4Addr, port: u16, timeout: Duration) -> Vec<u8> {
    let addr = SocketAddr::from((ip, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, timeout) else {
        return Vec::new();
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let request = format!("GET / HTTP/1.0\r\nHost: {ip}\r\nUser-Agent: toposight\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return Vec::new();
    }

    let mut buf = [0u8; 512];
    match stream.read(&mut buf) {
        Ok(n) if n > 0 => buf[..n].to_vec(),
        _ => Vec::new(),
    }
}

/// Keep the first line, drop control characters, and bound the length.
fn clean_banner(banner: &str) -> Option<String> {
    let line = banner
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let cleaned: String = line
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return None;
    }
    Some(cleaned.chars().take(BANNER_LIMIT).collect())
}

/// Ports where a plain-text `GET /` is worth trying.
fn is_http_port(port: u16) -> bool {
    matches!(
        port,
        80 | 81
            | 88
            | 4430
            | 5000
            | 8000
            | 8008
            | 8080
            | 8081
            | 8088
            | 8090
            | 8888
            | 9000
            | 9200
            | 9080
    )
}

/// Name a service from its banner, falling back to the port.
fn guess_service(port: u16, banner: &str) -> Option<String> {
    let text = banner.to_ascii_lowercase();

    let from_banner = if banner.starts_with("SSH-") {
        Some("ssh")
    } else if text.contains("http/") {
        Some("http")
    } else if banner.starts_with("220") {
        if text.contains("smtp") || text.contains("mail") || text.contains("esmtp") {
            Some("smtp")
        } else {
            Some("ftp")
        }
    } else if banner.starts_with("+OK") {
        Some("pop3")
    } else if text.starts_with("* ok") {
        Some("imap")
    } else if banner.starts_with("RFB ") {
        Some("vnc")
    } else if text.contains("redis") || banner.starts_with("-NOAUTH") || text.starts_with("-err") {
        Some("redis")
    } else if text.contains("mysql") || text.contains("mariadb") {
        Some("mysql")
    } else if text.contains("postgres") {
        Some("postgresql")
    } else if text.contains("mongodb") || text.contains("mongo") {
        Some("mongodb")
    } else if text.contains("rtsp") {
        Some("rtsp")
    } else if text.contains("telnet") {
        Some("telnet")
    } else if text.contains("smb") {
        Some("smb")
    } else {
        None
    };

    from_banner
        .map(str::to_string)
        .or_else(|| port_hint(port).map(str::to_string))
}

/// The service a well-known port usually means.
fn port_hint(port: u16) -> Option<&'static str> {
    Some(match port {
        21 => "ftp",
        22 => "ssh",
        23 => "telnet",
        25 => "smtp",
        53 => "dns",
        80 | 81 | 88 | 8000 | 8008 | 8080 | 8081 | 8088 | 8090 | 8888 | 9080 => "http",
        110 => "pop3",
        139 => "netbios",
        143 => "imap",
        161 => "snmp",
        389 => "ldap",
        443 | 4430 | 8443 | 9443 => "https",
        445 => "smb",
        515 => "lpd",
        548 => "afp",
        554 => "rtsp",
        631 => "ipp",
        1433 => "mssql",
        1521 => "oracle",
        1723 => "pptp",
        1883 => "mqtt",
        2049 => "nfs",
        2375 => "docker",
        3306 => "mysql",
        3389 => "rdp",
        5000 | 5001 => "upnp",
        5432 => "postgresql",
        5900 | 5901 => "vnc",
        6379 => "redis",
        9100 => "printer",
        9200 => "elasticsearch",
        11211 => "memcached",
        27017 | 27018 => "mongodb",
        32400 => "plex",
        62078 => "iphone-sync",
        _ => return None,
    })
}

/// Ordered rules for turning evidence into a [`DeviceKind`].
///
/// The order encodes confidence: port evidence beats vendor strings, which beat
/// hostname guesses. Virtualisation is checked before generic vendor rules
/// because "VMware" must not become "computer".
pub fn classify(
    mac: Option<&MacAddr>,
    vendor: Option<&str>,
    hostname: Option<&str>,
    iface: Option<&str>,
    open_ports: &[u16],
) -> DeviceKind {
    let vendor_l = vendor.unwrap_or("").to_ascii_lowercase();
    let host_l = hostname.unwrap_or("").to_ascii_lowercase();
    let iface_l = iface.unwrap_or("").to_ascii_lowercase();

    let has = |port: u16| open_ports.contains(&port);
    let vendor_has = |needles: &[&str]| needles.iter().any(|n| vendor_l.contains(n));

    // ---- 1. Port evidence -------------------------------------------------
    if has(9100) || has(631) || has(515) {
        return DeviceKind::Printer;
    }
    if has(62078) {
        return DeviceKind::Phone;
    }
    if has(5000) && has(5001) {
        return DeviceKind::Nas;
    }
    if has(3389) {
        return DeviceKind::Computer;
    }
    if has(1883) {
        return DeviceKind::Iot;
    }
    if has(554) {
        return DeviceKind::Camera;
    }
    if has(2049) {
        return DeviceKind::Nas;
    }

    // ---- 2. Virtualisation -------------------------------------------------
    if vendor_has(&[
        "vmware",
        "virtualbox",
        "innotek",
        "qemu",
        "parallels",
        "xen",
    ]) {
        return DeviceKind::VirtualMachine;
    }

    // ---- 3. Locally administered MACs on virtual bridges ------------------
    // Docker/libvirt bridges hand out addresses like 06:fb:4d:... which are
    // locally administered and therefore absent from the OUI registry.
    if mac.is_some_and(|mac| mac.is_locally_administered())
        && (iface_l.starts_with("br-")
            || iface_l.starts_with("docker")
            || iface_l.starts_with("veth")
            || iface_l.starts_with("virbr")
            || iface_l.starts_with("vnet"))
    {
        return DeviceKind::Container;
    }

    // ---- 4. Vendor families ------------------------------------------------
    if vendor_has(&[
        "espressif",
        "tuya",
        "itead",
        "shelly",
        "allterco",
        "sonoff",
        "nordic semiconductor",
        "particle industries",
        "shenzhen bilian",
    ]) {
        return DeviceKind::Iot;
    }
    if vendor_has(&[
        "hikvision",
        "dahua",
        "axis communications",
        "reolink",
        "foscam",
        "amcrest",
        "uniview",
        "arlo",
    ]) {
        return DeviceKind::Camera;
    }
    if vendor_has(&[
        "synology",
        "qnap",
        "buffalo",
        "drobo",
        "western digital",
        "seagate",
        "terra master",
    ]) {
        return DeviceKind::Nas;
    }
    if vendor_has(&[
        "hewlett",
        "brother industries",
        "canon",
        "seiko epson",
        "epson",
        "xerox",
        "ricoh",
        "lexmark",
        "kyocera",
    ]) {
        return DeviceKind::Printer;
    }
    if vendor_has(&["raspberry pi", "raspberr"]) {
        return DeviceKind::Computer;
    }
    if vendor_has(&["nintendo", "sony interactive", "playstation"]) {
        return DeviceKind::GameConsole;
    }
    if vendor_has(&["roku", "vizio", "sonos", "hisense", "tcl technology"]) {
        return DeviceKind::Tv;
    }
    if vendor_has(&[
        "cisco",
        "mikrotik",
        "routerboard",
        "ubiquiti",
        "netgear",
        "tp-link",
        "d-link",
        "aruba",
        "ruckus",
        "juniper",
        "fortinet",
        "netgate",
        "sophos",
    ]) {
        return DeviceKind::Router;
    }

    // ---- 5. Hostname hints -------------------------------------------------
    if let Some(kind) = kind_from_hostname(&host_l) {
        return kind;
    }

    // ---- 6. Broad vendor buckets (weakest) --------------------------------
    // Only vendors that are genuinely dominated by one product category belong
    // here. Apple and Samsung are deliberately *absent*: they ship phones,
    // laptops, tablets and TVs under the same handful of OUIs, so guessing
    // would be wrong more often than right. Those stay Unknown.
    if vendor_has(&[
        "xiaomi",
        "oppo",
        "vivo mobile",
        "oneplus",
        "motorola",
        "huawei device",
        "bbk electronics",
    ]) {
        return DeviceKind::Phone;
    }
    if vendor_has(&[
        "intel corporate",
        "asustek",
        "micro-star",
        "gigabyte",
        "dell",
        "lenovo",
        "hp inc",
        "framework computer",
        "supermicro",
    ]) {
        return DeviceKind::Computer;
    }

    // SMB/NetBIOS is the last resort: a file share is a computer *or* a NAS, so
    // it only decides once no vendor or hostname already did.
    if has(445) || has(139) {
        return DeviceKind::Computer;
    }

    DeviceKind::Unknown
}

/// Classify from mDNS/SSDP-announced services and model strings alone.
///
/// Conservative, exactly like the rest of [`classify`]: only strings that are
/// effectively unique to one product category map to a [`DeviceKind`]. Apple
/// models get the granular treatment (iPhone → phone, Apple TV → tv) because
/// those model strings are unambiguous.
pub fn kind_from_hints(hints: Option<&ObservationInfo>) -> Option<DeviceKind> {
    let hints = hints?;
    let services = hints.normalized_services();
    let model = hints.model.as_deref().map(|m| m.to_ascii_lowercase());
    let model_has = |needles: &[&str]| {
        model
            .as_deref()
            .is_some_and(|m| needles.iter().any(|n| m.contains(n)))
    };
    let service_has = |needles: &[&str]| {
        needles
            .iter()
            .any(|needle| services.iter().any(|s| s.contains(needle)))
    };

    // Evidence is weighed in order of how precisely it identifies the device.
    // 1. A model identifier: `model=iPhone14,2` says exactly what it is, where
    //    no keyword in a hostname ever will.
    if let Some(kind) = hints
        .model
        .as_deref()
        .and_then(crate::engine::fingerprint::mobile::apple_model_kind)
    {
        return Some(kind);
    }
    // 2. What it announces: `_companion-link` is iOS-only, `_googlecast` is a
    //    television, `_ipp` is a printer.
    if let Some(kind) = services
        .iter()
        .find_map(|service| crate::engine::fingerprint::mobile::service_kind(service))
    {
        return Some(kind);
    }
    // 3. A vendor model string from UPnP, Chromecast's `md=` or a leaky
    //    hostname.
    if let Some(kind) = hints
        .model
        .as_deref()
        .and_then(crate::engine::fingerprint::mobile::model_kind)
    {
        return Some(kind);
    }

    if service_has(&["_apple-mobdev2", "_apple-mobdev"]) {
        return Some(DeviceKind::Phone);
    }
    if model_has(&["appletv", "chromecast", "fire tv", "firetv"]) {
        return Some(DeviceKind::Tv);
    }
    if model_has(&[
        "macbook", "imac", "mac mini", "macmini", "mac pro", "macpro",
    ]) {
        return Some(DeviceKind::Computer);
    }
    if service_has(&["_airplay", "_airtunes", "_googlecast", "_mediaremotetv"]) {
        return Some(DeviceKind::Tv);
    }
    // `_raop` used to be here, which filed every Apple speaker and Apple TV as
    // a router. Remote Audio Output is a speaker; only the AirPort base station
    // is a router.
    if service_has(&["_airport"]) {
        return Some(DeviceKind::Router);
    }
    if service_has(&[
        "internetgatewaydevice",
        "wanipconnection",
        "wandevice",
        "wancommoninterfaceconfig",
    ]) {
        return Some(DeviceKind::Router);
    }
    if service_has(&["mediarenderer", "mediaserver"]) {
        return Some(DeviceKind::Tv);
    }
    if service_has(&["_ipp", "_ipps", "_printer", "pdl-datastream", "printbasic"]) {
        return Some(DeviceKind::Printer);
    }
    if service_has(&["_sonos", "_spotify-connect", "_googlezone"]) {
        return Some(DeviceKind::Iot);
    }
    if model_has(&["esp32", "esp8266", "espressif", "tasmota", "shelly"]) {
        return Some(DeviceKind::Iot);
    }
    if service_has(&["_hap"]) && model_has(&["homepod"]) {
        return Some(DeviceKind::Iot);
    }

    None
}

/// Classify from a hostname alone (`router.lan`, `annas-iphone`).
///
/// Shared by the classic pipeline (reverse DNS) and by the mDNS/SSDP paths,
/// which learn a hostname without a PTR round trip.
pub fn kind_from_hostname(name: &str) -> Option<DeviceKind> {
    let host = name.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| host.contains(n));

    if has(&[
        "router",
        "gateway",
        "openwrt",
        "routeros",
        "pfsense",
        "opnsense",
        "edgerouter",
        "unifi",
        "fritz.box",
        "speedport",
    ]) {
        return Some(DeviceKind::Router);
    }
    if has(&["access-point", "accesspoint", "ap-", "-ap", "wlan-ap"]) {
        return Some(DeviceKind::AccessPoint);
    }
    // Tablets first: an iPad is not a phone, and "Galaxy Tab" must not be read
    // as a "Galaxy".
    if has(&[
        "ipad",
        "galaxytab",
        "galaxy-tab",
        "matepad",
        "mediapad",
        "lenovo tab",
        "tablet",
    ]) {
        return Some(DeviceKind::Tablet);
    }
    if has(&[
        "iphone", "ipod", "android", "galaxy", "pixel", "redmi", "poco", "oneplus", "realme",
        "oppo", "vivo", "xperia", "honor", "harmony", "phone",
    ]) {
        return Some(DeviceKind::Phone);
    }
    if has(&[
        "nas",
        "diskstation",
        "synology",
        "qnap",
        "truenas",
        "freenas",
    ]) {
        return Some(DeviceKind::Nas);
    }
    if has(&["printer", "prn", "mfp", "scanner"]) {
        return Some(DeviceKind::Printer);
    }
    if has(&["cam", "ipcam", "nvr", "doorbell"]) {
        return Some(DeviceKind::Camera);
    }
    if has(&["tv", "appletv", "chromecast", "firetv", "roku", "bravia"]) {
        return Some(DeviceKind::Tv);
    }
    if has(&["esphome", "tasmota", "shelly", "sonoff", "esp32", "esp8266"]) {
        return Some(DeviceKind::Iot);
    }
    if has(&[
        "macbook",
        "imac",
        "mac-mini",
        "macmini",
        "laptop",
        "desktop",
        "notebook",
        "workstation",
    ]) {
        return Some(DeviceKind::Computer);
    }

    None
}

/// Default ports worth trying when the operator wants a broader fingerprint.
pub fn common_service_ports() -> Vec<u16> {
    DEFAULT_PROBE_PORTS.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mac(s: &str) -> MacAddr {
        s.parse().unwrap()
    }

    #[test]
    fn port_evidence_beats_everything_else() {
        // An Apple vendor string would otherwise classify as a phone.
        assert_eq!(
            classify(
                Some(&mac("00:11:22:33:44:55")),
                Some("Apple, Inc."),
                None,
                None,
                &[9100]
            ),
            DeviceKind::Printer
        );
        assert_eq!(
            classify(Some(&mac("00:11:22:33:44:55")), None, None, None, &[62078]),
            DeviceKind::Phone
        );
        assert_eq!(
            classify(
                Some(&mac("00:11:22:33:44:55")),
                None,
                None,
                None,
                &[5000, 5001]
            ),
            DeviceKind::Nas
        );
    }

    #[test]
    fn service_ports_fill_in_device_type() {
        let mac = mac("aa:bb:cc:dd:ee:ff");
        assert_eq!(
            classify(Some(&mac), None, None, None, &[554]),
            DeviceKind::Camera
        );
        assert_eq!(
            classify(Some(&mac), None, None, None, &[2049]),
            DeviceKind::Nas
        );
        assert_eq!(
            classify(Some(&mac), None, None, None, &[445]),
            DeviceKind::Computer
        );
        // A NAS vendor still beats the SMB fallback.
        assert_eq!(
            classify(
                Some(&mac),
                Some("Synology Incorporated"),
                None,
                None,
                &[445]
            ),
            DeviceKind::Nas
        );
    }

    #[test]
    fn virtualisation_is_detected_before_generic_vendors() {
        assert_eq!(
            classify(
                Some(&mac("00:0c:29:aa:bb:cc")),
                Some("VMware, Inc."),
                None,
                None,
                &[]
            ),
            DeviceKind::VirtualMachine
        );
        assert_eq!(
            classify(
                Some(&mac("52:54:00:aa:bb:cc")),
                Some("QEMU"),
                None,
                None,
                &[]
            ),
            DeviceKind::VirtualMachine
        );
    }

    #[test]
    fn locally_administered_bridge_addresses_are_containers() {
        assert_eq!(
            classify(
                Some(&mac("06:fb:4d:6d:5d:81")),
                None,
                None,
                Some("br-9795562469ff"),
                &[]
            ),
            DeviceKind::Container
        );
        // Same MAC on a physical interface is not a container.
        assert_eq!(
            classify(
                Some(&mac("06:fb:4d:6d:5d:81")),
                None,
                None,
                Some("enp5s0"),
                &[]
            ),
            DeviceKind::Unknown
        );
    }

    #[test]
    fn vendor_families_classify() {
        assert_eq!(
            classify(
                Some(&mac("84:f3:eb:00:00:01")),
                Some("Espressif Inc."),
                None,
                None,
                &[]
            ),
            DeviceKind::Iot
        );
        assert_eq!(
            classify(
                Some(&mac("2c:a0:42:30:3c:7e")),
                Some("Huawei Device Co., Ltd."),
                None,
                None,
                &[]
            ),
            DeviceKind::Phone
        );
        assert_eq!(
            classify(
                Some(&mac("44:03:2c:af:69:0f")),
                Some("Intel Corporate"),
                None,
                None,
                &[]
            ),
            DeviceKind::Computer
        );
        assert_eq!(
            classify(
                Some(&mac("00:11:32:aa:bb:cc")),
                Some("Synology Incorporated"),
                None,
                None,
                &[]
            ),
            DeviceKind::Nas
        );
    }

    #[test]
    fn hostname_hints_are_used_when_vendor_is_silent() {
        assert_eq!(
            classify(
                Some(&mac("aa:bb:cc:dd:ee:ff")),
                None,
                Some("router.lan"),
                None,
                &[]
            ),
            DeviceKind::Router
        );
        assert_eq!(
            classify(
                Some(&mac("aa:bb:cc:dd:ee:ff")),
                None,
                Some("annas-iphone"),
                None,
                &[]
            ),
            DeviceKind::Phone
        );
        assert_eq!(
            classify(
                Some(&mac("aa:bb:cc:dd:ee:ff")),
                None,
                Some("esphome-bathroom"),
                None,
                &[]
            ),
            DeviceKind::Iot
        );
    }

    #[test]
    fn unknown_stays_unknown() {
        assert_eq!(
            classify(
                Some(&mac("aa:bb:cc:dd:ee:ff")),
                None,
                None,
                Some("enp5s0"),
                &[]
            ),
            DeviceKind::Unknown
        );
        // An unregistered vendor string with no other evidence.
        assert_eq!(
            classify(
                Some(&mac("aa:bb:cc:dd:ee:ff")),
                Some("Some Obscure Ltd"),
                None,
                None,
                &[]
            ),
            DeviceKind::Unknown
        );
    }

    #[test]
    fn category_spanning_vendors_are_not_guessed() {
        // Apple and Samsung cover phones, laptops, tablets and TVs; they must
        // stay Unknown rather than being reported as phones.
        for vendor in ["Apple, Inc.", "Samsung Electronics Co.,Ltd"] {
            assert_eq!(
                classify(
                    Some(&mac("aa:bb:cc:dd:ee:ff")),
                    Some(vendor),
                    None,
                    None,
                    &[]
                ),
                DeviceKind::Unknown,
                "{vendor} should not be guessed"
            );
        }
        // A hostname hint still resolves them.
        assert_eq!(
            classify(
                Some(&mac("aa:bb:cc:dd:ee:ff")),
                Some("Apple, Inc."),
                Some("annas-iphone"),
                None,
                &[]
            ),
            DeviceKind::Phone
        );
    }

    #[test]
    fn probe_finds_an_open_listener() {
        // Bind an ephemeral port, then confirm the probe reports it and not the
        // neighbouring closed port.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let probe = PortProbe::new(vec![port]).with_timeout(Duration::from_millis(500));
        let open = probe.probe(Ipv4Addr::LOCALHOST);
        assert_eq!(
            open.iter().map(|info| info.port).collect::<Vec<_>>(),
            vec![port]
        );
    }

    #[test]
    fn probe_captures_a_banner_and_names_the_service() {
        use std::io::Write;

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        // A service that speaks first, like SSH.
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = stream.write_all(b"SSH-2.0-OpenSSH_9.0p1\r\n");
            }
        });

        let probe = PortProbe::new(vec![port]).with_timeout(Duration::from_millis(500));
        let open = probe.probe(Ipv4Addr::LOCALHOST);

        assert_eq!(open.len(), 1);
        assert_eq!(open[0].port, port);
        assert_eq!(open[0].service.as_deref(), Some("ssh"));
        assert_eq!(open[0].banner.as_deref(), Some("SSH-2.0-OpenSSH_9.0p1"));
    }

    #[test]
    fn services_are_named_from_banners_ports_and_http() {
        assert_eq!(guess_service(22, "SSH-2.0-x").as_deref(), Some("ssh"));
        assert_eq!(
            guess_service(25, "220 mail.example ESMTP").as_deref(),
            Some("smtp")
        );
        assert_eq!(guess_service(21, "220 ProFTPD").as_deref(), Some("ftp"));
        assert_eq!(
            guess_service(8080, "HTTP/1.1 200 OK").as_deref(),
            Some("http")
        );
        assert_eq!(guess_service(5900, "RFB 003.008").as_deref(), Some("vnc"));
        // Nothing in the banner: fall back to the port.
        assert_eq!(guess_service(6379, "").as_deref(), Some("redis"));
        assert_eq!(guess_service(65500, ""), None);
    }
}
