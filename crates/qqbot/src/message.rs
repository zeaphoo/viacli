//! 消息发送模块

use anyhow::Result;
use std::sync::Arc;

use crate::api::QQBotApi;
use crate::types::*;

/// 发送结果
#[derive(Debug, Clone)]
pub struct SendResult {
    pub success: bool,
    pub message_id: Option<String>,
    pub error: Option<String>,
}

/// 消息发送器
pub struct MessageSender {
    api: Arc<QQBotApi>,
}

impl MessageSender {
    pub fn new(api: Arc<QQBotApi>) -> Self {
        Self { api }
    }

    /// 发送文本消息到指定目标
    pub async fn send_text(
        &self,
        target: &str,
        content: &str,
        target_type: TargetType,
    ) -> Result<SendResult> {
        let options = SendOptions::default();

        let result = match target_type {
            TargetType::C2C => {
                self.api.send_c2c_message(target, content, options).await
            }
            TargetType::Group => {
                self.api.send_group_message(target, content, options).await
            }
            TargetType::Channel => {
                self.api.send_channel_message(target, content, options).await
            }
        };

        match result {
            Ok(resp) => Ok(SendResult {
                success: true,
                message_id: Some(resp.id),
                error: None,
            }),
            Err(e) => Ok(SendResult {
                success: false,
                message_id: None,
                error: Some(e.to_string()),
            }),
        }
    }

    /// 发送 Markdown 消息
    pub async fn send_markdown(
        &self,
        target: &str,
        markdown: &str,
        target_type: TargetType,
    ) -> Result<SendResult> {
        let options = SendOptions::default();

        let result = match target_type {
            TargetType::C2C => {
                self.api.send_c2c_markdown(target, markdown, options).await
            }
            TargetType::Group => {
                self.api.send_group_markdown(target, markdown, options).await
            }
            TargetType::Channel => {
                // 频道暂不支持 Markdown，降级为普通文本
                self.api.send_channel_message(target, markdown, options).await
            }
        };

        match result {
            Ok(resp) => Ok(SendResult {
                success: true,
                message_id: Some(resp.id),
                error: None,
            }),
            Err(e) => Ok(SendResult {
                success: false,
                message_id: None,
                error: Some(e.to_string()),
            }),
        }
    }

    /// 发送图片消息
    pub async fn send_image(
        &self,
        target: &str,
        image_url: &str,
        target_type: TargetType,
        caption: Option<&str>,
    ) -> Result<SendResult> {
        let options = SendOptions::default();

        let result = match target_type {
            TargetType::C2C => {
                let upload = self.upload_c2c_image(target, image_url).await?;
                self.api.send_c2c_media_message(
                    target,
                    &upload.file_info,
                    caption,
                    options,
                ).await
            }
            TargetType::Group => {
                let upload = self.upload_group_image(target, image_url).await?;
                self.api.send_group_media_message(
                    target,
                    &upload.file_info,
                    caption,
                    options,
                ).await
            }
            TargetType::Channel => {
                return Ok(SendResult {
                    success: false,
                    message_id: None,
                    error: Some("Channel image not supported".to_string()),
                });
            }
        };

        match result {
            Ok(resp) => Ok(SendResult {
                success: true,
                message_id: Some(resp.id),
                error: None,
            }),
            Err(e) => Ok(SendResult {
                success: false,
                message_id: None,
                error: Some(e.to_string()),
            }),
        }
    }

    /// 发送文件
    pub async fn send_file(
        &self,
        target: &str,
        file_path: &str,
        target_type: TargetType,
    ) -> Result<SendResult> {
        // 读取文件
        let file_content = tokio::fs::read(file_path).await?;
        let file_data = base64::encode(&file_content);
        let file_name = std::path::Path::new(file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file");

        let options = SendOptions::default();

        let result = match target_type {
            TargetType::C2C => {
                let upload = self.api.upload_c2c_media(
                    target,
                    MediaFileType::File,
                    None,
                    Some(&file_data),
                    Some(file_name),
                ).await?;

                self.api.send_c2c_media_message(
                    target,
                    &upload.file_info,
                    None,
                    options,
                ).await
            }
            TargetType::Group => {
                let upload = self.api.upload_group_media(
                    target,
                    MediaFileType::File,
                    None,
                    Some(&file_data),
                    Some(file_name),
                ).await?;

                self.api.send_group_media_message(
                    target,
                    &upload.file_info,
                    None,
                    options,
                ).await
            }
            TargetType::Channel => {
                return Ok(SendResult {
                    success: false,
                    message_id: None,
                    error: Some("Channel file not supported".to_string()),
                });
            }
        };

        match result {
            Ok(resp) => Ok(SendResult {
                success: true,
                message_id: Some(resp.id),
                error: None,
            }),
            Err(e) => Ok(SendResult {
                success: false,
                message_id: None,
                error: Some(e.to_string()),
            }),
        }
    }

    /// 上传 C2C 图片
    async fn upload_c2c_image(
        &self,
        openid: &str,
        image_url: &str,
    ) -> Result<UploadMediaResponse> {
        if image_url.starts_with("data:") {
            // Base64 图片
            let parts: Vec<&str> = image_url.splitn(2, ',').collect();
            if parts.len() == 2 {
                self.api.upload_c2c_media(
                    openid,
                    MediaFileType::Image,
                    None,
                    Some(parts[1]),
                    None,
                ).await
            } else {
                Err(anyhow::anyhow!("Invalid base64 data URL"))
            }
        } else if image_url.starts_with("http://") || image_url.starts_with("https://") {
            // URL 图片
            self.api.upload_c2c_media(
                openid,
                MediaFileType::Image,
                Some(image_url),
                None,
                None,
            ).await
        } else {
            // 本地文件
            let file_content = tokio::fs::read(image_url).await?;
            let file_data = base64::encode(&file_content);
            self.api.upload_c2c_media(
                openid,
                MediaFileType::Image,
                None,
                Some(&file_data),
                None,
            ).await
        }
    }

    /// 上传群聊图片
    async fn upload_group_image(
        &self,
        group_openid: &str,
        image_url: &str,
    ) -> Result<UploadMediaResponse> {
        if image_url.starts_with("data:") {
            let parts: Vec<&str> = image_url.splitn(2, ',').collect();
            if parts.len() == 2 {
                self.api.upload_group_media(
                    group_openid,
                    MediaFileType::Image,
                    None,
                    Some(parts[1]),
                    None,
                ).await
            } else {
                Err(anyhow::anyhow!("Invalid base64 data URL"))
            }
        } else if image_url.starts_with("http://") || image_url.starts_with("https://") {
            self.api.upload_group_media(
                group_openid,
                MediaFileType::Image,
                Some(image_url),
                None,
                None,
            ).await
        } else {
            let file_content = tokio::fs::read(image_url).await?;
            let file_data = base64::encode(&file_content);
            self.api.upload_group_media(
                group_openid,
                MediaFileType::Image,
                None,
                Some(&file_data),
                None,
            ).await
        }
    }
}

/// 格式化消息用于显示
pub fn format_message(msg: &ReceivedMessage) -> String {
    let sender = msg.sender_name.as_ref().unwrap_or(&msg.sender_id);
    let chat_type = match msg.chat_type {
        TargetType::C2C => "私聊",
        TargetType::Group => "群聊",
        TargetType::Channel => "频道",
    };

    let mut result = format!(
        "[{}] {}: {}",
        chat_type,
        sender,
        msg.content
    );

    if !msg.attachments.is_empty() {
        result.push_str(&format!("\n  [附件: {}个]", msg.attachments.len()));
    }

    result
}

/// 打印消息列表
pub fn print_messages(messages: &[ReceivedMessage]) {
    for msg in messages {
        println!("{}", format_message(msg));
    }
}

// Base64 编码辅助函数
mod base64 {
    pub fn encode(data: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(data)
    }
}
