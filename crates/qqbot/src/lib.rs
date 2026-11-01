//! QQ Bot API 客户端库
//! 参考 openclaw-qqbot 实现，提供单聊和群聊功能

pub mod api;
pub mod cli;
pub mod client;
pub mod config;
pub mod gateway;
pub mod gateway_v2;
pub mod image_server;
pub mod media_tags;
pub mod message;
pub mod types;

pub use api::{QQBotApi, TokenCache};
pub use client::QQBotClient;
pub use config::{QQBotConfig, ConfigManager};
pub use message::{MessageSender, SendResult};
pub use types::*;

/// 库版本
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// QQ Bot API 基础 URL
pub const API_BASE: &str = "https://api.sgroup.qq.com";

/// Token 获取 URL
pub const TOKEN_URL: &str = "https://bots.qq.com/app/getAppAccessToken";

/// Gateway URL
pub const GATEWAY_URL: &str = "https://api.sgroup.qq.com/gateway";

/// 创建默认配置的 API 客户端
pub fn create_client(app_id: &str, client_secret: &str) -> QQBotClient {
    QQBotClient::new(app_id, client_secret)
}

/// 从配置管理器创建 API 客户端
pub fn create_client_from_config(config_manager: &ConfigManager, account_id: &str) -> anyhow::Result<QQBotClient> {
    let config = config_manager
        .load_config(account_id)
        .ok_or_else(|| anyhow::anyhow!("账号不存在: {}", account_id))?;

    Ok(QQBotClient::new(&config.app_id, &config.client_secret))
}
