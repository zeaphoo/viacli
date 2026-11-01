//! 宿主请求（协议 §2.2）。配置直接沿用引擎的 [`engine::config::Config`]。

use serde::Deserialize;

use crate::engine::config::Config;

/// 宿主发来的请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub protocol: u32,
    #[serde(default)]
    pub run_id: Option<String>,
    /// 宿主当前时间（Unix 毫秒）。采集器不要自己造时间（§2.5）。
    #[serde(default)]
    pub now: Option<i64>,
    /// `oneshot` | `stream`（§4）
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub interval_ms: Option<u64>,
    #[serde(default)]
    pub config: Option<Config>,
    #[serde(default)]
    #[allow(dead_code)]
    pub state: Option<serde_json::Value>,
    #[serde(default)]
    pub limits: Option<Limits>,
    /// 数据源名（协议 §2.2 的非必填字段）。用来给自管数据库命名、写日志。
    #[serde(default)]
    pub datasource: Option<String>,
}

fn default_mode() -> String {
    "oneshot".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    #[serde(default)]
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

    pub fn datasource(&self) -> String {
        self.datasource
            .clone()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "default".to_string())
    }
}
