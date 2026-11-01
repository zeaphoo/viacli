//! The network segments a scan covers.
//!
//! A *segment* is a CIDR plus where it came from: a subnet of a local interface
//! (the machine is attached to it), a subnet listed in `extra_subnets` (reached
//! through a router), or one learned from a device later on.
//!
//! This exists so that "which networks do we scan" is a first-class, inspectable
//! answer rather than a side effect of two unrelated settings. Sources keep
//! consuming it as before; the API and the UI can show it.

use std::net::Ipv4Addr;

use serde::Serialize;

use crate::engine::config::Config;
use crate::engine::error::Result;
use crate::engine::netinfo::{self, LocalSubnet};

/// Where a segment came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SegmentOrigin {
    /// A subnet of a local interface: this machine is attached to it.
    Connected,
    /// Listed in `extra_subnets`: reached over L3, through a router.
    Explicit,
    /// Learned from a device — a router's table, an SNMP interface list.
    ///
    /// Nothing produces these yet; the variant is here so the model does not
    /// have to change when something does.
    Learned,
}

/// One network the scan covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Segment {
    /// Canonical `network/prefix`, e.g. `192.168.3.0/24`.
    pub cidr: String,
    /// The interface carrying it, when this machine is attached to it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iface: Option<String>,
    /// This machine's own address on it, when attached.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_addr: Option<Ipv4Addr>,
    pub origin: SegmentOrigin,
    /// Addresses it can hold, network and broadcast excluded (`/31` and `/32`
    /// keep both, per RFC 3021).
    pub hosts: u64,
}

/// Every segment the configured sources cover.
///
/// Locally attached subnets are listed first: they are a fact of the machine and
/// the ARP sources sweep them. `extra_subnets` are added on top. A subnet that
/// appears both ways is reported once, as `connected`.
///
/// Unparseable `extra_subnets` entries are skipped here rather than reported:
/// the sweep source parses the same strings and its per-cycle warning already
/// names them, so diagnostics stay in one place.
pub fn resolve(config: &Config) -> Result<Vec<Segment>> {
    let connected = netinfo::scan_subnets(&config.ifaces)?;
    Ok(merge(connected, &config.extra_subnets))
}

/// The pure half of [`resolve`]: no interfaces are read, so the merge and
/// canonicalisation rules can be tested on their own.
pub fn merge(connected: Vec<LocalSubnet>, explicit: &[String]) -> Vec<Segment> {
    let mut segments: Vec<Segment> = Vec::new();

    for subnet in connected {
        let network = subnet.network();
        let cidr = format!("{network}/{}", subnet.prefix_len);
        if segments.iter().any(|segment| segment.cidr == cidr) {
            continue;
        }
        segments.push(Segment {
            cidr,
            iface: Some(subnet.iface),
            local_addr: Some(subnet.addr),
            origin: SegmentOrigin::Connected,
            hosts: netinfo::host_count(subnet.prefix_len),
        });
    }

    for spec in explicit {
        let Ok((network, prefix_len)) = netinfo::parse_cidr(spec) else {
            continue; // the sweep source reports it
        };
        let cidr = format!("{network}/{prefix_len}");
        if segments.iter().any(|segment| segment.cidr == cidr) {
            continue;
        }
        segments.push(Segment {
            cidr,
            iface: None,
            local_addr: None,
            origin: SegmentOrigin::Explicit,
            hosts: netinfo::host_count(prefix_len),
        });
    }

    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subnet(iface: &str, addr: &str, prefix_len: u8) -> LocalSubnet {
        LocalSubnet {
            iface: iface.to_string(),
            addr: addr.parse().unwrap(),
            prefix_len,
        }
    }

    #[test]
    fn connected_subnets_are_reported_with_their_interface_and_own_address() {
        let segments = merge(vec![subnet("enp5s0", "192.168.3.128", 24)], &[]);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].cidr, "192.168.3.0/24", "canonical network form");
        assert_eq!(segments[0].iface.as_deref(), Some("enp5s0"));
        assert_eq!(segments[0].local_addr.unwrap().to_string(), "192.168.3.128");
        assert_eq!(segments[0].origin, SegmentOrigin::Connected);
        assert_eq!(segments[0].hosts, 254);
    }

    #[test]
    fn explicit_subnets_are_added_and_never_shadow_a_connected_one() {
        let connected = vec![subnet("enp5s0", "192.168.3.128", 24)];
        let explicit = vec![
            "192.168.3.0/24".to_string(), // already attached: stays `connected`
            "10.9.0.0/24".to_string(),    // reached through a router
        ];
        let segments = merge(connected, &explicit);

        assert_eq!(segments.len(), 2, "the duplicate collapses");
        assert_eq!(segments[0].origin, SegmentOrigin::Connected);
        assert_eq!(segments[1].cidr, "10.9.0.0/24");
        assert_eq!(segments[1].origin, SegmentOrigin::Explicit);
        assert!(segments[1].iface.is_none());
        assert!(segments[1].local_addr.is_none());
    }

    #[test]
    fn a_host_address_is_read_as_its_segment() {
        // The setting is a description of a network, so 10.0.0.7/24 and
        // 10.0.0.0/24 must not be two different segments.
        let segments = merge(Vec::new(), &["10.0.0.7/24".into(), "10.0.0.0/24".into()]);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].cidr, "10.0.0.0/24");
    }

    #[test]
    fn unparseable_specs_are_skipped_without_losing_the_rest() {
        let segments = merge(
            Vec::new(),
            &[
                "nonsense".into(),
                "10.1.2.0/24".into(),
                "10.1.3.0/33".into(),
            ],
        );
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].cidr, "10.1.2.0/24");
    }

    #[test]
    fn host_counts_follow_the_prefix_length() {
        let segments = merge(
            Vec::new(),
            &[
                "10.0.0.0/31".into(),
                "10.0.1.1/32".into(),
                "10.0.2.0/16".into(),
            ],
        );
        let hosts: Vec<u64> = segments.iter().map(|segment| segment.hosts).collect();
        assert_eq!(hosts, vec![2, 1, 65534]);
    }

    #[test]
    fn the_order_is_connected_first_then_config_order() {
        let segments = merge(
            vec![
                subnet("wlan0", "10.1.0.5", 24),
                subnet("eth0", "10.2.0.5", 24),
            ],
            &["192.168.9.0/24".into()],
        );
        let cidrs: Vec<&str> = segments
            .iter()
            .map(|segment| segment.cidr.as_str())
            .collect();
        assert_eq!(cidrs, vec!["10.1.0.0/24", "10.2.0.0/24", "192.168.9.0/24"]);
    }
}
