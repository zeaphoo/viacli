use anyhow::Result;
use clap::{Parser, ValueEnum};
use chromiumoxide::{Browser, BrowserConfig};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Parser, Debug)]
pub struct WebSearchArgs {
    /// 搜索关键词
    pub query: String,

    /// 搜索引擎
    #[arg(short, long, value_enum, default_value = "bing")]
    pub engine: SearchEngine,

    /// 输出格式
    #[arg(long, value_enum, default_value = "text")]
    pub outformat: OutputFormat,

    /// 等待时间（毫秒），用于等待页面加载完成
    #[arg(long, default_value = "2000")]
    pub wait_time: u64,

    /// 是否启用 JavaScript（默认启用）
    #[arg(long)]
    pub enable_js: bool,

    /// User-Agent 字符串
    #[arg(long)]
    pub user_agent: Option<String>,

    /// 视口宽度
    #[arg(long, default_value = "1920")]
    pub viewport_width: u32,

    /// 视口高度
    #[arg(long, default_value = "1080")]
    pub viewport_height: u32,

    /// 结果数量（最多显示的搜索结果数）
    #[arg(long, default_value = "10")]
    pub limit: usize,
}

#[derive(ValueEnum, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SearchEngine {
    /// Bing 搜索
    Bing,
    /// Brave 搜索
    Brave,
    /// DuckDuckGo 搜索
    DuckDuckGo,
}

impl SearchEngine {
    pub fn build_url(&self, query: &str) -> String {
        match self {
            SearchEngine::Bing => {
                format!("https://www.bing.com/search?q={}", urlencoding::encode(query))
            }
            SearchEngine::Brave => {
                format!("https://search.brave.com/search?q={}", urlencoding::encode(query))
            }
            SearchEngine::DuckDuckGo => {
                format!("https://duckduckgo.com/?q={}", urlencoding::encode(query))
            }
        }
    }
}

#[derive(ValueEnum, Clone, Debug, PartialEq)]
pub enum OutputFormat {
    /// 文本格式输出
    Text,
    /// JSON 格式输出
    Json,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchResult {
    pub engine: String,
    pub query: String,
    pub url: String,
    pub results: Vec<SearchItem>,
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SearchItem {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

pub async fn run(args: &WebSearchArgs) -> Result<()> {
    // 构建搜索 URL
    let search_url = args.engine.build_url(&args.query);
    
    // 配置浏览器 - 使用 chromiumoxide fetcher 自动下载 Chromium
    let mut browser_config = BrowserConfig::builder()
        .window_size(args.viewport_width, args.viewport_height)
        .arg("--no-sandbox")
        .arg("--disable-dev-shm-usage")
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--disable-software-rasterizer")
        .arg("--disable-setuid-sandbox")
        .arg("--disable-background-networking")
        .arg("--disable-default-apps")
        .arg("--disable-extensions")
        .arg("--disable-sync")
        .arg("--no-first-run")
        .arg("--disable-features=TranslateUI,BrowserSideNavigationPrompt,VizDisplayCompositor")
        .arg("--disable-web-security")
        .arg("--disable-ipc-flooding-protection")
        .arg("--disable-breakpad")
        .arg("--disable-component-update")
        .arg("--disable-dinosaur-easter-egg");

    // 设置 User-Agent
    if let Some(ua) = &args.user_agent {
        browser_config = browser_config.arg(format!("--user-agent={}", ua));
    } else {
        browser_config = browser_config.arg("--user-agent=Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36");
    }

    // 如果不启用 JavaScript，添加相关参数
    if !args.enable_js {
        browser_config = browser_config.arg("--disable-javascript");
    }

    let browser_config = browser_config
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to build browser config: {}", e))?;

    // 启动浏览器
    let (mut browser, mut handler) = Browser::launch(browser_config).await?;
    
    // 创建浏览器处理任务
    let handle = tokio::spawn(async move {
        use futures_util::StreamExt;
        while let Some(_) = handler.next().await {}
    });

    // 导航到搜索页面
    let page = browser
        .new_page(&search_url)
        .await?;

    // 等待指定时间让页面加载
    if args.wait_time > 0 {
        tokio::time::sleep(Duration::from_millis(args.wait_time)).await;
    }

    // 根据不同搜索引擎提取搜索结果
    let results = extract_search_results(&page, &args.engine, args.limit).await?;
    
    // 构建结果
    let result = SearchResult {
        engine: format!("{:?}", args.engine),
        query: args.query.clone(),
        url: search_url,
        results,
        status: "success".to_string(),
    };

    // 根据输出格式输出结果
    match args.outformat {
        OutputFormat::Text => {
            println!("Search Engine: {:?}", args.engine);
            println!("Query: {}", args.query);
            println!("URL: {}", result.url);
            println!("\n--- Search Results ---\n");
            
            for (i, item) in result.results.iter().enumerate() {
                println!("{}. {}", i + 1, item.title);
                println!("   URL: {}", item.url);
                println!("   {}\n", item.snippet);
            }
            
            if result.results.is_empty() {
                println!("No results found.");
            }
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }

    // 关闭浏览器
    browser.close().await?;
    drop(browser);
    
    // 给 handler 一点时间处理关闭事件
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    
    // 取消 handler 任务
    handle.abort();

    Ok(())
}

async fn extract_search_results(
    page: &chromiumoxide::Page,
    engine: &SearchEngine,
    limit: usize,
) -> Result<Vec<SearchItem>> {
    match engine {
        SearchEngine::Bing => extract_bing_results(page, limit).await,
        SearchEngine::Brave => extract_brave_results(page, limit).await,
        SearchEngine::DuckDuckGo => extract_duckduckgo_results(page, limit).await,
    }
}

async fn extract_bing_results(page: &chromiumoxide::Page, limit: usize) -> Result<Vec<SearchItem>> {
    // Bing 搜索结果通常位于 #b_results 容器中，每个结果在 li.b_algo 里
    let javascript_code = format!(r#"
        () => {{
            const results = [];
            const items = document.querySelectorAll('li.b_algo');
            for (let i = 0; i < Math.min({}, items.length); i++) {{
                const item = items[i];
                const titleEl = item.querySelector('h2 a');
                const snippetEl = item.querySelector('.b_caption p');
                if (titleEl && snippetEl) {{
                    results.push({{
                        title: titleEl.textContent.trim(),
                        url: titleEl.href,
                        snippet: snippetEl.textContent.trim()
                    }});
                }}
            }}
            return results;
        }}
    "#, limit);

    let result: Vec<SearchItem> = match page.evaluate(javascript_code.as_str()).await {
        Ok(res) => res.into_value().unwrap_or_default(),
        Err(_) => Vec::new(),
    };

    Ok(result)
}

async fn extract_brave_results(page: &chromiumoxide::Page, limit: usize) -> Result<Vec<SearchItem>> {
    // Brave 搜索结果通常在 article 标签中，类名包含 result
    let javascript_code = format!(r#"
        () => {{
            const results = [];
            const items = document.querySelectorAll('article');
            for (let i = 0; i < Math.min({}, items.length); i++) {{
                const item = items[i];
                const titleEl = item.querySelector('a[data-title]');
                const snippetEl = item.querySelector('.snippet');
                if (titleEl && snippetEl) {{
                    results.push({{
                        title: titleEl.getAttribute('data-title') || titleEl.textContent.trim(),
                        url: titleEl.href,
                        snippet: snippetEl.textContent.trim()
                    }});
                }}
            }}
            return results;
        }}
    "#, limit);

    let result: Vec<SearchItem> = match page.evaluate(javascript_code.as_str()).await {
        Ok(res) => res.into_value().unwrap_or_default(),
        Err(_) => Vec::new(),
    };

    Ok(result)
}

async fn extract_duckduckgo_results(page: &chromiumoxide::Page, limit: usize) -> Result<Vec<SearchItem>> {
    // DuckDuckGo 搜索结果通常在 article 标签中，类名为 yY8dD
    let javascript_code = format!(r#"
        () => {{
            const results = [];
            // 尝试多种选择器
            const selectors = [
                'article[data-layout="organic"]',
                '.result__body',
                'div[data-testid="result"]'
            ];
            
            let items = [];
            for (const selector of selectors) {{
                items = document.querySelectorAll(selector);
                if (items.length > 0) break;
            }}
            
            for (let i = 0; i < Math.min({}, items.length); i++) {{
                const item = items[i];
                const titleEl = item.querySelector('a.result__a, h2 a');
                const snippetEl = item.querySelector('.result__snippet, .result__body > p');
                if (titleEl && snippetEl) {{
                    results.push({{
                        title: titleEl.textContent.trim(),
                        url: titleEl.href,
                        snippet: snippetEl.textContent.trim()
                    }});
                }}
            }}
            return results;
        }}
    "#, limit);

    let result: Vec<SearchItem> = match page.evaluate(javascript_code.as_str()).await {
        Ok(res) => res.into_value().unwrap_or_default(),
        Err(_) => Vec::new(),
    };

    Ok(result)
}
