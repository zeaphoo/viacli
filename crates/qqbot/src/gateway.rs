//! WebSocket Gateway 连接管理
//! 处理消息接收和连接保持

use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{interval, timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::api::QQBotApi;
use crate::types::*;

/// Gateway 事件
#[derive(Debug, Clone)]
pub enum GatewayEvent {
    /// 连接就绪
    Ready { session_id: String },
    /// 会话恢复
    Resumed,
    /// 收到消息
    Message(ReceivedMessage),
    /// 连接断开
    Disconnected { code: u16, reason: String },
    /// 错误
    Error(String),
}

/// Gateway 连接配置
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// 重连延迟（毫秒）
    pub reconnect_delays: Vec<u64>,
    /// 最大重连次数
    pub max_reconnect_attempts: usize,
    /// 心跳间隔（毫秒）
    pub heartbeat_interval: u64,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            reconnect_delays: vec![1000, 2000, 5000, 10000, 30000],
            max_reconnect_attempts: 100,
            heartbeat_interval: 30000,
        }
    }
}

/// Gateway 连接管理器
pub struct GatewayConnection {
    api: Arc<QQBotApi>,
    config: GatewayConfig,
    event_tx: mpsc::Sender<GatewayEvent>,
    session_id: Option<String>,
    last_seq: Option<i64>,
}

impl GatewayConnection {
    /// 创建新的 Gateway 连接
    pub fn new(
        api: Arc<QQBotApi>,
        config: GatewayConfig,
        event_tx: mpsc::Sender<GatewayEvent>,
    ) -> Self {
        Self {
            api,
            config,
            event_tx,
            session_id: None,
            last_seq: None,
        }
    }

    /// 启动连接
    pub async fn start(&mut self) -> Result<()> {
        let mut reconnect_attempts = 0;

        loop {
            match self.connect().await {
                Ok(()) => {
                    reconnect_attempts = 0;
                    log::info!("[qqbot] Gateway connection closed, reconnecting...");
                }
                Err(e) => {
                    log::error!("[qqbot] Gateway error: {}", e);

                    if reconnect_attempts >= self.config.max_reconnect_attempts {
                        return Err(anyhow!("Max reconnect attempts reached"));
                    }

                    let delay = self
                        .config
                        .reconnect_delays
                        .get(reconnect_attempts)
                        .copied()
                        .unwrap_or(60000);

                    log::info!("[qqbot] Reconnecting in {}ms (attempt {})", delay, reconnect_attempts + 1);
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    reconnect_attempts += 1;
                }
            }
        }
    }

    /// 建立连接
    async fn connect(&mut self) -> Result<()> {
        let gateway_url = self.api.get_gateway_url().await?;
        log::info!("[qqbot] Connecting to gateway: {}", gateway_url);

        let (ws_stream, _) = connect_async(&gateway_url).await?;
        log::info!("[qqbot] WebSocket connected");

        let (mut write, mut read) = ws_stream.split();

        // 启动心跳任务
        let (heartbeat_tx, mut heartbeat_rx) = mpsc::channel::<i64>(10);
        let heartbeat_handle = tokio::spawn(async move {
            let mut interval = interval(Duration::from_millis(30000));
            loop {
                interval.tick().await;
                if let Some(seq) = heartbeat_rx.recv().await {
                    let payload = json!({
                        "op": op_codes::HEARTBEAT,
                        "d": seq,
                    });
                    if let Err(e) = write.send(Message::Text(payload.to_string())).await {
                        log::error!("[qqbot] Heartbeat error: {}", e);
                        break;
                    }
                }
            }
        });

        // 处理消息
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Err(e) = self.handle_payload(&text, &heartbeat_tx).await {
                        log::error!("[qqbot] Handle payload error: {}", e);
                    }
                }
                Ok(Message::Close(frame)) => {
                    let code = frame.as_ref().map(|f| f.code.into()).unwrap_or(1000);
                    let reason = frame
                        .as_ref()
                        .map(|f| f.reason.to_string())
                        .unwrap_or_else(|| "Unknown".to_string());

                    log::info!("[qqbot] WebSocket closed: {} - {}", code, reason);

                    let _ = self
                        .event_tx
                        .send(GatewayEvent::Disconnected { code, reason })
                        .await;
                    break;
                }
                Ok(_) => {}
                Err(e) => {
                    log::error!("[qqbot] WebSocket error: {}", e);
                    let _ = self.event_tx.send(GatewayEvent::Error(e.to_string())).await;
                    break;
                }
            }
        }

        // 清理
        drop(heartbeat_tx);
        let _ = heartbeat_handle.await;

        Ok(())
    }

    /// 处理 WebSocket 消息
    async fn handle_payload(
        &mut self,
        text: &str,
        heartbeat_tx: &mpsc::Sender<i64>,
    ) -> Result<()> {
        let payload: WsPayload = serde_json::from_str(text)?;

        match payload.op {
            op_codes::HELLO => {
                log::info!("[qqbot] Received Hello");

                // 获取心跳间隔
                let heartbeat_interval = payload
                    .data
                    .as_ref()
                    .and_then(|d| d.get("heartbeat_interval"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(30000);

                log::info!("[qqbot] Heartbeat interval: {}ms", heartbeat_interval);

                // 发送 Identify 或 Resume
                let token = self.api.get_access_token().await?;

                let identify = if let Some(ref session_id) = self.session_id {
                    // Resume
                    json!({
                        "op": op_codes::RESUME,
                        "d": {
                            "token": format!("QQBot {}", token),
                            "session_id": session_id,
                            "seq": self.last_seq.unwrap_or(0),
                        }
                    })
                } else {
                    // Identify
                    json!({
                        "op": op_codes::IDENTIFY,
                        "d": {
                            "token": format!("QQBot {}", token),
                            "intents": intents::FULL,
                            "shard": [0, 1],
                        }
                    })
                };

                // 这里需要通过某种方式发送消息，暂时跳过
                log::info!("[qqbot] Sending Identify/Resume");
            }

            op_codes::DISPATCH => {
                if let Some(seq) = payload.seq {
                    self.last_seq = Some(seq);
                }

                if let Some(event_type) = payload.event_type {
                    self.handle_dispatch(&event_type, payload.data).await?;
                }
            }

            op_codes::HEARTBEAT_ACK => {
                log::debug!("[qqbot] Heartbeat ACK");
            }

            op_codes::RECONNECT => {
                log::info!("[qqbot] Server requested reconnect");
            }

            op_codes::INVALID_SESSION => {
                log::error!("[qqbot] Invalid session");
                self.session_id = None;
                self.last_seq = None;
            }

            _ => {
                log::debug!("[qqbot] Unknown op code: {}", payload.op);
            }
        }

        Ok(())
    }

    /// 处理 Dispatch 事件
    async fn handle_dispatch(
        &mut self,
        event_type: &str,
        data: Option<serde_json::Value>,
    ) -> Result<()> {
        match event_type {
            event_types::READY => {
                if let Some(data) = data {
                    if let Some(session_id) = data.get("session_id").and_then(|v| v.as_str()) {
                        self.session_id = Some(session_id.to_string());
                        log::info!("[qqbot] Ready with session: {}", session_id);

                        let _ = self
                            .event_tx
                            .send(GatewayEvent::Ready {
                                session_id: session_id.to_string(),
                            })
                            .await;
                    }
                }
            }

            event_types::RESUMED => {
                log::info!("[qqbot] Session resumed");
                let _ = self.event_tx.send(GatewayEvent::Resumed).await;
            }

            event_types::C2C_MESSAGE_CREATE => {
                if let Some(data) = data {
                    let event: C2CMessageEvent = serde_json::from_value(data)?;
                    let msg = ReceivedMessage {
                        id: event.id,
                        content: event.content,
                        sender_id: event.author.user_openid.clone(),
                        sender_name: Some(event.author.user_openid),
                        timestamp: event.timestamp,
                        chat_type: TargetType::C2C,
                        group_id: None,
                        channel_id: None,
                        attachments: event.attachments,
                    };
                    let _ = self.event_tx.send(GatewayEvent::Message(msg)).await;
                }
            }

            event_types::GROUP_AT_MESSAGE_CREATE => {
                if let Some(data) = data {
                    let event: GroupMessageEvent = serde_json::from_value(data)?;
                    let msg = ReceivedMessage {
                        id: event.id,
                        content: event.content,
                        sender_id: event.author.member_openid.clone(),
                        sender_name: Some(event.author.member_openid),
                        timestamp: event.timestamp,
                        chat_type: TargetType::Group,
                        group_id: Some(event.group_openid.clone()),
                        channel_id: None,
                        attachments: event.attachments,
                    };
                    let _ = self.event_tx.send(GatewayEvent::Message(msg)).await;
                }
            }

            event_types::AT_MESSAGE_CREATE => {
                if let Some(data) = data {
                    let event: GuildMessageEvent = serde_json::from_value(data)?;
                    let msg = ReceivedMessage {
                        id: event.id,
                        content: event.content,
                        sender_id: event.author.id.clone(),
                        sender_name: Some(event.author.username.clone()),
                        timestamp: event.timestamp,
                        chat_type: TargetType::Channel,
                        group_id: None,
                        channel_id: Some(event.channel_id),
                        attachments: event.attachments,
                    };
                    let _ = self.event_tx.send(GatewayEvent::Message(msg)).await;
                }
            }

            _ => {
                log::debug!("[qqbot] Unknown event type: {}", event_type);
            }
        }

        Ok(())
    }
}

/// 启动 Gateway（简化版）
pub async fn start_gateway(
    api: Arc<QQBotApi>,
    event_tx: mpsc::Sender<GatewayEvent>,
) -> Result<()> {
    let config = GatewayConfig::default();
    let mut connection = GatewayConnection::new(api, config, event_tx);
    connection.start().await
}
