//! 消息处理

use anyhow::Result;
use colored::Colorize;

use crate::types::{MessageItem, MessageItemType, WeixinMessage};

fn generate_client_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 16];
    rng.fill(&mut bytes);
    format!("viacli-weixin-{}", hex::encode(bytes))
}

/// 格式化消息为文本
pub fn format_message(msg: &WeixinMessage) -> String {
    let mut parts = vec![];

    // 发送者信息
    let from = msg.from_user_id.clone().unwrap_or_default();
    let to = msg.to_user_id.clone().unwrap_or_default();

    if !from.is_empty() {
        parts.push(format!("{} 来自: {}", "👤".cyan(), from.dimmed()));
    }
    if !to.is_empty() {
        parts.push(format!("{} 发送至: {}", "📤".cyan(), to.dimmed()));
    }

    // 时间
    if let Some(time_ms) = msg.create_time_ms {
        let datetime = chrono::DateTime::from_timestamp_millis(time_ms);
        if let Some(dt) = datetime {
            parts.push(format!("{} {}", "🕐".cyan(), dt.format("%Y-%m-%d %H:%M:%S")));
        }
    }

    // 消息内容
    if let Some(items) = &msg.item_list {
        for item in items {
            let content = format_message_item(item);
            if !content.is_empty() {
                parts.push(content);
            }
        }
    }

    // 上下文 Token
    if let Some(token) = &msg.context_token {
        parts.push(format!("{} 上下文: {}", "🔗".cyan(), token.chars().take(20).collect::<String>().dimmed()));
    }

    parts.join("\n")
}

/// 格式化消息项
fn format_message_item(item: &MessageItem) -> String {
    match item.r#type {
        Some(t) if t == MessageItemType::Text as i32 => {
            if let Some(text_item) = &item.text_item {
                if let Some(text) = &text_item.text {
                    return format!("{} {}", "💬".cyan(), text.white());
                }
            }
        }
        Some(t) if t == MessageItemType::Image as i32 => {
            return format!("{} [图片消息]", "🖼️".cyan());
        }
        Some(t) if t == MessageItemType::Voice as i32 => {
            return format!("{} [语音消息]", "🎤".cyan());
        }
        Some(t) if t == MessageItemType::File as i32 => {
            if let Some(file_item) = &item.file_item {
                if let Some(name) = &file_item.file_name {
                    return format!("{} [文件] {}", "📎".cyan(), name.white());
                }
            }
            return format!("{} [文件消息]", "📎".cyan());
        }
        Some(t) if t == MessageItemType::Video as i32 => {
            return format!("{} [视频消息]", "🎬".cyan());
        }
        _ => {
            return format!("{} [未知消息类型]", "❓".cyan());
        }
    }
    String::new()
}

/// 打印消息
pub fn print_message(msg: &WeixinMessage, index: Option<usize>) {
    if let Some(i) = index {
        println!("\n{} {}", "━".repeat(50).dimmed(), format!("消息 #{}", i).cyan());
    } else {
        println!("\n{}", "━".repeat(50).dimmed());
    }
    println!("{}", format_message(msg));
    println!("{}", "━".repeat(50).dimmed());
}

/// 打印消息列表
pub fn print_messages(msgs: &[WeixinMessage]) {
    if msgs.is_empty() {
        println!("{} 没有新消息", "📭".cyan());
        return;
    }

    println!("\n{} 收到 {} 条新消息", "📬".cyan(), msgs.len().to_string().green().bold());

    for (i, msg) in msgs.iter().enumerate() {
        print_message(msg, Some(i + 1));
    }
}

/// 创建文本消息
pub fn create_text_message(to_user_id: impl Into<String>, text: impl Into<String>) -> WeixinMessage {
    WeixinMessage {
        to_user_id: Some(to_user_id.into()),
        from_user_id: Some("".to_string()),
        client_id: Some(generate_client_id()),
        message_type: Some(crate::types::MessageType::Bot as i32),
        message_state: Some(crate::types::MessageState::Finish as i32),
        item_list: Some(vec![MessageItem {
            r#type: Some(MessageItemType::Text as i32),
            text_item: Some(crate::types::TextItem {
                text: Some(text.into()),
            }),
            ..Default::default()
        }]),
        ..Default::default()
    }
}

/// 创建回复消息
pub fn create_reply_message(
    to_user_id: impl Into<String>,
    text: impl Into<String>,
    context_token: impl Into<String>,
) -> WeixinMessage {
    WeixinMessage {
        to_user_id: Some(to_user_id.into()),
        from_user_id: Some("".to_string()),
        client_id: Some(generate_client_id()),
        message_type: Some(crate::types::MessageType::Bot as i32),
        message_state: Some(crate::types::MessageState::Finish as i32),
        context_token: Some(context_token.into()),
        item_list: Some(vec![MessageItem {
            r#type: Some(MessageItemType::Text as i32),
            text_item: Some(crate::types::TextItem {
                text: Some(text.into()),
            }),
            ..Default::default()
        }]),
        ..Default::default()
    }
}
