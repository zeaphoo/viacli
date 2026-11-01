//! 会话保护模块
//! 
//! 处理会话过期保护，当收到会话过期错误时暂停 API 调用一段时间

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use once_cell::sync::Lazy;

/// 会话过期错误码
pub const SESSION_EXPIRED_ERRCODE: i32 = -14;

/// 会话暂停时长（1小时）
const SESSION_PAUSE_DURATION: Duration = Duration::from_secs(60 * 60);

/// 全局会话暂停状态
static PAUSE_UNTIL_MAP: Lazy<Mutex<HashMap<String, Instant>>> = Lazy::new(|| {
    Mutex::new(HashMap::new())
});

/// 暂停指定账号的所有 API 调用
pub fn pause_session(account_id: &str) {
    let until = Instant::now() + SESSION_PAUSE_DURATION;
    let mut map = PAUSE_UNTIL_MAP.lock().unwrap();
    map.insert(account_id.to_string(), until);
    log::info!(
        "session-guard: paused account_id={} until={:?} ({}s)",
        account_id,
        until,
        SESSION_PAUSE_DURATION.as_secs()
    );
}

/// 检查账号是否处于暂停状态
pub fn is_session_paused(account_id: &str) -> bool {
    let mut map = PAUSE_UNTIL_MAP.lock().unwrap();
    
    if let Some(until) = map.get(account_id) {
        if Instant::now() >= *until {
            // 暂停已过期
            map.remove(account_id);
            return false;
        }
        return true;
    }
    false
}

/// 获取剩余暂停时间（毫秒），未暂停返回 0
pub fn get_remaining_pause_ms(account_id: &str) -> u64 {
    let mut map = PAUSE_UNTIL_MAP.lock().unwrap();
    
    if let Some(until) = map.get(account_id) {
        let now = Instant::now();
        if now >= *until {
            map.remove(account_id);
            return 0;
        }
        return until.duration_since(now).as_millis() as u64;
    }
    0
}

/// 断言会话处于活动状态，如果暂停则返回错误
pub fn assert_session_active(account_id: &str) -> anyhow::Result<()> {
    if is_session_paused(account_id) {
        let remaining_min = (get_remaining_pause_ms(account_id) + 59_999) / 60_000;
        return Err(anyhow::anyhow!(
            "session paused for account_id={}, {} min remaining (errcode {})",
            account_id,
            remaining_min,
            SESSION_EXPIRED_ERRCODE
        ));
    }
    Ok(())
}

/// 重置内部状态（仅用于测试）
pub fn _reset_for_test() {
    let mut map = PAUSE_UNTIL_MAP.lock().unwrap();
    map.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_pause() {
        _reset_for_test();
        
        let account_id = "test-account";
        
        // 初始状态未暂停
        assert!(!is_session_paused(account_id));
        assert_eq!(get_remaining_pause_ms(account_id), 0);
        assert!(assert_session_active(account_id).is_ok());
        
        // 暂停会话
        pause_session(account_id);
        assert!(is_session_paused(account_id));
        assert!(get_remaining_pause_ms(account_id) > 0);
        assert!(assert_session_active(account_id).is_err());
    }
}
