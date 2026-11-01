//! `--describe` 自描述（协议 §8.1）。
//!
//! 宿主拿它生成配置表单、决定运行模式、知道记录类型该留多久。
//! 这里刻意只描述"采集器能做什么"，不掺任何运行时状态。

use crate::protocol::Retention;
use serde_json::{Map, Value, json};

/// 记录类型清单（协议 §8.1 的 `recordKinds`）。
///
/// 记录类型是**静态**的（由代码决定），所以在 `--describe` 里声明；
/// 序列是动态的，随 `series` 消息声明。两者用同一套保留字段（协议 §2.8）。
fn record_kinds() -> Value {
    let mut out = Vec::new();
    for (kind, help, retention) in [(
        "rss.item",
        "RSS/Atom 订阅条目（标题/链接/发布时间）——不可再生，永久保留",
        Retention::Forever,
    )] {
        let mut obj = Map::new();
        obj.insert("kind".into(), json!(kind));
        obj.insert("help".into(), json!(help));
        for (k, v) in retention.fields() {
            obj.insert(k.to_string(), v);
        }
        out.push(Value::Object(obj));
    }
    Value::Array(out)
}

pub fn describe() -> Value {
    json!({
        "protocol": 1,
        "kind": "rss",
        "title": "RSS/Atom 订阅",
        "version": env!("CARGO_PKG_VERSION"),
        "modes": ["oneshot", "stream"],
        // 拉取型、无进程内状态 → oneshot 是自然模式（DESIGN.md §6.2.13.5）
        "defaultMode": "oneshot",
        // 15 分钟：资讯源更新频率远低于此；宿主按时调用时采集器还会按
        // config.intervalMs 再自行限流一次，避免默认 60s 的调度打上游。
        "defaultIntervalMs": 900000,
        "output": "both",
        "recordKinds": record_kinds(),
        "configSchema": {
            "type": "object",
            "properties": {
                "feeds": {
                    "type": "array",
                    "title": "额外订阅源",
                    "help": "在默认源之外追加的 RSS/Atom 地址；按 URL 去重",
                    "items": {
                        "type": "object",
                        "properties": {
                            "url": { "type": "string", "title": "地址" },
                            "name": { "type": "string", "title": "显示名（可选）" },
                            "enabled": { "type": "boolean", "title": "启用", "default": true }
                        },
                        "required": ["url"]
                    },
                    "default": []
                },
                "useDefaultFeeds": {
                    "type": "boolean", "title": "使用内置默认源", "default": true
                },
                "maxItemsPerFeed": {
                    "type": "integer", "title": "每个源最多产出的条目数",
                    "default": 20, "minimum": 1
                },
                "intervalMs": {
                    "type": "integer", "title": "两次抓取的最小间隔(ms)",
                    "default": 900000, "minimum": 60000
                },
                "timeoutMs": {
                    "type": "integer", "title": "单个请求超时(ms)",
                    "default": 15000, "minimum": 1000, "maximum": 120000
                },
                "maxConcurrency": {
                    "type": "integer", "title": "并发抓取数",
                    "default": 8, "minimum": 1, "maximum": 32
                },
                "includeContent": {
                    "type": "boolean", "title": "记录里包含摘要/正文", "default": false
                },
                "maxContentChars": {
                    "type": "integer", "title": "摘要/正文截断字符数（0=不截断）",
                    "default": 2000, "minimum": 0
                },
                "maxBodyBytes": {
                    "type": "integer", "title": "单个响应的字节上限",
                    "default": 5242880, "minimum": 1024
                },
                "userAgent": {
                    "type": "string", "title": "User-Agent",
                    "default": concat!("viacli-rss/", env!("CARGO_PKG_VERSION"))
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_is_self_consistent() {
        let d = describe();
        assert_eq!(d["protocol"], 1);
        assert_eq!(d["kind"], "rss");
        assert_eq!(d["output"], "both");
        let modes = d["modes"].as_array().unwrap();
        assert!(modes.contains(&json!("oneshot")));
        assert!(modes.contains(&json!("stream")));
        let default_mode = d["defaultMode"].as_str().unwrap();
        assert!(modes.iter().any(|m| m == default_mode));
        assert_eq!(d["configSchema"]["type"], "object");
        assert!(d["configSchema"]["properties"]["intervalMs"]["minimum"].is_number());
        let kinds = d["recordKinds"].as_array().unwrap();
        assert!(kinds.iter().any(|k| k["kind"] == "rss.item"));
    }

    #[test]
    fn record_kind_declares_permanent_retention() {
        let kinds = record_kinds();
        let item = kinds
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["kind"] == "rss.item")
            .unwrap();
        assert!(item["maxRetentionMs"].is_null(), "条目不可再生，应永久保留");
    }
}
