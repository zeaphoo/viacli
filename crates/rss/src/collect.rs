//! 采集逻辑：抓取 → 去重 → 产出协议消息。
//!
//! # 为什么需要 `state`（协议 §5）
//!
//! 订阅源不像 CPU 使用率那样"每秒都有意义"：一篇稿子在发布后几天内都是同一条。
//! 如果每轮都把整页条目重发一遍，宿主虽然有 `(kind, key, ts)` 幂等兜底，
//! 但"产出量"与"抓取量"会永远相等，健康统计看不出任何变化。
//! 所以这里用 `state` 记住"每个源最近见过哪些条目 id"，只产出新增的。
//!
//! # 与宿主"空轮 = 失败"的配合
//!
//! 宿主把"退出码 0 但一条数据都没有"判定为失败（协议 §11.3）。
//! 而"这一轮没有新条目"是完全正常的，因此每轮都必须至少产出一条**数据消息**。
//! 这里用三个状态 `sample`（成功源数/失败源数/新增条目数）承担这个职责——
//! 它们本身也是健康的真实读数，不是为凑数而造的假数据。

use std::collections::BTreeMap;
use std::io::{self, Write};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::Config;
use crate::fetch::{self, FetchedEntry, FetchedFeed};
use crate::protocol::{Kind, Retention, Sink, Unit};

/// 状态类读数的保留上限：1 天。它们是"3 秒粒度抖动读数"的近亲，不需要长期保留。
const R_DAY: Retention = Retention::Max(86_400_000);

/// 跨运行记忆（协议 §5）。`stream` 模式下同一份留在进程内。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    /// 上次真正发起网络抓取的时刻（Unix 毫秒）。
    #[serde(default)]
    pub last_fetch_ms: Option<i64>,
    /// `feed url` → 已见过的条目 id。按 URL 绑定，不参与条目身份判定。
    #[serde(default)]
    pub feeds: BTreeMap<String, FeedSeen>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeedSeen {
    #[serde(default)]
    pub seen: Vec<String>,
}

impl State {
    /// 从请求里的 `state` 反序列化；格式不对就当作"首次运行"，
    /// 而不是让一个坏状态把整轮采集打断。
    pub fn from_request(value: Option<&Value>) -> Self {
        value
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|_| json!({}))
    }
}

/// 一轮采集的结果，供调用方决定退出码。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RoundStats {
    pub feeds_total: usize,
    pub feeds_ok: usize,
    pub feeds_failed: usize,
    pub items_emitted: usize,
}

pub struct Collector {
    cfg: Config,
    state: State,
}

impl Collector {
    pub fn new(cfg: Config, state: State) -> Self {
        Self { cfg, state }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// 距上次抓取还不足 `intervalMs` 时，`oneshot` 这一轮应跳过网络请求。
    ///
    /// 宿主对 `oneshot` 的调度间隔是宿主自己的事（实测默认 60s，且不读数据源的
    /// `intervalMs`）。订阅源经不起 1 分钟一次的全量抓取，所以采集器**自己**限流：
    /// 这是"采集器 SHOULD 自行控制产出量"（协议 §4.5）的合理延伸。
    pub fn should_skip(&self, now: Option<i64>) -> bool {
        let (Some(last), Some(now)) = (self.state.last_fetch_ms, now) else {
            return false;
        };
        // 时钟回拨时 now-last 为负，不能当成"很久没抓"而狂刷；按"没到点"处理。
        now.saturating_sub(last) < self.cfg.interval_ms as i64
    }

    /// 跳过一轮：只发一条状态样本，保证宿主不会把它当成空轮。
    pub fn emit_skipped<W: Write>(&self, sink: &mut Sink<W>, now: Option<i64>) -> io::Result<()> {
        declare_status(sink)?;
        sink.sample_i64("rss/items_emitted", now, 0)
    }

    /// 抓取全部订阅源并产出记录。总是返回本轮的数据消息条数。
    pub fn collect<W: Write>(
        &mut self,
        sink: &mut Sink<W>,
        now: Option<i64>,
    ) -> io::Result<RoundStats> {
        declare_status(sink)?;

        let feeds = self.cfg.resolve_feeds();
        let results = fetch::fetch_all(&feeds, &self.cfg);

        let mut stats = RoundStats {
            feeds_total: feeds.len(),
            ..Default::default()
        };

        for (feed_cfg, result) in feeds.iter().zip(results) {
            match result {
                Ok(feed) => {
                    stats.feeds_ok += 1;
                    stats.items_emitted += self.emit_feed(sink, &feed, now)?;
                }
                Err(e) => {
                    stats.feeds_failed += 1;
                    // 单个源抓取失败是常态（限流、临时 5xx、证书过期），
                    // 不报 error 消息打断整轮，只在 stderr 与 log 里说明。
                    eprintln!(
                        "viacli-rss: 抓取 {} 失败（{:?}）: {}",
                        feed_cfg.url, e.kind, e.message
                    );
                    sink.log(
                        "warn",
                        &format!("抓取 {} 失败: {}", feed_cfg.url, e.message),
                    )?;
                }
            }
        }

        if let Some(now) = now {
            self.state.last_fetch_ms = Some(now);
        }

        // 全部失败时不发状态样本：宿主对 stream 的"活着但没数据"监视会据此
        // 在几轮之后重启采集器——这正是"上游长期不可用"需要的反应。
        // 发样本会让它永远显示健康，把静默失败藏起来。错误本身由调用方
        // 根据 `feeds_ok` 决定怎么报（oneshot 直接以退出码 4 结束，stream 发一条 error）。
        if stats.feeds_ok > 0 {
            sink.sample_i64("rss/feeds_ok", now, stats.feeds_ok as i64)?;
            sink.sample_i64("rss/feeds_failed", now, stats.feeds_failed as i64)?;
            sink.sample_i64("rss/items_emitted", now, stats.items_emitted as i64)?;
        }

        Ok(stats)
    }

    /// 产出单个源的新条目，并推进该源的"已见"窗口。
    fn emit_feed<W: Write>(
        &mut self,
        sink: &mut Sink<W>,
        feed: &FetchedFeed,
        now: Option<i64>,
    ) -> io::Result<usize> {
        let url = feed.config.url.clone();
        let display_title = feed
            .config
            .name
            .clone()
            .or_else(|| feed.title.clone())
            .unwrap_or_else(|| url.clone());

        let previously_seen = self
            .state
            .feeds
            .get(&url)
            .map(|f| f.seen.clone())
            .unwrap_or_default();

        let mut emitted = 0;
        let mut window = Vec::with_capacity(feed.entries.len());
        for entry in &feed.entries {
            if !previously_seen.iter().any(|s| s == &entry.id) {
                let ts = entry
                    .effective_time()
                    .map(|d| d.timestamp_millis())
                    .or(now);
                let key = format!("{url}#{}", entry.id);
                sink.record(
                    "rss.item",
                    Some(&key),
                    ts,
                    item_payload(&url, &display_title, feed, entry, &self.cfg),
                )?;
                emitted += 1;
            }
            window.push(entry.id.clone());
        }

        // 只保留当前窗口：窗口外的旧 id 迟早会被重新看到，但那时条目早已超时，
        // 留在 state 里除了撑大它没有任何用处。
        self.state.feeds.insert(url, FeedSeen { seen: window });
        Ok(emitted)
    }
}

fn declare_status<W: Write>(sink: &mut Sink<W>) -> io::Result<()> {
    sink.declare(
        "rss/feeds_ok",
        Kind::Gauge,
        Unit::Count,
        "本轮成功抓取的订阅源数",
        R_DAY,
    )?;
    sink.declare(
        "rss/feeds_failed",
        Kind::Gauge,
        Unit::Count,
        "本轮抓取失败的订阅源数",
        R_DAY,
    )?;
    sink.declare(
        "rss/items_emitted",
        Kind::Gauge,
        Unit::Count,
        "本轮产出的新条目数",
        R_DAY,
    )
}

/// 组装一条 `rss.item` 记录的 payload。
///
/// 字段名自带口径（`publishedMs` / `contentHtml`），因为 `record` 没有
/// `unit`/`help` 字段，字段名是宿主唯一能依赖的说明（协议 §8.1）。
fn item_payload(
    feed_url: &str,
    feed_title: &str,
    feed: &FetchedFeed,
    entry: &FetchedEntry,
    cfg: &Config,
) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert("feedUrl".into(), json!(feed_url));
    payload.insert("feedTitle".into(), json!(feed_title));
    if let Some(site) = &feed.site_url {
        payload.insert("feedSiteUrl".into(), json!(site));
    }
    payload.insert("id".into(), json!(entry.id));
    if let Some(title) = &entry.title {
        payload.insert("title".into(), json!(title));
    }
    if let Some(link) = &entry.link {
        payload.insert("link".into(), json!(link));
    }
    if let Some(author) = &entry.author {
        payload.insert("author".into(), json!(author));
    }
    if let Some(time) = entry.effective_time() {
        payload.insert("publishedAt".into(), json!(time.to_rfc3339()));
        payload.insert("publishedMs".into(), json!(time.timestamp_millis()));
    }
    if !entry.categories.is_empty() {
        payload.insert("categories".into(), json!(entry.categories));
    }
    if cfg.include_content {
        if let Some(summary) = &entry.summary {
            payload.insert("summary".into(), json!(summary));
        }
        if let Some(content) = &entry.content_html {
            payload.insert("contentHtml".into(), json!(content));
        }
    }
    Value::Object(payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FeedConfig;

    fn feed_with(ids: &[&str]) -> FetchedFeed {
        FetchedFeed {
            config: FeedConfig {
                url: "https://a.test/feed".into(),
                name: Some("A".into()),
                enabled: true,
            },
            title: Some("A".into()),
            site_url: Some("https://a.test/".into()),
            updated: None,
            entries: ids
                .iter()
                .map(|id| FetchedEntry {
                    id: (*id).into(),
                    title: Some(format!("t-{id}")),
                    link: Some(format!("https://a.test/{id}")),
                    author: None,
                    published: None,
                    updated: None,
                    summary: None,
                    content_html: None,
                    categories: vec![],
                })
                .collect(),
        }
    }

    fn emit(collector: &mut Collector, feed: &FetchedFeed) -> (usize, Vec<Value>) {
        let mut sink = Sink::new(Vec::new());
        let n = collector.emit_feed(&mut sink, feed, Some(1_000)).unwrap();
        let msgs: Vec<Value> = String::from_utf8(sink.into_inner())
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        (n, msgs)
    }

    #[test]
    fn only_new_entries_are_emitted() {
        let cfg = Config::default();
        let mut c = Collector::new(cfg, State::default());

        let (n, msgs) = emit(&mut c, &feed_with(&["1", "2"]));
        assert_eq!(n, 2);
        assert_eq!(msgs.iter().filter(|m| m["type"] == "record").count(), 2);

        // 同一个窗口再来一次：不应重复产出
        let (n, msgs) = emit(&mut c, &feed_with(&["1", "2"]));
        assert_eq!(n, 0);
        assert!(msgs.is_empty());

        // feed 顶部多了一条新稿：只产出这一条，窗口也要带上旧的
        let (n, _) = emit(&mut c, &feed_with(&["3", "2", "1"]));
        assert_eq!(n, 1);
        assert_eq!(c.state().feeds["https://a.test/feed"].seen, vec!["3", "2", "1"]);
    }

    #[test]
    fn state_survives_serialization() {
        let mut c = Collector::new(Config::default(), State::default());
        emit(&mut c, &feed_with(&["1"]));
        let value = c.state().to_value();
        let restored = State::from_request(Some(&value));
        assert_eq!(restored.feeds.len(), 1);
        assert_eq!(restored.feeds["https://a.test/feed"].seen, vec!["1"]);
    }

    #[test]
    fn skip_respects_the_interval() {
        let cfg = Config {
            interval_ms: 900_000,
            ..Default::default()
        };
        let mut c = Collector::new(cfg, State::default());
        assert!(!c.should_skip(Some(1_000)), "首次运行不该跳过");
        c.state.last_fetch_ms = Some(1_000_000);
        assert!(c.should_skip(Some(1_000_000 + 899_999)));
        assert!(!c.should_skip(Some(1_000_000 + 900_000)));
        // 时钟回拨：不能因为 now < last 就认定"很久没抓"
        assert!(c.should_skip(Some(999_000)));
        assert!(!c.should_skip(None));
    }

    #[test]
    fn payload_has_units_in_field_names() {
        let cfg = Config::default();
        let feed = feed_with(&["1"]);
        let payload = item_payload(
            "https://a.test/feed",
            "A",
            &feed,
            &feed.entries[0],
            &cfg,
        );
        assert_eq!(payload["feedUrl"], "https://a.test/feed");
        assert_eq!(payload["id"], "1");
        assert_eq!(payload["link"], "https://a.test/1");
        // 默认不带正文/摘要
        assert!(payload.get("summary").is_none());
    }
}
