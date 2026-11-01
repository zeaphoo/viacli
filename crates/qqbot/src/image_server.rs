//! 图片服务器模块
//! 提供本地图片的 HTTP 访问服务

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tokio::time::interval;

/// 图片服务器配置
#[derive(Debug, Clone)]
pub struct ImageServerConfig {
    /// 监听端口
    pub port: u16,
    /// 存储目录
    pub storage_dir: PathBuf,
    /// 公网基础 URL
    pub base_url: String,
    /// 文件过期时间（秒）
    pub ttl_seconds: u64,
}

impl Default for ImageServerConfig {
    fn default() -> Self {
        Self {
            port: 18765,
            storage_dir: PathBuf::from("./qqbot_images"),
            base_url: "http://localhost:18765".to_string(),
            ttl_seconds: 3600,
        }
    }
}

/// 图片文件信息
#[derive(Debug, Clone)]
struct ImageFile {
    path: PathBuf,
    created_at: SystemTime,
    content_type: String,
}

/// 图片服务器
pub struct ImageServer {
    config: ImageServerConfig,
    files: Arc<RwLock<HashMap<String, ImageFile>>>,
    running: Arc<RwLock<bool>>,
}

impl ImageServer {
    /// 创建新的图片服务器
    pub fn new(config: ImageServerConfig) -> Self {
        Self {
            config,
            files: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(RwLock::new(false)),
        }
    }

    /// 启动服务器
    pub async fn start(&self) -> Result<()> {
        // 确保存储目录存在
        fs::create_dir_all(&self.config.storage_dir).await?;

        let addr = SocketAddr::from(([0, 0, 0, 0], self.config.port));
        let listener = TcpListener::bind(addr).await?;

        *self.running.write().await = true;
        let running = self.running.clone();
        let files = self.files.clone();
        let storage_dir = self.config.storage_dir.clone();
        let ttl = Duration::from_secs(self.config.ttl_seconds);

        // 启动清理任务
        let cleanup_files = files.clone();
        let cleanup_running = running.clone();
        let storage_dir_clone = storage_dir.clone();
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                if !*cleanup_running.read().await {
                    break;
                }
                Self::cleanup_expired_files(&cleanup_files, &storage_dir_clone, ttl).await;
            }
        });

        // 接受连接
        while *running.read().await {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let files = files.clone();
                    let storage_dir = storage_dir.clone();
                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_connection(stream, files, storage_dir).await {
                            log::error!("[image-server] Connection error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    log::error!("[image-server] Accept error: {}", e);
                }
            }
        }

        Ok(())
    }

    /// 停止服务器
    pub async fn stop(&self) {
        *self.running.write().await = false;
    }

    /// 保存图片并返回 URL
    pub async fn save_image(&self, filename: &str, data: &[u8], content_type: &str) -> Result<String> {
        let id = format!("{}_{}", uuid::Uuid::new_v4().to_string().replace("-", ""), filename);
        let path = self.config.storage_dir.join(&id);

        // 写入文件
        let mut file = fs::File::create(&path).await?;
        file.write_all(data).await?;

        // 记录文件信息
        let image_file = ImageFile {
            path: path.clone(),
            created_at: SystemTime::now(),
            content_type: content_type.to_string(),
        };

        self.files.write().await.insert(id.clone(), image_file);

        // 返回公网 URL
        let url = format!("{}/{}", self.config.base_url, id);
        Ok(url)
    }

    /// 保存 Base64 图片
    pub async fn save_base64_image(&self, filename: &str, base64_data: &str) -> Result<String> {
        let data = base64_decode(base64_data)?;
        let content_type = guess_content_type(filename);
        self.save_image(filename, &data, &content_type).await
    }

    /// 获取图片 URL（如果已存在）
    pub async fn get_image_url(&self, id: &str) -> Option<String> {
        let files = self.files.read().await;
        if files.contains_key(id) {
            Some(format!("{}/{}", self.config.base_url, id))
        } else {
            None
        }
    }

    /// 处理 HTTP 连接
    async fn handle_connection(
        mut stream: tokio::net::TcpStream,
        files: Arc<RwLock<HashMap<String, ImageFile>>>,
        storage_dir: PathBuf,
    ) -> Result<()> {
        use tokio::io::AsyncReadExt;

        let mut buffer = [0u8; 1024];
        let n = stream.read(&mut buffer).await?;
        let request = String::from_utf8_lossy(&buffer[..n]);

        // 解析请求路径
        let lines: Vec<&str> = request.lines().collect();
        if lines.is_empty() {
            return Ok(());
        }

        let parts: Vec<&str> = lines[0].split_whitespace().collect();
        if parts.len() < 2 {
            return Ok(());
        }

        let path = parts[1];
        let id = path.trim_start_matches('/');

        // 查找文件
        let files_read = files.read().await;
        if let Some(image_file) = files_read.get(id) {
            let file_path = &image_file.path;
            let content_type = &image_file.content_type;

            // 读取文件内容
            match fs::read(file_path).await {
                Ok(content) => {
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: public, max-age=3600\r\n\r\n",
                        content_type,
                        content.len()
                    );
                    stream.write_all(response.as_bytes()).await?;
                    stream.write_all(&content).await?;
                }
                Err(_) => {
                    let response = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
                    stream.write_all(response.as_bytes()).await?;
                }
            }
        } else {
            // 尝试从磁盘读取
            let file_path = storage_dir.join(id);
            if file_path.exists() {
                if let Ok(content) = fs::read(&file_path).await {
                    let content_type = guess_content_type(id);
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: public, max-age=3600\r\n\r\n",
                        content_type,
                        content.len()
                    );
                    stream.write_all(response.as_bytes()).await?;
                    stream.write_all(&content).await?;
                } else {
                    let response = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n";
                    stream.write_all(response.as_bytes()).await?;
                }
            } else {
                let response = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
                stream.write_all(response.as_bytes()).await?;
            }
        }

        Ok(())
    }

    /// 清理过期文件
    async fn cleanup_expired_files(
        files: &Arc<RwLock<HashMap<String, ImageFile>>>,
        storage_dir: &Path,
        ttl: Duration,
    ) {
        let now = SystemTime::now();
        let mut files_write = files.write().await;
        let mut to_remove = Vec::new();

        for (id, file) in files_write.iter() {
            if let Ok(elapsed) = now.duration_since(file.created_at) {
                if elapsed > ttl {
                    to_remove.push(id.clone());
                    // 删除物理文件
                    let _ = fs::remove_file(&file.path).await;
                }
            }
        }

        for id in to_remove {
            files_write.remove(&id);
            log::info!("[image-server] Cleaned up expired file: {}", id);
        }
    }

    /// 检查服务器是否运行
    pub async fn is_running(&self) -> bool {
        *self.running.read().await
    }
}

/// 简单的图片服务器管理器（单例模式）
use once_cell::sync::OnceCell;

static IMAGE_SERVER: OnceCell<Arc<ImageServer>> = OnceCell::new();

/// 初始化图片服务器
pub async fn init_image_server(config: ImageServerConfig) -> Result<Arc<ImageServer>> {
    let server = Arc::new(ImageServer::new(config));
    IMAGE_SERVER
        .set(server.clone())
        .map_err(|_| anyhow!("Image server already initialized"))?;
    Ok(server)
}

/// 获取图片服务器实例
pub fn get_image_server() -> Option<Arc<ImageServer>> {
    IMAGE_SERVER.get().cloned()
}

/// Base64 解码
fn base64_decode(data: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| anyhow!("Base64 decode error: {}", e))
}

/// 猜测 Content-Type
fn guess_content_type(filename: &str) -> String {
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "png" => "image/png".to_string(),
        "gif" => "image/gif".to_string(),
        "webp" => "image/webp".to_string(),
        "bmp" => "image/bmp".to_string(),
        "svg" => "image/svg+xml".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

/// 便捷函数：保存图片并返回 URL
pub async fn save_image_to_server(
    filename: &str,
    data: &[u8],
) -> Result<String> {
    if let Some(server) = get_image_server() {
        let content_type = guess_content_type(filename);
        server.save_image(filename, data, &content_type).await
    } else {
        Err(anyhow!("Image server not initialized"))
    }
}

/// 便捷函数：下载远程图片并保存到服务器
pub async fn download_and_save_image(url: &str, filename: &str) -> Result<String> {
    let response = reqwest::get(url).await?;
    let data = response.bytes().await?;
    save_image_to_server(filename, &data).await
}
