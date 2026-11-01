//! `--describe` 自描述（协议 §8.1）。
//!
//! 配置字段与引擎的 [`engine::config::Config`] 一一对应（camelCase）。

use crate::emit::{R_FOREVER, R_SNAPSHOT, R_WEEK};
use serde_json::{Map, Value, json};

/// 记录类型清单（协议 §8.1 的 `recordKinds`）。
fn record_kinds() -> Value {
    let mut out = Vec::new();
    for (kind, help, retention) in [
        (
            "networkdevice.endpoint",
            "一个 IP 的端点快照：在线状态、MAC/厂商/操作系统/类型、主机名/型号/服务、开放端口",
            R_SNAPSHOT,
        ),
        (
            "networkdevice.device",
            "一台设备的快照：由共享 MAC/主机名/型号归并出的多个 IP、类型与信任标记",
            R_SNAPSHOT,
        ),
        (
            "networkdevice.change",
            "变化时间线：发现/上线/离线/识别出新信息——不可再生，永久保留",
            R_FOREVER,
        ),
        (
            "networkdevice.scan",
            "每轮扫描的汇总：运行的源数、观测数、端点/设备数、耗时",
            R_WEEK,
        ),
    ] {
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
        "kind": "networkdevice",
        "title": "网络设备发现（IP/端口/设备/厂商）",
        "version": env!("CARGO_PKG_VERSION"),
        "modes": ["oneshot", "stream"],
        "defaultMode": "stream",
        "defaultIntervalMs": 60000,
        "output": "both",
        "recordKinds": record_kinds(),
        "configSchema": {
            "type": "object",
            "properties": {
                "ifaces": {
                    "type": "array", "title": "扫描网卡",
                    "help": "留空 = 默认路由那张。多网卡/隧道环境可显式指定",
                    "items": { "type": "string" }, "default": []
                },
                "intensity": {
                    "type": "integer", "title": "扫描强度（1 温和 ~ 5 激进）",
                    "help": "统一缩放各发现源的周期", "default": 3, "minimum": 1, "maximum": 5
                },
                "sources": {
                    "type": "array", "title": "启用的发现源",
                    "items": { "enum": ["neigh", "mdns", "ssdp", "sweep", "probe"] },
                    "default": []
                },
                "extraSubnets": {
                    "type": "array", "title": "额外扫描网段（L3 sweep，可跨路由器）",
                    "items": { "type": "string" }, "default": []
                },
                "neighBaseline": { "type": "boolean", "title": "并入内核邻居表", "default": true },
                "neighSweep": {
                    "type": "boolean", "title": "先 TCP 扫一遍填充 ARP 缓存", "default": true
                },
                "neighSweepTimeoutMs": { "type": "integer", "title": "邻居预扫超时(ms)", "default": 300, "minimum": 1 },
                "neighConcurrency": { "type": "integer", "title": "邻居预扫并发", "default": 64, "minimum": 1 },
                "neighMaxTargets": { "type": "integer", "title": "邻居预扫地址上限", "default": 1024, "minimum": 1 },
                "mdnsListenMs": { "type": "integer", "title": "每轮 mDNS 监听(ms)", "default": 500, "minimum": 1 },
                "ssdpListenMs": { "type": "integer", "title": "每轮 SSDP 监听(ms)", "default": 2500, "minimum": 1 },
                "ssdpDescriptions": { "type": "boolean", "title": "抓取 UPnP 描述 XML", "default": true },
                "sweepPorts": {
                    "type": "array", "title": "L3 sweep 探测端口",
                    "items": { "type": ["integer", "string"] },
                    "default": [80, 443, 22, 445, 3389, 8080, 8443]
                },
                "sweepTimeoutMs": { "type": "integer", "title": "L3 sweep 超时(ms)", "default": 500, "minimum": 1 },
                "sweepConcurrency": { "type": "integer", "title": "L3 sweep 并发", "default": 128, "minimum": 1 },
                "sweepMaxTargets": { "type": "integer", "title": "L3 sweep 地址上限", "default": 4096, "minimum": 1 },
                "offlineAfterSecs": {
                    "type": ["integer", "null"], "title": "多久没见到就判离线(s)",
                    "help": "留空 = 由最慢的源推导（2× 周期，最少 60s）", "default": null
                },
                "resolveDns": { "type": "boolean", "title": "反向 DNS（主机名）", "default": true },
                "portProbe": {
                    "type": "boolean", "title": "主动端口/指纹探测",
                    "help": "会主动连接主机；同时是 Web/TLS/SNMP/Modbus/BACnet/NetBIOS 探测的同意开关",
                    "default": true
                },
                "probePorts": {
                    "type": "array", "title": "端口探测清单",
                    "items": { "type": ["integer", "string"] }, "default": ["common"]
                },
                "probeTimeoutMs": { "type": "integer", "title": "单端口探测超时(ms)", "default": 250, "minimum": 1 },
                "identifyBudget": { "type": "integer", "title": "每轮最多探测的主机数", "default": 64, "minimum": 1 },
                "probeTtlSecs": {
                    "type": ["integer", "null"], "title": "同一地址重探间隔(s)",
                    "default": 21600
                },
                "trimHistHours": { "type": ["integer", "null"], "title": "事件保留小时数", "default": null },
                "historySampleSecs": {
                    "type": "integer", "title": "快照刷新间隔(s)",
                    "help": "未变化的端点/设备至少间隔多久重新产出一条快照", "default": 300, "minimum": 0
                },
                "historyRetainHours": { "type": ["integer", "null"], "title": "历史保留小时数", "default": 720 },
                "ouiFile": { "type": ["string", "null"], "title": "自定义 IEEE OUI CSV", "default": null },
                "probesFile": { "type": ["string", "null"], "title": "自定义 nmap-service-probes", "default": null }
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
        assert_eq!(d["kind"], "networkdevice");
        assert_eq!(d["output"], "both");
        let modes = d["modes"].as_array().unwrap();
        assert!(modes.contains(&json!("oneshot")) && modes.contains(&json!("stream")));
        assert_eq!(d["configSchema"]["type"], "object");
        assert!(d["configSchema"]["properties"]["intensity"]["minimum"].is_number());
        assert!(d["configSchema"]["properties"].get("banner").is_none());
        assert!(d["configSchema"]["properties"].get("dbPath").is_none());
    }

    #[test]
    fn record_kinds_declare_retention() {
        let arr = record_kinds();
        let arr = arr.as_array().unwrap();
        assert!(arr.iter().any(|k| k["kind"] == "networkdevice.endpoint"));
        let change = arr.iter().find(|k| k["kind"] == "networkdevice.change").unwrap();
        assert!(change["maxRetentionMs"].is_null());
    }
}
