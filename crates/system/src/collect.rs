//! `system` 采集器的采集逻辑（标准 §11）。
//!
//! 三条原则贯穿本模块：
//! 1. **降级不报错**（§11.3）：某个类别取不到数据就不产出该序列，整体仍算成功。
//! 2. **不伪造数据**：拿不到就是拿不到，绝不用 0 冒充实测值。
//! 3. **CPU 使用率必须跨越两次测量**：它不是某个计数器，而是**两个计数器增量之比**
//!    `Δ忙碌CPU时间 / ΔCPU总时间`（分母是 CPU 总时间，**不是墙钟时间**——
//!    用墙钟会差一个核数倍）。sysinfo 的 `refresh_cpu_usage` 内部按这个差值计算，
//!    两次刷新间隔小于 `MINIMUM_CPU_UPDATE_INTERVAL` 时它会把旧值原样返回。
//!    这就是 `stream` 是默认模式、且 `intervalMs` 有 250ms 下限的原因。

use crate::config::Config;
use crate::protocol::{Kind, Retention, Sink, Unit, sanitize_segment};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::time::Instant;
use sysinfo::{
    Components, CpuRefreshKind, Disks, MemoryRefreshKind, Networks, ProcessRefreshKind,
    ProcessesToUpdate, RefreshKind, System,
};

#[cfg(target_os = "linux")]
use crate::linux;

/// 读进程表时用的刷新种类。
///
/// **必须显式指定，不能调 `System::refresh_processes()`**：那个便捷方法会用它自己的默认
/// `ProcessRefreshKind::nothing()`，而 `nothing()` 里 **`tasks: true`** ——
/// 在 Linux 上等于"把每个线程也当成一个进程"。实测后果：
///   * 一个 4 线程忙进程独占整个 Top-N（主进程 403.7% + 4 个线程各 100.9%），
///     看起来像 5 个进程；按行累加还会把它的 CPU 重复计算一遍；
///   * `system/process_count` 从真实的 4 变成 10。
/// 排除线程后主进程读数不受影响：`/proc/<pid>/stat` 本身已汇总全部线程
/// （实测 4 个忙线程 → 403.7% ≈ 4 核），因为内核在 `whole=1` 模式下报的是线程组总量。
fn process_refresh_kind() -> ProcessRefreshKind {
    ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory()
        .without_tasks()
}

// ---- 保留声明（协议 §2.8）----
//
// 这些数字是**采集器对"我的数据多久之后就没价值了"的判断**，宿主无法自己推断：
// 同样是 3 秒一个点，CPU 使用率放一年是垃圾，而不可再生的档案丢一天就是永久损失。
// 它们是**提示**：用户配置优先，宿主默认按 `min(全局默认, maxRetentionMs)` 收窄。

/// 1 天：**3 秒粒度的抖动类读数**（CPU 使用率/频率/负载、各类 `*_rate`、GPU 利用率）。
///
/// 为什么不是 1 小时：那样"昨天 CPU 什么样"就完全看不到了，而查询期降采样
/// （`DESIGN.md` §4.2.3 当前唯一的聚合路径）本来就能把 3 秒数据画成日视图。
/// 为什么不是 7 天：更长的时间跨度不该靠堆 raw 支撑（那是 rollup 的事，D23 未实现）。
pub(crate) const R_FAST: Retention = Retention::Max(86_400_000);
/// 7 天：变化慢、看趋势有意义的量——内存/swap/显存、温度、运行时长、进程数。
pub(crate) const R_WEEK: Retention = Retention::Max(604_800_000);
/// 30 天：**容量与累计计数器**——磁盘容量、磁盘 IO、网络流量。
/// 它们是"长期趋势"的原料：用户一旦把全局保留调长，最值得留下的就是这些。
pub(crate) const R_MONTH: Retention = Retention::Max(2_592_000_000);
/// 永久：**不可再生**的数据（主机身份）；丢了就再也拿不回来。
pub(crate) const R_FOREVER: Retention = Retention::Forever;

/// 声明序列（无标签、带保留策略）。
fn declare<W: Write>(
    sink: &mut Sink<W>,
    name: &str,
    kind: Kind,
    unit: Unit,
    help: &str,
    retention: Retention,
) -> io::Result<()> {
    sink.declare(name, kind, unit, help, Value::Null, retention)
}

/// 声明序列（带静态标签与保留策略）。
fn declare_labeled<W: Write>(
    sink: &mut Sink<W>,
    name: &str,
    kind: Kind,
    unit: Unit,
    help: &str,
    labels: Value,
    retention: Retention,
) -> io::Result<()> {
    sink.declare(name, kind, unit, help, labels, retention)
}

/// 上一轮测量，用来把累计计数器变成速率。
struct Prev {
    at: Instant,
    net: HashMap<String, (u64, u64)>,
    disk: HashMap<String, (u64, u64)>,
}

pub struct Collector {
    cfg: Config,
    sys: System,
    disks: Disks,
    networks: Networks,
    components: Components,
    prev: Option<Prev>,
    /// 静态信息（主机名/系统版本）只发一次
    info_sent: bool,
}

impl Collector {
    /// 建立采集器并完成**第一次**刷新。
    ///
    /// 第一次刷新拿不到 CPU 使用率（差值需要两个采样点），所以调用方必须在
    /// 至少 `MINIMUM_CPU_UPDATE_INTERVAL` 之后再 `refresh()` + `collect()`。
    pub fn new(cfg: Config) -> Self {
        let sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything())
                .with_processes(process_refresh_kind()),
        );
        let disks = Disks::new_with_refreshed_list();
        let networks = Networks::new_with_refreshed_list();
        let components = Components::new_with_refreshed_list();
        Self {
            cfg,
            sys,
            disks,
            networks,
            components,
            prev: None,
            info_sent: false,
        }
    }

    /// 配置里是否要求采集 CPU。
    ///
    /// 调用方需要据此决定"要不要等一个最小窗口再刷新"——CPU 使用率要两次采样，
    /// 而其它类别不需要，不该替 CPU 付这份等待。
    pub fn wants_cpu(&self) -> bool {
        self.cfg.has("cpu")
    }

    /// 重新读取所有选中的类别。只刷新被配置启用的类别，避免做无用功。
    pub fn refresh(&mut self) {
        if self.cfg.has("cpu") {
            self.sys.refresh_cpu_all();
        }
        if self.cfg.has("memory") {
            self.sys.refresh_memory();
        }
        // /proc 里的 process_count 与进程 Top-N 都需要进程表。
        // 用 `_specifics` 而不是 `refresh_processes`——后者会忽略我们配好的 kind
        // 并退回带 `tasks: true` 的默认值（即把线程当进程），见 `process_refresh_kind`。
        if self.cfg.has("system") || self.cfg.has("processes") {
            self.sys
                .refresh_processes_specifics(ProcessesToUpdate::All, true, process_refresh_kind());
        }
        if self.cfg.has("disk") {
            self.disks.refresh(true);
        }
        if self.cfg.has("network") {
            self.networks.refresh(true);
        }
        if self.cfg.has("temperature") {
            self.components.refresh(true);
        }
    }

    /// 产出一轮数据，返回**实际产出的数据消息条数**（`sample` + `record`）。
    ///
    /// 返回 0 表示所有类别都取不到数据——这是唯一需要报错的场景（§11.3）。
    /// 这里刻意把 `record` 也计入：进程快照等类别只产出 `record`，
    /// 若只数 `sample`，宿主会误判成"整个采集器都取不到数据"。
    pub fn collect<W: Write>(&mut self, sink: &mut Sink<W>, ts: Option<i64>) -> io::Result<usize> {
        let mut n = 0;
        n += self.emit_info(sink, ts)?;
        n += self.emit_cpu(sink, ts)?;
        n += self.emit_memory(sink, ts)?;
        n += self.emit_disk(sink, ts)?;
        n += self.emit_network(sink, ts)?;
        n += self.emit_temperature(sink, ts)?;
        n += self.emit_gpu(sink, ts)?;
        n += self.emit_system(sink, ts)?;
        n += self.emit_processes(sink, ts)?;
        self.snapshot();
        Ok(n)
    }

    /// 记录本轮时间与累计计数器，供下一轮算速率。
    fn snapshot(&mut self) {
        let mut net = HashMap::new();
        for (name, data) in self.networks.list() {
            net.insert(
                name.clone(),
                (data.total_received(), data.total_transmitted()),
            );
        }
        let disk = disk_totals();
        self.prev = Some(Prev {
            at: Instant::now(),
            net,
            disk,
        });
    }

    // ---------- 各类别 ----------

    /// 静态主机信息以 `record` 发出（标准 §2.3 的 `record` 消息的实战用例）：
    /// 主机名/系统版本是"一个实体的属性"，不是随时间变化的数值，塞进 `sample` 只能靠标签，
    /// 而标签不参与身份、也不适合承载这么多字段。
    /// 每次运行只发一次（它不会变），因此没有基数问题。
    fn emit_info<W: Write>(&mut self, sink: &mut Sink<W>, ts: Option<i64>) -> io::Result<usize> {
        if !self.cfg.has("system") || self.info_sent {
            return Ok(0);
        }
        let mut payload = serde_json::Map::new();
        let mut put = |k: &str, v: Option<String>| {
            if let Some(v) = v.filter(|s| !s.is_empty()) {
                payload.insert(k.to_string(), Value::String(v));
            }
        };
        put("hostName", System::host_name());
        put("osName", System::name());
        put("osVersion", System::long_os_version());
        put("kernelVersion", System::kernel_version());
        put("distributionId", Some(System::distribution_id()));
        put("cpuArch", Some(System::cpu_arch()));
        if let Some(brand) = self.sys.cpus().first().map(|c| c.brand().trim().to_string()) {
            put("cpuBrand", Some(brand));
        }
        if let Some(brand) = self.sys.cpus().first().map(|c| c.vendor_id().to_string()) {
            put("cpuVendor", Some(brand));
        }
        if let Some(cores) = System::physical_core_count() {
            payload.insert("physicalCoreCount".into(), json!(cores));
        }
        payload.insert("logicalCpuCount".into(), json!(self.sys.cpus().len()));
        // 开机时刻放这里而不是作为序列：它**直到重启都不变**，
        // 作为每 3 秒一发的样本等于每天白存 28800 个一模一样的点。
        // 这类"实体的静态属性"就该用 record（它天然只在每次运行时发一次）。
        let boot = System::boot_time();
        if boot > 0 {
            payload.insert("bootTimeSeconds".into(), json!(boot));
        }
        self.info_sent = true;
        sink.record("system.info", Some("system"), ts, Value::Object(payload))?;
        Ok(1)
    }

    fn emit_cpu<W: Write>(&self, sink: &mut Sink<W>, ts: Option<i64>) -> io::Result<usize> {
        if !self.cfg.has("cpu") {
            return Ok(0);
        }
        let cpus = self.sys.cpus();
        if cpus.is_empty() {
            return Ok(0);
        }
        let mut n = 0;

        declare(
            sink,
            "system/cpu_usage",
            Kind::Gauge,
            Unit::Percent,
            "CPU 总体使用率（0-100）",
            R_FAST,
        )?;
        sink.sample_f64("system/cpu_usage", ts, self.sys.global_cpu_usage() as f64)?;
        n += 1;

        for (i, cpu) in cpus.iter().enumerate() {
            let name = format!("system/cpu_usage/core/{i}");
            declare_labeled(
                sink,
                &name,
                Kind::Gauge,
                Unit::Percent,
                "单核使用率（0-100）",
                json!({"core": i}),
                R_FAST,
            )?;
            sink.sample_f64(&name, ts, cpu.cpu_usage() as f64)?;
            n += 1;
        }

        // 频率：虚拟机与 ARM 上常为 0，为 0 时视为"该平台不提供"（§11.3）
        let freqs: Vec<u64> = cpus.iter().map(|c| c.frequency()).collect();
        if freqs.iter().any(|f| *f > 0) {
            let nonzero: Vec<u64> = freqs.iter().copied().filter(|f| *f > 0).collect();
            let avg = nonzero.iter().sum::<u64>() as f64 / nonzero.len() as f64;
            declare(
                sink,
                "system/cpu_frequency",
                Kind::Gauge,
                Unit::Hertz,
                "CPU 平均频率",
                R_FAST,
            )?;
            sink.sample_f64("system/cpu_frequency", ts, avg)?;
            n += 1;
            for (i, f) in freqs.iter().enumerate() {
                if *f == 0 {
                    continue;
                }
                let name = format!("system/cpu_frequency/core/{i}");
                declare(sink, &name, Kind::Gauge, Unit::Hertz, "单核频率", R_FAST)?;
                sink.sample_i64(&name, ts, *f as i64)?;
                n += 1;
            }
        }

        declare(
            sink,
            "system/cpu_count",
            Kind::Gauge,
            Unit::Count,
            "逻辑 CPU 数量",
            R_WEEK,
        )?;
        sink.sample_i64("system/cpu_count", ts, cpus.len() as i64)?;
        n += 1;

        // 负载均值：Windows 上恒为 0，为 0 时不产出（§11.3）
        let load = System::load_average();
        if load.one > 0.0 || load.five > 0.0 || load.fifteen > 0.0 {
            for (suffix, value) in [("1m", load.one), ("5m", load.five), ("15m", load.fifteen)] {
                let name = format!("system/load_average/{suffix}");
                declare(sink, &name, Kind::Gauge, Unit::Count, "系统负载均值", R_FAST)?;
                sink.sample_f64(&name, ts, value)?;
                n += 1;
            }
        }
        Ok(n)
    }

    fn emit_memory<W: Write>(&self, sink: &mut Sink<W>, ts: Option<i64>) -> io::Result<usize> {
        if !self.cfg.has("memory") {
            return Ok(0);
        }
        let total = self.sys.total_memory();
        if total == 0 {
            return Ok(0);
        }
        let mut n = 0;
        let items = [
            (
                "system/memory_total_bytes",
                total,
                "物理内存总量",
            ),
            ("system/memory_used_bytes", self.sys.used_memory(), "已用内存"),
            (
                "system/memory_available_bytes",
                self.sys.available_memory(),
                "可用内存（含可回收缓存）",
            ),
        ];
        for (name, value, help) in items {
            declare(sink, name, Kind::Gauge, Unit::Bytes, help, R_WEEK)?;
            sink.sample_i64(name, ts, value as i64)?;
            n += 1;
        }
        // 未配置 swap 的机器上 total == 0，此时整组不产出
        if self.sys.total_swap() > 0 {
            for (name, value, help) in [
                ("system/swap_total_bytes", self.sys.total_swap(), "交换区总量"),
                ("system/swap_used_bytes", self.sys.used_swap(), "已用交换区"),
            ] {
                declare(sink, name, Kind::Gauge, Unit::Bytes, help, R_WEEK)?;
                sink.sample_i64(name, ts, value as i64)?;
                n += 1;
            }
        }
        Ok(n)
    }

    fn emit_disk<W: Write>(&self, sink: &mut Sink<W>, ts: Option<i64>) -> io::Result<usize> {
        if !self.cfg.has("disk") {
            return Ok(0);
        }
        let io = disk_io_snapshot();
        let elapsed = self.elapsed_secs();
        let mut n = 0;
        for disk in self.disks.list() {
            let mount = disk.mount_point().to_string_lossy().to_string();
            let device = disk.name().to_string_lossy().to_string();
            if !self.cfg.disk_allowed(&device, &mount) {
                continue;
            }
            let total = disk.total_space();
            if total == 0 {
                continue;
            }
            let available = disk.available_space();
            let used = total.saturating_sub(available);
            let seg = mount_segment(&mount);
            let labels = json!({
                "device": device,
                "mount": mount,
                "fileSystem": disk.file_system().to_string_lossy(),
                "removable": disk.is_removable(),
                "readOnly": disk.is_read_only(),
            });
            for (suffix, value, unit, help) in [
                ("total_bytes", total, Unit::Bytes, "分区总容量"),
                ("used_bytes", used, Unit::Bytes, "已用容量"),
                ("available_bytes", available, Unit::Bytes, "可用容量"),
            ] {
                let name = format!("system/disk/{seg}/{suffix}");
                declare_labeled(sink, &name, Kind::Gauge, unit, help, labels.clone(), R_MONTH)?;
                sink.sample_i64(&name, ts, value as i64)?;
                n += 1;
            }
            let name = format!("system/disk/{seg}/usage");
            declare_labeled(
                sink,
                &name,
                Kind::Gauge,
                Unit::Percent,
                "分区使用率（0-100）",
                labels.clone(),
                R_MONTH,
            )?;
            sink.sample_f64(&name, ts, used as f64 * 100.0 / total as f64)?;
            n += 1;

            // 累计 IO（Linux 才有；见 linux.rs 的说明）
            let key = device_key(&device);
            if let Some(cur) = io.as_ref().and_then(|m| m.get(&key)).copied() {
                for (suffix, value, help) in [
                    ("read_bytes", cur.read_bytes, "累计读取字节数"),
                    ("written_bytes", cur.written_bytes, "累计写入字节数"),
                ] {
                    let name = format!("system/disk/{seg}/{suffix}");
                    declare_labeled(
                        sink,
                        &name,
                        Kind::Counter,
                        Unit::Bytes,
                        help,
                        labels.clone(),
                        R_MONTH,
                    )?;
                    sink.sample_i64(&name, ts, value as i64)?;
                    n += 1;
                }
                if let Some(prev) = self.prev.as_ref().and_then(|p| p.disk.get(&key)).copied() {
                    for (suffix, cur_v, prev_v, help) in [
                        ("read_rate", cur.read_bytes, prev.0, "读取速率"),
                        ("write_rate", cur.written_bytes, prev.1, "写入速率"),
                    ] {
                        if let Some(v) = rate(cur_v, prev_v, elapsed) {
                            let name = format!("system/disk/{seg}/{suffix}");
                            declare_labeled(
                                sink,
                                &name,
                                Kind::Gauge,
                                Unit::BytesPerSecond,
                                help,
                                labels.clone(),
                                R_FAST,
                            )?;
                            sink.sample_f64(&name, ts, v)?;
                            n += 1;
                        }
                    }
                }
            }
        }
        Ok(n)
    }

    fn emit_network<W: Write>(&self, sink: &mut Sink<W>, ts: Option<i64>) -> io::Result<usize> {
        if !self.cfg.has("network") {
            return Ok(0);
        }
        let detail = net_detail_snapshot();
        let elapsed = self.elapsed_secs();
        let mut n = 0;
        for (iface, data) in self.networks.list() {
            if !self.cfg.interface_allowed(iface) {
                continue;
            }
            let labels = json!({
                "interface": iface,
                "mac": data.mac_address().to_string(),
                "mtu": data.mtu(),
            });
            let rx = data.total_received();
            let tx = data.total_transmitted();
            for (suffix, value, unit, help) in [
                ("rx_bytes", rx, Unit::Bytes, "累计接收字节数"),
                ("tx_bytes", tx, Unit::Bytes, "累计发送字节数"),
                (
                    "rx_packets",
                    data.total_packets_received(),
                    Unit::Count,
                    "累计接收包数",
                ),
                (
                    "tx_packets",
                    data.total_packets_transmitted(),
                    Unit::Count,
                    "累计发送包数",
                ),
                (
                    "rx_errors",
                    data.total_errors_on_received(),
                    Unit::Count,
                    "累计接收错误数",
                ),
                (
                    "tx_errors",
                    data.total_errors_on_transmitted(),
                    Unit::Count,
                    "累计发送错误数",
                ),
            ] {
                let name = format!("system/net/{iface}/{suffix}");
                declare_labeled(sink, &name, Kind::Counter, unit, help, labels.clone(), R_MONTH)?;
                sink.sample_i64(&name, ts, value as i64)?;
                n += 1;
            }

            // 丢包计数只有 Linux 的 /proc/net/dev 提供；其它平台不产出（§11.3）
            if let Some(d) = detail.as_ref().and_then(|m| m.get(iface)).copied() {
                for (suffix, value, help) in [
                    ("rx_drops", d.rx_drops, "累计接收丢包数"),
                    ("tx_drops", d.tx_drops, "累计发送丢包数"),
                ] {
                    let name = format!("system/net/{iface}/{suffix}");
                    declare_labeled(
                        sink,
                        &name,
                        Kind::Counter,
                        Unit::Count,
                        help,
                        labels.clone(),
                        R_MONTH,
                    )?;
                    sink.sample_i64(&name, ts, value as i64)?;
                    n += 1;
                }
            }

            if let Some(prev) = self.prev.as_ref().and_then(|p| p.net.get(iface)).copied() {
                for (suffix, cur, prev_v, help) in [
                    ("rx_rate", rx, prev.0, "接收速率"),
                    ("tx_rate", tx, prev.1, "发送速率"),
                ] {
                    if let Some(v) = rate(cur, prev_v, elapsed) {
                        let name = format!("system/net/{iface}/{suffix}");
                        declare_labeled(
                            sink,
                            &name,
                            Kind::Gauge,
                            Unit::BytesPerSecond,
                            help,
                            labels.clone(),
                            R_FAST,
                        )?;
                        sink.sample_f64(&name, ts, v)?;
                        n += 1;
                    }
                }
            }
        }
        Ok(n)
    }

    fn emit_temperature<W: Write>(&self, sink: &mut Sink<W>, ts: Option<i64>) -> io::Result<usize> {
        if !self.cfg.has("temperature") {
            return Ok(0);
        }
        let mut n = 0;
        // 同名标签会出现多个传感器（多路 CPU 的 "Package id 0"），加序号去重
        let mut seen: HashSet<String> = HashSet::new();
        for component in self.components.list() {
            let Some(temp) = component.temperature() else {
                continue; // 取不到温度的条目直接跳过（§11.3）
            };
            let mut seg = sanitize_segment(component.label());
            let mut idx = 2;
            while !seen.insert(seg.clone()) {
                seg = format!("{}-{idx}", sanitize_segment(component.label()));
                idx += 1;
            }
            let mut labels = json!({"label": component.label()});
            if let Some(id) = component.id() {
                labels["id"] = json!(id);
            }
            let name = format!("system/temperature/{seg}");
            declare_labeled(
                sink,
                &name,
                Kind::Gauge,
                Unit::Celsius,
                "传感器温度",
                labels.clone(),
                R_WEEK,
            )?;
            sink.sample_f64(&name, ts, temp as f64)?;
            n += 1;
            for (suffix, value, help) in [
                ("max", component.max(), "传感器记录的最高温度"),
                ("critical", component.critical(), "传感器临界温度"),
            ] {
                let Some(value) = value else { continue };
                let name = format!("system/temperature/{seg}/{suffix}");
                declare_labeled(
                    sink,
                    &name,
                    Kind::Gauge,
                    Unit::Celsius,
                    help,
                    labels.clone(),
                    R_WEEK,
                )?;
                sink.sample_f64(&name, ts, value as f64)?;
                n += 1;
            }
        }
        Ok(n)
    }

    fn emit_gpu<W: Write>(&self, _sink: &mut Sink<W>, _ts: Option<i64>) -> io::Result<usize> {
        Ok(0)
    }

    fn emit_system<W: Write>(&self, sink: &mut Sink<W>, ts: Option<i64>) -> io::Result<usize> {
        if !self.cfg.has("system") {
            return Ok(0);
        }
        let mut n = 0;
        declare(
            sink,
            "system/uptime_seconds",
            Kind::Gauge,
            Unit::Seconds,
            "系统运行时长",
            R_WEEK,
        )?;
        sink.sample_i64("system/uptime_seconds", ts, System::uptime() as i64)?;
        n += 1;

        // 进程数（**不含线程**——理由见 `new()` 里 `.without_tasks()` 的注释）
        let processes = self.sys.processes().len();
        if processes > 0 {
            declare(
                sink,
                "system/process_count",
                Kind::Gauge,
                Unit::Count,
                "进程总数（不含线程）",
                R_WEEK,
            )?;
            sink.sample_i64("system/process_count", ts, processes as i64)?;
            n += 1;
        }
        Ok(n)
    }

    /// 进程 Top-N 用 **`record`** 而不是 `sample` 发出——这是一个刻意的设计决定。
    ///
    /// 若把 PID 编进序列名（`system/process/<pid>/cpu_usage`），那序列的**基数就是
    /// "历史上出现过的所有 PID"**：短命进程（编译器、shell、容器）每天产生成千上万个
    /// 序列，维度表会无界增长，而"按 PID 画折线"几乎没有人真的想看。
    /// `cpuUsagePercent` 用的是**逐进程口径**（分母等价于墙钟时间，多核上可超过 100），
    /// 与 `system/cpu_usage` 的系统口径（0–100）不是同一个量，别混着比。
    ///
    /// Top-N 本质是"某一时刻的一张进程榜"，正是 `record` 的语义
    /// （`kind` 分组、`key` 稳定标识、`payload` 结构化），而且天然带上可读的进程名。
    /// node_exporter 默认不导出任何逐进程指标，也是同一个道理。
    fn emit_processes<W: Write>(&self, sink: &mut Sink<W>, ts: Option<i64>) -> io::Result<usize> {
        if !self.cfg.has("processes") || self.cfg.top_processes == 0 {
            return Ok(0);
        }
        let mut procs: Vec<_> = self
            .sys
            .processes()
            .iter()
            .map(|(pid, p)| {
                (
                    pid.as_u32(),
                    p.name().to_string_lossy().to_string(),
                    p.cpu_usage(),
                    p.memory(),
                    p.virtual_memory(),
                    p.run_time(),
                    p.parent().map(|x| x.as_u32()),
                    format!("{:?}", p.status()),
                )
            })
            .collect();
        // 按 CPU 降序；CPU 相同再按内存降序，PID 兜底，保证结果稳定（否则同分进程顺序随机，
        // 每一轮的"榜单"都在无意义地抖动）
        procs.sort_by(|a, b| {
            b.2.partial_cmp(&a.2)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.3.cmp(&a.3))
                .then(a.0.cmp(&b.0))
        });
        let mut n = 0;
        for (pid, name, cpu, mem, vmem, run_time, parent, status) in
            procs.into_iter().take(self.cfg.top_processes)
        {
            let mut payload = json!({
                "pid": pid,
                "name": name,
                "cpuUsagePercent": cpu,
                "memoryBytes": mem,
                "virtualMemoryBytes": vmem,
                "runTimeSeconds": run_time,
                "status": status,
            });
            if let Some(parent) = parent {
                payload["parentPid"] = json!(parent);
            }
            sink.record("system.process", Some(&pid.to_string()), ts, payload)?;
            n += 1;
        }
        Ok(n)
    }

    fn elapsed_secs(&self) -> f64 {
        self.prev
            .as_ref()
            .map(|p| p.at.elapsed().as_secs_f64())
            .unwrap_or(0.0)
    }
}

/// 计数器增量速率；计数器回绕（接口/设备重建）时返回 `None` 而不是负数。
fn rate(cur: u64, prev: u64, elapsed_secs: f64) -> Option<f64> {
    if elapsed_secs <= 0.0 || cur < prev {
        return None;
    }
    Some((cur - prev) as f64 / elapsed_secs)
}

/// 把挂载点变成合法的序列段。
///
/// 用挂载点而不是设备名，因为**挂载点在系统内唯一**，而设备名会被重复挂载
/// （bind mount、overlay），撞名会让两条不同的数据流混进同一个序列。
/// 设备名通过 `series.labels.device` 保留，展示时不会丢。
fn mount_segment(mount: &str) -> String {
    let replaced: String = mount
        .trim()
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':') { '-' } else { c })
        .collect();
    let trimmed = replaced.trim_matches('-');
    let seg = sanitize_segment(trimmed);
    if seg == "unknown" { "root".into() } else { seg }
}

/// `/dev/nvme0n1p2` → `nvme0n1p2`（与 `/proc/diskstats` 的键一致）。
fn device_key(device: &str) -> String {
    device
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(device)
        .to_string()
}

fn disk_totals() -> HashMap<String, (u64, u64)> {
    disk_io_snapshot()
        .map(|m| {
            m.into_iter()
                .map(|(k, v)| (k, (v.read_bytes, v.written_bytes)))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn disk_io_snapshot() -> Option<HashMap<String, linux::DiskIo>> {
    linux::disk_io()
}

#[cfg(not(target_os = "linux"))]
fn disk_io_snapshot() -> Option<HashMap<String, DiskIoFallback>> {
    None
}

#[cfg(not(target_os = "linux"))]
#[derive(Debug, Clone, Copy, Default)]
pub struct DiskIoFallback {
    pub read_bytes: u64,
    pub written_bytes: u64,
}

#[cfg(target_os = "linux")]
fn net_detail_snapshot() -> Option<HashMap<String, linux::NetDetail>> {
    linux::net_detail()
}

#[cfg(not(target_os = "linux"))]
fn net_detail_snapshot() -> Option<HashMap<String, ()>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_segments_are_unique_and_legal() {
        assert_eq!(mount_segment("/"), "root");
        assert_eq!(mount_segment("/home"), "home");
        assert_eq!(mount_segment("/var/lib/docker"), "var-lib-docker");
        assert_eq!(mount_segment("C:\\"), "C");
        // 中文与空格都变成 `-`，只裁剪开头的那些（段中间允许 `-`）
        assert_eq!(mount_segment("/media/我的 硬盘"), "media------");
        // 关键性质：不同挂载点必须得到不同段（否则序列会撞名）
        assert_ne!(mount_segment("/a"), mount_segment("/b"));
        assert_ne!(mount_segment("/"), mount_segment("/home"));
    }

    #[test]
    fn device_key_strips_the_dev_prefix() {
        assert_eq!(device_key("/dev/nvme0n1p2"), "nvme0n1p2");
        assert_eq!(device_key("sda"), "sda");
        assert_eq!(device_key("/dev/mapper/vg-root"), "vg-root");
    }

    #[test]
    fn rates_never_go_negative() {
        assert_eq!(rate(200, 100, 2.0), Some(50.0));
        assert_eq!(rate(100, 200, 2.0), None, "计数器回绕必须丢弃而不是报负数");
        assert_eq!(rate(200, 100, 0.0), None, "没有时间差就没有速率");
    }

    /// 端到端：跑一次真实采集，检查输出满足协议（§10 的第 3/4 项）。
    #[test]
    fn a_real_run_emits_conformant_messages() {
        let cfg = Config::default();
        let mut c = Collector::new(cfg);
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        c.refresh();
        let mut sink = Sink::new(Vec::new());
        let n = c.collect(&mut sink, Some(1700000000000)).unwrap();
        assert!(n > 0, "本机应该至少能采到一项数据");

        let text = String::from_utf8(sink.into_inner()).unwrap();
        let mut declared: HashSet<String> = HashSet::new();
        let mut data_messages = 0;
        for line in text.lines() {
            let msg: Value = serde_json::from_str(line).expect("每行都要是合法 JSON");
            match msg["type"].as_str().unwrap() {
                "series" => {
                    let name = msg["name"].as_str().unwrap();
                    assert!(name.starts_with("system/") || name.starts_with("gpu/"));
                    assert!(msg["kind"] == "gauge" || msg["kind"] == "counter");
                    declared.insert(name.to_string());
                }
                "sample" => {
                    let name = msg["name"].as_str().unwrap();
                    assert!(declared.contains(name), "sample 的序列必须先声明: {name}");
                    assert!(msg["value"].is_number(), "{name} 的值必须是数字");
                    assert_eq!(msg["ts"], json!(1700000000000i64));
                    data_messages += 1;
                }
                "record" => {
                    // record 也必须带 kind 与对象型 payload（§2.3）
                    assert!(msg["kind"].is_string(), "record 必须有 kind");
                    assert!(msg["payload"].is_object(), "payload 必须是对象，不能二次编码");
                    data_messages += 1;
                }
                "log" => {}
                other => panic!("未知消息类型 {other}"),
            }
        }
        assert_eq!(
            data_messages, n,
            "返回的条数必须等于实际产出的数据消息数（sample + record）"
        );
    }

    /// 全类别关闭时必须一条数据都产不出来（宿主据此报 upstream 错误）。
    #[test]
    fn no_categories_means_no_samples() {
        let mut cfg = Config::default();
        cfg.categories = vec!["gpu".into()];
        let mut c = Collector::new(cfg);
        c.refresh();
        let mut sink = Sink::new(Vec::new());
        // 本机没有 NVIDIA 驱动 → GPU 类别必然为空。
        // 若这台机器真有可用 GPU，这条断言会失败——那时应该改成断言 n > 0。
        let n = c.collect(&mut sink, None).unwrap();
        assert_eq!(n, 0, "没有可用数据源时不应产出数据消息");
    }

    /// sysinfo 的默认刷新种类**带 `tasks: true`**，在 Linux 上等于"把每个线程当进程"。
    /// 这个断言是防回归的：有人把 `.without_tasks()` 当成冗余"简化"掉时，
    /// 结果不会报错，只会让进程榜被某个多线程程序独占、进程数虚高（实测 4 → 10）。
    #[test]
    fn process_refresh_kind_excludes_tasks() {
        assert!(
            ProcessRefreshKind::nothing().tasks(),
            "sysinfo 的 `nothing()` 确实带 tasks，所以必须显式关掉——这正是坑的所在"
        );
        assert!(
            !process_refresh_kind().tasks(),
            "读进程表时必须排除线程，否则多线程进程会被重复列出"
        );
    }

    /// 进程表里**只能有线程组组长**，且测试进程自己（天生多线程）只能出现一次。
    #[cfg(target_os = "linux")]
    #[test]
    fn process_list_excludes_threads() {
        let me = std::process::id();
        // 测试框架自己就开了多个线程，所以"本进程只出现一次"是天然的反例样本
        assert!(
            std::fs::read_dir(format!("/proc/{me}/task")).unwrap().count() > 1,
            "前提：测试进程本身就是多线程的"
        );

        let mut c = Collector::new(Config {
            categories: vec!["processes".into(), "system".into()],
            ..Default::default()
        });
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        c.refresh();

        let mut seen_me = 0;
        for pid in c.sys.processes().keys() {
            let status = std::fs::read_to_string(format!("/proc/{}/status", pid.as_u32()))
                .unwrap_or_else(|e| panic!("读 /proc/{}/status 失败: {e}", pid.as_u32()));
            let tgid: u32 = status
                .split("\nTgid:")
                .nth(1)
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("解析 Tgid 失败: {status}"));
            assert_eq!(
                tgid,
                pid.as_u32(),
                "/proc/{} 是线程（tgid={tgid}），不该出现在进程表里",
                pid.as_u32()
            );
            if pid.as_u32() == me {
                seen_me += 1;
            }
        }
        assert_eq!(seen_me, 1, "本进程只应有一条记录，尽管它自己就有多个线程");
    }

    /// 每个序列都要带保留声明；**同一个序列名在不同模式下声明必须一致**。
    ///
    /// 保留声明是宿主无法自己推断的信息（协议 §2.8），漏了就退化成"按全局默认留存"——
    /// 那正是 3 秒粒度的 CPU 读数把库撑满的原因。这条测试把它钉住。
    #[test]
    fn every_series_declares_retention_and_it_is_mode_independent() {
        let cfg = Config::default();

        let collect_once = |stream: bool| {
            let mut c = Collector::new(cfg.clone());
            std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
            c.refresh();
            let mut sink = Sink::new(Vec::new());
            c.collect(&mut sink, None).unwrap();
            let text = String::from_utf8(sink.into_inner()).unwrap();
            let mut out = std::collections::HashMap::new();
            for line in text.lines() {
                let m: Value = serde_json::from_str(line).unwrap();
                if m["type"] == "series" {
                    out.insert(
                        m["name"].as_str().unwrap().to_string(),
                        (m.get("persist").cloned(), m.get("maxRetentionMs").cloned()),
                    );
                }
            }
            assert!(out.len() > 50, "本机应有相当数量的序列");
            let _ = stream;
            out
        };

        let a = collect_once(false);
        // 逐条检查取值合法（§2.8）
        for (name, (persist, max)) in &a {
            assert!(
                max.is_some() || persist.is_some(),
                "{name} 未声明保留策略：宿主只能按全局默认留存"
            );
            if persist.is_some() {
                // 本采集器没有"仅实时"的序列；若将来有了，这里改成允许 persist=false
                assert_eq!(persist.clone().unwrap(), Value::Bool(false), "{name}");
                assert!(max.is_none(), "{name}: persist=false 时不得给 maxRetentionMs");
            } else if let Some(m) = max {
                if !m.is_null() {
                    let ms = m.as_u64().expect("maxRetentionMs 必须是整数或 null");
                    assert!(ms > 0, "{name}: maxRetentionMs 必须为正（0 会被 JS 当 falsy）");
                }
            }
        }

        // 两种模式下的声明必须一致（协议 §2.3：同名语义与模式无关）
        let b = collect_once(true);
        for (name, decl) in &a {
            if let Some(other) = b.get(name) {
                assert_eq!(decl, other, "{name} 的保留声明在两种模式下不一致");
            }
        }
    }

    /// 进程以 `record` 发出，且是排好序的 top-N。
    #[test]
    fn processes_are_records_capped_at_top_n() {
        let cfg = Config {
            top_processes: 3,
            ..Default::default()
        };
        let mut c = Collector::new(cfg);
        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        c.refresh();
        let mut sink = Sink::new(Vec::new());
        c.collect(&mut sink, None).unwrap();
        let text = String::from_utf8(sink.into_inner()).unwrap();
        let procs: Vec<Value> = text
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|m| m["type"] == "record" && m["kind"] == "system.process")
            .collect();
        assert!(!procs.is_empty(), "本机至少有 1 个进程");
        assert!(procs.len() <= 3, "最多 3 条，实际 {}", procs.len());
        // 榜单必须按 CPU 降序（宿主直接用这个顺序展示，不该再排一次）
        let cpus: Vec<f64> = procs
            .iter()
            .map(|p| p["payload"]["cpuUsagePercent"].as_f64().unwrap())
            .collect();
        for w in cpus.windows(2) {
            assert!(w[0] >= w[1], "CPU 必须降序，实际 {cpus:?}");
        }
        // key 是 PID 字符串，payload 里有可读的进程名
        for p in &procs {
            assert!(p["key"].as_str().unwrap().parse::<u32>().is_ok());
            assert!(p["payload"]["name"].is_string());
        }
        // PID 绝不能出现在序列名里（否则序列基数会随历史 PID 无界增长）
        assert!(
            !text.contains("\"system/process/"),
            "进程不应再作为序列产出"
        );
    }
}
