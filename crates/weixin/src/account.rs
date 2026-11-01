//! 账号管理

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::types::AccountData;

/// 账号管理器
#[derive(Clone)]
pub struct AccountManager {
    state_dir: PathBuf,
}

impl AccountManager {
    /// 创建新的账号管理器（使用默认目录）
    pub fn new() -> Result<Self> {
        let state_dir = Self::resolve_state_dir()?;
        fs::create_dir_all(&state_dir)?;
        fs::create_dir_all(state_dir.join("accounts"))?;

        Ok(Self { state_dir })
    }

    /// 创建新的账号管理器（指定目录）
    pub fn with_state_dir(state_dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&state_dir)?;
        fs::create_dir_all(state_dir.join("accounts"))?;

        Ok(Self { state_dir })
    }

    /// 解析状态目录
    fn resolve_state_dir() -> Result<PathBuf> {
        use directories::ProjectDirs;

        if let Some(proj_dirs) = ProjectDirs::from("com", "viacli", "weixin") {
            Ok(proj_dirs.data_dir().to_path_buf())
        } else {
            // 回退到用户主目录
            let home = dirs::home_dir().ok_or_else(|| anyhow!("无法获取主目录"))?;
            Ok(home.join(".viacli").join("weixin"))
        }
    }

    /// 获取账号索引文件路径
    fn account_index_path(&self) -> PathBuf {
        self.state_dir.join("accounts.json")
    }

    /// 获取账号文件路径
    fn account_path(&self, account_id: &str) -> PathBuf {
        self.state_dir.join("accounts").join(format!("{}.json", account_id))
    }

    /// 列出所有账号 ID
    pub fn list_account_ids(&self) -> Vec<String> {
        let index_path = self.account_index_path();
        if !index_path.exists() {
            return vec![];
        }

        match fs::read_to_string(&index_path) {
            Ok(content) => {
                match serde_json::from_str::<Vec<String>>(&content) {
                    Ok(ids) => ids.into_iter().filter(|id| !id.is_empty()).collect(),
                    Err(_) => vec![],
                }
            }
            Err(_) => vec![],
        }
    }

    /// 注册账号 ID
    pub fn register_account_id(&self, account_id: &str) -> Result<()> {
        let mut ids = self.list_account_ids();
        if !ids.contains(&account_id.to_string()) {
            ids.push(account_id.to_string());
            let content = serde_json::to_string_pretty(&ids)?;
            fs::write(self.account_index_path(), content)?;
        }
        Ok(())
    }

    /// 注销账号 ID
    pub fn unregister_account_id(&self, account_id: &str) -> Result<()> {
        let mut ids = self.list_account_ids();
        ids.retain(|id| id != account_id);
        let content = serde_json::to_string_pretty(&ids)?;
        fs::write(self.account_index_path(), content)?;
        Ok(())
    }

    /// 加载账号数据
    pub fn load_account(&self, account_id: &str) -> Option<AccountData> {
        let path = self.account_path(account_id);
        if !path.exists() {
            return None;
        }

        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).ok(),
            Err(_) => None,
        }
    }

    /// 保存账号数据
    pub fn save_account(&self, account_id: &str, data: &AccountData) -> Result<()> {
        let path = self.account_path(account_id);
        let content = serde_json::to_string_pretty(data)?;
        fs::write(&path, content)?;

        // 设置权限为 600
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path)?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&path, perms)?;
        }

        Ok(())
    }

    /// 清除账号数据
    pub fn clear_account(&self, account_id: &str) -> Result<()> {
        let path = self.account_path(account_id);
        if path.exists() {
            fs::remove_file(&path)?;
        }

        // 删除同步缓存
        let sync_path = self.state_dir.join("accounts").join(format!("{}.sync.json", account_id));
        if sync_path.exists() {
            fs::remove_file(&sync_path)?;
        }

        self.unregister_account_id(account_id)?;
        Ok(())
    }

    /// 规范化账号 ID
    pub fn normalize_account_id(&self, account_id: &str) -> String {
        account_id.replace("@", "-").replace(".", "-")
    }

    /// 获取状态目录路径
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }
}

/// 同步缓存管理
pub struct SyncCache {
    state_dir: PathBuf,
}

impl SyncCache {
    /// 创建新的同步缓存
    pub fn new(state_dir: impl AsRef<Path>) -> Self {
        Self {
            state_dir: state_dir.as_ref().to_path_buf(),
        }
    }

    /// 获取同步缓存文件路径
    fn sync_path(&self, account_id: &str) -> PathBuf {
        self.state_dir.join("accounts").join(format!("{}.sync.json", account_id))
    }

    /// 加载同步缓存
    pub fn load(&self, account_id: &str) -> Option<SyncData> {
        let path = self.sync_path(account_id);
        if !path.exists() {
            return None;
        }

        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).ok(),
            Err(_) => None,
        }
    }

    /// 保存同步缓存
    pub fn save(&self, account_id: &str, data: &SyncData) -> Result<()> {
        let path = self.sync_path(account_id);
        let content = serde_json::to_string_pretty(data)?;
        fs::write(&path, content)?;
        Ok(())
    }

    /// 清除同步缓存
    pub fn clear(&self, account_id: &str) -> Result<()> {
        let path = self.sync_path(account_id);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }
}

/// 同步数据
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncData {
    pub get_updates_buf: String,
    pub last_update_ms: Option<i64>,
}

/// 上下文 Token 管理
pub struct ContextTokenManager {
    state_dir: PathBuf,
}

impl ContextTokenManager {
    /// 创建新的上下文 Token 管理器
    pub fn new(state_dir: impl AsRef<Path>) -> Self {
        Self {
            state_dir: state_dir.as_ref().to_path_buf(),
        }
    }

    /// 获取上下文 Token 文件路径
    fn token_path(&self, account_id: &str) -> PathBuf {
        self.state_dir.join("accounts").join(format!("{}.context-tokens.json", account_id))
    }

    /// 加载上下文 Token
    pub fn load(&self, account_id: &str) -> HashMap<String, String> {
        let path = self.token_path(account_id);
        if !path.exists() {
            return HashMap::new();
        }

        match fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => HashMap::new(),
        }
    }

    /// 保存上下文 Token
    pub fn save(&self, account_id: &str, tokens: &HashMap<String, String>) -> Result<()> {
        let path = self.token_path(account_id);
        let content = serde_json::to_string_pretty(tokens)?;
        fs::write(&path, content)?;
        Ok(())
    }

    /// 设置上下文 Token
    pub fn set(&self, account_id: &str, user_id: &str, token: &str) -> Result<()> {
        let mut tokens = self.load(account_id);
        tokens.insert(user_id.to_string(), token.to_string());
        self.save(account_id, &tokens)?;
        Ok(())
    }

    /// 获取上下文 Token
    pub fn get(&self, account_id: &str, user_id: &str) -> Option<String> {
        let tokens = self.load(account_id);
        tokens.get(user_id).cloned()
    }

    /// 清除上下文 Token
    pub fn clear(&self, account_id: &str) -> Result<()> {
        let path = self.token_path(account_id);
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }
}
