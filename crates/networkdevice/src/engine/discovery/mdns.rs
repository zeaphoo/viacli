//! mDNS / DNS-SD browsing (RFC 6762 / RFC 6761).
//!
//! This is the source that makes *mobile* devices legible. A phone, a TV or a
//! speaker announces itself over multicast DNS with a hostname, a model string
//! and a list of services (`_airplay._tcp`, `_googlecast._tcp`, ...). ARP can
//! only ever say "Huawei device"; mDNS says "AppleTV6,1".
//!
//! The browse is a bounded, poll-style exchange, so it fits the [`Discovery`]
//! trait exactly like the ARP sources:
//!
//! 1. `_services._dns.local.` PTR — which service types this network offers.
//! 2. One PTR per service type — the instances (`Living Room._airplay._tcp.local`).
//! 3. SRV + TXT per instance — the target host and the `model=`/`md=` strings.
//! 4. A per target host — the IPv4 address.
//!
//! Rounds 2–4 are also run against a built-in list of common service types, so
//! devices are found even when nobody answers the top-level enumeration.
//!
//! Queries are sent with the unicast-response bit set (`QU`), so answers come
//! back to this process's ephemeral port instead of the well-known 5353. That
//! avoids fighting the host's own avahi/mDNSResponder over the port. The
//! trade-off is that a responder which ignores `QU` and multicasts its answer
//! will not be seen; each protocol also has the neighbour-table baseline, so a
//! missed answer costs enrichment, not a device.

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::engine::discovery::arpjoin::ArpJoin;
use crate::engine::discovery::dns::{self, Answer, AnswerData};
use crate::engine::discovery::{Discovery, ScanOutcome};
use crate::engine::error::Result;
use crate::engine::model::{DiscoverySource, ObservationInfo};

/// Well-known mDNS group and port.
const MDNS_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const MDNS_PORT: u16 = 5353;
const SERVICES: &str = "_services._dns.local";

/// Service types worth asking for even when `_services._dns.local` stays quiet.
/// Chosen for device legibility: phones, TVs, speakers, printers.
const DEFAULT_SERVICE_TYPES: [&str; 10] = [
    "_apple-mobdev2._tcp.local",
    "_airplay._tcp.local",
    "_raop._tcp.local",
    "_googlecast._tcp.local",
    "_mediaremotetv._tcp.local",
    "_spotify-connect._tcp.local",
    "_ipp._tcp.local",
    "_printer._tcp.local",
    "_hap._tcp.local",
    "_companion-link._tcp.local",
];

const MAX_SERVICES: usize = 32;
const MAX_INSTANCES: usize = 128;
const RECV_BUF: usize = 4096;

/// One host as assembled from the browse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdnsHost {
    pub ip: Ipv4Addr,
    /// Local interface the answer arrived on.
    pub iface: Option<String>,
    pub hostname: Option<String>,
    pub model: Option<String>,
    pub services: Vec<String>,
}

/// mDNS / DNS-SD discovery source.
#[derive(Debug, Clone)]
pub struct MdnsDiscovery {
    ifaces: Vec<String>,
    /// How long to listen after each round of questions.
    listen: Duration,
}

impl Default for MdnsDiscovery {
    fn default() -> Self {
        MdnsDiscovery {
            ifaces: Vec::new(),
            listen: Duration::from_millis(500),
        }
    }
}

impl MdnsDiscovery {
    pub fn new(ifaces: Vec<String>) -> Self {
        MdnsDiscovery {
            ifaces,
            ..Default::default()
        }
    }

    pub fn with_listen(mut self, listen: Duration) -> Self {
        self.listen = listen;
        self
    }

    /// Interfaces to browse: the configured ones, else the default-route one.
    fn interfaces(&self) -> Vec<(String, Ipv4Addr)> {
        let Ok(subnets) = crate::engine::netinfo::scan_subnets(&self.ifaces) else {
            return Vec::new();
        };
        subnets.into_iter().map(|s| (s.iface, s.addr)).collect()
    }
}

impl Discovery for MdnsDiscovery {
    fn source(&self) -> DiscoverySource {
        DiscoverySource::Mdns
    }

    fn describe(&self) -> String {
        format!(
            "mDNS/DNS-SD browse ({}ms listen per round, {} service types)",
            self.listen.as_millis(),
            DEFAULT_SERVICE_TYPES.len()
        )
    }

    fn scan(&self) -> Result<ScanOutcome> {
        let interfaces = self.interfaces();
        if interfaces.is_empty() {
            return Ok(ScanOutcome::warning(
                Vec::new(),
                "mDNS: no usable IPv4 interface",
            ));
        }

        let join = ArpJoin::from_proc();
        let mut warnings = Vec::new();
        let mut hosts: BTreeMap<Ipv4Addr, MdnsHost> = BTreeMap::new();

        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for (iface, addr) in &interfaces {
                let listen = self.listen;
                handles.push((
                    iface.clone(),
                    *addr,
                    scope.spawn(move || browse_iface(addr, listen)),
                ));
            }
            for (iface, _addr, handle) in handles {
                match handle.join() {
                    Ok(Ok(found)) => {
                        for mut host in found {
                            host.iface = Some(iface.clone());
                            let ip = host.ip;
                            let slot = hosts.entry(ip).or_insert_with(|| MdnsHost {
                                ip,
                                iface: None,
                                hostname: None,
                                model: None,
                                services: Vec::new(),
                            });
                            merge_host(slot, host);
                        }
                    }
                    Ok(Err(e)) => warnings.push(format!("mDNS on {iface}: {e}")),
                    Err(_) => warnings.push(format!("mDNS on {iface}: browse thread panicked")),
                }
            }
        });

        let observations = hosts
            .into_values()
            .map(|host| {
                let info = ObservationInfo {
                    hostname: host.hostname,
                    model: host.model,
                    services: host.services,
                    ..Default::default()
                };
                join.observation(
                    host.ip,
                    host.iface.as_deref(),
                    DiscoverySource::Mdns,
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

/// Merge one host into an accumulator, keeping the first non-empty identity.
fn merge_host(slot: &mut MdnsHost, other: MdnsHost) {
    if slot.iface.is_none() {
        slot.iface = other.iface;
    }
    if slot.hostname.is_none() {
        slot.hostname = other.hostname;
    }
    if slot.model.is_none() {
        slot.model = other.model;
    }
    for service in other.services {
        if !slot.services.contains(&service) {
            slot.services.push(service);
        }
    }
}

/// Browse one interface, returning the hosts it could assemble.
fn browse_iface(addr: &Ipv4Addr, listen: Duration) -> Result<Vec<MdnsHost>> {
    // Bind to the interface address: that fixes the outgoing multicast
    // interface and makes unicast (`QU`) replies come back here.
    let socket = UdpSocket::bind((*addr, 0))?;
    socket.set_read_timeout(Some(Duration::from_millis(150)))?;

    let target = SocketAddr::from((MDNS_GROUP, MDNS_PORT));

    // 1. Which service types does this network advertise?
    let answers = ask(&socket, target, &[(SERVICES, dns::qtype::PTR)], listen);
    let mut services: Vec<String> = Vec::new();
    for answer in &answers {
        if let AnswerData::Ptr(service) = &answer.data {
            let service = trim_dot(service);
            if !service.is_empty() && !services.contains(&service) {
                services.push(service);
            }
        }
    }
    for known in DEFAULT_SERVICE_TYPES {
        if !services.iter().any(|s| s.eq_ignore_ascii_case(known)) {
            services.push(known.to_string());
        }
    }
    services.truncate(MAX_SERVICES);

    // 2. Instances per service type. Keep the pairing: instance -> service.
    let questions: Vec<(&str, u16)> = services
        .iter()
        .map(|s| (s.as_str(), dns::qtype::PTR))
        .collect();
    let answers = ask(&socket, target, &questions, listen);
    let mut instances: Vec<(String, String)> = Vec::new();
    for answer in &answers {
        if let AnswerData::Ptr(instance) = &answer.data {
            let instance = trim_dot(instance);
            if !instance.is_empty() && !instances.iter().any(|(name, _)| name == &instance) {
                instances.push((instance, trim_dot(&answer.name)));
            }
        }
    }
    instances.truncate(MAX_INSTANCES);

    // 3. SRV (target host) and TXT (model) per instance.
    let questions: Vec<(&str, u16)> = instances
        .iter()
        .flat_map(|(instance, _)| {
            [
                (instance.as_str(), dns::qtype::SRV),
                (instance.as_str(), dns::qtype::TXT),
            ]
        })
        .collect();
    let answers = ask(&socket, target, &questions, listen);

    let mut srv: BTreeMap<String, String> = BTreeMap::new();
    let mut txt: BTreeMap<String, String> = BTreeMap::new();
    for answer in &answers {
        let owner = trim_dot(&answer.name);
        match &answer.data {
            AnswerData::Srv { target, .. } => {
                srv.entry(owner).or_insert_with(|| trim_dot(target));
            }
            AnswerData::Txt(entries) => {
                if let Some(model) = model_from_txt(entries) {
                    txt.entry(owner).or_insert(model);
                }
            }
            _ => {}
        }
    }

    // 4. Addresses for the SRV targets.
    let mut targets: Vec<String> = Vec::new();
    for target in srv.values() {
        if !target.is_empty() && !targets.contains(target) {
            targets.push(target.clone());
        }
    }
    targets.truncate(MAX_INSTANCES);
    let questions: Vec<(&str, u16)> = targets
        .iter()
        .map(|t| (t.as_str(), dns::qtype::A))
        .collect();
    let answers = ask(&socket, target, &questions, listen);

    let mut addresses: BTreeMap<String, Ipv4Addr> = BTreeMap::new();
    for answer in &answers {
        if let AnswerData::A(ip) = &answer.data {
            addresses.entry(trim_dot(&answer.name)).or_insert(*ip);
        }
    }

    // Assemble: one host per address, unioning services.
    let mut hosts: BTreeMap<Ipv4Addr, MdnsHost> = BTreeMap::new();
    for (instance, service) in &instances {
        let Some(target) = srv.get(instance) else {
            continue;
        };
        let Some(ip) = addresses.get(target) else {
            continue;
        };
        let host = hosts.entry(*ip).or_insert_with(|| MdnsHost {
            ip: *ip,
            iface: None,
            hostname: Some(target.clone()),
            model: None,
            services: Vec::new(),
        });
        if host.hostname.is_none() {
            host.hostname = Some(target.clone());
        }
        if host.model.is_none() {
            host.model = txt.get(instance).cloned();
        }
        if !service.is_empty() && !host.services.contains(service) {
            host.services.push(service.clone());
        }
    }

    Ok(hosts.into_values().collect())
}

/// Send each question in its own datagram, then listen for `listen`.
fn ask(
    socket: &UdpSocket,
    target: SocketAddr,
    questions: &[(&str, u16)],
    listen: Duration,
) -> Vec<Answer> {
    for (name, ot) in questions {
        let query = dns::encode_question(name, *ot, dns::CLASS_IN_QU);
        let _ = socket.send_to(&query, target);
    }
    drain(socket, listen)
}

/// Read datagrams until `listen` elapses, parsing whatever arrives.
fn drain(socket: &UdpSocket, listen: Duration) -> Vec<Answer> {
    let deadline = Instant::now() + listen;
    let mut buf = [0u8; RECV_BUF];
    let mut answers = Vec::new();

    while Instant::now() < deadline {
        match socket.recv_from(&mut buf) {
            Ok((len, _peer)) => {
                if let Ok(mut parsed) = dns::parse_answers(&buf[..len]) {
                    answers.append(&mut parsed);
                }
            }
            Err(_) => continue, // read timeout: keep listening until the deadline
        }
    }

    answers
}

/// Pull a model string out of TXT entries, preferring the specific keys.
fn model_from_txt(entries: &[String]) -> Option<String> {
    const KEYS: [&str; 6] = ["model=", "md=", "mn=", "am=", "ty=", "modelname="];
    for key in KEYS {
        for entry in entries {
            let lower = entry.to_ascii_lowercase();
            if let Some(value) = lower.strip_prefix(key) {
                if !value.trim().is_empty() {
                    return Some(entry[key.len()..].trim().to_string());
                }
            }
        }
    }
    None
}

fn trim_dot(name: &str) -> String {
    name.trim_end_matches('.').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_are_extracted_from_txt_with_priority() {
        let entries = vec!["model=AppleTV6,1".to_string(), "md=ignored".to_string()];
        assert_eq!(model_from_txt(&entries).as_deref(), Some("AppleTV6,1"));

        let entries = vec!["md=Chromecast Ultra".to_string()];
        assert_eq!(
            model_from_txt(&entries).as_deref(),
            Some("Chromecast Ultra")
        );

        assert_eq!(model_from_txt(&["flags=1".to_string()]), None);
        assert_eq!(model_from_txt(&["model=".to_string()]), None);
    }

    #[test]
    fn merge_keeps_first_identity_and_unions_services() {
        let mut slot = MdnsHost {
            ip: "10.0.0.1".parse().unwrap(),
            iface: Some("eth0".into()),
            hostname: Some("tv.local".into()),
            model: None,
            services: vec!["_airplay._tcp".into()],
        };
        merge_host(
            &mut slot,
            MdnsHost {
                ip: "10.0.0.1".parse().unwrap(),
                iface: Some("wlan0".into()),
                hostname: Some("other.local".into()),
                model: Some("AppleTV6,1".into()),
                services: vec!["_airplay._tcp".into(), "_raop._tcp".into()],
            },
        );
        assert_eq!(slot.iface.as_deref(), Some("eth0"));
        assert_eq!(slot.hostname.as_deref(), Some("tv.local"));
        assert_eq!(slot.model.as_deref(), Some("AppleTV6,1"));
        assert_eq!(slot.services, vec!["_airplay._tcp", "_raop._tcp"]);
    }

    #[test]
    fn describe_reports_the_listen_window() {
        let discovery = MdnsDiscovery::new(vec![]);
        assert!(discovery.describe().contains("mDNS"));
        assert_eq!(discovery.source(), DiscoverySource::Mdns);
    }

    #[test]
    fn trim_dot_removes_one_trailing_dot() {
        assert_eq!(trim_dot("_airplay._tcp.local."), "_airplay._tcp.local");
        assert_eq!(trim_dot("host.local"), "host.local");
    }
}
