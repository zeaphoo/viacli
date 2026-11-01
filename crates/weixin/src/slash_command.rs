//! 斜杠命令处理

use anyhow::{anyhow, Result};

/// 斜杠命令
#[derive(Debug, Clone)]
pub enum SlashCommand {
    /// 帮助
    Help,
    /// 开始
    Start,
    /// 设置
    Settings,
    /// 取消
    Cancel,
    /// 未知命令
    Unknown(String),
}

/// 解析斜杠命令
pub fn parse_slash_command(text: &str) -> Option<SlashCommand> {
    if !text.starts_with('/') {
        return None;
    }

    let parts: Vec<&str> = text.trim().split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    let cmd = parts[0].to_lowercase();
    let cmd = cmd.as_str();

    match cmd {
        "/help" | "/h" => Some(SlashCommand::Help),
        "/start" => Some(SlashCommand::Start),
        "/settings" | "/setting" => Some(SlashCommand::Settings),
        "/cancel" | "/stop" => Some(SlashCommand::Cancel),
        _ => Some(SlashCommand::Unknown(cmd.to_string())),
    }
}

/// 执行斜杠命令
pub async fn execute_slash_command(cmd: SlashCommand) -> Result<String> {
    match cmd {
        SlashCommand::Help => Ok(
            "可用命令:\n"
                .to_string()
                + "/help - 显示帮助\n"
                + "/start - 开始对话\n"
                + "/settings - 设置\n"
                + "/cancel - 取消当前操作"
        ),
        SlashCommand::Start => Ok("欢迎使用微信机器人! 发送消息开始对话。".to_string()),
        SlashCommand::Settings => Ok("设置功能暂未实现。".to_string()),
        SlashCommand::Cancel => Ok("已取消当前操作。".to_string()),
        SlashCommand::Unknown(cmd) => Err(anyhow!("未知命令: {}", cmd)),
    }
}

/// 检查是否是斜杠命令
pub fn is_slash_command(text: &str) -> bool {
    text.trim().starts_with('/')
}

/// 提取命令名称
pub fn extract_command_name(text: &str) -> Option<String> {
    if !is_slash_command(text) {
        return None;
    }

    text.trim()
        .split_whitespace()
        .next()
        .map(|s| s.to_lowercase())
}

/// 提取命令参数
pub fn extract_command_args(text: &str) -> Vec<String> {
    if !is_slash_command(text) {
        return vec![];
    }

    text.trim()
        .split_whitespace()
        .skip(1)
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_slash_command() {
        assert!(matches!(parse_slash_command("/help"), Some(SlashCommand::Help)));
        assert!(matches!(parse_slash_command("/start"), Some(SlashCommand::Start)));
        assert!(matches!(parse_slash_command("/unknown"), Some(SlashCommand::Unknown(_))));
        assert_eq!(parse_slash_command("hello"), None);
    }

    #[test]
    fn test_is_slash_command() {
        assert!(is_slash_command("/help"));
        assert!(is_slash_command("  /start  "));
        assert!(!is_slash_command("hello"));
    }

    #[test]
    fn test_extract_command_name() {
        assert_eq!(extract_command_name("/help"), Some("/help".to_string()));
        assert_eq!(extract_command_name("/start args"), Some("/start".to_string()));
        assert_eq!(extract_command_name("hello"), None);
    }

    #[test]
    fn test_extract_command_args() {
        assert_eq!(extract_command_args("/help arg1 arg2"), vec!["arg1", "arg2"]);
        assert_eq!(extract_command_args("/start"), vec![]);
    }
}
