//! Building the discovery sources a service polls.
//!
//! Kept separate from the service loop so the mapping from configuration to
//! concrete [`Discovery`] implementations is testable on its own.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::engine::config::Config;
use crate::engine::discovery::{self, Discovery, SourceKind};
use crate::engine::error::Result;
use crate::engine::fingerprint::tcp::Engine;
use crate::engine::identify::{Identifier, PortProbe};
use crate::engine::model::ProbeTarget;
use crate::engine::oui::OuiDb;
use crate::engine::probe::ProbeDiscovery;

/// Build the concrete [`Discovery`] implementation for one source kind.
///
/// Called on every cycle by the service's source thread, with whatever the
/// current settings say — which is what makes configuration changes take effect
/// on the next run without restarting anything. Construction is cheap; the work
/// is in `scan()`.
pub(crate) fn build_source(
    kind: SourceKind,
    config: &Config,
    targets: &[ProbeTarget],
) -> Result<Arc<dyn Discovery>> {
    let discovery: Arc<dyn Discovery> = match kind {
        SourceKind::Neigh => {
            let discovery = discovery::NeighborDiscovery::new(config.ifaces.clone())
                .with_sweep(config.neigh_sweep)
                .with_connect_timeout(config.neigh_sweep_timeout())
                .with_concurrency(config.neigh_concurrency)
                .with_max_targets(config.neigh_max_targets);
            Arc::new(discovery)
        }
        SourceKind::Pnet => {
            return Err(crate::engine::error::Error::unsupported(
                "raw ARP (pnet) 源未编译进本采集器",
            ));
        }
        SourceKind::Mdns => Arc::new(
            discovery::MdnsDiscovery::new(config.ifaces.clone()).with_listen(config.mdns_listen()),
        ),
        SourceKind::Ssdp => Arc::new(
            discovery::SsdpDiscovery::new(config.ifaces.clone())
                .with_listen(config.ssdp_listen())
                .with_descriptions(config.ssdp_descriptions),
        ),
        SourceKind::Probe => {
            // The port probe is the consent switch for everything the probe
            // does to a host, so it also decides whether the web and SNMP
            // probes may run at all.
            let identifier = if config.port_probe {
                Identifier::with_probe(
                    config.resolve_dns,
                    PortProbe::new(config.probe_port_list())
                        .with_timeout(config.probe_timeout()),
                )
            } else {
                Identifier::passive(config.resolve_dns)
            };
            Arc::new(ProbeDiscovery::new(
                targets.to_vec(),
                identifier,
                config.probe_timeout(),
                probe_engine(config.probes_file.as_deref()),
            ))
        }
        SourceKind::Sweep => Arc::new(
            discovery::SweepDiscovery::new(config.extra_subnets.clone())
                .with_ports(config.sweep_port_list())
                .with_timeout(config.sweep_timeout())
                .with_concurrency(config.sweep_concurrency)
                .with_max_targets(config.sweep_max_targets),
        ),
        SourceKind::Nmap => {
            return Err(crate::engine::error::Error::unsupported(
                "nmap 源未编译进本采集器",
            ));
        }
    };

    Ok(discovery)
}

/// The probe table, parsed once per configured file and reused for the life of
/// the process.
///
/// Compiling a full `nmap-service-probes` runs into seconds, and the service
/// rebuilds its sources every cycle to pick up setting changes; without this the
/// table would be recompiled on every probe pass. A setting change to a new path
/// is a new cache key, so the new file is loaded on the next cycle.
fn probe_engine(path: Option<&Path>) -> Arc<Engine> {
    static CACHE: OnceLock<Mutex<HashMap<Option<PathBuf>, Arc<Engine>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = path.map(Path::to_path_buf);
    let mut engines = cache.lock().expect("probe engine cache");
    engines
        .entry(key)
        .or_insert_with(|| Arc::new(Engine::load(path)))
        .clone()
}

/// Load the OUI table: a custom IEEE CSV when configured, else the embedded one.
pub fn load_oui(config: &Config) -> Result<OuiDb> {
    match config.oui_file.as_ref() {
        Some(path) => OuiDb::from_ieee_csv(path),
        None => Ok(OuiDb::embedded().clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_source_builds_and_describes_itself() {
        let config = Config::default();
        for kind in config.enabled_sources() {
            let discovery =
                build_source(kind, &config, &[]).unwrap_or_else(|e| panic!("{kind}: {e}"));
            assert!(!discovery.describe().is_empty(), "{kind}");
        }
    }

    #[test]
    fn an_explicitly_configured_source_builds() {
        let config = Config {
            sources: vec![SourceKind::Mdns, SourceKind::Sweep],
            extra_subnets: vec!["192.168.3.0/24".into()],
            ..Config::default()
        };
        for kind in [SourceKind::Mdns, SourceKind::Sweep] {
            assert!(build_source(kind, &config, &[]).is_ok(), "{kind}");
        }
    }

    #[test]
    fn load_oui_uses_the_embedded_table() {
        let oui = load_oui(&Config::default()).unwrap();
        assert!(oui.lookup(&"d0:57:7e:35:f5:2f".parse().unwrap()).is_some());
    }

    #[test]
    fn the_probe_engine_is_parsed_once_per_path() {
        let first = probe_engine(None);
        let second = probe_engine(None);
        assert!(Arc::ptr_eq(&first, &second), "the table must be reused");
    }
}
