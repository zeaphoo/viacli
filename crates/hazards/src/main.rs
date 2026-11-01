//! 灾害与环境采集器（`hazards`）——`docs/07-collector-protocol.md` 的实现。
//!
//! 覆盖三个免注册、免 key 的公共源：
//! * USGS 地震（`hazard.earthquake` 记录 + 计数序列）
//! * NWS 天气预警（`hazard.weather_alert` 记录 + 计数序列）
//! * NOAA SWPC 空间天气（Kp 指数序列）
//!
//! 拉取型、无需进程内必需状态，因此 `oneshot` 是自然模式；`state` 只用于
//! "距上次抓取不足 intervalMs 就跳过"的自我限流。

mod collect;
mod config;
mod describe;
mod sources;

use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use base_http::{ErrorKind, Sink};

use crate::collect::{Collector, State};
use crate::config::{Config, Request};

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("viacli-hazards: 内部错误: {e}");
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
viacli-hazards —— 灾害与环境采集器（遵循 docs/07-collector-protocol.md）

用法：
  viacli-hazards --describe            输出自描述 JSON（宿主用它登记采集器）
  viacli-hazards [--config <path>]     从 stdin 读一行 JSON 请求后开始采集

参数：
  --describe          自描述，不采集
  --config <path>     从文件读取请求（含 config 字段）或配置对象；stdin 优先
  -h, --help          显示本帮助
  -V, --version       显示版本

退出码：0 成功 / 2 配置错误 / 3 凭据 / 4 上游不可用 / 5 内部错误
";

fn fatal<W: Write>(sink: &mut Sink<W>, kind: ErrorKind, message: &str) -> ExitCode {
    let _ = sink.error(kind, kind.retryable(), message);
    eprintln!("viacli-hazards: {message}");
    ExitCode::from(kind.exit_code() as u8)
}

/// 从 stdin 的一行 / `--config` 文件里取回请求（§2.2：stdin 优先）。
fn read_request(args: &Args, sink: &mut Sink<impl Write>) -> io::Result<Result<Request, ExitCode>> {
    let file_arg = match &args.config_path {
        Some(p) => match std::fs::read_to_string(p) {
            Ok(s) => Some(s),
            Err(e) => {
                return Ok(Err(fatal(
                    sink,
                    ErrorKind::Config,
                    &format!("读取 --config {} 失败: {e}", p.display()),
                )));
            }
        },
        None => None,
    };

    let stdin_is_terminal = io::stdin().is_terminal();
    let mut line = String::new();
    let mut stdin_alive = false;
    if !stdin_is_terminal {
        let n = {
            let mut lock = io::stdin().lock();
            lock.read_line(&mut line)?
        };
        stdin_alive = n > 0;
    }

    let mut req: Option<Request> = if stdin_alive {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            None
        } else {
            match serde_json::from_str(trimmed) {
                Ok(r) => Some(r),
                Err(e) => {
                    return Ok(Err(fatal(
                        sink,
                        ErrorKind::Config,
                        &format!("请求不是合法 JSON: {e}；原始行: {trimmed}"),
                    )));
                }
            }
        }
    } else if stdin_is_terminal && file_arg.is_none() {
        eprint!("{USAGE}");
        eprintln!("\n错误：没有 stdin 请求，也没有 --config。");
        return Ok(Err(ExitCode::from(ErrorKind::Config.exit_code() as u8)));
    } else {
        None
    };

    if let (None, Some(text)) = (&req, &file_arg) {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            match serde_json::from_str::<serde_json::Value>(trimmed) {
                Ok(v) if v.get("config").is_some() => match serde_json::from_value(v) {
                    Ok(r) => req = Some(r),
                    Err(e) => {
                        return Ok(Err(fatal(
                            sink,
                            ErrorKind::Config,
                            &format!("--config 文件不是合法请求: {e}"),
                        )));
                    }
                },
                Ok(v) => match serde_json::from_value::<Config>(v) {
                    Ok(c) => {
                        req = Some(Request {
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
                        return Ok(Err(fatal(
                            sink,
                            ErrorKind::Config,
                            &format!("--config 文件不是合法配置: {e}"),
                        )));
                    }
                },
                Err(e) => {
                    return Ok(Err(fatal(
                        sink,
                        ErrorKind::Config,
                        &format!("--config 文件不是合法 JSON: {e}"),
                    )));
                }
            }
        }
    }

    match req {
        Some(r) => Ok(Ok(r)),
        None => Ok(Err(fatal(
            sink,
            ErrorKind::Config,
            "没有收到请求：stdin 为空且未提供 --config",
        ))),
    }
}

fn run() -> io::Result<ExitCode> {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("viacli-hazards: {e}");
            return Ok(ExitCode::from(ErrorKind::Config.exit_code() as u8));
        }
    };
    if args.help {
        eprint!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    if args.version {
        println!("viacli-hazards {}", env!("CARGO_PKG_VERSION"));
        return Ok(ExitCode::SUCCESS);
    }

    let mut out = io::stdout().lock();
    if args.describe {
        serde_json::to_writer(&mut out, &describe::describe())?;
        writeln!(out)?;
        return Ok(ExitCode::SUCCESS);
    }

    let mut sink = Sink::new(out);
    let req = match read_request(&args, &mut sink)? {
        Ok(r) => r,
        Err(code) => return Ok(code),
    };

    // ---- 版本协商（§9：不兼容时绝不"尽力而为"）----
    if req.protocol != 1 {
        return Ok(fatal(
            &mut sink,
            ErrorKind::Config,
            &format!("协议版本不兼容：请求 protocol={}，本采集器只支持 1", req.protocol),
        ));
    }

    // ---- 配置 ----
    let cfg = req.config.clone().unwrap_or_default();
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
    sink.meta("hazards", env!("CARGO_PKG_VERSION"))?;

    let stream_mode = req.mode == "stream";
    let host_now = req.now;
    let tag = req.tag();
    let max_lines = req.limits.as_ref().and_then(|l| l.max_lines);
    let max_bytes = req.limits.as_ref().and_then(|l| l.max_output_bytes);
    // stream 的循环间隔取"宿主期望值"与"采集器下限"的较大者：宿主配 1 秒也只是白打上游。
    let stream_interval = Duration::from_millis(
        req.interval_ms.unwrap_or(cfg.interval_ms).max(cfg.interval_ms),
    );

    let mut collector = match Collector::new(cfg, State::from_request(req.state.as_ref())) {
        Ok(c) => c,
        Err(e) => return Ok(fatal(&mut sink, e.kind, &e.message)),
    };

    if !stream_mode {
        if collector.should_skip(host_now) {
            collector.emit_skipped(&mut sink, host_now)?;
            sink.state(collector.state().to_value())?;
            sink.log("info", &format!("{tag}: 距上次抓取不足 intervalMs，本轮跳过网络请求"))?;
            return Ok(ExitCode::SUCCESS);
        }
        let stats = collector.collect(&mut sink, host_now)?;
        if stats.sources_ok == 0 {
            return Ok(fatal(
                &mut sink,
                ErrorKind::Upstream,
                &format!("{} 个数据源全部抓取失败", stats.sources_total),
            ));
        }
        sink.state(collector.state().to_value())?;
        sink.log(
            "info",
            &format!(
                "oneshot 采集完成（{tag}）：成功 {}/{} 个源，产出 {} 条记录",
                stats.sources_ok, stats.sources_total, stats.records_emitted
            ),
        )?;
        return Ok(ExitCode::SUCCESS);
    }

    // ---- stream：常驻，直到 stdin 关闭（§4.2）----
    let (tx, rx) = mpsc::channel::<()>();
    if io::stdin().is_terminal() {
        let _ = tx.send(());
    } else {
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
    }

    let run_start = Instant::now();
    loop {
        // 时间戳：以宿主给的 now 为基准 + 单调流逝（§2.5），避免自造时间与长期漂移。
        let ts = host_now.map(|base| base + run_start.elapsed().as_millis() as i64);
        let round_start = Instant::now();

        let stats = collector.collect(&mut sink, ts)?;
        sink.state(collector.state().to_value())?;
        if stats.sources_ok == 0 {
            // 单轮全失败很可能是网络抖动，不结束会话；宿主会靠"活着但没数据"的监视重启我们。
            sink.error(
                ErrorKind::Upstream,
                true,
                &format!("{} 个数据源全部抓取失败", stats.sources_total),
            )?;
        }

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

        let wait = stream_interval.saturating_sub(round_start.elapsed());
        let wait = if wait.is_zero() { Duration::from_millis(1) } else { wait };
        match rx.recv_timeout(wait) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }

    sink.log("info", &format!("{tag}: stdin 已关闭，正常退出"))?;
    Ok(ExitCode::SUCCESS)
}
