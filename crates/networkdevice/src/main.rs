//! 网络设备发现采集器（`networkdevice`）：扫描 IP 网段，识别端口/设备/厂商。
//!
//! 扫描与清点引擎逐字移植自用户自己的 `.links/toposight` 项目（见 `engine/`）；
//! 这一层把它接到 keendeck 的采集器协议上：自管清点数据库 + NDJSON 产出。
#![recursion_limit = "512"]

mod collect;
mod describe;
mod emit;
mod engine;
mod protocol;
mod request;
mod store;

use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::watch;

use crate::collect::{Collector, EmitLimits};
use crate::engine::config::{Config, default_sources};
use crate::engine::discovery::SourceKind;
use crate::protocol::{ErrorKind, Sink};
use crate::request::{Limits, Request};
use serde_json::json;

fn main() -> ExitCode {
    // 手工建 runtime：退出时用 `shutdown_timeout(0)`，**不等仍在阻塞中的扫描任务**
    // （阻断式的 neigh/sweep/probe 可能还要跑好几秒）。进程结束时这些线程随之终止，
    // 而每条协议消息都已 flush、每轮落库都是事务，不会丢数据。
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("viacli-networkdevice: 无法启动异步运行时: {e}");
            return ExitCode::from(ErrorKind::Internal.exit_code() as u8);
        }
    };
    let code = match runtime.block_on(run()) {
        Ok(code) => code,
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("viacli-networkdevice: 内部错误: {e}");
            ExitCode::from(ErrorKind::Internal.exit_code() as u8)
        }
    };
    runtime.shutdown_timeout(Duration::ZERO);
    code
}

#[derive(Debug, Default)]
struct Overrides {
    ifaces: Vec<String>,
    extra_subnets: Vec<String>,
    sources: Option<Vec<String>>,
    intensity: Option<u8>,
    no_mdns: bool,
    no_ssdp: bool,
    no_neigh_sweep: bool,
    no_dns: bool,
    no_port_probe: bool,
    probe_ports: Option<Vec<crate::engine::ports::PortSpec>>,
    offline_after_secs: Option<u64>,
    oui_file: Option<PathBuf>,
    probes_file: Option<PathBuf>,
}

impl Overrides {
    fn apply(&self, cfg: &mut Config) {
        if !self.ifaces.is_empty() {
            cfg.ifaces = self.ifaces.clone();
        }
        if !self.extra_subnets.is_empty() {
            cfg.extra_subnets = self.extra_subnets.clone();
        }
        if let Some(intensity) = self.intensity {
            cfg.intensity = intensity;
        }
        if self.no_neigh_sweep {
            cfg.neigh_sweep = false;
        }
        if self.no_dns {
            cfg.resolve_dns = false;
        }
        if self.no_port_probe {
            cfg.port_probe = false;
        }
        if let Some(ports) = &self.probe_ports {
            cfg.probe_ports = ports.clone();
        }
        if let Some(secs) = self.offline_after_secs {
            cfg.offline_after_secs = Some(secs);
        }
        if let Some(path) = &self.oui_file {
            cfg.oui_file = Some(path.clone());
        }
        if let Some(path) = &self.probes_file {
            cfg.probes_file = Some(path.clone());
        }

        // 显式 sources 或 --no-mdns/--no-ssdp：重写源清单。
        if self.sources.is_some() || self.no_mdns || self.no_ssdp {
            let mut kinds: Vec<SourceKind> = match &self.sources {
                Some(list) => list.iter().filter_map(|s| s.parse().ok()).collect(),
                None => {
                    if cfg.sources.is_empty() {
                        default_sources()
                    } else {
                        cfg.sources.clone()
                    }
                }
            };
            if self.no_mdns {
                kinds.retain(|k| *k != SourceKind::Mdns);
            }
            if self.no_ssdp {
                kinds.retain(|k| *k != SourceKind::Ssdp);
            }
            cfg.sources = kinds;
        }
    }
}

struct Args {
    describe: bool,
    config_path: Option<PathBuf>,
    // 清点数据库位置；缺省按 `store::default_db_path()` 落到 viacli 数据目录。
    db_path: Option<PathBuf>,
    help: bool,
    version: bool,
    overrides: Overrides,
}

fn value(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    it.next().ok_or_else(|| format!("{flag} 需要一个参数值"))
}

fn split_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn parse_ports(raw: &str) -> Vec<crate::engine::ports::PortSpec> {
    use crate::engine::ports::PortSpec;
    split_list(raw)
        .into_iter()
        .map(|t| match t.parse::<u16>() {
            Ok(n) => PortSpec::Number(n),
            Err(_) => PortSpec::Group(t),
        })
        .collect()
}

fn parse_u64(flag: &str, raw: &str) -> Result<u64, String> {
    raw.parse::<u64>()
        .map_err(|_| format!("{flag} 需要一个非负整数，收到 {raw:?}"))
}

fn parse_u8(flag: &str, raw: &str) -> Result<u8, String> {
    raw.parse::<u8>()
        .map_err(|_| format!("{flag} 需要一个 1..=5 的整数，收到 {raw:?}"))
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        describe: false,
        config_path: None,
        db_path: None,
        help: false,
        version: false,
        overrides: Overrides::default(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let o = &mut args.overrides;
        match a.as_str() {
            "--describe" => args.describe = true,
            "--help" | "-h" => args.help = true,
            "--version" | "-V" => args.version = true,
            "--config" => args.config_path = Some(PathBuf::from(value(&mut it, "--config")?)),
            "--db" => args.db_path = Some(PathBuf::from(value(&mut it, "--db")?)),
            "--ifaces" => o.ifaces.extend(split_list(&value(&mut it, "--ifaces")?)),
            "--extra-subnets" => o
                .extra_subnets
                .extend(split_list(&value(&mut it, "--extra-subnets")?)),
            "--sources" => {
                o.sources = Some(split_list(&value(&mut it, "--sources")?));
            }
            "--intensity" => {
                o.intensity = Some(parse_u8("--intensity", &value(&mut it, "--intensity")?)?)
            }
            "--probe-ports" => {
                o.probe_ports = Some(parse_ports(&value(&mut it, "--probe-ports")?))
            }
            "--offline-after-secs" => {
                o.offline_after_secs = Some(parse_u64(
                    "--offline-after-secs",
                    &value(&mut it, "--offline-after-secs")?,
                )?)
            }
            "--oui-file" => {
                o.oui_file = Some(PathBuf::from(value(&mut it, "--oui-file")?))
            }
            "--probes-file" => {
                o.probes_file = Some(PathBuf::from(value(&mut it, "--probes-file")?))
            }
            "--no-mdns" => o.no_mdns = true,
            "--no-ssdp" => o.no_ssdp = true,
            "--no-neigh-sweep" => o.no_neigh_sweep = true,
            "--no-dns" => o.no_dns = true,
            "--no-port-probe" => o.no_port_probe = true,
            other => return Err(format!("未知参数 {other:?}（用 --help 查看用法）")),
        }
    }
    Ok(args)
}

const USAGE: &str = "\
viacli-networkdevice —— 网络设备发现采集器（遵循 docs/07-collector-protocol.md）

用法：
  viacli-networkdevice --describe       输出自描述 JSON
  viacli-networkdevice [选项]           从 stdin 读一行 JSON 请求后开始采集

来源与目标：
  --config <path>           从文件读取请求（含 config 字段）或配置对象；stdin 优先
  --db <path>               清点数据库位置（默认 $VIACLI_DATA_ROOT/networkdevice/<数据源名>.db）
  --ifaces <列表>           扫描网卡，逗号分隔；留空 = 默认路由那张
  --extra-subnets <列表>    L3 sweep 的网段（可跨路由器），逗号分隔
  --sources <列表>          启用的发现源：neigh,mdns,ssdp,sweep,probe

频率与探测：
  --intensity <1-5>         扫描强度（统一缩放各源周期，默认 3）
  --probe-ports <列表>      端口探测清单（数字或分组 common/web/db）
  --no-port-probe           只被动识别（反向 DNS + 厂商），不主动探测端口
  --no-dns                  不做反向 DNS
  --no-neigh-sweep          不先 TCP 预扫填充 ARP
  --no-mdns / --no-ssdp     关闭对应发现源
  --offline-after-secs <s>  多久没见到判离线
  --oui-file <path>         自定义 IEEE OUI CSV
  --probes-file <path>      自定义 nmap-service-probes

其它：
  -h, --help                显示本帮助
  -V, --version             显示版本

说明：
  本采集器自己维护一个 SQLite 清点库（默认在 viacli 数据目录的
  networkdevice/ 下，文件名取自数据源名，可用 --db 指定），用于增量扫描、差分与设备归并。
  它只把结果以协议消息产出，绝不写宿主的数据库。

退出码：0 成功 / 2 配置错误 / 4 上游不可用 / 5 内部错误
";

fn fatal<W: Write>(sink: &mut Sink<W>, kind: ErrorKind, message: &str) -> ExitCode {
    let retryable = matches!(
        kind,
        ErrorKind::Network | ErrorKind::Upstream | ErrorKind::Internal
    );
    let _ = sink.error(kind, retryable, message);
    eprintln!("viacli-networkdevice: {message}");
    ExitCode::from(kind.exit_code() as u8)
}

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

    if req.is_none()
        && let Some(text) = &file_arg
    {
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
                            datasource: None,
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

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

async fn run() -> io::Result<ExitCode> {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("viacli-networkdevice: {e}");
            return Ok(ExitCode::from(ErrorKind::Config.exit_code() as u8));
        }
    };
    if args.help {
        eprint!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    if args.version {
        println!("viacli-networkdevice {}", env!("CARGO_PKG_VERSION"));
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

    let mut cfg = req.config.clone().unwrap_or_default();
    args.overrides.apply(&mut cfg);
    if !matches!(req.mode.as_str(), "oneshot" | "stream") {
        return Ok(fatal(
            &mut sink,
            ErrorKind::Config,
            &format!("未知 mode {:?}；可用值：oneshot, stream", req.mode),
        ));
    }

    let datasource = req.datasource();
    sink.meta("networkdevice", env!("CARGO_PKG_VERSION"))?;

    let db_path = args
        .db_path
        .clone()
        .unwrap_or_else(|| store::default_db_path(&datasource));
    let mut collector = match Collector::new(cfg, datasource.clone(), &db_path) {
        Ok(c) => c,
        Err(e) => {
            return Ok(fatal(
                &mut sink,
                ErrorKind::Internal,
                &format!("初始化清点数据库失败: {e}"),
            ));
        }
    };

    let limits = EmitLimits {
        max_lines: req.limits.as_ref().and_then(|l| l.max_lines),
        max_bytes: req.limits.as_ref().and_then(|l| l.max_output_bytes),
    };
    let host_now = req.now;
    let tag = req.tag();
    let limits_cfg: Option<Limits> = req.limits.clone();

    if req.mode == "oneshot" {
        return run_oneshot(&mut collector, &mut sink, limits_cfg, host_now, limits, &tag).await;
    }
    run_stream(&mut collector, &mut sink, host_now, limits, &tag).await
}

async fn run_oneshot<W: Write>(
    collector: &mut Collector,
    sink: &mut Sink<W>,
    limits_cfg: Option<Limits>,
    host_now: Option<i64>,
    limits: EmitLimits,
    tag: &str,
) -> io::Result<ExitCode> {
    let now = host_now.unwrap_or_else(now_ms);
    // 宿主给了硬超时就按它留 3s 收尾；没有（如试采集/一致性检查）就给一个 4s 的
    // 保守预算——宁可这一轮少扫一点，也不要被外部超时杀掉（那会被判为失败）。
    let budget = limits_cfg
        .as_ref()
        .and_then(|l| l.timeout_ms)
        .map(|t| t.saturating_sub(3_000).max(1_000))
        .unwrap_or(4_000);
    let deadline = Instant::now() + Duration::from_millis(budget);

    let stats = collector
        .run_cycle(sink, now, Some(deadline), limits)
        .await?;
    if stats.stopped {
        return Ok(ExitCode::SUCCESS);
    }
    sink.state(json!({
        "lastCycleMs": collector.last_cycle_ms(),
        "database": collector.db_path(),
    }))?;
    sink.log(
        "info",
        &format!(
            "{tag}: oneshot 一轮完成：运行源 {}、观测 {}、端点快照 {}、设备快照 {}、事件 {}",
            stats.sources_run,
            stats.observations,
            stats.endpoints_emitted,
            stats.devices_emitted,
            stats.events
        ),
    )?;
    Ok(ExitCode::SUCCESS)
}

async fn run_stream<W: Write>(
    collector: &mut Collector,
    sink: &mut Sink<W>,
    host_now: Option<i64>,
    limits: EmitLimits,
    tag: &str,
) -> io::Result<ExitCode> {
    let tick = Duration::from_millis(collector.tick_ms());

    let stopped = Arc::new(AtomicBool::new(false));
    collector.set_stop(Arc::clone(&stopped));
    let (stop_tx, mut stop_rx) = watch::channel(false);
    if io::stdin().is_terminal() {
        stopped.store(true, Ordering::SeqCst);
        let _ = stop_tx.send(true);
    } else {
        let stopped = Arc::clone(&stopped);
        std::thread::spawn(move || {
            let mut stdin = io::stdin();
            let mut buf = [0u8; 512];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => continue,
                }
            }
            stopped.store(true, Ordering::SeqCst);
            let _ = stop_tx.send(true);
        });
    }

    let run_start = Instant::now();
    loop {
        if *stop_rx.borrow() {
            break;
        }
        let now = host_now
            .map(|base| base + run_start.elapsed().as_millis() as i64)
            .unwrap_or_else(now_ms);
        let round_start = Instant::now();

        let stats = collector.run_cycle(sink, now, None, limits).await?;
        if stats.stopped || limits.hit(sink) {
            return Ok(ExitCode::SUCCESS);
        }
        sink.state(json!({
            "lastCycleMs": collector.last_cycle_ms(),
            "database": collector.db_path(),
        }))?;

        let wait = tick.saturating_sub(round_start.elapsed());
        let wait = if wait.is_zero() {
            Duration::from_millis(1)
        } else {
            wait
        };
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = stop_rx.changed() => break,
        }
    }

    sink.log("info", &format!("{tag}: stdin 已关闭，正常退出"))?;
    Ok(ExitCode::SUCCESS)
}
