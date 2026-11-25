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

use super::common::encode_query_component;

const DEMO_BASE_URL: &str = "https://api.coingecko.com";
const PRO_BASE_URL: &str = "https://pro-api.coingecko.com";
const API_KEY_ENV: &str = "COINGECKO_API_KEY";

pub struct CoinGeckoFeed;

impl MarketDataFeed for CoinGeckoFeed {
    fn search_symbols(&self, query: SymbolSearch) -> Result<Vec<SearchSymbol>, DatafeedError> {
        let symbol = normalized_symbol(&query.query);
        if symbol.is_empty() {
            return Ok(Vec::new());
        }

        Ok(vec![SearchSymbol {
            symbol: symbol.clone(),
            full_name: symbol.clone(),
            description: symbol.clone(),
            exchange: MarketSource::CoinGecko.label().to_string(),
            ticker: symbol,
            symbol_type: SymbolType::Crypto,
        }])
    }

    fn resolve_symbol(&self, symbol: &str) -> Result<SymbolInfo, DatafeedError> {
        Ok(symbol_info(
            MarketSource::CoinGecko,
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
            "CoinGecko adapter does not support websocket bar subscriptions",
        ))
    }

    fn unsubscribe_bars(&self, _id: SubscriptionId) {}
}

pub fn normalized_symbol(symbol: &str) -> String {
    let trimmed = symbol.trim();
    let slash_base = trimmed.split_once('/').map_or(trimmed, |(base, _)| base);
    let base = slash_base
        .split_once('-')
        .and_then(|(base, quote)| is_quote_suffix(quote).then_some(base))
        .unwrap_or(slash_base)
        .trim()
        .to_ascii_lowercase();
    let base = strip_quote_suffix(&base);
    coin_id_alias(&base).unwrap_or(base)
}

pub fn fetch_bars(symbol: &str, interval: BarInterval) -> Result<Vec<OhlcBar>, DatafeedError> {
    let request = HistoryRequest {
        symbol: symbol_info(MarketSource::CoinGecko, symbol, interval),
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
    let coin_id = normalized_symbol(symbol);
    let path = if request.to.is_some() {
        coingecko_ohlc_range_path(&coin_id, request)
    } else {
        coingecko_ohlc_days_path(&coin_id, request)
    };
    let body = get_coingecko(&path)?;
    parse_coingecko_ohlc(&body)
        .map_err(DatafeedError::parse)
        .map(|bars| bars.into_iter().filter(|bar| bar.is_valid()).collect())
}

fn get_coingecko(path: &str) -> Result<String, DatafeedError> {
    let key = coingecko_api_key();
    let candidates = coingecko_request_candidates(key.as_ref());
    let mut errors = Vec::new();

    for candidate in candidates {
        let url = format!("{}{}", candidate.base_url, path);
        let mut request = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(12))
            .user_agent("rustchart/0.1")
            .build()
            .map_err(|error| DatafeedError::network(format!("building HTTP client: {error}")))?
            .get(&url);

        if let Some(api_key) = key.as_ref() {
            request = request.header(candidate.header, api_key.value.as_str());
        }

        match request.send() {
            Ok(response) => {
                let status = response.status();
                let body = response.text().map_err(|error| {
                    DatafeedError::network(format!("reading CoinGecko response body: {error}"))
                })?;
                if status.is_success() {
                    return Ok(body);
                }
                errors.push(format!(
                    "{} HTTP {status}: {}",
                    candidate.base_url,
                    body.trim()
                ));
            }
            Err(error) => errors.push(format!("{} request failed: {error}", candidate.base_url)),
        }
    }

    Err(DatafeedError::network(errors.join(" | ")))
}

#[derive(Debug, Clone, Copy)]
struct CoinGeckoRequestCandidate {
    base_url: &'static str,
    header: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoinGeckoKeyKind {
    Demo,
    Pro,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CoinGeckoApiKey {
    value: String,
    kind: CoinGeckoKeyKind,
}

fn coingecko_request_candidates(key: Option<&CoinGeckoApiKey>) -> Vec<CoinGeckoRequestCandidate> {
    if env::var("COINGECKO_API_BASE_URL")
        .ok()
        .as_deref()
        .is_some_and(|base| base.contains("pro-api"))
    {
        return vec![CoinGeckoRequestCandidate {
            base_url: PRO_BASE_URL,
            header: "x-cg-pro-api-key",
        }];
    }

    match key.map(|key| key.kind) {
        Some(CoinGeckoKeyKind::Demo) => vec![CoinGeckoRequestCandidate {
            base_url: DEMO_BASE_URL,
            header: "x-cg-demo-api-key",
        }],
        Some(CoinGeckoKeyKind::Pro) => vec![CoinGeckoRequestCandidate {
            base_url: PRO_BASE_URL,
            header: "x-cg-pro-api-key",
        }],
        Some(CoinGeckoKeyKind::Unknown) => vec![
            CoinGeckoRequestCandidate {
                base_url: PRO_BASE_URL,
                header: "x-cg-pro-api-key",
            },
            CoinGeckoRequestCandidate {
                base_url: DEMO_BASE_URL,
                header: "x-cg-demo-api-key",
            },
        ],
        None => vec![CoinGeckoRequestCandidate {
            base_url: DEMO_BASE_URL,
            header: "x-cg-demo-api-key",
        }],
    }
}

fn coingecko_api_key() -> Option<CoinGeckoApiKey> {
    raw_coingecko_api_key().and_then(|value| parse_coingecko_api_key(&value))
}

fn raw_coingecko_api_key() -> Option<String> {
    env::var(API_KEY_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| dotenv_value(API_KEY_ENV))
}

fn parse_coingecko_api_key(raw: &str) -> Option<CoinGeckoApiKey> {
    let value = raw.trim().trim_matches('"').trim_matches('\'');
    if value.is_empty() {
        return None;
    }

    for (name, kind) in [
        ("x_cg_demo_api_key", CoinGeckoKeyKind::Demo),
        ("x-cg-demo-api-key", CoinGeckoKeyKind::Demo),
        ("x_cg_pro_api_key", CoinGeckoKeyKind::Pro),
        ("x-cg-pro-api-key", CoinGeckoKeyKind::Pro),
    ] {
        if let Some(key) = extract_key_parameter(value, name) {
            return Some(CoinGeckoApiKey { value: key, kind });
        }
    }

    let kind = if value.contains("pro-api.coingecko.com") {
        CoinGeckoKeyKind::Pro
    } else if value.contains("api.coingecko.com") {
        CoinGeckoKeyKind::Demo
    } else {
        CoinGeckoKeyKind::Unknown
    };
    Some(CoinGeckoApiKey {
        value: value.to_string(),
        kind,
    })
}

fn extract_key_parameter(value: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=");
    let start = value.find(&needle)? + needle.len();
    let key = value[start..]
        .split(['&', '#', ' ', '\t', '\r', '\n'])
        .next()?
        .trim()
        .trim_matches('"')
        .trim_matches('\'');
    (!key.is_empty()).then(|| key.to_string())
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

fn coingecko_ohlc_days_path(coin_id: &str, request: &HistoryRequest) -> String {
    let days = coingecko_days(request);
    format!(
        "/api/v3/coins/{}/ohlc?vs_currency=usd&days={days}&precision=full",
        encode_query_component(coin_id)
    )
}

fn coingecko_ohlc_range_path(coin_id: &str, request: &HistoryRequest) -> String {
    let to = request.to.unwrap_or_else(now_seconds);
    let from = request.from.unwrap_or_else(|| {
        coingecko_range_start(
            to,
            request
                .count_back
                .unwrap_or(crate::market_data::DEFAULT_HISTORY_LIMIT),
            request.resolution,
        )
    });
    let interval = coingecko_range_interval(request.resolution);
    format!(
        "/api/v3/coins/{}/ohlc/range?vs_currency=usd&from={}&to={}&interval={interval}",
        encode_query_component(coin_id),
        from.max(0),
        to.max(1),
    )
}

fn coingecko_days(request: &HistoryRequest) -> &'static str {
    let seconds = request.resolution.seconds().unwrap_or(86_400);
    let count = request
        .count_back
        .unwrap_or(crate::market_data::DEFAULT_HISTORY_LIMIT)
        .max(1) as i64;
    let needed_days = ((seconds.saturating_mul(count) + 86_399) / 86_400).max(1);
    match needed_days {
        0..=1 => "1",
        2..=7 => "7",
        8..=14 => "14",
        15..=30 => "30",
        31..=90 => "90",
        91..=180 => "180",
        181..=365 => "365",
        _ => "max",
    }
}

fn coingecko_range_interval(resolution: Resolution) -> &'static str {
    match resolution {
        Resolution::Ticks(_)
        | Resolution::Seconds(_)
        | Resolution::Minutes(_)
        | Resolution::Hours(_) => "hourly",
        Resolution::Days(_) | Resolution::Weeks(_) | Resolution::Months(_) => "daily",
    }
}

fn coingecko_range_start(to: i64, count_back: usize, resolution: Resolution) -> i64 {
    let seconds = resolution.seconds().unwrap_or(86_400).max(1);
    to.saturating_sub(seconds.saturating_mul(count_back.max(1) as i64 + 2))
}

fn parse_coingecko_ohlc(body: &str) -> Result<Vec<OhlcBar>, String> {
    let rows: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
    let rows = rows.as_array().ok_or_else(|| {
        coingecko_error_message(&rows).unwrap_or_else(|| "missing OHLC rows".into())
    })?;
    let mut bars = Vec::with_capacity(rows.len());

    for row in rows {
        let Some(fields) = row.as_array() else {
            continue;
        };
        if fields.len() < 5 {
            continue;
        }
        let Some(time_ms) = fields[0].as_i64() else {
            continue;
        };
        let Some(open) = number(&fields[1]) else {
            continue;
        };
        let Some(high) = number(&fields[2]) else {
            continue;
        };
        let Some(low) = number(&fields[3]) else {
            continue;
        };
        let Some(close) = number(&fields[4]) else {
            continue;
        };
        bars.push(OhlcBar {
            time: time_ms / 1_000,
            open,
            high,
            low,
            close,
            volume: 0.0,
        });
    }

    Ok(bars)
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|value| value as f64))
        .or_else(|| value.as_str()?.parse::<f64>().ok())
        .filter(|value| value.is_finite())
}

fn coingecko_error_message(value: &Value) -> Option<String> {
    value
        .get("error")
        .or_else(|| {
            value
                .get("status")
                .and_then(|status| status.get("error_message"))
        })
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn coin_id_alias(symbol: &str) -> Option<String> {
    match symbol {
        "btc" | "xbt" => Some("bitcoin".to_string()),
        "eth" => Some("ethereum".to_string()),
        "sol" => Some("solana".to_string()),
        "bnb" => Some("binancecoin".to_string()),
        "xrp" => Some("ripple".to_string()),
        "doge" => Some("dogecoin".to_string()),
        "ada" => Some("cardano".to_string()),
        "avax" => Some("avalanche-2".to_string()),
        "dot" => Some("polkadot".to_string()),
        "link" => Some("chainlink".to_string()),
        "matic" | "pol" => Some("matic-network".to_string()),
        _ => None,
    }
}

fn is_quote_suffix(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "usd" | "usdt" | "usdc" | "btc" | "eth" | "eur"
    )
}

fn strip_quote_suffix(symbol: &str) -> String {
    for quote in ["usdt", "usdc", "usd", "eur"] {
        if let Some(base) = symbol.strip_suffix(quote) {
            if !base.is_empty() {
                return base.to_string();
            }
        }
    }
    symbol.to_string()
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::{
        CoinGeckoKeyKind, coingecko_days, coingecko_ohlc_days_path, coingecko_ohlc_range_path,
        coingecko_range_interval, coingecko_range_start, coingecko_request_candidates,
        normalized_symbol, parse_coingecko_api_key, parse_coingecko_ohlc,
    };
    use crate::market_data::{DEFAULT_INTERVAL, MarketSource, Resolution, history_request};

    #[test]
    fn normalizes_common_crypto_tickers_to_coin_ids() {
        assert_eq!(normalized_symbol("BTC/USD"), "bitcoin");
        assert_eq!(normalized_symbol(" eth "), "ethereum");
        assert_eq!(normalized_symbol("avalanche-2"), "avalanche-2");
        assert_eq!(normalized_symbol("sol-usd"), "solana");
        assert_eq!(normalized_symbol("btcusd"), "bitcoin");
        assert_eq!(normalized_symbol("ETHUSDT"), "ethereum");
    }

    #[test]
    fn extracts_demo_key_from_pasted_url() {
        let parsed = parse_coingecko_api_key(
            "https://api.coingecko.com/api/v3/simple/price?ids=bitcoin&x_cg_demo_api_key=CG-demo",
        )
        .expect("demo key");

        assert_eq!(parsed.value, "CG-demo");
        assert_eq!(parsed.kind, CoinGeckoKeyKind::Demo);

        let candidates = coingecko_request_candidates(Some(&parsed));
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].base_url, "https://api.coingecko.com");
        assert_eq!(candidates[0].header, "x-cg-demo-api-key");
    }

    #[test]
    fn parses_ohlc_rows_without_volume() {
        let body = r#"[[1709395200000,61942,62211,61721,61845]]"#;

        let bars = parse_coingecko_ohlc(body).expect("valid CoinGecko OHLC");

        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].time, 1_709_395_200);
        assert_eq!(bars[0].open, 61_942.0);
        assert_eq!(bars[0].volume, 0.0);
    }

    #[test]
    fn initial_ohlc_request_uses_days_endpoint() {
        let request = history_request(MarketSource::CoinGecko, "BTC", DEFAULT_INTERVAL);

        assert_eq!(
            coingecko_ohlc_days_path("bitcoin", &request),
            "/api/v3/coins/bitcoin/ohlc?vs_currency=usd&days=7&precision=full"
        );
    }

    #[test]
    fn backfill_ohlc_request_uses_range_endpoint() {
        let mut request = history_request(MarketSource::CoinGecko, "BTC", DEFAULT_INTERVAL);
        request.to = Some(1_700_000_000);
        request.count_back = Some(24);

        assert_eq!(
            coingecko_ohlc_range_path("bitcoin", &request),
            "/api/v3/coins/bitcoin/ohlc/range?vs_currency=usd&from=1699992200&to=1700000000&interval=hourly"
        );
    }

    #[test]
    fn range_helpers_pick_interval_and_start() {
        assert_eq!(coingecko_range_interval(Resolution::Minutes(5)), "hourly");
        assert_eq!(coingecko_range_interval(Resolution::Days(1)), "daily");
        assert_eq!(
            coingecko_range_start(1_700_000_000, 10, Resolution::Hours(1)),
            1_699_956_800
        );
        let mut request = history_request(MarketSource::CoinGecko, "BTC", DEFAULT_INTERVAL);
        request.count_back = Some(720);
        assert_eq!(coingecko_days(&request), "7");
    }
}
