use std::{env, fs, time::Duration};

use serde_json::Value;

use crate::{
    chart_core::OhlcBar,
    market_data::{
        BarInterval, BarUpdateSink, DatafeedError, HistoryRequest, HistoryResponse, MarketDataFeed,
        MarketSource, Resolution, SearchSymbol, SubscribeRequest, SubscriptionId, SymbolInfo,
        SymbolSearch, SymbolType, symbol_info,
    },
};

use super::common::{encode_query_component, non_empty_bars, number};

const MASSIVE_BASE_URL: &str = "https://api.massive.com";
const API_KEY_ENV: &str = "MASSIVE_API_KEY";
const BASE_URL_ENV: &str = "MASSIVE_API_BASE_URL";

pub struct MassiveFeed;

impl MarketDataFeed for MassiveFeed {
    fn search_symbols(&self, query: SymbolSearch) -> Result<Vec<SearchSymbol>, DatafeedError> {
        let symbol = normalized_symbol(&query.query);
        if symbol.is_empty() {
            return Ok(Vec::new());
        }

        Ok(vec![SearchSymbol {
            symbol: symbol.clone(),
            full_name: symbol.clone(),
            description: symbol.clone(),
            exchange: MarketSource::Massive.label().to_string(),
            ticker: symbol,
            symbol_type: SymbolType::Stock,
        }])
    }

    fn resolve_symbol(&self, symbol: &str) -> Result<SymbolInfo, DatafeedError> {
        Ok(symbol_info(
            MarketSource::Massive,
            &normalized_symbol(symbol),
            crate::market_data::DEFAULT_INTERVAL,
        ))
    }

    fn get_bars(&self, request: HistoryRequest) -> Result<HistoryResponse, DatafeedError> {
        let interval = BarInterval::from_resolution(request.resolution)
            .unwrap_or(crate::market_data::DEFAULT_INTERVAL);
        let bars = fetch_bars_for_request(&request.symbol.ticker, interval, &request)?;
        Ok(HistoryResponse::for_request(bars, &request))
    }

    fn subscribe_bars(
        &self,
        _request: SubscribeRequest,
        _sink: BarUpdateSink,
    ) -> Result<SubscriptionId, DatafeedError> {
        Err(DatafeedError::unsupported(
            "Massive adapter does not support realtime bar subscriptions yet",
        ))
    }

    fn unsubscribe_bars(&self, _id: SubscriptionId) {}
}

pub fn normalized_symbol(symbol: &str) -> String {
    symbol
        .trim()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '.' || *ch == '-')
        .flat_map(char::to_uppercase)
        .collect()
}

pub fn fetch_bars(symbol: &str, interval: BarInterval) -> Result<Vec<OhlcBar>, DatafeedError> {
    let request = HistoryRequest {
        symbol: symbol_info(MarketSource::Massive, symbol, interval),
        resolution: interval.into(),
        from: None,
        to: None,
        count_back: Some(crate::market_data::DEFAULT_HISTORY_LIMIT),
        first_data_request: true,
    };
    fetch_bars_for_request(symbol, interval, &request)
}

fn fetch_bars_for_request(
    symbol: &str,
    _interval: BarInterval,
    request: &HistoryRequest,
) -> Result<Vec<OhlcBar>, DatafeedError> {
    let symbol = normalized_symbol(symbol);
    let path = massive_aggs_path(&symbol, request)?;
    let body = get_massive(&path)?;
    let bars = parse_massive_aggs(&body).map_err(DatafeedError::parse)?;
    if request.to.is_some() {
        Ok(bars)
    } else {
        non_empty_bars(bars, &symbol).map_err(DatafeedError::network)
    }
}

fn get_massive(path: &str) -> Result<String, DatafeedError> {
    let api_key = massive_api_key().ok_or_else(|| {
        DatafeedError::network(format!(
            "Massive API key missing; set {API_KEY_ENV} in the environment or ignored .env"
        ))
    })?;
    let base_url = env::var(BASE_URL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| MASSIVE_BASE_URL.to_string());
    let url = format!("{}{}", base_url.trim_end_matches('/'), path);
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent("rustchart/0.1")
        .build()
        .map_err(|error| DatafeedError::network(format!("building HTTP client: {error}")))?
        .get(&url)
        .bearer_auth(api_key)
        .send()
        .map_err(|error| DatafeedError::network(format!("Massive request failed: {error}")))?;

    let status = response.status();
    let body = response.text().map_err(|error| {
        DatafeedError::network(format!("reading Massive response body: {error}"))
    })?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(DatafeedError::network(format!(
            "Massive HTTP {status}: {}",
            massive_error_message(&body).unwrap_or_else(|| body.trim().to_string())
        )))
    }
}

fn massive_api_key() -> Option<String> {
    env::var(API_KEY_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| dotenv_value(API_KEY_ENV))
        .map(|value| {
            value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string()
        })
        .filter(|value| !value.is_empty())
}

fn dotenv_value(key: &str) -> Option<String> {
    let contents = fs::read_to_string(".env").ok()?;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() == key {
            return Some(
                value
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            );
        }
    }
    None
}

fn massive_aggs_path(symbol: &str, request: &HistoryRequest) -> Result<String, DatafeedError> {
    let (multiplier, timespan) = massive_resolution(request.resolution)?;
    let to = request.to.unwrap_or_else(now_seconds);
    let from = request.from.unwrap_or_else(|| {
        massive_range_start(
            to,
            request
                .count_back
                .unwrap_or(crate::market_data::DEFAULT_HISTORY_LIMIT),
            request.resolution,
        )
    });
    Ok(format!(
        "/v2/aggs/ticker/{}/range/{}/{}/{}/{}?adjusted=true&sort=asc&limit={}",
        encode_query_component(symbol),
        multiplier,
        timespan,
        seconds_to_millis(from.max(0)),
        seconds_to_millis(to.max(1)),
        request
            .count_back
            .unwrap_or(crate::market_data::DEFAULT_HISTORY_LIMIT)
            .clamp(1, 50_000)
    ))
}

fn massive_resolution(resolution: Resolution) -> Result<(u32, &'static str), DatafeedError> {
    match resolution {
        Resolution::Minutes(value) => Ok((value.max(1), "minute")),
        Resolution::Hours(value) => Ok((value.max(1), "hour")),
        Resolution::Days(value) => Ok((value.max(1), "day")),
        Resolution::Weeks(value) => Ok((value.max(1), "week")),
        Resolution::Months(value) => Ok((value.max(1), "month")),
        Resolution::Ticks(_) | Resolution::Seconds(_) => Err(DatafeedError::unsupported(
            "Massive adapter supports minute-or-higher aggregate bars",
        )),
    }
}

fn massive_range_start(to: i64, count_back: usize, resolution: Resolution) -> i64 {
    let seconds = resolution.seconds().unwrap_or(30 * 86_400).max(1);
    to.saturating_sub(seconds.saturating_mul(count_back.max(1) as i64 + 2))
}

fn seconds_to_millis(seconds: i64) -> i64 {
    seconds.saturating_mul(1_000)
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

fn parse_massive_aggs(body: &str) -> Result<Vec<OhlcBar>, String> {
    let value: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
    if let Some(message) = massive_error_message_from_value(&value) {
        return Err(message);
    }
    let rows = value
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing Massive aggregate results".to_string())?;
    let mut bars = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(time_ms) = row.get("t").and_then(Value::as_i64) else {
            continue;
        };
        let Some(open) = row.get("o").and_then(number) else {
            continue;
        };
        let Some(high) = row.get("h").and_then(number) else {
            continue;
        };
        let Some(low) = row.get("l").and_then(number) else {
            continue;
        };
        let Some(close) = row.get("c").and_then(number) else {
            continue;
        };
        let volume = row.get("v").and_then(number).unwrap_or(0.0);
        let bar = OhlcBar {
            time: time_ms / 1_000,
            open,
            high,
            low,
            close,
            volume,
        };
        if bar.is_valid() {
            bars.push(bar);
        }
    }
    Ok(bars)
}

fn massive_error_message(body: &str) -> Option<String> {
    let value = serde_json::from_str::<Value>(body).ok()?;
    massive_error_message_from_value(&value)
}

fn massive_error_message_from_value(value: &Value) -> Option<String> {
    value
        .get("error")
        .and_then(Value::as_str)
        .or_else(|| value.get("message").and_then(Value::as_str))
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{
        massive_aggs_path, massive_resolution, normalized_symbol, parse_massive_aggs,
        seconds_to_millis,
    };
    use crate::market_data::{DEFAULT_INTERVAL, MarketSource, Resolution, history_request};

    #[test]
    fn massive_symbols_preserve_stock_suffixes() {
        assert_eq!(normalized_symbol(" brk.b "), "BRK.B");
        assert_eq!(normalized_symbol("rklb-ws"), "RKLB-WS");
    }

    #[test]
    fn massive_path_uses_aggregate_range_endpoint() {
        let mut request = history_request(MarketSource::Massive, "AAPL", DEFAULT_INTERVAL);
        request.from = Some(1_700_000_000);
        request.to = Some(1_700_086_400);
        request.count_back = Some(720);

        assert_eq!(
            massive_aggs_path("AAPL", &request).expect("path"),
            "/v2/aggs/ticker/AAPL/range/5/minute/1700000000000/1700086400000?adjusted=true&sort=asc&limit=720"
        );
    }

    #[test]
    fn massive_parser_reads_ohlcv_results() {
        let body = r#"{
          "ticker": "AAPL",
          "status": "OK",
          "results": [
            {"t": 1700000000000, "o": 100.0, "h": 102.0, "l": 99.0, "c": 101.0, "v": 12345},
            {"t": 1700000300000, "o": 101.0, "h": 103.0, "l": 100.0, "c": 102.0, "v": 23456}
          ]
        }"#;

        let bars = parse_massive_aggs(body).expect("valid Massive aggs");

        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].time, 1_700_000_000);
        assert_eq!(bars[1].close, 102.0);
        assert_eq!(bars[1].volume, 23_456.0);
    }

    #[test]
    fn massive_resolution_maps_supported_chart_resolutions() {
        assert_eq!(
            massive_resolution(Resolution::Minutes(5)).expect("resolution"),
            (5, "minute")
        );
        assert_eq!(
            massive_resolution(Resolution::Months(3)).expect("resolution"),
            (3, "month")
        );
    }

    #[test]
    fn massive_seconds_to_millis_saturates() {
        assert_eq!(seconds_to_millis(1_700_000_000), 1_700_000_000_000);
    }
}
