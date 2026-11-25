#![allow(dead_code)]

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use serde_json::Value;
use tungstenite::{Message, connect};

use crate::{
    chart_core::OhlcBar,
    market_data::{
        BarInterval, BarUpdate, BarUpdateSink, DatafeedError, HistoryRequest, HistoryResponse,
        MarketDataFeed, MarketSource, SearchSymbol, SubscribeRequest, SubscriptionId,
        SubscriptionKey, SubscriptionRouter, SymbolInfo, SymbolSearch, SymbolType, symbol_info,
    },
};

use super::common::{fetch_with_fallback, fetch_with_fallback_allow_empty, number};

const BINANCE_LIMIT: usize = 720;
const BINANCE_REST_BASE_URLS: &[&str] = &["https://api.binance.com", "https://api.binance.us"];
const BINANCE_WS_BASE_URLS: &[&str] = &[
    "wss://stream.binance.com:9443",
    "wss://stream.binance.us:9443",
];

pub struct BinanceFeed {
    router: Arc<Mutex<SubscriptionRouter>>,
    cancels: Arc<Mutex<HashMap<SubscriptionId, Arc<AtomicBool>>>>,
}

impl BinanceFeed {
    pub fn new() -> Self {
        Self {
            router: Arc::new(Mutex::new(SubscriptionRouter::new())),
            cancels: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl Default for BinanceFeed {
    fn default() -> Self {
        Self::new()
    }
}

impl MarketDataFeed for BinanceFeed {
    fn search_symbols(&self, query: SymbolSearch) -> Result<Vec<SearchSymbol>, DatafeedError> {
        let symbol = normalized_symbol(&query.query);
        if symbol.is_empty() {
            return Ok(Vec::new());
        }

        Ok(vec![SearchSymbol {
            symbol: symbol.clone(),
            full_name: symbol.clone(),
            description: symbol.clone(),
            exchange: MarketSource::Binance.label().to_string(),
            ticker: symbol,
            symbol_type: SymbolType::Crypto,
        }])
    }

    fn resolve_symbol(&self, symbol: &str) -> Result<SymbolInfo, DatafeedError> {
        Ok(symbol_info(
            MarketSource::Binance,
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
        request: SubscribeRequest,
        mut sink: BarUpdateSink,
    ) -> Result<SubscriptionId, DatafeedError> {
        let symbol = normalized_symbol(&request.symbol.ticker);
        let resolution = request.resolution;
        let id = self
            .router
            .lock()
            .map_err(|_| DatafeedError::network("Binance subscription router lock poisoned"))?
            .subscribe(SubscriptionKey {
                symbol: symbol.clone(),
                resolution,
            });
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancels
            .lock()
            .map_err(|_| DatafeedError::network("Binance subscription lock poisoned"))?
            .insert(id, cancel.clone());

        let cancels = self.cancels.clone();
        let router = self.router.clone();
        let interval = BarInterval::from_resolution(resolution)
            .unwrap_or(crate::market_data::DEFAULT_INTERVAL);
        thread::spawn(move || {
            let stream_symbol = symbol.clone();
            let _ = stream_binance_klines(&stream_symbol, interval, cancel, |bar| {
                let accepts = router
                    .lock()
                    .map(|router| router.accepts(id, &stream_symbol, resolution))
                    .unwrap_or(false);
                if !accepts {
                    return;
                }
                sink(BarUpdate {
                    subscription_id: id,
                    symbol: stream_symbol.clone(),
                    resolution,
                    bar,
                });
            });
            if let Ok(mut cancels) = cancels.lock() {
                cancels.remove(&id);
            }
            if let Ok(mut router) = router.lock() {
                router.unsubscribe(id);
            }
        });

        Ok(id)
    }

    fn unsubscribe_bars(&self, id: SubscriptionId) {
        if let Ok(mut router) = self.router.lock() {
            router.unsubscribe(id);
        }
        if let Ok(mut cancels) = self.cancels.lock()
            && let Some(cancel) = cancels.remove(&id)
        {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

pub fn normalized_symbol(symbol: &str) -> String {
    symbol
        .trim()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect()
}

pub fn fetch_bars(symbol: &str, interval: BarInterval) -> Result<Vec<OhlcBar>, DatafeedError> {
    let request = HistoryRequest {
        symbol: symbol_info(MarketSource::Binance, symbol, interval),
        resolution: interval.into(),
        from: None,
        to: None,
        count_back: Some(BINANCE_LIMIT),
        first_data_request: true,
    };
    fetch_bars_for_request(symbol, interval, &request)
}

fn fetch_bars_for_request(
    symbol: &str,
    interval: BarInterval,
    request: &HistoryRequest,
) -> Result<Vec<OhlcBar>, DatafeedError> {
    let symbol = normalized_symbol(symbol);
    let path = binance_klines_path(
        &symbol,
        interval.binance_interval(),
        request.count_back.unwrap_or(BINANCE_LIMIT),
        request.to,
    );
    if request.to.is_some() {
        fetch_with_fallback_allow_empty(BINANCE_REST_BASE_URLS, &path, parse_binance_klines)
            .map_err(DatafeedError::network)
    } else {
        fetch_with_fallback(BINANCE_REST_BASE_URLS, &path, &symbol, parse_binance_klines)
            .map_err(DatafeedError::network)
    }
}

fn binance_klines_path(
    symbol: &str,
    interval: &str,
    limit: usize,
    exclusive_to_seconds: Option<i64>,
) -> String {
    let limit = limit.clamp(1, 1_000);
    let mut path = format!("/api/v3/klines?symbol={symbol}&interval={interval}&limit={limit}");
    if let Some(to) = exclusive_to_seconds
        && to > 0
    {
        path.push_str("&endTime=");
        path.push_str(&(to.saturating_mul(1_000).saturating_sub(1)).to_string());
    }
    path
}

pub fn stream_binance_klines<F>(
    symbol: &str,
    interval: BarInterval,
    cancel: Arc<AtomicBool>,
    mut on_bar: F,
) -> Result<(), String>
where
    F: FnMut(OhlcBar),
{
    let symbol = normalized_symbol(symbol);
    let mut errors = Vec::new();
    for base_url in BINANCE_WS_BASE_URLS {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        match stream_binance_klines_from(
            base_url,
            &symbol,
            interval.binance_interval(),
            cancel.as_ref(),
            &mut on_bar,
        ) {
            Ok(()) => return Ok(()),
            Err(error) => errors.push(format!("{base_url}: {error}")),
        }
    }
    Err(errors.join(" | "))
}

fn stream_binance_klines_from<F>(
    base_url: &str,
    symbol: &str,
    interval: &str,
    cancel: &AtomicBool,
    on_bar: &mut F,
) -> Result<(), String>
where
    F: FnMut(OhlcBar),
{
    let stream = format!("{}@kline_{}", symbol.to_ascii_lowercase(), interval);
    let url = format!("{base_url}/ws/{stream}");
    let (mut websocket, _) =
        connect(url.as_str()).map_err(|error| format!("opening Binance stream: {error}"))?;

    while !cancel.load(Ordering::Relaxed) {
        match websocket.read() {
            Ok(Message::Text(text)) => {
                if let Some(bar) = parse_binance_kline_message(text.as_str())? {
                    on_bar(bar);
                }
            }
            Ok(Message::Ping(payload)) => {
                let _ = websocket.send(Message::Pong(payload));
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(error) => {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                return Err(format!("reading Binance stream: {error}"));
            }
        }
    }

    let _ = websocket.close(None);
    Ok(())
}

pub fn parse_binance_klines(body: &str) -> Result<Vec<OhlcBar>, String> {
    let rows: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
    let rows = rows.as_array().ok_or_else(|| {
        binance_error_message(&rows).unwrap_or_else(|| "missing kline rows".into())
    })?;
    let mut bars = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(fields) = row.as_array() else {
            continue;
        };
        if fields.len() < 6 {
            continue;
        }
        let Some(open_time_ms) = fields[0].as_i64() else {
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
        let Some(volume) = number(&fields[5]) else {
            continue;
        };
        let bar = OhlcBar {
            time: open_time_ms / 1_000,
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

pub fn parse_binance_kline_message(body: &str) -> Result<Option<OhlcBar>, String> {
    let value: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
    let Some(kline) = value.get("k") else {
        return Ok(None);
    };
    let Some(open_time_ms) = kline.get("t").and_then(Value::as_i64) else {
        return Ok(None);
    };
    let Some(open) = kline.get("o").and_then(number) else {
        return Ok(None);
    };
    let Some(high) = kline.get("h").and_then(number) else {
        return Ok(None);
    };
    let Some(low) = kline.get("l").and_then(number) else {
        return Ok(None);
    };
    let Some(close) = kline.get("c").and_then(number) else {
        return Ok(None);
    };
    let Some(volume) = kline.get("v").and_then(number) else {
        return Ok(None);
    };
    let bar = OhlcBar {
        time: open_time_ms / 1_000,
        open,
        high,
        low,
        close,
        volume,
    };
    Ok(bar.is_valid().then_some(bar))
}

fn binance_error_message(value: &Value) -> Option<String> {
    value
        .get("msg")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{
        binance_klines_path, normalized_symbol, parse_binance_kline_message, parse_binance_klines,
    };

    #[test]
    fn binance_rest_parser_converts_milliseconds_to_seconds() {
        let body = r#"[
          [1499040000000,"0.01634790","0.80000000","0.01575800","0.01577100","148976.11427815",1499644799999,"2434.19055334",308,"1756.87402397","28.46694368","0"]
        ]"#;

        let bars = parse_binance_klines(body).expect("valid Binance klines");

        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].time, 1_499_040_000);
        assert_eq!(bars[0].open, 0.01634790);
    }

    #[test]
    fn binance_stream_parser_reads_kline_payload() {
        let body = r#"{
          "e": "kline",
          "E": 1672515782136,
          "s": "BNBBTC",
          "k": {
            "t": 1672515780000,
            "T": 1672515839999,
            "s": "BNBBTC",
            "i": "1m",
            "f": 100,
            "L": 200,
            "o": "0.0010",
            "c": "0.0020",
            "h": "0.0025",
            "l": "0.0010",
            "v": "1000",
            "n": 100,
            "x": false,
            "q": "1.0000",
            "V": "500",
            "Q": "0.500",
            "B": "123456"
          }
        }"#;

        let bar = parse_binance_kline_message(body)
            .expect("valid stream message")
            .expect("kline bar");

        assert_eq!(bar.time, 1_672_515_780);
        assert_eq!(bar.high, 0.0025);
        assert_eq!(bar.volume, 1000.0);
    }

    #[test]
    fn normalization_strips_common_separators() {
        assert_eq!(normalized_symbol("btc/usdt"), "BTCUSDT");
        assert_eq!(normalized_symbol(" eth-usdt "), "ETHUSDT");
    }

    #[test]
    fn klines_path_uses_exclusive_to_as_end_time() {
        assert_eq!(
            binance_klines_path("BTCUSDT", "1m", 2_000, Some(1_700_000_000)),
            "/api/v3/klines?symbol=BTCUSDT&interval=1m&limit=1000&endTime=1699999999999"
        );
    }
}
