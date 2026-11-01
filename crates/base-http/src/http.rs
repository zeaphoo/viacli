//! 阻塞 HTTP 客户端：超时、UA、响应体积上限、可重试退避。
//!
//! 采集器是同步程序，不引入 async 运行时；因此用 `reqwest::blocking`。
//! 这里只解决"每个抓取型采集器都要重复写的那点事"：
//!
//! * **响应体积上限**：先限流再读，避免一个几百 MB 的错误页把内存撑爆；
//! * **可重试分类**：网络/5xx/429 退避重试，401/403 立刻以 `auth` 失败（不要反复打上游）；
//! * **退避 + 抖动**：多个采集器/多台机器同时重试不会形成脉冲。

use std::io::Read;
use std::time::Duration;

use crate::protocol::ErrorKind;

/// HTTP 客户端参数（来自采集器配置，值都已在配置校验阶段收敛）。
#[derive(Debug, Clone)]
pub struct HttpConfig {
    /// 单次请求总超时。
    pub timeout: Duration,
    /// 连接建立超时（一般收得比总超时更短）。
    pub connect_timeout: Duration,
    /// 出站 User-Agent。**必须**是可识别、可联系的值，别用库默认值。
    pub user_agent: String,
    /// 单次响应允许的最大字节数。
    pub max_body_bytes: usize,
    /// 可重试错误的最大重试次数（不含首次）。
    pub max_retries: u32,
    /// 退避基准。
    pub base_backoff: Duration,
    /// 退避上限。
    pub max_backoff: Duration,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15),
            connect_timeout: Duration::from_secs(8),
            user_agent: "viacli-collector/0.1.0".to_string(),
            max_body_bytes: 16 * 1024 * 1024,
            max_retries: 2,
            base_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(10),
        }
    }
}

/// HTTP 失败。`kind` 遵循协议 §6.1；`retryable` 决定是否值得重试。
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct HttpError {
    pub kind: ErrorKind,
    pub retryable: bool,
    pub message: String,
}

impl HttpError {
    pub fn new(kind: ErrorKind, retryable: bool, message: impl Into<String>) -> Self {
        Self {
            kind,
            retryable,
            message: message.into(),
        }
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Network, true, message)
    }

    pub fn upstream(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Upstream, true, message)
    }

    pub fn auth(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Auth, false, message)
    }

    pub fn parse(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Parse, false, message)
    }

    pub fn config(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Config, false, message)
    }
}

/// 阻塞 HTTP 客户端。内部 `reqwest::blocking::Client` 可跨线程共享。
pub struct HttpClient {
    client: reqwest::blocking::Client,
    cfg: HttpConfig,
}

impl HttpClient {
    pub fn new(cfg: HttpConfig) -> Result<Self, HttpError> {
        let client = reqwest::blocking::Client::builder()
            .user_agent(cfg.user_agent.clone())
            .timeout(cfg.timeout)
            .connect_timeout(cfg.connect_timeout)
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .map_err(|e| HttpError::config(format!("创建 HTTP 客户端失败: {e}")))?;
        Ok(Self { client, cfg })
    }

    pub fn config(&self) -> &HttpConfig {
        &self.cfg
    }

    /// GET 一个 JSON 文档。
    pub fn get_json(&self, url: &str) -> Result<serde_json::Value, HttpError> {
        self.get_json_with(url, &[])
    }

    /// 带额外请求头 GET 一个 JSON 文档（如 NWS 需要 `Accept: application/geo+json`）。
    pub fn get_json_with(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<serde_json::Value, HttpError> {
        let bytes = self.get_bytes_with(url, headers)?;
        serde_json::from_slice(&bytes)
            .map_err(|e| HttpError::parse(format!("解析 {url} JSON 失败: {e}")))
    }

    /// GET 原始字节（调用方自己解析）。
    pub fn get_bytes(&self, url: &str) -> Result<Vec<u8>, HttpError> {
        self.get_bytes_with(url, &[])
    }

    pub fn get_bytes_with(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<Vec<u8>, HttpError> {
        let mut attempt = 0u32;
        loop {
            match self.try_once(url, headers) {
                Ok(bytes) => return Ok(bytes),
                Err(e) if e.retryable && attempt < self.cfg.max_retries => {
                    attempt += 1;
                    std::thread::sleep(self.backoff(attempt));
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn try_once(&self, url: &str, headers: &[(&str, &str)]) -> Result<Vec<u8>, HttpError> {
        let mut req = self.client.get(url);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = req
            .send()
            .map_err(|e| classify_reqwest(&e, url))?;

        let status = resp.status();
        if !status.is_success() {
            return Err(classify_status(status.as_u16(), url));
        }

        // 先限流再读：错误页往往很大，读完再判大小就晚了。
        let limit = self.cfg.max_body_bytes as u64;
        let mut body = Vec::new();
        resp.take(limit + 1)
            .read_to_end(&mut body)
            .map_err(|e| HttpError::network(format!("读取 {url} 响应失败: {e}")))?;
        if body.len() as u64 > limit {
            return Err(HttpError::upstream(format!(
                "{url} 响应超过 {} 字节上限，已放弃",
                self.cfg.max_body_bytes
            )));
        }
        Ok(body)
    }

    /// 指数退避 + 抖动。抖动用时间纳秒的低位，避免额外依赖 `rand`。
    fn backoff(&self, attempt: u32) -> Duration {
        let base = self.cfg.base_backoff.as_millis() as u64;
        let cap = self.cfg.max_backoff.as_millis() as u64;
        let exp = base.saturating_mul(1u64 << attempt.min(6));
        let capped = exp.min(cap);
        let jitter = pseudo_jitter_ms(capped);
        Duration::from_millis(capped + jitter)
    }
}

/// 把状态码分类成协议错误类别。**注意**：`upstream` 默认可重试，
/// 但 4xx（除 429）重试也不会变好，所以显式降级为不可重试。
pub fn classify_status(status: u16, url: &str) -> HttpError {
    if status == 401 || status == 403 {
        return HttpError::auth(format!("HTTP {status} {url}（凭据无效或无权访问）"));
    }
    if status == 429 {
        return HttpError::upstream(format!("HTTP 429 {url}（被限流，稍后重试）"));
    }
    if (500..600).contains(&status) {
        return HttpError::upstream(format!("HTTP {status} {url}（上游错误，可重试）"));
    }
    // 其它 4xx：配置/请求本身有问题，重试无意义。
    HttpError::new(
        ErrorKind::Upstream,
        false,
        format!("HTTP {status} {url}"),
    )
}

fn classify_reqwest(e: &reqwest::Error, url: &str) -> HttpError {
    if e.is_timeout() || e.is_connect() || e.is_request() {
        HttpError::network(format!("请求 {url} 失败: {e}"))
    } else {
        HttpError::upstream(format!("请求 {url} 失败: {e}"))
    }
}

/// 0..=capped/4 毫秒的确定性抖动（同一次退避内不重复调用同一纳秒，实际表现为随机）。
fn pseudo_jitter_ms(capped: u64) -> u64 {
    if capped == 0 {
        return 0;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let span = (capped / 4).max(1);
    nanos % span
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_classification_matches_the_spec() {
        assert_eq!(classify_status(401, "u").kind, ErrorKind::Auth);
        assert!(!classify_status(403, "u").retryable);
        assert_eq!(classify_status(429, "u").kind, ErrorKind::Upstream);
        assert!(classify_status(429, "u").retryable);
        assert!(classify_status(503, "u").retryable);
        assert!(!classify_status(400, "u").retryable, "4xx 重试无意义");
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        let c = HttpClient::new(HttpConfig {
            base_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_millis(1000),
            ..Default::default()
        })
        .unwrap();
        assert!(c.backoff(1) >= Duration::from_millis(100));
        assert!(c.backoff(10) <= Duration::from_millis(1250), "必须被 max_backoff 收敛");
    }
}
