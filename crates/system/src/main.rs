//! 本地系统采集器（`system`）——`docs/07-collector-protocol.md` 的参考实现。
//!
//! 这个二进制的作用不只是"采数据"，更是**验证协议标准是否好用**：
//! 它只依赖标准里定义的机制（`--describe`、stdin 请求、NDJSON 输出、两种运行模式、
//! 退出码、降级语义），没有任何"只有它自己知道"的约定。

mod collect;
mod config;
#[cfg(target_os = "linux")]
mod linux;
mod protocol;

use crate::collect::Collector;
use crate::config::Config;
use crate::protocol::{ErrorKind, Sink};
use serde_json::json;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// CPU 使用率需要两次采样，两次之间至少要等这么久（sysinfo 的硬性要求）。
const MIN_CPU_WINDOW: Duration = Duration::from_millis(250);

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
            // 宿主主动关闭了管道（采集器被取消）——不是错误，安静退出
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("viacli-sys: 内部错误: {e}");
            ExitCode::from(ErrorKind::Internal.exit_code() as u8)
        }
    }
}

struct Args {
    describe: bool,
    config_path: Option<PathBuf>,
    help: bool,
    version: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        describe: false,
        config_path: None,
        help: false,
        version: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--describe" => args.describe = true,
            "--help" | "-h" => args.help = true,
            "--version" | "-V" => args.version = true,
            "--config" => {
                let p = it.next().ok_or("--config 需要一个路径参数")?;
                args.config_path = Some(PathBuf::from(p));
            }
            other => return Err(format!("未知参数 {other:?}（用 --help 查看用法）")),
        }
    }
    Ok(args)
}

const USAGE: &str = "\
viacli-sys —— 本地系统信息采集器（遵循 docs/07-collector-protocol.md）

用法：
  viacli-sys --describe            输出自描述 JSON（宿主用它登记采集器）
  viacli-sys [--config <path>]     从 stdin 读一行 JSON 请求后开始采集

参数：
  --describe          自描述，不采集
  --config <path>     从文件读取请求（该文件含 config 字段）或配置对象；stdin 优先
  -h, --help          显示本帮助
  -V, --version       显示版本

退出码：0 成功 / 2 配置错误 / 3 认证错误 / 4 上游不可用 / 5 内部错误
";

/// 记录类型清单（协议 §8.1 的 `recordKinds`）。
///
/// 保留声明与序列**共用同一套常量**（`collect::R_*`），避免"序列写 1 小时、
/// 同类的记录写 1 天"这种两边不一致。
fn record_kinds() -> serde_json::Value {
    let mut out = Vec::new();
    for (kind, help, retention) in [
        (
            "system.info",
            "主机静态信息（主机名/系统版本/CPU 型号/开机时刻）——实体的静态属性",
            collect::R_FOREVER,
        ),
        (
            "system.process",
            "进程榜快照（Top-N；cpuUsagePercent 为逐进程口径，可超过 100）",
            collect::R_FAST,
        ),
    ] {
        let mut obj = serde_json::Map::new();
        obj.insert("kind".into(), json!(kind));
        obj.insert("help".into(), json!(help));
        for (k, v) in retention.fields() {
            obj.insert(k.to_string(), v);
        }
        out.push(serde_json::Value::Object(obj));
    }
    serde_json::Value::Array(out)
}

fn describe() -> serde_json::Value {
    json!({
        "protocol": 1,
        "kind": "system",
        "title": "本机系统信息",
        "version": env!("CARGO_PKG_VERSION"),
        "modes": ["oneshot", "stream"],
        "defaultMode": "stream",
        "defaultIntervalMs": 2000,
        "output": "both",
        // 记录类型是**静态**的（由代码决定），所以在这里声明；序列是动态的，
        // 随 `series` 消息声明。保留字段与序列完全一致（协议 §2.8）。
        "recordKinds": record_kinds(),
        "configSchema": {
            "type": "object",
            "properties": {
                "intervalMs": {
                    "type": "integer", "title": "采样间隔(ms)", "default": 2000, "minimum": 250
                },
                "categories": {
                    "type": "array", "title": "采集类别",
                    "items": { "enum": config::ALL_CATEGORIES },
                    "default": config::ALL_CATEGORIES
                },
                "topProcesses": {
                    "type": "integer", "title": "进程 Top N（0 表示不采集）", "default": 5, "minimum": 0
                },
                "includeInterfaces": {
                    "type": "array", "title": "只采集匹配的网卡（子串）",
                    "items": { "type": "string" }, "default": []
                },
                "excludeInterfaces": {
                    "type": "array", "title": "排除匹配的网卡（子串）",
                    "items": { "type": "string" },
                    "default": ["lo", "docker", "veth", "br-", "virbr", "tun", "tap", "wg", "zt"]
                },
                "includeDisks": {
                    "type": "array", "title": "只采集匹配的磁盘（子串）",
                    "items": { "type": "string" }, "default": []
                },
                "excludeDisks": {
                    "type": "array", "title": "排除匹配的磁盘（子串）",
                    "items": { "type": "string" }, "default": []
                }
            }
        }
    })
}

/// 报一个致命错误：发 `error` 消息（宿主可能读不到 stderr）+ 走 stderr + 返回退出码。
fn fatal<W: Write>(sink: &mut Sink<W>, kind: ErrorKind, message: &str) -> ExitCode {
    let retryable = matches!(
        kind,
        ErrorKind::Network | ErrorKind::Upstream | ErrorKind::Internal
    );
    let _ = sink.error(kind, retryable, message);
    eprintln!("viacli-sys: {message}");
    ExitCode::from(kind.exit_code() as u8)
}

fn run() -> io::Result<ExitCode> {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("viacli-sys: {e}");
            return Ok(ExitCode::from(ErrorKind::Config.exit_code() as u8));
        }
    };
    if args.help {
        eprint!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    if args.version {
        println!("viacli-sys {}", env!("CARGO_PKG_VERSION"));
        return Ok(ExitCode::SUCCESS);
    }

    let mut out = io::stdout().lock();
    if args.describe {
        serde_json::to_writer(&mut out, &describe())?;
        writeln!(out)?;
        return Ok(ExitCode::SUCCESS);
    }

    let mut sink = Sink::new(out);

    // ---- 取得请求（§2.2：stdin 优先；--config 是替代来源）----
    let file_arg = match &args.config_path {
        Some(p) => match std::fs::read_to_string(p) {
            Ok(s) => Some(s),
            Err(e) => {
                return Ok(fatal(
                    &mut sink,
                    ErrorKind::Config,
                    &format!("读取 --config {} 失败: {e}", p.display()),
                ));
            }
        },
        None => None,
    };

    let stdin_is_terminal = io::stdin().is_terminal();
    // 注意：只在读请求这一小段持有 stdin 锁。`StdinLock` 不是 `Send`，
    // 不能跨线程持有；真正的 stdin 句柄（`Stdin`）才是 `Send`，交给监听线程用。
    let mut line = String::new();
    let mut stdin_alive = false;
    if !(stdin_is_terminal) {
        let n = {
            let mut lock = io::stdin().lock();
            lock.read_line(&mut line)?
        };
        stdin_alive = n > 0;
    }

    let mut req: Option<config::Request> = if stdin_alive {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            stdin_alive = false;
            None
        } else {
            match serde_json::from_str(trimmed) {
                Ok(r) => Some(r),
                Err(e) => {
                    return Ok(fatal(
                        &mut sink,
                        ErrorKind::Config,
                        &format!("请求不是合法 JSON: {e}；原始行: {trimmed}"),
                    ));
                }
            }
        }
    } else if stdin_is_terminal && file_arg.is_none() {
        eprint!("{USAGE}");
        eprintln!("\n错误：没有 stdin 请求，也没有 --config。");
        return Ok(ExitCode::from(ErrorKind::Config.exit_code() as u8));
    } else {
        None
    };
    // 终端交互 + 给了配置文件：不要傻等 stdin（否则看起来"卡住"）
    if stdin_is_terminal {
        stdin_alive = false;
    }

    // --config 文件既可能是请求（含 config 字段），也可能直接是配置对象
    if req.is_none()
        && let Some(text) = &file_arg
    {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            match serde_json::from_str::<serde_json::Value>(trimmed) {
                Ok(v) if v.get("config").is_some() => match serde_json::from_value(v) {
                    Ok(r) => req = Some(r),
                    Err(e) => {
                        return Ok(fatal(
                            &mut sink,
                            ErrorKind::Config,
                            &format!("--config 文件不是合法请求: {e}"),
                        ));
                    }
                },
                Ok(v) => match serde_json::from_value::<Config>(v) {
                    Ok(c) => {
                        req = Some(config::Request {
                            protocol: 1,
                            run_id: None,
                            now: None,
                            mode: "oneshot".into(),
                            interval_ms: None,
                            config: Some(c),
                            state: None,
                            limits: None,
                        });
                    }
                    Err(e) => {
                        return Ok(fatal(
                            &mut sink,
                            ErrorKind::Config,
                            &format!("--config 文件不是合法配置: {e}"),
                        ));
                    }
                },
                Err(e) => {
                    return Ok(fatal(
                        &mut sink,
                        ErrorKind::Config,
                        &format!("--config 文件不是合法 JSON: {e}"),
                    ));
                }
            }
        }
    }

    let Some(req) = req else {
        return Ok(fatal(
            &mut sink,
            ErrorKind::Config,
            "没有收到请求：stdin 为空且未提供 --config",
        ));
    };

    // ---- 版本协商（§9：不兼容时绝不"尽力而为"）----
    if req.protocol != 1 {
        return Ok(fatal(
            &mut sink,
            ErrorKind::Config,
            &format!(
                "协议版本不兼容：请求 protocol={}，本采集器只支持 1",
                req.protocol
            ),
        ));
    }

    // ---- 配置 ----
    let mut cfg = req.config.clone().unwrap_or_default();
    if let Some(ms) = req.interval_ms {
        // 宿主的调度间隔优先于用户配置：它才是真正决定产出节奏的一方
        cfg.interval_ms = ms;
    }
    if let Err(e) = cfg.validate() {
        return Ok(fatal(&mut sink, ErrorKind::Config, &e));
    }
    if !matches!(req.mode.as_str(), "oneshot" | "stream") {
        return Ok(fatal(
            &mut sink,
            ErrorKind::Config,
            &format!("未知 mode {:?}；可用值：oneshot, stream", req.mode),
        ));
    }

    // meta 必须在任何其它消息之前（§2.3）
    sink.meta("system", env!("CARGO_PKG_VERSION"))?;

    let stream_mode = req.mode == "stream";
    let interval = Duration::from_millis(cfg.interval_ms);
    let timeout_millis = req.limits.as_ref().and_then(|l| l.timeout_ms);
    let max_lines = req.limits.as_ref().and_then(|l| l.max_lines);
    let max_bytes = req.limits.as_ref().and_then(|l| l.max_output_bytes);

    let tag = req.tag();
    let host_now = req.now;
    let run_start = Instant::now();
    let mut collector = Collector::new(cfg);
    // 第一次刷新在构造里完成，此时 CPU 值是空的；等过一个最小窗口再刷新才有意义。
    //
    // **只在真的采 CPU 时才等**：这段等待是 oneshot 里最大的一笔固定开销
    // （实测整轮 ~265ms，其中 250ms 就是这里），而"内存/磁盘容量/温度/进程"这类
    // 单次取值就有的量根本不需要时间差。采内存的采集器不该替 CPU 付这 250ms。
    if collector.wants_cpu() {
        thread::sleep(MIN_CPU_WINDOW);
    }
    collector.refresh();

    if !stream_mode {
        let n = collector.collect(&mut sink, host_now)?;
        if n == 0 {
            return Ok(fatal(
                &mut sink,
                ErrorKind::Upstream,
                "所有类别都取不到数据：可能缺少权限或运行在不提供这些信息的虚拟化环境中",
            ));
        }
        sink.log(
            "info",
            &format!("oneshot 采集完成（{tag}），产出 {n} 条数据（sample + record）"),
        )?;
        return Ok(ExitCode::SUCCESS);
    }

    // ---- stream：常驻，直到 stdin 关闭（§4.2）----
    // 宿主关闭 stdin 就表示"该结束了"；用一个线程监听读端并在 EOF 时通知主循环，
    // 这样轮询 sleep 不会拖慢退出（§4.4：应尽快退出）。
    let (tx, rx) = mpsc::channel::<()>();
    if stdin_alive {
        thread::spawn(move || {
            let mut stdin = io::stdin();
            let mut buf = [0u8; 512];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => continue,
                }
            }
            let _ = tx.send(());
        });
    } else {
        let _ = tx.send(());
    }

    let mut empty_rounds = 0u32;
    loop {
        // 时间戳：以宿主给的 now 为基准 + 单调流逝，避免自己造时间又避免长期漂移（§2.5）
        let ts = host_now.map(|base| base + run_start.elapsed().as_millis() as i64);
        let round_start = Instant::now();
        collector.refresh();
        let n = collector.collect(&mut sink, ts)?;
        if n == 0 {
            empty_rounds += 1;
            if empty_rounds >= 3 {
                return Ok(fatal(
                    &mut sink,
                    ErrorKind::Upstream,
                    "连续 3 轮都没有采到任何数据，停止运行",
                ));
            }
        } else {
            empty_rounds = 0;
        }

        // 自己遵守 limits，而不是等宿主来打断（§4.5）
        let hit_lines = max_lines.is_some_and(|max| sink.emitted() >= max);
        let hit_bytes = max_bytes.is_some_and(|max| sink.bytes() >= max);
        if hit_lines || hit_bytes {
            sink.log(
                "warn",
                &format!(
                    "已达到 limits（已写 {} 行 / {} 字节），主动停止（§4.5）",
                    sink.emitted(),
                    sink.bytes()
                ),
            )?;
            return Ok(ExitCode::SUCCESS);
        }

        // 超时约束是"单轮产出的宽限期"：采集本身没有阻塞点，此处仅用于提示
        let _ = timeout_millis;

        // `intervalMs` 是**两轮开始之间的目标间隔**（§4.2），因此要扣掉本轮耗时；
        // 否则采集越慢，实际周期越长（间隔会漂成 interval + 采集耗时）。
        let wait = interval.saturating_sub(round_start.elapsed());
        // 本轮已经超时（采集慢于间隔）时不要紧循环，至少歇 1ms
        let wait = if wait.is_zero() {
            Duration::from_millis(1)
        } else {
            wait
        };
        match rx.recv_timeout(wait) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }

    sink.log("info", &format!("{tag}: stdin 已关闭，正常退出"))?;
    Ok(ExitCode::SUCCESS)
}
