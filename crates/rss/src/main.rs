//! RSS/Atom 订阅采集器（`rss`）——`docs/07-collector-protocol.md` 的又一个参考实现。
//!
//! 它验证协议里"拉取型数据源"的那一半：`oneshot` 为主、`state` 做增量游标、
//! `record` 承载结构化快照、`sample` 承载健康读数。
//!
//! 设计取舍见 `DESIGN.md` §6.2.13.5：订阅源是**拉取**型、无需进程内状态，
//! 因此 `oneshot` 才是它的自然模式；`stream` 也支持，但默认用 `oneshot`。

mod collect;
mod config;
mod describe;
mod feeds;
mod fetch;
mod protocol;

use crate::collect::{Collector, State};
use crate::config::{Config, Request};
use crate::protocol::{ErrorKind, Sink};
use serde_json::Value;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => {
            // 宿主主动关闭了管道（采集器被取消）——不是错误，安静退出
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("viacli-rss: 内部错误: {e}");
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
viacli-rss —— RSS/Atom 订阅采集器（遵循 docs/07-collector-protocol.md）

用法：
  viacli-rss --describe            输出自描述 JSON（宿主用它登记采集器）
  viacli-rss [--config <path>]     从 stdin 读一行 JSON 请求后开始采集

参数：
  --describe          自描述，不采集
  --config <path>     从文件读取请求（该文件含 config 字段）或配置对象；stdin 优先
  -h, --help          显示本帮助
  -V, --version       显示版本

退出码：0 成功 / 2 配置错误 / 4 上游不可用 / 5 内部错误
";

/// 报一个致命错误：发 `error` 消息（宿主可能读不到 stderr）+ 走 stderr + 返回退出码。
fn fatal<W: Write>(sink: &mut Sink<W>, kind: ErrorKind, message: &str) -> ExitCode {
    let retryable = matches!(
        kind,
        ErrorKind::Network | ErrorKind::Upstream | ErrorKind::Internal
    );
    let _ = sink.error(kind, retryable, message);
    eprintln!("viacli-rss: {message}");
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

    // --config 文件既可能是请求（含 config 字段），也可能直接是配置对象
    if req.is_none()
        && let Some(text) = &file_arg
    {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            match serde_json::from_str::<Value>(trimmed) {
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
            eprintln!("viacli-rss: {e}");
            return Ok(ExitCode::from(ErrorKind::Config.exit_code() as u8));
        }
    };
    if args.help {
        eprint!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    if args.version {
        println!("viacli-rss {}", env!("CARGO_PKG_VERSION"));
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
            &format!(
                "协议版本不兼容：请求 protocol={}，本采集器只支持 1",
                req.protocol
            ),
        ));
    }

    // ---- 配置 ----
    // 注意：**不用请求的 `intervalMs` 覆盖 `config.intervalMs`**。
    // 前者是"宿主希望多久产出一次"（属于调度），后者是"采集器允许的最短抓取间隔"
    // （属于对上游客气）。二者混用会让 `{"intervalMs": 1}` 这种坏配置不被发现。
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
    sink.meta("rss", env!("CARGO_PKG_VERSION"))?;

    let stream_mode = req.mode == "stream";
    let host_now = req.now;
    let tag = req.tag();
    let max_lines = req.limits.as_ref().and_then(|l| l.max_lines);
    let max_bytes = req.limits.as_ref().and_then(|l| l.max_output_bytes);
    // stream 的循环间隔取"宿主期望值"与"采集器下限"的较大者：
    // 宿主配 1 秒也只是白打上游，采集器自己兜底。
    let stream_interval = Duration::from_millis(
        req.interval_ms
            .unwrap_or(cfg.interval_ms)
            .max(cfg.interval_ms),
    );

    let mut collector = Collector::new(cfg, State::from_request(req.state.as_ref()));

    if !stream_mode {
        if collector.should_skip(host_now) {
            collector.emit_skipped(&mut sink, host_now)?;
            sink.state(collector.state().to_value())?;
            sink.log(
                "info",
                &format!("{tag}: 距上次抓取不足 intervalMs，本轮跳过网络请求"),
            )?;
            return Ok(ExitCode::SUCCESS);
        }
        let stats = collector.collect(&mut sink, host_now)?;
        if stats.feeds_ok == 0 {
            return Ok(fatal(
                &mut sink,
                ErrorKind::Upstream,
                &format!("{} 个订阅源全部抓取失败", stats.feeds_total),
            ));
        }
        sink.state(collector.state().to_value())?;
        sink.log(
            "info",
            &format!(
                "oneshot 采集完成（{tag}）：成功 {}/{} 个源，新增 {} 条",
                stats.feeds_ok, stats.feeds_total, stats.items_emitted
            ),
        )?;
        return Ok(ExitCode::SUCCESS);
    }

    // ---- stream：常驻，直到 stdin 关闭（§4.2）----
    let (tx, rx) = mpsc::channel::<()>();
    if io::stdin().is_terminal() {
        // 终端交互不会有人关 stdin，直接允许首轮后退出
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
        // 时间戳：以宿主给的 now 为基准 + 单调流逝（§2.5），避免自造时间与长期漂移
        let ts = host_now.map(|base| base + run_start.elapsed().as_millis() as i64);
        let round_start = Instant::now();

        let stats = collector.collect(&mut sink, ts)?;
        sink.state(collector.state().to_value())?;
        if stats.feeds_ok == 0 {
            // 单轮全失败很可能是网络抖动，不结束会话；宿主会靠"活着但没数据"的
            // 监视在连续几轮后重启我们。这里只把原因发清楚。
            sink.error(
                ErrorKind::Upstream,
                true,
                &format!("{} 个订阅源全部抓取失败", stats.feeds_total),
            )?;
        }

        // 自己遵守 limits（§4.5）
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
