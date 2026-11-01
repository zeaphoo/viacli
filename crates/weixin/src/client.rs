//! 微信 iLink API 客户端

use anyhow::{anyhow, Result};
use reqwest::{Client, Response};
use serde::{de::DeserializeOwned, Serialize};
use std::time::Duration;

use crate::types::*;

/// API 客户端
pub struct WeixinApiClient {
    client: Client,
    base_url: String,
    token: Option<String>,
    route_tag: Option<String>,
    long_poll_timeout: Duration,
    api_timeout: Duration,
    config_timeout: Duration,
}

impl WeixinApiClient {
    /// 创建新的 API 客户端
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into(),
            token: None,
            route_tag: None,
            long_poll_timeout: Duration::from_millis(35_000),
            api_timeout: Duration::from_millis(15_000),
            config_timeout: Duration::from_millis(10_000),
        }
    }

    /// 设置认证 Token
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// 设置路由标签 (SKRouteTag)
    pub fn with_route_tag(mut self, route_tag: impl Into<String>) -> Self {
        self.route_tag = Some(route_tag.into());
        self
    }

    /// 设置长轮询超时
    pub fn with_long_poll_timeout(mut self, timeout_ms: u64) -> Self {
        self.long_poll_timeout = Duration::from_millis(timeout_ms);
        self
    }

    /// 设置 API 超时
    pub fn with_api_timeout(mut self, timeout_ms: u64) -> Self {
        self.api_timeout = Duration::from_millis(timeout_ms);
        self
    }

    /// 设置配置超时
    pub fn with_config_timeout(mut self, timeout_ms: u64) -> Self {
        self.config_timeout = Duration::from_millis(timeout_ms);
        self
    }

    /// 确保 URL 以斜杠结尾
    fn ensure_trailing_slash(&self, url: &str) -> String {
        if url.ends_with('/') {
            url.to_string()
        } else {
            format!("{}/", url)
        }
    }

    /// 生成随机 X-WECHAT-UIN 头部
    fn random_wechat_uin(&self) -> String {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let uint32: u32 = rng.gen();
        base64::encode(uint32.to_string())
    }

    /// 构建请求头
    fn build_headers(&self, body: &str) -> Vec<(String, String)> {
        let mut headers = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("AuthorizationType".to_string(), "ilink_bot_token".to_string()),
            (
                "Content-Length".to_string(),
                body.bytes().len().to_string(),
            ),
            ("X-WECHAT-UIN".to_string(), self.random_wechat_uin()),
        ];

        if let Some(token) = &self.token {
            headers.push(("Authorization".to_string(), format!("Bearer {}", token)));
        }

        if let Some(route_tag) = &self.route_tag {
            headers.push(("SKRouteTag".to_string(), route_tag.clone()));
        }

        headers
    }

    /// 发送 POST 请求
    async fn post<T: Serialize, R: DeserializeOwned>(
        &self,
        endpoint: &str,
        body: &T,
        timeout: Duration,
    ) -> Result<R> {
        let base = self.ensure_trailing_slash(&self.base_url);
        let url = format!("{}ilink/bot/{}", base, endpoint);
        let body_json = serde_json::to_string(body)?;
        
        // 调试日志：打印请求体
        log::debug!("API Request [{}]: {}", endpoint, body_json);
        
        let headers = self.build_headers(&body_json);

        let response = self
            .client
            .post(&url)
            .headers(headers.into_iter().map(|(k, v)| (k.parse().unwrap(), v.parse().unwrap())).collect())
            .timeout(timeout)
            .body(body_json)
            .send()
            .await?;

        self.handle_response(response).await
    }

    /// 处理响应
    async fn handle_response<R: DeserializeOwned>(&self, response: Response) -> Result<R> {
        let status = response.status();
        let text = response.text().await?;

        // 调试日志：打印响应内容
        log::debug!("API Response [{}]: {}", status, text);

        if !status.is_success() {
            return Err(anyhow!("HTTP {}: {}", status, text));
        }

        let result: R = serde_json::from_str(&text)?;
        Ok(result)
    }

    /// 获取更新（长轮询）
    pub async fn get_updates(&self, buf: impl Into<String>) -> Result<GetUpdatesResp> {
        let req = GetUpdatesReq {
            get_updates_buf: buf.into(),
            base_info: Some(BaseInfo {
                channel_version: env!("CARGO_PKG_VERSION").to_string(),
            }),
            ..Default::default()
        };

        match self.post("getupdates", &req, self.long_poll_timeout).await {
            Ok(resp) => Ok(resp),
            Err(e) => {
                // 长轮询超时是正常的，返回空响应
                if e.to_string().contains("timeout") {
                    Ok(GetUpdatesResp {
                        ret: 0,
                        msgs: Some(vec![]),
                        get_updates_buf: Some(req.get_updates_buf),
                        ..Default::default()
                    })
                } else {
                    Err(e)
                }
            }
        }
    }

    /// 发送消息
    pub async fn send_message(&self, msg: WeixinMessage) -> Result<SendMessageResp> {
        let req = SendMessageReq {
            msg: Some(msg),
            base_info: Some(BaseInfo {
                channel_version: env!("CARGO_PKG_VERSION").to_string(),
            }),
        };

        self.post("sendmessage", &req, self.api_timeout).await
    }

    /// 获取上传 URL
    pub async fn get_upload_url(&self, req: GetUploadUrlReq) -> Result<GetUploadUrlResp> {
        let req = GetUploadUrlReq {
            base_info: Some(BaseInfo {
                channel_version: env!("CARGO_PKG_VERSION").to_string(),
            }),
            ..req
        };

        self.post("getuploadurl", &req, self.api_timeout).await
    }

    /// 获取配置
    pub async fn get_config(&self, user_id: impl Into<String>, context_token: Option<String>) -> Result<GetConfigResp> {
        let req = GetConfigReq {
            ilink_user_id: user_id.into(),
            context_token,
            base_info: Some(BaseInfo {
                channel_version: env!("CARGO_PKG_VERSION").to_string(),
            }),
        };

        self.post("getconfig", &req, self.config_timeout).await
    }

    /// 发送输入状态
    pub async fn send_typing(&self, user_id: impl Into<String>, ticket: impl Into<String>, status: TypingStatus) -> Result<SendTypingResp> {
        let req = SendTypingReq {
            ilink_user_id: user_id.into(),
            typing_ticket: ticket.into(),
            status: status as i32,
            base_info: Some(BaseInfo {
                channel_version: env!("CARGO_PKG_VERSION").to_string(),
            }),
        };

        self.post("sendtyping", &req, self.config_timeout).await
    }
}

/// 二维码登录客户端
pub struct QRLoginClient {
    client: Client,
    base_url: String,
}

impl QRLoginClient {
    /// 创建新的二维码登录客户端
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.into(),
        }
    }

    /// 确保 URL 以斜杠结尾
    fn ensure_trailing_slash(&self, url: &str) -> String {
        if url.ends_with('/') {
            url.to_string()
        } else {
            format!("{}/", url)
        }
    }

    /// 获取二维码
    pub async fn fetch_qr_code(&self, bot_type: impl AsRef<str>) -> Result<QRCodeResponse> {
        let base = self.ensure_trailing_slash(&self.base_url);
        let url = format!(
            "{}ilink/bot/get_bot_qrcode?bot_type={}",
            base,
            urlencoding::encode(bot_type.as_ref())
        );

        log::debug!("QR Login Request [fetch_qr_code]: GET {}", url);

        let response = self.client.get(&url).send().await?;
        let status = response.status();
        let body = response.text().await?;

        log::debug!("QR Login Response [fetch_qr_code]: [{}] {}", status, body);

        if !status.is_success() {
            return Err(anyhow!("获取二维码失败: {} - {}", status, body));
        }

        let qr_response: QRCodeResponse = serde_json::from_str(&body)?;
        Ok(qr_response)
    }

    /// 轮询二维码状态
    pub async fn poll_qr_status(&self, qr_code: impl AsRef<str>) -> Result<StatusResponse> {
        let base = self.ensure_trailing_slash(&self.base_url);
        let url = format!(
            "{}ilink/bot/get_qrcode_status?qrcode={}",
            base,
            urlencoding::encode(qr_code.as_ref())
        );

        log::debug!("QR Login Request [poll_qr_status]: GET {}", url);

        let response = self
            .client
            .get(&url)
            .header("iLink-App-ClientVersion", "1")
            .timeout(Duration::from_millis(35_000))
            .send()
            .await?;
        let status_code = response.status();
        let body = response.text().await?;

        log::debug!("QR Login Response [poll_qr_status]: [{}] {}", status_code, body);

        if !status_code.is_success() {
            return Err(anyhow!("轮询状态失败: {} - {}", status_code, body));
        }

        let status: StatusResponse = serde_json::from_str(&body)?;
        Ok(status)
    }
}
