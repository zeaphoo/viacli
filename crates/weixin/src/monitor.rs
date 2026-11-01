//! 消息监控守护进程
//!
//! 长轮询循环：getUpdates -> 处理消息 -> 分发回复

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time::{sleep, Instant};

use crate::client::WeixinApiClient;
use crate::config_cache::{CachedConfig, ConfigCache};
use crate::session_guard::{is_session_paused, pause_session, SESSION_EXPIRED_ERRCODE};
use crate::sync_buf::SyncBufManager;
use crate::types::WeixinMessage;

/// 默认长轮询超时（35秒）
const DEFAULT_LONG_POLL_TIMEOUT_MS: u64 = 35_000;
/// 最大连续失败次数
const MAX_CONSECUTIVE_FAILURES: u32 = 3;
/// 退避延迟（30秒）
const BACKOFF_DELAY_MS: u64 = 30_000;
/// 重试延迟（2秒）
const RETRY_DELAY_MS: u64 = 2_000;

/// 监控状态
#[derive(Debug, Clone)]
pub struct MonitorState {
    pub account_id: String,
    pub running: bool,
    pub last_event_at: Option<i64>,
    pub last_inbound_at: Option<i64>,
    pub last_error: Option<String>,
    pub consecutive_failures: u32,
}

impl MonitorState {
    fn new(account_id: String) -> Self {
        Self {
            account_id,
            running: false,
            last_event_at: None,
            last_inbound_at: None,
            last_error: None,
            consecutive_failures: 0,
        }
    }
}

/// 消息处理器 trait
#[async_trait::async_trait]
pub trait MessageHandler: Send + Sync {
    /// 处理入站消息
    async fn handle_message(
        &self,
        message: &WeixinMessage,
        account_id: &str,
        config: &CachedConfig,
    ) -> anyhow::Result<()>;
}

/// 监控选项
pub struct MonitorOptions {
    pub base_url: String,
    pub cdn_base_url: String,
    pub token: Option<String>,
    pub account_id: String,
    pub long_poll_timeout_ms: Option<u64>,
    pub allow_from: Option<Vec<String>>,
}

/// 消息监控器
pub struct MessageMonitor {
    client: Arc<WeixinApiClient>,
    config_cache: Arc<ConfigCache>,
    sync_buf_manager: Arc<SyncBufManager>,
    state: Arc<RwLock<MonitorState>>,
    options: MonitorOptions,
}

impl MessageMonitor {
    /// 创建新的消息监控器
    pub fn new(options: MonitorOptions, state_dir: &std::path::Path) -> Self {
        let client = Arc::new(
            WeixinApiClient::new(&options.base_url)
                .with_token(options.token.clone().unwrap_or_default()),
        );
        
        let config_cache = Arc::new(ConfigCache::new(client.clone()));
        let sync_buf_manager = Arc::new(SyncBufManager::new(state_dir));
        let state = Arc::new(RwLock::new(MonitorState::new(options.account_id.clone())));

        Self {
            client,
            config_cache,
            sync_buf_manager,
            state,
            options,
        }
    }

    /// 启动监控循环
    pub async fn run<H: MessageHandler>(
        &self,
        handler: Arc<H>,
        mut shutdown: tokio::sync::mpsc::Receiver<()>,
    ) -> anyhow::Result<()> {
        let account_id = &self.options.account_id;
        
        log::info!("[weixin] starting monitor for account_id={}", account_id);

        // 恢复之前的 sync buf
        let mut get_updates_buf = self.sync_buf_manager.load(account_id).unwrap_or_default();
        
        if !get_updates_buf.is_empty() {
            log::info!(
                "[weixin] resuming from previous sync buf ({} bytes)",
                get_updates_buf.len()
            );
        } else {
            log::info!("[weixin] no previous sync buf, starting fresh");
        }

        let mut next_timeout_ms = self.options.long_poll_timeout_ms.unwrap_or(DEFAULT_LONG_POLL_TIMEOUT_MS);
        let mut last_success = Instant::now();

        // 更新状态为运行中
        {
            let mut state = self.state.write().await;
            state.running = true;
        }

        loop {
            // 检查关闭信号
            if let Ok(()) = shutdown.try_recv() {
                log::info!("[weixin] monitor shutting down for account_id={}", account_id);
                break;
            }

            // 检查会话是否暂停
            if is_session_paused(account_id) {
                let pause_ms = crate::session_guard::get_remaining_pause_ms(account_id);
                log::warn!(
                    "[weixin] session paused for account_id={}, sleeping {}s",
                    account_id,
                    pause_ms / 1000
                );
                sleep(Duration::from_millis(pause_ms)).await;
                continue;
            }

            // 执行 getUpdates
            match self.client.get_updates(&get_updates_buf).await {
                Ok(resp) => {
                    // 更新超时
                    if let Some(timeout) = resp.longpolling_timeout_ms {
                        if timeout > 0 {
                            next_timeout_ms = timeout as u64;
                        }
                    }

                    // 检查 API 错误
                    let is_api_error = resp.ret != 0 || resp.errcode.unwrap_or(0) != 0;
                    
                    if is_api_error {
                        let errcode = resp.errcode.unwrap_or(resp.ret);
                        
                        // 检查会话过期
                        if errcode == SESSION_EXPIRED_ERRCODE {
                            pause_session(account_id);
                            let pause_ms = crate::session_guard::get_remaining_pause_ms(account_id);
                            log::error!(
                                "[weixin] session expired (errcode {}), pausing for {} min",
                                SESSION_EXPIRED_ERRCODE,
                                pause_ms / 60_000
                            );
                            
                            {
                                let mut state = self.state.write().await;
                                state.last_error = Some(format!("session expired (errcode {})", SESSION_EXPIRED_ERRCODE));
                            }
                            
                            sleep(Duration::from_millis(pause_ms)).await;
                            continue;
                        }

                        // 其他 API 错误
                        let errmsg = resp.errmsg.clone().unwrap_or_default();
                        log::error!(
                            "[weixin] getUpdates failed: ret={} errcode={} errmsg={}",
                            resp.ret,
                            errcode,
                            errmsg
                        );

                        {
                            let mut state = self.state.write().await;
                            state.consecutive_failures += 1;
                            state.last_error = Some(format!("API error: {} (ret={})", errmsg, resp.ret));
                        }

                        // 连续失败处理
                        let failures = {
                            let state = self.state.read().await;
                            state.consecutive_failures
                        };

                        if failures >= MAX_CONSECUTIVE_FAILURES {
                            log::error!(
                                "[weixin] {} consecutive failures, backing off {}s",
                                MAX_CONSECUTIVE_FAILURES,
                                BACKOFF_DELAY_MS / 1000
                            );
                            sleep(Duration::from_millis(BACKOFF_DELAY_MS)).await;
                            {
                                let mut state = self.state.write().await;
                                state.consecutive_failures = 0;
                            }
                        } else {
                            sleep(Duration::from_millis(RETRY_DELAY_MS)).await;
                        }
                        continue;
                    }

                    // 成功响应
                    {
                        let mut state = self.state.write().await;
                        state.consecutive_failures = 0;
                        state.last_error = None;
                        state.last_event_at = Some(chrono::Utc::now().timestamp_millis());
                    }

                    // 保存新的 sync buf
                    if let Some(new_buf) = &resp.get_updates_buf {
                        if new_buf != &get_updates_buf {
                            if let Err(e) = self.sync_buf_manager.save(account_id, new_buf) {
                                log::warn!("[weixin] failed to save sync buf: {}", e);
                            }
                            get_updates_buf = new_buf.clone();
                        }
                    }

                    // 处理消息
                    let messages = resp.msgs.unwrap_or_default();
                    for msg in &messages {
                        let from_user_id = msg.from_user_id.clone().unwrap_or_default();
                        let context_token = msg.context_token.clone();

                        log::info!(
                            "[weixin] inbound message: from={} types={:?}",
                            from_user_id,
                            msg.item_list.as_ref().map(|list| {
                                list.iter().map(|i| i.r#type.clone().unwrap_or_default()).collect::<Vec<_>>()
                            })
                        );

                        // 获取用户配置
                        let config = self.config_cache.get_for_user(&from_user_id, context_token.as_deref()).await;

                        // 处理消息
                        if let Err(e) = handler.handle_message(msg, account_id, &config).await {
                            log::error!("[weixin] failed to handle message: {}", e);
                        }

                        {
                            let mut state = self.state.write().await;
                            state.last_inbound_at = Some(chrono::Utc::now().timestamp_millis());
                        }
                    }

                    last_success = Instant::now();
                }
                Err(e) => {
                    let err_str = e.to_string();
                    log::error!("[weixin] getUpdates error: {}", err_str);

                    {
                        let mut state = self.state.write().await;
                        state.consecutive_failures += 1;
                        state.last_error = Some(err_str);
                    }

                    // 连续失败处理
                    let failures = {
                        let state = self.state.read().await;
                        state.consecutive_failures
                    };

                    if failures >= MAX_CONSECUTIVE_FAILURES {
                        log::error!(
                            "[weixin] {} consecutive errors, backing off {}s",
                            MAX_CONSECUTIVE_FAILURES,
                            BACKOFF_DELAY_MS / 1000
                        );
                        sleep(Duration::from_millis(BACKOFF_DELAY_MS)).await;
                        {
                            let mut state = self.state.write().await;
                            state.consecutive_failures = 0;
                        }
                    } else {
                        sleep(Duration::from_millis(RETRY_DELAY_MS)).await;
                    }
                }
            }

            // 检查是否长时间无响应
            if last_success.elapsed() > Duration::from_secs(120) {
                log::warn!("[weixin] no successful response for 120s, may need to check connection");
            }
        }

        // 更新状态为停止
        {
            let mut state = self.state.write().await;
            state.running = false;
        }

        log::info!("[weixin] monitor stopped for account_id={}", account_id);
        Ok(())
    }

    /// 获取当前状态
    pub async fn get_state(&self) -> MonitorState {
        self.state.read().await.clone()
    }

    /// 停止监控
    pub async fn stop(&self) {
        let mut state = self.state.write().await;
        state.running = false;
    }
}

/// 默认消息处理器（打印到日志）
pub struct DefaultMessageHandler;

#[async_trait::async_trait]
impl MessageHandler for DefaultMessageHandler {
    async fn handle_message(
        &self,
        message: &WeixinMessage,
        account_id: &str,
        _config: &CachedConfig,
    ) -> anyhow::Result<()> {
        log::info!(
            "[weixin] DefaultMessageHandler: received message from={} account_id={}",
            message.from_user_id.as_ref().unwrap_or(&"unknown".to_string()),
            account_id
        );
        Ok(())
    }
}
