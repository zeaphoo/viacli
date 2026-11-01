//! 协议消息发送端（见 `docs/07-collector-protocol.md` §2）。
//!
//! 与 `crates/system/src/protocol.rs` 同源，但只保留本采集器用得到的子集：
//! `meta` / `series` / `sample` / `record` / `state` / `log` / `error`。
//! 两条硬约束不变：
//! 1. **stdout 只放协议消息**（日志一律走 stderr）；
//! 2. **每条消息都立即 flush**（§3.3）——不 flush 会让宿主读不到任何输出，
//!    表现为"采集器卡住直到超时"，而采集器自己看起来一切正常。

use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::{self, Write};

/// 序列名长度上限（字节），与标准 §2.4 一致。
pub const MAX_NAME_LEN: usize = 255;

/// 名字是否合法（长度 + 字符集 + 首段首字符）。规则见协议 §2.4。
pub fn name_is_legal(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return false;
    }
    let mut segments = name.split('/');
    let first = segments.next().unwrap_or("");
    if !first.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return false;
    }
    for seg in std::iter::once(first).chain(segments) {
        if seg.is_empty() || seg.starts_with(['-', '.']) {
            return false;
        }
        if !seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        {
            return false;
        }
    }
    true
}

/// `series.kind`（§2.6）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Gauge,
    #[allow(dead_code)]
    Counter,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Gauge => "gauge",
            Kind::Counter => "counter",
        }
    }
}

/// 推荐单位（§2.7）。本采集器只用到 `count`，其余留给协议词汇表。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Bytes,
    BytesPerSecond,
    Percent,
    Celsius,
    Seconds,
    Hertz,
    Watts,
    Count,
}

impl Unit {
    fn as_str(self) -> &'static str {
        match self {
            Unit::Bytes => "bytes",
            Unit::BytesPerSecond => "bytes_per_second",
            Unit::Percent => "percent",
            Unit::Celsius => "celsius",
            Unit::Seconds => "seconds",
            Unit::Hertz => "hertz",
            Unit::Watts => "watts",
            Unit::Count => "count",
        }
    }
}

/// 数据的保留声明（§2.8）。记录类型在 `--describe` 里声明，序列随 `series` 消息声明。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retention {
    /// 不发表意见，由宿主的策略决定（字段缺省）。
    #[allow(dead_code)]
    Unspecified,
    /// `persist: false`：只为实时展示，不要落库。
    #[allow(dead_code)]
    LiveOnly,
    /// `maxRetentionMs: N`：超过 N 毫秒就没有保留价值。
    Max(u64),
    /// `maxRetentionMs: null`：永久保留（不可再生的数据）。
    Forever,
}

impl Retention {
    pub fn fields(self) -> Vec<(&'static str, Value)> {
        match self {
            Retention::Unspecified => Vec::new(),
            Retention::LiveOnly => vec![("persist", Value::Bool(false))],
            Retention::Max(ms) => vec![("maxRetentionMs", json!(ms))],
            Retention::Forever => vec![("maxRetentionMs", Value::Null)],
        }
    }
}

/// 错误类别（§6.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Config,
    Network,
    Upstream,
    Parse,
    Internal,
}

impl ErrorKind {
    fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Config => "config",
            ErrorKind::Network => "network",
            ErrorKind::Upstream => "upstream",
            ErrorKind::Parse => "parse",
            ErrorKind::Internal => "internal",
        }
    }

    /// 退出码（§3.4）
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorKind::Config => 2,
            ErrorKind::Network | ErrorKind::Upstream => 4,
            ErrorKind::Internal | ErrorKind::Parse => 5,
        }
    }
}

/// 消息发送端。泛型是为了让测试可以写入内存缓冲区。
pub struct Sink<W: Write> {
    out: W,
    declared: HashSet<String>,
    emitted: u64,
    bytes: u64,
}

impl<W: Write> Sink<W> {
    pub fn new(out: W) -> Self {
        Self {
            out,
            declared: HashSet::new(),
            emitted: 0,
            bytes: 0,
        }
    }

    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// 取出底层写入端（本 crate 里只有测试会用到）。
    #[cfg(test)]
    pub fn into_inner(self) -> W {
        self.out
    }

    fn emit(&mut self, msg: Value) -> io::Result<()> {
        let line = serde_json::to_string(&msg)?;
        writeln!(self.out, "{line}")?;
        // §3.3：必须及时 flush，不能等进程退出
        self.out.flush()?;
        self.emitted += 1;
        self.bytes += line.len() as u64 + 1;
        Ok(())
    }

    pub fn meta(&mut self, collector: &str, version: &str) -> io::Result<()> {
        self.emit(json!({"type": "meta", "collector": collector, "version": version}))
    }

    /// 声明序列元数据；同名只声明一次（§2.3）。`labels` 只能放不随时间变化的值。
    pub fn declare(
        &mut self,
        name: &str,
        kind: Kind,
        unit: Unit,
        help: &str,
        retention: Retention,
    ) -> io::Result<()> {
        if !name_is_legal(name) {
            eprintln!("viacli-rss: 丢弃非法序列名 {name:?}");
            return Ok(());
        }
        if !self.declared.insert(name.to_string()) {
            return Ok(());
        }
        let mut msg = json!({
            "type": "series",
            "name": name,
            "kind": kind.as_str(),
            "unit": unit.as_str(),
            "help": help,
        });
        for (k, v) in retention.fields() {
            msg[k] = v;
        }
        self.emit(msg)
    }

    pub fn sample_i64(&mut self, name: &str, ts: Option<i64>, value: i64) -> io::Result<()> {
        if !name_is_legal(name) {
            return Ok(());
        }
        let mut msg = json!({"type": "sample", "name": name, "value": value});
        if let Some(ts) = ts {
            msg["ts"] = json!(ts);
        }
        self.emit(msg)
    }

    pub fn record(
        &mut self,
        kind: &str,
        key: Option<&str>,
        ts: Option<i64>,
        payload: Value,
    ) -> io::Result<()> {
        let mut msg = json!({"type": "record", "kind": kind, "payload": payload});
        if let Some(k) = key {
            msg["key"] = json!(k);
        }
        if let Some(ts) = ts {
            msg["ts"] = json!(ts);
        }
        self.emit(msg)
    }

    pub fn log(&mut self, level: &str, message: &str) -> io::Result<()> {
        self.emit(json!({"type": "log", "level": level, "message": message}))
    }

    pub fn error(&mut self, kind: ErrorKind, retryable: bool, message: &str) -> io::Result<()> {
        self.emit(json!({
            "type": "error",
            "kind": kind.as_str(),
            "retryable": retryable,
            "message": message,
        }))
    }

    /// 提交本次运行的状态；宿主仅在退出码为 0 时保存（§5）。
    pub fn state(&mut self, state: Value) -> io::Result<()> {
        self.emit(json!({"type": "state", "state": state}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sink() -> Sink<Vec<u8>> {
        Sink::new(Vec::new())
    }

    fn lines(s: &Sink<Vec<u8>>) -> Vec<Value> {
        String::from_utf8(s.out.clone())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).expect("每行必须是合法 JSON"))
            .collect()
    }

    #[test]
    fn series_is_declared_only_once() {
        let mut s = sink();
        s.declare(
            "rss/items_emitted",
            Kind::Gauge,
            Unit::Count,
            "x",
            Retention::Max(1),
        )
        .unwrap();
        s.declare(
            "rss/items_emitted",
            Kind::Gauge,
            Unit::Count,
            "x",
            Retention::Max(1),
        )
        .unwrap();
        s.sample_i64("rss/items_emitted", Some(1), 2).unwrap();
        let msgs = lines(&s);
        assert_eq!(msgs.iter().filter(|m| m["type"] == "series").count(), 1);
    }

    #[test]
    fn every_line_is_valid_json_with_known_type() {
        let mut s = sink();
        s.meta("rss", "0.1.0").unwrap();
        s.declare("rss/x", Kind::Gauge, Unit::Count, "x", Retention::Forever)
            .unwrap();
        s.sample_i64("rss/x", Some(1), 42).unwrap();
        s.record("rss.item", Some("k"), Some(1), json!({"a": 1})).unwrap();
        s.log("info", "ok").unwrap();
        s.error(ErrorKind::Upstream, true, "nope").unwrap();
        s.state(json!({"a": 1})).unwrap();
        for m in lines(&s) {
            let t = m["type"].as_str().expect("type 必填");
            assert!(
                ["meta", "series", "sample", "record", "state", "log", "error"].contains(&t),
                "未知 type: {t}"
            );
        }
    }

    #[test]
    fn name_rules_match_the_spec() {
        for ok in ["rss/feeds_ok", "rss/items_emitted"] {
            assert!(name_is_legal(ok), "{ok} 应合法");
        }
        for bad in ["", "rss/温度", "1rss/x", "rss//x"] {
            assert!(!name_is_legal(bad), "{bad:?} 应非法");
        }
    }

    #[test]
    fn exit_codes_follow_the_spec() {
        assert_eq!(ErrorKind::Config.exit_code(), 2);
        assert_eq!(ErrorKind::Upstream.exit_code(), 4);
        assert_eq!(ErrorKind::Internal.exit_code(), 5);
    }
}
