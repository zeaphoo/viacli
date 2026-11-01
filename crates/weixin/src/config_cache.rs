//! 配置缓存管理器
//!
//! 每用户的 getConfig 缓存，支持周期性随机刷新和失败指数退避重试

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::client::WeixinApiClient;

/// 配置缓存 TTL（24小时）
const CONFIG_CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// 初始重试延迟
const CONFIG_CACHE_INITIAL_RETRY_MS: u64 = 2_000;
/// 最大重试延迟（1小时）
const CONFIG_CACHE_MAX_RETRY_MS: u64 = 60 * 60 * 1000;

/// 缓存配置子集
#[derive(Debug, Clone, Default)]
pub struct CachedConfig {
    pub typing_ticket: String,
}

/// 缓存条目
#[derive(Debug)]
struct ConfigCacheEntry {
    config: CachedConfig,
    ever_succeeded: bool,
    next_fetch_at: Instant,
    retry_delay_ms: u64,
}

/// 配置缓存管理器
pub struct ConfigCache {
    cache: Mutex<HashMap<String, ConfigCacheEntry>>,
    client: Arc<WeixinApiClient>,
}

impl ConfigCache {
    /// 创建新的配置缓存管理器
    pub fn new(client: Arc<WeixinApiClient>) -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            client,
        }
    }

    /// 获取用户的配置
    pub async fn get_for_user(
        &self,
        user_id: &str,
        context_token: Option<&str>,
    ) -> CachedConfig {
        let now = Instant::now();
        let should_fetch = {
            let cache = self.cache.lock().unwrap();
            match cache.get(user_id) {
                None => true,
                Some(entry) => now >= entry.next_fetch_at,
            }
        };

        if should_fetch {
            let mut fetch_ok = false;
            
            match self.client.get_config(user_id, context_token.map(|s| s.to_string())).await {
                Ok(resp) => {
                    if resp.ret == 0 {
                        let config = CachedConfig {
                            typing_ticket: resp.typing_ticket.unwrap_or_default(),
                        };
                        
                        let ever_succeeded = {
                            let cache = self.cache.lock().unwrap();
                            cache.get(user_id).map(|e| e.ever_succeeded).unwrap_or(false)
                        };
                        
                        let mut cache = self.cache.lock().unwrap();
                        cache.insert(
                            user_id.to_string(),
                            ConfigCacheEntry {
                                config,
                                ever_succeeded: true,
                                next_fetch_at: now + Self::random_ttl(),
                                retry_delay_ms: CONFIG_CACHE_INITIAL_RETRY_MS,
                            },
                        );
                        
                        log::info!(
                            "[weixin] config {} for {}",
                            if ever_succeeded { "refreshed" } else { "cached" },
                            user_id
                        );
                        fetch_ok = true;
                    }
                }
                Err(e) => {
                    log::warn!("[weixin] getConfig failed for {} (ignored): {}", user_id, e);
                }
            }

            if !fetch_ok {
                let (prev_delay, ever_succeeded) = {
                    let cache = self.cache.lock().unwrap();
                    match cache.get(user_id) {
                        Some(entry) => (entry.retry_delay_ms, entry.ever_succeeded),
                        None => (CONFIG_CACHE_INITIAL_RETRY_MS, false),
                    }
                };

                let next_delay = (prev_delay * 2).min(CONFIG_CACHE_MAX_RETRY_MS);
                
                let mut cache = self.cache.lock().unwrap();
                if let Some(entry) = cache.get_mut(user_id) {
                    entry.next_fetch_at = now + Duration::from_millis(next_delay);
                    entry.retry_delay_ms = next_delay;
                } else {
                    cache.insert(
                        user_id.to_string(),
                        ConfigCacheEntry {
                            config: CachedConfig::default(),
                            ever_succeeded: false,
                            next_fetch_at: now + Duration::from_millis(CONFIG_CACHE_INITIAL_RETRY_MS),
                            retry_delay_ms: CONFIG_CACHE_INITIAL_RETRY_MS,
                        },
                    );
                }
            }
        }

        let cache = self.cache.lock().unwrap();
        cache.get(user_id).map(|e| e.config.clone()).unwrap_or_default()
    }

    /// 生成随机 TTL（0-24小时）
    fn random_ttl() -> Duration {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let millis = rng.gen_range(0..CONFIG_CACHE_TTL.as_millis() as u64);
        Duration::from_millis(millis)
    }

    /// 清除指定用户的缓存
    pub fn clear_user(&self, user_id: &str) {
        let mut cache = self.cache.lock().unwrap();
        cache.remove(user_id);
    }

    /// 清除所有缓存
    pub fn clear_all(&self) {
        let mut cache = self.cache.lock().unwrap();
        cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_random_ttl() {
        let ttl = ConfigCache::random_ttl();
        assert!(ttl <= CONFIG_CACHE_TTL);
    }
}
