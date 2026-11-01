//! 调试模式

use colored::Colorize;
use serde_json::Value;

/// 调试配置
#[derive(Debug, Clone)]
pub struct DebugConfig {
    /// 启用调试输出
    pub enabled: bool,
    /// 显示原始请求/响应
    pub show_raw_traffic: bool,
    /// 显示详细消息内容
    pub show_message_details: bool,
    /// 显示时间戳
    pub show_timestamps: bool,
    /// 显示堆栈跟踪
    pub show_backtrace: bool,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            show_raw_traffic: true,
            show_message_details: true,
            show_timestamps: true,
            show_backtrace: false,
        }
    }
}

impl DebugConfig {
    /// 创建新的调试配置
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            ..Default::default()
        }
    }

    /// 启用所有调试选项
    pub fn all() -> Self {
        Self {
            enabled: true,
            show_raw_traffic: true,
            show_message_details: true,
            show_timestamps: true,
            show_backtrace: true,
        }
    }
}

use std::sync::OnceLock;

static DEBUG_CONFIG: OnceLock<DebugConfig> = OnceLock::new();

/// 初始化调试配置
pub fn init_debug(config: DebugConfig) {
    let _ = DEBUG_CONFIG.set(config);
}

/// 获取调试配置
pub fn debug_config() -> &'static DebugConfig {
    DEBUG_CONFIG.get_or_init(DebugConfig::default)
}

/// 检查是否启用调试
pub fn is_debug_enabled() -> bool {
    debug_config().enabled
}

/// 调试打印
#[macro_export]
macro_rules! debug_print {
    ($($arg:tt)*) => {
        if $crate::debug::is_debug_enabled() {
            eprintln!($($arg)*);
        }
    };
}

/// 打印调试头部
pub fn print_debug_header(title: &str) {
    if !is_debug_enabled() {
        return;
    }
    eprintln!("\n{}", "═".repeat(60).dimmed());
    eprintln!("{} {}", "🔍 DEBUG:".cyan(), title.cyan().bold());
    eprintln!("{}", "═".repeat(60).dimmed());
}

/// 打印调试尾部
pub fn print_debug_footer() {
    if !is_debug_enabled() {
        return;
    }
    eprintln!("{}", "═".repeat(60).dimmed());
}

/// 打印调试信息
pub fn debug_info(label: &str, value: impl AsRef<str>) {
    if !is_debug_enabled() {
        return;
    }
    let timestamp = if debug_config().show_timestamps {
        format!("[{}] ", chrono::Local::now().format("%H:%M:%S%.3f"))
    } else {
        String::new()
    };
    eprintln!("{}{}: {}", timestamp.dimmed(), label.cyan(), value.as_ref());
}

/// 打印 JSON 数据
pub fn debug_json(label: &str, value: &Value) {
    if !is_debug_enabled() {
        return;
    }
    let timestamp = if debug_config().show_timestamps {
        format!("[{}] ", chrono::Local::now().format("%H:%M:%S%.3f"))
    } else {
        String::new()
    };
    let json_str = serde_json::to_string_pretty(value).unwrap_or_default();
    eprintln!("{}{}:", timestamp.dimmed(), label.cyan());
    for line in json_str.lines() {
        eprintln!("  {}", line.dimmed());
    }
}

/// 打印原始请求
pub fn debug_request(method: &str, url: &str, body: Option<&Value>) {
    if !is_debug_enabled() || !debug_config().show_raw_traffic {
        return;
    }
    print_debug_header("HTTP Request");
    debug_info("Method", method);
    debug_info("URL", url);
    if let Some(body) = body {
        debug_json("Body", body);
    }
    print_debug_footer();
}

/// 打印原始响应
pub fn debug_response(status: u16, body: &Value) {
    if !is_debug_enabled() || !debug_config().show_raw_traffic {
        return;
    }
    print_debug_header("HTTP Response");
    debug_info("Status", status.to_string());
    debug_json("Body", body);
    print_debug_footer();
}

/// 打印消息详情
pub fn debug_message(msg: &crate::types::WeixinMessage) {
    if !is_debug_enabled() || !debug_config().show_message_details {
        return;
    }
    print_debug_header("Message Details");
    
    if let Some(ref id) = msg.message_id {
        debug_info("Message ID", id.to_string());
    }
    if let Some(ref from) = msg.from_user_id {
        debug_info("From", from);
    }
    if let Some(ref to) = msg.to_user_id {
        debug_info("To", to);
    }
    if let Some(ref client_id) = msg.client_id {
        debug_info("Client ID", client_id);
    }
    if let Some(time_ms) = msg.create_time_ms {
        let dt = chrono::DateTime::from_timestamp_millis(time_ms);
        if let Some(dt) = dt {
            debug_info("Timestamp", dt.format("%Y-%m-%d %H:%M:%S").to_string());
        }
    }
    if let Some(ref token) = msg.context_token {
        debug_info("Context Token", format!("{}...", &token[..token.len().min(20)]));
    }
    
    if let Some(ref items) = msg.item_list {
        debug_info("Item Count", items.len().to_string());
        for (i, item) in items.iter().enumerate() {
            if let Some(t) = item.r#type {
                let type_name = match t {
                    1 => "Text",
                    2 => "Image",
                    3 => "Voice",
                    4 => "File",
                    5 => "Video",
                    _ => "Unknown",
                };
                debug_info(&format!("  Item {}", i), type_name);
            }
        }
    }
    
    print_debug_footer();
}

/// 打印错误详情
pub fn debug_error(error: &anyhow::Error) {
    if !is_debug_enabled() {
        return;
    }
    print_debug_header("Error Details");
    eprintln!("{}", error.to_string().red());
    
    if debug_config().show_backtrace {
        let backtrace = std::backtrace::Backtrace::capture();
    if backtrace.status() == std::backtrace::BacktraceStatus::Captured {
        eprintln!("\n{}", "Backtrace:".dimmed());
        eprintln!("{}", backtrace);
    }
    }
    
    print_debug_footer();
}

/// 打印性能计时
pub struct DebugTimer {
    label: String,
    start: std::time::Instant,
}

impl DebugTimer {
    /// 创建新的计时器
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            start: std::time::Instant::now(),
        }
    }

    /// 停止计时并打印结果
    pub fn stop(self) {
        if !is_debug_enabled() {
            return;
        }
        let elapsed = self.start.elapsed();
        debug_info(&format!("⏱️  {}", self.label), format!("{:?}", elapsed));
    }
}

impl Drop for DebugTimer {
    fn drop(&mut self) {
        if is_debug_enabled() {
            let elapsed = self.start.elapsed();
            debug_info(&format!("⏱️  {}", self.label), format!("{:?}", elapsed));
        }
    }
}
