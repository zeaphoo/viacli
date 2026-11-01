//! `--describe` 自描述（协议 §8.1）。

use serde_json::{Value, json};

pub fn describe() -> Value {
    json!({
        "protocol": 1,
        "kind": "space",
        "title": "太空",
        "version": env!("CARGO_PKG_VERSION"),
        "modes": ["stream", "oneshot"],
        // TLE 需在进程内缓存（不能每轮都打 CelesTrak），所以默认 stream（DESIGN §6.2.13.5）。
        "defaultMode": "stream",
        "defaultIntervalMs": 60000,
        "output": "both",
        "recordKinds": [
            {
                "kind": "space.satellite",
                "help": "卫星目录 + SGP4 星下点（名称/NORAD/倾角/经纬高），位置可再生",
                "persist": true,
                "maxRetentionMs": 86400000_i64,     // 1 天
            },
            {
                "kind": "space.ground_station",
                "help": "SatNOGS/TinyGS 地面站（名称/经纬度/观测数），半静态",
                "persist": true,
                "maxRetentionMs": 2592000000_i64,   // 30 天
            },
        ],
        "configSchema": {
            "type": "object",
            "properties": {
                "intervalMs": {
                    "type": "integer", "title": "位置更新间隔(ms)",
                    "default": 60000, "minimum": 30000
                },
                "tleRefreshMs": {
                    "type": "integer", "title": "TLE 刷新间隔(ms)",
                    "default": 21600000, "minimum": 1800000
                },
                "timeoutMs": {
                    "type": "integer", "title": "单个请求超时(ms)",
                    "default": 20000, "minimum": 1000, "maximum": 120000
                },
                "userAgent": {
                    "type": "string", "title": "User-Agent",
                    "default": concat!("viacli-space/", env!("CARGO_PKG_VERSION"), " (https://github.com/zeaphoo/viacli)")
                },
                "celestrakGroups": {
                    "type": "array", "title": "CelesTrak 分组",
                    "help": "可选：stations / weather / gps-ops / science / active（active 上万颗，慎用）",
                    "items": { "type": "string" },
                    "default": ["stations"]
                },
                "maxSatellites": {
                    "type": "integer", "title": "单轮最多传播卫星数",
                    "default": 500, "minimum": 1, "maximum": 20000
                },
                "maxGroundStations": {
                    "type": "integer", "title": "地面站上限",
                    "default": 2000, "minimum": 1
                },
                "maxBodyBytes": {
                    "type": "integer", "title": "单个响应的字节上限",
                    "default": 25165824, "minimum": 65536
                },
                "maxRetries": {
                    "type": "integer", "title": "可重试错误的最大重试次数",
                    "default": 1, "minimum": 0, "maximum": 8
                },
                "includeTle": {
                    "type": "boolean", "title": "记录里包含原始 TLE", "default": false
                },
                "enableGroundStations": {
                    "type": "boolean", "title": "启用 SatNOGS 地面站（单次约 3.4MB）", "default": false
                },
                "enableTinygs": {
                    "type": "boolean", "title": "启用 TinyGS（端点不稳定）", "default": false
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
        assert_eq!(d["kind"], "space");
        assert_eq!(d["output"], "both");
        let modes = d["modes"].as_array().unwrap();
        assert!(modes.iter().any(|m| m == "stream"));
        let dm = d["defaultMode"].as_str().unwrap();
        assert!(modes.iter().any(|m| m == dm));
        assert_eq!(d["configSchema"]["type"], "object");
    }
}
