//! QQ Bot API 封装
//! 处理 Token 获取和 API 请求

use anyhow::{anyhow, Result};
use reqwest::{Client, Response};
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::types::*;
use crate::{API_BASE, TOKEN_URL};

/// Token 缓存
#[derive(Debug, Clone)]
pub struct TokenCache {
    token: Option<String>,
    expires_at: Option<Instant>,
    app_id: String,
}

impl TokenCache {
    pub fn new(app_id: String) -> Self {
        Self {
            token: None,
            expires_at: None,
            app_id,
        }
    }

    pub fn is_valid(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            // 提前 5 分钟刷新
            Instant::now() < expires_at - Duration::from_secs(300)
        } else {
            false
        }
    }

    pub fn get_token(&self) -> Option<String> {
        if self.is_valid() {
            self.token.clone()
        } else {
            None
        }
    }

    pub fn set_token(&mut self, token: String, expires_in: u64) {
        self.token = Some(token);
        self.expires_at = Some(Instant::now() + Duration::from_secs(expires_in));
    }

    pub fn clear(&mut self) {
        self.token = None;
        self.expires_at = None;
    }
}

/// QQ Bot API 客户端
pub struct QQBotApi {
    client: Client,
    app_id: String,
    client_secret: String,
    token_cache: Arc<Mutex<TokenCache>>,
}

impl QQBotApi {
    pub fn new(app_id: &str, client_secret: &str) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            app_id: app_id.to_string(),
            client_secret: client_secret.to_string(),
            token_cache: Arc::new(Mutex::new(TokenCache::new(app_id.to_string()))),
        }
    }

    /// 获取访问令牌
    pub async fn get_access_token(&self) -> Result<String> {
        // 先检查缓存
        {
            let cache = self.token_cache.lock().unwrap();
            if let Some(token) = cache.get_token() {
                log::debug!("[qqbot] Using cached access token");
                return Ok(token);
            }
        }

        log::info!("[qqbot] Fetching new access token for app_id: {}", self.app_id);

        let body = serde_json::json!({
            "appId": self.app_id,
            "clientSecret": self.client_secret,
        });

        let response = self
            .client
            .post(TOKEN_URL)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("Failed to get access token: HTTP {} - {}", status, text));
        }

        let token_resp: AccessTokenResponse = response.json().await?;

        // 更新缓存
        {
            let mut cache = self.token_cache.lock().unwrap();
            cache.set_token(token_resp.access_token.clone(), token_resp.expires_in);
        }

        log::info!("[qqbot] Access token obtained, expires in {}s", token_resp.expires_in);

        Ok(token_resp.access_token)
    }

    /// 清除 Token 缓存
    pub fn clear_token_cache(&self) {
        let mut cache = self.token_cache.lock().unwrap();
        cache.clear();
        log::info!("[qqbot] Token cache cleared");
    }

    /// API 请求封装
    async fn api_request<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: Option<impl Serialize>,
    ) -> Result<T> {
        let token = self.get_access_token().await?;
        let url = format!("{}{}", API_BASE, path);

        log::debug!("[qqbot] API Request: {} {}", method, url);

        let mut request = self
            .client
            .request(reqwest::Method::from_bytes(method.as_bytes())?, &url)
            .header("Authorization", format!("QQBot {}", token))
            .header("Content-Type", "application/json");

        if let Some(body) = body {
            request = request.json(&body);
        }

        let response = request.send().await?;
        self.handle_response(response).await
    }

    /// 处理 API 响应
    async fn handle_response<T: DeserializeOwned>(&self, response: Response) -> Result<T> {
        let status = response.status();
        let text = response.text().await?;

        log::debug!("[qqbot] API Response: HTTP {} - {}", status, text);

        if !status.is_success() {
            // Token 过期，清除缓存
            if status.as_u16() == 401 {
                self.clear_token_cache();
            }
            return Err(anyhow!("API error: HTTP {} - {}", status, text));
        }

        let result = serde_json::from_str(&text)?;
        Ok(result)
    }

    /// 获取 Gateway URL
    pub async fn get_gateway_url(&self) -> Result<String> {
        let resp: GatewayResponse = self.api_request("GET", "/gateway", None::<()>).await?;
        Ok(resp.url)
    }

    /// 发送 C2C 消息
    pub async fn send_c2c_message(
        &self,
        openid: &str,
        content: &str,
        options: SendOptions,
    ) -> Result<MessageResponse> {
        let path = format!("/v2/users/{}/messages", openid);

        let mut body = serde_json::json!({
            "content": content,
            "msg_type": 0,
            "msg_seq": options.msg_seq.unwrap_or(1),
        });

        // 添加引用回复
        if let Some(ref_id) = options.message_reference {
            body["message_reference"] = serde_json::json!({
                "message_id": ref_id
            });
        }

        self.api_request("POST", &path, Some(body)).await
    }

    /// 发送群聊消息
    pub async fn send_group_message(
        &self,
        group_openid: &str,
        content: &str,
        options: SendOptions,
    ) -> Result<MessageResponse> {
        let path = format!("/v2/groups/{}/messages", group_openid);

        let mut body = serde_json::json!({
            "content": content,
            "msg_type": 0,
            "msg_seq": options.msg_seq.unwrap_or(1),
        });

        // 添加引用回复
        if let Some(ref_id) = options.message_reference {
            body["message_reference"] = serde_json::json!({
                "message_id": ref_id
            });
        }

        self.api_request("POST", &path, Some(body)).await
    }

    /// 发送频道消息
    pub async fn send_channel_message(
        &self,
        channel_id: &str,
        content: &str,
        options: SendOptions,
    ) -> Result<MessageResponse> {
        let path = format!("/channels/{}/messages", channel_id);

        let body = serde_json::json!({
            "content": content,
        });

        self.api_request("POST", &path, Some(body)).await
    }

    /// 发送 Markdown 消息 (C2C)
    pub async fn send_c2c_markdown(
        &self,
        openid: &str,
        markdown: &str,
        options: SendOptions,
    ) -> Result<MessageResponse> {
        let path = format!("/v2/users/{}/messages", openid);

        let body = serde_json::json!({
            "markdown": {
                "content": markdown
            },
            "msg_type": 2,
            "msg_seq": options.msg_seq.unwrap_or(1),
        });

        self.api_request("POST", &path, Some(body)).await
    }

    /// 发送 Markdown 消息 (群聊)
    pub async fn send_group_markdown(
        &self,
        group_openid: &str,
        markdown: &str,
        options: SendOptions,
    ) -> Result<MessageResponse> {
        let path = format!("/v2/groups/{}/messages", group_openid);

        let body = serde_json::json!({
            "markdown": {
                "content": markdown
            },
            "msg_type": 2,
            "msg_seq": options.msg_seq.unwrap_or(1),
        });

        self.api_request("POST", &path, Some(body)).await
    }

    /// 上传 C2C 媒体
    pub async fn upload_c2c_media(
        &self,
        openid: &str,
        file_type: MediaFileType,
        url: Option<&str>,
        file_data: Option<&str>,
        file_name: Option<&str>,
    ) -> Result<UploadMediaResponse> {
        let path = format!("/v2/users/{}/files", openid);

        let mut body = serde_json::json!({
            "file_type": file_type as i32,
            "srv_send_msg": false,
        });

        if let Some(url) = url {
            body["url"] = serde_json::json!(url);
        }

        if let Some(file_data) = file_data {
            body["file_data"] = serde_json::json!(file_data);
        }

        if let Some(file_name) = file_name {
            body["file_name"] = serde_json::json!(file_name);
        }

        self.api_request("POST", &path, Some(body)).await
    }

    /// 上传群聊媒体
    pub async fn upload_group_media(
        &self,
        group_openid: &str,
        file_type: MediaFileType,
        url: Option<&str>,
        file_data: Option<&str>,
        file_name: Option<&str>,
    ) -> Result<UploadMediaResponse> {
        let path = format!("/v2/groups/{}/files", group_openid);

        let mut body = serde_json::json!({
            "file_type": file_type as i32,
            "srv_send_msg": false,
        });

        if let Some(url) = url {
            body["url"] = serde_json::json!(url);
        }

        if let Some(file_data) = file_data {
            body["file_data"] = serde_json::json!(file_data);
        }

        if let Some(file_name) = file_name {
            body["file_name"] = serde_json::json!(file_name);
        }

        self.api_request("POST", &path, Some(body)).await
    }

    /// 发送 C2C 富媒体消息
    pub async fn send_c2c_media_message(
        &self,
        openid: &str,
        file_info: &str,
        content: Option<&str>,
        options: SendOptions,
    ) -> Result<MessageResponse> {
        let path = format!("/v2/users/{}/messages", openid);

        let mut body = serde_json::json!({
            "msg_type": 7,
            "media": {
                "file_info": file_info
            },
            "msg_seq": options.msg_seq.unwrap_or(1),
        });

        if let Some(content) = content {
            body["content"] = serde_json::json!(content);
        }

        self.api_request("POST", &path, Some(body)).await
    }

    /// 发送群聊富媒体消息
    pub async fn send_group_media_message(
        &self,
        group_openid: &str,
        file_info: &str,
        content: Option<&str>,
        options: SendOptions,
    ) -> Result<MessageResponse> {
        let path = format!("/v2/groups/{}/messages", group_openid);

        let mut body = serde_json::json!({
            "msg_type": 7,
            "media": {
                "file_info": file_info
            },
            "msg_seq": options.msg_seq.unwrap_or(1),
        });

        if let Some(content) = content {
            body["content"] = serde_json::json!(content);
        }

        self.api_request("POST", &path, Some(body)).await
    }

    /// 发送输入状态提示 (C2C)
    pub async fn send_c2c_input_notify(
        &self,
        openid: &str,
        input_second: i32,
    ) -> Result<serde_json::Value> {
        let path = format!("/v2/users/{}/messages", openid);

        let body = serde_json::json!({
            "msg_type": 6,
            "input_notify": {
                "input_type": 1,
                "input_second": input_second
            },
        });

        self.api_request("POST", &path, Some(body)).await
    }
}

/// 获取下一个消息序号 (0-65535)
pub fn get_next_msg_seq(_msg_id: &str) -> i32 {
    let time_part = (chrono::Utc::now().timestamp_millis() % 100000000) as i32;
    let random = rand::random::<i32>().abs() % 65536;
    (time_part ^ random) % 65536
}
