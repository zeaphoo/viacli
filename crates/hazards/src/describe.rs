//! `--describe` 自描述（协议 §8.1）。
//!
//! 宿主拿它生成配置表单、决定运行模式、知道记录类型该留多久。
//! 这里只描述"采集器能做什么"，不掺任何运行时状态。

use serde_json::{Value, json};

pub fn describe() -> Value {
    json!({
        "protocol": 1,
        "kind": "hazards",
        "title": "灾害与环境",
        "version": env!("CARGO_PKG_VERSION"),
        "modes": ["oneshot", "stream"],
        // 拉取型、无进程内必需状态 → oneshot 是自然模式（DESIGN.md §6.2.13.5）。
        "defaultMode": "oneshot",
        // 2 分钟：USGS/NWS 的更新频率远低于此；宿主按时调用时采集器还会按
        // config.intervalMs 再自行限流一次，避免默认调度打上游。
        "defaultIntervalMs": 120000,
        "output": "both",
        "recordKinds": [
            {
                "kind": "hazard.earthquake",
                "help": "USGS 过去 24h 地震事件（震级/位置/深度/海啸旗标）",
                "persist": true,
                "maxRetentionMs": 2592000000_i64,  // 30 天
            },
            {
                "kind": "hazard.weather_alert",
                "help": "NWS 当前生效天气预警（类型/严重度/区域/代表点）",
                "persist": true,
                "maxRetentionMs": 604800000_i64,   // 7 天
            },
        ],
        "configSchema": {
            "type": "object",
            "properties": {
                "intervalMs": {
                    "type": "integer", "title": "两次抓取的最小间隔(ms)",
                    "default": 120000, "minimum": 60000
                },
                "timeoutMs": {
                    "type": "integer", "title": "单个请求超时(ms)",
                    "default": 15000, "minimum": 1000, "maximum": 120000
                },
                "userAgent": {
                    "type": "string", "title": "User-Agent",
                    "default": concat!("viacli-hazards/", env!("CARGO_PKG_VERSION"), " (https://github.com/zeaphoo/viacli)")
                },
                "minMagnitude": {
                    "type": "number", "title": "地震最小震级",
                    "default": 2.5, "minimum": 0, "maximum": 10
                },
                "maxRecords": {
                    "type": "integer", "title": "每类记录最大条数",
                    "default": 2000, "minimum": 1, "maximum": 20000
                },
                "maxBodyBytes": {
                    "type": "integer", "title": "单个响应的字节上限",
                    "default": 16777216, "minimum": 65536
                },
                "maxRetries": {
                    "type": "integer", "title": "可重试错误的最大重试次数",
                    "default": 2, "minimum": 0, "maximum": 8
                },
                "enableEarthquakes": {
                    "type": "boolean", "title": "启用 USGS 地震", "default": true
                },
                "enableWeatherAlerts": {
                    "type": "boolean", "title": "启用 NWS 天气预警", "default": true
                },
                "enableSpaceWeather": {
                    "type": "boolean", "title": "启用 NOAA SWPC 空间天气", "default": true
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
        assert_eq!(d["kind"], "hazards");
        assert_eq!(d["output"], "both");
        let modes = d["modes"].as_array().unwrap();
        assert!(modes.iter().any(|m| m == "oneshot"));
        let default_mode = d["defaultMode"].as_str().unwrap();
        assert!(modes.iter().any(|m| m == default_mode));
        assert_eq!(d["configSchema"]["type"], "object");
    }

    #[test]
    fn record_kinds_are_declared_with_retention() {
        let kinds = describe()["recordKinds"].clone();
        for kind in kinds.as_array().unwrap() {
            assert!(kind["kind"].is_string());
            assert!(kind["maxRetentionMs"].is_i64() || kind["maxRetentionMs"].is_null());
        }
    }
}
