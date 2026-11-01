//! 采集逻辑：跑引擎的发现源 → 并入清点 → 产出 keendeck 协议消息。
//!
//! 引擎（`crate::engine`）负责"扫描与清点"的全部逻辑；这一层只做三件事：
//!
//! 1. **调度**：按配置里各发现源自己的间隔（`source_interval`）决定这一轮跑哪些源；
//!    每个源上次运行的时刻持久化在自管库里，跨进程重启也记得。
//! 2. **归并**：把各源产出的观测交给 `Inventory::observe`，再应用 OUI 厂商、
//!    按 `offline_after` 把超时未见的端点标记离线。
//! 3. **产出**：把端点/设备/事件的**变化**映射成协议记录，并用自管库做差分，
//!    避免每轮把整份清单重发（宿主的存储是只追加的）。

use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::task::JoinSet;

use crate::emit;
use crate::engine::config::Config;
use crate::engine::discovery::{Phase, ScanOutcome, SourceKind};
use crate::engine::inventory::{Device, Endpoint, Inventory};
use crate::engine::oui::OuiDb;
use crate::engine::sources::{build_source, load_oui};
use crate::protocol::Sink;
use crate::store::Store;

/// 宿主施加的产出上限（§4.5）。
#[derive(Debug, Clone, Copy, Default)]
pub struct EmitLimits {
    pub max_lines: Option<u64>,
    pub max_bytes: Option<u64>,
}

impl EmitLimits {
    pub fn hit<W: Write>(&self, sink: &Sink<W>) -> bool {
        self.max_lines.is_some_and(|m| sink.emitted() >= m)
            || self.max_bytes.is_some_and(|m| sink.bytes() >= m)
    }
}

/// 一轮的结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CycleStats {
    pub sources_run: usize,
    pub observations: usize,
    pub endpoints_emitted: usize,
    pub devices_emitted: usize,
    pub events: usize,
    /// 是否因为触碰 `limits`/被要求停止而提前收尾。
    pub stopped: bool,
}

pub struct Collector {
    cfg: Config,
    inv: Inventory,
    oui: OuiDb,
    store: Store,
    datasource: String,
    stop: Arc<AtomicBool>,
    /// 上次产出到宿主的时刻（按实体）。
    endpoint_emitted: HashMap<String, i64>,
    device_emitted: HashMap<String, i64>,
    /// 上次看到的内容签名（忽略 last_seen 这类易变字段）。
    endpoint_sig: HashMap<String, String>,
    device_sig: HashMap<String, String>,
    /// 每个发现源上次运行的时刻（毫秒）。
    source_runs: HashMap<String, i64>,
}

impl Collector {
    /// 打开自管库、装载 OUI 与清点，任何一步失败都在扫描前报出来。
    pub fn new(cfg: Config, datasource: String, db_path: &std::path::Path) -> Result<Collector, String> {
        let oui = load_oui(&cfg).map_err(|e| format!("加载 OUI 厂商库失败: {e}"))?;
        let store = Store::open(db_path)?;
        let loaded = store.load()?;
        eprintln!(
            "viacli-networkdevice: 自管清点数据库 {}（已登记 {} 端点 / {} 设备）",
            db_path.display(),
            loaded.inventory.endpoints().count(),
            loaded.inventory.devices().count()
        );

        let mut endpoint_sig = HashMap::new();
        for e in loaded.inventory.endpoints() {
            endpoint_sig.insert(e.ip.to_string(), endpoint_signature(e));
        }
        let mut device_sig = HashMap::new();
        for d in loaded.inventory.devices() {
            device_sig.insert(d.id.clone(), device_signature(d));
        }

        Ok(Collector {
            cfg,
            inv: loaded.inventory,
            oui,
            store,
            datasource,
            stop: Arc::new(AtomicBool::new(false)),
            endpoint_emitted: loaded.endpoint_emitted,
            device_emitted: loaded.device_emitted,
            endpoint_sig,
            device_sig,
            source_runs: loaded.source_runs,
        })
    }

    pub fn set_stop(&mut self, flag: Arc<AtomicBool>) {
        self.stop = flag;
    }

    pub fn db_path(&self) -> String {
        self.store.path().to_string_lossy().to_string()
    }

    /// 进程内最小调度节拍：取所有启用源里最短的间隔，clamp 到 1s..10min。
    pub fn tick_ms(&self) -> u64 {
        self.cfg
            .enabled_sources()
            .iter()
            .map(|k| self.cfg.source_interval(*k).as_millis() as u64)
            .min()
            .unwrap_or(60_000)
            .clamp(1_000, 600_000)
    }

    pub fn last_cycle_ms(&self) -> Option<i64> {
        self.store.meta_i64("last_cycle_ms").ok().flatten()
    }

    /// 跑一轮：到期的源各扫一次，并入清点，然后落库 + 产出。
    pub async fn run_cycle<W: Write>(
        &mut self,
        sink: &mut Sink<W>,
        now: i64,
        deadline: Option<Instant>,
        limits: EmitLimits,
    ) -> io::Result<CycleStats> {
        emit::declare_series(sink)?;
        emit::emit_totals(sink, &self.inv, Some(now))?;

        let mut stats = CycleStats::default();
        if self.stop.load(Ordering::Relaxed) || limits.hit(sink) {
            stats.stopped = true;
            return Ok(stats);
        }

        let now_dt = DateTime::<Utc>::from_timestamp_millis(now).unwrap_or_else(Utc::now);
        let started = Instant::now();
        let refresh_ms = self.cfg.history_sample_secs as i64 * 1000;

        // 两相调度：先跑**发现地址**的源（neigh/mdns/ssdp/sweep），再跑**描述地址**的
        // 探测源。探测源的目标来自清点本身，所以必须在发现源之后跑，否则第一轮会
        // 因为没有目标而空转（上游用独立线程调度，这里用两相同样达到效果）。
        let due: Vec<SourceKind> = self
            .cfg
            .enabled_sources()
            .into_iter()
            .filter(|k| self.due(*k, now))
            .collect();
        let (presence, probes): (Vec<SourceKind>, Vec<SourceKind>) = due
            .into_iter()
            .partition(|k| k.phase() != Phase::Probe);

        for group in [presence, probes] {
            if group.is_empty() {
                continue;
            }
            let sub = self.run_sources(group, sink, now, now_dt, deadline).await?;
            stats.sources_run += sub.sources_run;
            stats.observations += sub.observations;
            if sub.stopped {
                stats.stopped = true;
                return Ok(stats);
            }
            if limits.hit(sink) {
                stats.stopped = true;
                return Ok(stats);
            }
        }

        // 厂商只看 MAC，所以每个有 MAC 的端点都在这里补上，不必等探测。
        self.inv.apply_oui(&self.oui, now_dt);

        // 超时未见的端点/设备标记离线。
        let window = chrono::Duration::from_std(self.cfg.offline_after())
            .unwrap_or_else(|_| chrono::Duration::seconds(60));
        self.inv.prune_stale(now_dt - window, now_dt);

        let events = self.inv.take_events();
        stats.events = events.len();

        // 落库（端点/设备/事件），并记住各源与整轮的时刻。
        self.store
            .save(&self.inv, &self.endpoint_emitted, &self.device_emitted, &events)
            .map_err(to_io)?;
        for (source, at) in &self.source_runs {
            self.store.set_source_run(source, *at).map_err(to_io)?;
        }
        self.store.set_meta_i64("last_cycle_ms", now).map_err(to_io)?;
        if let Some(hours) = self.cfg.trim_hist_hours {
            let _ = self
                .store
                .prune_events(now - hours as i64 * 3_600_000);
        }

        // 产出：先事件，再变化过的端点/设备快照。
        for event in &events {
            emit::emit_event(sink, event)?;
        }
        self.emit_snapshots(sink, now, refresh_ms, &mut stats)?;

        let duration_s = started.elapsed().as_secs_f64();
        sink.sample_i64(
            "networkdevice/sources_run",
            Some(now),
            stats.sources_run as i64,
        )?;
        sink.sample_i64(
            "networkdevice/observations",
            Some(now),
            stats.observations as i64,
        )?;
        sink.sample_f64(
            "networkdevice/cycle_duration_seconds",
            Some(now),
            duration_s,
        )?;
        sink.record(
            "networkdevice.scan",
            Some(&format!("{}#{}", self.datasource, now)),
            Some(now),
            serde_json::json!({
                "datasource": self.datasource,
                "sourcesRun": stats.sources_run,
                "observations": stats.observations,
                "endpointsKnown": self.inv.endpoints().count(),
                "endpointsOnline": self.inv.endpoints().filter(|e| e.online).count(),
                "devicesKnown": self.inv.devices().count(),
                "devicesOnline": self.inv.devices().filter(|d| d.online).count(),
                "events": stats.events,
                "durationMs": (duration_s * 1000.0).round() as i64,
                "database": self.db_path(),
            }),
        )?;
        Ok(stats)
    }

    /// 并发运行一组源，并把产出的观测并入清点。
    async fn run_sources<W: Write>(
        &mut self,
        kinds: Vec<SourceKind>,
        sink: &mut Sink<W>,
        now: i64,
        now_dt: DateTime<Utc>,
        deadline: Option<Instant>,
    ) -> io::Result<RunStat> {
        let mut tasks: JoinSet<(
            SourceKind,
            Result<ScanOutcome, crate::engine::error::Error>,
        )> = JoinSet::new();
        for kind in kinds {
            // 探测源的目标来自清点本身（先发现地址，再描述它们）。
            let targets = if kind.phase() == Phase::Probe {
                self.inv.probe_targets(
                    now_dt,
                    self.cfg.probe_ttl_secs,
                    self.cfg.identify_budget,
                )
            } else {
                Vec::new()
            };
            let cfg = self.cfg.clone();
            tasks.spawn_blocking(move || {
                let result = match build_source(kind, &cfg, &targets) {
                    Ok(discovery) => discovery.scan(),
                    Err(e) => Err(e),
                };
                (kind, result)
            });
        }

        let mut stat = RunStat::default();
        while let Some(joined) = next_source(&mut tasks, deadline, &self.stop).await {
            let Ok((kind, result)) = joined else {
                continue;
            };
            match result {
                Ok(outcome) => {
                    for observation in &outcome.observations {
                        self.inv.observe(observation, now_dt);
                    }
                    stat.observations += outcome.observations.len();
                    for warning in &outcome.warnings {
                        sink.log("debug", warning)?;
                    }
                }
                Err(e) => {
                    sink.log("warn", &format!("发现源 {kind} 失败: {e}"))?;
                }
            }
            self.source_runs.insert(kind.label().to_string(), now);
            stat.sources_run += 1;
        }
        if self.stop.load(Ordering::Relaxed) {
            stat.stopped = true;
        }
        Ok(stat)
    }

    /// 某个源这一轮是否到期。
    fn due(&self, kind: SourceKind, now: i64) -> bool {
        let interval = self.cfg.source_interval(kind).as_millis() as i64;
        match self.source_runs.get(kind.label()) {
            Some(last) if *last != 0 => now.saturating_sub(*last) >= interval,
            _ => true,
        }
    }

    /// 产出内容有变化、或距上次产出超过刷新窗口的端点/设备快照。
    fn emit_snapshots<W: Write>(
        &mut self,
        sink: &mut Sink<W>,
        now: i64,
        refresh_ms: i64,
        stats: &mut CycleStats,
    ) -> io::Result<()> {
        let endpoints: Vec<Endpoint> = self.inv.endpoints().cloned().collect();
        for endpoint in &endpoints {
            let ip = endpoint.ip.to_string();
            let sig = endpoint_signature(endpoint);
            let changed = self.endpoint_sig.get(&ip) != Some(&sig);
            let last = self.endpoint_emitted.get(&ip).copied().unwrap_or(0);
            if changed || now.saturating_sub(last) >= refresh_ms {
                let status = if self.endpoint_sig.contains_key(&ip) {
                    if changed { "changed" } else { "unchanged" }
                } else {
                    "new"
                };
                emit::emit_endpoint(sink, endpoint, status)?;
                self.endpoint_emitted.insert(ip.clone(), now);
                self.endpoint_sig.insert(ip, sig);
                stats.endpoints_emitted += 1;
            }
        }

        let devices: Vec<Device> = self.inv.devices().cloned().collect();
        for device in &devices {
            let sig = device_signature(device);
            let changed = self.device_sig.get(&device.id) != Some(&sig);
            let last = self.device_emitted.get(&device.id).copied().unwrap_or(0);
            if changed || now.saturating_sub(last) >= refresh_ms {
                let status = if self.device_sig.contains_key(&device.id) {
                    if changed { "changed" } else { "unchanged" }
                } else {
                    "new"
                };
                emit::emit_device(sink, device, status)?;
                self.device_emitted.insert(device.id.clone(), now);
                self.device_sig.insert(device.id.clone(), sig);
                stats.devices_emitted += 1;
            }
        }
        Ok(())
    }
}

/// 一组源运行的结果。
#[derive(Debug, Clone, Copy, Default)]
struct RunStat {
    sources_run: usize,
    observations: usize,
    stopped: bool,
}

/// 等下一个发现源完成；宿主要求停止、到达 deadline、或没有任务时返回 `None`。
async fn next_source(
    tasks: &mut JoinSet<(SourceKind, Result<ScanOutcome, crate::engine::error::Error>)>,
    deadline: Option<Instant>,
    stop: &AtomicBool,
) -> Option<Result<(SourceKind, Result<ScanOutcome, crate::engine::error::Error>), tokio::task::JoinError>>
{
    loop {
        if stop.load(Ordering::Relaxed) || tasks.is_empty() {
            return None;
        }
        let joined = match deadline {
            Some(d) => {
                let left = d.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return None;
                }
                tokio::select! {
                    r = tasks.join_next() => r,
                    _ = tokio::time::sleep(left) => return None,
                }
            }
            None => tokio::select! {
                r = tasks.join_next() => r,
                // 没有硬 deadline 时也定期醒来，以便及时看到停止标志。
                _ = tokio::time::sleep(Duration::from_millis(200)) => continue,
            },
        };
        return joined;
    }
}

/// 端点的内容签名：忽略 `last_seen`/`sightings` 这类每次都变的字段。
fn endpoint_signature(e: &Endpoint) -> String {
    let fp = &e.fingerprint;
    let ports: Vec<u16> = fp.ports.iter().map(|p| p.port).collect();
    let hostnames: Vec<&str> = fp.hostnames.iter().map(|l| l.value.as_str()).collect();
    let models: Vec<&str> = fp.models.iter().map(|l| l.value.as_str()).collect();
    let sources: Vec<&str> = e.sources.iter().map(|s| s.label()).collect();
    format!(
        "online={};kind={};mac={:?};vendor={:?};os={:?};host={:?};model={:?};svc={:?};ports={:?};sources={:?};identified={}",
        e.online,
        fp.kind.label(),
        fp.mac,
        fp.vendor.as_ref().map(|l| l.value.as_str()),
        fp.os.as_ref().map(|l| l.value.as_str()),
        hostnames,
        models,
        fp.services,
        ports,
        sources,
        fp.identified,
    )
}

/// 设备的内容签名：忽略 `last_seen`。
fn device_signature(d: &Device) -> String {
    format!(
        "ips={:?};kind={};name={:?};known={};online={};merged={:?};conf={:?}",
        d.ips,
        d.kind.label(),
        d.name,
        d.known,
        d.online,
        d.merged_by.map(|m| m.label()),
        d.confidence,
    )
}

fn to_io(e: String) -> io::Error {
    io::Error::other(e)
}
