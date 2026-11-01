//! 媒体消息发送功能

use anyhow::Result;

use crate::client::WeixinApiClient;
use crate::media_upload::UploadedFileInfo;
use crate::types::*;

/// 发送图片消息
pub async fn send_image_message(
    client: &WeixinApiClient,
    to: &str,
    text: &str,
    uploaded: &UploadedFileInfo,
    context_token: Option<String>,
) -> Result<String> {
    let image_item = MessageItem {
        r#type: Some(MessageItemType::Image as i32),
        image_item: Some(ImageItem {
            media: Some(CdnMedia {
                encrypt_query_param: Some(uploaded.download_encrypted_query_param.clone()),
                aes_key: Some(base64::encode(uploaded.aeskey.as_bytes())),
                encrypt_type: Some(1),
            }),
            mid_size: Some(uploaded.file_size_ciphertext),
            ..Default::default()
        }),
        ..Default::default()
    };

    send_media_items(client, to, text, image_item, context_token).await
}

/// 发送视频消息
pub async fn send_video_message(
    client: &WeixinApiClient,
    to: &str,
    text: &str,
    uploaded: &UploadedFileInfo,
    context_token: Option<String>,
) -> Result<String> {
    let video_item = MessageItem {
        r#type: Some(MessageItemType::Video as i32),
        video_item: Some(VideoItem {
            media: Some(CdnMedia {
                encrypt_query_param: Some(uploaded.download_encrypted_query_param.clone()),
                aes_key: Some(base64::encode(uploaded.aeskey.as_bytes())),
                encrypt_type: Some(1),
            }),
            video_size: Some(uploaded.file_size_ciphertext),
            ..Default::default()
        }),
        ..Default::default()
    };

    send_media_items(client, to, text, video_item, context_token).await
}

/// 发送文件消息
pub async fn send_file_message(
    client: &WeixinApiClient,
    to: &str,
    text: &str,
    file_name: &str,
    uploaded: &UploadedFileInfo,
    context_token: Option<String>,
) -> Result<String> {
    let file_item = MessageItem {
        r#type: Some(MessageItemType::File as i32),
        file_item: Some(FileItem {
            media: Some(CdnMedia {
                encrypt_query_param: Some(uploaded.download_encrypted_query_param.clone()),
                aes_key: Some(base64::encode(uploaded.aeskey.as_bytes())),
                encrypt_type: Some(1),
            }),
            file_name: Some(file_name.to_string()),
            len: Some(uploaded.file_size.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    };

    send_media_items(client, to, text, file_item, context_token).await
}

/// 发送媒体消息项（可选带文本说明）
async fn send_media_items(
    client: &WeixinApiClient,
    to: &str,
    text: &str,
    media_item: MessageItem,
    context_token: Option<String>,
) -> Result<String> {
    // 如果有文本，先发送文本消息
    if !text.is_empty() {
        let text_client_id = generate_client_id();
        let text_msg = WeixinMessage {
            from_user_id: Some("".to_string()),
            to_user_id: Some(to.to_string()),
            client_id: Some(text_client_id),
            message_type: Some(MessageType::Bot as i32),
            message_state: Some(MessageState::Finish as i32),
            item_list: Some(vec![MessageItem {
                r#type: Some(MessageItemType::Text as i32),
                text_item: Some(TextItem {
                    text: Some(text.to_string()),
                }),
                ..Default::default()
            }]),
            context_token: context_token.clone(),
            ..Default::default()
        };
        client.send_message(text_msg).await?;
    }

    // 发送媒体消息
    let media_client_id = generate_client_id();
    let media_msg = WeixinMessage {
        from_user_id: Some("".to_string()),
        to_user_id: Some(to.to_string()),
        client_id: Some(media_client_id.clone()),
        message_type: Some(MessageType::Bot as i32),
        message_state: Some(MessageState::Finish as i32),
        item_list: Some(vec![media_item]),
        context_token,
        ..Default::default()
    };
    client.send_message(media_msg).await?;

    Ok(media_client_id)
}

/// 生成客户端 ID
fn generate_client_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 16];
    rng.fill(&mut bytes);
    format!("weixin-{}", hex::encode(bytes))
}

/// Markdown 转纯文本
pub fn markdown_to_plain_text(text: &str) -> String {
    let mut result = text.to_string();

    // 代码块：去除围栏，保留代码内容
    result = regex::Regex::new(r"```[^\n]*\n?([\s\S]*?)```")
        .unwrap()
        .replace_all(&result, |caps: &regex::Captures| {
            caps.get(1).map(|m| m.as_str().trim()).unwrap_or("").to_string()
        })
        .to_string();

    // 图片：完全移除
    result = regex::Regex::new(r"!\[[^\]]*\]\([^)]*\)")
        .unwrap()
        .replace_all(&result, "")
        .to_string();

    // 链接：只保留显示文本
    result = regex::Regex::new(r"\[([^\]]+)\]\([^)]*\)")
        .unwrap()
        .replace_all(&result, "$1")
        .to_string();

    // 表格：移除分隔行，然后去除首尾管道符
    result = regex::Regex::new(r"^\|[\s:|-]+\|$")
        .unwrap()
        .replace_all(&result, "")
        .to_string();

    result = regex::Regex::new(r"^\|(.+)\|$")
        .unwrap()
        .replace_all(&result, |caps: &regex::Captures| {
            caps.get(1)
                .map(|m| {
                    m.as_str()
                        .split('|')
                        .map(|s| s.trim())
                        .collect::<Vec<_>>()
                        .join("  ")
                })
                .unwrap_or_default()
        })
        .to_string();

    // 粗体和斜体
    result = regex::Regex::new(r"\*\*([^*]+)\*\*")
        .unwrap()
        .replace_all(&result, "$1")
        .to_string();
    result = regex::Regex::new(r"\*([^*]+)\*")
        .unwrap()
        .replace_all(&result, "$1")
        .to_string();
    result = regex::Regex::new(r"__([^_]+)__")
        .unwrap()
        .replace_all(&result, "$1")
        .to_string();
    result = regex::Regex::new(r"_([^_]+)_")
        .unwrap()
        .replace_all(&result, "$1")
        .to_string();

    // 标题
    result = regex::Regex::new(r"^#+\s*")
        .unwrap()
        .replace_all(&result, "")
        .to_string();

    // 列表符号
    result = regex::Regex::new(r"^[-*+]\s+")
        .unwrap()
        .replace_all(&result, "")
        .to_string();
    result = regex::Regex::new(r"^\d+\.\s+")
        .unwrap()
        .replace_all(&result, "")
        .to_string();

    result.trim().to_string()
}
