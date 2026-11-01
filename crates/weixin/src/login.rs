//! 二维码登录

use anyhow::{anyhow, Result};
use colored::Colorize;
use serde::Serialize;
use std::time::{Duration, Instant};
use tokio::time::sleep;

use crate::account::AccountManager;
use crate::client::QRLoginClient;
use crate::types::*;

/// 登录事件（用于流式输出）
#[derive(Debug, Clone, Serialize)]
pub enum QRLoginEvent {
    /// 二维码已生成
    QrCode {
        /// 二维码图片 URL
        qr_url: String,
        /// 二维码标识（用于轮询）
        qr_code: String,
    },
    /// 用户已扫码
    Scanned,
    /// 二维码已过期，正在刷新
    Expired { refresh_count: i32, max_refresh: i32 },
    /// 二维码已刷新
    QrRefreshed {
        qr_url: String,
        qr_code: String,
        refresh_count: i32,
    },
    /// 登录成功
    Success {
        account_id: String,
        user_id: Option<String>,
    },
    /// 登录失败
    Failed { message: String },
}

/// 二维码登录结果
#[derive(Debug, Clone)]
pub struct QRLoginResult {
    pub success: bool,
    pub account_id: Option<String>,
    pub token: Option<String>,
    pub base_url: Option<String>,
    pub user_id: Option<String>,
    pub message: String,
}

/// 二维码登录器
pub struct QRLogin {
    client: QRLoginClient,
    account_manager: AccountManager,
    bot_type: String,
}

impl QRLogin {
    /// 创建新的二维码登录器（使用默认 state_dir）
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        Ok(Self {
            client: QRLoginClient::new(base_url),
            account_manager: AccountManager::new()?,
            bot_type: DEFAULT_BOT_TYPE.to_string(),
        })
    }

    /// 创建新的二维码登录器（使用自定义 account_manager）
    pub fn with_account_manager(base_url: impl Into<String>, account_manager: AccountManager) -> Result<Self> {
        Ok(Self {
            client: QRLoginClient::new(base_url),
            account_manager,
            bot_type: DEFAULT_BOT_TYPE.to_string(),
        })
    }

    /// 设置机器人类型
    pub fn with_bot_type(mut self, bot_type: impl Into<String>) -> Self {
        self.bot_type = bot_type.into();
        self
    }

    /// 开始登录流程（带事件回调，支持流式输出）
    pub async fn login_with_callback<F>(&self, timeout_ms: u64, mut on_event: F) -> Result<QRLoginResult>
    where
        F: FnMut(QRLoginEvent) + Send,
    {
        // 获取二维码
        let qr_response = match self.client.fetch_qr_code(&self.bot_type).await {
            Ok(resp) => resp,
            Err(e) => {
                let msg = format!("获取二维码失败: {}", e);
                on_event(QRLoginEvent::Failed { message: msg.clone() });
                return Ok(QRLoginResult {
                    success: false,
                    account_id: None,
                    token: None,
                    base_url: None,
                    user_id: None,
                    message: msg,
                });
            }
        };

        // 发送二维码事件
        on_event(QRLoginEvent::QrCode {
            qr_url: qr_response.qrcode_img_content.clone(),
            qr_code: qr_response.qrcode.clone(),
        });

        // 等待扫码结果
        let result = self.wait_for_login_with_callback(&qr_response.qrcode, timeout_ms, &mut on_event).await;

        // 如果登录成功，保存账号信息
        if result.success {
            if let (Some(account_id), Some(token)) = (&result.account_id, &result.token) {
                let normalized_id = self.account_manager.normalize_account_id(account_id);
                let account_data = AccountData {
                    token: Some(token.clone()),
                    saved_at: Some(chrono::Utc::now().to_rfc3339()),
                    base_url: result.base_url.clone(),
                    user_id: result.user_id.clone(),
                    route_tag: None,
                };

                if let Err(e) = self.account_manager.save_account(&normalized_id, &account_data) {
                    eprintln!("{} 保存账号数据失败: {}", "⚠️".yellow(), e);
                } else if let Err(e) = self.account_manager.register_account_id(&normalized_id) {
                    eprintln!("{} 注册账号失败: {}", "⚠️".yellow(), e);
                }
            }
        }

        Ok(result)
    }

    /// 开始登录流程（传统终端输出）
    pub async fn login(&self, timeout_ms: u64) -> Result<QRLoginResult> {
        println!("{}", "正在启动微信扫码登录...".cyan());

        // 获取二维码
        let qr_response = match self.client.fetch_qr_code(&self.bot_type).await {
            Ok(resp) => resp,
            Err(e) => {
                return Ok(QRLoginResult {
                    success: false,
                    account_id: None,
                    token: None,
                    base_url: None,
                    user_id: None,
                    message: format!("获取二维码失败: {}", e),
                });
            }
        };

        // 显示二维码
        println!("\n{}", "使用微信扫描以下二维码，以完成连接：".cyan());
        println!();

        // 尝试在终端显示二维码
        if let Err(e) = self.display_qr_code(&qr_response.qrcode_img_content) {
            println!("{} 无法显示二维码: {}", "⚠️".yellow(), e);
            println!("请用浏览器打开以下链接扫码：");
            println!("{}", qr_response.qrcode_img_content.bright_blue());
        }

        println!();
        println!("{}", "等待连接结果...".cyan());

        // 等待扫码结果
        let result = self.wait_for_login(&qr_response.qrcode, timeout_ms).await;

        // 如果登录成功，保存账号信息
        if result.success {
            if let (Some(account_id), Some(token)) = (&result.account_id, &result.token) {
                let normalized_id = self.account_manager.normalize_account_id(account_id);
                let account_data = AccountData {
                    token: Some(token.clone()),
                    saved_at: Some(chrono::Utc::now().to_rfc3339()),
                    base_url: result.base_url.clone(),
                    user_id: result.user_id.clone(),
                    route_tag: None,
                };

                if let Err(e) = self.account_manager.save_account(&normalized_id, &account_data) {
                    eprintln!("{} 保存账号数据失败: {}", "⚠️".yellow(), e);
                } else if let Err(e) = self.account_manager.register_account_id(&normalized_id) {
                    eprintln!("{} 注册账号失败: {}", "⚠️".yellow(), e);
                }
            }
        }

        Ok(result)
    }

    /// 在终端显示二维码
    fn display_qr_code(&self, qr_content: &str) -> Result<()> {
        // 尝试使用 qr2term 显示二维码
        if qr_content.starts_with("http") {
            // 如果是 URL，使用 qr2term 生成二维码
            qr2term::print_qr(qr_content)?;
        } else {
            // 否则直接打印内容
            println!("{}", qr_content);
        }
        Ok(())
    }

    /// 等待登录结果（带事件回调）
    async fn wait_for_login_with_callback<F>(&self, qr_code: &str, timeout_ms: u64, on_event: &mut F) -> QRLoginResult
    where
        F: FnMut(QRLoginEvent) + Send,
    {
        let timeout = Duration::from_millis(timeout_ms.max(60_000));
        let deadline = Instant::now() + timeout;
        let mut scanned_printed = false;
        let mut refresh_count = 0;
        const MAX_REFRESH: i32 = 3;

        let mut current_qr_code = qr_code.to_string();

        while Instant::now() < deadline {
            match self.client.poll_qr_status(&current_qr_code).await {
                Ok(status) => {
                    match status.status.as_str() {
                        "wait" => {
                            // 继续等待，静默
                        }
                        "scaned" => {
                            if !scanned_printed {
                                on_event(QRLoginEvent::Scanned);
                                scanned_printed = true;
                            }
                        }
                        "expired" => {
                            refresh_count += 1;
                            if refresh_count > MAX_REFRESH {
                                let msg = "登录超时: 二维码多次过期".to_string();
                                on_event(QRLoginEvent::Failed { message: msg.clone() });
                                return QRLoginResult {
                                    success: false,
                                    account_id: None,
                                    token: None,
                                    base_url: None,
                                    user_id: None,
                                    message: msg,
                                };
                            }

                            on_event(QRLoginEvent::Expired {
                                refresh_count,
                                max_refresh: MAX_REFRESH,
                            });

                            // 刷新二维码
                            match self.client.fetch_qr_code(&self.bot_type).await {
                                Ok(new_qr) => {
                                    current_qr_code = new_qr.qrcode.clone();
                                    scanned_printed = false;
                                    on_event(QRLoginEvent::QrRefreshed {
                                        qr_url: new_qr.qrcode_img_content,
                                        qr_code: new_qr.qrcode,
                                        refresh_count,
                                    });
                                }
                                Err(e) => {
                                    let msg = format!("刷新二维码失败: {}", e);
                                    on_event(QRLoginEvent::Failed { message: msg.clone() });
                                    return QRLoginResult {
                                        success: false,
                                        account_id: None,
                                        token: None,
                                        base_url: None,
                                        user_id: None,
                                        message: msg,
                                    };
                                }
                            }
                        }
                        "confirmed" => {
                            let account_id_raw = status.ilink_bot_id;
                            let token = status.bot_token;
                            let base_url = status.baseurl;
                            let user_id = status.ilink_user_id.clone();

                            if account_id_raw.is_none() {
                                let msg = "登录失败: 服务器未返回 ilink_bot_id".to_string();
                                on_event(QRLoginEvent::Failed { message: msg.clone() });
                                return QRLoginResult {
                                    success: false,
                                    account_id: None,
                                    token: None,
                                    base_url: None,
                                    user_id: None,
                                    message: msg,
                                };
                            }

                            // 规范化 account_id，与 list 命令显示的一致
                            let account_id = account_id_raw.map(|id| self.account_manager.normalize_account_id(&id));

                            on_event(QRLoginEvent::Success {
                                account_id: account_id.clone().unwrap(),
                                user_id,
                            });

                            return QRLoginResult {
                                success: true,
                                account_id,
                                token,
                                base_url,
                                user_id: status.ilink_user_id,
                                message: "✅ 与微信连接成功!".to_string(),
                            };
                        }
                        _ => {
                            // 未知状态
                        }
                    }
                }
                Err(e) => {
                    if e.to_string().contains("timeout") {
                        // 超时是正常的，继续轮询
                        continue;
                    }
                    let msg = format!("轮询登录状态失败: {}", e);
                    on_event(QRLoginEvent::Failed { message: msg.clone() });
                    return QRLoginResult {
                        success: false,
                        account_id: None,
                        token: None,
                        base_url: None,
                        user_id: None,
                        message: msg,
                    };
                }
            }

            sleep(Duration::from_secs(1)).await;
        }

        let msg = "登录超时,请重试".to_string();
        on_event(QRLoginEvent::Failed { message: msg.clone() });
        QRLoginResult {
            success: false,
            account_id: None,
            token: None,
            base_url: None,
            user_id: None,
            message: msg,
        }
    }

    /// 等待登录结果
    async fn wait_for_login(&self, qr_code: &str, timeout_ms: u64) -> QRLoginResult {
        let timeout = Duration::from_millis(timeout_ms.max(60_000));
        let deadline = Instant::now() + timeout;
        let mut scanned_printed = false;
        let mut refresh_count = 0;
        const MAX_REFRESH: i32 = 3;

        let mut current_qr_code = qr_code.to_string();

        while Instant::now() < deadline {
            match self.client.poll_qr_status(&current_qr_code).await {
                Ok(status) => {
                    match status.status.as_str() {
                        "wait" => {
                            // 继续等待
                            print!(".");
                            std::io::Write::flush(&mut std::io::stdout()).ok();
                        }
                        "scaned" => {
                            if !scanned_printed {
                                println!("\n{} {}", "👀".cyan(), "已扫码，在微信继续操作...".green());
                                scanned_printed = true;
                            }
                        }
                        "expired" => {
                            refresh_count += 1;
                            if refresh_count > MAX_REFRESH {
                                return QRLoginResult {
                                    success: false,
                                    account_id: None,
                                    token: None,
                                    base_url: None,
                                    user_id: None,
                                    message: "登录超时: 二维码多次过期".to_string(),
                                };
                            }

                            println!();
                            println!("{} 二维码已过期,正在刷新...({}/{})\n", "⏳".yellow(), refresh_count, MAX_REFRESH);

                            // 刷新二维码
                            match self.client.fetch_qr_code(&self.bot_type).await {
                                Ok(new_qr) => {
                                    current_qr_code = new_qr.qrcode;
                                    scanned_printed = false;
                                    println!("{} {}", "🔄".cyan(), "新二维码已生成，请重新扫描".green());
                                    if let Err(_) = self.display_qr_code(&new_qr.qrcode_img_content) {
                                        println!("请用浏览器打开以下链接扫码：");
                                        println!("{}", new_qr.qrcode_img_content.bright_blue());
                                    }
                                }
                                Err(e) => {
                                    return QRLoginResult {
                                        success: false,
                                        account_id: None,
                                        token: None,
                                        base_url: None,
                                        user_id: None,
                                        message: format!("刷新二维码失败: {}", e),
                                    };
                                }
                            }
                        }
                        "confirmed" => {
                            let account_id_raw = status.ilink_bot_id;
                            let token = status.bot_token;
                            let base_url = status.baseurl;
                            let user_id = status.ilink_user_id;

                            if account_id_raw.is_none() {
                                return QRLoginResult {
                                    success: false,
                                    account_id: None,
                                    token: None,
                                    base_url: None,
                                    user_id: None,
                                    message: "登录失败: 服务器未返回 ilink_bot_id".to_string(),
                                };
                            }

                            // 规范化 account_id，与 list 命令显示的一致
                            let account_id = account_id_raw.map(|id| self.account_manager.normalize_account_id(&id));

                            return QRLoginResult {
                                success: true,
                                account_id,
                                token,
                                base_url,
                                user_id,
                                message: "✅ 与微信连接成功!".to_string(),
                            };
                        }
                        _ => {
                            // 未知状态
                        }
                    }
                }
                Err(e) => {
                    if e.to_string().contains("timeout") {
                        // 超时是正常的，继续轮询
                        continue;
                    }
                    return QRLoginResult {
                        success: false,
                        account_id: None,
                        token: None,
                        base_url: None,
                        user_id: None,
                        message: format!("轮询登录状态失败: {}", e),
                    };
                }
            }

            sleep(Duration::from_secs(1)).await;
        }

        QRLoginResult {
            success: false,
            account_id: None,
            token: None,
            base_url: None,
            user_id: None,
            message: "登录超时,请重试".to_string(),
        }
    }
}

/// 快速登录（带默认超时）
pub async fn quick_login(base_url: impl Into<String>) -> Result<QRLoginResult> {
    let login = QRLogin::new(base_url)?;
    login.login(480_000).await
}
