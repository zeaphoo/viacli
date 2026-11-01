//! 采集逻辑：抓取两个预测市场源 → 产出协议消息。
//!
//! 每个市场是**高基数、带稳定 id 的实体** → `record`（协议 §11.1）。
//! 每轮的计数与成交额是**数值时间线** → `sample`；它们同时承担"证明本轮活着
//! 且有产出"的职责（宿主把"退出码 0 但零数据"判定为失败，§11.3）。

use std::io::{self, Write};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use base_http::{HttpClient, HttpError, Kind, Retention, Sink, Unit};

use crate::config::Config;
use crate::sources::{self, PredictionMarket};

/// 状态/计数类读数的保留上限：1 天。
const R_STATUS: Retention = Retention::Max(86_400_000);

const K_POLY_COUNT: &str = "markets/polymarket_markets_count";
const K_POLY_VOLUME: &str = "markets/polymarket_volume_usd";
const K_KALSHI_COUNT: &str = "markets/kalshi_markets_count";
const K_KALSHI_VOLUME: &str = "markets/kalshi_volume_usd";
const K_SKIPPED: &str = "markets/fetch_skipped";

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
    pub markets_emitted: usize,
}

pub struct Collector {
    cfg: Config,
    state: State,
    client: HttpClient,
}

impl Collector {
    pub fn new(cfg: Config, state: State) -> Result<Self, HttpError> {
        let client = cfg.build_client()?;
        Ok(Self { cfg, state, client })
    }

    pub fn state(&self) -> &State {
        &self.state
    }

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

        if self.cfg.enable_polymarket {
            stats.sources_total += 1;
            match sources::fetch_polymarket(&self.client, &self.cfg) {
                Ok(list) => {
                    stats.sources_ok += 1;
                    let (count, volume) = emit_markets(sink, &list, now)?;
                    stats.markets_emitted += count;
                    sink.sample_i64(K_POLY_COUNT, now, count as i64)?;
                    sink.sample_f64(K_POLY_VOLUME, now, volume)?;
                }
                Err(e) => {
                    stats.sources_failed += 1;
                    report(sink, "Polymarket", &e)?;
                }
            }
        }

        if self.cfg.enable_kalshi {
            stats.sources_total += 1;
            match sources::fetch_kalshi(&self.client, &self.cfg) {
                Ok(list) => {
                    stats.sources_ok += 1;
                    let (count, volume) = emit_markets(sink, &list, now)?;
                    stats.markets_emitted += count;
                    sink.sample_i64(K_KALSHI_COUNT, now, count as i64)?;
                    sink.sample_f64(K_KALSHI_VOLUME, now, volume)?;
                }
                Err(e) => {
                    stats.sources_failed += 1;
                    report(sink, "Kalshi", &e)?;
                }
            }
        }

        if now.is_some() {
            self.state.last_fetch_ms = now;
        }
        Ok(stats)
    }
}

/// 产出市场记录，返回 `(条数, 成交额合计)`。
fn emit_markets<W: Write>(
    sink: &mut Sink<W>,
    list: &[PredictionMarket],
    now: Option<i64>,
) -> io::Result<(usize, f64)> {
    let mut volume = 0.0;
    for m in list {
        if let Some(v) = m.volume_usd
            && v.is_finite()
        {
            volume += v;
        }
        sink.record("markets.prediction_market", Some(&record_key(m)), now, payload(m))?;
    }
    Ok((list.len(), volume))
}

fn record_key(m: &PredictionMarket) -> String {
    format!("{}:{}", m.source, m.id)
}

fn declare_status<W: Write>(sink: &mut Sink<W>) -> io::Result<()> {
    sink.declare(K_POLY_COUNT, Kind::Gauge, Unit::Count, "Polymarket 活跃市场数", R_STATUS)?;
    sink.declare(K_POLY_VOLUME, Kind::Gauge, Unit::Count, "Polymarket 成交额合计（USD）", R_STATUS)?;
    sink.declare(K_KALSHI_COUNT, Kind::Gauge, Unit::Count, "Kalshi 开放市场数", R_STATUS)?;
    sink.declare(K_KALSHI_VOLUME, Kind::Gauge, Unit::Count, "Kalshi 成交额合计（USD）", R_STATUS)?;
    sink.declare(K_SKIPPED, Kind::Gauge, Unit::Count, "本轮是否跳过网络请求（1/0）", R_STATUS)
}

fn report<W: Write>(sink: &mut Sink<W>, label: &str, e: &HttpError) -> io::Result<()> {
    eprintln!("viacli-markets: {label} 抓取失败（{:?}）: {}", e.kind, e.message);
    sink.log("warn", &format!("{label} 抓取失败: {}", e.message))
}

fn payload(m: &PredictionMarket) -> Value {
    json!({
        "source": m.source,
        "id": m.id,
        "title": m.title,
        "category": m.category,
        "yesProbability": m.yes_probability,
        "volumeUsd": m.volume_usd,
        "liquidityUsd": m.liquidity_usd,
        "openInterest": m.open_interest,
        "closeTime": m.close_time,
        "url": m.url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_key_is_source_qualified() {
        let m = PredictionMarket {
            source: "kalshi",
            id: "ABC-1".into(),
            title: "t".into(),
            category: None,
            yes_probability: None,
            volume_usd: None,
            liquidity_usd: None,
            open_interest: None,
            close_time: None,
            url: None,
        };
        assert_eq!(record_key(&m), "kalshi:ABC-1");
    }

    #[test]
    fn state_roundtrips() {
        let s = State { last_fetch_ms: Some(5) };
        assert_eq!(State::from_request(Some(&s.to_value())).last_fetch_ms, Some(5));
    }
}
