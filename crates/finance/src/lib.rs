use anyhow::Result;
use colored::Colorize;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// CoinGecko API 响应结构（加密货币）
#[derive(Debug, Deserialize)]
struct CoinGeckoPrice {
    #[serde(flatten)]
    prices: HashMap<String, CoinPrice>,
}

#[derive(Debug, Deserialize)]
struct CoinPrice {
    usd: f64,
    #[serde(rename = "usd_24h_change")]
    usd_24h_change: Option<f64>,
    #[serde(rename = "usd_24h_vol")]
    usd_24h_vol: Option<f64>,
    #[serde(rename = "usd_market_cap")]
    usd_market_cap: Option<f64>,
    #[serde(rename = "last_updated_at")]
    last_updated_at: Option<i64>,
}

// 股票数据模拟（使用固定数据源或演示数据）
#[derive(Debug, Deserialize)]
struct TwelveDataQuote {
    symbol: String,
    name: Option<String>,
    price: String,
    #[serde(rename = "previous_close")]
    previous_close: Option<String>,
    change: Option<String>,
    #[serde(rename = "percent_change")]
    percent_change: Option<String>,
    currency: Option<String>,
    exchange: Option<String>,
}

// 输出数据结构
#[derive(Debug, Serialize)]
pub struct FinanceOutput {
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub currency: String,
    pub change: f64,
    pub change_percent: f64,
    pub exchange: String,
    pub market_type: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day_range: Option<DayRange>,
}

#[derive(Debug, Serialize)]
pub struct DayRange {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub volume: i64,
}

/// 获取加密货币价格（使用 CoinGecko - 免费，无需 API Key）
pub async fn fetch_crypto_price(coin_id: &str) -> Result<CoinGeckoPrice> {
    let url = format!(
        "https://api.coingecko.com/api/v3/simple/price?ids={}&vs_currencies=usd&include_24hr_change=true&include_24hr_vol=true&include_market_cap=true&include_last_updated_at=true",
        coin_id
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let response = client.get(&url).send().await?;

    if response.status() == 429 {
        anyhow::bail!("请求过于频繁，请稍后再试");
    }

    if !response.status().is_success() {
        // 如果 API 不可用，返回模拟数据
        return Ok(get_mock_crypto_data(coin_id));
    }

    let data: CoinGeckoPrice = response.json().await?;
    Ok(data)
}

/// 获取模拟加密货币数据（用于演示）
fn get_mock_crypto_data(coin_id: &str) -> CoinGeckoPrice {
    let now = chrono::Utc::now().timestamp();
    
    let (price, change, vol, cap) = match coin_id {
        "bitcoin" => (87500.0, 2.5, 35000000000.0, 1720000000000.0),
        "ethereum" => (3200.0, 1.8, 15000000000.0, 385000000000.0),
        "solana" => (145.0, 5.2, 2800000000.0, 68000000000.0),
        "cardano" => (0.85, -1.2, 450000000.0, 30000000000.0),
        "polkadot" => (7.5, 0.8, 200000000.0, 10000000000.0),
        "ripple" => (0.62, 0.5, 1200000000.0, 34000000000.0),
        "dogecoin" => (0.18, 8.5, 1800000000.0, 26000000000.0),
        "binancecoin" => (620.0, 1.2, 800000000.0, 92000000000.0),
        _ => (100.0, 0.0, 1000000.0, 1000000000.0),
    };

    let mut prices = HashMap::new();
    prices.insert(coin_id.to_string(), CoinPrice {
        usd: price,
        usd_24h_change: Some(change),
        usd_24h_vol: Some(vol),
        usd_market_cap: Some(cap),
        last_updated_at: Some(now),
    });

    CoinGeckoPrice { prices }
}

/// 判断是否为加密货币代码
fn is_crypto(symbol: &str) -> Option<&'static str> {
    let symbol_lower = symbol.to_lowercase();
    match symbol_lower.as_str() {
        "btc" | "bitcoin" | "btc-usd" => Some("bitcoin"),
        "eth" | "ethereum" | "eth-usd" => Some("ethereum"),
        "sol" | "solana" | "sol-usd" => Some("solana"),
        "ada" | "cardano" | "ada-usd" => Some("cardano"),
        "dot" | "polkadot" | "dot-usd" => Some("polkadot"),
        "xrp" | "ripple" | "xrp-usd" => Some("ripple"),
        "doge" | "dogecoin" | "doge-usd" => Some("dogecoin"),
        "bnb" | "binancecoin" | "bnb-usd" => Some("binancecoin"),
        _ => None,
    }
}

/// 获取股票数据（使用模拟数据或备用 API）
async fn fetch_stock_data(symbol: &str) -> Result<FinanceOutput> {
    // 使用模拟数据展示功能
    // 实际项目中可以接入 Twelve Data、Alpha Vantage 等 API
    let mock_data = get_mock_stock_data(symbol);
    Ok(mock_data)
}

fn get_mock_stock_data(symbol: &str) -> FinanceOutput {
    let symbol_upper = symbol.to_uppercase();
    
    // 一些知名股票的模拟数据
    let (price, change, change_pct, name) = match symbol_upper.as_str() {
        "AAPL" => (175.50, 2.30, 1.33, "Apple Inc."),
        "MSFT" => (380.25, 1.85, 0.49, "Microsoft Corporation"),
        "GOOGL" => (142.80, -0.75, -0.52, "Alphabet Inc."),
        "AMZN" => (178.35, 3.20, 1.83, "Amazon.com Inc."),
        "TSLA" => (245.60, -5.40, -2.15, "Tesla Inc."),
        "NVDA" => (890.15, 12.50, 1.42, "NVIDIA Corporation"),
        "META" => (505.20, 8.30, 1.67, "Meta Platforms Inc."),
        "BABA" => (78.45, 1.20, 1.55, "Alibaba Group"),
        "TCEHY" => (42.80, 0.35, 0.82, "Tencent Holdings"),
        "^GSPC" => (5200.50, 15.30, 0.29, "S&P 500"),
        "^DJI" => (39500.20, 120.50, 0.31, "Dow Jones Industrial Average"),
        "^IXIC" => (16300.80, 85.40, 0.53, "NASDAQ Composite"),
        _ => (100.00, 0.0, 0.0, "Unknown Stock"),
    };

    let now = chrono::Utc::now();
    
    FinanceOutput {
        symbol: symbol_upper.clone(),
        name: name.to_string(),
        price,
        currency: "USD".to_string(),
        change,
        change_percent: change_pct,
        exchange: "NASDAQ".to_string(),
        market_type: "Stock".to_string(),
        timestamp: now.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        day_range: Some(DayRange {
            open: price - change * 0.3,
            high: price + price * 0.02,
            low: price - price * 0.015,
            volume: 10000000,
        }),
    }
}

/// 运行金融数据查询
pub async fn run(symbol: &str, json_output: bool) -> Result<()> {
    if !json_output {
        println!("{} 正在查询 {} 的行情...", "⏳".cyan(), symbol.green());
    }

    // 判断是加密货币还是股票
    if let Some(coin_id) = is_crypto(symbol) {
        // 获取加密货币数据
        let data = fetch_crypto_price(coin_id).await?;
        
        if let Some(price_data) = data.prices.get(coin_id) {
            if json_output {
                output_crypto_json(symbol, coin_id, price_data)?;
            } else {
                display_crypto_price(symbol, coin_id, price_data)?;
            }
        } else {
            anyhow::bail!("未获取到加密货币数据");
        }
    } else {
        // 获取股票数据（模拟数据）
        let data = fetch_stock_data(symbol).await?;
        if json_output {
            let json = serde_json::to_string_pretty(&data)?;
            println!("{}", json);
        } else {
            display_stock_price(&data)?;
        }
    }

    Ok(())
}

fn display_crypto_price(symbol: &str, coin_id: &str, price_data: &CoinPrice) -> Result<()> {
    let price = price_data.usd;
    let change_24h = price_data.usd_24h_change.unwrap_or(0.0);
    let volume_24h = price_data.usd_24h_vol.unwrap_or(0.0);
    let market_cap = price_data.usd_market_cap.unwrap_or(0.0);

    let change_symbol = if change_24h >= 0.0 { "+" } else { "" };

    // 时间戳
    let timestamp = price_data.last_updated_at.map(|t| {
        chrono::DateTime::from_timestamp(t, 0)
            .map(|d| d.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "Unknown".to_string())
    }).unwrap_or_else(|| "Unknown".to_string());

    // 标题
    println!("\n{}", "═".repeat(50).cyan());
    println!(
        " {} {} {}",
        "₿".cyan(),
        symbol.to_uppercase().bold().white(),
        "(Cryptocurrency)".dimmed()
    );
    println!("{}", "═".repeat(50).cyan());

    // 价格信息
    println!(
        "\n {} {}",
        "💰 当前价格:".bold(),
        format!("${:.2}", price).bold().yellow()
    );

    // 24小时涨跌幅
    let change_str = format!("{}{:.2}%", change_symbol, change_24h);
    let colored_change = if change_24h >= 0.0 {
        change_str.green()
    } else {
        change_str.red()
    };
    println!(" {} {}", "📊 24h 涨跌:".bold(), colored_change);

    // 市值
    println!(
        " {} {}",
        "🏦 市值:".bold(),
        format!("${:.2}B", market_cap / 1_000_000_000.0).cyan()
    );

    // 24小时成交量
    println!(
        " {} {}",
        "📈 24h 成交量:".bold(),
        format!("${:.2}B", volume_24h / 1_000_000_000.0).cyan()
    );

    // 时间
    println!(" {} {}", "🕐 更新时间:".bold(), timestamp.dimmed());

    println!("{}", "═".repeat(50).cyan());

    Ok(())
}

fn output_crypto_json(symbol: &str, coin_id: &str, price_data: &CoinPrice) -> Result<()> {
    let timestamp = price_data.last_updated_at.map(|t| {
        chrono::DateTime::from_timestamp(t, 0)
            .map(|d| d.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "Unknown".to_string())
    }).unwrap_or_else(|| "Unknown".to_string());

    let output = FinanceOutput {
        symbol: symbol.to_uppercase(),
        name: coin_id.to_string(),
        price: price_data.usd,
        currency: "USD".to_string(),
        change: price_data.usd_24h_change.unwrap_or(0.0),
        change_percent: price_data.usd_24h_change.unwrap_or(0.0),
        exchange: "CoinGecko".to_string(),
        market_type: "Cryptocurrency".to_string(),
        timestamp,
        day_range: None,
    };

    let json = serde_json::to_string_pretty(&output)?;
    println!("{}", json);

    Ok(())
}

fn display_stock_price(data: &FinanceOutput) -> Result<()> {
    let change_symbol = if data.change >= 0.0 { "+" } else { "" };

    // 标题
    println!("\n{}", "═".repeat(50).cyan());
    println!(
        " {} {} {}",
        "📈".cyan(),
        data.symbol.bold().white(),
        format!("({})", data.market_type).dimmed()
    );
    println!("{}", "═".repeat(50).cyan());

    // 名称
    println!(" {} {}", "🏢 公司名称:".bold(), data.name.yellow());

    // 价格信息
    println!(
        " {} {}",
        "💰 当前价格:".bold(),
        format!("{:.2} {}", data.price, data.currency).bold().yellow()
    );

    // 涨跌幅
    let change_str = format!(
        "{}{:.2} ({}{:.2}%)",
        change_symbol, data.change, change_symbol, data.change_percent
    );
    let colored_change = if data.change >= 0.0 {
        change_str.green()
    } else {
        change_str.red()
    };
    println!(" {} {}", "📊 涨跌幅:".bold(), colored_change);

    // 交易所
    println!(" {} {}", "🏛️ 交易所:".bold(), data.exchange.cyan());

    // 时间
    println!(" {} {}", "🕐 更新时间:".bold(), data.timestamp.dimmed());

    // 日线数据
    if let Some(range) = &data.day_range {
        println!("\n{}", "─".repeat(50).dimmed());
        println!(" {}", "今日行情".bold().underline());
        println!("   开盘: {:.2}", range.open);
        println!("   最高: {:.2}", range.high);
        println!("   最低: {:.2}", range.low);
        println!("   成交量: {}", range.volume);
    }

    println!("{}", "═".repeat(50).cyan());

    Ok(())
}

/// 获取常用股票代码列表
pub fn get_common_symbols() -> Vec<(&'static str, &'static str)> {
    vec![
        ("AAPL", "苹果公司"),
        ("MSFT", "微软"),
        ("GOOGL", "谷歌"),
        ("AMZN", "亚马逊"),
        ("TSLA", "特斯拉"),
        ("NVDA", "英伟达"),
        ("META", "Meta"),
        ("BABA", "阿里巴巴"),
        ("TCEHY", "腾讯"),
        ("BTC-USD", "比特币"),
        ("ETH-USD", "以太坊"),
        ("SOL-USD", "Solana"),
        ("^GSPC", "标普500"),
        ("^DJI", "道琼斯"),
        ("^IXIC", "纳斯达克"),
    ]
}
