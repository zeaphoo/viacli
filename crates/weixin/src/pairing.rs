//! 用户配对授权管理模块
//!
//! 管理通过配对授权的用户列表（allowFrom）

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 授权列表文件内容
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AllowFromFile {
    version: u32,
    allow_from: Vec<String>,
}

impl Default for AllowFromFile {
    fn default() -> Self {
        Self {
            version: 1,
            allow_from: Vec::new(),
        }
    }
}

/// 用户配对管理器
pub struct PairingManager {
    state_dir: PathBuf,
}

impl PairingManager {
    /// 创建新的配对管理器
    pub fn new(state_dir: impl AsRef<Path>) -> Self {
        Self {
            state_dir: state_dir.as_ref().to_path_buf(),
        }
    }

    /// 解析凭证目录
    fn credentials_dir(&self) -> PathBuf {
        self.state_dir.join("credentials")
    }

    /// 生成安全键名（用于文件名）
    fn safe_key(&self, raw: &str) -> String {
        let trimmed = raw.trim().to_lowercase();
        if trimmed.is_empty() {
            panic!("invalid key for allowFrom path");
        }
        let safe = trimmed
            .replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_")
            .replace("..", "_");
        if safe.is_empty() || safe == "_" {
            panic!("invalid key for allowFrom path");
        }
        safe
    }

    /// 解析授权列表文件路径
    fn allow_from_path(&self, account_id: &str) -> PathBuf {
        let base = self.safe_key("weixin");
        let safe_account = self.safe_key(account_id);
        self.credentials_dir().join(format!("{}-{}-allowFrom.json", base, safe_account))
    }

    /// 读取授权列表
    pub fn read_allow_from_list(&self, account_id: &str) -> Vec<String> {
        let path = self.allow_from_path(account_id);
        
        if !path.exists() {
            return Vec::new();
        }

        match fs::read_to_string(&path) {
            Ok(content) => {
                match serde_json::from_str::<AllowFromFile>(&content) {
                    Ok(file) => file
                        .allow_from
                        .into_iter()
                        .filter(|id| !id.trim().is_empty())
                        .collect(),
                    Err(_) => Vec::new(),
                }
            }
            Err(_) => Vec::new(),
        }
    }

    /// 注册用户到授权列表
    pub fn register_user(&self, account_id: &str, user_id: &str) -> anyhow::Result<bool> {
        let trimmed_user_id = user_id.trim();
        if trimmed_user_id.is_empty() {
            return Ok(false);
        }

        let path = self.allow_from_path(account_id);
        let dir = path.parent().unwrap_or(&self.state_dir);
        fs::create_dir_all(dir)?;

        // 确保文件存在
        if !path.exists() {
            let initial = AllowFromFile::default();
            fs::write(&path, serde_json::to_string_pretty(&initial)?)?;
        }

        // 读取当前内容
        let mut content = match fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str::<AllowFromFile>(&s).unwrap_or_default(),
            Err(_) => AllowFromFile::default(),
        };

        // 检查是否已存在
        if content.allow_from.contains(&trimmed_user_id.to_string()) {
            return Ok(false);
        }

        // 添加新用户
        content.allow_from.push(trimmed_user_id.to_string());
        fs::write(&path, serde_json::to_string_pretty(&content)?)?;

        log::info!(
            "registerUserInAllowFromStore: added user_id={} account_id={} path={:?}",
            trimmed_user_id,
            account_id,
            path
        );

        Ok(true)
    }

    /// 从授权列表移除用户
    pub fn unregister_user(&self, account_id: &str, user_id: &str) -> anyhow::Result<bool> {
        let path = self.allow_from_path(account_id);
        
        if !path.exists() {
            return Ok(false);
        }

        let mut content = match fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str::<AllowFromFile>(&s).unwrap_or_default(),
            Err(_) => return Ok(false),
        };

        let original_len = content.allow_from.len();
        content.allow_from.retain(|id| id != user_id);

        if content.allow_from.len() == original_len {
            return Ok(false);
        }

        fs::write(&path, serde_json::to_string_pretty(&content)?)?;

        log::info!(
            "unregisterUserFromAllowFromStore: removed user_id={} account_id={}",
            user_id,
            account_id
        );

        Ok(true)
    }

    /// 检查用户是否在授权列表中
    pub fn is_user_allowed(&self, account_id: &str, user_id: &str) -> bool {
        let list = self.read_allow_from_list(account_id);
        list.iter().any(|id| id == user_id)
    }

    /// 清除账号的所有授权
    pub fn clear_allow_from(&self, account_id: &str) -> anyhow::Result<()> {
        let path = self.allow_from_path(account_id);
        if path.exists() {
            fs::remove_file(&path)?;
            log::info!("clearAllowFrom: cleared account_id={}", account_id);
        }
        Ok(())
    }

    /// 获取所有授权的账号 ID
    pub fn list_allowed_account_ids(&self, user_id: &str) -> Vec<String> {
        let mut result = Vec::new();
        
        let cred_dir = self.credentials_dir();
        if !cred_dir.exists() {
            return result;
        }

        let entries = match fs::read_dir(&cred_dir) {
            Ok(entries) => entries,
            Err(_) => return result,
        };

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let file_name = match path.file_name() {
                Some(name) => name.to_string_lossy().to_string(),
                None => continue,
            };

            // 解析文件名: weixin-{account_id}-allowFrom.json
            if !file_name.ends_with("-allowFrom.json") {
                continue;
            }

            let content = match fs::read_to_string(&path) {
                Ok(s) => s,
                Err(_) => continue,
            };

            let file: AllowFromFile = match serde_json::from_str(&content) {
                Ok(f) => f,
                Err(_) => continue,
            };

            if file.allow_from.contains(&user_id.to_string()) {
                // 提取 account_id
                let account_id = file_name
                    .trim_start_matches("weixin-")
                    .trim_end_matches("-allowFrom.json");
                result.push(account_id.to_string());
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_safe_key() {
        let temp_dir = TempDir::new().unwrap();
        let manager = PairingManager::new(temp_dir.path());
        
        assert_eq!(manager.safe_key("Test-Key"), "test-key");
        assert_eq!(manager.safe_key("path/to/key"), "path_to_key");
        assert_eq!(manager.safe_key("key..with..dots"), "key_with_dots");
    }

    #[test]
    fn test_allow_from_path() {
        let temp_dir = TempDir::new().unwrap();
        let manager = PairingManager::new(temp_dir.path());
        
        let path = manager.allow_from_path("test-account");
        assert!(path.to_string_lossy().contains("weixin-test-account-allowFrom.json"));
    }

    #[test]
    fn test_read_write_allow_from() {
        let temp_dir = TempDir::new().unwrap();
        let manager = PairingManager::new(temp_dir.path());
        
        let account_id = "test-account";
        let user_id = "user@example.com";

        // 初始为空
        let list = manager.read_allow_from_list(account_id);
        assert!(list.is_empty());

        // 注册用户
        assert!(manager.register_user(account_id, user_id).unwrap());
        
        // 重复注册返回 false
        assert!(!manager.register_user(account_id, user_id).unwrap());

        // 读取列表
        let list = manager.read_allow_from_list(account_id);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0], user_id);

        // 检查授权
        assert!(manager.is_user_allowed(account_id, user_id));
        assert!(!manager.is_user_allowed(account_id, "other@example.com"));

        // 注销用户
        assert!(manager.unregister_user(account_id, user_id).unwrap());
        assert!(!manager.unregister_user(account_id, user_id).unwrap());
    }
}
