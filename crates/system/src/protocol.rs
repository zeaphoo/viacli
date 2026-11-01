//! 协议消息发送端（见 `docs/07-collector-protocol.md` §2）。
//!
//! 两条硬约束体现在这个模块里：
//! 1. **stdout 只放协议消息**（日志一律走 stderr，由 `log()` 系列之外的地方保证）；
//! 2. **每条消息都立即 flush**（§3.3）——不 flush 会让宿主读不到任何输出、
//!    表现为"采集器卡住直到超时"，而采集器自己看起来一切正常。

use serde_json::{Number, Value, json};
use std::collections::HashSet;
use std::io::{self, Write};

/// 序列名长度上限（字节），与标准 §2.4 一致。
pub const MAX_NAME_LEN: usize = 255;

/// 名字是否合法（长度 + 字符集 + 首段首字符）。
///
/// 为什么采集器要自己判一遍：名字是**从平台数据拼出来的**（挂载点、网卡名、传感器标签），
/// 用户机器上出现超长挂载点（深层 bind mount）或怪字符是常态。
/// 不判的话，问题会在宿主落库那一刻才以一句
/// `CHECK constraint failed` 爆出来——那时已经分不清是哪条数据的问题了。
pub fn name_is_legal(name: &str) -> bool {
    // 长度按**字节**算：标准写的是 ≤255 字节，而字符集限定为 ASCII，两者等价
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        return false;
    }
    let mut segments = name.split('/');
    let first = segments.next().unwrap_or("");
    // 首段的第一个字符必须是字母（写"首段"而不是"整名"，因为 `system/cpu_usage/core/0`
    // 的最后一段就是纯数字，那是合法的）
    if !first.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return false;
    }
    for seg in std::iter::once(first).chain(segments) {
        // `//`、开头的 `/`、结尾的 `/` 都会产生空段，而语法要求 segment 至少 1 个字符
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

/// 推荐单位（§2.7）。
///
/// 这是协议的**词汇表**：本采集器只用到其中一部分（`watts` 仅在 `nvidia` 特性下用到），
/// 其余留给别的采集器，因此允许未构造。
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

/// 数据的保留声明（协议 §2.8）。
///
/// 这是**宿主无法自己推断**的信息：同样是 3 秒一个点，
/// "CPU 使用率"放一年是垃圾，而"抓回来的文章"丢一天就是永久损失。
/// 只有采集器知道自己的数据属于哪种，所以必须由采集器声明。
///
/// 本采集器只用到其中一部分（没有"仅实时展示"的序列），其余留给别的采集器，
/// 因此允许未构造——它是协议词汇表，不是本采集器的可选值集合。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retention {
    /// 不发表意见，由宿主的策略决定（字段缺省）。
    Unspecified,
    /// `persist: false`：只为实时展示，不要落库。
    LiveOnly,
    /// `maxRetentionMs: N`：超过 N 毫秒就没有保留价值。
    Max(u64),
    /// `maxRetentionMs: null`：永久保留（不可再生的数据）。
    Forever,
}

impl Retention {
    /// 转成协议里的字段（§2.8）。`--describe` 的 `recordKinds` 与 `series` 消息共用同一套表达，
    /// 因此只在这里写一次，避免两边漂移。
    pub fn fields(self) -> Vec<(&'static str, Value)> {
        match self {
            Retention::Unspecified => Vec::new(),
            Retention::LiveOnly => vec![("persist", Value::Bool(false))],
            Retention::Max(ms) => vec![("maxRetentionMs", json!(ms))],
            Retention::Forever => vec![("maxRetentionMs", Value::Null)],
        }
    }

}

/// 错误类别（§6.1）。同样是协议词汇表，本采集器只用到 config/upstream/internal。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Config,
    Auth,
    Network,
    Upstream,
    Parse,
    Internal,
}

impl ErrorKind {
    fn as_str(self) -> &'static str {
        match self {
            ErrorKind::Config => "config",
            ErrorKind::Auth => "auth",
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
            ErrorKind::Auth => 3,
            ErrorKind::Network | ErrorKind::Upstream => 4,
            ErrorKind::Internal | ErrorKind::Parse => 5,
        }
    }
}

/// 消息发送端。泛型是为了让测试可以写入内存缓冲区。
pub struct Sink<W: Write> {
    out: W,
    declared: HashSet<String>,
    /// 名字非法而被丢弃的序列：只警告一次，避免每秒刷屏
    rejected: HashSet<String>,
    emitted: u64,
    bytes: u64,
}

impl<W: Write> Sink<W> {
    pub fn new(out: W) -> Self {
        Self {
            out,
            declared: HashSet::new(),
            rejected: HashSet::new(),
            emitted: 0,
            bytes: 0,
        }
    }

    /// 已写出的消息行数。用于自行遵守请求里的 `limits.maxLines`（§4.5）。
    pub fn emitted(&self) -> u64 {
        self.emitted
    }

    /// 已写出的字节数（含换行）。用于自行遵守 `limits.maxOutputBytes`（§4.5）。
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// 取出底层写入端（本 crate 里只有测试会用到）。
    #[cfg(test)]
    pub fn into_inner(self) -> W {
        self.out
    }

    /// 借用底层写入端，用于检查已写出的内容。
    #[cfg(test)]
    pub fn raw(&self) -> &W {
        &self.out
    }

    /// 名字检查：非法就整条序列丢掉（§11.3 降级），并且只警告一次。
    ///
    /// 宁可少一个序列，也不能让整轮采集因为一个畸形挂载点而失败——
    /// 那会连带丢掉本来正常的所有数据。
    fn accept(&mut self, name: &str) -> bool {
        if name_is_legal(name) {
            return true;
        }
        if self.rejected.insert(name.to_string()) {
            eprintln!(
                "viacli-sys: 丢弃非法序列名 {name:?}（长度上限 {MAX_NAME_LEN} 字节，                 只允许 A-Za-z0-9_.-/ 且首段首字符为字母）"
            );
        }
        false
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

    /// 声明序列元数据；同名只声明一次（§2.3）。
    ///
    /// `labels` 只声明一次，因此**只能放不随时间变化的值**；把 PID 之类易变的量放进标签
    /// 会让宿主把同一个序列名下的数据混在一起。
    ///
    /// `retention` 见 §2.8——它是宿主**无法自己推断**的信息，所以必须在这里声明。
    #[allow(clippy::too_many_arguments)]
    pub fn declare(
        &mut self,
        name: &str,
        kind: Kind,
        unit: Unit,
        help: &str,
        labels: Value,
        retention: Retention,
    ) -> io::Result<()> {
        if !self.accept(name) {
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
        if !labels.is_null() {
            msg["labels"] = labels;
        }
        if let Retention::Max(ms) = retention {
            // §2.8：`maxRetentionMs: 0` 会被 JS 当成 falsy，表达"不落库"必须用 persist:false
            debug_assert!(ms > 0, "maxRetentionMs 必须是正整数；'不落库'请用 LiveOnly");
        }
        for (k, v) in retention.fields() {
            msg[k] = v;
        }
        self.emit(msg)
    }

    pub fn sample_f64(&mut self, name: &str, ts: Option<i64>, value: f64) -> io::Result<()> {
        if !self.accept(name) {
            return Ok(());
        }
        // JSON 无法表达 NaN/Infinity（§2.6）→ 跳过并说明，绝不写非法 JSON
        let Some(num) = Number::from_f64(value) else {
            eprintln!("viacli-sys: 跳过非有限值 {name}={value}");
            return Ok(());
        };
        let mut msg = json!({"type": "sample", "name": name, "value": num});
        if let Some(ts) = ts {
            msg["ts"] = json!(ts);
        }
        self.emit(msg)
    }

    pub fn sample_i64(&mut self, name: &str, ts: Option<i64>, value: i64) -> io::Result<()> {
        if !self.accept(name) {
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
    ///
    /// `system` 采集器不需要跨运行记忆（它按轮次连续采集），但协议要求这个能力存在——
    /// 需要增量游标的采集器（HTTP 分页、日志 tail）才用它。
    #[allow(dead_code)]
    pub fn state(&mut self, state: Value) -> io::Result<()> {
        self.emit(json!({"type": "state", "state": state}))
    }
}

/// 把平台给的原始名字（设备名、接口名、传感器标签）规范成合法序列段（§2.4）。
///
/// 平台名字里可能有空格、`:`、`/`、`{}`、中文等，**直接塞进序列名就是非法协议输出**。
/// 规则：允许 `A-Za-z0-9_.-`，其余字符替换为 `-`；去掉开头的 `-`/`.`；空则回退为 `unknown`。
pub fn sanitize_segment(raw: &str) -> String {
    let mut out: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    while out.starts_with('-') || out.starts_with('.') {
        out.remove(0);
    }
    if out.is_empty() {
        out.push_str("unknown");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sink() -> Sink<Vec<u8>> {
        Sink::new(Vec::new())
    }

    fn lines(s: &Sink<Vec<u8>>) -> Vec<Value> {
        String::from_utf8(s.raw().clone())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).expect("每行必须是合法 JSON"))
            .collect()
    }

    #[test]
    fn series_is_declared_only_once() {
        let mut s = sink();
        s.declare("system/cpu_usage", Kind::Gauge, Unit::Percent, "总体使用率", Value::Null, Retention::Unspecified)
            .unwrap();
        s.declare("system/cpu_usage", Kind::Gauge, Unit::Percent, "总体使用率", Value::Null, Retention::Unspecified)
            .unwrap();
        s.sample_f64("system/cpu_usage", None, 12.5).unwrap();
        let msgs = lines(&s);
        let series: Vec<_> = msgs.iter().filter(|m| m["type"] == "series").collect();
        assert_eq!(series.len(), 1, "同名序列只应声明一次");
        assert_eq!(series[0]["unit"], "percent");
        assert_eq!(series[0]["kind"], "gauge");
    }

    #[test]
    fn every_line_is_valid_json_with_known_type() {
        // 协议一致性检查的第 3 项（最关键的一项）
        let mut s = sink();
        s.meta("system", "0.1.0").unwrap();
        s.declare("system/x", Kind::Counter, Unit::Bytes, "x", Value::Null, Retention::Unspecified)
            .unwrap();
        s.sample_i64("system/x", Some(1), 42).unwrap();
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
    fn integers_stay_integers_and_non_finite_values_are_skipped() {
        let mut s = sink();
        s.sample_i64("system/big", None, 9_007_199_254_740_993).unwrap();
        s.sample_f64("system/nan", None, f64::NAN).unwrap();
        s.sample_f64("system/inf", None, f64::INFINITY).unwrap();
        s.sample_f64("system/f", None, 1.5).unwrap();
        let msgs = lines(&s);
        assert_eq!(msgs.len(), 2, "NaN/Infinity 必须被跳过，不能写出非法 JSON");
        assert_eq!(msgs[0]["value"].as_i64(), Some(9_007_199_254_740_993));
        assert_eq!(msgs[1]["value"].as_f64(), Some(1.5));
    }

    #[test]
    fn sanitize_keeps_platform_names_legal() {
        assert_eq!(sanitize_segment("eth0"), "eth0");
        assert_eq!(sanitize_segment("enp0s3"), "enp0s3");
        assert_eq!(sanitize_segment("nvme0n1p2"), "nvme0n1p2");
        assert_eq!(sanitize_segment("web-01.prod"), "web-01.prod");
        assert_eq!(sanitize_segment("core temp"), "core-temp");
        assert_eq!(sanitize_segment("CPU Core #1"), "CPU-Core--1");
        assert_eq!(sanitize_segment("-dev-sda"), "dev-sda");
        assert_eq!(sanitize_segment(".hidden"), "hidden");
        assert_eq!(sanitize_segment(""), "unknown");
        // 非 ASCII 字符全部变成 `-`，再被开头的裁剪吃掉 → 回退为 unknown。
        // 这是刻意的：名字里出现中文说明采集器作者忘了用 help 字段，而不是要在这里做音译。
        assert_eq!(sanitize_segment("中文标签"), "unknown");
    }

    #[test]
    fn illegal_series_names_are_dropped_not_emitted() {
        // 现实里挂载点可以很深：拼出来的序列名会超过 255 字节。
        // 这类序列必须**整条丢掉**（连带它的 sample），而不是产出非法数据让宿主落库失败。
        let too_long = format!("system/disk/{}/used_bytes", "x".repeat(300));
        assert!(!name_is_legal(&too_long));

        let mut s = sink();
        s.declare(&too_long, Kind::Gauge, Unit::Bytes, "太长", Value::Null, Retention::Unspecified)
            .unwrap();
        s.sample_i64(&too_long, None, 1).unwrap();
        // 正常序列不受影响
        s.declare("system/cpu_usage", Kind::Gauge, Unit::Percent, "ok", Value::Null, Retention::Unspecified)
            .unwrap();
        s.sample_f64("system/cpu_usage", None, 1.0).unwrap();

        let msgs = lines(&s);
        assert_eq!(msgs.len(), 2, "非法名字的 series/sample 都不该出现");
        assert_eq!(msgs[0]["name"], "system/cpu_usage");
    }

    #[test]
    fn name_rules_match_the_spec() {
        for ok in [
            "system/cpu_usage",
            "system/cpu_usage/core/0",
            "system/net/web-01.prod/rx_rate",
            "gpu/nvidia/0/temperature_celsius",
            "system/temperature/nvme-Sensor-1-Fanxiang-S790MAX-4TB",
            "system/disk/var-lib-docker/used_bytes",
        ] {
            assert!(name_is_legal(ok), "{ok} 应合法");
        }
        for bad in [
            "",
            "system/cpu usage",     // 空格
            "system/温度",           // 非 ASCII
            "system//cpu",          // 空段
            "system/-bad",          // 段首是 '-'
            "system/.bad",          // 段首是 '.'
            "1system/cpu",          // 首段以数字开头
            "system/cpu#x",         // 集合外字符
        ] {
            assert!(!name_is_legal(bad), "{bad:?} 应非法");
        }
        assert!(name_is_legal(&format!("a/{}", "b".repeat(MAX_NAME_LEN - 2))));
        assert!(!name_is_legal(&format!("a/{}", "b".repeat(MAX_NAME_LEN - 1))));
    }

    #[test]
    fn exit_codes_follow_the_spec() {
        assert_eq!(ErrorKind::Config.exit_code(), 2);
        assert_eq!(ErrorKind::Auth.exit_code(), 3);
        assert_eq!(ErrorKind::Upstream.exit_code(), 4);
        assert_eq!(ErrorKind::Internal.exit_code(), 5);
    }
}
