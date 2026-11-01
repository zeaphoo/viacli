//! Joining IP-only sightings to MACs.
//!
//! mDNS, SSDP and L3 sweeps identify devices by IPv4 address, not by hardware
//! address. The inventory is keyed by address, so a MAC is pure enrichment: the
//! neighbour table is consulted when one is available, and a sighting without
//! one is still a perfectly valid observation.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use crate::engine::model::{DiscoverySource, MacAddr, Observation, ObservationInfo};

/// IP → MAC lookup, normally built from the neighbour table.
#[derive(Debug, Clone, Default)]
pub struct ArpJoin {
    map: BTreeMap<Ipv4Addr, MacAddr>,
}

impl ArpJoin {
    pub fn new() -> Self {
        ArpJoin::default()
    }

    /// Read `/proc/net/arp`. One file read, no privileges.
    pub fn from_proc() -> Self {
        let mut join = ArpJoin::new();
        if let Ok(entries) = crate::engine::netinfo::read_proc_net_arp() {
            for entry in entries {
                if !entry.complete {
                    continue;
                }
                if let Ok(mac) = entry.mac.parse::<MacAddr>() {
                    if mac.is_trackable() {
                        join.map.insert(entry.ip, mac);
                    }
                }
            }
        }
        join
    }

    /// Fold in hardware addresses from observations already collected.
    pub fn absorb(&mut self, observations: &[Observation]) {
        for obs in observations {
            if let Some(mac) = obs.mac.filter(|mac| mac.is_trackable()) {
                self.map.insert(obs.ip, mac);
            }
        }
    }

    /// The real MAC for `ip`, when the neighbour table knows it.
    pub fn resolve(&self, ip: Ipv4Addr) -> Option<MacAddr> {
        self.map.get(&ip).copied()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Build the observation for `ip`, carrying the MAC when it is known.
    pub fn observation(
        &self,
        ip: Ipv4Addr,
        iface: Option<&str>,
        source: DiscoverySource,
        info: Option<ObservationInfo>,
    ) -> Observation {
        let mut obs = match self.resolve(ip) {
            Some(mac) => Observation::from_mac(mac, ip, iface.map(str::to_string)),
            None => Observation::from_ip(ip, iface.map(str::to_string)),
        }
        .with_source(source);

        if let Some(info) = info {
            if !info.is_empty() {
                obs = obs.with_info(info);
            }
        }
        obs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_without_a_known_mac_is_address_only() {
        let join = ArpJoin::new();
        let obs = join.observation(
            "10.0.0.5".parse().unwrap(),
            Some("eth0"),
            DiscoverySource::Mdns,
            None,
        );
        assert_eq!(obs.mac, None);
        assert_eq!(obs.ip, "10.0.0.5".parse::<Ipv4Addr>().unwrap());
        assert_eq!(obs.source, DiscoverySource::Mdns);
    }

    #[test]
    fn observation_prefers_a_real_mac() {
        let ip: Ipv4Addr = "192.168.3.96".parse().unwrap();
        let mut join = ArpJoin::new();
        let real: MacAddr = "d0:57:7e:35:f5:2f".parse().unwrap();
        join.absorb(&[Observation::from_mac(real, ip, Some("eth0".into()))]);
        assert_eq!(
            join.observation(ip, None, DiscoverySource::Mdns, None).mac,
            Some(real)
        );
    }

    #[test]
    fn absorb_ignores_untrackable_macs() {
        let ip: Ipv4Addr = "10.0.0.1".parse().unwrap();
        let mut join = ArpJoin::new();
        // Zero and multicast addresses are dropped by Observation::new already.
        join.absorb(&[Observation::from_mac(MacAddr::ZERO, ip, None)]);
        assert!(join.is_empty());
    }
}
