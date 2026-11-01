//! QQ Bot 类型定义

use serde::{Deserialize, Serialize};

/// QQ Bot 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QQBotConfig {
    pub app_id: String,
    pub client_secret: String,
    pub name: Option<String>,
    pub enabled: bool,
}

/// 访问令牌响应
#[derive(Debug, Deserialize)]
pub struct AccessTokenResponse {
    pub access_token: String,
    pub expires_in: u64,
}

/// 消息响应
#[derive(Debug, Deserialize)]
pub struct MessageResponse {
    pub id: String,
    pub timestamp: String,
    #[serde(rename = "ext_info")]
    pub ext_info: Option<ExtInfo>,
}

#[derive(Debug, Deserialize)]
pub struct ExtInfo {
    #[serde(rename = "ref_idx")]
    pub ref_idx: Option<String>,
}

/// Gateway 响应
#[derive(Debug, Deserialize)]
pub struct GatewayResponse {
    pub url: String,
}

/// WebSocket 消息负载
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsPayload {
    pub op: i32,
    #[serde(rename = "d", skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(rename = "s", skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
    #[serde(rename = "t", skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
}

/// C2C 消息事件
#[derive(Debug, Clone, Deserialize)]
pub struct C2CMessageEvent {
    pub id: String,
    pub content: String,
    pub timestamp: String,
    pub author: C2CAuthor,
    #[serde(rename = "attachments", default)]
    pub attachments: Vec<MessageAttachment>,
    #[serde(rename = "message_scene")]
    pub message_scene: Option<MessageScene>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct C2CAuthor {
    pub id: String,
    #[serde(rename = "user_openid")]
    pub user_openid: String,
    #[serde(rename = "union_openid")]
    pub union_openid: String,
}

/// 群聊消息事件
#[derive(Debug, Clone, Deserialize)]
pub struct GroupMessageEvent {
    pub id: String,
    pub content: String,
    pub timestamp: String,
    #[serde(rename = "group_id")]
    pub group_id: String,
    #[serde(rename = "group_openid")]
    pub group_openid: String,
    pub author: GroupAuthor,
    #[serde(rename = "attachments", default)]
    pub attachments: Vec<MessageAttachment>,
    #[serde(rename = "message_scene")]
    pub message_scene: Option<MessageScene>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GroupAuthor {
    pub id: String,
    #[serde(rename = "member_openid")]
    pub member_openid: String,
}

/// 频道消息事件
#[derive(Debug, Clone, Deserialize)]
pub struct GuildMessageEvent {
    pub id: String,
    pub content: String,
    pub timestamp: String,
    #[serde(rename = "channel_id")]
    pub channel_id: String,
    #[serde(rename = "guild_id")]
    pub guild_id: String,
    pub author: GuildAuthor,
    #[serde(rename = "attachments", default)]
    pub attachments: Vec<MessageAttachment>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GuildAuthor {
    pub id: String,
    pub username: String,
    #[serde(rename = "bot", default)]
    pub bot: bool,
}

/// 消息附件
#[derive(Debug, Clone, Deserialize)]
pub struct MessageAttachment {
    #[serde(rename = "content_type")]
    pub content_type: String,
    pub filename: Option<String>,
    pub url: String,
    pub size: Option<i64>,
    pub height: Option<i32>,
    pub width: Option<i32>,
}

/// 消息场景
#[derive(Debug, Clone, Deserialize)]
pub struct MessageScene {
    pub source: String,
    #[serde(default)]
    pub ext: Vec<String>,
}

/// 消息类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    Text = 0,
    Markdown = 2,
    Ark = 3,
    Embed = 4,
    Media = 7,
}

/// 目标类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetType {
    C2C,
    Group,
    Channel,
}

impl std::fmt::Display for TargetType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetType::C2C => write!(f, "c2c"),
            TargetType::Group => write!(f, "group"),
            TargetType::Channel => write!(f, "channel"),
        }
    }
}

/// 消息结构
#[derive(Debug, Clone, Serialize)]
pub struct Message {
    #[serde(rename = "msg_type")]
    pub msg_type: i32,
    pub content: Option<String>,
    pub markdown: Option<MarkdownContent>,
    #[serde(rename = "msg_id", skip_serializing_if = "Option::is_none")]
    pub msg_id: Option<String>,
    #[serde(rename = "msg_seq", skip_serializing_if = "Option::is_none")]
    pub msg_seq: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarkdownContent {
    pub content: String,
}

/// 富媒体文件类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaFileType {
    Image = 1,
    Video = 2,
    Voice = 3,
    File = 4,
}

/// 上传媒体响应
#[derive(Debug, Deserialize)]
pub struct UploadMediaResponse {
    #[serde(rename = "file_uuid")]
    pub file_uuid: String,
    #[serde(rename = "file_info")]
    pub file_info: String,
    pub ttl: i64,
}

/// 发送消息选项
#[derive(Debug, Clone, Default)]
pub struct SendOptions {
    pub reply_to: Option<String>,
    pub msg_seq: Option<i32>,
    /// 引用的消息 ID (用于回复)
    pub message_reference: Option<String>,
}

/// 接收到的消息
#[derive(Debug, Clone)]
pub struct ReceivedMessage {
    pub id: String,
    pub content: String,
    pub sender_id: String,
    pub sender_name: Option<String>,
    pub timestamp: String,
    pub chat_type: TargetType,
    pub group_id: Option<String>,
    pub channel_id: Option<String>,
    pub attachments: Vec<MessageAttachment>,
}

/// 会话信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub last_seq: Option<i64>,
    pub last_connected_at: Option<i64>,
}

/// 账号信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountInfo {
    pub account_id: String,
    pub app_id: String,
    pub name: Option<String>,
    pub enabled: bool,
}

/// WebSocket 操作码
pub mod op_codes {
    pub const DISPATCH: i32 = 0;
    pub const HEARTBEAT: i32 = 1;
    pub const IDENTIFY: i32 = 2;
    pub const PRESENCE_UPDATE: i32 = 3;
    pub const VOICE_STATE_UPDATE: i32 = 4;
    pub const RESUME: i32 = 6;
    pub const RECONNECT: i32 = 7;
    pub const REQUEST_GUILD_MEMBERS: i32 = 8;
    pub const INVALID_SESSION: i32 = 9;
    pub const HELLO: i32 = 10;
    pub const HEARTBEAT_ACK: i32 = 11;
}

/// Intent 常量
pub mod intents {
    pub const GUILDS: i32 = 1 << 0;
    pub const GUILD_MEMBERS: i32 = 1 << 1;
    pub const DIRECT_MESSAGE: i32 = 1 << 12;
    pub const GROUP_AND_C2C: i32 = 1 << 25;
    pub const PUBLIC_GUILD_MESSAGES: i32 = 1 << 30;

    /// 完整权限（群聊 + 私信 + 频道）
    pub const FULL: i32 = PUBLIC_GUILD_MESSAGES | DIRECT_MESSAGE | GROUP_AND_C2C;
}

/// 事件类型
pub mod event_types {
    pub const READY: &str = "READY";
    pub const RESUMED: &str = "RESUMED";
    pub const C2C_MESSAGE_CREATE: &str = "C2C_MESSAGE_CREATE";
    pub const GROUP_AT_MESSAGE_CREATE: &str = "GROUP_AT_MESSAGE_CREATE";
    pub const AT_MESSAGE_CREATE: &str = "AT_MESSAGE_CREATE";
    pub const DIRECT_MESSAGE_CREATE: &str = "DIRECT_MESSAGE_CREATE";
}
