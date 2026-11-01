//! CDN 上传和下载功能

use anyhow::{anyhow, Result};
use reqwest::Client;
use std::time::Duration;

use crate::crypto::encrypt_aes_ecb;

/// 最大重试次数
const UPLOAD_MAX_RETRIES: u32 = 3;

/// 上传缓冲区到微信 CDN
pub async fn upload_buffer_to_cdn(
    client: &Client,
    buf: &[u8],
    upload_param: &str,
    filekey: &str,
    cdn_base_url: &str,
    aeskey: &[u8],
) -> Result<String> {
    // 加密数据
    let ciphertext = encrypt_aes_ecb(buf, aeskey);
    let cdn_url = build_cdn_upload_url(cdn_base_url, upload_param, filekey);

    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 1..=UPLOAD_MAX_RETRIES {
        match try_upload(client, &cdn_url, &ciphertext).await {
            Ok(download_param) => return Ok(download_param),
            Err(e) => {
                let err_msg = e.to_string();
                // 客户端错误 (4xx) 直接失败，不重试
                if err_msg.contains("client error") {
                    return Err(e);
                }
                last_error = Some(e);
                if attempt < UPLOAD_MAX_RETRIES {
                    tokio::time::sleep(Duration::from_millis(100 * attempt as u64)).await;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow!("CDN upload failed after {} attempts", UPLOAD_MAX_RETRIES)))
}

/// 尝试上传
async fn try_upload(client: &Client, cdn_url: &str, ciphertext: &[u8]) -> Result<String> {
    let response = client
        .post(cdn_url)
        .header("Content-Type", "application/octet-stream")
        .body(ciphertext.to_vec())
        .send()
        .await?;

    let status = response.status();

    // 检查客户端错误 (4xx)
    if status.as_u16() >= 400 && status.as_u16() < 500 {
        let err_msg = response
            .headers()
            .get("x-error-message")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "Unknown client error".to_string());
        return Err(anyhow!("CDN upload client error {}: {}", status, err_msg));
    }

    // 检查服务器错误 (5xx)
    if !status.is_success() {
        let err_msg = response
            .headers()
            .get("x-error-message")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("status {}", status));
        return Err(anyhow!("CDN upload server error: {}", err_msg));
    }

    // 获取下载参数
    let download_param = response
        .headers()
        .get("x-encrypted-param")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("CDN upload response missing x-encrypted-param header"))?;

    Ok(download_param)
}

/// 构建 CDN 上传 URL
pub fn build_cdn_upload_url(cdn_base_url: &str, upload_param: &str, filekey: &str) -> String {
    format!(
        "{}/upload?encrypted_query_param={}&filekey={}",
        cdn_base_url,
        urlencoding::encode(upload_param),
        urlencoding::encode(filekey)
    )
}

/// 构建 CDN 下载 URL
pub fn build_cdn_download_url(cdn_base_url: &str, encrypted_query_param: &str) -> String {
    format!(
        "{}/download?encrypted_query_param={}",
        cdn_base_url,
        urlencoding::encode(encrypted_query_param)
    )
}

/// 从 CDN 下载文件
pub async fn download_from_cdn(
    client: &Client,
    cdn_base_url: &str,
    encrypted_query_param: &str,
    aeskey: &[u8],
) -> Result<Vec<u8>> {
    let url = build_cdn_download_url(cdn_base_url, encrypted_query_param);

    let response = client
        .get(&url)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!("CDN download failed: {}", response.status()));
    }

    let ciphertext = response.bytes().await?;
    let plaintext = crate::crypto::decrypt_aes_ecb(&ciphertext, aeskey);

    Ok(plaintext)
}
