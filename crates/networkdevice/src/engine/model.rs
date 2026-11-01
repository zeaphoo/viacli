//! Domain primitives: addresses, kinds, observations and the evidence labels
//! that ride along with them.
//!
//! The inventory layers themselves live in [`crate::engine::inventory`]; this module is
//! only the vocabulary they share.

use std::fmt;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::engine::error::{Error, Result};

/// A 48-bit Ethernet MAC address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MacAddr([u8; 6]);

impl MacAddr {
    pub const BROADCAST: MacAddr = MacAddr([0xff; 6]);
    pub const ZERO: MacAddr = MacAddr([0x00; 6]);

    pub fn new(bytes: [u8; 6]) -> Self {
        MacAddr(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 6] {
        &self.0
    }

    /// Uppercase hex without separators, e.g. `D0577E35F52F`.
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(12);
        for b in self.0 {
            s.push_str(&format!("{b:02X}"));
        }
        s
    }

    /// True for the all-zero address, which some backends report for
    /// incomplete neighbour entries.
    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 6]
    }

    /// True for broadcast/multicast addresses (LSB of the first octet).
    pub fn is_multicast(&self) -> bool {
        self.0[0] & 0x01 != 0
    }

    /// True for locally administered addresses (bit 1 of the first octet).
    pub fn is_locally_administered(&self) -> bool {
        self.0[0] & 0x02 != 0
    }

    /// Addresses worth tracking: not zero, not multicast/broadcast.
    pub fn is_trackable(&self) -> bool {
        !self.is_zero() && !self.is_multicast()
    }
}

impl FromStr for MacAddr {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let cleaned: String = s
            .trim()
            .chars()
            .filter(|c| *c != ':' && *c != '-' && *c != '.')
            .collect();

        if cleaned.len() != 12 {
            return Err(Error::parse(
                "MAC address",
                format!("`{s}` does not contain 12 hexadecimal digits"),
            ));
        }

        let mut bytes = [0u8; 6];
        for (i, byte) in bytes.iter_mut().enumerate() {
            let pair = &cleaned[i * 2..i * 2 + 2];
            *byte = u8::from_str_radix(pair, 16).map_err(|_| {
                Error::parse("MAC address", format!("`{pair}` is not valid hexadecimal"))
            })?;
        }
        Ok(MacAddr(bytes))
    }
}

impl fmt::Display for MacAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5]
        )
    }
}

impl Serialize for MacAddr {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for MacAddr {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        MacAddr::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// Best-effort classification of what a device is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceKind {
    Router,
    AccessPoint,
    Switch,
    Computer,
    Server,
    VirtualMachine,
    Container,
    Phone,
    Tablet,
    Printer,
    Camera,
    Nas,
    Tv,
    GameConsole,
    Iot,
    #[default]
    Unknown,
}

impl DeviceKind {
    pub const ALL: [DeviceKind; 16] = [
        DeviceKind::Router,
        DeviceKind::AccessPoint,
        DeviceKind::Switch,
        DeviceKind::Computer,
        DeviceKind::Server,
        DeviceKind::VirtualMachine,
        DeviceKind::Container,
        DeviceKind::Phone,
        DeviceKind::Tablet,
        DeviceKind::Printer,
        DeviceKind::Camera,
        DeviceKind::Nas,
        DeviceKind::Tv,
        DeviceKind::GameConsole,
        DeviceKind::Iot,
        DeviceKind::Unknown,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            DeviceKind::Router => "router",
            DeviceKind::AccessPoint => "ap",
            DeviceKind::Switch => "switch",
            DeviceKind::Computer => "computer",
            DeviceKind::Server => "server",
            DeviceKind::VirtualMachine => "vm",
            DeviceKind::Container => "container",
            DeviceKind::Phone => "phone",
            DeviceKind::Tablet => "tablet",
            DeviceKind::Printer => "printer",
            DeviceKind::Camera => "camera",
            DeviceKind::Nas => "nas",
            DeviceKind::Tv => "tv",
            DeviceKind::GameConsole => "console",
            DeviceKind::Iot => "iot",
            DeviceKind::Unknown => "unknown",
        }
    }
}

impl fmt::Display for DeviceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Where an observation came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiscoverySource {
    /// Kernel neighbour table (`/proc/net/arp`).
    NeighborTable,
    /// The external `arp-scan` process. The source is gone, but old endpoint
    /// rows still name it, so the tag survives to keep them readable.
    ArpScan,
    /// In-process ARP sweep over a raw socket.
    PnetArp,
    /// mDNS / DNS-SD multicast browsing (`_services._dns.local.`).
    Mdns,
    /// SSDP / UPnP discovery (`M-SEARCH` on 239.255.255.250).
    Ssdp,
    /// L3 TCP sweep of explicitly configured subnets.
    Sweep,
    /// Reverse-DNS PTR lookup.
    Dns,
    /// The port/fingerprint probe (`probe.rs`): describes addresses that are
    /// already known rather than finding new ones.
    Probe,
    /// Service/version detection by the external `nmap` process.
    Nmap,
    /// Derived from the hardware address: the OUI registry's vendor name. Not a
    /// scan — a lookup the aggregator does for every address that has a MAC.
    Oui,
    /// Operator-supplied input (imported fixtures).
    Manual,
}

impl DiscoverySource {
    pub fn label(&self) -> &'static str {
        match self {
            DiscoverySource::NeighborTable => "neigh",
            DiscoverySource::ArpScan => "arp-scan",
            DiscoverySource::PnetArp => "pnet",
            DiscoverySource::Mdns => "mdns",
            DiscoverySource::Ssdp => "ssdp",
            DiscoverySource::Sweep => "sweep",
            DiscoverySource::Dns => "dns",
            DiscoverySource::Probe => "probe",
            DiscoverySource::Nmap => "nmap",
            DiscoverySource::Oui => "oui",
            DiscoverySource::Manual => "manual",
        }
    }
}

impl fmt::Display for DiscoverySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Identity hints carried by a sighting, used to enrich identification.
///
/// ARP-family sources only know the MAC; mDNS and SSDP know much more (a
/// hostname, a model string, and the services a device announces).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ObservationInfo {
    /// Best-effort host name (mDNS PTR target, UPnP friendlyName).
    pub hostname: Option<String>,
    /// Open ports and what answered on them. Only the probe fills this in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<PortInfo>,
    /// Vendor, from the OUI registry. The MAC is the probe's to look up
    /// because it is the only source that carries the table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// Model string when the device tells us one (mDNS TXT `model=`,
    /// UPnP `modelName`).
    pub model: Option<String>,
    /// A kind a fingerprint rule deduced outright — a web rule that recognises
    /// a printer's UI, or a service template that names the product. The
    /// strongest evidence there is, because the classification is already done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<DeviceKind>,
    /// Service types the device announced: `_airplay._tcp`, `googlecast`, ...
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
}

impl ObservationInfo {
    /// `true` when there is nothing here yet.
    pub fn is_empty(&self) -> bool {
        self.hostname.is_none()
            && self.model.is_none()
            && self.services.is_empty()
            && self.ports.is_empty()
            && self.vendor.is_none()
    }

    /// Non-empty service types, lower-cased, deduplicated, sorted.
    pub fn normalized_services(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for service in &self.services {
            let s = service.trim().to_ascii_lowercase();
            if !s.is_empty() && !out.contains(&s) {
                out.push(s);
            }
        }
        out.sort();
        out
    }
}

/// An address the probe should describe, and what is already known about it.
///
/// It carries only what the probe needs to do its job; the inventory stays the
/// single owner of everything else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeTarget {
    pub ip: std::net::Ipv4Addr,
    pub mac: Option<MacAddr>,
    pub iface: Option<String>,
}

/// A single sighting of a device during one scan.
///
/// The IPv4 address is the identity: it is what can be addressed, scanned and
/// fingerprinted, and what survives crossing a router. The MAC is optional —
/// ARP-family sources know it, L3 scans across routers cannot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub ip: std::net::Ipv4Addr,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac: Option<MacAddr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iface: Option<String>,
    pub source: DiscoverySource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info: Option<ObservationInfo>,
}

impl Observation {
    pub fn new(mac: Option<MacAddr>, ip: std::net::Ipv4Addr, iface: Option<String>) -> Self {
        let mac = mac.filter(|mac| mac.is_trackable());
        Observation {
            ip,
            mac,
            iface,
            source: DiscoverySource::NeighborTable,
            info: None,
        }
    }

    /// A sighting with a known hardware address.
    pub fn from_mac(mac: MacAddr, ip: std::net::Ipv4Addr, iface: Option<String>) -> Self {
        Observation::new(Some(mac), ip, iface)
    }

    /// A sighting that only knows the address (L3 scan, mDNS, UPnP).
    pub fn from_ip(ip: std::net::Ipv4Addr, iface: Option<String>) -> Self {
        Observation::new(None, ip, iface)
    }

    pub fn with_source(mut self, source: DiscoverySource) -> Self {
        self.source = source;
        self
    }

    pub fn with_info(mut self, info: ObservationInfo) -> Self {
        self.info = Some(info);
        self
    }
}

/// One piece of evidence with its provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Label {
    pub value: String,
    pub source: DiscoverySource,
    /// When this piece of evidence was collected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<DateTime<Utc>>,
}

/// What one probe found listening on an address.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortInfo {
    pub port: u16,
    /// Best-effort service name (from a banner or a port hint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// Raw banner, when the probe could capture one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banner: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_mac_formats() {
        let expected = MacAddr::new([0xd0, 0x57, 0x7e, 0x35, 0xf5, 0x2f]);
        for input in [
            "d0:57:7e:35:f5:2f",
            "D0-57-7E-35-F5-2F",
            "d057.7e35.f52f",
            "  d0:57:7e:35:f5:2f  ",
        ] {
            assert_eq!(MacAddr::from_str(input).unwrap(), expected, "input={input}");
        }
    }

    #[test]
    fn rejects_malformed_macs() {
        for input in ["", "d0:57:7e", "zz:57:7e:35:f5:2f", "d0:57:7e:35:f5:2f:00"] {
            assert!(MacAddr::from_str(input).is_err(), "input={input}");
        }
    }

    #[test]
    fn classifies_address_flags() {
        let local = MacAddr::from_str("06:fb:4d:6d:5d:81").unwrap();
        assert!(local.is_locally_administered());
        assert!(local.is_trackable());

        let multi = MacAddr::from_str("01:00:5e:00:00:01").unwrap();
        assert!(multi.is_multicast());
        assert!(!multi.is_trackable());

        assert!(!MacAddr::BROADCAST.is_trackable());
        assert!(MacAddr::new([0; 6]).is_zero());
    }

    #[test]
    fn mac_roundtrips_through_json() {
        let mac = MacAddr::from_str("2c:a0:42:30:3c:7e").unwrap();
        let json = serde_json::to_string(&mac).unwrap();
        assert_eq!(json, "\"2C:A0:42:30:3C:7E\"");
        assert_eq!(serde_json::from_str::<MacAddr>(&json).unwrap(), mac);
    }

    #[test]
    fn observations_drop_untrackable_macs() {
        let obs = Observation::from_mac(
            MacAddr::ZERO,
            "10.0.0.1".parse().unwrap(),
            Some("eth0".into()),
        );
        assert_eq!(obs.mac, None, "a zero MAC is not evidence");

        let obs = Observation::from_ip("10.0.0.1".parse().unwrap(), None);
        assert_eq!(obs.mac, None);
        assert!(obs.info.is_none());
    }

    #[test]
    fn observation_info_normalizes_services() {
        let info = ObservationInfo {
            hostname: None,
            model: None,
            services: vec![" _AirPlay._TCP ".into(), "".into(), "_airplay._tcp".into()],
            ..Default::default()
        };
        assert_eq!(info.normalized_services(), vec!["_airplay._tcp"]);
        assert!(!info.is_empty());

        let info = ObservationInfo {
            hostname: Some("x".into()),
            ..Default::default()
        };
        assert!(!info.is_empty());
        assert!(ObservationInfo::default().is_empty());
    }
}
