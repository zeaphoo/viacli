//! Discovery sources.
//!
//! Every source implements [`Discovery`] and returns raw [`Observation`]s.
//! Turning observations into inventory state is the job of
//! [`crate::engine::registry::Registry`]; identification is [`crate::engine::identify`].
//!
//! Sources are **not** mutually exclusive. The engine schedules several at once,
//! each on its own polling interval (see [`crate::engine::service`]), and
//! their observations merge in the registry. The engine has no preference for
//! one protocol: a device often shows up in more than one, which is exactly what
//! makes the merge (by MAC, in [`crate::engine::registry`]) and the identity hints worth
//! having.
//!
//! | source | privileges | what happens |
//! |---|---|---|
//! | [`neigh`] | none | reads the neighbour table, after a TCP sweep to populate it |
//! | [`mdns`] | none | mDNS/DNS-SD browse: hostname, model and service hints |
//! | [`ssdp`] | none | SSDP/UPnP `M-SEARCH` + description fetch |
//! | [`sweep`] | none | L3 TCP sweep of explicitly configured subnets |
//!
//! [`CompositeDiscovery`] is kept as a library building block (a primary plus a
//! neighbour baseline) for embedders, but the engine no longer needs it: each
//! source is scheduled on its own and a failure costs only that source.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::engine::error::{Error, Result};
use crate::engine::model::{DiscoverySource, Observation};

pub mod arpjoin;
pub mod composite;
pub mod dns;
pub mod mdns;
pub mod neigh;
pub mod ssdp;
pub mod sweep;

pub use arpjoin::ArpJoin;
pub use composite::CompositeDiscovery;
pub use mdns::MdnsDiscovery;
pub use neigh::NeighborDiscovery;
pub use ssdp::SsdpDiscovery;
pub use sweep::SweepDiscovery;

/// What one scan produced.
///
/// `Err` from [`Discovery::scan`] means the scan produced nothing usable. This
/// type is for the softer case: data came back, but part of the work failed and
/// the caller should be told. The `neigh` baseline makes that a normal
/// occurrence — the active scan can fail while the neighbour table still yields
/// hosts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScanOutcome {
    pub observations: Vec<Observation>,
    /// Non-fatal problems, already formatted for display.
    pub warnings: Vec<String>,
}

impl ScanOutcome {
    /// An outcome with no problems.
    pub fn clean(observations: Vec<Observation>) -> Self {
        ScanOutcome {
            observations,
            warnings: Vec::new(),
        }
    }

    /// An outcome with a warning attached.
    pub fn warning(observations: Vec<Observation>, warning: impl Into<String>) -> Self {
        ScanOutcome {
            observations,
            warnings: vec![warning.into()],
        }
    }

    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }

    /// Fold another outcome into this one: observations concatenated, warnings
    /// appended.
    pub fn absorb(&mut self, other: ScanOutcome) {
        self.observations.extend(other.observations);
        self.warnings.extend(other.warnings);
    }

    pub fn is_empty(&self) -> bool {
        self.observations.is_empty()
    }
}

/// A source of device observations.
pub trait Discovery: Send + Sync {
    /// Which backend this is, recorded on every observation.
    fn source(&self) -> DiscoverySource;

    /// Human-readable description of the concrete configuration.
    fn describe(&self) -> String;

    /// Cheap check that the backend can run before a scan is attempted, so the
    /// CLI can fail with a useful message instead of an empty result.
    fn preflight(&self) -> Result<()> {
        Ok(())
    }

    /// Perform one scan.
    ///
    /// Return `Err` only when nothing usable was produced; use
    /// [`ScanOutcome::warnings`] for partial failures.
    fn scan(&self) -> Result<ScanOutcome>;
}

/// Legacy single-source selector.
///
/// Kept so configuration files written before multi-source scheduling keep
/// loading: when `Config::sources` is empty, the `backend` key expands into one
/// [`SourceKind`]. New configurations use the `sources` list instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum BackendKind {
    /// Kernel neighbour table only (unprivileged, best-effort).
    #[default]
    Neigh,
    /// In-process ARP sweep (needs `CAP_NET_RAW`).
    Pnet,
}

impl BackendKind {
    pub const ALL: [BackendKind; 2] = [BackendKind::Neigh, BackendKind::Pnet];

    pub fn label(&self) -> &'static str {
        match self {
            BackendKind::Neigh => "neigh",
            BackendKind::Pnet => "pnet",
        }
    }
}

/// A discovery **source**: one protocol, scheduled at its own cadence.
///
/// Unlike [`BackendKind`] (one pick, one scanner), several sources run at once
/// and each reports at its own interval. ARP-family sources find hard identities
/// (MAC); mDNS and SSDP are chatty and cheap, so they run fast and enrich the
/// same inventory with names and models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    /// Kernel neighbour table (+ optional TCP sweep, unprivileged).
    #[default]
    Neigh,
    /// In-process raw-socket ARP sweep (needs `CAP_NET_RAW`).
    ///
    /// Not in the default set, because it needs privileges. Older
    /// configurations that named the removed external `arp-scan` source land
    /// here rather than failing to load.
    #[serde(alias = "arp-scan", alias = "arpscan", alias = "arp", alias = "external")]
    Pnet,
    /// mDNS / DNS-SD multicast browse. No privileges.
    Mdns,
    /// SSDP / UPnP M-SEARCH. No privileges.
    Ssdp,
    /// L3 TCP sweep of explicitly configured subnets (crosses routers).
    Sweep,
    /// Ports and fingerprints on addresses that are already known.
    ///
    /// The only source that describes what the others found, in-process: it
    /// consumes the inventory (targets come from it) and reads banners itself.
    Probe,
    /// Version detection on known addresses by running the `nmap` binary.
    ///
    /// A second probing method next to [`SourceKind::Probe`], for operators who
    /// would rather trust nmap's service database and scripts. Needs nmap in
    /// `PATH`; when it is missing the source is skipped and the internal probe
    /// carries on.
    Nmap,
}

/// Which part of the scan a source belongs to.
///
/// The pipeline has two jobs — *find addresses*, then *describe them* — and this
/// names the first split. mDNS and SSDP belong to the first job like everything
/// else: they add addresses to the inventory, they just learn them from
/// announcements instead of sweeping a range. That difference is real but it is
/// a *capability* (does a source enumerate a segment?), not a stage, and nothing
/// needs it yet — so it is deliberately not encoded here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    /// Finds addresses: ARP/neighbour sweeps over known segments, and the
    /// multicast protocols that hear devices announce themselves.
    Presence,
    /// Describes addresses that are already known: ports, service versions,
    /// certificates, web titles. No source is here yet — the port/fingerprint
    /// pass still runs inside the aggregator and moves here next.
    Probe,
}

impl Phase {
    pub const ALL: [Phase; 2] = [Phase::Presence, Phase::Probe];

    pub fn label(&self) -> &'static str {
        match self {
            Phase::Presence => "presence",
            Phase::Probe => "probe",
        }
    }
}

impl SourceKind {
    pub const ALL: [SourceKind; 7] = [
        SourceKind::Neigh,
        SourceKind::Pnet,
        SourceKind::Mdns,
        SourceKind::Ssdp,
        SourceKind::Sweep,
        SourceKind::Probe,
        SourceKind::Nmap,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            SourceKind::Neigh => "neigh",
            SourceKind::Pnet => "pnet",
            SourceKind::Mdns => "mdns",
            SourceKind::Ssdp => "ssdp",
            SourceKind::Sweep => "sweep",
            SourceKind::Probe => "probe",
            SourceKind::Nmap => "nmap",
        }
    }

    /// The phase this source belongs to.
    ///
    /// Everything but the probes finds addresses; the probes describe the ones
    /// that were found.
    pub const fn phase(&self) -> Phase {
        match self {
            SourceKind::Probe | SourceKind::Nmap => Phase::Probe,
            _ => Phase::Presence,
        }
    }

    /// The inverse of [`SourceKind::label`], for rows read back from storage.
    pub fn from_label(label: &str) -> Option<SourceKind> {
        SourceKind::ALL
            .into_iter()
            .find(|kind| kind.label() == label)
    }
}

impl std::fmt::Display for SourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

impl std::str::FromStr for SourceKind {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "neigh" | "neighbor" | "neighbour" | "proc" => Ok(SourceKind::Neigh),
            // The external `arp-scan` source is gone; its names now select the
            // built-in ARP sweep, so an old saved `sources` list still loads.
            "pnet" | "raw" | "builtin" | "arp-scan" | "arpscan" | "arp" | "external" => {
                Ok(SourceKind::Pnet)
            }
            "mdns" | "dns-sd" | "bonjour" => Ok(SourceKind::Mdns),
            "ssdp" | "upnp" | "upn" => Ok(SourceKind::Ssdp),
            "sweep" | "l3" | "subnet" => Ok(SourceKind::Sweep),
            "nmap" => Ok(SourceKind::Nmap),
            other => Err(Error::parse(
                "source",
                format!(
                    "unknown source `{other}` (expected one of: {})",
                    SourceKind::ALL
                        .iter()
                        .map(SourceKind::label)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )),
        }
    }
}

impl From<BackendKind> for SourceKind {
    fn from(backend: BackendKind) -> Self {
        match backend {
            BackendKind::Neigh => SourceKind::Neigh,
            BackendKind::Pnet => SourceKind::Pnet,
        }
    }
}

impl std::fmt::Display for BackendKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

impl std::str::FromStr for BackendKind {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "neigh" | "neighbor" | "neighbour" | "proc" => Ok(BackendKind::Neigh),
            "pnet" | "raw" | "builtin" => Ok(BackendKind::Pnet),
            other => Err(Error::parse(
                "backend",
                format!("unknown backend `{other}` (expected: neigh, pnet)"),
            )),
        }
    }
}

/// Normalise a raw observation list: drop untrackable MACs, decide which
/// interface an address belongs to when several backends disagree, and collapse
/// duplicate MACs.
///
/// Preference order when the same MAC appears twice: an entry that carries an IP
/// wins; otherwise the first sighting is kept.
/// Normalise a raw observation list: collapse duplicate addresses.
///
/// Preference when the same address appears twice: a sighting that carries a MAC
/// wins over one that does not, so a later L3/mDNS sighting never erases a
/// hardware address an ARP source already learned.
pub fn normalize(observations: Vec<Observation>) -> Vec<Observation> {
    let mut by_ip: BTreeMap<std::net::Ipv4Addr, Observation> = BTreeMap::new();

    for obs in observations {
        match by_ip.get(&obs.ip) {
            Some(existing) if existing.mac.is_some() && obs.mac.is_none() => {}
            _ => {
                by_ip.insert(obs.ip, obs);
            }
        }
    }

    by_ip.into_values().collect()
}

/// Filter observations to the configured interfaces (empty means "all").
pub fn filter_ifaces(observations: Vec<Observation>, ifaces: &[String]) -> Vec<Observation> {
    if ifaces.is_empty() {
        return observations;
    }
    observations
        .into_iter()
        .filter(|obs| match obs.iface.as_deref() {
            Some(iface) => ifaces.iter().any(|wanted| wanted == iface),
            None => true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(mac: &str, ip: Option<&str>, iface: Option<&str>) -> Observation {
        Observation::new(
            Some(mac.parse().unwrap()),
            ip.unwrap_or("10.0.0.1").parse().unwrap(),
            iface.map(str::to_string),
        )
    }

    #[test]
    fn backend_kind_parses_aliases() {
        for (input, expected) in [
            ("neigh", BackendKind::Neigh),
            (" raw ", BackendKind::Pnet),
        ] {
            assert_eq!(input.parse::<BackendKind>().unwrap(), expected);
        }
        assert!("nope".parse::<BackendKind>().is_err());
        assert!(
            "arp-scan".parse::<BackendKind>().is_err(),
            "the external arp-scan backend is gone"
        );
    }

    #[test]
    fn normalize_dedupes_by_address_and_keeps_the_mac() {
        let result = normalize(vec![
            obs("d0:57:7e:35:f5:2f", Some("192.168.3.96"), Some("eth0")),
            // A later L3/mDNS sighting of the same host that has no MAC must not
            // erase the hardware address the ARP source learned.
            Observation::from_ip("192.168.3.96".parse().unwrap(), Some("eth0".into())),
            Observation::from_ip("1.2.3.4".parse().unwrap(), None),
        ]);
        assert_eq!(result.len(), 2);
        let kept = result
            .iter()
            .find(|obs| obs.ip == "192.168.3.96".parse::<std::net::Ipv4Addr>().unwrap())
            .unwrap();
        assert_eq!(
            kept.mac.map(|m| m.to_string()).as_deref(),
            Some("D0:57:7E:35:F5:2F"),
            "the entry carrying a MAC should win"
        );
    }

    #[test]
    fn filter_ifaces_keeps_matching_and_unscoped() {
        let input = vec![
            obs("aa:bb:cc:00:00:01", Some("10.0.0.1"), Some("eth0")),
            obs("aa:bb:cc:00:00:02", Some("10.0.1.1"), Some("wlan0")),
            obs("aa:bb:cc:00:00:03", Some("10.0.2.1"), None),
        ];
        let filtered = filter_ifaces(input, &["eth0".to_string()]);
        assert_eq!(filtered.len(), 2);

        let all = filter_ifaces(
            vec![obs("aa:bb:cc:00:00:02", Some("10.0.1.1"), Some("wlan0"))],
            &[],
        );
        assert_eq!(all.len(), 1);
    }
}
