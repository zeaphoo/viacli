//! 日志上传功能
//!
//! 将本地日志上传到远程服务器

use std::path::Path;

use reqwest::Client;

/// 日志上传选项
pub struct LogUploadOptions {
    pub upload_url: String,
    pub api_key: Option<String>,
    pub timeout_ms: u64,
}

/// 日志上传结果
#[derive(Debug, Clone)]
pub struct LogUploadResult {
    pub success: bool,
    pub message: String,
    pub remote_url: Option<String>,
}

/// 上传日志文件
pub async fn upload_log_file(
    file_path: &Path,
    options: &LogUploadOptions,
) -> anyhow::Result<LogUploadResult> {
    if !file_path.exists() {
        return Ok(LogUploadResult {
            success: false,
            message: format!("file not found: {:?}", file_path),
            remote_url: None,
        });
    }

    let client = Client::new();
    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown.log");

    // 读取文件内容
    let file_content = tokio::fs::read(file_path).await?;

    // 构建 multipart 表单
    let form = reqwest::multipart::Form::new()
        .part(
            "file",
            reqwest::multipart::Part::bytes(file_content.clone())
                .file_name(file_name.to_string())
                .mime_str("text/plain")
                .map_err(|e| anyhow::anyhow!("failed to set mime type: {}", e))?,
        );

    // 构建请求
    let mut request = client
        .post(&options.upload_url)
        .timeout(std::time::Duration::from_millis(options.timeout_ms))
        .multipart(form);

    // 添加认证头
    if let Some(key) = &options.api_key {
        request = request.header("Authorization", format!("Bearer {}", key));
    }

    // 发送请求
    let response = request.send().await?;
    let status = response.status();

    if status.is_success() {
        let text = response.text().await?;
        
        // 尝试解析响应中的 URL
        let remote_url = extract_url_from_response(&text);
        
        Ok(LogUploadResult {
            success: true,
            message: format!("uploaded {} ({} bytes)", file_name, file_content.len()),
            remote_url,
        })
    } else {
        let body = response.text().await.unwrap_or_default();
        Ok(LogUploadResult {
            success: false,
            message: format!("upload failed: {} - {}", status, body),
            remote_url: None,
        })
    }
}

/// 从响应中提取 URL
fn extract_url_from_response(text: &str) -> Option<String> {
    // 尝试解析 JSON 响应
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(text) {
        // 尝试常见字段
        if let Some(url) = json.get("url").and_then(|v| v.as_str()) {
            return Some(url.to_string());
        }
        if let Some(url) = json.get("data").and_then(|v| v.get("url")).and_then(|v| v.as_str()) {
            return Some(url.to_string());
        }
        if let Some(url) = json.get("link").and_then(|v| v.as_str()) {
            return Some(url.to_string());
        }
    }
    
    // 尝试从文本中提取 URL
    for word in text.split_whitespace() {
        if word.starts_with("http://") || word.starts_with("https://") {
            return Some(word.trim_end_matches(|c| c == '"' || c == ',').to_string());
        }
    }
    
    None
}

/// 压缩并上传日志
pub async fn compress_and_upload_log(
    file_path: &Path,
    options: &LogUploadOptions,
) -> anyhow::Result<LogUploadResult> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    if !file_path.exists() {
        return Ok(LogUploadResult {
            success: false,
            message: format!("file not found: {:?}", file_path),
            remote_url: None,
        });
    }

    // 读取并压缩
    let content = tokio::fs::read(file_path).await?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&content)?;
    let compressed = encoder.finish()?;

    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown.log");
    let compressed_name = format!("{}.gz", file_name);

    // 上传压缩后的内容
    let client = Client::new();
    let form = reqwest::multipart::Form::new()
        .part(
            "file",
            reqwest::multipart::Part::bytes(compressed.clone())
                .file_name(compressed_name)
                .mime_str("application/gzip")
                .map_err(|e| anyhow::anyhow!("failed to set mime type: {}", e))?,
        );

    let mut request = client
        .post(&options.upload_url)
        .timeout(std::time::Duration::from_millis(options.timeout_ms))
        .multipart(form);

    if let Some(key) = &options.api_key {
        request = request.header("Authorization", format!("Bearer {}", key));
    }

    let response = request.send().await?;
    let status = response.status();

    if status.is_success() {
        let text = response.text().await?;
        let remote_url = extract_url_from_response(&text);
        
        Ok(LogUploadResult {
            success: true,
            message: format!(
                "uploaded {} ({} -> {} bytes, {:.1}%)",
                file_name,
                content.len(),
                compressed.len(),
                (compressed.len() as f64 / content.len() as f64) * 100.0
            ),
            remote_url,
        })
    } else {
        let body = response.text().await.unwrap_or_default();
        Ok(LogUploadResult {
            success: false,
            message: format!("upload failed: {} - {}", status, body),
            remote_url: None,
        })
    }
}

/// 批量上传日志
pub async fn upload_logs_batch(
    file_paths: &[&Path],
    options: &LogUploadOptions,
) -> Vec<(String, LogUploadResult)> {
    let mut results = Vec::new();

    for path in file_paths {
        let result = match upload_log_file(path, options).await {
            Ok(r) => r,
            Err(e) => LogUploadResult {
                success: false,
                message: e.to_string(),
                remote_url: None,
            },
        };

        results.push((path.to_string_lossy().to_string(), result));
    }

    results
}

/// 获取日志上传配置
pub fn get_log_upload_config() -> Option<LogUploadOptions> {
    // 从环境变量读取配置
    let upload_url = std::env::var("WEIXIN_LOG_UPLOAD_URL").ok()?;
    let api_key = std::env::var("WEIXIN_LOG_UPLOAD_KEY").ok();
    let timeout_ms = std::env::var("WEIXIN_LOG_UPLOAD_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30_000);

    Some(LogUploadOptions {
        upload_url,
        api_key,
        timeout_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_url_from_response() {
        // JSON 响应
        assert_eq!(
            extract_url_from_response(r#"{"url": "https://example.com/log.txt"}"#),
            Some("https://example.com/log.txt".to_string())
        );
        
        // data.url 格式
        assert_eq!(
            extract_url_from_response(r#"{"data": {"url": "https://example.com/log.txt"}}"#),
            Some("https://example.com/log.txt".to_string())
        );
        
        // 纯文本 URL
        assert_eq!(
            extract_url_from_response("uploaded to https://example.com/log.txt"),
            Some("https://example.com/log.txt".to_string())
        );
        
        // 无 URL
        assert_eq!(
            extract_url_from_response("success"),
            None
        );
    }
}
