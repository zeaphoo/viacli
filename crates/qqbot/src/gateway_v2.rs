//! WebSocket Gateway 连接管理 V2
//! 完整实现：心跳、重连、Session 恢复

use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::time::{interval, sleep, timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message, WebSocketStream};
use tokio_tungstenite::MaybeTlsStream;

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
    Disconnected { code: Option<u16>, reason: String },
    /// 错误
    Error(String),
    /// 心跳 ACK
    HeartbeatAck,
}

/// Gateway 连接状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Identifying,
    Ready,
    Resuming,
}

/// Gateway 配置
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// 重连延迟（毫秒）- 指数退避
    pub reconnect_delays: Vec<u64>,
    /// 最大重连次数
    pub max_reconnect_attempts: usize,
    /// 心跳超时（毫秒）
    pub heartbeat_timeout: u64,
    /// 连接超时（毫秒）
    pub connect_timeout: u64,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            reconnect_delays: vec![1000, 2000, 5000, 10000, 30000, 60000],
            max_reconnect_attempts: 100,
            heartbeat_timeout: 30000,
            connect_timeout: 30000,
        }
    }
}

/// 共享的 Gateway 状态
struct SharedState {
    session_id: Option<String>,
    last_seq: Option<i64>,
    heartbeat_interval: u64,
    last_heartbeat_ack: Option<Instant>,
    state: ConnectionState,
}

/// Gateway 客户端
pub struct GatewayClient {
    api: Arc<QQBotApi>,
    config: GatewayConfig,
    event_tx: mpsc::Sender<GatewayEvent>,
    state: Arc<RwLock<SharedState>>,
    ws_tx: Arc<Mutex<Option<mpsc::Sender<Message>>>>,
    shutdown_tx: Arc<Mutex<Option<mpsc::Sender<()>>>>,
}

impl GatewayClient {
    /// 创建新的 Gateway 客户端
    pub fn new(
        api: Arc<QQBotApi>,
        config: GatewayConfig,
        event_tx: mpsc::Sender<GatewayEvent>,
    ) -> Self {
        Self {
            api,
            config,
            event_tx,
            state: Arc::new(RwLock::new(SharedState {
                session_id: None,
                last_seq: None,
                heartbeat_interval: 30000,
                last_heartbeat_ack: None,
                state: ConnectionState::Disconnected,
            })),
            ws_tx: Arc::new(Mutex::new(None)),
            shutdown_tx: Arc::new(Mutex::new(None)),
        }
    }

    /// 启动 Gateway（带自动重连）
    pub async fn start(&self) -> Result<()> {
        let mut reconnect_attempts = 0;

        loop {
            // 更新状态
            {
                let mut state = self.state.write().await;
                state.state = ConnectionState::Connecting;
            }

            match self.connect_and_run().await {
                Ok(()) => {
                    log::info!("[gateway] Connection closed normally");
                    reconnect_attempts = 0;
                }
                Err(e) => {
                    log::error!("[gateway] Connection error: {}", e);

                    // 检查是否需要重连
                    if reconnect_attempts >= self.config.max_reconnect_attempts {
                        let _ = self.event_tx.send(GatewayEvent::Error(
                            "Max reconnect attempts reached".to_string()
                        )).await;
                        return Err(anyhow!("Max reconnect attempts reached"));
                    }

                    // 计算重连延迟（指数退避）
                    let delay = self.config.reconnect_delays
                        .get(reconnect_attempts)
                        .copied()
                        .unwrap_or(60000);

                    log::info!("[gateway] Reconnecting in {}ms (attempt {}/{})",
                        delay, reconnect_attempts + 1, self.config.max_reconnect_attempts);

                    sleep(Duration::from_millis(delay)).await;
                    reconnect_attempts += 1;
                }
            }
        }
    }

    /// 停止 Gateway
    pub async fn stop(&self) {
        if let Some(tx) = self.shutdown_tx.lock().await.take() {
            let _ = tx.send(()).await;
        }
    }

    /// 连接并运行
    async fn connect_and_run(&self) -> Result<()> {
        let gateway_url = timeout(
            Duration::from_millis(self.config.connect_timeout),
            self.api.get_gateway_url()
        ).await??;

        log::info!("[gateway] Connecting to: {}", gateway_url);

        // 建立 WebSocket 连接
        let (ws_stream, _) = timeout(
            Duration::from_millis(self.config.connect_timeout),
            connect_async(&gateway_url)
        ).await??;

        log::info!("[gateway] WebSocket connected");

        let (ws_sender, mut ws_receiver) = ws_stream.split();

        // 创建通道
        let (ws_tx, mut ws_rx) = mpsc::channel::<Message>(100);
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);

        // 保存发送通道
        *self.ws_tx.lock().await = Some(ws_tx.clone());
        *self.shutdown_tx.lock().await = Some(shutdown_tx);

        // 启动发送任务
        let ws_sender = Arc::new(Mutex::new(ws_sender));
        let ws_sender_clone = ws_sender.clone();
        tokio::spawn(async move {
            while let Some(msg) = ws_rx.recv().await {
                if let Err(e) = ws_sender_clone.lock().await.send(msg).await {
                    log::error!("[gateway] Send error: {}", e);
                    break;
                }
            }
        });

        // 启动心跳任务
        let heartbeat_handle = self.start_heartbeat_task(ws_tx.clone());

        // 主循环
        loop {
            tokio::select! {
                // 接收 WebSocket 消息
                msg = ws_receiver.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            if let Err(e) = self.handle_message(&text, &ws_tx).await {
                                log::error!("[gateway] Handle message error: {}", e);
                            }
                        }
                        Some(Ok(Message::Binary(data))) => {
                            log::debug!("[gateway] Received binary: {} bytes", data.len());
                        }
                        Some(Ok(Message::Close(frame))) => {
                            let code = frame.as_ref().map(|f| f.code.into());
                            let reason = frame.as_ref()
                                .map(|f| f.reason.to_string())
                                .unwrap_or_else(|| "Unknown".to_string());
                            log::info!("[gateway] Connection closed: {:?} - {}", code, reason);
                            let _ = self.event_tx.send(GatewayEvent::Disconnected { code, reason }).await;
                            break;
                        }
                        Some(Ok(Message::Ping(data))) => {
                            let _ = ws_tx.send(Message::Pong(data)).await;
                        }
                        Some(Ok(Message::Pong(_))) => {
                            // 更新心跳 ACK 时间
                            self.state.write().await.last_heartbeat_ack = Some(Instant::now());
                        }
                        Some(Err(e)) => {
                            log::error!("[gateway] WebSocket error: {}", e);
                            let _ = self.event_tx.send(GatewayEvent::Error(e.to_string())).await;
                            break;
                        }
                        None => {
                            log::info!("[gateway] WebSocket stream ended");
                            break;
                        }
                        _ => {}
                    }
                }
                // 关闭信号
                _ = shutdown_rx.recv() => {
                    log::info!("[gateway] Shutdown signal received");
                    break;
                }
            }
        }

        // 清理
        drop(ws_tx);
        heartbeat_handle.abort();

        Ok(())
    }

    /// 启动心跳任务
    fn start_heartbeat_task(
        &self,
        ws_tx: mpsc::Sender<Message>,
    ) -> tokio::task::AbortHandle {
        let state = self.state.clone();

        tokio::spawn(async move {
            loop {
                // 获取心跳间隔
                let interval_ms = {
                    let state = state.read().await;
                    state.heartbeat_interval
                };

                sleep(Duration::from_millis(interval_ms)).await;

                // 发送心跳
                let seq = {
                    let state = state.read().await;
                    state.last_seq
                };

                let payload = json!({
                    "op": op_codes::HEARTBEAT,
                    "d": seq,
                });

                if let Err(e) = ws_tx.send(Message::Text(payload.to_string())).await {
                    log::error!("[gateway] Heartbeat send error: {}", e);
                    break;
                }

                log::debug!("[gateway] Heartbeat sent with seq: {:?}", seq);
            }
        }).abort_handle()
    }

    /// 处理消息
    async fn handle_message(
        &self,
        text: &str,
        ws_tx: &mpsc::Sender<Message>,
    ) -> Result<()> {
        let payload: WsPayload = serde_json::from_str(text)?;

        match payload.op {
            op_codes::HELLO => {
                self.handle_hello(payload, ws_tx).await?;
            }

            op_codes::DISPATCH => {
                // 更新 seq
                if let Some(seq) = payload.seq {
                    self.state.write().await.last_seq = Some(seq);
                }

                if let Some(event_type) = payload.event_type {
                    self.handle_dispatch(&event_type, payload.data).await?;
                }
            }

            op_codes::HEARTBEAT_ACK => {
                self.state.write().await.last_heartbeat_ack = Some(Instant::now());
                let _ = self.event_tx.send(GatewayEvent::HeartbeatAck).await;
                log::debug!("[gateway] Heartbeat ACK received");
            }

            op_codes::RECONNECT => {
                log::info!("[gateway] Server requested reconnect");
                return Err(anyhow!("Server requested reconnect"));
            }

            op_codes::INVALID_SESSION => {
                log::error!("[gateway] Invalid session");
                // 清除 session，下次重新 Identify
                let mut state = self.state.write().await;
                state.session_id = None;
                state.last_seq = None;
                return Err(anyhow!("Invalid session"));
            }

            _ => {
                log::debug!("[gateway] Unknown op code: {}", payload.op);
            }
        }

        Ok(())
    }

    /// 处理 Hello
    async fn handle_hello(
        &self,
        payload: WsPayload,
        ws_tx: &mpsc::Sender<Message>,
    ) -> Result<()> {
        log::info!("[gateway] Received Hello");

        // 获取心跳间隔
        if let Some(data) = payload.data {
            if let Some(interval) = data.get("heartbeat_interval").and_then(|v| v.as_u64()) {
                self.state.write().await.heartbeat_interval = interval;
                log::info!("[gateway] Heartbeat interval: {}ms", interval);
            }
        }

        // 获取 Token
        let token = self.api.get_access_token().await?;

        // 检查是否有 session 可以恢复
        let (session_id, last_seq) = {
            let state = self.state.read().await;
            (state.session_id.clone(), state.last_seq)
        };

        if let Some(session_id) = session_id {
            // 尝试 Resume
            log::info!("[gateway] Attempting to resume session: {}", session_id);
            let resume = json!({
                "op": op_codes::RESUME,
                "d": {
                    "token": format!("QQBot {}", token),
                    "session_id": session_id,
                    "seq": last_seq.unwrap_or(0),
                }
            });
            ws_tx.send(Message::Text(resume.to_string())).await?;
            self.state.write().await.state = ConnectionState::Resuming;
        } else {
            // 发送 Identify
            log::info!("[gateway] Sending Identify");
            let identify = json!({
                "op": op_codes::IDENTIFY,
                "d": {
                    "token": format!("QQBot {}", token),
                    "intents": intents::FULL,
                    "shard": [0, 1],
                }
            });
            ws_tx.send(Message::Text(identify.to_string())).await?;
            self.state.write().await.state = ConnectionState::Identifying;
        }

        Ok(())
    }

    /// 处理 Dispatch 事件
    async fn handle_dispatch(
        &self,
        event_type: &str,
        data: Option<serde_json::Value>,
    ) -> Result<()> {
        match event_type {
            event_types::READY => {
                if let Some(data) = data {
                    if let Some(session_id) = data.get("session_id").and_then(|v| v.as_str()) {
                        // 保存 session_id
                        self.state.write().await.session_id = Some(session_id.to_string());
                        self.state.write().await.state = ConnectionState::Ready;

                        log::info!("[gateway] Ready with session: {}", session_id);

                        let _ = self.event_tx.send(GatewayEvent::Ready {
                            session_id: session_id.to_string(),
                        }).await;
                    }
                }
            }

            event_types::RESUMED => {
                self.state.write().await.state = ConnectionState::Ready;
                log::info!("[gateway] Session resumed");
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

            event_types::DIRECT_MESSAGE_CREATE => {
                if let Some(data) = data {
                    let event: GuildMessageEvent = serde_json::from_value(data)?;
                    let msg = ReceivedMessage {
                        id: event.id,
                        content: event.content,
                        sender_id: event.author.id.clone(),
                        sender_name: Some(event.author.username.clone()),
                        timestamp: event.timestamp,
                        chat_type: TargetType::C2C, // 私信按 C2C 处理
                        group_id: None,
                        channel_id: None,
                        attachments: event.attachments,
                    };
                    let _ = self.event_tx.send(GatewayEvent::Message(msg)).await;
                }
            }

            _ => {
                log::debug!("[gateway] Unknown event type: {}", event_type);
            }
        }

        Ok(())
    }

    /// 获取当前 session_id
    pub async fn get_session_id(&self) -> Option<String> {
        self.state.read().await.session_id.clone()
    }

    /// 获取当前 seq
    pub async fn get_last_seq(&self) -> Option<i64> {
        self.state.read().await.last_seq
    }

    /// 获取连接状态
    pub async fn get_state(&self) -> ConnectionState {
        self.state.read().await.state
    }
}

/// 启动 Gateway
pub async fn start_gateway(
    api: Arc<QQBotApi>,
    event_tx: mpsc::Sender<GatewayEvent>,
) -> Result<()> {
    let config = GatewayConfig::default();
    let client = GatewayClient::new(api, config, event_tx);
    client.start().await
}
