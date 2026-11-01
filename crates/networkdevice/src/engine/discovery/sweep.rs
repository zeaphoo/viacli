//! L3 host sweep for subnets this host is not attached to.
//!
//! ARP cannot cross a router and multicast discovery is link-local, so devices
//! on another segment are only reachable at the IP layer. This source sweeps
//! explicitly configured CIDRs with TCP connects: any address that completes a
//! handshake is a live host.
//!
//! It yields **address-only observations** — the frames go to the router, so
//! there is no MAC to learn — which is exactly what the inventory is built for:
//! devices are keyed by address and fingerprinted with reverse DNS and the
//! optional port probe, regardless of which source found them.

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

use crate::engine::discovery::{Discovery, ScanOutcome};
use crate::engine::error::{Error, Result};
use crate::engine::model::{DiscoverySource, Observation};
use crate::engine::netinfo::parse_cidr;

/// Ports tried per host. A single completed handshake proves the host is up.
pub const DEFAULT_SWEEP_PORTS: [u16; 7] = [80, 443, 22, 445, 3389, 8080, 8443];

/// Sweeps configured CIDRs over L3.
#[derive(Debug, Clone)]
pub struct SweepDiscovery {
    subnets: Vec<String>,
    ports: Vec<u16>,
    timeout: Duration,
    concurrency: usize,
    max_targets: usize,
}

impl Default for SweepDiscovery {
    fn default() -> Self {
        SweepDiscovery {
            subnets: Vec::new(),
            ports: DEFAULT_SWEEP_PORTS.to_vec(),
            timeout: Duration::from_millis(500),
            concurrency: 128,
            max_targets: 4096,
        }
    }
}

impl SweepDiscovery {
    pub fn new(subnets: Vec<String>) -> Self {
        SweepDiscovery {
            subnets,
            ..Default::default()
        }
    }

    pub fn with_ports(mut self, ports: Vec<u16>) -> Self {
        self.ports = ports;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency.max(1);
        self
    }

    pub fn with_max_targets(mut self, max: usize) -> Self {
        self.max_targets = max.max(1);
        self
    }

    /// Expand the configured CIDRs, capped at `max_targets`.
    ///
    /// Unparseable specifications are skipped and returned as warnings: one bad
    /// entry must not stop the others. Every specification is parsed before any
    /// expansion, so a cap hit on one subnet cannot hide a typo in a later one.
    pub fn targets(&self) -> (Vec<Ipv4Addr>, Vec<String>) {
        let mut warnings = Vec::new();
        let mut parsed = Vec::new();

        for spec in &self.subnets {
            match parse_cidr(spec) {
                Ok(subnet) => parsed.push(subnet),
                Err(e) => warnings.push(format!("sweep: ignoring `{spec}`: {e}")),
            }
        }

        let mut targets = Vec::new();
        for (network, prefix) in parsed {
            for host in hosts_in(network, prefix) {
                if targets.len() >= self.max_targets {
                    warnings.push(format!(
                        "sweep: target cap of {} reached; remaining addresses were skipped",
                        self.max_targets
                    ));
                    return (targets, warnings);
                }
                targets.push(host);
            }
        }

        (targets, warnings)
    }
}

impl Discovery for SweepDiscovery {
    fn source(&self) -> DiscoverySource {
        DiscoverySource::Sweep
    }

    fn describe(&self) -> String {
        let subnets = if self.subnets.is_empty() {
            "no subnets".to_string()
        } else {
            self.subnets.join(", ")
        };
        format!(
            "L3 TCP sweep of {subnets} ({} port(s), {}ms timeout)",
            self.ports.len(),
            self.timeout.as_millis()
        )
    }

    fn preflight(&self) -> Result<()> {
        let (targets, _) = self.targets();
        if targets.is_empty() {
            return Err(Error::unavailable(
                "sweep",
                "no sweepable subnet configured; pass --subnet <CIDR> (repeatable) \
                 or set `extra_subnets` in the config",
            ));
        }
        Ok(())
    }

    fn scan(&self) -> Result<ScanOutcome> {
        let (targets, mut warnings) = self.targets();
        if targets.is_empty() {
            return Err(Error::unavailable(
                "sweep",
                "no sweepable subnet configured; pass --subnet <CIDR> (repeatable)",
            ));
        }

        let started = Instant::now();

        // One flat list of every (address, port) pair: the connector bounds how
        // many are in flight, and a handshake on any of them proves the host is
        // alive.
        let targets: Vec<(Ipv4Addr, u16)> = targets
            .iter()
            .flat_map(|ip| self.ports.iter().map(move |port| (*ip, *port)))
            .collect();
        let mut alive: Vec<Ipv4Addr> =
            crate::engine::connect::open_ports(&targets, self.timeout, self.concurrency)
                .into_iter()
                .map(|(ip, _)| ip)
                .collect();
        alive.sort();
        alive.dedup();

        let mut observations: Vec<Observation> = alive
            .into_iter()
            .map(|ip| Observation::from_ip(ip, None).with_source(DiscoverySource::Sweep))
            .collect();
        observations.sort_by_key(|obs| obs.ip);

        warnings.push(format!(
            "sweep: {} of {} address(es) responded in {}ms",
            observations.len(),
            targets.len(),
            started.elapsed().as_millis()
        ));

        Ok(ScanOutcome {
            observations,
            warnings,
        })
    }
}

/// Every usable host address in a subnet, network and broadcast excluded.
fn hosts_in(ip: Ipv4Addr, prefix: u8) -> Vec<Ipv4Addr> {
    let mask = if prefix == 0 {
        0u32
    } else {
        u32::MAX << (32 - prefix)
    };
    let network = u32::from(ip) & mask;
    let broadcast = network | !mask;

    // /31 and /32 have no network/broadcast to exclude (RFC 3021).
    let (first, last) = if prefix >= 31 {
        (network, broadcast)
    } else {
        (network + 1, broadcast.saturating_sub(1))
    };

    (first..=last).map(Ipv4Addr::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cidr_and_bare_addresses() {
        assert_eq!(
            parse_cidr("192.168.2.0/24").unwrap(),
            ("192.168.2.0".parse().unwrap(), 24)
        );
        assert_eq!(
            parse_cidr("10.0.0.7").unwrap(),
            ("10.0.0.7".parse().unwrap(), 32)
        );
        assert!(parse_cidr("192.168.2.0/33").is_err());
        assert!(parse_cidr("not-an-ip/24").is_err());
        assert!(parse_cidr("10.0.0.0/abc").is_err());
    }

    #[test]
    fn hosts_exclude_network_and_broadcast() {
        let hosts = hosts_in("192.168.2.0".parse().unwrap(), 24);
        assert_eq!(hosts.len(), 254);
        assert_eq!(hosts[0], Ipv4Addr::new(192, 168, 2, 1));
        assert_eq!(hosts[253], Ipv4Addr::new(192, 168, 2, 254));

        // A /31 keeps both ends (point-to-point links, RFC 3021).
        let hosts = hosts_in("10.0.0.0".parse().unwrap(), 31);
        assert_eq!(
            hosts,
            vec![Ipv4Addr::new(10, 0, 0, 0), Ipv4Addr::new(10, 0, 0, 1)]
        );

        // A /32 is the single address itself.
        let hosts = hosts_in("10.0.0.5".parse().unwrap(), 32);
        assert_eq!(hosts, vec![Ipv4Addr::new(10, 0, 0, 5)]);
    }

    #[test]
    fn target_cap_and_bad_entries_are_warnings() {
        let sweep = SweepDiscovery::new(vec![
            "192.168.2.0/24".into(),
            "definitely-not-a-subnet".into(),
        ])
        .with_max_targets(10);

        let (targets, warnings) = sweep.targets();
        assert_eq!(targets.len(), 10);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("definitely-not-a-subnet"))
        );
        assert!(warnings.iter().any(|w| w.contains("cap")));
    }

    #[test]
    fn an_empty_sweep_source_fails_preflight() {
        let sweep = SweepDiscovery::new(Vec::new());
        assert!(sweep.preflight().is_err());
        assert!(sweep.describe().contains("no subnets"));
    }

    #[test]
    fn describe_names_the_subnets_and_ports() {
        let sweep = SweepDiscovery::new(vec!["192.168.2.0/24".into()]).with_ports(vec![80, 443]);
        let text = sweep.describe();
        assert!(text.contains("192.168.2.0/24"), "{text}");
        assert!(text.contains("2 port(s)"), "{text}");
        assert_eq!(sweep.source(), DiscoverySource::Sweep);
    }

    #[test]
    fn a_listening_socket_is_found() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let sweep = SweepDiscovery::new(vec!["127.0.0.1/32".into()])
            .with_ports(vec![port])
            .with_timeout(Duration::from_millis(500));

        let outcome = sweep.scan().unwrap();
        assert_eq!(outcome.observations.len(), 1);
        assert_eq!(
            outcome.observations[0].ip,
            "127.0.0.1".parse::<Ipv4Addr>().unwrap()
        );
        assert_eq!(
            outcome.observations[0].mac, None,
            "L3 sweeps cannot see a MAC"
        );
    }
}
