use serde_json::Value;

use crate::{
    chart_core::OhlcBar,
    market_data::{
        BarInterval, BarUpdateSink, DatafeedError, HistoryRequest, HistoryResponse, MarketDataFeed,
        MarketSource, SearchSymbol, SubscribeRequest, SubscriptionId, SymbolInfo, SymbolSearch,
        symbol_info,
    },
};

use super::common::{
    apply_interval_aggregation, encode_query_component, fetch_with_fallback,
    fetch_with_fallback_allow_empty, number,
};

const YFINANCE_BASE_URLS: &[&str] = &[
    "https://query1.finance.yahoo.com",
    "https://query2.finance.yahoo.com",
];

pub struct YFinanceFeed;

impl MarketDataFeed for YFinanceFeed {
    fn search_symbols(&self, query: SymbolSearch) -> Result<Vec<SearchSymbol>, DatafeedError> {
        let symbol = normalized_symbol(&query.query);
        if symbol.is_empty() {
            return Ok(Vec::new());
        }

        Ok(vec![SearchSymbol {
            symbol: symbol.clone(),
            full_name: symbol.clone(),
            description: symbol.clone(),
            exchange: MarketSource::YFinance.label().to_string(),
            ticker: symbol,
            symbol_type: crate::market_data::SymbolType::Stock,
        }])
    }

    fn resolve_symbol(&self, symbol: &str) -> Result<SymbolInfo, DatafeedError> {
        Ok(symbol_info(
            MarketSource::YFinance,
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
            "YFinance adapter does not support realtime bar subscriptions",
        ))
    }

    fn unsubscribe_bars(&self, _id: SubscriptionId) {}
}

pub fn normalized_symbol(symbol: &str) -> String {
    symbol.trim().to_ascii_uppercase()
}

pub fn fetch_bars(symbol: &str, interval: BarInterval) -> Result<Vec<OhlcBar>, DatafeedError> {
    let request = HistoryRequest {
        symbol: symbol_info(MarketSource::YFinance, symbol, interval),
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
    interval: BarInterval,
    request: &HistoryRequest,
) -> Result<Vec<OhlcBar>, DatafeedError> {
    let symbol = normalized_symbol(symbol);
    let path = yfinance_chart_path(&symbol, interval, request);
    let bars = if request.to.is_some() {
        fetch_with_fallback_allow_empty(YFINANCE_BASE_URLS, &path, parse_yfinance_chart)
            .map_err(DatafeedError::network)?
    } else {
        fetch_with_fallback(YFINANCE_BASE_URLS, &path, &symbol, parse_yfinance_chart)
            .map_err(DatafeedError::network)?
    };
    Ok(apply_interval_aggregation(bars, interval))
}

fn yfinance_chart_path(symbol: &str, interval: BarInterval, request: &HistoryRequest) -> String {
    let symbol = encode_query_component(symbol);
    if let Some(to) = request.to {
        let from = request.from.unwrap_or_else(|| {
            yfinance_period_start(
                to,
                request
                    .count_back
                    .unwrap_or(crate::market_data::DEFAULT_HISTORY_LIMIT),
                interval,
                request.resolution,
            )
        });
        format!(
            "/v8/finance/chart/{symbol}?period1={}&period2={}&interval={}&includePrePost=false&events=div%2Csplits",
            from.max(0),
            to.max(1),
            interval.yfinance_interval(),
        )
    } else {
        format!(
            "/v8/finance/chart/{symbol}?range={}&interval={}&includePrePost=false&events=div%2Csplits",
            interval.yfinance_range(),
            interval.yfinance_interval(),
        )
    }
}

fn yfinance_period_start(
    to: i64,
    count_back: usize,
    interval: BarInterval,
    resolution: crate::market_data::Resolution,
) -> i64 {
    let seconds_per_bar = interval
        .aggregate_seconds()
        .or_else(|| resolution.seconds())
        .unwrap_or(86_400)
        .max(1);
    let requested = count_back.max(1) as i64;
    to.saturating_sub(seconds_per_bar.saturating_mul(requested + 2))
}

pub fn parse_yfinance_chart(body: &str) -> Result<Vec<OhlcBar>, String> {
    let value: Value = serde_json::from_str(body).map_err(|error| error.to_string())?;
    let result = value.pointer("/chart/result/0").ok_or_else(|| {
        yahoo_error_message(&value).unwrap_or_else(|| "missing chart result".into())
    })?;
    let timestamps = result
        .get("timestamp")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing Yahoo timestamp array".to_string())?;
    let quote = result
        .pointer("/indicators/quote/0")
        .ok_or_else(|| "missing Yahoo quote data".to_string())?;
    let opens = quote_array(quote, "open")?;
    let highs = quote_array(quote, "high")?;
    let lows = quote_array(quote, "low")?;
    let closes = quote_array(quote, "close")?;
    let volumes = quote_array(quote, "volume")?;

    let mut bars = Vec::with_capacity(timestamps.len());
    for index in 0..timestamps.len() {
        let Some(time) = timestamps[index].as_i64() else {
            continue;
        };
        let Some(open) = number_at(opens, index) else {
            continue;
        };
        let Some(high) = number_at(highs, index) else {
            continue;
        };
        let Some(low) = number_at(lows, index) else {
            continue;
        };
        let Some(close) = number_at(closes, index) else {
            continue;
        };
        let volume = number_at(volumes, index).unwrap_or(0.0);
        let bar = OhlcBar {
            time,
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

fn quote_array<'a>(quote: &'a Value, key: &str) -> Result<&'a [Value], String> {
    quote
        .get(key)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("missing Yahoo {key} array"))
}

fn number_at(values: &[Value], index: usize) -> Option<f64> {
    values.get(index).and_then(number)
}

fn yahoo_error_message(value: &Value) -> Option<String> {
    value
        .pointer("/chart/error/description")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{parse_yfinance_chart, yfinance_chart_path, yfinance_period_start};
    use crate::market_data::{
        DEFAULT_INTERVAL, HistoryRequest, MarketSource, Resolution, TIMEFRAMES, history_request,
    };

    #[test]
    fn yfinance_parser_reads_quote_arrays() {
        let body = r#"{
          "chart": {
            "result": [{
              "timestamp": [1714620000, 1714620060],
              "indicators": {
                "quote": [{
                  "open": [100.0, 101.0],
                  "high": [102.0, 103.0],
                  "low": [99.0, 100.5],
                  "close": [101.0, 102.5],
                  "volume": [1500, 1800]
                }]
              }
            }],
            "error": null
          }
        }"#;

        let bars = parse_yfinance_chart(body).expect("valid yahoo chart");

        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].time, 1_714_620_000);
        assert_eq!(bars[1].close, 102.5);
    }

    #[test]
    fn yfinance_initial_request_uses_range_query() {
        let request = history_request(MarketSource::YFinance, "NVDA", DEFAULT_INTERVAL);

        assert_eq!(
            yfinance_chart_path("NVDA", DEFAULT_INTERVAL, &request),
            "/v8/finance/chart/NVDA?range=1mo&interval=5m&includePrePost=false&events=div%2Csplits"
        );
    }

    #[test]
    fn yfinance_backfill_request_uses_period_window() {
        let mut request = history_request(MarketSource::YFinance, "NVDA", DEFAULT_INTERVAL);
        request.to = Some(1_700_000_000);
        request.count_back = Some(720);

        assert_eq!(
            yfinance_chart_path("NVDA", DEFAULT_INTERVAL, &request),
            "/v8/finance/chart/NVDA?period1=1699783400&period2=1700000000&interval=5m&includePrePost=false&events=div%2Csplits"
        );
    }

    #[test]
    fn yfinance_period_start_uses_aggregate_resolution_for_synthetic_intervals() {
        assert_eq!(
            yfinance_period_start(1_700_000_000, 10, TIMEFRAMES[1], Resolution::Minutes(3),),
            1_699_997_840
        );
    }
}
