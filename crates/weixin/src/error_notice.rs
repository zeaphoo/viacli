//! 错误通知机制

use anyhow::Result;
use colored::Colorize;

/// 错误级别
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorLevel {
    /// 警告
    Warning,
    /// 错误
    Error,
    /// 致命错误
    Fatal,
}

impl ErrorLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorLevel::Warning => "WARNING",
            ErrorLevel::Error => "ERROR",
            ErrorLevel::Fatal => "FATAL",
        }
    }

    pub fn colored(&self, text: &str) -> String {
        match self {
            ErrorLevel::Warning => text.yellow().to_string(),
            ErrorLevel::Error => text.red().to_string(),
            ErrorLevel::Fatal => text.red().bold().to_string(),
        }
    }
}

/// 错误通知
#[derive(Debug, Clone)]
pub struct ErrorNotice {
    /// 级别
    pub level: ErrorLevel,
    /// 标题
    pub title: String,
    /// 消息
    pub message: String,
    /// 详情
    pub details: Option<String>,
    /// 建议
    pub suggestion: Option<String>,
}

/// 发送错误通知
pub fn send_error_notice(notice: &ErrorNotice) {
    let icon = match notice.level {
        ErrorLevel::Warning => "⚠️",
        ErrorLevel::Error => "❌",
        ErrorLevel::Fatal => "💥",
    };

    eprintln!("\n{}", "━".repeat(50).dimmed());
    eprintln!("{} {}", icon, notice.level.colored(&notice.title));
    eprintln!("{}", "━".repeat(50).dimmed());
    eprintln!("{}", notice.message);

    if let Some(ref details) = notice.details {
        eprintln!("\n{}", "详细信息:".dimmed());
        eprintln!("{}", details.dimmed());
    }

    if let Some(ref suggestion) = notice.suggestion {
        eprintln!("\n{}", "建议:".cyan());
        eprintln!("{}", suggestion);
    }

    eprintln!("{}", "━".repeat(50).dimmed());
}

/// 创建警告通知
pub fn warning_notice(title: impl Into<String>, message: impl Into<String>) -> ErrorNotice {
    ErrorNotice {
        level: ErrorLevel::Warning,
        title: title.into(),
        message: message.into(),
        details: None,
        suggestion: None,
    }
}

/// 创建错误通知
pub fn error_notice(
    title: impl Into<String>,
    message: impl Into<String>,
    details: impl Into<String>,
) -> ErrorNotice {
    ErrorNotice {
        level: ErrorLevel::Error,
        title: title.into(),
        message: message.into(),
        details: Some(details.into()),
        suggestion: None,
    }
}

/// 创建致命错误通知
pub fn fatal_notice(
    title: impl Into<String>,
    message: impl Into<String>,
    suggestion: impl Into<String>,
) -> ErrorNotice {
    ErrorNotice {
        level: ErrorLevel::Fatal,
        title: title.into(),
        message: message.into(),
        details: None,
        suggestion: Some(suggestion.into()),
    }
}

/// 处理错误并发送通知
pub fn handle_error<E: std::fmt::Display>(
    result: Result<(), E>,
    level: ErrorLevel,
    title: impl Into<String>,
) {
    if let Err(e) = result {
        let notice = ErrorNotice {
            level,
            title: title.into(),
            message: e.to_string(),
            details: None,
            suggestion: None,
        };
        send_error_notice(&notice);
    }
}

/// 登录错误通知
pub fn login_error_notice(error: &str) -> ErrorNotice {
    ErrorNotice {
        level: ErrorLevel::Error,
        title: "登录失败".to_string(),
        message: error.to_string(),
        details: None,
        suggestion: Some("请检查网络连接和账号配置，然后重试。".to_string()),
    }
}

/// API 错误通知
pub fn api_error_notice(endpoint: &str, error: &str) -> ErrorNotice {
    ErrorNotice {
        level: ErrorLevel::Error,
        title: format!("API 调用失败: {}", endpoint),
        message: error.to_string(),
        details: None,
        suggestion: Some("请检查账号是否已登录，或稍后重试。".to_string()),
    }
}

/// 网络错误通知
pub fn network_error_notice(error: &str) -> ErrorNotice {
    ErrorNotice {
        level: ErrorLevel::Warning,
        title: "网络连接问题".to_string(),
        message: error.to_string(),
        details: None,
        suggestion: Some("请检查网络连接，系统将自动重试。".to_string()),
    }
}

/// 配置错误通知
pub fn config_error_notice(error: &str) -> ErrorNotice {
    ErrorNotice {
        level: ErrorLevel::Error,
        title: "配置错误".to_string(),
        message: error.to_string(),
        details: None,
        suggestion: Some("请检查配置文件或重新登录。".to_string()),
    }
}
