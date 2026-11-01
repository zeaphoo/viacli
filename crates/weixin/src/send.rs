//! 发送消息高级功能
//!
//! 提供高级消息发送功能，包括分块发送、打字状态等

use std::sync::Arc;

use crate::client::WeixinApiClient;
use crate::message::{create_reply_message, create_text_message};
use crate::types::*;

/// 文本分块限制（4000字符）
const TEXT_CHUNK_LIMIT: usize = 4000;

/// 发送选项
pub struct SendOptions {
    pub base_url: String,
    pub token: Option<String>,
    pub context_token: Option<String>,
    pub typing_ticket: Option<String>,
    pub timeout_ms: Option<u64>,
}

/// 发送结果
#[derive(Debug, Clone)]
pub struct SendResult {
    pub message_id: String,
    pub chunks_sent: usize,
}

/// 发送消息构建器
pub struct MessageSender {
    client: Arc<WeixinApiClient>,
}

impl MessageSender {
    /// 创建新的消息发送器
    pub fn new(options: &SendOptions) -> Self {
        let mut client = WeixinApiClient::new(&options.base_url);
        
        if let Some(token) = &options.token {
            client = client.with_token(token);
        }
        
        if let Some(timeout) = options.timeout_ms {
            client = client.with_api_timeout(timeout);
        }

        Self {
            client: Arc::new(client),
        }
    }

    /// 发送文本消息（支持分块）
    pub async fn send_text(
        &self,
        to: &str,
        text: &str,
        context_token: Option<&str>,
    ) -> anyhow::Result<SendResult> {
        // 分块发送长文本
        let chunks = self.chunk_text(text);
        let mut last_message_id = String::new();

        for (i, chunk) in chunks.iter().enumerate() {
            let msg = if let Some(token) = context_token {
                create_reply_message(to, chunk, token)
            } else {
                create_text_message(to, chunk)
            };
            
            let _resp = self.client.send_message(msg).await?;
            
            // API 不返回 message_id
            last_message_id = format!("chunk_{}", i);

            log::debug!("sent text chunk {}/{} to {}", i + 1, chunks.len(), to);

            // 非最后一块时添加小延迟
            if i < chunks.len() - 1 {
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            }
        }

        Ok(SendResult {
            message_id: last_message_id,
            chunks_sent: chunks.len(),
        })
    }

    /// 发送打字状态
    pub async fn send_typing(
        &self,
        to: &str,
        ticket: &str,
        _status: TypingStatus,
    ) -> anyhow::Result<()> {
        // 发送打字状态，status: 1=typing, 2=cancel
        self.client.send_typing(to, ticket, TypingStatus::Typing).await?;
        Ok(())
    }

    /// 发送带打字指示的文本消息
    pub async fn send_text_with_typing(
        &self,
        to: &str,
        text: &str,
        typing_ticket: Option<&str>,
        context_token: Option<&str>,
    ) -> anyhow::Result<SendResult> {
        // 发送打字状态开始
        if let Some(ticket) = typing_ticket {
            if let Err(e) = self.send_typing(to, ticket, TypingStatus::Typing).await {
                log::warn!("failed to send typing start: {}", e);
            }
        }

        // 发送消息
        let result = self.send_text(to, text, context_token).await;

        // 发送打字状态结束 (使用 cancel 状态)
        if let Some(ticket) = typing_ticket {
            if let Err(e) = self.client.send_typing(to, ticket, TypingStatus::Cancel).await {
                log::warn!("failed to send typing stop: {}", e);
            }
        }

        result
    }

    /// 分块长文本
    fn chunk_text(&self, text: &str) -> Vec<String> {
        if text.len() <= TEXT_CHUNK_LIMIT {
            return vec![text.to_string()];
        }

        let mut chunks = Vec::new();
        let mut start = 0;

        while start < text.len() {
            let end = (start + TEXT_CHUNK_LIMIT).min(text.len());
            
            // 尝试在单词边界分块
            let chunk_end = if end < text.len() {
                // 向前查找空格或换行
                let search_start = end.saturating_sub(100);
                match text[search_start..end].rfind(|c: char| c.is_whitespace()) {
                    Some(pos) => search_start + pos,
                    None => end,
                }
            } else {
                end
            };

            chunks.push(text[start..chunk_end].to_string());
            start = chunk_end;

            // 跳过开头的空白字符
            while start < text.len() && text.chars().nth(start).map(|c| c.is_whitespace()).unwrap_or(false) {
                start += 1;
            }
        }

        chunks
    }

    /// 发送引用消息
    pub async fn send_ref_message(
        &self,
        to: &str,
        _ref_message_id: &str,
        text: Option<&str>,
        context_token: Option<&str>,
    ) -> anyhow::Result<SendResult> {
        // 引用消息类型为 6
        let items = vec![
            MessageItem {
                r#type: Some(6),  // Ref type
                ref_msg: Some(Box::new(RefMessage {
                    title: text.map(|t| t.to_string()),
                    ..Default::default()
                })),
                ..Default::default()
            },
            MessageItem {
                r#type: Some(MessageItemType::Text as i32),
                text_item: text.map(|t| TextItem {
                    text: Some(t.to_string()),
                }),
                ..Default::default()
            },
        ];

        let msg = WeixinMessage {
            to_user_id: Some(to.to_string()),
            item_list: Some(items),
            context_token: context_token.map(|s| s.to_string()),
            ..Default::default()
        };

        let _resp = self.client.send_message(msg).await?;

        Ok(SendResult {
            message_id: String::new(),  // API 不返回 message_id
            chunks_sent: 1,
        })
    }

    /// 发送回复消息
    pub async fn send_reply(
        &self,
        to: &str,
        reply_to_message_id: &str,
        text: &str,
        context_token: Option<&str>,
    ) -> anyhow::Result<SendResult> {
        self.send_ref_message(to, reply_to_message_id, Some(text), context_token).await
    }
}

/// 便捷函数：发送文本消息
pub async fn send_text_message(
    to: &str,
    text: &str,
    opts: &SendOptions,
) -> anyhow::Result<SendResult> {
    let sender = MessageSender::new(opts);
    sender.send_text(to, text, opts.context_token.as_deref()).await
}

/// 便捷函数：发送带打字指示的文本消息
pub async fn send_text_with_typing(
    to: &str,
    text: &str,
    typing_ticket: Option<&str>,
    opts: &SendOptions,
) -> anyhow::Result<SendResult> {
    let sender = MessageSender::new(opts);
    sender.send_text_with_typing(to, text, typing_ticket, opts.context_token.as_deref()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_text() {
        let sender = MessageSender::new(&SendOptions {
            base_url: "https://example.com".to_string(),
            token: None,
            context_token: None,
            typing_ticket: None,
            timeout_ms: None,
        });

        // 短文本不分块
        let chunks = sender.chunk_text("hello");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "hello");

        // 长文本分块
        let long_text = "a".repeat(5000);
        let chunks = sender.chunk_text(&long_text);
        assert!(chunks.len() > 1);
        
        // 每个块不超过限制
        for chunk in &chunks {
            assert!(chunk.len() <= TEXT_CHUNK_LIMIT);
        }

        // 合并后应等于原文
        let merged: String = chunks.join("");
        assert_eq!(merged, long_text);
    }
}
