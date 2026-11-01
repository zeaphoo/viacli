//! QQ Bot CLI 命令实现

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand, ValueEnum};
use colored::Colorize;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::client::QQBotClient;
use crate::config::{ConfigManager, parse_token};
use crate::gateway::{GatewayEvent, start_gateway};
use crate::message::{format_message, MessageSender};
use crate::types::*;

/// 输出格式
#[derive(ValueEnum, Clone, Debug)]
pub enum OutputFormat {
    /// 文本格式输出
    Text,
    /// JSON 格式输出
    Json,
}

/// QQ Bot CLI 命令
#[derive(Parser)]
#[command(name = "qqbot")]
#[command(about = "QQ Bot API 命令行工具")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// 输出格式
    #[arg(long, value_enum, global = true, default_value = "text")]
    pub outformat: OutputFormat,

    /// 账号 ID
    #[arg(short, long, global = true)]
    pub account: Option<String>,
}

/// 子命令
#[derive(Subcommand)]
pub enum Commands {
    /// 配置账号
    Config {
        /// App ID
        #[arg(short, long)]
        app_id: Option<String>,

        /// Client Secret
        #[arg(short, long)]
        secret: Option<String>,

        /// Token (格式: app_id:client_secret)
        #[arg(short, long)]
        token: Option<String>,

        /// 账号名称
        #[arg(short, long)]
        name: Option<String>,

        /// 使用环境变量 (QQBOT_APP_ID, QQBOT_CLIENT_SECRET)
        #[arg(long)]
        use_env: bool,
    },

    /// 列出已配置账号
    List,

    /// 删除账号配置
    Remove {
        /// 账号 ID
        account_id: String,
    },

    /// 发送消息
    Send {
        /// 目标 ID (C2C: openid, 群聊: group_openid)
        to: String,

        /// 消息内容
        text: String,

        /// 目标类型
        #[arg(short, long, value_enum, default_value = "c2c")]
        target_type: TargetTypeArg,

        /// 回复的消息 ID
        #[arg(short, long)]
        reply_to: Option<String>,
    },

    /// 发送 Markdown 消息
    SendMarkdown {
        /// 目标 ID
        to: String,

        /// Markdown 内容
        text: String,

        /// 目标类型
        #[arg(short, long, value_enum, default_value = "c2c")]
        target_type: TargetTypeArg,
    },

    /// 发送图片
    SendImage {
        /// 目标 ID
        to: String,

        /// 图片路径或 URL
        image: String,

        /// 目标类型
        #[arg(short, long, value_enum, default_value = "c2c")]
        target_type: TargetTypeArg,

        /// 图片说明
        #[arg(short, long)]
        caption: Option<String>,
    },

    /// 发送文件
    SendFile {
        /// 目标 ID
        to: String,

        /// 文件路径
        file: String,

        /// 目标类型
        #[arg(short, long, value_enum, default_value = "c2c")]
        target_type: TargetTypeArg,
    },

    /// 发送输入状态
    Typing {
        /// 目标 ID
        to: String,

        /// 输入状态持续时间（秒）
        #[arg(short, long, default_value = "60")]
        seconds: i32,
    },

    /// 获取 Gateway URL
    Gateway,

    /// 监控模式（接收消息）
    Monitor {
        /// 自动回复内容（可选）
        #[arg(short, long)]
        auto_reply: Option<String>,
    },

    /// 测试连接
    Ping,
}

/// 目标类型参数
#[derive(ValueEnum, Clone, Debug)]
pub enum TargetTypeArg {
    C2C,
    Group,
    Channel,
}

impl From<TargetTypeArg> for TargetType {
    fn from(arg: TargetTypeArg) -> Self {
        match arg {
            TargetTypeArg::C2C => TargetType::C2C,
            TargetTypeArg::Group => TargetType::Group,
            TargetTypeArg::Channel => TargetType::Channel,
        }
    }
}

/// 运行 CLI
pub async fn run(cli: Cli) -> Result<()> {
    let config_manager = ConfigManager::new()?;

    match cli.command {
        Commands::Config { app_id, secret, token, name, use_env } => {
            cmd_config(&config_manager, app_id, secret, token, name, use_env, &cli.outformat).await?;
        }
        Commands::List => {
            cmd_list(&config_manager, &cli.outformat)?;
        }
        Commands::Remove { account_id } => {
            cmd_remove(&config_manager, &account_id, &cli.outformat)?;
        }
        Commands::Send { to, text, target_type, reply_to } => {
            let account_id = cli.account.ok_or_else(|| anyhow!("请指定账号 ID (--account)"))?;
            cmd_send(&config_manager, &account_id, &to, &text, target_type.into(), reply_to, &cli.outformat).await?;
        }
        Commands::SendMarkdown { to, text, target_type } => {
            let account_id = cli.account.ok_or_else(|| anyhow!("请指定账号 ID (--account)"))?;
            cmd_send_markdown(&config_manager, &account_id, &to, &text, target_type.into(), &cli.outformat).await?;
        }
        Commands::SendImage { to, image, target_type, caption } => {
            let account_id = cli.account.ok_or_else(|| anyhow!("请指定账号 ID (--account)"))?;
            cmd_send_image(&config_manager, &account_id, &to, &image, target_type.into(), caption, &cli.outformat).await?;
        }
        Commands::SendFile { to, file, target_type } => {
            let account_id = cli.account.ok_or_else(|| anyhow!("请指定账号 ID (--account)"))?;
            cmd_send_file(&config_manager, &account_id, &to, &file, target_type.into(), &cli.outformat).await?;
        }
        Commands::Typing { to, seconds } => {
            let account_id = cli.account.ok_or_else(|| anyhow!("请指定账号 ID (--account)"))?;
            cmd_typing(&config_manager, &account_id, &to, seconds, &cli.outformat).await?;
        }
        Commands::Gateway => {
            let account_id = cli.account.ok_or_else(|| anyhow!("请指定账号 ID (--account)"))?;
            cmd_gateway(&config_manager, &account_id, &cli.outformat).await?;
        }
        Commands::Monitor { auto_reply } => {
            let account_id = cli.account.ok_or_else(|| anyhow!("请指定账号 ID (--account)"))?;
            cmd_monitor(&config_manager, &account_id, auto_reply, &cli.outformat).await?;
        }
        Commands::Ping => {
            let account_id = cli.account.ok_or_else(|| anyhow!("请指定账号 ID (--account)"))?;
            cmd_ping(&config_manager, &account_id, &cli.outformat).await?;
        }
    }

    Ok(())
}

/// 配置命令
async fn cmd_config(
    config_manager: &ConfigManager,
    app_id: Option<String>,
    secret: Option<String>,
    token: Option<String>,
    name: Option<String>,
    use_env: bool,
    outformat: &OutputFormat,
) -> Result<()> {
    let (final_app_id, final_secret, account_id) = if use_env {
        let app_id = std::env::var("QQBOT_APP_ID")
            .map_err(|_| anyhow!("环境变量 QQBOT_APP_ID 未设置"))?;
        let secret = std::env::var("QQBOT_CLIENT_SECRET")
            .map_err(|_| anyhow!("环境变量 QQBOT_CLIENT_SECRET 未设置"))?;
        let account_id = std::env::var("QQBOT_ACCOUNT_ID").unwrap_or_else(|_| "default".to_string());
        (app_id, secret, account_id)
    } else if let Some(token) = token {
        let (app_id, secret) = parse_token(&token)
            .ok_or_else(|| anyhow!("Token 格式错误，应为: app_id:client_secret"))?;
        let account_id = app_id.clone();
        (app_id, secret, account_id)
    } else if let (Some(app_id), Some(secret)) = (app_id, secret) {
        let account_id = app_id.clone();
        (app_id, secret, account_id)
    } else {
        return Err(anyhow!("请提供 --token 或 (--app-id 和 --secret) 或 --use-env"));
    };

    let config = crate::config::QQBotConfig {
        app_id: final_app_id.clone(),
        client_secret: final_secret,
        name: name.clone(),
        enabled: true,
    };

    config_manager.save_config(&account_id, &config)?;

    match outformat {
        OutputFormat::Json => {
            let output = json!({
                "success": true,
                "account_id": account_id,
                "app_id": final_app_id,
                "name": name,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Text => {
            println!("{} 账号配置已保存", "✓".green());
            println!("  账号 ID: {}", account_id.cyan());
            println!("  App ID: {}", final_app_id.cyan());
            if let Some(name) = name {
                println!("  名称: {}", name.cyan());
            }
        }
    }

    Ok(())
}

/// 列出账号命令
fn cmd_list(config_manager: &ConfigManager, outformat: &OutputFormat) -> Result<()> {
    let accounts = config_manager.list_accounts();

    match outformat {
        OutputFormat::Json => {
            let accounts_json: Vec<_> = accounts
                .iter()
                .map(|(id, config)| {
                    json!({
                        "account_id": id,
                        "app_id": config.app_id,
                        "name": config.name,
                        "enabled": config.enabled,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&json!({ "accounts": accounts_json }))?);
        }
        OutputFormat::Text => {
            if accounts.is_empty() {
                println!("{} 没有已配置的账号", "ℹ".cyan());
                println!("使用 `viacli qqbot config --token <app_id:secret>` 配置账号");
            } else {
                println!("\n{}", "已配置的账号:".cyan().bold());
                println!("{}", "━".repeat(50).dimmed());
                for (id, config) in &accounts {
                    let status = if config.enabled {
                        "✓ 启用".green()
                    } else {
                        "✗ 禁用".red()
                    };
                    println!("{} {} - {}", "👤".cyan(), id.cyan(), status);
                    println!("   App ID: {}", config.app_id.dimmed());
                    if let Some(ref name) = config.name {
                        println!("   名称: {}", name.dimmed());
                    }
                }
                println!("{}", "━".repeat(50).dimmed());
            }
        }
    }

    Ok(())
}

/// 删除账号命令
fn cmd_remove(config_manager: &ConfigManager, account_id: &str, outformat: &OutputFormat) -> Result<()> {
    config_manager.delete_config(account_id)?;

    match outformat {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&json!({ "success": true }))?);
        }
        OutputFormat::Text => {
            println!("{} 账号 {} 已删除", "✓".green(), account_id.cyan());
        }
    }

    Ok(())
}

/// 发送消息命令
async fn cmd_send(
    config_manager: &ConfigManager,
    account_id: &str,
    to: &str,
    text: &str,
    target_type: TargetType,
    reply_to: Option<String>,
    outformat: &OutputFormat,
) -> Result<()> {
    let config = config_manager
        .load_config(account_id)
        .ok_or_else(|| anyhow!("账号不存在: {}", account_id))?;

    let client = QQBotClient::new(&config.app_id, &config.client_secret);

    let options = SendOptions {
        reply_to,
        msg_seq: Some(1),
        message_reference: None,
    };

    let result = match target_type {
        TargetType::C2C => {
            client.api().send_c2c_message(to, text, options).await
        }
        TargetType::Group => {
            client.api().send_group_message(to, text, options).await
        }
        TargetType::Channel => {
            client.api().send_channel_message(to, text, options).await
        }
    };

    match result {
        Ok(resp) => {
            match outformat {
                OutputFormat::Json => {
                    let output = json!({
                        "success": true,
                        "message_id": resp.id,
                        "timestamp": resp.timestamp,
                    });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                }
                OutputFormat::Text => {
                    println!("{} 消息已发送", "✓".green());
                    println!("  消息 ID: {}", resp.id.cyan());
                    println!("  时间戳: {}", resp.timestamp.dimmed());
                }
            }
        }
        Err(e) => {
            match outformat {
                OutputFormat::Json => {
                    let output = json!({
                        "success": false,
                        "error": e.to_string(),
                    });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                }
                OutputFormat::Text => {
                    println!("{} 发送失败: {}", "✗".red(), e);
                }
            }
        }
    }

    Ok(())
}

/// 发送 Markdown 消息命令
async fn cmd_send_markdown(
    config_manager: &ConfigManager,
    account_id: &str,
    to: &str,
    text: &str,
    target_type: TargetType,
    outformat: &OutputFormat,
) -> Result<()> {
    let config = config_manager
        .load_config(account_id)
        .ok_or_else(|| anyhow!("账号不存在: {}", account_id))?;

    let client = QQBotClient::new(&config.app_id, &config.client_secret);

    let options = SendOptions::default();

    let result = match target_type {
        TargetType::C2C => {
            client.api().send_c2c_markdown(to, text, options).await
        }
        TargetType::Group => {
            client.api().send_group_markdown(to, text, options).await
        }
        TargetType::Channel => {
            client.api().send_channel_message(to, text, options).await
        }
    };

    match result {
        Ok(resp) => {
            match outformat {
                OutputFormat::Json => {
                    let output = json!({
                        "success": true,
                        "message_id": resp.id,
                        "timestamp": resp.timestamp,
                    });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                }
                OutputFormat::Text => {
                    println!("{} Markdown 消息已发送", "✓".green());
                    println!("  消息 ID: {}", resp.id.cyan());
                }
            }
        }
        Err(e) => {
            match outformat {
                OutputFormat::Json => {
                    let output = json!({
                        "success": false,
                        "error": e.to_string(),
                    });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                }
                OutputFormat::Text => {
                    println!("{} 发送失败: {}", "✗".red(), e);
                }
            }
        }
    }

    Ok(())
}

/// 发送图片命令
async fn cmd_send_image(
    config_manager: &ConfigManager,
    account_id: &str,
    to: &str,
    image: &str,
    target_type: TargetType,
    caption: Option<String>,
    outformat: &OutputFormat,
) -> Result<()> {
    let config = config_manager
        .load_config(account_id)
        .ok_or_else(|| anyhow!("账号不存在: {}", account_id))?;

    let client = QQBotClient::new(&config.app_id, &config.client_secret);

    if !outformat.is_json() {
        println!("{} 正在发送图片...", "⏳".cyan());
    }

    let result = client.send_image(to, image, target_type, caption.as_deref()).await;

    match result {
        Ok(resp) => {
            match outformat {
                OutputFormat::Json => {
                    let output = json!({
                        "success": true,
                        "message_id": resp.id,
                        "timestamp": resp.timestamp,
                    });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                }
                OutputFormat::Text => {
                    println!("{} 图片已发送", "✓".green());
                    println!("  消息 ID: {}", resp.id.cyan());
                }
            }
        }
        Err(e) => {
            match outformat {
                OutputFormat::Json => {
                    let output = json!({
                        "success": false,
                        "error": e.to_string(),
                    });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                }
                OutputFormat::Text => {
                    println!("{} 发送失败: {}", "✗".red(), e);
                }
            }
        }
    }

    Ok(())
}

/// 发送文件命令
async fn cmd_send_file(
    config_manager: &ConfigManager,
    account_id: &str,
    to: &str,
    file: &str,
    target_type: TargetType,
    outformat: &OutputFormat,
) -> Result<()> {
    let config = config_manager
        .load_config(account_id)
        .ok_or_else(|| anyhow!("账号不存在: {}", account_id))?;

    let client = QQBotClient::new(&config.app_id, &config.client_secret);
    let sender = client.sender();

    if !outformat.is_json() {
        println!("{} 正在发送文件...", "⏳".cyan());
    }

    let result = sender.send_file(to, file, target_type).await;

    match result {
        Ok(send_result) => {
            if send_result.success {
                match outformat {
                    OutputFormat::Json => {
                        let output = json!({
                            "success": true,
                            "message_id": send_result.message_id,
                        });
                        println!("{}", serde_json::to_string_pretty(&output)?);
                    }
                    OutputFormat::Text => {
                        println!("{} 文件已发送", "✓".green());
                        if let Some(id) = send_result.message_id {
                            println!("  消息 ID: {}", id.cyan());
                        }
                    }
                }
            } else {
                match outformat {
                    OutputFormat::Json => {
                        let output = json!({
                            "success": false,
                            "error": send_result.error,
                        });
                        println!("{}", serde_json::to_string_pretty(&output)?);
                    }
                    OutputFormat::Text => {
                        println!("{} 发送失败: {}", "✗".red(), send_result.error.unwrap_or_default());
                    }
                }
            }
        }
        Err(e) => {
            match outformat {
                OutputFormat::Json => {
                    let output = json!({
                        "success": false,
                        "error": e.to_string(),
                    });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                }
                OutputFormat::Text => {
                    println!("{} 发送失败: {}", "✗".red(), e);
                }
            }
        }
    }

    Ok(())
}

/// 发送输入状态命令
async fn cmd_typing(
    config_manager: &ConfigManager,
    account_id: &str,
    to: &str,
    seconds: i32,
    outformat: &OutputFormat,
) -> Result<()> {
    let config = config_manager
        .load_config(account_id)
        .ok_or_else(|| anyhow!("账号不存在: {}", account_id))?;

    let client = QQBotClient::new(&config.app_id, &config.client_secret);

    match client.send_typing(to, seconds).await {
        Ok(_) => {
            match outformat {
                OutputFormat::Json => {
                    let output = json!({
                        "success": true,
                        "target": to,
                        "seconds": seconds,
                    });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                }
                OutputFormat::Text => {
                    println!("{} 已发送输入状态到 {}", "✓".green(), to.cyan());
                    println!("  持续时间: {}秒", seconds.to_string().dimmed());
                }
            }
        }
        Err(e) => {
            match outformat {
                OutputFormat::Json => {
                    let output = json!({
                        "success": false,
                        "error": e.to_string(),
                    });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                }
                OutputFormat::Text => {
                    println!("{} 发送失败: {}", "✗".red(), e);
                }
            }
        }
    }

    Ok(())
}

/// 获取 Gateway URL 命令
async fn cmd_gateway(
    config_manager: &ConfigManager,
    account_id: &str,
    outformat: &OutputFormat,
) -> Result<()> {
    let config = config_manager
        .load_config(account_id)
        .ok_or_else(|| anyhow!("账号不存在: {}", account_id))?;

    let client = QQBotClient::new(&config.app_id, &config.client_secret);

    let url = client.get_gateway_url().await?;

    match outformat {
        OutputFormat::Json => {
            let output = json!({
                "success": true,
                "gateway_url": url,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Text => {
            println!("{} Gateway URL: {}", "✓".green(), url.cyan());
        }
    }

    Ok(())
}

/// 监控模式命令
async fn cmd_monitor(
    config_manager: &ConfigManager,
    account_id: &str,
    auto_reply: Option<String>,
    outformat: &OutputFormat,
) -> Result<()> {
    let config = config_manager
        .load_config(account_id)
        .ok_or_else(|| anyhow!("账号不存在: {}", account_id))?;

    let client = QQBotClient::new(&config.app_id, &config.client_secret);

    if !outformat.is_json() {
        println!("{} 启动监控模式...", "👁".cyan());
        println!("{} 按 Ctrl+C 退出\n", "ℹ".dimmed());
    }

    let (event_tx, mut event_rx) = mpsc::channel(100);
    let api = client.api();

    // 启动 Gateway
    tokio::spawn(async move {
        let _ = start_gateway(api, event_tx).await;
    });

    // 处理事件
    while let Some(event) = event_rx.recv().await {
        match event {
            GatewayEvent::Ready { session_id } => {
                if !outformat.is_json() {
                    println!("{} Gateway 已就绪 (Session: {})", "✓".green(), session_id.cyan());
                }
            }
            GatewayEvent::Resumed => {
                if !outformat.is_json() {
                    println!("{} 会话已恢复", "✓".green());
                }
            }
            GatewayEvent::Message(msg) => {
                match outformat {
                    OutputFormat::Json => {
                        let output = json!({
                            "type": "message",
                            "message": {
                                "id": msg.id,
                                "content": msg.content,
                                "sender_id": msg.sender_id,
                                "sender_name": msg.sender_name,
                                "timestamp": msg.timestamp,
                                "chat_type": format!("{:?}", msg.chat_type),
                                "group_id": msg.group_id,
                                "channel_id": msg.channel_id,
                            }
                        });
                        println!("{}", serde_json::to_string_pretty(&output)?);
                    }
                    OutputFormat::Text => {
                        println!("{}", format_message(&msg));
                    }
                }

                // 自动回复
                if let Some(ref reply_text) = auto_reply {
                    let target_id = match msg.chat_type {
                        TargetType::C2C => msg.sender_id.clone(),
                        TargetType::Group => msg.group_id.clone().unwrap_or_default(),
                        TargetType::Channel => msg.channel_id.clone().unwrap_or_default(),
                    };

                    if !target_id.is_empty() {
                        let sender = client.sender();
                        let _ = sender.send_text(&target_id, reply_text, msg.chat_type).await;
                    }
                }
            }
            GatewayEvent::Disconnected { code, reason } => {
                if !outformat.is_json() {
                    println!("{} 连接断开: {} - {}", "⚠".yellow(), code, reason);
                }
            }
            GatewayEvent::Error(err) => {
                if !outformat.is_json() {
                    eprintln!("{} 错误: {}", "✗".red(), err);
                }
            }
        }
    }

    Ok(())
}

/// Ping 命令（测试连接）
async fn cmd_ping(
    config_manager: &ConfigManager,
    account_id: &str,
    outformat: &OutputFormat,
) -> Result<()> {
    let config = config_manager
        .load_config(account_id)
        .ok_or_else(|| anyhow!("账号不存在: {}", account_id))?;

    let client = QQBotClient::new(&config.app_id, &config.client_secret);

    if !outformat.is_json() {
        println!("{} 测试连接...", "⏳".cyan());
    }

    let start = std::time::Instant::now();

    match client.get_gateway_url().await {
        Ok(url) => {
            let elapsed = start.elapsed();
            match outformat {
                OutputFormat::Json => {
                    let output = json!({
                        "success": true,
                        "latency_ms": elapsed.as_millis(),
                        "gateway_url": url,
                    });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                }
                OutputFormat::Text => {
                    println!("{} 连接成功! 延迟: {}ms", "✓".green(), elapsed.as_millis().to_string().cyan());
                    println!("  Gateway: {}", url.dimmed());
                }
            }
        }
        Err(e) => {
            match outformat {
                OutputFormat::Json => {
                    let output = json!({
                        "success": false,
                        "error": e.to_string(),
                    });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                }
                OutputFormat::Text => {
                    println!("{} 连接失败: {}", "✗".red(), e);
                }
            }
        }
    }

    Ok(())
}

impl OutputFormat {
    fn is_json(&self) -> bool {
        matches!(self, OutputFormat::Json)
    }
}
