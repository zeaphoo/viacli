//! 采集逻辑：CelesTrak TLE → SGP4 星下点，加 SatNOGS/TinyGS 地面站。
//!
//! # 为什么默认 `stream`
//!
//! TLE 一天只更新几次，而星下点要秒级刷新。若用 `oneshot`，每轮都去 CelesTrak
//! 拉一次 TLE，既慢又不礼貌（CelesTrak 有公平使用要求）。常驻时在**进程内**缓存
//! TLE、每 `tleRefreshMs` 才刷新一次，正好符合 `DESIGN.md` §6.2.13.5 里
//! "每轮握手很贵 → 用 stream" 的判据。

use std::collections::HashSet;
use std::io::{self, Write};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sgp4::chrono::NaiveDateTime;
use base_http::{HttpClient, HttpError, Kind, Retention, Sink, Unit};

use crate::config::Config;
use crate::sources::{self, RawTle};

/// 状态/计数类读数的保留上限：1 天。
const R_STATUS: Retention = Retention::Max(86_400_000);

const K_SATS: &str = "space/satellites_count";
const K_PROP_FAIL: &str = "space/propagation_failures";
const K_STATIONS: &str = "space/ground_stations_count";
const K_SKIPPED: &str = "space/fetch_skipped";

/// 跨运行记忆（协议 §5）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub last_fetch_ms: Option<i64>,
}

impl State {
    pub fn from_request(value: Option<&Value>) -> Self {
        value
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoundStats {
    pub sources_total: usize,
    pub sources_ok: usize,
    pub sources_failed: usize,
    pub satellites_emitted: usize,
    pub stations_emitted: usize,
    pub propagation_failures: usize,
}

pub struct Collector {
    cfg: Config,
    state: State,
    client: HttpClient,
    /// 进程内 TLE 缓存：`(拉取时刻, [(分组, 原始 TLE)])`。
    tle_cache: Option<(i64, Vec<(String, RawTle)>)>,
}

impl Collector {
    pub fn new(cfg: Config, state: State) -> Result<Self, HttpError> {
        let client = cfg.build_client()?;
        Ok(Self {
            cfg,
            state,
            client,
            tle_cache: None,
        })
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// oneshot 下距上次抓取不足 `intervalMs` 就跳过（避免每轮都打 CelesTrak）。
    pub fn should_skip(&self, now: Option<i64>) -> bool {
        let (Some(last), Some(now)) = (self.state.last_fetch_ms, now) else {
            return false;
        };
        now.saturating_sub(last) < self.cfg.interval_ms as i64
    }

    pub fn emit_skipped<W: Write>(&self, sink: &mut Sink<W>, now: Option<i64>) -> io::Result<()> {
        declare_status(sink)?;
        sink.sample_i64(K_SKIPPED, now, 1)
    }

    pub fn collect<W: Write>(
        &mut self,
        sink: &mut Sink<W>,
        now: Option<i64>,
    ) -> io::Result<RoundStats> {
        declare_status(sink)?;
        let mut stats = RoundStats::default();

        let target = match sources::target_datetime(now) {
            Some(t) => t,
            None => {
                sink.error(base_http::ErrorKind::Internal, true, "无法确定传播时刻")?;
                return Ok(stats);
            }
        };

        self.ensure_tle(sink, now, &mut stats)?;
        self.emit_satellites(sink, now, &target, &mut stats)?;

        if self.cfg.enable_ground_stations {
            stats.sources_total += 1;
            match sources::fetch_satnogs_stations(&self.client, &self.cfg) {
                Ok(list) => {
                    stats.sources_ok += 1;
                    for gs in &list {
                        sink.record("space.ground_station", Some(&station_key(gs)), now, station_payload(gs))?;
                    }
                    stats.stations_emitted += list.len();
                    sink.sample_i64(K_STATIONS, now, list.len() as i64)?;
                }
                Err(e) => {
                    stats.sources_failed += 1;
                    report(sink, "SatNOGS", &e)?;
                }
            }
        }

        if self.cfg.enable_tinygs {
            stats.sources_total += 1;
            match sources::fetch_tinygs_stations(&self.client, &self.cfg) {
                Ok(list) => {
                    stats.sources_ok += 1;
                    for gs in &list {
                        sink.record("space.ground_station", Some(&station_key(gs)), now, station_payload(gs))?;
                    }
                    stats.stations_emitted += list.len();
                }
                Err(e) => {
                    stats.sources_failed += 1;
                    report(sink, "TinyGS", &e)?;
                }
            }
        }

        if now.is_some() {
            self.state.last_fetch_ms = now;
        }
        Ok(stats)
    }

    /// 保证进程内有 TLE；超过 `tleRefreshMs` 才重新拉取。
    fn ensure_tle<W: Write>(
        &mut self,
        sink: &mut Sink<W>,
        now: Option<i64>,
        stats: &mut RoundStats,
    ) -> io::Result<()> {
        let fresh = match (&self.tle_cache, now) {
            (Some((fetched_at, _)), Some(now)) => {
                now.saturating_sub(*fetched_at) < self.cfg.tle_refresh_ms as i64
            }
            (Some(_), None) => true,
            (None, _) => false,
        };
        if fresh {
            return Ok(());
        }

        let groups = self.cfg.celestrak_groups.clone();
        let mut collected: Vec<(String, RawTle)> = Vec::new();
        for group in &groups {
            stats.sources_total += 1;
            match sources::fetch_tle_group(&self.client, group) {
                Ok(list) => {
                    stats.sources_ok += 1;
                    for raw in list {
                        collected.push((group.clone(), raw));
                    }
                }
                Err(e) => {
                    stats.sources_failed += 1;
                    report(sink, &format!("CelesTrak/{group}"), &e)?;
                }
            }
        }
        if !collected.is_empty() {
            let fetched_at = now.unwrap_or(0);
            self.tle_cache = Some((fetched_at, collected));
        }
        Ok(())
    }

    fn emit_satellites<W: Write>(
        &mut self,
        sink: &mut Sink<W>,
        now: Option<i64>,
        target: &NaiveDateTime,
        stats: &mut RoundStats,
    ) -> io::Result<()> {
        let Some((_, entries)) = &self.tle_cache else {
            return Ok(());
        };
        let mut seen: HashSet<u64> = HashSet::new();
        let mut emitted = 0usize;
        for (group, raw) in entries {
            if emitted >= self.cfg.max_satellites {
                break;
            }
            let Some(el) = sources::parse_elements(raw) else {
                continue;
            };
            if !seen.insert(el.norad_id) {
                continue;
            }
            let position = sources::propagate(&el, target);
            if position.is_none() {
                stats.propagation_failures += 1;
            }
            let (lat, lon, alt) = match position {
                Some(p) => (Some(p.lat), Some(p.lon), Some(p.alt_km)),
                None => (None, None, None),
            };
            let mut payload = json!({
                "name": el.object_name,
                "noradId": el.norad_id,
                "group": group,
                "intlDesignator": el.international_designator,
                "epoch": el.datetime.and_utc().to_rfc3339(),
                "inclinationDeg": el.inclination,
                "meanMotion": el.mean_motion,
                "eccentricity": el.eccentricity,
                "lat": lat,
                "lon": lon,
                "altKm": alt,
            });
            if self.cfg.include_tle {
                payload["tle1"] = json!(raw.line1);
                payload["tle2"] = json!(raw.line2);
            }
            sink.record("space.satellite", Some(&el.norad_id.to_string()), now, payload)?;
            emitted += 1;
        }
        stats.satellites_emitted = emitted;
        sink.sample_i64(K_SATS, now, emitted as i64)?;
        sink.sample_i64(K_PROP_FAIL, now, stats.propagation_failures as i64)?;
        Ok(())
    }
}

fn declare_status<W: Write>(sink: &mut Sink<W>) -> io::Result<()> {
    sink.declare(K_SATS, Kind::Gauge, Unit::Count, "本轮产出卫星数", R_STATUS)?;
    sink.declare(K_PROP_FAIL, Kind::Gauge, Unit::Count, "SGP4 传播失败数", R_STATUS)?;
    sink.declare(K_STATIONS, Kind::Gauge, Unit::Count, "地面站数", R_STATUS)?;
    sink.declare(K_SKIPPED, Kind::Gauge, Unit::Count, "本轮是否跳过网络请求（1/0）", R_STATUS)
}

fn station_key(gs: &sources::GroundStation) -> String {
    format!("{}:{}", gs.network, gs.id)
}

fn station_payload(gs: &sources::GroundStation) -> Value {
    json!({
        "network": gs.network,
        "id": gs.id,
        "name": gs.name,
        "lat": gs.lat,
        "lon": gs.lon,
        "altM": gs.alt_m,
        "lastSeen": gs.last_seen,
        "observations": gs.observations,
        "owner": gs.owner,
        "clientVersion": gs.client_version,
    })
}

fn report<W: Write>(sink: &mut Sink<W>, label: &str, e: &HttpError) -> io::Result<()> {
    eprintln!("viacli-space: {label} 抓取失败（{:?}）: {}", e.kind, e.message);
    sink.log("warn", &format!("{label} 抓取失败: {}", e.message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn station_key_is_network_qualified() {
        let gs = sources::GroundStation {
            network: "satnogs",
            id: "1".into(),
            name: "n".into(),
            lat: 0.0,
            lon: 0.0,
            alt_m: None,
            last_seen: None,
            observations: None,
            owner: None,
            client_version: None,
        };
        assert_eq!(station_key(&gs), "satnogs:1");
    }
}
