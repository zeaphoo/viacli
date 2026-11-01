//! 把引擎的清点结果映射成 keendeck 采集器协议消息。

use std::io::{self, Write};

use serde_json::{Value, json};

use crate::engine::inventory::{Device, Endpoint, Event, Inventory};
use crate::engine::model::{DiscoverySource, PortInfo};
use crate::protocol::{Kind, Retention, Sink, Unit};

/// 抖动读数（本轮扫描量/耗时）：1 天。
pub const R_DAY: Retention = Retention::Max(86_400_000);
/// 当前清单的汇总：30 天。
pub const R_MONTH: Retention = Retention::Max(30 * 86_400_000);
/// 端点/设备快照：30 天。
pub const R_SNAPSHOT: Retention = Retention::Max(30 * 86_400_000);
/// 每轮扫描汇总：7 天。
pub const R_WEEK: Retention = Retention::Max(7 * 86_400_000);
/// 事件时间线：永久（不可再生）。
pub const R_FOREVER: Retention = Retention::Forever;

/// 声明本采集器会用到的全部序列。
pub fn declare_series<W: Write>(sink: &mut Sink<W>) -> io::Result<()> {
    for (name, help, retention) in [
        ("networkdevice/endpoints_known", "清单里登记过的 IP 端点总数", R_MONTH),
        ("networkdevice/endpoints_online", "在线的端点数", R_MONTH),
        ("networkdevice/devices_known", "归并出来的设备总数", R_MONTH),
        ("networkdevice/devices_online", "在线的设备数", R_MONTH),
        ("networkdevice/ports_open", "当前已知的开放端口总数", R_MONTH),
        ("networkdevice/sources_run", "本轮实际运行的发现源数量", R_DAY),
        ("networkdevice/observations", "本轮收到并并入清单的观测数", R_DAY),
        ("networkdevice/cycle_duration_seconds", "本轮耗时（秒）", R_DAY),
    ] {
        let unit = if name == "networkdevice/cycle_duration_seconds" {
            Unit::Seconds
        } else {
            Unit::Count
        };
        sink.declare(name, Kind::Gauge, unit, help, retention)?;
    }
    Ok(())
}

/// 产出当前总览样本。
pub fn emit_totals<W: Write>(sink: &mut Sink<W>, inv: &Inventory, now: Option<i64>) -> io::Result<()> {
    let endpoints_known = inv.endpoints().count() as i64;
    let endpoints_online = inv.endpoints().filter(|e| e.online).count() as i64;
    let devices_known = inv.devices().count() as i64;
    let devices_online = inv.devices().filter(|d| d.online).count() as i64;
    let ports: usize = inv
        .endpoints()
        .filter(|e| e.online)
        .map(|e| e.fingerprint.ports.len())
        .sum();

    sink.sample_i64("networkdevice/endpoints_known", now, endpoints_known)?;
    sink.sample_i64("networkdevice/endpoints_online", now, endpoints_online)?;
    sink.sample_i64("networkdevice/devices_known", now, devices_known)?;
    sink.sample_i64("networkdevice/devices_online", now, devices_online)?;
    sink.sample_i64("networkdevice/ports_open", now, ports as i64)
}

fn sources_json(sources: &[DiscoverySource]) -> Value {
    json!(sources.iter().map(|s| s.label()).collect::<Vec<_>>())
}

fn ports_json(ports: &[PortInfo]) -> Value {
    json!(
        ports
            .iter()
            .map(|p| {
                let mut o = json!({"port": p.port});
                if let Some(s) = &p.service {
                    o["service"] = json!(s);
                }
                if let Some(b) = &p.banner {
                    o["banner"] = json!(b);
                }
                o
            })
            .collect::<Vec<_>>()
    )
}

/// 产出一条端点快照。
pub fn emit_endpoint<W: Write>(
    sink: &mut Sink<W>,
    endpoint: &Endpoint,
    status: &str,
) -> io::Result<()> {
    let fp = &endpoint.fingerprint;
    let mut payload = json!({
        "ip": endpoint.ip.to_string(),
        "online": endpoint.online,
        "status": status,
        "kind": fp.kind.label(),
        "sources": sources_json(&endpoint.sources),
        "sightings": endpoint.sightings,
        "firstSeenMs": endpoint.first_seen.timestamp_millis(),
        "lastSeenMs": endpoint.last_seen.timestamp_millis(),
        "openPortCount": fp.ports.len(),
        "openPorts": ports_json(&fp.ports),
    });
    if let Some(iface) = &endpoint.iface {
        payload["iface"] = json!(iface);
    }
    if let Some(mac) = fp.mac {
        payload["mac"] = json!(mac.to_string());
    }
    if let Some(vendor) = &fp.vendor {
        payload["vendor"] = json!(vendor.value);
    }
    if let Some(os) = &fp.os {
        payload["os"] = json!(os.value);
    }
    if let Some(h) = fp.hostname() {
        payload["hostname"] = json!(h);
    }
    if let Some(m) = fp.model() {
        payload["model"] = json!(m);
    }
    if !fp.services.is_empty() {
        payload["services"] = json!(fp.services);
    }
    sink.record(
        "networkdevice.endpoint",
        Some(&endpoint.ip.to_string()),
        Some(endpoint.last_seen.timestamp_millis()),
        payload,
    )
}

/// 产出一条设备快照。
pub fn emit_device<W: Write>(sink: &mut Sink<W>, device: &Device, status: &str) -> io::Result<()> {
    let ips: Vec<String> = device.ips.iter().map(|ip| ip.to_string()).collect();
    let mut payload = json!({
        "id": device.id,
        "ips": ips,
        "kind": device.kind.label(),
        "known": device.known,
        "online": device.online,
        "status": status,
        "firstSeenMs": device.first_seen.timestamp_millis(),
        "lastSeenMs": device.last_seen.timestamp_millis(),
    });
    if let Some(name) = &device.name {
        payload["name"] = json!(name);
    }
    if let Some(merged) = device.merged_by {
        payload["mergedBy"] = json!(merged.label());
    }
    if let Some(confidence) = device.confidence {
        payload["confidence"] = json!(confidence);
    }
    sink.record(
        "networkdevice.device",
        Some(&device.id),
        Some(device.last_seen.timestamp_millis()),
        payload,
    )
}

/// 产出一条事件（引擎的 `Discovered/Online/Offline/Identified/Edited` 时间线）。
pub fn emit_event<W: Write>(sink: &mut Sink<W>, event: &Event) -> io::Result<()> {
    let mut payload = json!({
        "change": event.kind.label(),
        "detail": event.detail,
        "atMs": event.at.timestamp_millis(),
        "seq": event.seq,
    });
    if let Some(device) = &event.device {
        payload["device"] = json!(device);
    }
    if let Some(ip) = event.ip {
        payload["ip"] = json!(ip.to_string());
    }
    let key = format!("{}#{}", event.kind.label(), event.seq);
    sink.record(
        "networkdevice.change",
        Some(&key),
        Some(event.at.timestamp_millis()),
        payload,
    )
}
