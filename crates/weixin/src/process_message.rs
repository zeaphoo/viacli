//! 消息处理管道
//!
//! 处理单条入站消息，包括媒体下载、消息转换等

use std::path::PathBuf;
use std::sync::Arc;

use crate::cdn::download_from_cdn;
use crate::client::WeixinApiClient;
use crate::config_cache::CachedConfig;
use crate::media_send::markdown_to_plain_text;
use crate::silk_transcode::{is_silk_format, silk_to_wav};
use crate::types::*;

/// 消息处理依赖
pub struct ProcessMessageDeps {
    pub account_id: String,
    pub base_url: String,
    pub cdn_base_url: String,
    pub token: Option<String>,
    pub typing_ticket: String,
    pub http_client: Arc<reqwest::Client>,
}

/// 处理后的消息上下文
#[derive(Debug, Clone)]
pub struct ProcessedMessage {
    pub message: WeixinMessage,
    pub text_content: Option<String>,
    pub media_files: Vec<MediaFile>,
    pub is_slash_command: bool,
    pub slash_command: Option<String>,
    pub slash_args: Option<String>,
}

/// 媒体文件信息
#[derive(Debug, Clone)]
pub struct MediaFile {
    pub file_path: PathBuf,
    pub mime_type: String,
    pub original_name: Option<String>,
}

/// 处理单条消息
pub async fn process_one_message(
    message: &WeixinMessage,
    deps: &ProcessMessageDeps,
    config: &CachedConfig,
) -> anyhow::Result<ProcessedMessage> {
    let mut processed = ProcessedMessage {
        message: message.clone(),
        text_content: None,
        media_files: Vec::new(),
        is_slash_command: false,
        slash_command: None,
        slash_args: None,
    };

    let items = message.item_list.clone().unwrap_or_default();
    let from_user_id = message.from_user_id.clone().unwrap_or_default();

    for item in &items {
        let item_type = item.r#type.unwrap_or(0);

        match item_type {
            // 文本消息
            t if t == MessageItemType::Text as i32 => {
                if let Some(text_item) = &item.text_item {
                    let content = text_item.text.clone().unwrap_or_default();
                    
                    // 检查是否为斜杠命令
                    if content.starts_with('/') {
                        let parts: Vec<&str> = content.split_whitespace().collect();
                        if !parts.is_empty() {
                            processed.is_slash_command = true;
                            processed.slash_command = Some(parts[0].to_string());
                            processed.slash_args = if parts.len() > 1 {
                                Some(parts[1..].join(" "))
                            } else {
                                None
                            };
                        }
                    }

                    // 转换 Markdown 为纯文本
                    let plain_text = markdown_to_plain_text(&content);
                    processed.text_content = Some(plain_text);
                }
            }

            // 图片消息
            t if t == MessageItemType::Image as i32 => {
                if let Some(image) = &item.image_item {
                    if let Some(cdn) = &image.media {
                        if let Ok(file) = download_media_from_item(
                            cdn,
                            &deps.cdn_base_url,
                            &deps.http_client,
                            Some("image"),
                        ).await {
                            processed.media_files.push(file);
                        }
                    }
                }
            }

            // 语音消息
            t if t == MessageItemType::Voice as i32 => {
                if let Some(voice) = &item.voice_item {
                    if let Some(cdn) = &voice.media {
                        if let Ok(mut file) = download_media_from_item(
                            cdn,
                            &deps.cdn_base_url,
                            &deps.http_client,
                            Some("voice"),
                        ).await {
                            // 尝试转码 SILK 为 WAV
                            if is_silk_format(&std::fs::read(&file.file_path)?) {
                                if let Some(result) = silk_to_wav(&std::fs::read(&file.file_path)?).await {
                                    let wav_path = file.file_path.with_extension("wav");
                                    std::fs::write(&wav_path, result.wav_data)?;
                                    file.file_path = wav_path;
                                    file.mime_type = "audio/wav".to_string();
                                }
                            }
                            processed.media_files.push(file);
                        }
                    }
                }
            }

            // 文件消息
            t if t == MessageItemType::File as i32 => {
                if let Some(file_item) = &item.file_item {
                    if let Some(cdn) = &file_item.media {
                        if let Ok(file) = download_media_from_item(
                            cdn,
                            &deps.cdn_base_url,
                            &deps.http_client,
                            Some("file"),
                        ).await {
                            processed.media_files.push(file);
                        }
                    }
                }
            }

            // 视频消息
            t if t == MessageItemType::Video as i32 => {
                if let Some(video) = &item.video_item {
                    if let Some(cdn) = &video.media {
                        if let Ok(file) = download_media_from_item(
                            cdn,
                            &deps.cdn_base_url,
                            &deps.http_client,
                            Some("video"),
                        ).await {
                            processed.media_files.push(file);
                        }
                    }
                }
            }

            // 引用消息
            t if t == 6 => {  // Ref 类型
                if let Some(ref_msg) = &item.ref_msg {
                    log::debug!(
                        "received ref message: title={:?}",
                        ref_msg.title
                    );
                }
            }

            _ => {
                log::debug!("unknown message item type: {}", item_type);
            }
        }
    }

    // 发送打字状态
    if !config.typing_ticket.is_empty() {
        let client = WeixinApiClient::new(&deps.base_url);
        let client = if let Some(token) = &deps.token {
            client.with_token(token)
        } else {
            client
        };

        if let Err(e) = client.send_typing(&from_user_id, &config.typing_ticket, TypingStatus::Typing).await {
            log::debug!("failed to send typing status: {}", e);
        }
    }

    Ok(processed)
}

/// 从 CDN 下载媒体
async fn download_media_from_item(
    cdn: &CdnMedia,
    cdn_base_url: &str,
    http_client: &reqwest::Client,
    media_type: Option<&str>,
) -> anyhow::Result<MediaFile> {
    let encrypted_query = cdn.encrypt_query_param.clone().unwrap_or_default();
    let aes_key = cdn.aes_key.clone().unwrap_or_default();

    // 下载并解密
    let data = download_from_cdn(
        http_client,
        cdn_base_url,
        &encrypted_query,
        aes_key.as_bytes(),
    ).await?;

    // 保存到临时文件
    let temp_dir = std::env::temp_dir().join("weixin").join("downloads");
    std::fs::create_dir_all(&temp_dir)?;

    let file_name = format!("unknown.{}" , media_type.unwrap_or("bin"));
    let ext = media_type.map(|t| match t {
        "image" => "jpg",
        "voice" => "silk",
        "video" => "mp4",
        "file" => "bin",
        _ => "bin",
    }).unwrap_or("bin");

    let file_path = temp_dir.join(format!("{}.{}", uuid::Uuid::new_v4(), ext));
    std::fs::write(&file_path, &data)?;

    // 确定 MIME 类型
    let mime_type = media_type.map(|t| match t {
        "image" => "image/jpeg",
        "voice" => "audio/silk",
        "video" => "video/mp4",
        "file" => "application/octet-stream",
        _ => "application/octet-stream",
    }).unwrap_or("application/octet-stream");

    Ok(MediaFile {
        file_path,
        mime_type: mime_type.to_string(),
        original_name: Some(file_name),
    })
}

/// 获取消息文本内容
pub fn get_message_text(message: &WeixinMessage) -> Option<String> {
    let items = message.item_list.as_ref()?;
    
    for item in items {
        if item.r#type == Some(MessageItemType::Text as i32) {
            return item.text_item.as_ref()?.text.clone();
        }
    }
    
    None
}

/// 检查消息是否包含媒体
pub fn has_media(message: &WeixinMessage) -> bool {
    let empty_vec = Vec::new();
    let items = message.item_list.as_ref().unwrap_or(&empty_vec);
    
    items.iter().any(|item| {
        let item_type = item.r#type.unwrap_or(0);
        item_type == MessageItemType::Image as i32
            || item_type == MessageItemType::Voice as i32
            || item_type == MessageItemType::File as i32
            || item_type == MessageItemType::Video as i32
    })
}

/// 获取消息发送者
pub fn get_message_sender(message: &WeixinMessage) -> String {
    message.from_user_id.clone().unwrap_or_default()
}

/// 获取消息上下文 Token
pub fn get_message_context_token(message: &WeixinMessage) -> Option<String> {
    message.context_token.clone()
}
