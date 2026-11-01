//! 日志系统

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use chrono::Local;

/// 日志级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

impl LogLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }
}

/// 日志记录器
pub struct Logger {
    level: LogLevel,
    log_file: Option<Mutex<std::fs::File>>,
    enable_console: bool,
}

impl Logger {
    /// 创建新的日志记录器
    pub fn new(level: LogLevel) -> Self {
        Self {
            level,
            log_file: None,
            enable_console: true,
        }
    }

    /// 设置日志文件
    pub fn with_log_file(mut self, path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        self.log_file = Some(Mutex::new(file));
        Ok(self)
    }

    /// 禁用控制台输出
    pub fn disable_console(mut self) -> Self {
        self.enable_console = false;
        self
    }

    /// 记录日志
    fn log(&self, level: LogLevel, message: &str) {
        if level < self.level {
            return;
        }

        let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let log_line = format!("[{}] [{}] {}\n", timestamp, level.as_str(), message);

        // 写入文件
        if let Some(ref file) = self.log_file {
            if let Ok(mut f) = file.lock() {
                let _ = f.write_all(log_line.as_bytes());
                let _ = f.flush();
            }
        }

        // 输出到控制台
        if self.enable_console {
            use colored::Colorize;
            let colored_line = match level {
                LogLevel::Debug => log_line.dimmed(),
                LogLevel::Info => log_line.normal(),
                LogLevel::Warn => log_line.yellow(),
                LogLevel::Error => log_line.red(),
            };
            eprint!("{}", colored_line);
        }
    }

    pub fn debug(&self, message: impl AsRef<str>) {
        self.log(LogLevel::Debug, message.as_ref());
    }

    pub fn info(&self, message: impl AsRef<str>) {
        self.log(LogLevel::Info, message.as_ref());
    }

    pub fn warn(&self, message: impl AsRef<str>) {
        self.log(LogLevel::Warn, message.as_ref());
    }

    pub fn error(&self, message: impl AsRef<str>) {
        self.log(LogLevel::Error, message.as_ref());
    }
}

use std::sync::OnceLock;

static GLOBAL_LOGGER: OnceLock<Logger> = OnceLock::new();

/// 初始化全局日志记录器
pub fn init_logger(level: LogLevel, log_file: Option<PathBuf>) -> anyhow::Result<()> {
    let mut logger = Logger::new(level);
    
    if let Some(path) = log_file {
        logger = logger.with_log_file(path)?;
    }
    
    GLOBAL_LOGGER.set(logger).map_err(|_| anyhow::anyhow!("Logger already initialized"))?;
    Ok(())
}

/// 获取全局日志记录器
pub fn logger() -> &'static Logger {
    GLOBAL_LOGGER.get_or_init(|| Logger::new(LogLevel::Info))
}

/// 调试日志
#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        $crate::logger::logger().debug(format!($($arg)*))
    };
}

/// 信息日志
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        $crate::logger::logger().info(format!($($arg)*))
    };
}

/// 警告日志
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        $crate::logger::logger().warn(format!($($arg)*))
    };
}

/// 错误日志
#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        $crate::logger::logger().error(format!($($arg)*))
    };
}
