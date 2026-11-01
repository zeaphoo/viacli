//! 微信 iLink API 客户端库

pub mod account;
pub mod cli;
pub mod client;
pub mod cdn;
pub mod config_cache;
pub mod crypto;
pub mod debug;
pub mod error_notice;
pub mod inbound;
pub mod log_upload;
pub mod logger;
pub mod login;
pub mod media_send;
pub mod media_upload;
pub mod message;
pub mod mime;
pub mod monitor;
pub mod pairing;
pub mod process_message;
pub mod redact;
pub mod send;
pub mod session_guard;
pub mod silk_transcode;
pub mod slash_command;
pub mod sync_buf;
pub mod types;

pub use account::{AccountManager, ContextTokenManager, SyncCache, SyncData};
pub use client::{QRLoginClient, WeixinApiClient};
pub use config_cache::{CachedConfig, ConfigCache};
pub use crypto::{aes_ecb_padded_size, decrypt_aes_ecb, encrypt_aes_ecb, generate_aes_key};
pub use log_upload::{compress_and_upload_log, upload_log_file, LogUploadOptions, LogUploadResult};
pub use login::{quick_login, QRLogin, QRLoginEvent, QRLoginResult};
pub use message::{create_reply_message, create_text_message, format_message, print_message, print_messages};
pub use mime::{get_extension_from_content_type_or_url, get_extension_from_mime, get_file_category, get_mime_from_filename, is_audio_mime, is_document_mime, is_image_mime, is_video_mime};
pub use monitor::{DefaultMessageHandler, MessageHandler, MessageMonitor, MonitorOptions, MonitorState};
pub use pairing::PairingManager;
pub use process_message::{get_message_context_token, get_message_sender, get_message_text, has_media, process_one_message, MediaFile, ProcessMessageDeps, ProcessedMessage};
pub use redact::{redact_body, redact_body_default, redact_headers, redact_token, redact_token_default, redact_url, truncate};
pub use send::{send_text_message, send_text_with_typing, MessageSender, SendOptions, SendResult};
pub use session_guard::{assert_session_active, get_remaining_pause_ms, is_session_paused, pause_session, SESSION_EXPIRED_ERRCODE};
pub use silk_transcode::{is_silk_format, silk_to_wav, SilkTranscodeResult};
pub use sync_buf::{SyncBufData, SyncBufManager};
pub use types::*;

use anyhow::Result;

/// 库版本
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 创建默认配置的 API 客户端
pub fn create_client(token: Option<String>) -> Result<WeixinApiClient> {
    let mut client = WeixinApiClient::new(DEFAULT_BASE_URL);
    if let Some(t) = token {
        client = client.with_token(t);
    }
    Ok(client)
}

/// 从账号管理器创建 API 客户端
pub fn create_client_from_account(account_manager: &AccountManager, account_id: &str) -> Result<WeixinApiClient> {
    let account = account_manager
        .load_account(account_id)
        .ok_or_else(|| anyhow::anyhow!("账号不存在: {}", account_id))?;

    let base_url = account
        .base_url
        .clone()
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());

    let token = account.token.ok_or_else(|| anyhow::anyhow!("账号未登录: {}", account_id))?;

    let mut client = WeixinApiClient::new(base_url).with_token(token);
    
    if let Some(route_tag) = account.route_tag {
        client = client.with_route_tag(route_tag);
    }
    
    Ok(client)
}
