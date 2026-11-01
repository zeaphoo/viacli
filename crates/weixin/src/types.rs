//! 微信 iLink API 类型定义

use serde::{Deserialize, Serialize};

/// 默认 iLink API 基础 URL
pub const DEFAULT_BASE_URL: &str = "https://ilinkai.weixin.qq.com";
/// 默认 CDN 基础 URL
pub const CDN_BASE_URL: &str = "https://novac2c.cdn.weixin.qq.com/c2c";
/// 默认机器人类型
pub const DEFAULT_BOT_TYPE: &str = "3";

/// 基础信息，附加到每个 CGI 请求
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BaseInfo {
    pub channel_version: String,
}

/// 上传媒体类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadMediaType {
    Image = 1,
    Video = 2,
    File = 3,
    Voice = 4,
}

impl UploadMediaType {
    pub fn as_i32(&self) -> i32 {
        *self as i32
    }
}

/// 消息类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageType {
    None = 0,
    User = 1,
    Bot = 2,
}

/// 消息项类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageItemType {
    None = 0,
    Text = 1,
    Image = 2,
    Voice = 3,
    File = 4,
    Video = 5,
}

/// 消息状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageState {
    New = 0,
    Generating = 1,
    Finish = 2,
}

/// 输入状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypingStatus {
    Typing = 1,
    Cancel = 2,
}

/// CDN 媒体引用
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CdnMedia {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypt_query_param: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aes_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypt_type: Option<i32>,
}

/// 文本消息项
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TextItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// 图片消息项
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImageItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<CdnMedia>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_media: Option<CdnMedia>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aeskey: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mid_size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_height: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hd_size: Option<i64>,
}

/// 语音消息项
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VoiceItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<CdnMedia>,
    /// 语音编码类型：1=pcm 2=adpcm 3=feature 4=speex 5=amr 6=silk 7=mp3 8=ogg-speex
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encode_type: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bits_per_sample: Option<i32>,
    /// 采样率 (Hz)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<i32>,
    /// 语音长度 (毫秒)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playtime: Option<i64>,
    /// 语音转文字内容
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// 文件消息项
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<CdnMedia>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub md5: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub len: Option<String>,
}

/// 视频消息项
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VideoItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media: Option<CdnMedia>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub play_length: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_md5: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_media: Option<CdnMedia>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_height: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_width: Option<i32>,
}

/// 引用消息
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RefMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_item: Option<Box<MessageItem>>,
    /// 摘要
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// 消息项
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create_time_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_time_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_completed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_msg: Option<Box<RefMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_item: Option<TextItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_item: Option<ImageItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice_item: Option<VoiceItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_item: Option<FileItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_item: Option<VideoItem>,
}

/// 微信消息
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WeixinMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seq: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub create_time_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_time_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete_time_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_type: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_state: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_list: Option<Vec<MessageItem>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_token: Option<String>,
}

/// 获取更新请求
#[derive(Debug, Clone, Serialize, Default)]
pub struct GetUpdatesReq {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_buf: Option<String>,
    pub get_updates_buf: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_info: Option<BaseInfo>,
}

/// 获取更新响应
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GetUpdatesResp {
    #[serde(default)]
    pub ret: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errcode: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errmsg: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msgs: Option<Vec<WeixinMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_buf: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub get_updates_buf: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub longpolling_timeout_ms: Option<i64>,
}

/// 发送消息请求
#[derive(Debug, Clone, Serialize, Default)]
pub struct SendMessageReq {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msg: Option<WeixinMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_info: Option<BaseInfo>,
}

/// 发送消息响应
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SendMessageResp {
    #[serde(default)]
    pub ret: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errmsg: Option<String>,
}

/// 获取上传 URL 请求
#[derive(Debug, Clone, Serialize, Default)]
pub struct GetUploadUrlReq {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filekey: Option<String>,
    pub media_type: i32,
    pub to_user_id: String,
    /// 原文件明文大小
    pub rawsize: i64,
    /// 原文件明文 MD5
    pub rawfilemd5: String,
    /// 原文件密文大小（AES-128-ECB 加密后）
    pub filesize: i64,
    /// 缩略图明文大小（IMAGE/VIDEO 时必填）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_rawsize: Option<i64>,
    /// 缩略图明文 MD5（IMAGE/VIDEO 时必填）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_rawfilemd5: Option<String>,
    /// 缩略图密文大小（IMAGE/VIDEO 时必填）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_filesize: Option<i64>,
    /// 不需要缩略图上传 URL，默认 false
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_need_thumb: Option<bool>,
    /// 加密 key
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aeskey: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_info: Option<BaseInfo>,
}

/// 获取上传 URL 响应
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GetUploadUrlResp {
    #[serde(default)]
    pub ret: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errmsg: Option<String>,
    /// 原图上传加密参数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_param: Option<String>,
    /// 缩略图上传加密参数，无缩略图时为空
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_upload_param: Option<String>,
}

/// 获取配置请求
#[derive(Debug, Clone, Serialize, Default)]
pub struct GetConfigReq {
    pub ilink_user_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_info: Option<BaseInfo>,
}

/// 获取配置响应
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GetConfigResp {
    #[serde(default)]
    pub ret: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errmsg: Option<String>,
    /// Base64-encoded typing ticket for sendTyping
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typing_ticket: Option<String>,
}

/// 发送输入状态请求
#[derive(Debug, Clone, Serialize, Default)]
pub struct SendTypingReq {
    pub ilink_user_id: String,
    pub typing_ticket: String,
    /// 1=typing (default), 2=cancel typing
    pub status: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_info: Option<BaseInfo>,
}

/// 发送输入状态响应
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SendTypingResp {
    #[serde(default)]
    pub ret: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errmsg: Option<String>,
}

/// 二维码响应
#[derive(Debug, Clone, Deserialize)]
pub struct QRCodeResponse {
    pub qrcode: String,
    pub qrcode_img_content: String,
}

/// 登录状态响应
#[derive(Debug, Clone, Deserialize)]
pub struct StatusResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ilink_bot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseurl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ilink_user_id: Option<String>,
}

/// 账号数据
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AccountData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_tag: Option<String>,
}

/// 账号信息
#[derive(Debug, Clone)]
pub struct Account {
    pub account_id: String,
    pub base_url: String,
    pub cdn_base_url: String,
    pub token: Option<String>,
    pub enabled: bool,
    pub configured: bool,
    pub name: Option<String>,
    pub route_tag: Option<String>,
}
