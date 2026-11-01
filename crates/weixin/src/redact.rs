//! 日志脱敏模块
//!
//! 用于安全地记录日志，脱敏敏感信息

const DEFAULT_BODY_MAX_LEN: usize = 200;
const DEFAULT_TOKEN_PREFIX_LEN: usize = 6;

/// 截断字符串，超过长度时添加长度指示器
pub fn truncate(s: Option<&str>, max: usize) -> String {
    match s {
        None | Some("") => String::new(),
        Some(s) if s.len() <= max => s.to_string(),
        Some(s) => format!("{}...(len={})", &s[..max], s.len()),
    }
}

/// 脱敏 Token/密钥，只显示前几个字符和总长度
pub fn redact_token(token: Option<&str>, prefix_len: usize) -> String {
    match token {
        None | Some("") => "(none)".to_string(),
        Some(t) if t.len() <= prefix_len => format!("****(len={})", t.len()),
        Some(t) => format!("{}...(len={})", &t[..prefix_len], t.len()),
    }
}

/// 使用默认前缀长度脱敏 Token
pub fn redact_token_default(token: Option<&str>) -> String {
    redact_token(token, DEFAULT_TOKEN_PREFIX_LEN)
}

/// 脱敏 JSON 请求体，隐藏敏感字段
pub fn redact_body(body: Option<&str>, max_len: usize) -> String {
    let body = match body {
        None | Some("") => return "(empty)".to_string(),
        Some(b) => b,
    };

    // 脱敏已知敏感字段
    let re = regex::Regex::new(r#""(context_token|bot_token|token|authorization|Authorization)"\s*:\s*"[^"]*""#).unwrap();
    let redacted = re.replace_all(body, r#""$1":"***""#).to_string();

    if redacted.len() <= max_len {
        redacted
    } else {
        format!(
            "{}...(truncated, totalLen={})",
            &redacted[..max_len],
            redacted.len()
        )
    }
}

/// 使用默认最大长度脱敏请求体
pub fn redact_body_default(body: Option<&str>) -> String {
    redact_body(body, DEFAULT_BODY_MAX_LEN)
}

/// 脱敏 URL，移除查询字符串（通常包含签名/Token）
pub fn redact_url(raw_url: &str) -> String {
    match url::Url::parse(raw_url) {
        Ok(u) => {
            let base = format!("{}{}", u.origin().ascii_serialization(), u.path());
            if u.query().is_some() {
                format!("{}?<redacted>", base)
            } else {
                base
            }
        }
        Err(_) => truncate(Some(raw_url), 80),
    }
}

/// 脱敏请求头
pub fn redact_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    const SENSITIVE_HEADERS: &[&str] = &[
        "authorization",
        "x-auth-token",
        "cookie",
        "x-api-key",
        "x-wechat-uin",
    ];

    headers
        .iter()
        .map(|(k, v)| {
            let k_lower = k.to_lowercase();
            if SENSITIVE_HEADERS.iter().any(|h| k_lower.contains(*h)) {
                (k.clone(), redact_token(Some(v), 4))
            } else {
                (k.clone(), v.clone())
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate() {
        assert_eq!(truncate(None, 10), "");
        assert_eq!(truncate(Some(""), 10), "");
        assert_eq!(truncate(Some("hello"), 10), "hello");
        assert_eq!(truncate(Some("hello world"), 5), "hello...(len=11)");
    }

    #[test]
    fn test_redact_token() {
        assert_eq!(redact_token(None, 6), "(none)");
        assert_eq!(redact_token(Some(""), 6), "(none)");
        assert_eq!(redact_token(Some("short"), 6), "****(len=5)");
        assert_eq!(
            redact_token(Some("long_token_here"), 6),
            "long_t...(len=15)"
        );
    }

    #[test]
    fn test_redact_url() {
        assert_eq!(
            redact_url("https://example.com/path"),
            "https://example.com/path"
        );
        assert_eq!(
            redact_url("https://example.com/path?token=secret"),
            "https://example.com/path?<redacted>"
        );
    }
}
