//! Weather from Open-Meteo (<https://open-meteo.com>, no account or key).
//!
//! Only used when the user enabled the weather widget and chose a place;
//! requests carry the coordinates and nothing else (generic user agent).
//! These are blocking calls — run them off the UI thread.

use std::time::Duration;

use serde::Deserialize;

const UA: &str = "m3-desktop (weather widget)";

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).user_agent(UA).build().new_agent()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Day {
    /// `YYYY-MM-DD`.
    pub date: String,
    pub code: u32,
    pub max: f64,
    pub min: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Forecast {
    pub temperature: f64,
    pub code: u32,
    pub is_day: bool,
    /// Today first.
    pub days: Vec<Day>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Place {
    pub name: String,
    #[serde(default)]
    pub admin1: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
    pub latitude: f64,
    pub longitude: f64,
}

impl Place {
    /// "Warsaw, Masovia, Poland".
    pub fn label(&self) -> String {
        [Some(self.name.as_str()), self.admin1.as_deref(), self.country.as_deref()]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[derive(Deserialize)]
struct RawCurrent {
    temperature_2m: f64,
    weather_code: u32,
    #[serde(default)]
    is_day: u8,
}

#[derive(Deserialize)]
struct RawDaily {
    time: Vec<String>,
    weather_code: Vec<u32>,
    temperature_2m_max: Vec<f64>,
    temperature_2m_min: Vec<f64>,
}

#[derive(Deserialize)]
struct RawForecast {
    current: RawCurrent,
    daily: RawDaily,
}

/// Parse an Open-Meteo forecast response (pure; tested).
pub fn parse_forecast(json: &str) -> Result<Forecast, String> {
    let r: RawForecast = serde_json::from_str(json).map_err(|e| format!("unexpected forecast data: {e}"))?;
    let d = &r.daily;
    let n = d.time.len().min(d.weather_code.len()).min(d.temperature_2m_max.len()).min(d.temperature_2m_min.len());
    let days = (0..n)
        .map(|i| Day {
            date: d.time[i].clone(),
            code: d.weather_code[i],
            max: d.temperature_2m_max[i],
            min: d.temperature_2m_min[i],
        })
        .collect();
    Ok(Forecast {
        temperature: r.current.temperature_2m,
        code: r.current.weather_code,
        is_day: r.current.is_day != 0,
        days,
    })
}

/// Current weather and a 4-day forecast.
pub fn forecast(latitude: f64, longitude: f64, fahrenheit: bool) -> Result<Forecast, String> {
    let unit = if fahrenheit { "fahrenheit" } else { "celsius" };
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={latitude:.4}&longitude={longitude:.4}\
         &current=temperature_2m,weather_code,is_day&daily=weather_code,temperature_2m_max,temperature_2m_min\
         &timezone=auto&forecast_days=4&temperature_unit={unit}"
    );
    let body = agent()
        .get(&url)
        .call()
        .map_err(|e| format!("weather unavailable: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("weather unavailable: {e}"))?;
    parse_forecast(&body)
}

/// Places matching `name` (for choosing the weather location).
pub fn search_places(name: &str) -> Result<Vec<Place>, String> {
    #[derive(Deserialize)]
    struct R {
        #[serde(default)]
        results: Vec<Place>,
    }
    let q: String = name.trim().chars().filter(|c| !c.is_control()).take(80).collect();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    let r: R = agent()
        .get("https://geocoding-api.open-meteo.com/v1/search")
        .query("name", &q)
        .query("count", "6")
        .query("format", "json")
        .call()
        .map_err(|e| format!("place search unavailable: {e}"))?
        .body_mut()
        .read_json()
        .map_err(|e| format!("place search unavailable: {e}"))?;
    Ok(r.results)
}

/// WMO weather code → (description, Material Symbols icon).
pub fn describe(code: u32, is_day: bool) -> (&'static str, &'static str) {
    match code {
        0 => ("Clear", if is_day { "clear_day" } else { "clear_night" }),
        1 => ("Mostly clear", if is_day { "partly_cloudy_day" } else { "partly_cloudy_night" }),
        2 => ("Partly cloudy", if is_day { "partly_cloudy_day" } else { "partly_cloudy_night" }),
        3 => ("Overcast", "cloud"),
        45 | 48 => ("Fog", "foggy"),
        51 | 53 | 55 | 56 | 57 => ("Drizzle", "rainy_light"),
        61 | 63 | 80 | 81 => ("Rain", "rainy"),
        65 | 82 => ("Heavy rain", "rainy_heavy"),
        66 | 67 => ("Freezing rain", "weather_mix"),
        71 | 73 | 77 | 85 => ("Snow", "weather_snowy"),
        75 | 86 => ("Heavy snow", "snowing_heavy"),
        95 => ("Thunderstorm", "thunderstorm"),
        96 | 99 => ("Hail storm", "thunderstorm"),
        _ => ("Weather", "partly_cloudy_day"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_open_meteo() {
        let json = r#"{"current":{"temperature_2m":21.4,"weather_code":2,"is_day":1},
            "daily":{"time":["2026-11-11","2026-11-12"],"weather_code":[2,61],
            "temperature_2m_max":[22.0,18.5],"temperature_2m_min":[12.0,10.1]}}"#;
        let f = parse_forecast(json).expect("forecast");
        assert_eq!((f.temperature, f.code, f.is_day, f.days.len()), (21.4, 2, true, 2));
        assert_eq!(f.days[1], Day { date: "2026-11-12".into(), code: 61, max: 18.5, min: 10.1 });
        assert!(parse_forecast("{}").is_err());
    }

    #[test]
    fn codes_and_labels() {
        assert_eq!(describe(0, false).1, "clear_night");
        assert_eq!(describe(95, true).0, "Thunderstorm");
        let p = Place {
            name: "Warsaw".into(),
            admin1: Some("Masovia".into()),
            country: Some("Poland".into()),
            latitude: 52.2,
            longitude: 21.0,
        };
        assert_eq!(p.label(), "Warsaw, Masovia, Poland");
    }
}
