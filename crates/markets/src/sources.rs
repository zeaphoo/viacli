//! Polymarket 与 Kalshi 的只读、免 key 端点，归一化成同一个 `PredictionMarket`。
//!
//! * Polymarket Gamma：`gamma-api.polymarket.com/events?active=true&closed=false`
//!   一个 event 下挂多个二元 market（每个 candidate 一个），逐 market 归一化。
//! * Kalshi：`api.elections.kalshi.com/trade-api/v2/events?status=open&with_nested_markets=true`
//!   用 **events** 而不是 **markets**：events 带 `category`，且能把市场挂在真实事件下。
//!   无赔率的组合盘（multivariate parlay）会被过滤掉。
//!
//! 两个源的赔率口径不同：Polymarket 给 0–1 的小数字符串，Kalshi 新接口给
//! `*_dollars`（0–1 字符串）、旧接口给 0–100 的整数分。归一化后统一为
//! `yes_probability ∈ [0,1]`。

use std::time::Duration;

use serde_json::Value;
use base_http::{HttpClient, HttpError};

use crate::config::Config;

const POLYMARKET_EVENTS: &str = "https://gamma-api.polymarket.com/events";
const KALSHI_EVENTS: &str = "https://api.elections.kalshi.com/trade-api/v2/events";
/// 翻页硬上限，防止上游游标异常导致无限循环。
const MAX_PAGES: usize = 100;

/// 归一化后的一个二元预测市场。
#[derive(Debug, Clone, PartialEq)]
pub struct PredictionMarket {
    /// `polymarket` | `kalshi`
    pub source: &'static str,
    pub id: String,
    pub title: String,
    pub category: Option<String>,
    /// YES 的隐含概率，0–1。
    pub yes_probability: Option<f64>,
    pub volume_usd: Option<f64>,
    pub liquidity_usd: Option<f64>,
    pub open_interest: Option<f64>,
    pub close_time: Option<String>,
    pub url: Option<String>,
}

/// 抓取 Polymarket 全部未关闭事件下的活跃 market。
pub fn fetch_polymarket(
    client: &HttpClient,
    cfg: &Config,
) -> Result<Vec<PredictionMarket>, HttpError> {
    let mut out = Vec::new();
    let mut offset = 0usize;
    let page = cfg.page_size;
    let mut pages = 0usize;

    loop {
        // order=volume：先拿最大的市场，这样即使被 maxMarkets 截断，留下的也是最有意义的。
        let url = format!(
            "{POLYMARKET_EVENTS}?active=true&closed=false&limit={page}&offset={offset}&order=volume&ascending=false"
        );
        let doc = client.get_json(&url)?;
        let events = doc
            .as_array()
            .ok_or_else(|| HttpError::parse("Polymarket 响应不是数组"))?;
        if events.is_empty() {
            break;
        }

        for ev in events {
            let event_title = as_str(&ev["title"]).unwrap_or_default();
            let event_slug = as_str(&ev["slug"]);
            let event_volume = as_f64(&ev["volume"]);
            let event_end = as_str(&ev["endDate"]);
            let Some(markets) = ev["markets"].as_array() else {
                continue;
            };
            for m in markets {
                if m["closed"].as_bool() == Some(true) {
                    continue;
                }
                let Some(id) = as_str(&m["id"]).or_else(|| as_str(&m["conditionId"])) else {
                    continue;
                };
                let title = as_str(&m["question"]).unwrap_or_else(|| event_title.clone());
                let outcomes = json_string_array(&m["outcomes"]);
                let prices = json_string_array(&m["outcomePrices"]);
                out.push(PredictionMarket {
                    source: "polymarket",
                    id,
                    title,
                    category: None,
                    yes_probability: yes_probability(&outcomes, &prices),
                    volume_usd: as_f64(&m["volumeNum"])
                        .or_else(|| as_f64(&m["volume"]))
                        .or(event_volume),
                    liquidity_usd: as_f64(&m["liquidityNum"]).or_else(|| as_f64(&m["liquidity"])),
                    open_interest: None,
                    close_time: as_str(&m["endDate"]).or_else(|| event_end.clone()),
                    url: event_slug
                        .as_deref()
                        .map(|s| format!("https://polymarket.com/event/{s}")),
                });
                if out.len() >= cfg.max_markets {
                    break;
                }
            }
            if out.len() >= cfg.max_markets {
                break;
            }
        }

        pages += 1;
        if out.len() >= cfg.max_markets || events.len() < page || pages >= MAX_PAGES {
            break;
        }
        offset += page;
        pause(cfg.page_delay_ms);
    }
    Ok(out)
}

/// 抓取 Kalshi 全部开放事件及其嵌套 market（游标翻页）。
pub fn fetch_kalshi(client: &HttpClient, cfg: &Config) -> Result<Vec<PredictionMarket>, HttpError> {
    let mut out = Vec::new();
    let mut cursor = String::new();
    let page = cfg.page_size;
    let mut pages = 0usize;

    loop {
        let url = if cursor.is_empty() {
            format!("{KALSHI_EVENTS}?status=open&with_nested_markets=true&limit={page}")
        } else {
            format!(
                "{KALSHI_EVENTS}?status=open&with_nested_markets=true&limit={page}&cursor={}",
                percent_encode(&cursor)
            )
        };
        let doc = client.get_json(&url)?;
        let events = doc["events"]
            .as_array()
            .ok_or_else(|| HttpError::parse("Kalshi 响应缺少 events 数组"))?;
        if events.is_empty() {
            break;
        }

        for ev in events {
            let category = as_str(&ev["category"]);
            let event_title = as_str(&ev["title"]);
            let Some(markets) = ev["markets"].as_array() else {
                continue;
            };
            for m in markets {
                let Some(ticker) = as_str(&m["ticker"]) else {
                    continue;
                };
                // 无赔率的组合盘/占位盘直接跳过：没有展示价值，还会淹没真市场。
                let Some(yes_prob) = kalshi_yes_probability(m) else {
                    continue;
                };
                let title = kalshi_title(m, event_title.as_deref()).unwrap_or_else(|| ticker.clone());
                out.push(PredictionMarket {
                    source: "kalshi",
                    id: ticker.clone(),
                    title,
                    category: category.clone(),
                    yes_probability: Some(yes_prob),
                    volume_usd: kalshi_volume_usd(m),
                    liquidity_usd: None,
                    open_interest: as_f64(&m["open_interest_fp"])
                        .or_else(|| as_f64(&m["open_interest"])),
                    close_time: as_str(&m["close_time"]),
                    url: Some(format!("https://kalshi.com/markets/{ticker}")),
                });
                if out.len() >= cfg.max_markets {
                    break;
                }
            }
            if out.len() >= cfg.max_markets {
                break;
            }
        }

        let next = as_str(&doc["cursor"]).unwrap_or_default();
        pages += 1;
        if out.len() >= cfg.max_markets || next.is_empty() || next == cursor || pages >= MAX_PAGES {
            break;
        }
        cursor = next;
        pause(cfg.page_delay_ms);
    }
    Ok(out)
}

fn pause(ms: u64) {
    if ms > 0 {
        std::thread::sleep(Duration::from_millis(ms));
    }
}

/// Polymarket：outcomes/outcomePrices 是 **被二次编码的 JSON 字符串**数组。
fn json_string_array(v: &Value) -> Vec<String> {
    let parsed: Value = match v {
        Value::String(s) => serde_json::from_str(s).unwrap_or(Value::Null),
        other => other.clone(),
    };
    parsed
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// 取 "Yes" 的价格作为概率；没有 "Yes" 时取第一个（候选型市场即"该候选胜出"）。
fn yes_probability(outcomes: &[String], prices: &[String]) -> Option<f64> {
    let idx = outcomes
        .iter()
        .position(|o| o.eq_ignore_ascii_case("yes"))
        .unwrap_or(0);
    prices.get(idx).and_then(|p| p.trim().parse::<f64>().ok())
}

/// Kalshi：新接口 `*_dollars` 已是 0–1 字符串；旧接口是 0–100 的整数分。
fn kalshi_yes_probability(m: &Value) -> Option<f64> {
    for key in ["last_price_dollars", "previous_price_dollars"] {
        if let Some(p) = as_f64(&m[key])
            && p > 0.0
        {
            return Some(clamp01(p));
        }
    }
    if let Some(p) = as_f64(&m["last_price"])
        && p > 0.0
    {
        return Some(clamp01(p / 100.0));
    }
    let bid = as_f64(&m["yes_bid_dollars"]).or_else(|| as_f64(&m["yes_bid"]).map(|v| v / 100.0));
    let ask = as_f64(&m["yes_ask_dollars"]).or_else(|| as_f64(&m["yes_ask"]).map(|v| v / 100.0));
    match (bid, ask) {
        (Some(b), Some(a)) if b > 0.0 || a > 0.0 => Some(clamp01((b + a) / 2.0)),
        (Some(b), None) if b > 0.0 => Some(clamp01(b)),
        (None, Some(a)) if a > 0.0 => Some(clamp01(a)),
        _ => None,
    }
}

/// Kalshi 成交额：`volume_fp` 是**合约张数**，每张名义价值约 $1（`notional_value_dollars`）。
fn kalshi_volume_usd(m: &Value) -> Option<f64> {
    let contracts = as_f64(&m["volume_fp"]).or_else(|| as_f64(&m["volume"]))?;
    let notional = as_f64(&m["notional_value_dollars"]).unwrap_or(1.0);
    Some(contracts * notional)
}

/// 组合标题：优先 `<事件标题> — <候选子标题>`，退化到市场自己的 title / 事件 title。
fn kalshi_title(m: &Value, event_title: Option<&str>) -> Option<String> {
    if let Some(sub) = as_str(&m["yes_sub_title"]) {
        return Some(match event_title {
            Some(ev) if !ev.is_empty() => format!("{ev} — {sub}"),
            _ => sub,
        });
    }
    as_str(&m["title"]).or_else(|| event_title.map(str::to_string))
}

fn clamp01(v: f64) -> f64 {
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}

fn as_f64(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_i64().map(|i| i as f64))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
}

fn as_str(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 仅供游标安全放进查询串；游标是服务端不透明串，不保证 URL-safe。
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nested_json_string_array_is_decoded() {
        let v = json!("[\"Yes\", \"No\"]");
        assert_eq!(json_string_array(&v), vec!["Yes", "No"]);
    }

    #[test]
    fn polymarket_yes_takes_named_outcome() {
        let outcomes = vec!["No".to_string(), "Yes".to_string()];
        let prices = vec!["0.3".to_string(), "0.7".to_string()];
        assert_eq!(yes_probability(&outcomes, &prices), Some(0.7));
    }

    #[test]
    fn kalshi_reads_new_dollar_fields() {
        let m = json!({"last_price_dollars": "0.0900", "yes_bid_dollars": "0.09", "yes_ask_dollars": "0.11"});
        assert_eq!(kalshi_yes_probability(&m), Some(0.09));
        assert_eq!(kalshi_volume_usd(&json!({"volume_fp": "119640.94"})), Some(119640.94));
    }

    #[test]
    fn kalshi_falls_back_to_midpoint_then_none() {
        assert_eq!(
            kalshi_yes_probability(&json!({"last_price_dollars": "0.0000", "yes_bid_dollars": "0.40", "yes_ask_dollars": "0.50"})),
            Some(0.45)
        );
        assert_eq!(kalshi_yes_probability(&json!({})), None);
    }

    #[test]
    fn kalshi_title_prefers_event_plus_subtitle() {
        let m = json!({"yes_sub_title": "Mars", "title": "ignored"});
        assert_eq!(kalshi_title(&m, Some("Will Elon visit Mars?")).unwrap(), "Will Elon visit Mars? — Mars");
    }

    #[test]
    fn cursor_is_percent_encoded() {
        assert_eq!(percent_encode("a+b=c"), "a%2Bb%3Dc");
        assert_eq!(percent_encode("abc-_.~"), "abc-_.~");
    }
}
