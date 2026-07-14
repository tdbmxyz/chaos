//! Direct Open-Meteo access (no API key): geocoding + forecast, shared by
//! every client build. The server no longer proxies weather — this is THE
//! weather path, at home and away.

use std::time::Duration;

use chaos_domain::{DailyForecast, HourlyForecast, WeatherData};
use serde::{Deserialize, Serialize};

/// Per-request deadline: an unreachable Open-Meteo must fail the widget
/// fast, not hang "Loading" for minutes.
const TIMEOUT: Duration = Duration::from_secs(8);

/// A geocoded place — serializable because chaos-ui caches it per name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Place {
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
}

/// A fetched forecast plus what's needed to keep a cached copy honest:
/// `utc_offset_seconds` lets a reader recompute `now_index` for the
/// location-local current hour long after the fetch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaceForecast {
    pub data: WeatherData,
    pub utc_offset_seconds: i32,
}

/// Resolve a location string ("Osaka" or "Osaka, JP") to coordinates via
/// the Open-Meteo geocoding API.
pub async fn geocode(http: &reqwest::Client, location: &str) -> Result<Place, String> {
    // The geocoding API only searches names, so a trailing country part
    // becomes a filter on the results (handled by pick_place).
    let (name_query, _) = split_location(location);
    let url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=10&language=en&format=json",
        urlencoded(name_query)
    );
    let response: GeocodeResponse = get_json(http, &url)
        .await
        .map_err(|e| format!("geocoding: {e}"))?;
    pick_place(location, response)
}

/// Fetch the forecast for a resolved place.
pub async fn forecast(http: &reqwest::Client, place: &Place) -> Result<PlaceForecast, String> {
    let url = format!(
        "https://api.open-meteo.com/v1/forecast\
         ?latitude={lat}&longitude={lon}\
         &current=temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m\
         &daily=weather_code,temperature_2m_max,temperature_2m_min\
         &hourly=temperature_2m,weather_code\
         &timezone=auto&forecast_days=16&past_days=16",
        lat = place.latitude,
        lon = place.longitude,
    );
    let forecast: Forecast = get_json(http, &url)
        .await
        .map_err(|e| format!("open-meteo: {e}"))?;

    let current = forecast.current;
    // past_days extends BOTH series; only the hourly one should reach into
    // the past — the dashboard's daily rows start today.
    let today = current.time.date();
    let (daily, hourly) = build_series(forecast.daily, forecast.hourly);
    let daily = daily.into_iter().filter(|d| d.date >= today).collect();

    // The hourly series spans past_days back through the full forecast; the
    // UI needs to know where "now" sits in it, anchored to the local hour.
    let this_hour = truncate_to_hour(current.time);
    let now_index = now_index(&hourly, this_hour);

    Ok(PlaceForecast {
        data: WeatherData {
            location: place.name.clone(),
            temperature_c: current.temperature_2m,
            apparent_c: current.apparent_temperature,
            humidity_pct: current.relative_humidity_2m,
            wind_kmh: current.wind_speed_10m,
            weather_code: current.weather_code,
            description: describe(current.weather_code).to_string(),
            daily,
            hourly,
            now_index,
        },
        utc_offset_seconds: forecast.utc_offset_seconds,
    })
}

/// Where "now" sits in a (possibly cached) hourly series: convert UTC now
/// to the location's local clock, truncate to the hour, find the first
/// entry at or after it.
pub fn recompute_now_index(
    hourly: &[HourlyForecast],
    now_utc: chrono::DateTime<chrono::Utc>,
    utc_offset_seconds: i32,
) -> usize {
    let local = now_utc.naive_utc() + chrono::Duration::seconds(utc_offset_seconds as i64);
    now_index(hourly, truncate_to_hour(local))
}

fn truncate_to_hour(t: chrono::NaiveDateTime) -> chrono::NaiveDateTime {
    use chrono::Timelike;
    t.with_minute(0).and_then(|t| t.with_second(0)).unwrap_or(t)
}

/// Zip the raw Open-Meteo series into forecast entries, dropping any hour or
/// day with a null value (the model horizon's ragged edge — better a shorter
/// series than a failed fetch).
fn build_series(
    daily: DailySeries,
    hourly: HourlySeries,
) -> (Vec<DailyForecast>, Vec<HourlyForecast>) {
    let daily = daily
        .time
        .into_iter()
        .zip(daily.temperature_2m_min)
        .zip(daily.temperature_2m_max)
        .zip(daily.weather_code)
        .filter_map(|(((date, min_c), max_c), weather_code)| {
            Some(DailyForecast {
                date,
                min_c: min_c?,
                max_c: max_c?,
                weather_code: weather_code?,
            })
        })
        .collect();
    let hourly = hourly
        .time
        .into_iter()
        .zip(hourly.temperature_2m)
        .zip(hourly.weather_code)
        .filter_map(|((time, temp_c), weather_code)| {
            Some(HourlyForecast {
                time,
                temp_c: temp_c?,
                weather_code: weather_code?,
            })
        })
        .collect();
    (daily, hourly)
}

/// Index of the first hourly entry at or after `this_hour` — where "now"
/// sits in a series that reaches into the past. `hourly.len()` when every
/// entry is in the past (can't happen with a live forecast, but total).
fn now_index(hourly: &[HourlyForecast], this_hour: chrono::NaiveDateTime) -> usize {
    hourly
        .iter()
        .position(|h| h.time >= this_hour)
        .unwrap_or(hourly.len())
}

/// "Osaka, JP" style disambiguation: split a location string into the name
/// to search and an optional uppercased country filter.
fn split_location(location: &str) -> (&str, Option<String>) {
    match location.rsplit_once(',') {
        Some((name, country)) if !name.trim().is_empty() => {
            (name.trim(), Some(country.trim().to_ascii_uppercase()))
        }
        _ => (location.trim(), None),
    }
}

/// Pick the geocoding hit matching the location's country filter (if any) —
/// pure so the selection is testable without HTTP.
fn pick_place(location: &str, response: GeocodeResponse) -> Result<Place, String> {
    let (_, country) = split_location(location);
    let hit = response
        .results
        .into_iter()
        .find(|hit| match &country {
            // Match the ISO code ("JP") or a country name prefix ("Japan").
            Some(want) => {
                hit.country_code.as_deref() == Some(want)
                    || hit
                        .country
                        .as_deref()
                        .is_some_and(|c| c.to_ascii_uppercase().starts_with(want.as_str()))
            }
            None => true,
        })
        .ok_or_else(|| format!("location {location:?} not found by geocoding"))?;

    let name = match &hit.country_code {
        Some(cc) => format!("{}, {}", hit.name, cc),
        None => hit.name.clone(),
    };
    Ok(Place {
        name,
        latitude: hit.latitude,
        longitude: hit.longitude,
    })
}

fn urlencoded(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// GET a JSON document with the module's per-request deadline. Mirrors the
/// timeout pattern in `ChaosClient::check_status` (reqwest's builder-level
/// `.timeout()` isn't available on wasm; the request-level one is).
async fn get_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
) -> Result<T, String> {
    let mut request = http.get(url).build().map_err(|e| e.to_string())?;
    *request.timeout_mut() = Some(TIMEOUT);
    let response = http.execute(request).await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    response.json().await.map_err(|e| e.to_string())
}

/// WMO weather interpretation codes (Open-Meteo docs).
fn describe(code: i32) -> &'static str {
    match code {
        0 => "Clear sky",
        1 => "Mainly clear",
        2 => "Partly cloudy",
        3 => "Overcast",
        45 | 48 => "Fog",
        51 | 53 | 55 => "Drizzle",
        56 | 57 => "Freezing drizzle",
        61 | 63 | 65 => "Rain",
        66 | 67 => "Freezing rain",
        71 | 73 | 75 => "Snow",
        77 => "Snow grains",
        80..=82 => "Rain showers",
        85 | 86 => "Snow showers",
        95 => "Thunderstorm",
        96 | 99 => "Thunderstorm with hail",
        _ => "Unknown",
    }
}

#[derive(Debug, Deserialize)]
struct GeocodeResponse {
    #[serde(default)]
    results: Vec<GeocodeHit>,
}

#[derive(Debug, Deserialize)]
struct GeocodeHit {
    name: String,
    latitude: f64,
    longitude: f64,
    country: Option<String>,
    country_code: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Forecast {
    /// The location's offset from UTC (`timezone=auto`); defaulted for
    /// safety, though Open-Meteo always sends it.
    #[serde(default)]
    utc_offset_seconds: i32,
    current: CurrentWeather,
    daily: DailySeries,
    hourly: HourlySeries,
}

#[derive(Debug, Deserialize)]
struct CurrentWeather {
    /// Local time at the location (`timezone=auto`), anchoring the hourly
    /// series to "from now on".
    #[serde(deserialize_with = "local_time")]
    time: chrono::NaiveDateTime,
    temperature_2m: f64,
    apparent_temperature: f64,
    relative_humidity_2m: Option<f64>,
    weather_code: i32,
    wind_speed_10m: f64,
}

#[derive(Debug, Deserialize)]
struct HourlySeries {
    #[serde(deserialize_with = "local_times")]
    time: Vec<chrono::NaiveDateTime>,
    temperature_2m: Vec<Option<f64>>,
    weather_code: Vec<Option<i32>>,
}

/// Open-Meteo local times omit the seconds (`2026-07-07T02:45`), which the
/// default chrono deserializer rejects.
fn parse_local_time(raw: &str) -> Result<chrono::NaiveDateTime, chrono::ParseError> {
    chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S"))
}

fn local_time<'de, D: serde::Deserializer<'de>>(d: D) -> Result<chrono::NaiveDateTime, D::Error> {
    let raw = String::deserialize(d)?;
    parse_local_time(&raw).map_err(serde::de::Error::custom)
}

fn local_times<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<chrono::NaiveDateTime>, D::Error> {
    let raw = Vec::<String>::deserialize(d)?;
    raw.iter()
        .map(|s| parse_local_time(s).map_err(serde::de::Error::custom))
        .collect()
}

#[derive(Debug, Deserialize)]
struct DailySeries {
    time: Vec<chrono::NaiveDate>,
    weather_code: Vec<Option<i32>>,
    temperature_2m_max: Vec<Option<f64>>,
    temperature_2m_min: Vec<Option<f64>>,
}

#[cfg(test)]
mod tests {
    use super::{build_series, now_index, pick_place, recompute_now_index};
    use chaos_domain::HourlyForecast;
    use chrono::NaiveDate;

    fn hour(d: u32, h: u32) -> HourlyForecast {
        HourlyForecast {
            time: NaiveDate::from_ymd_opt(2026, 7, d)
                .unwrap()
                .and_hms_opt(h, 0, 0)
                .unwrap(),
            temp_c: 20.0,
            weather_code: 0,
        }
    }

    #[test]
    fn now_index_finds_first_entry_at_or_after_now() {
        let hourly = vec![hour(8, 22), hour(8, 23), hour(9, 0), hour(9, 1)];
        let now = NaiveDate::from_ymd_opt(2026, 7, 9)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        assert_eq!(now_index(&hourly, now), 2);
    }

    #[test]
    fn now_index_matches_exact_hour() {
        let hourly = vec![hour(9, 13), hour(9, 14), hour(9, 15)];
        let now = NaiveDate::from_ymd_opt(2026, 7, 9)
            .unwrap()
            .and_hms_opt(14, 0, 0)
            .unwrap();
        assert_eq!(now_index(&hourly, now), 1);
    }

    #[test]
    fn now_index_is_len_when_everything_is_past() {
        let hourly = vec![hour(1, 10), hour(1, 11)];
        let now = NaiveDate::from_ymd_opt(2026, 7, 9)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        assert_eq!(now_index(&hourly, now), 2);
    }

    #[test]
    fn forecast_tolerates_null_tail_entries() {
        // Open-Meteo leaves nulls where its model horizon ends (observed on
        // the 16th forecast day for some locations/timezones).
        let raw = r#"{
            "utc_offset_seconds": 7200,
            "current": {"time": "2026-07-11T00:00", "temperature_2m": 28.0,
                        "apparent_temperature": 28.1, "relative_humidity_2m": 44,
                        "weather_code": 0, "wind_speed_10m": 9.5},
            "daily": {"time": ["2026-07-11", "2026-07-12"],
                      "weather_code": [3, null],
                      "temperature_2m_max": [30.0, null],
                      "temperature_2m_min": [18.0, null]},
            "hourly": {"time": ["2026-07-11T00:00", "2026-07-11T01:00", "2026-07-11T02:00"],
                       "temperature_2m": [20.0, null, 21.0],
                       "weather_code": [0, null, 1]}
        }"#;
        let forecast: super::Forecast = serde_json::from_str(raw).expect("nulls must parse");
        let (daily, hourly) = build_series(forecast.daily, forecast.hourly);
        // Null-bearing entries are dropped, complete ones kept.
        assert_eq!(daily.len(), 1);
        assert_eq!(daily[0].max_c, 30.0);
        assert_eq!(hourly.len(), 2);
        assert_eq!(hourly[1].temp_c, 21.0);
    }

    #[test]
    fn forecast_carries_the_utc_offset() {
        let raw = r#"{
            "utc_offset_seconds": 7200,
            "current": {"time": "2026-07-11T00:00", "temperature_2m": 28.0,
                        "apparent_temperature": 28.1, "relative_humidity_2m": 44,
                        "weather_code": 0, "wind_speed_10m": 9.5},
            "daily": {"time": [], "weather_code": [],
                      "temperature_2m_max": [], "temperature_2m_min": []},
            "hourly": {"time": [], "temperature_2m": [], "weather_code": []}
        }"#;
        let forecast: super::Forecast = serde_json::from_str(raw).expect("offset must parse");
        assert_eq!(forecast.utc_offset_seconds, 7200);
    }

    #[test]
    fn recomputed_now_index_tracks_the_location_local_clock() {
        let hourly = vec![hour(9, 13), hour(9, 14), hour(9, 15)];
        // Location is UTC+2; at 12:30 UTC the local hour is 14:00.
        let utc = chrono::NaiveDate::from_ymd_opt(2026, 7, 9)
            .unwrap()
            .and_hms_opt(12, 30, 0)
            .unwrap()
            .and_utc();
        assert_eq!(recompute_now_index(&hourly, utc, 2 * 3600), 1);
    }

    #[test]
    fn geocode_response_prefers_the_country_filtered_hit() {
        let raw = r#"{"results":[
            {"name":"Paris","latitude":33.66,"longitude":-95.55,"country":"United States","country_code":"US"},
            {"name":"Paris","latitude":48.85,"longitude":2.35,"country":"France","country_code":"FR"}
        ]}"#;
        let place = pick_place("Paris, FR", serde_json::from_str(raw).unwrap()).unwrap();
        assert_eq!(place.name, "Paris, FR");
        assert!((place.latitude - 48.85).abs() < 0.01);
    }
}
