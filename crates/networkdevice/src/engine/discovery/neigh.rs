//! Unprivileged discovery through the kernel neighbour table.
//!
//! Reading `/proc/net/arp` needs no privileges, but it only contains entries the
//! kernel has recently resolved. To make it a usable *scanner* we first force
//! resolution with a **TCP connect sweep**: opening (and immediately closing) a
//! TCP connection to every address in the subnet makes the kernel ARP for each
//! one, whether or not anything is listening. No raw socket, no `CAP_NET_RAW`.
//!
//! Trade-off: hosts that are silent at the IP layer (firewalled, powered off,
//! or only speaking non-IP protocols) are invisible, and it does not work
//! across routers the way a real ARP sweep does. For accuracy use the `pnet`
//! source instead.

use std::net::Ipv4Addr;
use std::time::Duration;

use crate::engine::discovery::{Discovery, ScanOutcome};
use crate::engine::error::{Error, Result};
use crate::engine::model::{DiscoverySource, MacAddr, Observation};
use crate::engine::netinfo;

/// Ports tried during the population sweep. A closed port is enough: the SYN
/// still triggers ARP resolution, and the RST that follows is irrelevant.
const DEFAULT_SWEEP_PORTS: [u16; 3] = [80, 443, 22];

#[derive(Debug, Clone)]
pub struct NeighborDiscovery {
    ifaces: Vec<String>,
    /// Run a TCP connect sweep before reading the table.
    sweep: bool,
    connect_timeout: Duration,
    concurrency: usize,
    sweep_ports: Vec<u16>,
    /// Hard cap on swept addresses. A single /16 Docker bridge would otherwise
    /// mean 65k connects per scan, which is both slow and antisocial.
    max_targets: usize,
}

impl Default for NeighborDiscovery {
    fn default() -> Self {
        NeighborDiscovery {
            ifaces: Vec::new(),
            sweep: true,
            connect_timeout: Duration::from_millis(300),
            concurrency: 64,
            sweep_ports: DEFAULT_SWEEP_PORTS.to_vec(),
            max_targets: 1024,
        }
    }
}

impl NeighborDiscovery {
    pub fn new(ifaces: Vec<String>) -> Self {
        NeighborDiscovery {
            ifaces,
            ..Default::default()
        }
    }

    pub fn with_sweep(mut self, sweep: bool) -> Self {
        self.sweep = sweep;
        self
    }

    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency.max(1);
        self
    }

    pub fn with_sweep_ports(mut self, ports: Vec<u16>) -> Self {
        self.sweep_ports = ports;
        self
    }

    pub fn with_max_targets(mut self, max: usize) -> Self {
        self.max_targets = max.max(1);
        self
    }

    /// Addresses that the sweep would probe, after interface filtering, the
    /// large-subnet guard and the target cap.
    ///
    /// Subnets are visited smallest-first so that a modest /24 is always swept
    /// before a sprawling /16 bridge consumes the whole budget.
    pub fn sweep_targets(&self) -> Result<Vec<Ipv4Addr>> {
        // Interface selection (and loopback exclusion) is `netinfo`'s rule, so
        // "no interfaces configured" means the same thing in every source.
        let subnets = netinfo::scan_subnets(&self.ifaces)?;

        let mut candidates: Vec<_> = subnets
            .into_iter()
            .filter(|s| s.is_reasonable_to_sweep())
            .collect();
        candidates.sort_by_key(|s| s.host_count());

        let mut targets = Vec::new();
        for subnet in candidates {
            for host in subnet.hosts() {
                targets.push(host);
                if targets.len() >= self.max_targets {
                    return Ok(targets);
                }
            }
        }

        Ok(targets)
    }

    /// Best-effort TCP connect sweep, bounded by `concurrency` connections in
    /// flight (the async connector's job).
    ///
    /// Failures are ignored on purpose: the point is to populate the kernel's
    /// neighbour cache, not to learn anything from the result.
    fn populate(&self) -> Result<()> {
        let targets = self.sweep_targets()?;
        if targets.is_empty() {
            return Ok(());
        }

        // A connect attempt on every (address, port) pair. Either outcome is
        // fine: a handshake or a RST both prove the host answered and leave a
        // complete ARP entry, so the result is discarded.
        let pairs: Vec<(Ipv4Addr, u16)> = targets
            .iter()
            .flat_map(|ip| self.sweep_ports.iter().map(move |port| (*ip, *port)))
            .collect();
        let _ = crate::engine::connect::open_ports(&pairs, self.connect_timeout, self.concurrency);
        Ok(())
    }
}

impl Discovery for NeighborDiscovery {
    fn source(&self) -> DiscoverySource {
        DiscoverySource::NeighborTable
    }

    fn describe(&self) -> String {
        let sweep = if self.sweep {
            format!(
                "tcp sweep of up to {} addresses on port(s) {:?}, {}ms timeout",
                self.max_targets,
                self.sweep_ports,
                self.connect_timeout.as_millis()
            )
        } else {
            "no sweep (existing table entries only)".to_string()
        };
        format!("kernel neighbour table (/proc/net/arp), {sweep}")
    }

    fn preflight(&self) -> Result<()> {
        if !std::path::Path::new("/proc/net/arp").exists() {
            return Err(Error::unavailable(
                "neigh",
                "/proc/net/arp not present (this source is Linux-only; enable the pnet source)",
            ));
        }
        Ok(())
    }

    fn scan(&self) -> Result<ScanOutcome> {
        self.preflight()?;

        if self.sweep {
            self.populate()?;
        }

        let entries = netinfo::read_proc_net_arp()?;
        let allowed = netinfo::allowed_ifaces(&self.ifaces)?;
        let mut observations = Vec::with_capacity(entries.len());

        for entry in entries {
            if !entry.complete {
                continue;
            }
            // A zero hardware address marks an unresolved placeholder.
            let Ok(mac) = entry.mac.parse::<MacAddr>() else {
                continue;
            };
            if mac.is_zero() {
                continue;
            }
            if !allowed.contains(&entry.iface) {
                continue;
            }

            observations.push(
                Observation::from_mac(mac, entry.ip, Some(entry.iface))
                    .with_source(DiscoverySource::NeighborTable),
            );
        }

        Ok(ScanOutcome::clean(observations))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_reports_sweep_configuration() {
        let with = NeighborDiscovery::new(vec!["eth0".into()]);
        assert!(with.describe().contains("tcp sweep"));
        assert!(with.describe().contains("up to 1024"));
        let without = NeighborDiscovery::new(vec![]).with_sweep(false);
        assert!(without.describe().contains("no sweep"));
    }

    #[test]
    fn sweep_targets_are_capped() {
        // The cap must hold even on a host with huge bridges (this sandbox has a
        // /16 Docker bridge); without it a scan would take many minutes.
        let discovery = NeighborDiscovery::new(vec![]).with_max_targets(10);
        let targets = discovery.sweep_targets().expect("enumeration should work");
        assert!(
            targets.len() <= 10,
            "cap not enforced: got {} targets",
            targets.len()
        );
    }

    #[test]
    fn sweep_targets_never_include_loopback_or_broadcast() {
        let discovery = NeighborDiscovery::new(vec![]).with_max_targets(4096);
        for target in discovery.sweep_targets().expect("enumeration should work") {
            assert!(!target.is_loopback(), "loopback leaked: {target}");
            assert!(!target.is_broadcast(), "broadcast leaked: {target}");
        }
    }

    #[test]
    fn interface_filter_narrows_the_sweep() {
        // Filtering to a non-existent interface must yield nothing, which is
        // what keeps the integration test fast.
        let discovery =
            NeighborDiscovery::new(vec!["definitely-not-an-iface".into()]).with_max_targets(4096);
        let targets = discovery.sweep_targets().expect("enumeration should work");
        assert!(targets.is_empty(), "filter ignored: {targets:?}");
    }

    #[test]
    fn scan_returns_trackable_observations_on_linux() {
        // The sandbox has a populated neighbour table, so this should yield at
        // least one entry. Skip silently where /proc/net/arp is absent.
        let discovery = NeighborDiscovery::new(vec![]).with_sweep(false);
        if discovery.preflight().is_err() {
            return;
        }
        let found = discovery.scan().expect("scan should succeed");
        for obs in &found.observations {
            assert!(
                obs.mac.is_none_or(|mac| mac.is_trackable()),
                "untrackable MAC leaked: {obs:?}"
            );
            assert_eq!(obs.source, DiscoverySource::NeighborTable);
        }
    }
}
