//! 抓取订阅源并解析为统一模型（协议无关：本模块只认识 RSS/Atom，不认识协议消息）。
//!
//! 为什么用 `feed-rs` 而不是自己解 XML：真实订阅源的方言差异极多
//! （RSS 0.91/2.0、Atom 0.3/1.0、`content:encoded`、错误的时间格式……），
//! 手写解析器会在这些边角上不断踩坑，而"解析失败"最后会变成用户报的 bug。
//! HTTP 用 `reqwest::blocking`：采集器本身是同步的，不引入 async 运行时。

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use feed_rs::model::{Entry, Feed};

use crate::config::{Config, FeedConfig};
use crate::protocol::ErrorKind;

/// 解析后的一个条目。
#[derive(Debug, Clone, PartialEq)]
pub struct FetchedEntry {
    pub id: String,
    pub title: Option<String>,
    pub link: Option<String>,
    pub author: Option<String>,
    pub published: Option<DateTime<Utc>>,
    pub updated: Option<DateTime<Utc>>,
    pub summary: Option<String>,
    pub content_html: Option<String>,
    pub categories: Vec<String>,
}

impl FetchedEntry {
    /// 用于去重与记录 `key` 的稳定标识：优先发布日期，其次更新时间。
    pub fn effective_time(&self) -> Option<DateTime<Utc>> {
        self.published.or(self.updated)
    }
}

/// 解析后的一个订阅源。
#[derive(Debug, Clone, PartialEq)]
pub struct FetchedFeed {
    pub config: FeedConfig,
    pub title: Option<String>,
    pub site_url: Option<String>,
    pub updated: Option<DateTime<Utc>>,
    /// 最多 `maxItemsPerFeed` 条，按 feed 给出的顺序（通常新→旧）。
    pub entries: Vec<FetchedEntry>,
}

/// 抓取失败。`kind` 遵循协议 §6.1，用来决定"这一轮要不要算失败"。
#[derive(Debug, Clone)]
pub struct FetchError {
    pub kind: ErrorKind,
    pub message: String,
}

impl FetchError {
    fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Network, message)
    }

    pub fn upstream(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Upstream, message)
    }

    pub fn parse(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Parse, message)
    }
}

/// 并发抓取全部订阅源。返回顺序与 `feeds` 一致（便于输出稳定、便于测试）。
pub fn fetch_all(feeds: &[FeedConfig], cfg: &Config) -> Vec<Result<FetchedFeed, FetchError>> {
    if feeds.is_empty() {
        return Vec::new();
    }
    let client = match build_client(cfg) {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("创建 HTTP 客户端失败: {e}");
            return feeds.iter().map(|_| Err(FetchError::network(&msg))).collect();
        }
    };

    // 用"取号 - 抓取 - 放回原位"的简单工作队列，而不是给每个源起一个线程：
    // 源可能有几十个，而并发上限是固定的。
    let queue = Mutex::new((0..feeds.len()).collect::<VecDeque<usize>>());
    let slots: Vec<Mutex<Option<Result<FetchedFeed, FetchError>>>> =
        (0..feeds.len()).map(|_| Mutex::new(None)).collect();
    let workers = cfg.max_concurrency.clamp(1, feeds.len().max(1));

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let idx = queue.lock().expect("队列锁中毒").pop_front();
                    let Some(idx) = idx else { break };
                    let result = fetch_one(&client, &feeds[idx], cfg);
                    *slots[idx].lock().expect("结果锁中毒") = Some(result);
                }
            });
        }
    });

    slots
        .into_iter()
        .map(|m| {
            m.into_inner()
                .expect("结果锁中毒")
                .expect("每个编号都应被抓取")
        })
        .collect()
}

fn build_client(cfg: &Config) -> Result<reqwest::blocking::Client, reqwest::Error> {
    reqwest::blocking::Client::builder()
        .user_agent(cfg.user_agent.clone())
        .timeout(Duration::from_millis(cfg.timeout_ms))
        // 连接超时单独收短：15s 里大部分时间不该耗在"连不上"上
        .connect_timeout(Duration::from_millis(cfg.timeout_ms.min(10_000)))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
}

fn fetch_one(
    client: &reqwest::blocking::Client,
    feed: &FeedConfig,
    cfg: &Config,
) -> Result<FetchedFeed, FetchError> {
    let resp = client
        .get(&feed.url)
        .send()
        .map_err(|e| classify(&e, &feed.url))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(FetchError::upstream(format!(
            "HTTP {} {}",
            status.as_u16(),
            feed.url
        )));
    }

    // 先限流再读：响应可能是几百 MB 的 HTML 错误页，读完再判大小就晚了。
    let limit = cfg.max_body_bytes as u64;
    let mut body = Vec::new();
    resp.take(limit + 1)
        .read_to_end(&mut body)
        .map_err(|e| FetchError::network(format!("读取响应失败: {e}")))?;
    if body.len() as u64 > limit {
        return Err(FetchError::upstream(format!(
            "响应超过 {} 字节上限，已放弃",
            cfg.max_body_bytes
        )));
    }

    let parsed = feed_rs::parser::parse(&body[..])
        .map_err(|e| FetchError::parse(format!("解析订阅源失败: {e}")))?;
    Ok(normalize(feed, parsed, cfg))
}

fn classify(e: &reqwest::Error, url: &str) -> FetchError {
    if e.is_timeout() || e.is_connect() || e.is_request() {
        FetchError::network(format!("请求 {url} 失败: {e}"))
    } else {
        FetchError::upstream(format!("请求 {url} 失败: {e}"))
    }
}

/// 把 `feed-rs` 的模型转成本采集器的模型（只保留协议消息里用得到的字段）。
pub fn normalize(config: &FeedConfig, feed: Feed, cfg: &Config) -> FetchedFeed {
    let title = feed.title.map(|t| t.content).filter(|s| !s.trim().is_empty());
    let site_url = pick_link(&feed.links);
    let feed_updated = feed.updated;
    let feed_author = feed.authors.first().map(|p| p.name.clone());

    let entries = feed
        .entries
        .into_iter()
        // maxItemsPerFeed 是"每个源的窗口"：更旧的条目视为超出关注范围，直接不看。
        .take(cfg.max_items_per_feed)
        .map(|e| normalize_entry(e, feed_author.as_deref(), cfg))
        .collect();

    FetchedFeed {
        config: config.clone(),
        title,
        site_url,
        updated: feed_updated,
        entries,
    }
}

fn normalize_entry(e: Entry, feed_author: Option<&str>, cfg: &Config) -> FetchedEntry {
    // 先把各字段拆出来：`Entry` 里有多个 `Option`，边借边移会让借用检查器很为难。
    let Entry {
        id: raw_id,
        title,
        updated,
        authors,
        content,
        links,
        summary,
        categories,
        published,
        ..
    } = e;

    let link = pick_link(&links);
    let title = title.map(|t| t.content).filter(|s| !s.trim().is_empty());
    let author = authors
        .first()
        .map(|p| p.name.clone())
        .or_else(|| feed_author.map(str::to_string))
        .filter(|s| !s.trim().is_empty());

    let id = stable_id(&raw_id, link.as_deref(), title.as_deref(), published, updated);
    let mut categories: Vec<String> = categories
        .into_iter()
        .map(|c| c.term)
        .filter(|s| !s.trim().is_empty())
        .collect();
    categories.sort();
    categories.dedup();

    FetchedEntry {
        id,
        title,
        link,
        author,
        published,
        updated,
        summary: summary.map(|t| truncate_str(&t.content, cfg.max_content_chars)),
        content_html: content
            .and_then(|c| c.body)
            .map(|b| truncate_str(&b, cfg.max_content_chars))
            .filter(|s| !s.trim().is_empty()),
        categories,
    }
}

/// feed-rs 通常会给 `id`；为空时按"链接 → 标题+时间"退化成稳定值。
fn stable_id(
    raw_id: &str,
    link: Option<&str>,
    title: Option<&str>,
    published: Option<DateTime<Utc>>,
    updated: Option<DateTime<Utc>>,
) -> String {
    if !raw_id.trim().is_empty() {
        return raw_id.to_string();
    }
    if let Some(link) = link.filter(|l| !l.trim().is_empty()) {
        return link.to_string();
    }
    // 最后兜底：对标题+时间做哈希，保证同一篇稿子在重跑时得到同一个 id，
    // 而不是用一个随机值让去重失效。
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    title.unwrap_or("").hash(&mut hasher);
    published.or(updated).hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// 选一个"正文链接"：优先没有 rel / rel=alternate 的，否则取第一个。
fn pick_link(links: &[feed_rs::model::Link]) -> Option<String> {
    links
        .iter()
        .find(|l| matches!(l.rel.as_deref(), None | Some("alternate")))
        .or_else(|| links.first())
        .map(|l| l.href.clone())
        .filter(|s| !s.trim().is_empty())
}

/// 按**字符**（不是字节）截断，避免把多字节 UTF-8 切坏。
fn truncate_str(content: &str, max_chars: usize) -> String {
    if max_chars == 0 || content.chars().count() <= max_chars {
        return content.to_string();
    }
    let mut out: String = content.chars().take(max_chars).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;

    const RSS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>测试频道</title>
    <link>https://example.test/</link>
    <lastBuildDate>Mon, 06 Sep 2010 00:01:00 +0000</lastBuildDate>
    <item>
      <title>第一条</title>
      <link>https://example.test/a</link>
      <guid>guid-a</guid>
      <pubDate>Sun, 05 Sep 2010 16:20:00 +0000</pubDate>
      <description>hello</description>
      <category>tech</category>
    </item>
    <item>
      <title>第二条</title>
      <link>https://example.test/b</link>
      <guid>guid-b</guid>
      <pubDate>Mon, 06 Sep 2010 10:00:00 +0000</pubDate>
    </item>
  </channel>
</rss>"#;

    const ATOM: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Atom 频道</title>
  <link href="https://atom.test/"/>
  <updated>2010-09-06T00:01:00Z</updated>
  <entry>
    <title>原子条目</title>
    <link href="https://atom.test/x"/>
    <id>urn:uuid:1</id>
    <updated>2010-09-06T00:01:00Z</updated>
    <summary>摘要</summary>
  </entry>
</feed>"#;

    /// 起一个只服务一次请求的极简 HTTP 服务（纯 std，不引入测试依赖）。
    fn serve_once(status_line: &'static str, content_type: &'static str, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}/feed")
    }

    fn cfg_for(url: String) -> Config {
        Config {
            use_default_feeds: false,
            feeds: vec![FeedConfig {
                url,
                name: Some("本地测试".into()),
                enabled: true,
            }],
            ..Default::default()
        }
    }

    #[test]
    fn parses_rss_and_atom() {
        let cfg = Config::default();
        let rss = feed_rs::parser::parse(RSS.as_bytes()).unwrap();
        let out = normalize(
            &FeedConfig {
                url: "https://example.test/rss".into(),
                name: None,
                enabled: true,
            },
            rss,
            &cfg,
        );
        assert_eq!(out.title.as_deref(), Some("测试频道"));
        assert_eq!(out.site_url.as_deref(), Some("https://example.test/"));
        assert_eq!(out.entries.len(), 2);
        assert_eq!(out.entries[0].id, "guid-a");
        assert_eq!(out.entries[0].link.as_deref(), Some("https://example.test/a"));
        assert!(out.entries[0].published.is_some());
        assert_eq!(out.entries[0].categories, vec!["tech"]);

        let atom = feed_rs::parser::parse(ATOM.as_bytes()).unwrap();
        let out = normalize(
            &FeedConfig {
                url: "https://atom.test/feed".into(),
                name: None,
                enabled: true,
            },
            atom,
            &cfg,
        );
        assert_eq!(out.title.as_deref(), Some("Atom 频道"));
        assert_eq!(out.entries[0].id, "urn:uuid:1");
        assert_eq!(out.entries[0].summary.as_deref(), Some("摘要"));
    }

    #[test]
    fn fetches_over_http_and_parses() {
        let url = serve_once("200 OK", "application/rss+xml", RSS);
        let cfg = cfg_for(url);
        let feeds = cfg.resolve_feeds();
        let results = fetch_all(&feeds, &cfg);
        assert_eq!(results.len(), 1);
        let feed = results[0].as_ref().unwrap();
        assert_eq!(feed.entries.len(), 2);
        assert_eq!(feed.title.as_deref(), Some("测试频道"));
    }

    #[test]
    fn http_error_is_upstream_not_panic() {
        let url = serve_once("500 Internal Server Error", "text/plain", "boom");
        let cfg = cfg_for(url);
        let feeds = cfg.resolve_feeds();
        let results = fetch_all(&feeds, &cfg);
        let err = results[0].as_ref().unwrap_err();
        assert_eq!(err.kind, ErrorKind::Upstream);
        assert!(err.message.contains("500"));
    }

    #[test]
    fn non_feed_body_is_a_parse_error() {
        let url = serve_once("200 OK", "text/html", "<html><body>not a feed</body></html>");
        let cfg = cfg_for(url);
        let feeds = cfg.resolve_feeds();
        let results = fetch_all(&feeds, &cfg);
        let err = results[0].as_ref().unwrap_err();
        assert_eq!(err.kind, ErrorKind::Parse);
    }

    #[test]
    fn max_items_per_feed_truncates_the_window() {
        let cfg = Config {
            max_items_per_feed: 1,
            ..Default::default()
        };
        let rss = feed_rs::parser::parse(RSS.as_bytes()).unwrap();
        let out = normalize(
            &FeedConfig {
                url: "https://example.test/rss".into(),
                name: None,
                enabled: true,
            },
            rss,
            &cfg,
        );
        assert_eq!(out.entries.len(), 1);
        assert_eq!(out.entries[0].id, "guid-a", "应取 feed 给出的顺序（通常最新在前）");
    }

    #[test]
    fn content_is_truncated_by_chars_not_bytes() {
        let cfg = Config {
            include_content: false,
            max_content_chars: 3,
            ..Default::default()
        };
        let rss = feed_rs::parser::parse(RSS.as_bytes()).unwrap();
        let out = normalize(
            &FeedConfig {
                url: "https://example.test/rss".into(),
                name: None,
                enabled: true,
            },
            rss,
            &cfg,
        );
        assert_eq!(out.entries[0].summary.as_deref(), Some("hel…"));
    }
}
