//! 采集器**自管的清点数据库**——本采集器与其它采集器最大的不同。
//!
//! # 为什么一个采集器需要自己的数据库（协议 §5 的第二种状态形式）
//!
//! 扫描很贵、清单很大，而且需要 `first_seen`/`last_seen`、端口开合、厂商、
//! 设备归并与事件时间线这些"过去发生过什么"的事实——它们既装不进 64 KiB 的
//! `state`，也推不出来。因此本采集器把清单落在自己的 SQLite 文件里，
//! **绝不写宿主库**；宿主拿到的仍只是 NDJSON 记录流。
//!
//! 这里刻意采用**整实体 JSON 持久化**（而不是把端点的每个字段摊成列）：
//! 清点引擎的 `Endpoint`/`Device` 数据模型很丰富，摊平会带来大量迁移与映射，
//! 而采集器每轮本来就会把整份清单读进内存。JSON 列保留了引擎的完整模型，
//! 也让上游的 schema 变化不需要在这里跟着改。
//!
//! 数据库位置：平台数据目录下 `collectors/networkdevice/<数据源名>.db`。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};

use crate::engine::inventory::{Device, Endpoint, Event, Inventory};

/// 自管数据库的默认落盘路径：viacli 数据目录 + `networkdevice/<数据源名>.db`。
pub fn default_db_path(datasource: &str) -> PathBuf {
    data_base_dir()
        .join("networkdevice")
        .join(format!("{}.db", sanitize_name(datasource)))
}

/// viacli 数据根目录：`VIACLI_DATA_ROOT` > `XDG_DATA_HOME/viacli` > `~/.local/share/viacli`。
fn data_base_dir() -> PathBuf {
    if let Ok(v) = std::env::var("VIACLI_DATA_ROOT")
        && !v.is_empty()
    {
        return PathBuf::from(v);
    }
    #[cfg(target_os = "windows")]
    if let Ok(appdata) = std::env::var("APPDATA") {
        return PathBuf::from(appdata).join("viacli");
    }
    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
            && !xdg.is_empty()
        {
            return PathBuf::from(xdg).join("viacli");
        }
        if let Ok(home) = std::env::var("HOME")
            && !home.is_empty()
        {
            return PathBuf::from(home).join(".local/share/viacli");
        }
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".viacli")
}

fn sanitize_name(name: &str) -> String {
    let mut out: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        out = "default".to_string();
    }
    out.truncate(64);
    out
}

/// 打开数据库后读回的内存状态。
pub struct Loaded {
    pub inventory: Inventory,
    /// 端点/设备最近一次产出到宿主的时刻（跨运行抑制重复快照）。
    pub endpoint_emitted: HashMap<String, i64>,
    pub device_emitted: HashMap<String, i64>,
    /// 每个发现源上次运行的时刻（毫秒）。
    pub source_runs: HashMap<String, i64>,
}

pub struct Store {
    conn: Connection,
    path: PathBuf,
}

impl Store {
    pub fn open(path: &Path) -> Result<Store, String> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建数据库目录 {} 失败: {e}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .map_err(|e| format!("打开数据库 {} 失败: {e}", path.display()))?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;
        // 旧版采集器（端口扫描版）在同一路径下用过**不同的表结构**。
        // 清点库是可再生的缓存，遇到不兼容的旧表直接重建，而不是让采集器起不来。
        if table_exists(&conn, "endpoints")? && !column_exists(&conn, "endpoints", "json")? {
            conn.execute_batch(
                "DROP TABLE IF EXISTS endpoints;
                 DROP TABLE IF EXISTS devices;
                 DROP TABLE IF EXISTS events;
                 DROP TABLE IF EXISTS sources;",
            )
            .map_err(|e| format!("重建旧版数据库失败: {e}"))?;
        }
        conn.execute_batch(SCHEMA)
            .map_err(|e| format!("初始化数据库 schema 失败: {e}"))?;
        Ok(Store {
            conn,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 读回端点、设备、事件序号与各类游标。
    pub fn load(&self) -> Result<Loaded, String> {
        let mut endpoints: Vec<Endpoint> = Vec::new();
        let mut endpoint_emitted = HashMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT ip, json, last_emitted_ms FROM endpoints")
                .map_err(|e| format!("准备端点查询失败: {e}"))?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                })
                .map_err(|e| format!("查询端点失败: {e}"))?;
            for row in rows {
                let (ip, json, emitted) = row.map_err(|e| format!("读取端点失败: {e}"))?;
                let endpoint: Endpoint = serde_json::from_str(&json)
                    .map_err(|e| format!("端点 {ip} 的 JSON 损坏: {e}"))?;
                endpoint_emitted.insert(ip, emitted);
                endpoints.push(endpoint);
            }
        }

        let mut devices: Vec<Device> = Vec::new();
        let mut device_emitted = HashMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT id, json, last_emitted_ms FROM devices")
                .map_err(|e| format!("准备设备查询失败: {e}"))?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                })
                .map_err(|e| format!("查询设备失败: {e}"))?;
            for row in rows {
                let (id, json, emitted) = row.map_err(|e| format!("读取设备失败: {e}"))?;
                let device: Device =
                    serde_json::from_str(&json).map_err(|e| format!("设备 {id} 的 JSON 损坏: {e}"))?;
                device_emitted.insert(id, emitted);
                devices.push(device);
            }
        }

        let max_seq: Option<i64> = self
            .conn
            .query_row("SELECT MAX(seq) FROM events", [], |r| r.get(0))
            .optional()
            .map_err(|e| format!("读取事件序号失败: {e}"))?
            .flatten();
        let event_seq = max_seq.map(|s| s as u64 + 1).unwrap_or(1);

        let mut source_runs = HashMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT source, last_run_ms FROM sources")
                .map_err(|e| format!("准备发现源状态查询失败: {e}"))?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                .map_err(|e| format!("查询发现源状态失败: {e}"))?;
            for row in rows {
                let (source, at) = row.map_err(|e| format!("读取发现源状态失败: {e}"))?;
                source_runs.insert(source, at);
            }
        }

        Ok(Loaded {
            inventory: Inventory::resume(endpoints, devices, event_seq),
            endpoint_emitted,
            device_emitted,
            source_runs,
        })
    }

    /// 落盘整份清单：端点/设备 upsert，新事件追加，设备集合重新对齐。
    pub fn save(
        &mut self,
        inventory: &Inventory,
        endpoint_emitted: &HashMap<String, i64>,
        device_emitted: &HashMap<String, i64>,
        new_events: &[Event],
    ) -> Result<(), String> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| format!("开启事务失败: {e}"))?;

        for endpoint in inventory.endpoints() {
            let ip = endpoint.ip.to_string();
            let json = serde_json::to_string(endpoint)
                .map_err(|e| format!("序列化端点 {ip} 失败: {e}"))?;
            let emitted = endpoint_emitted.get(&ip).copied().unwrap_or(0);
            tx.execute(
                "INSERT INTO endpoints (ip, json, last_emitted_ms) VALUES (?1, ?2, ?3)
                 ON CONFLICT(ip) DO UPDATE SET json = excluded.json,
                                               last_emitted_ms = excluded.last_emitted_ms",
                params![ip, json, emitted],
            )
            .map_err(|e| format!("写入端点 {ip} 失败: {e}"))?;
        }

        // 设备集合整体对齐：归并不再产生的设备要删掉（端口/事件历史不动）。
        let ids: Vec<String> = inventory.devices().map(|d| d.id.clone()).collect();
        if ids.is_empty() {
            tx.execute("DELETE FROM devices", [])
                .map_err(|e| format!("清理设备失败: {e}"))?;
        } else {
            let placeholders = vec!["?"; ids.len()].join(",");
            let sql = format!("DELETE FROM devices WHERE id NOT IN ({placeholders})");
            tx.execute(&sql, rusqlite::params_from_iter(ids.iter()))
                .map_err(|e| format!("清理旧设备失败: {e}"))?;
        }
        for device in inventory.devices() {
            let json = serde_json::to_string(device)
                .map_err(|e| format!("序列化设备 {} 失败: {e}", device.id))?;
            let emitted = device_emitted.get(&device.id).copied().unwrap_or(0);
            tx.execute(
                "INSERT INTO devices (id, json, last_emitted_ms) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET json = excluded.json,
                                               last_emitted_ms = excluded.last_emitted_ms",
                params![device.id, json, emitted],
            )
            .map_err(|e| format!("写入设备 {} 失败: {e}", device.id))?;
        }

        for event in new_events {
            let json = serde_json::to_string(event)
                .map_err(|e| format!("序列化事件失败: {e}"))?;
            tx.execute(
                "INSERT OR REPLACE INTO events (seq, at_ms, json) VALUES (?1, ?2, ?3)",
                params![
                    event.seq as i64,
                    event.at.timestamp_millis(),
                    json
                ],
            )
            .map_err(|e| format!("写入事件失败: {e}"))?;
        }

        tx.commit().map_err(|e| format!("提交事务失败: {e}"))
    }

    pub fn set_source_run(&mut self, source: &str, at_ms: i64) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO sources (source, last_run_ms) VALUES (?1, ?2)
                 ON CONFLICT(source) DO UPDATE SET last_run_ms = excluded.last_run_ms",
                params![source, at_ms],
            )
            .map_err(|e| format!("写入发现源状态失败: {e}"))?;
        Ok(())
    }

    pub fn meta_i64(&self, key: &str) -> Result<Option<i64>, String> {
        let value: Option<String> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| {
                r.get(0)
            })
            .optional()
            .map_err(|e| format!("读取 meta {key} 失败: {e}"))?;
        Ok(value.and_then(|v| v.parse::<i64>().ok()))
    }

    pub fn set_meta_i64(&mut self, key: &str, value: i64) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value.to_string()],
            )
            .map_err(|e| format!("写入 meta {key} 失败: {e}"))?;
        Ok(())
    }

    /// 清理旧事件（默认保留由 `trim_hist_hours` 决定，缺省永久保留）。
    pub fn prune_events(&mut self, before_ms: i64) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM events WHERE at_ms < ?1", params![before_ms])
            .map_err(|e| format!("清理旧事件失败: {e}"))?;
        Ok(())
    }
}

const SCHEMA: &str = "\
PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
CREATE TABLE IF NOT EXISTS endpoints (
    ip              TEXT PRIMARY KEY,
    json            TEXT NOT NULL,
    last_emitted_ms INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS devices (
    id              TEXT PRIMARY KEY,
    json            TEXT NOT NULL,
    last_emitted_ms INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS events (
    seq   INTEGER PRIMARY KEY,
    at_ms INTEGER NOT NULL,
    json  TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS sources (
    source      TEXT PRIMARY KEY,
    last_run_ms INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
";

/// 表是否存在。
fn table_exists(conn: &Connection, table: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![table],
        |_| Ok(()),
    )
    .optional()
    .map(|o| o.is_some())
    .map_err(|e| format!("查询表 {table} 失败: {e}"))
}

/// 列是否存在（用于识别旧版 schema）。
fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| format!("读取表 {table} 结构失败: {e}"))?;
    let mut rows = stmt
        .query([])
        .map_err(|e| format!("读取表 {table} 结构失败: {e}"))?;
    while let Some(row) = rows
        .next()
        .map_err(|e| format!("读取表 {table} 结构失败: {e}"))?
    {
        let name: String = row.get(1).map_err(|e| e.to_string())?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::model::{DiscoverySource, Observation};
    use chrono::Utc;

    fn store() -> (Store, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("inv.db")).unwrap();
        (s, dir)
    }

    #[test]
    fn an_empty_database_loads_an_empty_inventory() {
        let (s, _d) = store();
        let loaded = s.load().unwrap();
        assert_eq!(loaded.inventory.endpoints().count(), 0);
        assert_eq!(loaded.inventory.event_seq(), 1);
    }

    #[test]
    fn saving_and_reloading_keeps_endpoints_devices_and_events() {
        let (mut s, _d) = store();
        let mut inv = Inventory::new();
        let obs = Observation::from_mac(
            "d0:57:7e:35:f5:2f".parse().unwrap(),
            "192.168.3.96".parse().unwrap(),
            Some("eth0".into()),
        )
        .with_source(DiscoverySource::NeighborTable);
        inv.observe(&obs, Utc::now());
        let events = inv.take_events();
        assert!(!events.is_empty());

        let ep: HashMap<String, i64> = HashMap::new();
        let dev: HashMap<String, i64> = HashMap::new();
        s.save(&inv, &ep, &dev, &events).unwrap();

        let loaded = s.load().unwrap();
        assert_eq!(loaded.inventory.endpoints().count(), 1);
        assert!(loaded.inventory.devices().count() >= 1);
        assert!(loaded.inventory.event_seq() > 1);
    }

    #[test]
    fn meta_and_source_runs_roundtrip() {
        let (mut s, _d) = store();
        assert_eq!(s.meta_i64("last_cycle_ms").unwrap(), None);
        s.set_meta_i64("last_cycle_ms", 1234).unwrap();
        assert_eq!(s.meta_i64("last_cycle_ms").unwrap(), Some(1234));
        s.set_source_run("mdns", 999).unwrap();
        let loaded = s.load().unwrap();
        assert_eq!(loaded.source_runs["mdns"], 999);
    }
}
