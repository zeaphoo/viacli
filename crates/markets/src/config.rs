//! 请求与配置（见 `docs/07-collector-protocol.md` §2.2、§8.1）。

use std::time::Duration;

use serde::Deserialize;
use base_http::{HttpClient, HttpConfig, HttpError};

/// 宿主发来的请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub protocol: u32,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub now: Option<i64>,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub interval_ms: Option<u64>,
    #[serde(default)]
    pub config: Option<Config>,
    #[serde(default)]
    pub state: Option<serde_json::Value>,
    #[serde(default)]
    pub limits: Option<Limits>,
}

fn default_mode() -> String {
    "oneshot".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    #[serde(default)]
    #[allow(dead_code)]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub max_output_bytes: Option<u64>,
    #[serde(default)]
    pub max_lines: Option<u64>,
}

impl Request {
    pub fn tag(&self) -> String {
        match &self.run_id {
            Some(id) => format!("run={id}"),
            None => "run=-".to_string(),
        }
    }
}

/// 采集器配置（对应 `--describe` 里的 `configSchema`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// 两次真正发起网络抓取之间的最小间隔（采集器自己限流，§4.5）。
    pub interval_ms: u64,
    pub timeout_ms: u64,
    pub user_agent: String,
    /// 每个源单轮最多产出多少个市场（防止分页失控）。
    pub max_markets: usize,
    /// 每页请求的条数。
    pub page_size: usize,
    /// 翻页之间的最小停顿（毫秒）——对公共端点保持礼貌。
    pub page_delay_ms: u64,
    pub max_body_bytes: usize,
    pub max_retries: u32,
    pub enable_polymarket: bool,
    pub enable_kalshi: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // 5 分钟：预测市场变动远慢于此，更密只是白打上游。
            interval_ms: 300_000,
            timeout_ms: 15_000,
            user_agent: concat!(
                "viacli-markets/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/zeaphoo/viacli)"
            )
            .to_string(),
            // 150：单机仪表盘看的是"最活跃的一批"，不是全量；需要更多可调高。
            // 默认小也让一轮抓取足够快（宿主对 oneshot/stream 都有超时约束）。
            max_markets: 150,
            page_size: 50,
            page_delay_ms: 50,
            max_body_bytes: 16 * 1024 * 1024,
            max_retries: 2,
            enable_polymarket: true,
            enable_kalshi: true,
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.interval_ms < 60_000 {
            return Err(format!(
                "intervalMs 不能小于 60000（当前 {}）——预测市场不需要秒级轮询",
                self.interval_ms
            ));
        }
        if !(1_000..=120_000).contains(&self.timeout_ms) {
            return Err(format!("timeoutMs 必须在 1000..=120000 之间（当前 {}）", self.timeout_ms));
        }
        if self.max_markets == 0 || self.max_markets > 20_000 {
            return Err(format!("maxMarkets 必须在 1..=20000 之间（当前 {}）", self.max_markets));
        }
        if !(1..=1000).contains(&self.page_size) {
            return Err(format!("pageSize 必须在 1..=1000 之间（当前 {}）", self.page_size));
        }
        if self.page_delay_ms > 5_000 {
            return Err(format!("pageDelayMs 过大（当前 {}）", self.page_delay_ms));
        }
        if self.max_body_bytes < 64 * 1024 {
            return Err(format!("maxBodyBytes 太小（当前 {}）", self.max_body_bytes));
        }
        if self.user_agent.trim().is_empty() {
            return Err("userAgent 不能为空".into());
        }
        if !self.enable_polymarket && !self.enable_kalshi {
            return Err("至少要启用一个数据源（polymarket / kalshi）".into());
        }
        Ok(())
    }

    pub fn http_config(&self) -> HttpConfig {
        HttpConfig {
            timeout: Duration::from_millis(self.timeout_ms),
            connect_timeout: Duration::from_millis(self.timeout_ms.min(10_000)),
            user_agent: self.user_agent.clone(),
            max_body_bytes: self.max_body_bytes,
            max_retries: self.max_retries,
            ..HttpConfig::default()
        }
    }

    pub fn build_client(&self) -> Result<HttpClient, HttpError> {
        HttpClient::new(self.http_config())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        assert!(Config::default().validate().is_ok());
    }

    #[test]
    fn camel_case_request_parses() {
        let raw = r#"{"protocol":1,"now":1,"mode":"oneshot","config":{"pageSize":50,"enableKalshi":false}}"#;
        let req: Request = serde_json::from_str(raw).unwrap();
        let cfg = req.config.unwrap();
        assert_eq!(cfg.page_size, 50);
        assert!(!cfg.enable_kalshi);
    }

    #[test]
    fn invalid_configs_are_rejected() {
        assert!(Config { page_size: 0, ..Default::default() }.validate().is_err());
        assert!(Config {
            enable_polymarket: false,
            enable_kalshi: false,
            ..Default::default()
        }
        .validate()
        .is_err());
    }
}
