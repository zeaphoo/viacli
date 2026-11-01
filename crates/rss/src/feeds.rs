//! 内置默认订阅源（`useDefaultFeeds: true` 时使用）。
//!
//! 选取标准只有一条：**今天真的能用**（2026-09 逐一验证过 HTTP 200 且返回 XML）。
//! 刻意不放"看起来应该能用"的地址——一个默认订阅源长期 404，用户会以为采集器坏了。
//!
//! 默认值只是"开箱即用"的起点：用户可以 `useDefaultFeeds: false` 全关，
//! 或保留默认再往 `feeds` 里加自己的地址。

use crate::config::FeedConfig;

/// 默认订阅源清单。顺序即输出顺序，与网络无关。
pub fn default_feeds() -> Vec<FeedConfig> {
    [
        ("少数派", "https://sspai.com/feed"),
        ("阮一峰的网络日志", "https://www.ruanyifeng.com/blog/atom.xml"),
        ("36氪", "https://36kr.com/feed"),
        ("InfoQ 中文", "https://www.infoq.cn/feed"),
        ("V2EX", "https://www.v2ex.com/index.xml"),
        ("Solidot", "https://www.solidot.org/index.rss"),
        ("小众软件", "https://www.appinn.com/feed/"),
        ("IT之家", "https://www.ithome.com/rss/"),
        ("Hacker News", "https://news.ycombinator.com/rss"),
        ("GitHub Blog", "https://github.blog/feed/"),
    ]
    .into_iter()
    .map(|(name, url)| FeedConfig {
        url: url.to_string(),
        name: Some(name.to_string()),
        enabled: true,
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_non_empty_and_https() {
        let feeds = default_feeds();
        assert!(feeds.len() >= 5, "默认源太少了，开箱即用的价值有限");
        for f in &feeds {
            assert!(f.enabled);
            assert!(
                f.url.starts_with("https://"),
                "默认源应走 https: {}",
                f.url
            );
            assert!(f.name.is_some(), "默认源应带可读名字");
        }
    }

    #[test]
    fn defaults_have_no_duplicates() {
        let feeds = default_feeds();
        let mut urls: Vec<&str> = feeds.iter().map(|f| f.url.as_str()).collect();
        let n = urls.len();
        urls.sort_unstable();
        urls.dedup();
        assert_eq!(urls.len(), n, "默认源里有重复 URL");
    }
}
