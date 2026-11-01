//! `--describe` 自描述（协议 §8.1）。

use serde_json::{Value, json};

pub fn describe() -> Value {
    json!({
        "protocol": 1,
        "kind": "markets",
        "title": "预测市场",
        "version": env!("CARGO_PKG_VERSION"),
        "modes": ["oneshot", "stream"],
        "defaultMode": "oneshot",
        // 5 分钟：预测市场变动远慢于此；采集器还会按 config.intervalMs 再限流一次。
        "defaultIntervalMs": 300000,
        "output": "both",
        "recordKinds": [
            {
                "kind": "markets.prediction_market",
                "help": "Polymarket/Kalshi 二元预测市场快照（YES 隐含概率/成交额/收盘时间）",
                "persist": true,
                "maxRetentionMs": 2592000000_i64,  // 30 天
            },
        ],
        "configSchema": {
            "type": "object",
            "properties": {
                "intervalMs": {
                    "type": "integer", "title": "两次抓取的最小间隔(ms)",
                    "default": 300000, "minimum": 60000
                },
                "timeoutMs": {
                    "type": "integer", "title": "单个请求超时(ms)",
                    "default": 15000, "minimum": 1000, "maximum": 120000
                },
                "userAgent": {
                    "type": "string", "title": "User-Agent",
                    "default": concat!("viacli-markets/", env!("CARGO_PKG_VERSION"), " (https://github.com/zeaphoo/viacli)")
                },
                "maxMarkets": {
                    "type": "integer", "title": "每个源单轮最多市场数",
                    "default": 150, "minimum": 1, "maximum": 20000
                },
                "pageSize": {
                    "type": "integer", "title": "每页请求条数",
                    "default": 50, "minimum": 1, "maximum": 1000
                },
                "pageDelayMs": {
                    "type": "integer", "title": "翻页之间的停顿(ms)",
                    "default": 50, "minimum": 0, "maximum": 5000
                },
                "maxBodyBytes": {
                    "type": "integer", "title": "单个响应的字节上限",
                    "default": 16777216, "minimum": 65536
                },
                "maxRetries": {
                    "type": "integer", "title": "可重试错误的最大重试次数",
                    "default": 2, "minimum": 0, "maximum": 8
                },
                "enablePolymarket": {
                    "type": "boolean", "title": "启用 Polymarket", "default": true
                },
                "enableKalshi": {
                    "type": "boolean", "title": "启用 Kalshi", "default": true
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
        assert_eq!(d["kind"], "markets");
        assert_eq!(d["output"], "both");
        assert_eq!(d["configSchema"]["type"], "object");
        assert!(
            d["recordKinds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|k| k["kind"] == "markets.prediction_market")
        );
    }
}
