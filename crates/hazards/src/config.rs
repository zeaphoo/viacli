//! 请求与配置（见 `docs/07-collector-protocol.md` §2.2、§8.1）。

use serde::Deserialize;
use base_http::{HttpConfig, HttpError};

use std::time::Duration;

/// 宿主发来的请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub protocol: u32,
    /// 仅用于日志关联：宿主会把它和自己的记录对上，采集器不需要据此改行为。
    #[serde(default)]
    pub run_id: Option<String>,
    /// 宿主当前时间（Unix 毫秒）。采集器不要自己造时间（§2.5）。
    #[serde(default)]
    pub now: Option<i64>,
    /// `oneshot` | `stream`（§4）
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub interval_ms: Option<u64>,
    #[serde(default)]
    pub config: Option<Config>,
    /// 上次成功运行保存的状态（§5）。
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
    /// 日志前缀，便于把 stderr 与某次运行对上。
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
    /// 单次 HTTP 请求超时。
    pub timeout_ms: u64,
    /// 出站 User-Agent。NWS 要求可识别的 UA。
    pub user_agent: String,
    /// 地震最小震级，低于此值不产出。
    pub min_magnitude: f64,
    /// 单轮每类记录的最大条数上限。
    pub max_records: usize,
    /// 单个响应的字节上限。
    pub max_body_bytes: usize,
    /// 可重试错误的最大重试次数。
    pub max_retries: u32,
    pub enable_earthquakes: bool,
    pub enable_weather_alerts: bool,
    pub enable_space_weather: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // 2 分钟：USGS/NWS 更新频率远低于此，更密只是白打上游。
            interval_ms: 120_000,
            timeout_ms: 15_000,
            user_agent: concat!(
                "viacli-hazards/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/zeaphoo/viacli)"
            )
            .to_string(),
            // 2.5 级：低于此值一天有上千条，展示价值低且会压垮记录表。
            min_magnitude: 2.5,
            max_records: 2000,
            max_body_bytes: 16 * 1024 * 1024,
            max_retries: 2,
            enable_earthquakes: true,
            enable_weather_alerts: true,
            enable_space_weather: true,
        }
    }
}

impl Config {
    /// 校验配置；返回人类可读的错误（宿主按退出码 2 处理，§3.4）。
    pub fn validate(&self) -> Result<(), String> {
        if self.interval_ms < 60_000 {
            return Err(format!(
                "intervalMs 不能小于 60000（当前 {}）——灾害源不需要秒级轮询",
                self.interval_ms
            ));
        }
        if !(1_000..=120_000).contains(&self.timeout_ms) {
            return Err(format!("timeoutMs 必须在 1000..=120000 之间（当前 {}）", self.timeout_ms));
        }
        if !(0.0..=10.0).contains(&self.min_magnitude) {
            return Err(format!("minMagnitude 必须在 0..=10 之间（当前 {}）", self.min_magnitude));
        }
        if self.max_records == 0 || self.max_records > 20_000 {
            return Err(format!("maxRecords 必须在 1..=20000 之间（当前 {}）", self.max_records));
        }
        if self.max_body_bytes < 64 * 1024 {
            return Err(format!("maxBodyBytes 太小（当前 {}）", self.max_body_bytes));
        }
        if self.user_agent.trim().is_empty() {
            return Err("userAgent 不能为空（NWS 要求可识别的 UA）".into());
        }
        if !self.enable_earthquakes && !self.enable_weather_alerts && !self.enable_space_weather {
            return Err("至少要启用一个数据源（earthquakes / weatherAlerts / spaceWeather）".into());
        }
        Ok(())
    }

    /// 由采集器配置派生出 HTTP 客户端配置。
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

    /// 建 HTTP 客户端；失败是配置错误，不可重试。
    pub fn build_client(&self) -> Result<base_http::HttpClient, HttpError> {
        base_http::HttpClient::new(self.http_config())
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
    fn camel_case_request_parses_and_ignores_unknown_fields() {
        let raw = r#"{
            "protocol": 1, "runId": "r1", "now": 123, "mode": "oneshot",
            "config": {"minMagnitude": 4.0, "enableWeatherAlerts": false},
            "unexpected": true,
            "limits": {"timeoutMs": 5000}
        }"#;
        let req: Request = serde_json::from_str(raw).unwrap();
        assert_eq!(req.protocol, 1);
        let cfg = req.config.unwrap();
        assert_eq!(cfg.min_magnitude, 4.0);
        assert!(!cfg.enable_weather_alerts);
        assert_eq!(req.limits.unwrap().timeout_ms, Some(5000));
    }

    #[test]
    fn invalid_configs_are_rejected_with_readable_message() {
        assert!(Config { interval_ms: 1, ..Default::default() }.validate().is_err());
        assert!(Config { min_magnitude: 99.0, ..Default::default() }.validate().is_err());
        assert!(Config {
            enable_earthquakes: false,
            enable_weather_alerts: false,
            enable_space_weather: false,
            ..Default::default()
        }
        .validate()
        .is_err());
    }
}
