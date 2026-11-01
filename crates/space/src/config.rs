//! 请求与配置（见 `docs/07-collector-protocol.md` §2.2、§8.1）。

use std::time::Duration;

use serde::Deserialize;
use base_http::{HttpClient, HttpConfig, HttpError};

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
    "stream".to_string()
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// 两轮之间的目标间隔（秒级位置更新）。
    pub interval_ms: u64,
    /// TLE 刷新间隔：TLE 一天只更新几次，**不能每轮都打 CelesTrak**。
    /// 常驻（stream）时在进程内缓存；oneshot 下只能每轮重新拉取，故用 intervalMs 兜底限流。
    pub tle_refresh_ms: u64,
    pub timeout_ms: u64,
    pub user_agent: String,
    /// CelesTrak 分组。越小越快；`active`/`starlink` 是上万颗，默认不选。
    pub celestrak_groups: Vec<String>,
    /// 单轮最多传播的卫星数（保护 CPU）。
    pub max_satellites: usize,
    pub max_ground_stations: usize,
    pub max_body_bytes: usize,
    pub max_retries: u32,
    /// 记录里是否带上原始 TLE（体积大，默认关）。
    pub include_tle: bool,
    pub enable_ground_stations: bool,
    /// TinyGS 端点目前不稳定，默认关。
    pub enable_tinygs: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            interval_ms: 60_000,
            tle_refresh_ms: 21_600_000, // 6 小时
            timeout_ms: 20_000,
            user_agent: concat!(
                "viacli-space/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/zeaphoo/viacli)"
            )
            .to_string(),
            // 默认只取最小集：一轮抓取要足够快（宿主对 oneshot/stream 都有超时约束）。
            // 想要更多卫星，自行加 weather / gps-ops / science / active。
            celestrak_groups: vec!["stations".to_string()],
            max_satellites: 500,
            max_ground_stations: 2000,
            max_body_bytes: 24 * 1024 * 1024,
            max_retries: 1,
            include_tle: false,
            // SatNOGS 单次 ~3.4MB，会显著拖慢一轮；默认关，按需开启。
            enable_ground_stations: false,
            enable_tinygs: false,
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.interval_ms < 30_000 {
            return Err(format!(
                "intervalMs 不能小于 30000（当前 {}）——卫星位置不需要亚分钟轮询",
                self.interval_ms
            ));
        }
        if self.tle_refresh_ms < 1_800_000 {
            return Err(format!(
                "tleRefreshMs 不能小于 1800000（当前 {}）——CelesTrak 的公平使用要求不要频繁拉取",
                self.tle_refresh_ms
            ));
        }
        if !(1_000..=120_000).contains(&self.timeout_ms) {
            return Err(format!("timeoutMs 必须在 1000..=120000 之间（当前 {}）", self.timeout_ms));
        }
        if self.celestrak_groups.is_empty() {
            return Err("celestrakGroups 不能为空".into());
        }
        for g in &self.celestrak_groups {
            if !g.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')) {
                return Err(format!("非法分组名 {g:?}"));
            }
        }
        if self.max_satellites == 0 || self.max_satellites > 20_000 {
            return Err(format!("maxSatellites 必须在 1..=20000 之间（当前 {}）", self.max_satellites));
        }
        if self.max_ground_stations == 0 {
            return Err("maxGroundStations 必须大于 0".into());
        }
        if self.max_body_bytes < 64 * 1024 {
            return Err(format!("maxBodyBytes 太小（当前 {}）", self.max_body_bytes));
        }
        if self.user_agent.trim().is_empty() {
            return Err("userAgent 不能为空（CelesTrak/SatNOGS 要求可识别的 UA）".into());
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
        let raw = r#"{"protocol":1,"now":1,"mode":"stream","config":{"maxSatellites":10,"celestrakGroups":["stations"]}}"#;
        let req: Request = serde_json::from_str(raw).unwrap();
        let cfg = req.config.unwrap();
        assert_eq!(cfg.max_satellites, 10);
        assert_eq!(cfg.celestrak_groups, vec!["stations"]);
    }

    #[test]
    fn invalid_configs_are_rejected() {
        assert!(Config { interval_ms: 1, ..Default::default() }.validate().is_err());
        assert!(Config { tle_refresh_ms: 1, ..Default::default() }.validate().is_err());
        assert!(Config { celestrak_groups: vec![], ..Default::default() }.validate().is_err());
    }
}
