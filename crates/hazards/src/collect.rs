//! 采集逻辑：抓取三个源 → 产出协议消息。
//!
//! # 记录 vs 序列（协议 §11.1）
//!
//! 地震与天气预警是**高基数、带坐标的实体** → `record`（`kind` + 稳定 `key`）。
//! 每轮的计数与健康读数是**数值时间线** → `sample`。两者都要产出：
//! 宿主把"退出码 0 但一条数据都没有"判定为失败（§11.3），而"这一轮没有新预警"
//! 是完全正常的，所以计数样本本身承担了"证明本轮活着且有产出"的职责。

use std::io::{self, Write};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use base_http::{ErrorKind, HttpClient, HttpError, Kind, Retention, Sink, Unit};

use crate::config::Config;
use crate::sources;

/// 状态/计数类读数的保留上限：1 天。抖动读数不需要长期保留。
/// 记录类型的保留在 `--describe` 的 `recordKinds` 里声明（协议 §8.1）。
const R_STATUS: Retention = Retention::Max(86_400_000);

const K_EARTHQUAKES: &str = "hazard/earthquakes_count";
const K_ALERTS: &str = "hazard/weather_alerts_count";
const K_KP: &str = "hazard/kp_index";
const K_SPACE_AVAIL: &str = "hazard/space_weather_available";
const K_SKIPPED: &str = "hazard/fetch_skipped";

/// 跨运行记忆（协议 §5）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    /// 上次真正发起网络抓取的时刻（Unix 毫秒）。
    #[serde(default)]
    pub last_fetch_ms: Option<i64>,
}

impl State {
    /// 从请求里的 `state` 反序列化；格式不对就当"首次运行"。
    pub fn from_request(value: Option<&Value>) -> Self {
        value
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

/// 一轮采集的结果，供调用方决定退出码。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoundStats {
    pub sources_total: usize,
    pub sources_ok: usize,
    pub sources_failed: usize,
    pub records_emitted: usize,
}

pub struct Collector {
    cfg: Config,
    state: State,
    client: HttpClient,
}

impl Collector {
    /// 建 HTTP 客户端失败属于配置错误，构造阶段就返回。
    pub fn new(cfg: Config, state: State) -> Result<Self, HttpError> {
        let client = cfg.build_client()?;
        Ok(Self { cfg, state, client })
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// 距上次抓取不足 `intervalMs` 时，`oneshot` 这一轮跳过网络请求。
    ///
    /// 宿主对 `oneshot` 的调度间隔是宿主自己的事；灾害源经不起 1 分钟一次的
    /// 全量抓取，所以采集器**自己**限流（协议 §4.5 的合理延伸）。
    pub fn should_skip(&self, now: Option<i64>) -> bool {
        let (Some(last), Some(now)) = (self.state.last_fetch_ms, now) else {
            return false;
        };
        // 时钟回拨时 now-last 为负，按"没到点"处理，不能当成"很久没抓"而狂刷。
        now.saturating_sub(last) < self.cfg.interval_ms as i64
    }

    /// 跳过一轮：发一条状态样本，保证宿主不把它当成空轮。
    pub fn emit_skipped<W: Write>(
        &self,
        sink: &mut Sink<W>,
        now: Option<i64>,
    ) -> io::Result<()> {
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

        if self.cfg.enable_earthquakes {
            stats.sources_total += 1;
            match sources::fetch_earthquakes(&self.client, &self.cfg) {
                Ok(list) => {
                    stats.sources_ok += 1;
                    for eq in &list {
                        sink.record("hazard.earthquake", Some(&eq.id), now, earthquake_payload(eq))?;
                    }
                    stats.records_emitted += list.len();
                    sink.sample_i64(K_EARTHQUAKES, now, list.len() as i64)?;
                }
                Err(e) => {
                    stats.sources_failed += 1;
                    report(sink, "USGS 地震", &e)?;
                }
            }
        }

        if self.cfg.enable_weather_alerts {
            stats.sources_total += 1;
            match sources::fetch_weather_alerts(&self.client, &self.cfg) {
                Ok(list) => {
                    stats.sources_ok += 1;
                    for a in &list {
                        sink.record("hazard.weather_alert", Some(&a.id), now, alert_payload(a))?;
                    }
                    stats.records_emitted += list.len();
                    sink.sample_i64(K_ALERTS, now, list.len() as i64)?;
                }
                Err(e) => {
                    stats.sources_failed += 1;
                    report(sink, "NWS 天气预警", &e)?;
                }
            }
        }

        if self.cfg.enable_space_weather {
            stats.sources_total += 1;
            match sources::fetch_kp_index(&self.client) {
                Ok(kp) => {
                    stats.sources_ok += 1;
                    // 源健康就用 1 表示；数组为空（None）说明上游暂时没数据，也是健康的。
                    sink.sample_i64(K_SPACE_AVAIL, now, if kp.is_some() { 1 } else { 0 })?;
                    if let Some(kp) = kp {
                        sink.sample_f64(K_KP, now, kp)?;
                    }
                }
                Err(e) => {
                    stats.sources_failed += 1;
                    report(sink, "NOAA SWPC 空间天气", &e)?;
                }
            }
        }

        if now.is_some() {
            self.state.last_fetch_ms = now;
        }
        Ok(stats)
    }
}

fn declare_status<W: Write>(sink: &mut Sink<W>) -> io::Result<()> {
    sink.declare(K_EARTHQUAKES, Kind::Gauge, Unit::Count, "过去 24h 地震条数", R_STATUS)?;
    sink.declare(K_ALERTS, Kind::Gauge, Unit::Count, "当前生效天气预警条数", R_STATUS)?;
    sink.declare(K_KP, Kind::Gauge, Unit::Count, "最新行星星际 Kp 指数", R_STATUS)?;
    sink.declare(
        K_SPACE_AVAIL,
        Kind::Gauge,
        Unit::Count,
        "空间天气源是否可用（1/0）",
        R_STATUS,
    )?;
    sink.declare(K_SKIPPED, Kind::Gauge, Unit::Count, "本轮是否跳过网络请求（1/0）", R_STATUS)
}

/// 单个源抓取失败是常态（限流、临时 5xx、上游字段漂移）：只在 stderr 与 log 里说明，
/// 不报 `error` 打断整轮；退出码由调用方根据 `sources_ok` 决定。
fn report<W: Write>(sink: &mut Sink<W>, label: &str, e: &HttpError) -> io::Result<()> {
    eprintln!("viacli-hazards: {label} 抓取失败（{:?}）: {}", e.kind, e.message);
    sink.log("warn", &format!("{label} 抓取失败: {}", e.message))
}

fn earthquake_payload(eq: &sources::Earthquake) -> Value {
    json!({
        "id": eq.id,
        "magnitude": eq.magnitude,
        "place": eq.place,
        "lat": eq.lat,
        "lon": eq.lon,
        "depthKm": eq.depth_km,
        "timeMs": eq.time_ms,
        "url": eq.url,
        "tsunami": eq.tsunami,
        "alert": eq.alert,
        "source": "USGS",
    })
}

fn alert_payload(a: &sources::WeatherAlert) -> Value {
    json!({
        "id": a.id,
        "event": a.event,
        "severity": a.severity,
        "urgency": a.urgency,
        "certainty": a.certainty,
        "headline": a.headline,
        "areaDesc": a.area_desc,
        "expires": a.expires,
        "lat": a.lat,
        "lon": a.lon,
        "source": "NWS",
    })
}

/// 供 `describe` 复用的错误类别（确认 base 库的错误类别被本 crate 用到）。
#[allow(dead_code)]
pub fn error_kind_exit(kind: ErrorKind) -> i32 {
    kind.exit_code()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_roundtrips() {
        let s = State { last_fetch_ms: Some(123) };
        let v = s.to_value();
        let back = State::from_request(Some(&v));
        assert_eq!(back.last_fetch_ms, Some(123));
    }

    #[test]
    fn bad_state_falls_back_to_first_run() {
        let back = State::from_request(Some(&json!({"unexpected": true})));
        assert_eq!(back.last_fetch_ms, None);
    }
}
