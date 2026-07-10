//! Weather provider backed by Open-Meteo (no API key required).
//!
//! The configured location string is geocoded once per process via the
//! Open-Meteo geocoding API; forecasts then go through the regular forecast
//! endpoint and are cached by the hub.

use std::collections::HashMap;

use chaos_domain::{DailyForecast, HourlyForecast, WeatherData, WidgetData};
use serde::Deserialize;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct Place {
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
}

pub async fn fetch(
    http: &reqwest::Client,
    geocode_cache: &RwLock<HashMap<String, Place>>,
    location: &str,
) -> Result<WidgetData, String> {
    let place = resolve(http, geocode_cache, location).await?;

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
    let forecast: Forecast = get_json(http, &url).await?;

    let current = forecast.current;
    // past_days extends BOTH series; only the hourly one should reach into
    // the past — the dashboard's daily rows start today.
    let today = current.time.date();
    let (daily, hourly) = build_series(forecast.daily, forecast.hourly);
    let daily = daily.into_iter().filter(|d| d.date >= today).collect();

    // The hourly series spans past_days back through the full forecast; the
    // UI needs to know where "now" sits in it, anchored to the local hour.
    let this_hour = {
        use chrono::Timelike;
        current
            .time
            .with_minute(0)
            .and_then(|t| t.with_second(0))
            .unwrap_or(current.time)
    };
    let now_index = now_index(&hourly, this_hour);

    Ok(WidgetData::Weather(WeatherData {
        location: place.name,
        temperature_c: current.temperature_2m,
        apparent_c: current.apparent_temperature,
        humidity_pct: current.relative_humidity_2m,
        wind_kmh: current.wind_speed_10m,
        weather_code: current.weather_code,
        description: describe(current.weather_code).to_string(),
        daily,
        hourly,
        now_index,
    }))
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

async fn resolve(
    http: &reqwest::Client,
    cache: &RwLock<HashMap<String, Place>>,
    location: &str,
) -> Result<Place, String> {
    if let Some(place) = cache.read().await.get(location) {
        return Ok(place.clone());
    }

    // "Osaka, JP" style disambiguation: the geocoding API only searches
    // names, so a trailing country part becomes a filter on the results.
    let (name_query, country) = match location.rsplit_once(',') {
        Some((name, country)) if !name.trim().is_empty() => {
            (name.trim(), Some(country.trim().to_ascii_uppercase()))
        }
        _ => (location.trim(), None),
    };

    let url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=10&language=en&format=json",
        urlencoded(name_query)
    );
    let response: GeocodeResponse = get_json(http, &url).await?;
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
    let place = Place {
        name,
        latitude: hit.latitude,
        longitude: hit.longitude,
    };
    cache
        .write()
        .await
        .insert(location.to_string(), place.clone());
    Ok(place)
}

async fn get_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
) -> Result<T, String> {
    let resp = http.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("open-meteo returned {}", resp.status()));
    }
    resp.json::<T>().await.map_err(|e| e.to_string())
}

fn urlencoded(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
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
    use super::now_index;
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
        let (daily, hourly) = super::build_series(forecast.daily, forecast.hourly);
        // Null-bearing entries are dropped, complete ones kept.
        assert_eq!(daily.len(), 1);
        assert_eq!(daily[0].max_c, 30.0);
        assert_eq!(hourly.len(), 2);
        assert_eq!(hourly[1].temp_c, 21.0);
    }
}
