//! 配置管理

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// QQ Bot 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QQBotConfig {
    pub app_id: String,
    pub client_secret: String,
    pub name: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

/// 配置存储
#[derive(Debug, Default, Serialize, Deserialize)]
struct ConfigStore {
    #[serde(default)]
    accounts: HashMap<String, QQBotConfig>,
}

/// 配置管理器
pub struct ConfigManager {
    config_dir: PathBuf,
    config_file: PathBuf,
}

impl ConfigManager {
    /// 创建默认配置管理器
    pub fn new() -> Result<Self> {
        let config_dir = get_config_dir()?;
        let config_file = config_dir.join("qqbot.json");

        // 确保配置目录存在
        fs::create_dir_all(&config_dir)?;

        Ok(Self {
            config_dir,
            config_file,
        })
    }

    /// 从指定目录创建
    pub fn with_dir(config_dir: impl AsRef<Path>) -> Result<Self> {
        let config_dir = config_dir.as_ref().to_path_buf();
        let config_file = config_dir.join("qqbot.json");

        fs::create_dir_all(&config_dir)?;

        Ok(Self {
            config_dir,
            config_file,
        })
    }

    /// 获取配置目录
    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    /// 加载配置存储
    fn load_store(&self) -> Result<ConfigStore> {
        if !self.config_file.exists() {
            return Ok(ConfigStore::default());
        }

        let content = fs::read_to_string(&self.config_file)?;
        let store: ConfigStore = serde_json::from_str(&content)?;
        Ok(store)
    }

    /// 保存配置存储
    fn save_store(&self, store: &ConfigStore) -> Result<()> {
        let content = serde_json::to_string_pretty(store)?;
        fs::write(&self.config_file, content)?;
        Ok(())
    }

    /// 保存账号配置
    pub fn save_config(&self, account_id: &str, config: &QQBotConfig) -> Result<()> {
        let mut store = self.load_store()?;
        store.accounts.insert(account_id.to_string(), config.clone());
        self.save_store(&store)?;
        Ok(())
    }

    /// 加载账号配置
    pub fn load_config(&self, account_id: &str) -> Option<QQBotConfig> {
        self.load_store().ok()?.accounts.get(account_id).cloned()
    }

    /// 删除账号配置
    pub fn delete_config(&self, account_id: &str) -> Result<()> {
        let mut store = self.load_store()?;
        store.accounts.remove(account_id);
        self.save_store(&store)?;
        Ok(())
    }

    /// 列出所有账号 ID
    pub fn list_account_ids(&self) -> Vec<String> {
        self.load_store()
            .map(|s| s.accounts.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// 列出所有账号
    pub fn list_accounts(&self) -> Vec<(String, QQBotConfig)> {
        self.load_store()
            .map(|s| {
                s.accounts
                    .into_iter()
                    .map(|(k, v)| (k, v))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 检查账号是否存在
    pub fn account_exists(&self, account_id: &str) -> bool {
        self.load_store()
            .map(|s| s.accounts.contains_key(account_id))
            .unwrap_or(false)
    }

    /// 设置账号启用状态
    pub fn set_enabled(&self, account_id: &str, enabled: bool) -> Result<()> {
        let mut store = self.load_store()?;
        if let Some(config) = store.accounts.get_mut(account_id) {
            config.enabled = enabled;
            self.save_store(&store)?;
        }
        Ok(())
    }

    /// 清除所有配置（登出）
    pub fn clear_all(&self) -> Result<()> {
        if self.config_file.exists() {
            fs::remove_file(&self.config_file)?;
        }
        Ok(())
    }
}

/// 获取配置目录
fn get_config_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("com", "viacli", "qqbot")
        .ok_or_else(|| anyhow::anyhow!("Failed to get config directory"))?;
    Ok(dirs.config_dir().to_path_buf())
}

/// 从环境变量创建配置
pub fn config_from_env() -> Option<(String, QQBotConfig)> {
    let app_id = std::env::var("QQBOT_APP_ID").ok()?;
    let client_secret = std::env::var("QQBOT_CLIENT_SECRET").ok()?;

    let config = QQBotConfig {
        app_id,
        client_secret,
        name: std::env::var("QQBOT_NAME").ok(),
        enabled: true,
    };

    let account_id = std::env::var("QQBOT_ACCOUNT_ID").unwrap_or_else(|_| "default".to_string());

    Some((account_id, config))
}

/// 解析 token 字符串 (格式: app_id:client_secret)
pub fn parse_token(token: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = token.splitn(2, ':').collect();
    if parts.len() == 2 {
        Some((parts[0].to_string(), parts[1].to_string()))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_token() {
        let result = parse_token("app123:secret456");
        assert!(result.is_some());
        let (app_id, secret) = result.unwrap();
        assert_eq!(app_id, "app123");
        assert_eq!(secret, "secret456");
    }

    #[test]
    fn test_parse_token_invalid() {
        let result = parse_token("invalid_token");
        assert!(result.is_none());
    }
}
