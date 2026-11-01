//! 入站消息处理管道

use crate::types::*;

/// 入站消息上下文
#[derive(Debug, Clone)]
pub struct InboundMessageContext {
    /// 消息内容
    pub body: String,
    /// 发送者
    pub from: String,
    /// 接收者
    pub to: String,
    /// 账号 ID
    pub account_id: String,
    /// 来源渠道
    pub originating_channel: String,
    /// 原始接收者
    pub originating_to: String,
    /// 消息 SID
    pub message_sid: String,
    /// 时间戳
    pub timestamp: Option<i64>,
    /// 提供商
    pub provider: String,
    /// 聊天类型
    pub chat_type: String,
    /// 上下文 Token
    pub context_token: Option<String>,
    /// 媒体 URL
    pub media_url: Option<String>,
    /// 媒体路径
    pub media_path: Option<String>,
    /// 媒体类型
    pub media_type: Option<String>,
    /// 命令内容
    pub command_body: Option<String>,
    /// 是否授权执行命令
    pub command_authorized: bool,
}

/// 媒体选项
#[derive(Debug, Clone, Default)]
pub struct InboundMediaOpts {
    /// 解密后的图片路径
    pub decrypted_pic_path: Option<String>,
    /// 解密后的语音路径
    pub decrypted_voice_path: Option<String>,
    /// 语音媒体类型
    pub voice_media_type: Option<String>,
    /// 解密后的文件路径
    pub decrypted_file_path: Option<String>,
    /// 文件媒体类型
    pub file_media_type: Option<String>,
    /// 解密后的视频路径
    pub decrypted_video_path: Option<String>,
}

/// 将微信消息转换为入站消息上下文
pub fn weixin_message_to_context(
    msg: &WeixinMessage,
    account_id: &str,
    opts: &InboundMediaOpts,
) -> InboundMessageContext {
    let from_user_id = msg.from_user_id.clone().unwrap_or_default();
    let body = body_from_item_list(msg.item_list.as_ref());

    let mut ctx = InboundMessageContext {
        body,
        from: from_user_id.clone(),
        to: from_user_id.clone(),
        account_id: account_id.to_string(),
        originating_channel: "weixin".to_string(),
        originating_to: from_user_id,
        message_sid: generate_message_sid(),
        timestamp: msg.create_time_ms,
        provider: "weixin".to_string(),
        chat_type: "direct".to_string(),
        context_token: msg.context_token.clone(),
        media_url: None,
        media_path: None,
        media_type: None,
        command_body: None,
        command_authorized: false,
    };

    // 设置媒体路径（优先级：图片 > 视频 > 文件 > 语音）
    if let Some(ref path) = opts.decrypted_pic_path {
        ctx.media_path = Some(path.clone());
        ctx.media_type = Some("image/*".to_string());
    } else if let Some(ref path) = opts.decrypted_video_path {
        ctx.media_path = Some(path.clone());
        ctx.media_type = Some("video/mp4".to_string());
    } else if let Some(ref path) = opts.decrypted_file_path {
        ctx.media_path = Some(path.clone());
        ctx.media_type = Some(opts.file_media_type.clone().unwrap_or_else(|| "application/octet-stream".to_string()));
    } else if let Some(ref path) = opts.decrypted_voice_path {
        ctx.media_path = Some(path.clone());
        ctx.media_type = Some(opts.voice_media_type.clone().unwrap_or_else(|| "audio/wav".to_string()));
    }

    // 提取命令内容
    if ctx.body.starts_with('/') {
        ctx.command_body = Some(ctx.body.clone());
    }

    ctx
}

/// 从消息项列表提取正文
fn body_from_item_list(item_list: Option<&Vec<MessageItem>>) -> String {
    let items = match item_list {
        Some(items) if !items.is_empty() => items,
        _ => return String::new(),
    };

    for item in items {
        match item.r#type {
            Some(t) if t == MessageItemType::Text as i32 => {
                if let Some(ref text_item) = item.text_item {
                    if let Some(ref text) = text_item.text {
                        let mut result = text.clone();
                        
                        // 处理引用消息
                        if let Some(ref ref_msg) = item.ref_msg {
                            // 如果被引用的是媒体，只返回当前文本
                            if let Some(ref msg_item) = ref_msg.message_item {
                                if is_media_item(msg_item) {
                                    return result;
                                }
                            }
                            
                            // 构建引用上下文
                            let mut parts = Vec::new();
                            if let Some(ref title) = ref_msg.title {
                                parts.push(title.clone());
                            }
                            if let Some(ref msg_item) = ref_msg.message_item {
                                let ref_body = body_from_item_list(Some(&vec![msg_item.as_ref().clone()]));
                                if !ref_body.is_empty() {
                                    parts.push(ref_body);
                                }
                            }
                            if !parts.is_empty() {
                                result = format!("[引用: {}]\n{}", parts.join(" | "), result);
                            }
                        }
                        
                        return result;
                    }
                }
            }
            Some(t) if t == MessageItemType::Voice as i32 => {
                // 语音转文字
                if let Some(ref voice_item) = item.voice_item {
                    if let Some(ref text) = voice_item.text {
                        return text.clone();
                    }
                }
            }
            _ => {}
        }
    }

    String::new()
}

/// 检查是否是媒体消息项
fn is_media_item(item: &MessageItem) -> bool {
    matches!(
        item.r#type,
        Some(t) if t == MessageItemType::Image as i32
            || t == MessageItemType::Video as i32
            || t == MessageItemType::File as i32
            || t == MessageItemType::Voice as i32
    )
}

/// 生成消息 SID
fn generate_message_sid() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 16];
    rng.fill(&mut bytes);
    format!("msg-{}", hex::encode(bytes))
}

/// 处理入站消息
pub fn process_inbound_message(msg: &WeixinMessage, account_id: &str) -> InboundMessageContext {
    weixin_message_to_context(msg, account_id, &InboundMediaOpts::default())
}

/// 批量处理入站消息
pub fn process_inbound_messages(
    msgs: &[WeixinMessage],
    account_id: &str,
) -> Vec<InboundMessageContext> {
    msgs.iter()
        .map(|msg| process_inbound_message(msg, account_id))
        .collect()
}
