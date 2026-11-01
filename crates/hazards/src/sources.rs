//! 三个免注册、免 key 的数据源（协议无关：本模块只认识各自的 JSON，不认识协议消息）。
//!
//! * USGS 地震：`earthquake.usgs.gov`（公共领域）
//! * NWS 天气预警：`api.weather.gov`（美国联邦公共领域，要求可识别 UA）
//! * NOAA SWPC 空间天气：`services.swpc.noaa.gov`（公共领域）
//!
//! 解析刻意"宽进"：上游字段缺失时跳过单条、不整轮失败——灾害源的字段长期会漂。

use serde_json::Value;
use base_http::{HttpClient, HttpError};

use crate::config::Config;

const USGS_DAY_FEED: &str =
    "https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/all_day.geojson";
const NWS_ACTIVE_ALERTS: &str = "https://api.weather.gov/alerts/active";
const SWPC_KP_INDEX: &str =
    "https://services.swpc.noaa.gov/json/planetary_k_index_1m.json";

/// 归一化后的地震事件。
#[derive(Debug, Clone, PartialEq)]
pub struct Earthquake {
    pub id: String,
    pub magnitude: f64,
    pub place: String,
    pub lat: f64,
    pub lon: f64,
    pub depth_km: f64,
    pub time_ms: Option<i64>,
    pub url: Option<String>,
    pub tsunami: i64,
    pub alert: Option<String>,
}

/// 归一化后的天气预警。
#[derive(Debug, Clone, PartialEq)]
pub struct WeatherAlert {
    pub id: String,
    pub event: String,
    pub severity: Option<String>,
    pub urgency: Option<String>,
    pub certainty: Option<String>,
    pub headline: Option<String>,
    pub area_desc: Option<String>,
    pub expires: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

/// 抓取 USGS 过去 24h 地震，按 `min_magnitude` 过滤。
pub fn fetch_earthquakes(
    client: &HttpClient,
    cfg: &Config,
) -> Result<Vec<Earthquake>, HttpError> {
    let doc = client.get_json(USGS_DAY_FEED)?;
    let features = doc
        .get("features")
        .and_then(Value::as_array)
        .ok_or_else(|| HttpError::parse("USGS 响应缺少 features 数组"))?;

    let mut out = Vec::new();
    for feat in features {
        let props = &feat["properties"];
        let Some(mag) = as_f64(&props["mag"]) else {
            continue;
        };
        if mag < cfg.min_magnitude {
            continue;
        }
        let coords = &feat["geometry"]["coordinates"];
        let (Some(lon), Some(lat)) = (as_f64(&coords[0]), as_f64(&coords[1])) else {
            continue;
        };
        let time_ms = props["time"].as_i64();
        let id = as_str(&feat["id"])
            .or_else(|| as_str(&props["code"]))
            .unwrap_or_else(|| format!("{lat:.4},{lon:.4},{}", time_ms.unwrap_or(0)));

        out.push(Earthquake {
            id,
            magnitude: mag,
            place: as_str(&props["place"]).unwrap_or_default(),
            lat,
            lon,
            depth_km: as_f64(&coords[2]).unwrap_or(0.0),
            time_ms,
            url: as_str(&props["url"]),
            tsunami: props["tsunami"].as_i64().unwrap_or(0),
            alert: as_str(&props["alert"]),
        });
        if out.len() >= cfg.max_records {
            break;
        }
    }
    // 震级从大到小，保证 maxRecords 截断时留下的是重要的那些。
    out.sort_by(|a, b| b.magnitude.total_cmp(&a.magnitude));
    Ok(out)
}

/// 抓取 NWS 当前生效的天气预警。
pub fn fetch_weather_alerts(
    client: &HttpClient,
    cfg: &Config,
) -> Result<Vec<WeatherAlert>, HttpError> {
    let doc = client.get_json_with(
        NWS_ACTIVE_ALERTS,
        &[("Accept", "application/geo+json")],
    )?;
    let features = doc
        .get("features")
        .and_then(Value::as_array)
        .ok_or_else(|| HttpError::parse("NWS 响应缺少 features 数组"))?;

    let mut out = Vec::new();
    for feat in features {
        let props = &feat["properties"];
        let Some(id) = as_str(&props["id"]).or_else(|| as_str(&feat["id"])) else {
            continue;
        };
        let (lat, lon) = centroid(&feat["geometry"]);
        out.push(WeatherAlert {
            id,
            event: as_str(&props["event"]).unwrap_or_else(|| "Unknown".into()),
            severity: as_str(&props["severity"]),
            urgency: as_str(&props["urgency"]),
            certainty: as_str(&props["certainty"]),
            headline: as_str(&props["headline"]),
            area_desc: as_str(&props["areaDesc"]),
            expires: as_str(&props["expires"]),
            lat,
            lon,
        });
        if out.len() >= cfg.max_records {
            break;
        }
    }
    Ok(out)
}

/// 抓取 NOAA SWPC 最新 Kp 指数。数组为空时返回 `Ok(None)`（不算失败）。
pub fn fetch_kp_index(client: &HttpClient) -> Result<Option<f64>, HttpError> {
    let doc = client.get_json(SWPC_KP_INDEX)?;
    let rows = doc
        .as_array()
        .ok_or_else(|| HttpError::parse("SWPC Kp 响应不是数组"))?;
    let latest = rows
        .iter()
        .rev()
        .find_map(|row| as_f64(&row["kp_index"]).or_else(|| as_f64(&row["estimated_kp"])));
    Ok(latest)
}

// ── 解析小工具（宽进：拿不到就当缺省，不让单条坏数据废掉整轮）──────────────

fn as_f64(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_i64().map(|i| i as f64))
}

fn as_str(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 从任意 GeoJSON 几何取一个代表点（点直接用；面取第一环平均）。
fn centroid(geom: &Value) -> (Option<f64>, Option<f64>) {
    match geom["type"].as_str() {
        Some("Point") => {
            let c = &geom["coordinates"];
            (as_f64(&c[1]), as_f64(&c[0]))
        }
        Some("Polygon") => ring_average(&geom["coordinates"][0]),
        Some("MultiPolygon") => ring_average(&geom["coordinates"][0][0]),
        _ => (None, None),
    }
}

fn ring_average(ring: &Value) -> (Option<f64>, Option<f64>) {
    let Some(points) = ring.as_array() else {
        return (None, None);
    };
    let (mut lat_sum, mut lon_sum, mut n) = (0.0f64, 0.0f64, 0u32);
    for p in points {
        if let (Some(lon), Some(lat)) = (as_f64(&p[0]), as_f64(&p[1])) {
            lon_sum += lon;
            lat_sum += lat;
            n += 1;
        }
    }
    if n == 0 {
        (None, None)
    } else {
        (Some(lat_sum / n as f64), Some(lon_sum / n as f64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn point_centroid() {
        let g = json!({"type": "Point", "coordinates": [-122.3, 47.6]});
        assert_eq!(centroid(&g), (Some(47.6), Some(-122.3)));
    }

    #[test]
    fn polygon_centroid_is_ring_average() {
        let g = json!({"type": "Polygon", "coordinates": [[[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0], [0.0, 0.0]]]});
        assert_eq!(centroid(&g), (Some(0.8), Some(0.8)));
    }

    #[test]
    fn missing_geometry_yields_no_point() {
        assert_eq!(centroid(&json!({"type": "Polygon", "coordinates": []})), (None, None));
    }
}
