use std::time::Duration;

use crate::{chart_core::OhlcBar, market_data::BarInterval};

pub(crate) fn http_get(url: &str) -> Result<String, String> {
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent("rustchart/0.1")
        .build()
        .map_err(|error| format!("building HTTP client: {error}"))?
        .get(url)
        .send()
        .map_err(|error| format!("request failed: {error}"))?;

    let status = response.status();
    let body = response
        .text()
        .map_err(|error| format!("reading response body: {error}"))?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(format!("HTTP {status}: {}", body.trim()))
    }
}

pub(crate) fn fetch_with_fallback(
    base_urls: &[&str],
    path: &str,
    symbol: &str,
    parser: fn(&str) -> Result<Vec<OhlcBar>, String>,
) -> Result<Vec<OhlcBar>, String> {
    let mut errors = Vec::new();
    for base_url in base_urls {
        let url = format!("{base_url}{path}");
        match http_get(&url)
            .and_then(|body| parser(&body))
            .and_then(|bars| non_empty_bars(bars, symbol))
        {
            Ok(bars) => return Ok(bars),
            Err(error) => errors.push(format!("{base_url}: {error}")),
        }
    }
    Err(errors.join(" | "))
}

pub(crate) fn fetch_with_fallback_allow_empty(
    base_urls: &[&str],
    path: &str,
    parser: fn(&str) -> Result<Vec<OhlcBar>, String>,
) -> Result<Vec<OhlcBar>, String> {
    let mut errors = Vec::new();
    for base_url in base_urls {
        let url = format!("{base_url}{path}");
        match http_get(&url).and_then(|body| parser(&body)) {
            Ok(bars) => return Ok(bars),
            Err(error) => errors.push(format!("{base_url}: {error}")),
        }
    }
    Err(errors.join(" | "))
}

pub(crate) fn apply_interval_aggregation(
    bars: Vec<OhlcBar>,
    interval: BarInterval,
) -> Vec<OhlcBar> {
    match interval.aggregate_seconds() {
        Some(seconds) => aggregate_bars(bars, seconds),
        None => bars,
    }
}

pub(crate) fn aggregate_bars(bars: Vec<OhlcBar>, seconds: i64) -> Vec<OhlcBar> {
    if seconds <= 0 {
        return bars;
    }

    let mut aggregated: Vec<OhlcBar> = Vec::new();
    for bar in bars {
        let bucket_time = bar.time.div_euclid(seconds) * seconds;
        if let Some(current) = aggregated.last_mut()
            && current.time == bucket_time
        {
            current.high = current.high.max(bar.high);
            current.low = current.low.min(bar.low);
            current.close = bar.close;
            current.volume += bar.volume;
            continue;
        }

        aggregated.push(OhlcBar {
            time: bucket_time,
            open: bar.open,
            high: bar.high,
            low: bar.low,
            close: bar.close,
            volume: bar.volume,
        });
    }

    aggregated
}

pub(crate) fn non_empty_bars(bars: Vec<OhlcBar>, symbol: &str) -> Result<Vec<OhlcBar>, String> {
    if bars.is_empty() {
        Err(format!("no valid bars returned for {symbol}"))
    } else {
        Ok(bars)
    }
}

pub(crate) fn number(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|value| value as f64))
        .or_else(|| value.as_str()?.parse::<f64>().ok())
        .filter(|value| value.is_finite())
}

pub(crate) fn encode_query_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}
