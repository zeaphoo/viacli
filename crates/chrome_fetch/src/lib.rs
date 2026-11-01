use anyhow::Result;
use clap::{Parser, ValueEnum};
use chromiumoxide::{Browser, BrowserConfig};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Parser, Debug)]
pub struct ChromeFetchArgs {
    /// 要抓取的网页 URL
    pub url: String,

    /// 输出格式
    pub outformat: OutputFormat,

    /// 等待时间（毫秒），用于等待页面加载完成
    #[arg(long, default_value = "1000")]
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
}

#[derive(ValueEnum, Clone, Debug, PartialEq)]
pub enum OutputFormat {
    /// 文本格式输出
    Text,
    /// JSON 格式输出
    Json,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FetchResult {
    pub url: String,
    pub title: Option<String>,
    pub content: String,
    pub html: Option<String>,
    pub status: String,
}

pub async fn run(args: &ChromeFetchArgs) -> Result<()> {
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
    let (browser, mut handler) = Browser::launch(browser_config).await?;
    
    // 创建浏览器处理任务
    let handle = tokio::spawn(async move {
        use futures_util::StreamExt;
        while let Some(_) = handler.next().await {}
    });

    // 导航到 URL
    let page = browser
        .new_page(&args.url)
        .await?;

    // 等待指定时间让页面加载
    if args.wait_time > 0 {
        tokio::time::sleep(Duration::from_millis(args.wait_time)).await;
    }

    // 获取页面内容
    let html = page.content().await?;
    
    // 获取页面标题
    let title: Option<String> = match page.evaluate("document.title").await {
        Ok(result) => result.into_value::<String>().ok().and_then(|s| if s.is_empty() { None } else { Some(s) }),
        Err(_) => None,
    };
    
    // 提取纯文本内容 - 使用 JavaScript 执行
    let text_content: String = match page.evaluate("document.body.innerText").await {
        Ok(result) => result.into_value::<String>().unwrap_or_default(),
        Err(_) => String::new(),
    };

    // 构建结果
    let result = FetchResult {
        url: args.url.clone(),
        title,
        content: text_content,
        html: Some(html),
        status: "success".to_string(),
    };

    // 根据输出格式输出结果
    match args.outformat {
        OutputFormat::Text => {
            println!("URL: {}", result.url);
            if let Some(title) = &result.title {
                println!("Title: {}", title);
            }
            println!("\n--- Content ---\n");
            println!("{}", result.content);
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }

    // 关闭浏览器
    drop(browser);
    let _ = handle.await;

    Ok(())
}
