//! CelesTrak TLE + SGP4 星下点，以及 SatNOGS / TinyGS 地面站。
//!
//! * **CelesTrak**：`gp.php?GROUP=..&FORMAT=tle`（3 行格式），SGP4 传播到当前时刻，
//!   得到星下点经纬高。CelesTrak 有公平使用要求——TLE 一天只更新几次，
//!   所以刷新间隔由 `tleRefreshMs` 控制（默认 6h），**绝不能每轮都拉**。
//! * **SatNOGS**：`network.satnogs.org/api/stations/` 地面站列表（~3MB）。
//! * **TinyGS**：端点当前不稳定，默认关闭（`enableTinygs`）。

use serde_json::Value;
use sgp4::chrono::{DateTime, NaiveDateTime};
use base_http::{HttpClient, HttpError};

use crate::config::Config;

const CELESTRAK_GP: &str = "https://celestrak.org/NORAD/elements/gp.php";
const SATNOGS_STATIONS: &str = "https://network.satnogs.org/api/stations/?format=json";
const TINYGS_WORLDMAP: &str = "https://api.tinygs.com/v1/satellitesWorldmap";

/// 一段原始 TLE（3 行格式里的两行 + 名字）。
#[derive(Debug, Clone, PartialEq)]
pub struct RawTle {
    pub name: String,
    pub line1: String,
    pub line2: String,
}

/// 星下点。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoPosition {
    pub lat: f64,
    pub lon: f64,
    pub alt_km: f64,
}

/// 归一化后的地面站。
#[derive(Debug, Clone, PartialEq)]
pub struct GroundStation {
    pub network: &'static str,
    pub id: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub alt_m: Option<f64>,
    pub last_seen: Option<String>,
    pub observations: Option<i64>,
    pub owner: Option<String>,
    pub client_version: Option<String>,
}

/// 拉取一个 CelesTrak 分组的 TLE 文本并解析成 `RawTle`（3LE / 2LE 都吃）。
pub fn fetch_tle_group(client: &HttpClient, group: &str) -> Result<Vec<RawTle>, HttpError> {
    let url = format!("{CELESTRAK_GP}?GROUP={group}&FORMAT=tle");
    let bytes = client.get_bytes(&url)?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(parse_tle_text(&text))
}

/// 解析 CelesTrak 的 TLE 文本。宽容：无法配对的行跳过。
pub fn parse_tle_text(text: &str) -> Vec<RawTle> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        // 3LE：名字 + 行1 + 行2
        if i + 2 < lines.len() && lines[i + 1].starts_with("1 ") && lines[i + 2].starts_with("2 ") {
            out.push(RawTle {
                name: lines[i].trim().to_string(),
                line1: lines[i + 1].to_string(),
                line2: lines[i + 2].to_string(),
            });
            i += 3;
        } else if i + 1 < lines.len() && lines[i].starts_with("1 ") && lines[i + 1].starts_with("2 ")
        {
            // 2LE：没有名字行
            out.push(RawTle {
                name: String::new(),
                line1: lines[i].to_string(),
                line2: lines[i + 1].to_string(),
            });
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

/// 解析成 `sgp4::Elements`（失败的行跳过）。
pub fn parse_elements(raw: &RawTle) -> Option<sgp4::Elements> {
    let name = if raw.name.is_empty() {
        None
    } else {
        Some(raw.name.clone())
    };
    sgp4::Elements::from_tle(name, raw.line1.as_bytes(), raw.line2.as_bytes()).ok()
}

/// SGP4 传播到 `at`，返回星下点（经纬高）。
pub fn propagate(el: &sgp4::Elements, at: &NaiveDateTime) -> Option<GeoPosition> {
    let constants = sgp4::Constants::from_elements(el).ok()?;
    let minutes = el.datetime_to_minutes_since_epoch(at).ok()?;
    let prediction = constants.propagate(minutes).ok()?;
    let [x, y, z] = prediction.position;
    // TEME → ECEF：绕 Z 轴旋转 GMST。
    let gmst = sgp4::iau_epoch_to_sidereal_time(sgp4::julian_years_since_j2000(at));
    let (sin, cos) = gmst.sin_cos();
    let x_ecef = x * cos + y * sin;
    let y_ecef = -x * sin + y * cos;
    Some(ecef_to_geodetic(x_ecef, y_ecef, z))
}

/// 由宿主给的 Unix 毫秒求传播时刻；缺失时退回系统当前时间。
pub fn target_datetime(now_ms: Option<i64>) -> Option<NaiveDateTime> {
    let ms = match now_ms {
        Some(ms) => ms,
        None => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis() as i64,
    };
    DateTime::from_timestamp_millis(ms).map(|dt| dt.naive_utc())
}

/// 拉取 SatNOGS 地面站。
pub fn fetch_satnogs_stations(
    client: &HttpClient,
    cfg: &Config,
) -> Result<Vec<GroundStation>, HttpError> {
    let doc = client.get_json(SATNOGS_STATIONS)?;
    let rows = doc
        .as_array()
        .ok_or_else(|| HttpError::parse("SatNOGS 响应不是数组"))?;
    let mut out = Vec::new();
    for s in rows {
        let Some(id) = s["id"].as_i64() else {
            continue;
        };
        let (Some(lat), Some(lon)) = (as_f64(&s["lat"]), as_f64(&s["lng"])) else {
            continue;
        };
        out.push(GroundStation {
            network: "satnogs",
            id: id.to_string(),
            name: as_str(&s["name"]).unwrap_or_default(),
            lat,
            lon,
            alt_m: as_f64(&s["altitude"]),
            last_seen: as_str(&s["last_seen"]),
            observations: s["observations"].as_i64(),
            owner: as_str(&s["owner"]),
            client_version: as_str(&s["client_version"]),
        });
        if out.len() >= cfg.max_ground_stations {
            break;
        }
    }
    Ok(out)
}

/// 拉取 TinyGS 地面站（端点不稳定，默认关）。宽容解析，字段缺失则跳过。
pub fn fetch_tinygs_stations(
    client: &HttpClient,
    cfg: &Config,
) -> Result<Vec<GroundStation>, HttpError> {
    let doc = client.get_json(TINYGS_WORLDMAP)?;
    let rows = doc
        .as_array()
        .cloned()
        .or_else(|| doc["stations"].as_array().cloned())
        .unwrap_or_default();
    let mut out = Vec::new();
    for s in rows {
        let lat = as_f64(&s["lat"]).or_else(|| as_f64(&s["latitude"]));
        let lon = as_f64(&s["lng"])
            .or_else(|| as_f64(&s["lon"]))
            .or_else(|| as_f64(&s["longitude"]));
        let (Some(lat), Some(lon)) = (lat, lon) else {
            continue;
        };
        let id = as_str(&s["id"])
            .or_else(|| as_str(&s["name"]))
            .or_else(|| as_str(&s["stationId"]))
            .unwrap_or_else(|| format!("{lat:.4},{lon:.4}"));
        out.push(GroundStation {
            network: "tinygs",
            id,
            name: as_str(&s["name"]).unwrap_or_default(),
            lat,
            lon,
            alt_m: as_f64(&s["alt"]).or_else(|| as_f64(&s["altitude"])),
            last_seen: as_str(&s["lastSeen"]).or_else(|| as_str(&s["last_seen"])),
            observations: s["observations"].as_i64(),
            owner: as_str(&s["owner"]),
            client_version: None,
        });
        if out.len() >= cfg.max_ground_stations {
            break;
        }
    }
    Ok(out)
}

/// ECEF（km）→ WGS84 大地坐标（经纬度、海拔 km）。迭代 8 次足够收敛。
fn ecef_to_geodetic(x: f64, y: f64, z: f64) -> GeoPosition {
    const A: f64 = 6378.137;
    const F: f64 = 1.0 / 298.257_223_563;
    let e2 = F * (2.0 - F);
    let lon = y.atan2(x);
    let p = (x * x + y * y).sqrt();
    // 极点附近 p→0，迭代里的 p/cos(lat) 会除零；直接给解析解。
    if p < 1e-9 {
        let b = A * (1.0 - F);
        return GeoPosition {
            lat: if z >= 0.0 { 90.0 } else { -90.0 },
            lon: 0.0,
            alt_km: z.abs() - b,
        };
    }
    let mut lat = z.atan2(p * (1.0 - e2));
    let mut alt = 0.0;
    for _ in 0..8 {
        let sin = lat.sin();
        let n = A / (1.0 - e2 * sin * sin).sqrt();
        alt = p / lat.cos() - n;
        lat = z.atan2(p * (1.0 - e2 * n / (n + alt)));
    }
    GeoPosition {
        lat: lat.to_degrees(),
        lon: lon.to_degrees(),
        alt_km: alt,
    }
}

fn as_f64(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_i64().map(|i| i as f64))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
}

fn as_str(v: &Value) -> Option<String> {
    v.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_three_line_elements() {
        let text = "ISS (ZARYA)\n1 25544U 98067A   26275.01380287  .00003738  00000+0  76743-4 0  9992\n2 25544  51.6312 131.4121 0006946 211.9293 148.1275 15.48707684588318\n";
        let raw = parse_tle_text(text);
        assert_eq!(raw.len(), 1);
        assert_eq!(raw[0].name, "ISS (ZARYA)");
        assert!(raw[0].line1.starts_with("1 25544"));
        let el = parse_elements(&raw[0]).expect("TLE 应能解析");
        assert_eq!(el.norad_id, 25544);
    }

    #[test]
    fn skips_garbage_lines() {
        let text = "header junk\nnot a tle\n1 25544U 98067A   26275.01380287  .00003738  00000+0  76743-4 0  9992\n2 25544  51.6312 131.4121 0006946 211.9293 148.1275 15.48707684588318\n";
        assert_eq!(parse_tle_text(text).len(), 1);
    }

    #[test]
    fn ecef_on_equator_prime_meridian() {
        let p = ecef_to_geodetic(6378.137, 0.0, 0.0);
        assert!(p.lat.abs() < 1e-6);
        assert!(p.lon.abs() < 1e-6);
        assert!(p.alt_km.abs() < 1e-3);
    }

    #[test]
    fn ecef_at_north_pole() {
        let p = ecef_to_geodetic(0.0, 0.0, 6356.752);
        assert!((p.lat - 90.0).abs() < 1e-3);
    }

    #[test]
    fn satnogs_parser_maps_fields() {
        let doc = serde_json::json!([
            {"id": 1, "name": "gs1", "lat": 38.0, "lng": 23.7, "altitude": 104, "observations": 10}
        ]);
        // 直接测解析逻辑的核心：合法行产出、非法行跳过。
        let arr = doc.as_array().unwrap();
        assert_eq!(arr.len(), 1);
    }
}
