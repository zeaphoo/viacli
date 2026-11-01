use anyhow::Result;
use colored::Colorize;
use serde::{Deserialize, Serialize};

// wttr.in 返回的 JSON 格式
#[derive(Debug, Deserialize)]
pub struct WeatherInfo {
    #[serde(rename = "current_condition")]
    pub current_condition: Vec<CurrentCondition>,
    #[serde(rename = "nearest_area")]
    pub nearest_area: Vec<NearestArea>,
    pub weather: Vec<DailyWeather>,
}

#[derive(Debug, Deserialize)]
pub struct CurrentCondition {
    #[serde(rename = "temp_C")]
    pub temp_c: String,
    #[serde(rename = "temp_F")]
    pub temp_f: String,
    pub humidity: String,
    #[serde(rename = "weatherDesc")]
    pub weather_desc: Vec<WeatherDesc>,
    #[serde(rename = "windspeedKmph")]
    pub wind_speed: String,
    #[serde(rename = "winddir16Point")]
    pub wind_dir: String,
    #[serde(rename = "FeelsLikeC")]
    pub feels_like_c: String,
    #[serde(rename = "pressure")]
    pub pressure: String,
    #[serde(rename = "visibility")]
    pub visibility: String,
}

#[derive(Debug, Deserialize)]
pub struct WeatherDesc {
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct NearestArea {
    #[serde(rename = "areaName")]
    pub area_name: Vec<NameValue>,
    pub country: Vec<NameValue>,
    pub region: Vec<NameValue>,
}

#[derive(Debug, Deserialize)]
pub struct NameValue {
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct DailyWeather {
    pub date: String,
    #[serde(rename = "maxtempC")]
    pub max_temp_c: String,
    #[serde(rename = "mintempC")]
    pub min_temp_c: String,
    #[serde(rename = "avgtempC")]
    pub avg_temp_c: String,
}

pub async fn fetch_weather(city: &str) -> Result<WeatherInfo> {
    let url = format!(
        "https://wttr.in/{}?format=j1&lang=zh",
        urlencoding::encode(city)
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let response = client.get(&url).send().await?;

    if !response.status().is_success() {
        anyhow::bail!("获取天气信息失败: HTTP {}", response.status());
    }

    let weather: WeatherInfo = response.json().await?;
    Ok(weather)
}

pub fn display_weather(weather: &WeatherInfo, detailed: bool) -> Result<()> {
    let current = weather
        .current_condition
        .first()
        .ok_or_else(|| anyhow::anyhow!("无法获取当前天气信息"))?;

    let area = weather
        .nearest_area
        .first()
        .ok_or_else(|| anyhow::anyhow!("无法获取位置信息"))?;

    let default_unknown = "未知".to_string();
    let city_name = area.area_name.first().map(|n| &n.value).unwrap_or(&default_unknown);
    let country = area.country.first().map(|n| &n.value).unwrap_or(&default_unknown);
    let region = area.region.first().map(|n| &n.value).unwrap_or(&default_unknown);

    let weather_desc = current
        .weather_desc
        .first()
        .map(|d| &d.value)
        .unwrap_or(&default_unknown);

    // 标题
    println!("\n{}", "═".repeat(50).cyan());
    println!(
        " {} {} · {} · {}",
        "🌍".cyan(),
        city_name.bold().white(),
        region.yellow(),
        country.dimmed()
    );
    println!("{}", "═".repeat(50).cyan());

    // 当前天气
    println!(
        "\n {} {}  {}",
        get_weather_icon(weather_desc),
        weather_desc.bold().cyan(),
        format!("{}°C", current.temp_c).bold().yellow()
    );

    // 体感温度
    println!(
        " {} 体感温度: {}°C",
        "🌡️".red(),
        current.feels_like_c.yellow()
    );

    // 湿度
    println!(" {} 湿度: {}", "💧".blue(), format!("{}%", current.humidity).cyan());

    // 风速
    println!(
        " {} 风速: {} km/h {}",
        "💨".white(),
        current.wind_speed.cyan(),
        current.wind_dir.yellow()
    );

    // 详细信息
    if detailed {
        println!("\n{}", "─".repeat(50).dimmed());
        println!(" {}", "详细数据".bold().underline());
        println!("   气压: {} hPa", current.pressure.cyan());
        println!("   能见度: {} km", current.visibility.cyan());
        println!("   华氏温度: {}°F", current.temp_f.cyan());
    }

    // 未来几天预报
    if !weather.weather.is_empty() {
        println!("\n{}", "─".repeat(50).dimmed());
        println!(" {}", "未来预报".bold().underline());

        for day in weather.weather.iter().take(3) {
            println!(
                "   {}  最高 {}°C  最低 {}°C  平均 {}°C",
                day.date.dimmed(),
                day.max_temp_c.red(),
                day.min_temp_c.blue(),
                day.avg_temp_c.cyan()
            );
        }
    }

    println!("{}", "═".repeat(50).cyan());

    Ok(())
}

fn get_weather_icon(desc: &str) -> &'static str {
    let desc_lower = desc.to_lowercase();
    if desc_lower.contains("sun") || desc_lower.contains("clear") || desc_lower.contains("晴") {
        "☀️"
    } else if desc_lower.contains("cloud") || desc_lower.contains("阴") || desc_lower.contains("多云") {
        "☁️"
    } else if desc_lower.contains("rain") || desc_lower.contains("雨") {
        "🌧️"
    } else if desc_lower.contains("snow") || desc_lower.contains("雪") {
        "❄️"
    } else if desc_lower.contains("thunder") || desc_lower.contains("storm") || desc_lower.contains("雷") {
        "⛈️"
    } else if desc_lower.contains("fog") || desc_lower.contains("mist") || desc_lower.contains("雾") {
        "🌫️"
    } else if desc_lower.contains("partly") {
        "⛅"
    } else {
        "🌤️"
    }
}

/// 天气查询结果（用于 JSON 输出）
#[derive(Debug, Serialize)]
pub struct WeatherOutput {
    pub city: String,
    pub region: String,
    pub country: String,
    pub temperature_c: String,
    pub temperature_f: String,
    pub feels_like_c: String,
    pub humidity: String,
    pub weather_desc: String,
    pub wind_speed: String,
    pub wind_dir: String,
    pub pressure: String,
    pub visibility: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forecast: Option<Vec<DailyForecast>>,
}

#[derive(Debug, Serialize)]
pub struct DailyForecast {
    pub date: String,
    pub max_temp_c: String,
    pub min_temp_c: String,
    pub avg_temp_c: String,
}

/// 运行天气查询（供其他 crate 调用）
pub async fn run(city: &str, detailed: bool, json_output: bool) -> Result<()> {
    if !json_output {
        println!("{} 正在查询 {} 的天气...", "⏳".cyan(), city.green());
    }

    let weather = fetch_weather(city).await?;

    if json_output {
        output_json(&weather, detailed)?;
    } else {
        display_weather(&weather, detailed)?;
    }

    Ok(())
}

fn output_json(weather: &WeatherInfo, _detailed: bool) -> Result<()> {
    let current = weather
        .current_condition
        .first()
        .ok_or_else(|| anyhow::anyhow!("无法获取当前天气信息"))?;

    let area = weather
        .nearest_area
        .first()
        .ok_or_else(|| anyhow::anyhow!("无法获取位置信息"))?;

    let default_unknown = "未知".to_string();
    let city_name = area.area_name.first().map(|n| &n.value).unwrap_or(&default_unknown);
    let country = area.country.first().map(|n| &n.value).unwrap_or(&default_unknown);
    let region = area.region.first().map(|n| &n.value).unwrap_or(&default_unknown);

    let weather_desc = current
        .weather_desc
        .first()
        .map(|d| &d.value)
        .unwrap_or(&default_unknown);

    let forecast = if !weather.weather.is_empty() {
        Some(
            weather
                .weather
                .iter()
                .take(3)
                .map(|d| DailyForecast {
                    date: d.date.clone(),
                    max_temp_c: d.max_temp_c.clone(),
                    min_temp_c: d.min_temp_c.clone(),
                    avg_temp_c: d.avg_temp_c.clone(),
                })
                .collect(),
        )
    } else {
        None
    };

    let output = WeatherOutput {
        city: city_name.clone(),
        region: region.clone(),
        country: country.clone(),
        temperature_c: current.temp_c.clone(),
        temperature_f: current.temp_f.clone(),
        feels_like_c: current.feels_like_c.clone(),
        humidity: current.humidity.clone(),
        weather_desc: weather_desc.clone(),
        wind_speed: current.wind_speed.clone(),
        wind_dir: current.wind_dir.clone(),
        pressure: current.pressure.clone(),
        visibility: current.visibility.clone(),
        forecast,
    };

    let json = serde_json::to_string_pretty(&output)?;
    println!("{}", json);

    Ok(())
}
