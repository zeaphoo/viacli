//! 请求与配置（见 `docs/07-collector-protocol.md` §2.2、§11.5）。

use serde::Deserialize;

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
    /// 单轮产出的宽限期；本采集器的网络请求已有自己的 timeout，这里只收下不误用。
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

/// 一个订阅源的配置。
///
/// `name` 只是展示用的可读名字（默认源的标题）；为空时用 feed 自身的 `<title>`。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FeedConfig {
    pub url: String,
    #[serde(default)]
    pub name: Option<String>,
    pub enabled: bool,
}

impl Default for FeedConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            name: None,
            enabled: true,
        }
    }
}

/// 采集器配置（对应 `--describe` 里的 `configSchema`）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// 用户额外添加的订阅源（与默认源合并，按 URL 去重）。
    pub feeds: Vec<FeedConfig>,
    /// 是否使用内置默认源。
    pub use_default_feeds: bool,
    /// 每个源最多产出多少条**新**条目。
    pub max_items_per_feed: usize,
    /// 两次真正发起网络请求之间的最小间隔。宿主按时调用 `oneshot` 时靠它限流，
    /// 避免把订阅源当成秒级指标来轮询。
    pub interval_ms: u64,
    /// 单个 HTTP 请求超时。
    pub timeout_ms: u64,
    /// 并发抓取的订阅源数量上限。
    pub max_concurrency: usize,
    /// 是否把摘要/正文写进记录（正文可能很大，默认关）。
    pub include_content: bool,
    /// 摘要/正文截断到多少个字符（0 = 不截断）。
    pub max_content_chars: usize,
    /// 单个响应的字节上限，防御超大响应把内存撑爆。
    pub max_body_bytes: usize,
    /// HTTP User-Agent。
    pub user_agent: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            feeds: Vec::new(),
            use_default_feeds: true,
            // 20 条/源：一次"未读"的量级，再多也不会有人从头翻。
            max_items_per_feed: 20,
            // 15 分钟：资讯类源的更新频率远低于此，更密只是白打上游。
            interval_ms: 900_000,
            timeout_ms: 15_000,
            // 8：默认 10 个源大约两轮并发就能跑完，又不会同时开几十条连接。
            max_concurrency: 8,
            include_content: false,
            max_content_chars: 2000,
            max_body_bytes: 5 * 1024 * 1024,
            user_agent: concat!("viacli-rss/", env!("CARGO_PKG_VERSION")).to_string(),
        }
    }
}

impl Config {
    /// 合并默认源与用户源，按 URL 去重（用户配置优先，可覆盖名字）。
    pub fn resolve_feeds(&self) -> Vec<FeedConfig> {
        let mut out: Vec<FeedConfig> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for f in &self.feeds {
            push_unique(&mut out, &mut seen, f);
        }
        if self.use_default_feeds {
            for f in crate::feeds::default_feeds() {
                push_unique(&mut out, &mut seen, &f);
            }
        }
        out
    }

    /// 校验配置；返回人类可读的错误（宿主会按退出码 2 处理，§3.4）。
    pub fn validate(&self) -> Result<(), String> {
        // 下限 1 分钟：订阅源不是秒级指标，低于这个值只是在打上游。
        if self.interval_ms < 60_000 {
            return Err(format!(
                "intervalMs 不能小于 60000（当前 {}）——订阅源不需要秒级轮询",
                self.interval_ms
            ));
        }
        if self.max_items_per_feed == 0 {
            return Err("maxItemsPerFeed 必须大于 0".into());
        }
        if !(1_000..=120_000).contains(&self.timeout_ms) {
            return Err(format!(
                "timeoutMs 必须在 1000..=120000 之间（当前 {}）",
                self.timeout_ms
            ));
        }
        if !(1..=32).contains(&self.max_concurrency) {
            return Err(format!(
                "maxConcurrency 必须在 1..=32 之间（当前 {}）",
                self.max_concurrency
            ));
        }
        if self.max_body_bytes < 1024 {
            return Err(format!(
                "maxBodyBytes 太小（当前 {}），一个正常的订阅源都放不下",
                self.max_body_bytes
            ));
        }
        if self.user_agent.trim().is_empty() {
            return Err("userAgent 不能为空".into());
        }
        for f in &self.feeds {
            validate_url(&f.url)?;
        }
        let feeds = self.resolve_feeds();
        if feeds.is_empty() {
            return Err(
                "没有可用的订阅源：feeds 为空且 useDefaultFeeds=false。至少需要一个地址。"
                    .into(),
            );
        }
        Ok(())
    }
}

/// 把一个订阅源加入结果，跳过禁用项与重复 URL。
fn push_unique(out: &mut Vec<FeedConfig>, seen: &mut Vec<String>, f: &FeedConfig) {
    let url = f.url.trim();
    if url.is_empty() || !f.enabled {
        return;
    }
    if seen.iter().any(|s| s == url) {
        return;
    }
    seen.push(url.to_string());
    out.push(FeedConfig {
        url: url.to_string(),
        name: f.name.clone(),
        enabled: true,
    });
}

/// 校验一个订阅源地址：必须是 http(s) 且能被 URL 解析器接受。
fn validate_url(raw: &str) -> Result<(), String> {
    let url = raw.trim();
    if url.is_empty() {
        return Err("feeds[].url 不能为空".into());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!("订阅源 {url:?} 必须以 http:// 或 https:// 开头"));
    }
    reqwest::Url::parse(url).map_err(|e| format!("订阅源 {url:?} 不是合法 URL: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane_and_resolve_to_feeds() {
        let c = Config::default();
        assert!(c.validate().is_ok());
        let feeds = c.resolve_feeds();
        assert!(feeds.len() >= 5, "默认应解析出多个源");
        assert!(feeds.iter().all(|f| f.url.starts_with("https://")));
    }

    #[test]
    fn camel_case_request_parses_and_ignores_unknown_fields() {
        let raw = r#"{
            "protocol": 1, "runId": "r1", "now": 123, "mode": "oneshot",
            "config": {"maxItemsPerFeed": 3, "feeds": [{"url": "https://a.example/feed"}]},
            "unexpected": true,
            "limits": {"timeoutMs": 5000}
        }"#;
        let req: Request = serde_json::from_str(raw).unwrap();
        assert_eq!(req.protocol, 1);
        assert_eq!(req.mode, "oneshot");
        let cfg = req.config.unwrap();
        assert_eq!(cfg.max_items_per_feed, 3);
        assert_eq!(cfg.feeds[0].url, "https://a.example/feed");
        assert!(cfg.feeds[0].enabled, "enabled 缺省应为 true");
        assert_eq!(req.limits.unwrap().timeout_ms, Some(5000));
    }

    #[test]
    fn user_feeds_merge_with_defaults_and_dedupe_by_url() {
        let c = Config {
            feeds: vec![FeedConfig {
                url: "https://sspai.com/feed".into(),
                name: Some("我的少数派".into()),
                enabled: true,
            }],
            ..Default::default()
        };
        let resolved = c.resolve_feeds();
        let sspai: Vec<_> = resolved
            .iter()
            .filter(|f| f.url == "https://sspai.com/feed")
            .collect();
        assert_eq!(sspai.len(), 1, "重复 URL 只能保留一个");
        assert_eq!(sspai[0].name.as_deref(), Some("我的少数派"), "用户配置优先");
    }

    #[test]
    fn disabled_defaults_can_be_turned_off() {
        let c = Config {
            use_default_feeds: false,
            feeds: vec![FeedConfig {
                url: "https://a.example/feed".into(),
                enabled: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(c.resolve_feeds().len(), 1);
        assert!(c.validate().is_ok());
    }

    #[test]
    fn invalid_config_is_rejected_with_readable_message() {
        // 一致性检查器用的默认坏配置就是 intervalMs=1
        let c = Config {
            interval_ms: 1,
            ..Default::default()
        };
        assert!(c.validate().unwrap_err().contains("intervalMs"));

        let c = Config {
            use_default_feeds: false,
            feeds: vec![],
            ..Default::default()
        };
        assert!(c.validate().unwrap_err().contains("没有可用的订阅源"));

        let c = Config {
            feeds: vec![FeedConfig {
                url: "ftp://a.example/feed".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(c.validate().unwrap_err().contains("http"));

        let c = Config {
            max_concurrency: 0,
            ..Default::default()
        };
        assert!(c.validate().unwrap_err().contains("maxConcurrency"));
    }
}
