//! CLI 命令实现

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand, ValueEnum};
use colored::Colorize;
use serde_json::json;

use crate::account::{AccountManager, ContextTokenManager, SyncCache, SyncData};
use crate::client::WeixinApiClient;
use crate::login::{QRLogin, QRLoginEvent};
use crate::message::{create_reply_message, create_text_message, print_messages};
use crate::types::*;
use crate::{create_client_from_account, DEFAULT_BASE_URL};

/// 输出格式
#[derive(ValueEnum, Clone, Debug)]
pub enum OutputFormat {
    /// 文本格式输出
    Text,
    /// JSON 格式输出
    Json,
}

/// 微信 CLI 命令
#[derive(Parser)]
#[command(name = "weixin")]
#[command(about = "微信 iLink API 命令行工具")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// 输出格式
    #[arg(long, value_enum, global = true, default_value = "text")]
    pub outformat: OutputFormat,

    /// API 基础 URL
    #[arg(long, global = true)]
    pub base_url: Option<String>,

    /// 账号 ID
    #[arg(short, long, global = true)]
    pub account: Option<String>,

    /// 状态数据目录（用于存储账号信息）
    #[arg(long, global = true)]
    pub state_dir: Option<String>,
}

/// 子命令
#[derive(Subcommand)]
pub enum Commands {
    /// 登录微信（扫码）
    Login {
        /// 登录超时时间（毫秒）
        #[arg(short, long, default_value = "480000")]
        timeout: u64,

        /// 机器人类型
        #[arg(long, default_value = "3")]
        bot_type: String,
    },

    /// 列出已登录账号
    List,

    /// 登出账号
    Logout {
        /// 账号 ID
        account_id: String,
    },

    /// 获取消息更新
    GetUpdates {
        /// 同步缓存（如果不提供则使用账号缓存）
        #[arg(short, long)]
        buf: Option<String>,

        /// 长轮询超时（毫秒）
        #[arg(short, long, default_value = "35000")]
        timeout: u64,
    },

    /// 发送消息
    Send {
        /// 目标用户 ID
        to: String,

        /// 消息内容
        text: String,

        /// 上下文 Token（用于回复）
        #[arg(short, long)]
        context: Option<String>,
    },

    /// 获取配置
    GetConfig {
        /// 用户 ID
        user_id: String,

        /// 上下文 Token
        #[arg(short, long)]
        context: Option<String>,
    },

    /// 发送输入状态
    Typing {
        /// 目标用户 ID
        to: String,

        /// 状态：typing 或 cancel
        #[arg(value_enum)]
        status: TypingStatusArg,
    },

    /// 获取上传 URL（用于发送媒体）
    GetUploadUrl {
        /// 文件路径
        file: String,

        /// 媒体类型
        #[arg(value_enum)]
        media_type: MediaTypeArg,

        /// 目标用户 ID
        to: String,
    },

    /// 监控模式（持续获取消息）
    Monitor {
        /// 轮询间隔（秒）
        #[arg(short, long, default_value = "1")]
        interval: u64,
    },

    /// 发送图片消息
    SendImage {
        /// 目标用户 ID
        to: String,

        /// 图片文件路径
        file: String,

        /// 可选的文本说明
        #[arg(short, long)]
        text: Option<String>,

        /// 上下文 Token
        #[arg(short, long)]
        context: Option<String>,
    },

    /// 发送视频消息
    SendVideo {
        /// 目标用户 ID
        to: String,

        /// 视频文件路径
        file: String,

        /// 可选的文本说明
        #[arg(short, long)]
        text: Option<String>,

        /// 上下文 Token
        #[arg(short, long)]
        context: Option<String>,
    },

    /// 发送文件消息
    SendFile {
        /// 目标用户 ID
        to: String,

        /// 文件路径
        file: String,

        /// 可选的文本说明
        #[arg(short, long)]
        text: Option<String>,

        /// 上下文 Token
        #[arg(short, long)]
        context: Option<String>,
    },
}

/// 输入状态参数
#[derive(ValueEnum, Clone, Debug)]
pub enum TypingStatusArg {
    Typing,
    Cancel,
}

/// 媒体类型参数
#[derive(ValueEnum, Clone, Debug)]
pub enum MediaTypeArg {
    Image,
    Video,
    File,
    Voice,
}

impl From<MediaTypeArg> for UploadMediaType {
    fn from(arg: MediaTypeArg) -> Self {
        match arg {
            MediaTypeArg::Image => UploadMediaType::Image,
            MediaTypeArg::Video => UploadMediaType::Video,
            MediaTypeArg::File => UploadMediaType::File,
            MediaTypeArg::Voice => UploadMediaType::Voice,
        }
    }
}

/// 运行 CLI
pub async fn run(cli: Cli) -> Result<()> {
    let base_url = cli
        .base_url
        .clone()
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

    // 创建账号管理器（使用自定义目录或默认目录）
    let account_manager = if let Some(state_dir) = &cli.state_dir {
        AccountManager::with_state_dir(std::path::PathBuf::from(state_dir))?
    } else {
        AccountManager::new()?
    };

    match cli.command {
        Commands::Login { timeout, bot_type } => {
            cmd_login(&account_manager, &base_url, timeout, &bot_type, &cli.outformat).await?;
        }
        Commands::List => {
            cmd_list(&account_manager, &cli.outformat)?;
        }
        Commands::Logout { account_id } => {
            cmd_logout(&account_manager, &account_id, &cli.outformat)?;
        }
        Commands::GetUpdates { buf, timeout } => {
            let account_id = cli.account.ok_or_else(|| anyhow!("请指定账号 ID (--account)"))?;
            cmd_get_updates(&account_manager, &account_id, buf, timeout, &cli.outformat).await?;
        }
        Commands::Send { to, text, context } => {
            let account_id = cli.account.ok_or_else(|| anyhow!("请指定账号 ID (--account)"))?;
            cmd_send(&account_manager, &account_id, &to, &text, context, &cli.outformat).await?;
        }
        Commands::GetConfig { user_id, context } => {
            let account_id = cli.account.ok_or_else(|| anyhow!("请指定账号 ID (--account)"))?;
            cmd_get_config(&account_manager, &account_id, &user_id, context, &cli.outformat).await?;
        }
        Commands::Typing { to, status } => {
            let account_id = cli.account.ok_or_else(|| anyhow!("请指定账号 ID (--account)"))?;
            let status = match status {
                TypingStatusArg::Typing => TypingStatus::Typing,
                TypingStatusArg::Cancel => TypingStatus::Cancel,
            };
            cmd_typing(&account_manager, &account_id, &to, status, &cli.outformat).await?;
        }
        Commands::GetUploadUrl { file, media_type, to } => {
            let account_id = cli.account.ok_or_else(|| anyhow!("请指定账号 ID (--account)"))?;
            cmd_get_upload_url(&account_manager, &account_id, &file, media_type.into(), &to, &cli.outformat).await?;
        }
        Commands::Monitor { interval } => {
            let account_id = cli.account.ok_or_else(|| anyhow!("请指定账号 ID (--account)"))?;
            cmd_monitor(&account_manager, &account_id, interval, &cli.outformat).await?;
        }
        Commands::SendImage { to, file, text, context } => {
            let account_id = cli.account.ok_or_else(|| anyhow!("请指定账号 ID (--account)"))?;
            cmd_send_image(&account_manager, &account_id, &to, &file, text.as_deref(), context, &cli.outformat).await?;
        }
        Commands::SendVideo { to, file, text, context } => {
            let account_id = cli.account.ok_or_else(|| anyhow!("请指定账号 ID (--account)"))?;
            cmd_send_video(&account_manager, &account_id, &to, &file, text.as_deref(), context, &cli.outformat).await?;
        }
        Commands::SendFile { to, file, text, context } => {
            let account_id = cli.account.ok_or_else(|| anyhow!("请指定账号 ID (--account)"))?;
            cmd_send_file(&account_manager, &account_id, &to, &file, text.as_deref(), context, &cli.outformat).await?;
        }
    }

    Ok(())
}

/// 登录命令
async fn cmd_login(account_manager: &AccountManager, base_url: &str, timeout: u64, bot_type: &str, outformat: &OutputFormat) -> Result<()> {
    let login = QRLogin::with_account_manager(base_url, account_manager.clone())?
        .with_bot_type(bot_type);

    match outformat {
        OutputFormat::Json => {
            // 流式输出 JSON 事件
            let result = login.login_with_callback(timeout, |event| {
                let output = match event {
                    QRLoginEvent::QrCode { qr_url, qr_code } => json!({
                        "event": "qr",
                        "qr_url": qr_url,
                        "qr_code": qr_code,
                    }),
                    QRLoginEvent::Scanned => json!({
                        "event": "scanned",
                    }),
                    QRLoginEvent::Expired { refresh_count, max_refresh } => json!({
                        "event": "expired",
                        "refresh_count": refresh_count,
                        "max_refresh": max_refresh,
                    }),
                    QRLoginEvent::QrRefreshed { qr_url, qr_code, refresh_count } => json!({
                        "event": "qr_refreshed",
                        "qr_url": qr_url,
                        "qr_code": qr_code,
                        "refresh_count": refresh_count,
                    }),
                    QRLoginEvent::Success { account_id, user_id } => json!({
                        "event": "success",
                        "account_id": account_id,
                        "user_id": user_id,
                    }),
                    QRLoginEvent::Failed { message } => json!({
                        "event": "failed",
                        "message": message,
                    }),
                };
                println!("{}", serde_json::to_string(&output).unwrap());
            }).await?;

            // 最终输出结果摘要
            let final_output = json!({
                "event": "done",
                "success": result.success,
                "account_id": result.account_id,
                "user_id": result.user_id,
                "message": result.message,
            });
            println!("{}", serde_json::to_string(&final_output)?);
        }
        OutputFormat::Text => {
            let result = login.login(timeout).await?;
            if result.success {
                println!("\n{}", result.message.green().bold());
                if let Some(account_id) = &result.account_id {
                    println!("{} 账号 ID: {}", "✓".green(), account_id.cyan());
                }
                if let Some(user_id) = &result.user_id {
                    println!("{} 用户 ID: {}", "✓".green(), user_id.cyan());
                }
            } else {
                println!("\n{} {}", "✗".red(), result.message.red());
            }
        }
    }

    Ok(())
}

/// 列出账号命令
fn cmd_list(account_manager: &AccountManager, outformat: &OutputFormat) -> Result<()> {
    let account_ids = account_manager.list_account_ids();

    match outformat {
        OutputFormat::Json => {
            let accounts: Vec<_> = account_ids
                .iter()
                .map(|id| {
                    let data = account_manager.load_account(id);
                    json!({
                        "account_id": id,
                        "configured": data.is_some() && data.as_ref().unwrap().token.is_some(),
                        "user_id": data.as_ref().and_then(|d| d.user_id.clone()),
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&json!({ "accounts": accounts }))?);
        }
        OutputFormat::Text => {
            if account_ids.is_empty() {
                println!("{} 没有已登录的账号", "ℹ".cyan());
                println!("使用 `viacli weixin login` 登录");
            } else {
                println!("\n{}", "已登录的账号:".cyan().bold());
                println!("{}", "━".repeat(50).dimmed());
                for id in &account_ids {
                    let data = account_manager.load_account(id);
                    let status = if data.is_some() && data.as_ref().unwrap().token.is_some() {
                        "✓ 已配置".green()
                    } else {
                        "✗ 未配置".red()
                    };
                    println!("{} {} - {}", "👤".cyan(), id.cyan(), status);
                    if let Some(d) = data {
                        if let Some(ref user_id) = d.user_id {
                            println!("   用户 ID: {}", user_id.dimmed());
                        }
                    }
                }
                println!("{}", "━".repeat(50).dimmed());
            }
        }
    }

    Ok(())
}

/// 登出命令
fn cmd_logout(account_manager: &AccountManager, account_id: &str, outformat: &OutputFormat) -> Result<()> {
    account_manager.clear_account(account_id)?;

    match outformat {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&json!({ "success": true }))?);
        }
        OutputFormat::Text => {
            println!("{} 账号 {} 已登出", "✓".green(), account_id.cyan());
        }
    }

    Ok(())
}

/// 获取更新命令
async fn cmd_get_updates(
    account_manager: &AccountManager,
    account_id: &str,
    buf: Option<String>,
    timeout: u64,
    outformat: &OutputFormat,
) -> Result<()> {
    let client = create_client_from_account(account_manager, account_id)?
        .with_long_poll_timeout(timeout);

    let sync_cache = SyncCache::new(account_manager.state_dir());
    let sync_data = sync_cache.load(account_id).unwrap_or_default();
    let buf = buf.unwrap_or_else(|| sync_data.get_updates_buf);

    let resp = client.get_updates(&buf).await?;

    // 保存同步缓存
    if let Some(new_buf) = &resp.get_updates_buf {
        sync_cache.save(
            account_id,
            &SyncData {
                get_updates_buf: new_buf.clone(),
                last_update_ms: Some(chrono::Utc::now().timestamp_millis()),
            },
        )?;
    }

    // 保存上下文 Token
    if let Some(msgs) = &resp.msgs {
        let ctx_manager = ContextTokenManager::new(account_manager.state_dir());
        for msg in msgs {
            if let (Some(from), Some(token)) = (&msg.from_user_id, &msg.context_token) {
                ctx_manager.set(account_id, from, token)?;
            }
        }
    }

    match outformat {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            if resp.ret != 0 {
                println!("{} 错误: ret={}, errmsg={:?}", "✗".red(), resp.ret, resp.errmsg);
            } else {
                if let Some(msgs) = &resp.msgs {
                    print_messages(msgs);
                } else {
                    println!("{} 没有新消息", "📭".cyan());
                }
            }
        }
    }

    Ok(())
}

/// 发送消息命令
async fn cmd_send(
    account_manager: &AccountManager,
    account_id: &str,
    to: &str,
    text: &str,
    context: Option<String>,
    outformat: &OutputFormat,
) -> Result<()> {
    let client = create_client_from_account(account_manager, account_id)?;

    let msg = if let Some(ctx) = context {
        create_reply_message(to, text, ctx)
    } else {
        create_text_message(to, text)
    };

    let resp = client.send_message(msg).await?;

    match outformat {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            if resp.ret == 0 {
                println!("{} 消息已发送", "✓".green());
            } else {
                println!("{} 发送失败: {:?}", "✗".red(), resp.errmsg);
            }
        }
    }

    Ok(())
}

/// 获取配置命令
async fn cmd_get_config(
    account_manager: &AccountManager,
    account_id: &str,
    user_id: &str,
    context: Option<String>,
    outformat: &OutputFormat,
) -> Result<()> {
    let client = create_client_from_account(account_manager, account_id)?;

    let resp = client.get_config(user_id, context).await?;

    match outformat {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            if resp.ret == 0 {
                println!("{} 配置获取成功", "✓".green());
                if let Some(ticket) = &resp.typing_ticket {
                    println!("{} Typing Ticket: {}", "🎫".cyan(), ticket.chars().take(30).collect::<String>().dimmed());
                }
            } else {
                println!("{} 获取失败: {:?}", "✗".red(), resp.errmsg);
            }
        }
    }

    Ok(())
}

/// 发送输入状态命令
async fn cmd_typing(
    account_manager: &AccountManager,
    account_id: &str,
    to: &str,
    status: TypingStatus,
    outformat: &OutputFormat,
) -> Result<()> {
    let client = create_client_from_account(account_manager, account_id)?;

    // 先获取配置以获取 typing_ticket
    let config = client.get_config(to, None).await?;
    let ticket = config
        .typing_ticket
        .ok_or_else(|| anyhow!("无法获取 typing ticket"))?;

    let resp = client.send_typing(to, ticket, status).await?;

    match outformat {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            if resp.ret == 0 {
                let status_text = match status {
                    TypingStatus::Typing => "正在输入",
                    TypingStatus::Cancel => "取消输入",
                };
                println!("{} {}", "✓".green(), status_text);
            } else {
                println!("{} 发送失败: {:?}", "✗".red(), resp.errmsg);
            }
        }
    }

    Ok(())
}

/// 获取上传 URL 命令
async fn cmd_get_upload_url(
    account_manager: &AccountManager,
    account_id: &str,
    file: &str,
    media_type: UploadMediaType,
    to: &str,
    outformat: &OutputFormat,
) -> Result<()> {
    use sha2::{Digest, Sha256};
    use std::fs;
    use crate::crypto::{aes_ecb_padded_size, generate_aes_key};

    let client = create_client_from_account(account_manager, account_id)?;

    // 读取文件信息
    let file_content = fs::read(file)?;
    let rawsize = file_content.len() as i64;
    let rawfilemd5 = format!("{:x}", md5::compute(&file_content));

    // 计算 AES-128-ECB (PKCS7) 加密后的大小（与 openclaw-weixin 一致）
    let filesize = aes_ecb_padded_size(file_content.len()) as i64;
    let aeskey = generate_aes_key();

    let req = GetUploadUrlReq {
        filekey: Some(file.to_string()),
        media_type: media_type.as_i32(),
        to_user_id: to.to_string(),
        rawsize,
        rawfilemd5,
        filesize,
        thumb_rawsize: None,
        thumb_rawfilemd5: None,
        thumb_filesize: None,
        no_need_thumb: Some(true),
        aeskey: Some(hex::encode(&aeskey)),
        base_info: None,
    };

    let resp = client.get_upload_url(req).await?;

    match outformat {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&resp)?);
        }
        OutputFormat::Text => {
            println!("{} 上传 URL 获取成功", "✓".green());
            if let Some(param) = &resp.upload_param {
                let param_str: String = param.chars().take(50).collect();
                println!("{} Upload Param: {}", "📤".cyan(), param_str.dimmed());
            }
        }
    }

    Ok(())
}

/// 监控命令
async fn cmd_monitor(
    account_manager: &AccountManager,
    account_id: &str,
    interval: u64,
    outformat: &OutputFormat,
) -> Result<()> {
    println!("{} 启动监控模式...", "👁".cyan());
    println!("{} 按 Ctrl+C 退出\n", "ℹ".dimmed());

    let client = create_client_from_account(account_manager, account_id)?;
    let sync_cache = SyncCache::new(account_manager.state_dir());
    let ctx_manager = ContextTokenManager::new(account_manager.state_dir());

    let mut sync_data = sync_cache.load(account_id).unwrap_or_default();

    loop {
        match client.get_updates(&sync_data.get_updates_buf).await {
            Ok(resp) => {
                // 保存同步缓存
                if let Some(ref new_buf) = resp.get_updates_buf {
                    sync_data.get_updates_buf = new_buf.clone();
                    sync_data.last_update_ms = Some(chrono::Utc::now().timestamp_millis());
                    let _ = sync_cache.save(account_id, &sync_data);
                }

                // 处理消息
                if let Some(ref msgs) = resp.msgs {
                    if !msgs.is_empty() {
                        // 保存上下文 Token
                        for msg in msgs {
                            if let (Some(ref from), Some(ref token)) = (&msg.from_user_id, &msg.context_token) {
                                let _ = ctx_manager.set(account_id, from, token);
                            }
                        }

                        match outformat {
                            OutputFormat::Json => {
                                println!("{}", serde_json::to_string_pretty(msgs)?);
                            }
                            OutputFormat::Text => {
                                print_messages(msgs);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("{} 获取更新失败: {}", "✗".red(), e);
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(interval)).await;
    }
}

/// 发送图片命令
async fn cmd_send_image(
    account_manager: &AccountManager,
    account_id: &str,
    to: &str,
    file: &str,
    text: Option<&str>,
    context: Option<String>,
    outformat: &OutputFormat,
) -> Result<()> {
    use crate::media_upload::upload_image_to_weixin;
    use crate::media_send::send_image_message;
    use reqwest::Client;

    let client = create_client_from_account(account_manager, account_id)?;
    let http_client = Client::new();

    // 加载账号获取 CDN 基础 URL
    let account_data = account_manager
        .load_account(account_id)
        .ok_or_else(|| anyhow!("账号不存在"))?;
    let cdn_base_url = "https://novac2c.cdn.weixin.qq.com/c2c".to_string();

    if !outformat.is_json() {
        println!("{} 正在上传图片...", "⏳".cyan());
    }

    // 上传图片
    let uploaded = upload_image_to_weixin(&client, &http_client, file, to, &cdn_base_url).await?;

    if !outformat.is_json() {
        println!("{} 图片上传成功，正在发送...", "✓".green());
    }

    // 发送图片消息
    let message_id = send_image_message(
        &client,
        to,
        text.unwrap_or(""),
        &uploaded,
        context,
    ).await?;

    match outformat {
        OutputFormat::Json => {
            let output = json!({
                "success": true,
                "message_id": message_id,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Text => {
            println!("{} 图片消息已发送", "✓".green());
        }
    }

    Ok(())
}

/// 发送视频命令
async fn cmd_send_video(
    account_manager: &AccountManager,
    account_id: &str,
    to: &str,
    file: &str,
    text: Option<&str>,
    context: Option<String>,
    outformat: &OutputFormat,
) -> Result<()> {
    use crate::media_upload::upload_video_to_weixin;
    use crate::media_send::send_video_message;
    use reqwest::Client;

    let client = create_client_from_account(account_manager, account_id)?;
    let http_client = Client::new();

    let cdn_base_url = "https://novac2c.cdn.weixin.qq.com/c2c".to_string();

    if !outformat.is_json() {
        println!("{} 正在上传视频...", "⏳".cyan());
    }

    let uploaded = upload_video_to_weixin(&client, &http_client, file, to, &cdn_base_url).await?;

    if !outformat.is_json() {
        println!("{} 视频上传成功，正在发送...", "✓".green());
    }

    let message_id = send_video_message(
        &client,
        to,
        text.unwrap_or(""),
        &uploaded,
        context,
    ).await?;

    match outformat {
        OutputFormat::Json => {
            let output = json!({
                "success": true,
                "message_id": message_id,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Text => {
            println!("{} 视频消息已发送", "✓".green());
        }
    }

    Ok(())
}

/// 发送文件命令
async fn cmd_send_file(
    account_manager: &AccountManager,
    account_id: &str,
    to: &str,
    file: &str,
    text: Option<&str>,
    context: Option<String>,
    outformat: &OutputFormat,
) -> Result<()> {
    use crate::media_upload::upload_file_attachment_to_weixin;
    use crate::media_send::send_file_message;
    use reqwest::Client;
    use std::path::Path;

    let client = create_client_from_account(account_manager, account_id)?;
    let http_client = Client::new();

    let cdn_base_url = "https://novac2c.cdn.weixin.qq.com/c2c".to_string();

    // 获取文件名
    let file_name = Path::new(file)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file");

    if !outformat.is_json() {
        println!("{} 正在上传文件...", "⏳".cyan());
    }

    let uploaded = upload_file_attachment_to_weixin(&client, &http_client, file, to, &cdn_base_url).await?;

    if !outformat.is_json() {
        println!("{} 文件上传成功，正在发送...", "✓".green());
    }

    let message_id = send_file_message(
        &client,
        to,
        text.unwrap_or(""),
        file_name,
        &uploaded,
        context,
    ).await?;

    match outformat {
        OutputFormat::Json => {
            let output = json!({
                "success": true,
                "message_id": message_id,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Text => {
            println!("{} 文件消息已发送", "✓".green());
        }
    }

    Ok(())
}

impl OutputFormat {
    fn is_json(&self) -> bool {
        matches!(self, OutputFormat::Json)
    }
}
