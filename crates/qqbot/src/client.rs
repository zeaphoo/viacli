//! QQ Bot 客户端
//! 高级封装，简化常用操作

use anyhow::Result;
use std::sync::Arc;

use crate::api::QQBotApi;
use crate::message::MessageSender;
use crate::types::*;

/// QQ Bot 客户端
pub struct QQBotClient {
    api: Arc<QQBotApi>,
    app_id: String,
}

impl QQBotClient {
    /// 创建新客户端
    pub fn new(app_id: &str, client_secret: &str) -> Self {
        Self {
            api: Arc::new(QQBotApi::new(app_id, client_secret)),
            app_id: app_id.to_string(),
        }
    }

    /// 获取 API 实例
    pub fn api(&self) -> Arc<QQBotApi> {
        self.api.clone()
    }

    /// 获取 App ID
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    /// 创建消息发送器
    pub fn sender(&self) -> MessageSender {
        MessageSender::new(self.api.clone())
    }

    /// 发送文本消息 (便捷方法)
    pub async fn send_text(&self, target: &str, content: &str, target_type: TargetType) -> Result<MessageResponse> {
        let options = SendOptions::default();

        match target_type {
            TargetType::C2C => {
                self.api.send_c2c_message(target, content, options).await
            }
            TargetType::Group => {
                self.api.send_group_message(target, content, options).await
            }
            TargetType::Channel => {
                self.api.send_channel_message(target, content, options).await
            }
        }
    }

    /// 发送 Markdown 消息 (便捷方法)
    pub async fn send_markdown(
        &self,
        target: &str,
        markdown: &str,
        target_type: TargetType,
    ) -> Result<MessageResponse> {
        let options = SendOptions::default();

        match target_type {
            TargetType::C2C => {
                self.api.send_c2c_markdown(target, markdown, options).await
            }
            TargetType::Group => {
                self.api.send_group_markdown(target, markdown, options).await
            }
            TargetType::Channel => {
                // 频道暂不支持 Markdown
                self.api.send_channel_message(target, markdown, options).await
            }
        }
    }

    /// 发送图片消息
    pub async fn send_image(
        &self,
        target: &str,
        image_url: &str,
        target_type: TargetType,
        content: Option<&str>,
    ) -> Result<MessageResponse> {
        let options = SendOptions::default();

        match target_type {
            TargetType::C2C => {
                // 上传图片
                let upload = if image_url.starts_with("data:") {
                    // Base64 图片
                    let parts: Vec<&str> = image_url.splitn(2, ',').collect();
                    if parts.len() == 2 {
                        self.api.upload_c2c_media(
                            target,
                            MediaFileType::Image,
                            None,
                            Some(parts[1]),
                            None,
                        ).await?
                    } else {
                        return Err(anyhow::anyhow!("Invalid base64 data URL"));
                    }
                } else {
                    // URL 图片
                    self.api.upload_c2c_media(
                        target,
                        MediaFileType::Image,
                        Some(image_url),
                        None,
                        None,
                    ).await?
                };

                // 发送媒体消息
                self.api.send_c2c_media_message(
                    target,
                    &upload.file_info,
                    content,
                    options,
                ).await
            }
            TargetType::Group => {
                let upload = if image_url.starts_with("data:") {
                    let parts: Vec<&str> = image_url.splitn(2, ',').collect();
                    if parts.len() == 2 {
                        self.api.upload_group_media(
                            target,
                            MediaFileType::Image,
                            None,
                            Some(parts[1]),
                            None,
                        ).await?
                    } else {
                        return Err(anyhow::anyhow!("Invalid base64 data URL"));
                    }
                } else {
                    self.api.upload_group_media(
                        target,
                        MediaFileType::Image,
                        Some(image_url),
                        None,
                        None,
                    ).await?
                };

                self.api.send_group_media_message(
                    target,
                    &upload.file_info,
                    content,
                    options,
                ).await
            }
            TargetType::Channel => {
                // 频道暂不支持富媒体
                Err(anyhow::anyhow!("Channel media not supported yet"))
            }
        }
    }

    /// 发送文件消息
    pub async fn send_file(
        &self,
        target: &str,
        file_data: &str,
        file_name: &str,
        target_type: TargetType,
    ) -> Result<MessageResponse> {
        let options = SendOptions::default();

        match target_type {
            TargetType::C2C => {
                let upload = self.api.upload_c2c_media(
                    target,
                    MediaFileType::File,
                    None,
                    Some(file_data),
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
                    Some(file_data),
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
                Err(anyhow::anyhow!("Channel file not supported yet"))
            }
        }
    }

    /// 获取 Gateway URL
    pub async fn get_gateway_url(&self) -> Result<String> {
        self.api.get_gateway_url().await
    }

    /// 发送输入状态
    pub async fn send_typing(&self, openid: &str, seconds: i32) -> Result<()> {
        self.api.send_c2c_input_notify(openid, seconds).await?;
        Ok(())
    }
}
