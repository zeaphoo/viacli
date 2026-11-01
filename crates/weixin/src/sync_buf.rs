//! 同步缓存持久化模块
//!
//! 管理 get_updates_buf 的持久化存储，支持兼容性回退

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 同步数据
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncBufData {
    pub get_updates_buf: String,
}

/// 同步缓存管理器
pub struct SyncBufManager {
    state_dir: PathBuf,
}

impl SyncBufManager {
    /// 创建新的同步缓存管理器
    pub fn new(state_dir: impl AsRef<Path>) -> Self {
        Self {
            state_dir: state_dir.as_ref().to_path_buf(),
        }
    }

    /// 获取同步缓存文件路径
    pub fn sync_file_path(&self, account_id: &str) -> PathBuf {
        self.state_dir.join("accounts").join(format!("{}.sync.json", account_id))
    }

    /// 旧版单账号同步缓存路径（兼容性）
    fn legacy_sync_buf_path(&self) -> PathBuf {
        self.state_dir.join("sync").join("default.json")
    }

    /// 从文件读取同步缓存
    fn read_sync_file(&self, path: &Path) -> Option<String> {
        match fs::read_to_string(path) {
            Ok(content) => {
                match serde_json::from_str::<SyncBufData>(&content) {
                    Ok(data) => Some(data.get_updates_buf),
                    Err(_) => None,
                }
            }
            Err(_) => None,
        }
    }

    /// 加载同步缓存
    /// 
    /// 回退顺序：
    /// 1. 主路径（规范化 accountId，新安装）
    /// 2. 兼容路径（从模式派生的原始 accountId，旧安装）
    /// 3. 旧版单账号路径（无多账号支持的旧安装）
    pub fn load(&self, account_id: &str) -> Option<String> {
        let primary_path = self.sync_file_path(account_id);
        
        // 尝试主路径
        if let Some(buf) = self.read_sync_file(&primary_path) {
            return Some(buf);
        }

        // 尝试兼容路径（如果 account_id 是规范化的）
        if let Some(raw_id) = derive_raw_account_id(account_id) {
            let compat_path = self.state_dir.join("accounts").join(format!("{}.sync.json", raw_id));
            if let Some(buf) = self.read_sync_file(&compat_path) {
                return Some(buf);
            }
        }

        // 尝试旧版路径
        self.read_sync_file(&self.legacy_sync_buf_path())
    }

    /// 保存同步缓存
    pub fn save(&self, account_id: &str, get_updates_buf: &str) -> anyhow::Result<()> {
        let path = self.sync_file_path(account_id);
        
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let data = SyncBufData {
            get_updates_buf: get_updates_buf.to_string(),
        };
        
        let content = serde_json::to_string(&data)?;
        fs::write(&path, content)?;
        
        Ok(())
    }

    /// 清除同步缓存
    pub fn clear(&self, account_id: &str) -> anyhow::Result<()> {
        let path = self.sync_file_path(account_id);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }
}

/// 从规范化的 account_id 派生原始 ID
/// 
/// 例如："hex-im-bot" -> "hex@im.bot"
fn derive_raw_account_id(normalized: &str) -> Option<String> {
    // 检查是否符合规范化格式（包含 -im-bot 或类似模式）
    if normalized.contains("-im-") {
        // 尝试还原：将最后一个 -im- 替换为 @im.
        let parts: Vec<&str> = normalized.rsplitn(2, "-im-").collect();
        if parts.len() == 2 {
            return Some(format!("{}@im.{}", parts[1], parts[0]));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_raw_account_id() {
        assert_eq!(
            derive_raw_account_id("hex-im-bot"),
            Some("hex@im.bot".to_string())
        );
        assert_eq!(
            derive_raw_account_id("test123-im-bot"),
            Some("test123@im.bot".to_string())
        );
        assert_eq!(derive_raw_account_id("invalid"), None);
    }
}
