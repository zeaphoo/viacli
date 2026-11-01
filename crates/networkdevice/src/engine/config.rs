//! Behaviour settings, and the few bootstrap values that cannot live in the
//! database.
//!
//! There is no configuration file and no environment override for behaviour:
//! the settings live in the `settings` table, they are the single source of
//! truth, and the service picks up changes on its next cycle without a restart.
//! [`Bootstrap`] is the exception — the database path, the listen address and
//! the log level must be known *before* the database can be opened, so they come
//! from the command line.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::engine::discovery::SourceKind;
use crate::engine::error::{Error, Result};
use crate::engine::ports::PortSpec;

/// Scan intensity: how hard the sources work, 1 (gentle) to 5 (aggressive).
///
/// There is no per-source schedule to configure — a single preset scales every
/// source's period. Level 3 is the default: the numbers in [`base_interval`] are
/// the periods at level 3, and the factors around it.
pub const DEFAULT_INTENSITY: u8 = 3;
pub const MIN_INTENSITY: u8 = 1;
pub const MAX_INTENSITY: u8 = 5;

/// Shortest period any source is allowed to run at, whatever the preset.
const MIN_INTERVAL_SECS: u64 = 5;

/// The period of each source at intensity 3, in seconds.
fn base_interval(kind: SourceKind) -> u64 {
    match kind {
        // Active sweeps: the accurate, heavier backbone.
        SourceKind::Pnet | SourceKind::Sweep => 120,
        // A free file read, but there is no point doing it often.
        SourceKind::Neigh => 600,
        // Multicast browsing is cheap; mDNS gets the fastest heartbeat.
        SourceKind::Mdns => 15,
        SourceKind::Ssdp => 120,
        // A bounded batch of port scans and fingerprints per cycle. Slower than
        // the sweeps that feed it, which is the point: it only ever looks at
        // addresses the sweeps already found alive.
        SourceKind::Probe => 60,
        // nmap runs its own scan per batch, so it gets a slower heartbeat than
        // the in-process probe.
        SourceKind::Nmap => 300,
    }
}

/// The sources a default installation polls.
///
/// The raw-socket `pnet` sweep is **not** here: it needs `CAP_NET_RAW`, so it is
/// opt-in. The defaults are the unprivileged ones. The probe is also absent — it
/// is the identification pass, and [`Config::enabled_sources`] adds it whenever
/// identification is on.
pub fn default_sources() -> Vec<SourceKind> {
    vec![
        SourceKind::Neigh,
        SourceKind::Mdns,
        SourceKind::Ssdp,
    ]
}

/// Everything the engine behaves like. Stored in the `settings` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct Config {
    // ---- what to scan ----
    /// Interfaces to scan. Empty means auto: the interface carrying the default
    /// route, else every non-loopback IPv4 interface.
    pub ifaces: Vec<String>,
    /// Scan intensity preset, 1..=5. Scales every source's period.
    pub intensity: u8,
    /// Which discovery sources to run. Empty means the defaults.
    pub sources: Vec<SourceKind>,

    // ---- legacy: the removed external `arp-scan` source ----
    //
    // Nothing reads these any more. They are still accepted (but no longer
    // written) so a database or export from an older version loads instead of
    // failing on an unknown field; the next save drops the rows.
    #[serde(skip_serializing)]
    pub arp_scan_cmd: String,
    #[serde(skip_serializing)]
    pub arp_args: Vec<String>,
    #[serde(skip_serializing)]
    pub arp_iface_flag: String,
    #[serde(skip_serializing)]
    pub arp_raw: Vec<String>,

    // ---- neigh source ----
    /// Fold in the kernel's neighbour table. It costs one file read and no
    /// privileges, and can only add hosts the active scan missed.
    pub neigh_baseline: bool,
    /// Force the kernel's neighbour table to fill up with a TCP connect sweep.
    pub neigh_sweep: bool,
    pub neigh_sweep_timeout_ms: u64,
    pub neigh_concurrency: usize,
    /// Cap on swept addresses, so a /16 bridge cannot make a scan crawl.
    pub neigh_max_targets: usize,

    // ---- pnet source ----
    pub pnet_reply_timeout_ms: u64,
    pub pnet_request_interval_ms: u64,
    pub pnet_max_targets: usize,
    /// Passes over the target list. More passes catch slow hosts.
    pub pnet_retries: u32,
    /// Tag outgoing ARP frames with this 802.1Q VLAN identifier.
    pub pnet_vlan_id: Option<u16>,
    /// Put this address in the ARP "sender" field instead of the interface's.
    pub pnet_source_ip: Option<std::net::Ipv4Addr>,

    // ---- mdns / ssdp sources ----
    /// How long the mDNS browse listens after each round of questions.
    pub mdns_listen_ms: u64,
    /// How long the SSDP M-SEARCH listens for answers.
    pub ssdp_listen_ms: u64,
    /// Fetch the UPnP description XML (friendlyName/modelName) after answering.
    pub ssdp_descriptions: bool,

    // ---- sweep source ----
    /// Extra subnets to sweep over L3, e.g. `192.168.2.0/24`.
    pub extra_subnets: Vec<String>,
    /// Ports tried per swept address. One completed handshake means "alive".
    /// Ports the L3 sweep tries per address. A mixed list of numbers and group
    /// names (`web`, `db`, `top`, `all`).
    pub sweep_ports: Vec<PortSpec>,
    pub sweep_timeout_ms: u64,
    pub sweep_concurrency: usize,
    /// Cap on expanded addresses, so a `/16` cannot turn one poll into 65k
    /// connects.
    pub sweep_max_targets: usize,

    // ---- multi-source reconciliation ----
    /// How long a device may go unseen before it is marked offline. `None`
    /// derives it from the slowest enabled source (twice its interval, min 60s).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offline_after_secs: Option<u64>,

    // ---- identification ----
    pub resolve_dns: bool,
    /// Enable the TCP port probe (touches every discovered host).
    pub port_probe: bool,
    pub probe_ports: Vec<PortSpec>,
    pub probe_timeout_ms: u64,
    /// Cap on how many devices are identified per scan.
    pub identify_budget: usize,
    /// Re-probe an address this many seconds after the last probe. `None` probes
    /// each address once and never again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub probe_ttl_secs: Option<u64>,

    // ---- retention ----
    /// Drop events older than this many hours. `None` keeps everything.
    pub trim_hist_hours: Option<u32>,
    /// Snapshot endpoints into `endpoints_history` this often. 0 disables it.
    pub history_sample_secs: u64,
    /// Drop samples older than this many hours. `None` keeps everything.
    pub history_retain_hours: Option<u32>,

    /// Override the embedded OUI table with an IEEE registry CSV.
    pub oui_file: Option<PathBuf>,
    /// Add probes from an `nmap-service-probes` file, tried before the built-in
    /// table. The file is nmap's format; its data stays yours to supply.
    pub probes_file: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            ifaces: Vec::new(),
            intensity: DEFAULT_INTENSITY,
            sources: Vec::new(),

            arp_scan_cmd: "arp-scan".to_string(),
            arp_args: vec!["-glNx".to_string()],
            arp_iface_flag: "-I".to_string(),
            arp_raw: Vec::new(),

            neigh_baseline: true,
            neigh_sweep: true,
            neigh_sweep_timeout_ms: 300,
            neigh_concurrency: 64,
            neigh_max_targets: 1024,

            pnet_reply_timeout_ms: 1200,
            pnet_request_interval_ms: 1,
            pnet_max_targets: 4096,
            pnet_retries: 1,
            pnet_vlan_id: None,
            pnet_source_ip: None,

            mdns_listen_ms: 500,
            ssdp_listen_ms: 2500,
            ssdp_descriptions: true,

            extra_subnets: Vec::new(),
            sweep_ports: crate::engine::ports::numbers(&crate::engine::discovery::sweep::DEFAULT_SWEEP_PORTS),
            sweep_timeout_ms: 500,
            sweep_concurrency: 128,
            sweep_max_targets: 4096,

            offline_after_secs: None,

            resolve_dns: true,
            port_probe: true,
            probe_ports: vec![PortSpec::Group("common".to_string())],
            probe_timeout_ms: 250,
            identify_budget: 64,
            // Six hours: long enough to be invisible in traffic, short enough
            // that a service which starts later is noticed the same day.
            probe_ttl_secs: Some(6 * 60 * 60),

            trim_hist_hours: None,
            history_sample_secs: 300,
            history_retain_hours: Some(720),

            oui_file: None,
            probes_file: None,
        }
    }
}

impl Config {
    /// Defaults overlaid with the JSON values stored in the `settings` table.
    ///
    /// A row whose value is not valid JSON is an error rather than a silent
    /// fallback: the database is the single source of truth, so a broken value
    /// must be loud.
    pub fn from_settings(rows: &[(String, String)]) -> Result<Config> {
        let mut value = serde_json::to_value(Config::default())?;
        if let serde_json::Value::Object(map) = &mut value {
            for (key, raw) in rows {
                let parsed: serde_json::Value = serde_json::from_str(raw).map_err(|e| {
                    Error::parse("settings", format!("`{key}` is not valid JSON: {e}"))
                })?;
                map.insert(key.clone(), parsed);
            }
        }
        serde_json::from_value(value).map_err(Error::from)
    }

    /// One JSON value per top-level field, ready to store.
    pub fn to_settings(&self) -> Result<Vec<(String, String)>> {
        let value = serde_json::to_value(self)?;
        let map = value.as_object().cloned().unwrap_or_default();
        map.into_iter()
            .map(|(key, value)| Ok((key, serde_json::to_string(&value)?)))
            .collect()
    }

    /// The whole configuration as a JSON document (for export).
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse an exported document (for import). Unknown keys are rejected.
    pub fn from_json(text: &str) -> Result<Config> {
        Ok(serde_json::from_str(text)?)
    }

    // ---------------------------------------------------------------- sources

    /// Every source that will actually be polled.
    ///
    /// A non-empty `extra_subnets` adds the `sweep` source, so listing a subnet
    /// is enough to make it run.
    pub fn resolved_sources(&self) -> Vec<SourceKind> {
        let mut kinds = if self.sources.is_empty() {
            default_sources()
        } else {
            self.sources.clone()
        };

        if !self.extra_subnets.is_empty() && !kinds.contains(&SourceKind::Sweep) {
            kinds.push(SourceKind::Sweep);
        }

        kinds
    }

    /// Enabled sources, with `neigh_baseline = false` dropping the neighbour
    /// source even when it is listed.
    pub fn enabled_sources(&self) -> Vec<SourceKind> {
        let mut kinds: Vec<SourceKind> = self
            .resolved_sources()
            .into_iter()
            .filter(|kind| self.neigh_baseline || *kind != SourceKind::Neigh)
            .filter(|kind| *kind != SourceKind::Probe)
            .collect();

        // The probe follows the identification switches rather than the source
        // list: one control per thing, and an install whose saved `sources` list
        // predates the probe keeps identifying instead of silently stopping.
        if self.resolve_dns || self.port_probe {
            kinds.push(SourceKind::Probe);
        }
        // 本采集器未编译 nmap 源（按需求移除）。
        kinds
    }

    /// Whether one source is currently on.
    pub fn source_enabled(&self, kind: SourceKind) -> bool {
        self.enabled_sources().contains(&kind)
    }

    /// The clamp used everywhere, so a hand-edited value cannot break scheduling.
    pub fn intensity(&self) -> u8 {
        self.intensity.clamp(MIN_INTENSITY, MAX_INTENSITY)
    }

    /// How often one source polls, given the intensity preset.
    pub fn source_interval(&self, kind: SourceKind) -> Duration {
        let base = base_interval(kind);
        let secs = match self.intensity() {
            1 => base * 8,
            2 => base * 4,
            3 => base,
            4 => base / 2,
            _ => base / 4,
        };
        Duration::from_secs(secs.max(MIN_INTERVAL_SECS))
    }

    /// How long a device may go unseen before it is marked offline.
    pub fn offline_after(&self) -> Duration {
        if let Some(secs) = self.offline_after_secs {
            return Duration::from_secs(secs.max(1));
        }
        let slowest = self
            .enabled_sources()
            .iter()
            .map(|kind| self.source_interval(*kind))
            .max()
            .unwrap_or_else(|| Duration::from_secs(120));
        (slowest * 2).max(Duration::from_secs(60))
    }

    /// How often to snapshot endpoints, or `None` when sampling is off.
    pub fn history_sample(&self) -> Option<Duration> {
        (self.history_sample_secs > 0).then(|| Duration::from_secs(self.history_sample_secs))
    }

    /// How long samples are kept, or `None` to keep everything.
    pub fn history_retain(&self) -> Option<chrono::Duration> {
        self.history_retain_hours
            .map(|hours| chrono::Duration::hours(i64::from(hours)))
    }

    pub fn mdns_listen(&self) -> Duration {
        Duration::from_millis(self.mdns_listen_ms.max(1))
    }

    pub fn ssdp_listen(&self) -> Duration {
        Duration::from_millis(self.ssdp_listen_ms.max(1))
    }

    pub fn sweep_timeout(&self) -> Duration {
        Duration::from_millis(self.sweep_timeout_ms.max(1))
    }

    /// The L3 sweep's ports, with any group names expanded.
    pub fn sweep_port_list(&self) -> Vec<u16> {
        crate::engine::ports::expand(&self.sweep_ports)
    }

    /// The port probe's ports, with any group names expanded.
    pub fn probe_port_list(&self) -> Vec<u16> {
        crate::engine::ports::expand(&self.probe_ports)
    }

    pub fn probe_timeout(&self) -> Duration {
        Duration::from_millis(self.probe_timeout_ms.max(1))
    }

    pub fn neigh_sweep_timeout(&self) -> Duration {
        Duration::from_millis(self.neigh_sweep_timeout_ms.max(1))
    }

    pub fn pnet_reply_timeout(&self) -> Duration {
        Duration::from_millis(self.pnet_reply_timeout_ms.max(1))
    }

    pub fn pnet_request_interval(&self) -> Duration {
        Duration::from_millis(self.pnet_request_interval_ms)
    }
}

// ------------------------------------------------------------------ bootstrap

/// How much the service logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
}

impl LogLevel {
    pub fn label(&self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
        }
    }

    /// Whether a message at `level` should be printed.
    pub fn allows(&self, level: LogLevel) -> bool {
        level <= *self
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

impl std::str::FromStr for LogLevel {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "error" => Ok(LogLevel::Error),
            "warn" | "warning" => Ok(LogLevel::Warn),
            "info" => Ok(LogLevel::Info),
            "debug" => Ok(LogLevel::Debug),
            other => Err(Error::parse(
                "log level",
                format!("unknown log level `{other}` (expected: error, warn, info, debug)"),
            )),
        }
    }
}

/// The values that must be known before the database can be opened.
///
/// Everything else lives in the database; these three come from the command
/// line because there is no way to read them out of a database that has not
/// been opened yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bootstrap {
    pub database: PathBuf,
    pub bind: String,
    pub log_level: LogLevel,
}

impl Default for Bootstrap {
    fn default() -> Self {
        Bootstrap {
            database: default_database_path(),
            bind: default_bind(),
            log_level: LogLevel::Info,
        }
    }
}

pub fn default_bind() -> String {
    "127.0.0.1:6200".to_string()
}

/// `$XDG_DATA_HOME/toposight/toposight.db`, falling back to `$HOME/.local/share`
/// and finally to the working directory.
pub fn default_database_path() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(xdg).join("toposight").join("toposight.db");
    }
    if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("toposight")
            .join("toposight.db");
    }
    PathBuf::from("toposight.db")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let config = Config::default();
        assert_eq!(config.intensity, 3, "the default preset is the middle one");
        assert_eq!(
            config.enabled_sources(),
            vec![
                SourceKind::Neigh,
                SourceKind::Mdns,
                SourceKind::Ssdp,
                SourceKind::Probe
            ]
        );
        assert!(config.resolve_dns);
        // 本采集器以端口发现为目的，默认开启主动探测（与上游的 opt-in 默认不同）。
        assert!(config.port_probe);
        assert_eq!(config.identify_budget, 64);
    }

    #[test]
    fn intensity_scales_every_source_period() {
        let mut config = Config::default();
        assert_eq!(
            config.source_interval(SourceKind::Mdns),
            Duration::from_secs(15)
        );
        assert_eq!(
            config.source_interval(SourceKind::Pnet),
            Duration::from_secs(120)
        );
        assert_eq!(
            config.source_interval(SourceKind::Neigh),
            Duration::from_secs(600)
        );

        config.intensity = 1;
        assert_eq!(
            config.source_interval(SourceKind::Pnet),
            Duration::from_secs(960)
        );

        config.intensity = 5;
        assert_eq!(
            config.source_interval(SourceKind::Mdns),
            Duration::from_secs(5)
        );
        assert_eq!(
            config.source_interval(SourceKind::Pnet),
            Duration::from_secs(30)
        );

        config.intensity = 99;
        assert_eq!(config.intensity(), MAX_INTENSITY);
        config.intensity = 0;
        assert_eq!(config.intensity(), MIN_INTENSITY);
    }

    #[test]
    fn extra_subnets_enable_the_sweep_source_automatically() {
        let config = Config {
            extra_subnets: vec!["10.0.0.0/24".into()],
            ..Config::default()
        };
        assert!(config.source_enabled(SourceKind::Sweep));

        let config = Config {
            sources: vec![SourceKind::Sweep],
            extra_subnets: vec!["10.0.0.0/24".into()],
            ..Config::default()
        };
        let sweeps = config
            .enabled_sources()
            .iter()
            .filter(|kind| **kind == SourceKind::Sweep)
            .count();
        assert_eq!(sweeps, 1);
    }

    #[test]
    fn the_probe_runs_when_identification_is_on() {
        let config = Config {
            port_probe: true,
            ..Config::default()
        };
        let kinds = config.enabled_sources();
        assert!(kinds.contains(&SourceKind::Probe));

        // Resolve DNS alone is enough to enable the (passive) probe.
        let passive = Config {
            resolve_dns: true,
            port_probe: false,
            ..Config::default()
        };
        assert!(passive.enabled_sources().contains(&SourceKind::Probe));
        // 未编译 nmap 源：任何情况下都不应出现。
        assert!(!config.enabled_sources().contains(&SourceKind::Nmap));
    }

    #[test]
    fn the_neighbour_source_can_be_switched_off() {
        let config = Config {
            neigh_baseline: false,
            ..Config::default()
        };
        assert!(!config.source_enabled(SourceKind::Neigh));
        assert!(config.source_enabled(SourceKind::Mdns));
    }

    #[test]
    fn offline_window_follows_the_slowest_source() {
        let config = Config::default();
        assert_eq!(config.offline_after(), Duration::from_secs(1200));

        let config = Config {
            offline_after_secs: Some(45),
            ..Config::default()
        };
        assert_eq!(config.offline_after(), Duration::from_secs(45));
    }

    #[test]
    fn history_sampling_is_bounded_by_default() {
        let config = Config::default();
        assert_eq!(config.history_sample(), Some(Duration::from_secs(300)));
        assert_eq!(config.history_retain(), Some(chrono::Duration::hours(720)));

        let config = Config {
            history_sample_secs: 0,
            history_retain_hours: None,
            ..Config::default()
        };
        assert_eq!(config.history_sample(), None, "0 disables sampling");
        assert_eq!(config.history_retain(), None);
    }

    #[test]
    fn settings_round_trip_through_the_database_shape() {
        let config = Config {
            intensity: 5,
            sources: vec![SourceKind::Mdns, SourceKind::Sweep],
            extra_subnets: vec!["10.0.0.0/24".into()],
            port_probe: true,
            ..Config::default()
        };

        let rows = config.to_settings().unwrap();
        assert!(rows.iter().any(|(key, _)| key == "intensity"));

        let restored = Config::from_settings(&rows).unwrap();
        assert_eq!(restored, config);
    }

    #[test]
    fn absent_settings_fall_back_to_defaults() {
        let restored = Config::from_settings(&[]).unwrap();
        assert_eq!(restored, Config::default());
    }

    #[test]
    fn clearing_a_setting_deletes_its_row() {
        let mut config = Config {
            offline_after_secs: Some(90),
            ..Config::default()
        };
        let rows = config.to_settings().unwrap();
        assert!(rows.iter().any(|(key, _)| key == "offlineAfterSecs"));

        config.offline_after_secs = None;
        let rows = config.to_settings().unwrap();
        assert!(
            !rows.iter().any(|(key, _)| key == "offlineAfterSecs"),
            "a cleared option should not be stored"
        );
    }

    #[test]
    fn a_broken_setting_is_loud() {
        let rows = vec![("intensity".to_string(), "not json".to_string())];
        assert!(Config::from_settings(&rows).is_err());
    }

    #[test]
    fn export_and_import_round_trip() {
        let config = Config {
            intensity: 1,
            extra_subnets: vec!["192.168.9.0/24".into()],
            ..Config::default()
        };
        let text = config.to_json().unwrap();
        assert_eq!(Config::from_json(&text).unwrap(), config);
    }

    #[test]
    fn unknown_keys_are_rejected_so_typos_are_loud() {
        assert!(Config::from_json("{\"intensitty\": 3}").is_err());
    }

    #[test]
    fn log_levels_parse_and_gate() {
        assert_eq!("debug".parse::<LogLevel>().unwrap(), LogLevel::Debug);
        assert!("verbose".parse::<LogLevel>().is_err());
        assert!(LogLevel::Info.allows(LogLevel::Warn));
        assert!(!LogLevel::Error.allows(LogLevel::Info));
    }

    #[test]
    fn bootstrap_has_usable_defaults() {
        let bootstrap = Bootstrap::default();
        assert_eq!(bootstrap.bind, default_bind());
        assert!(bootstrap.database.to_string_lossy().ends_with(".db"));
    }
}
