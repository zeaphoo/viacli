//! 媒体文件上传功能

use anyhow::{anyhow, Result};
use reqwest::Client;
use std::path::Path;
use tokio::fs;

use crate::cdn::upload_buffer_to_cdn;
use crate::client::WeixinApiClient;
use crate::crypto::{aes_ecb_padded_size, generate_aes_key};
use crate::types::*;

/// 上传文件信息
#[derive(Debug, Clone)]
pub struct UploadedFileInfo {
    /// 文件密钥
    pub filekey: String,
    /// CDN 返回的下载加密参数
    pub download_encrypted_query_param: String,
    /// AES-128-ECB 密钥 (hex 编码)
    pub aeskey: String,
    /// 明文文件大小（字节）
    pub file_size: i64,
    /// 密文文件大小（字节）
    pub file_size_ciphertext: i64,
}

/// 上传媒体到微信 CDN
pub async fn upload_media_to_cdn(
    client: &WeixinApiClient,
    http_client: &Client,
    file_path: &str,
    to_user_id: &str,
    cdn_base_url: &str,
    media_type: UploadMediaType,
) -> Result<UploadedFileInfo> {
    // 读取文件
    let plaintext = fs::read(file_path).await?;
    let rawsize = plaintext.len() as i64;

    // 计算 MD5
    let rawfilemd5 = format!("{:x}", md5::compute(&plaintext));

    // 计算密文大小
    let filesize = aes_ecb_padded_size(plaintext.len()) as i64;

    // 生成文件密钥和 AES 密钥
    let filekey = generate_filekey();
    let aeskey = generate_aes_key();

    // 调用 getUploadUrl 获取上传参数
    let upload_url_resp = client
        .get_upload_url(GetUploadUrlReq {
            filekey: Some(filekey.clone()),
            media_type: media_type.as_i32(),
            to_user_id: to_user_id.to_string(),
            rawsize,
            rawfilemd5,
            filesize,
            thumb_rawsize: None,
            thumb_rawfilemd5: None,
            thumb_filesize: None,
            no_need_thumb: Some(true),
            aeskey: Some(hex::encode(&aeskey)),
            base_info: None,
        })
        .await?;

    let upload_param = upload_url_resp
        .upload_param
        .ok_or_else(|| anyhow!("getUploadUrl returned no upload_param"))?;

    // 上传文件到 CDN
    let download_param = upload_buffer_to_cdn(
        http_client,
        &plaintext,
        &upload_param,
        &filekey,
        cdn_base_url,
        &aeskey,
    )
    .await?;

    Ok(UploadedFileInfo {
        filekey,
        download_encrypted_query_param: download_param,
        aeskey: hex::encode(&aeskey),
        file_size: rawsize,
        file_size_ciphertext: filesize,
    })
}

/// 上传图片文件
pub async fn upload_image_to_weixin(
    client: &WeixinApiClient,
    http_client: &Client,
    file_path: &str,
    to_user_id: &str,
    cdn_base_url: &str,
) -> Result<UploadedFileInfo> {
    upload_media_to_cdn(
        client,
        http_client,
        file_path,
        to_user_id,
        cdn_base_url,
        UploadMediaType::Image,
    )
    .await
}

/// 上传视频文件
pub async fn upload_video_to_weixin(
    client: &WeixinApiClient,
    http_client: &Client,
    file_path: &str,
    to_user_id: &str,
    cdn_base_url: &str,
) -> Result<UploadedFileInfo> {
    upload_media_to_cdn(
        client,
        http_client,
        file_path,
        to_user_id,
        cdn_base_url,
        UploadMediaType::Video,
    )
    .await
}

/// 上传文件附件
pub async fn upload_file_attachment_to_weixin(
    client: &WeixinApiClient,
    http_client: &Client,
    file_path: &str,
    to_user_id: &str,
    cdn_base_url: &str,
) -> Result<UploadedFileInfo> {
    upload_media_to_cdn(
        client,
        http_client,
        file_path,
        to_user_id,
        cdn_base_url,
        UploadMediaType::File,
    )
    .await
}

/// 生成文件密钥
fn generate_filekey() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 16];
    rng.fill(&mut bytes);
    hex::encode(bytes)
}

/// 下载远程媒体文件到临时目录
pub async fn download_remote_media_to_temp(
    http_client: &Client,
    url: &str,
    dest_dir: &str,
) -> Result<String> {
    let response = http_client.get(url).send().await?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "remote media download failed: {} {}",
            response.status(),
            url
        ));
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let bytes = response.bytes().await?;

    // 创建目标目录
    tokio::fs::create_dir_all(dest_dir).await?;

    // 推断扩展名
    let ext = get_extension_from_content_type_or_url(content_type.as_deref(), url);
    let filename = format!("weixin-remote-{}.{}", generate_random_id(), ext);
    let file_path = Path::new(dest_dir).join(&filename);

    tokio::fs::write(&file_path, bytes).await?;

    Ok(file_path.to_string_lossy().to_string())
}

/// 从 Content-Type 或 URL 获取扩展名
fn get_extension_from_content_type_or_url(content_type: Option<&str>, url: &str) -> &'static str {
    // 先尝试从 Content-Type 获取
    if let Some(ct) = content_type {
        let ct = ct.split(';').next().unwrap_or(ct).trim();
        match ct {
            "image/jpeg" | "image/jpg" => return "jpg",
            "image/png" => return "png",
            "image/gif" => return "gif",
            "image/webp" => return "webp",
            "video/mp4" => return "mp4",
            "video/quicktime" => return "mov",
            "audio/mpeg" => return "mp3",
            "audio/wav" => return "wav",
            "application/pdf" => return "pdf",
            _ => {}
        }
    }

    // 从 URL 路径获取
    if let Some(path) = url.split('?').next() {
        if let Some(ext) = path.rsplit('.').next() {
            let ext = ext.to_lowercase();
            if !ext.is_empty() && ext.len() < 10 {
                return match ext.as_str() {
                    "jpg" | "jpeg" => "jpg",
                    "png" => "png",
                    "gif" => "gif",
                    "webp" => "webp",
                    "mp4" => "mp4",
                    "mov" => "mov",
                    "mp3" => "mp3",
                    "wav" => "wav",
                    "pdf" => "pdf",
                    "txt" => "txt",
                    _ => "bin",
                };
            }
        }
    }

    "bin"
}

/// 生成随机 ID
fn generate_random_id() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 8];
    rng.fill(&mut bytes);
    hex::encode(bytes)
}
