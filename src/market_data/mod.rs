#![allow(dead_code)]
#![allow(unused_imports)]

pub mod adapters;
pub mod contract;
pub mod service;
pub mod subscription;

pub use adapters::binance::BinanceFeed;
pub use adapters::coingecko::CoinGeckoFeed;
pub use adapters::csv::load_csv;
pub use adapters::massive::MassiveFeed;
pub use adapters::yfinance::YFinanceFeed;
pub use contract::{
    BarUpdate, BarUpdateSink, DatafeedError, DatafeedErrorKind, HistoryRequest, HistoryResponse,
    MarketDataFeed, PriceFormat, Resolution, SearchSymbol, SessionSpec, SubscribeRequest,
    SubscriptionId, SymbolInfo, SymbolSearch, SymbolType,
};
pub use service::{MarketDataKey, MarketDataService, MarketDataSnapshot};
pub use subscription::{SubscriptionKey, SubscriptionRouter};

use crate::chart_core::OhlcBar;

pub(crate) const DEFAULT_HISTORY_LIMIT: usize = 720;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarInterval {
    label: &'static str,
    yfinance_range: &'static str,
    yfinance_interval: &'static str,
    binance_interval: &'static str,
    aggregate_seconds: Option<i64>,
}

impl BarInterval {
    const fn new(
        label: &'static str,
        yfinance_range: &'static str,
        yfinance_interval: &'static str,
        binance_interval: &'static str,
        aggregate_seconds: Option<i64>,
    ) -> Self {
        Self {
            label,
            yfinance_range,
            yfinance_interval,
            binance_interval,
            aggregate_seconds,
        }
    }

    pub const fn label(self) -> &'static str {
        self.label
    }

    pub fn from_label(label: &str) -> Option<Self> {
        TIMEFRAMES
            .iter()
            .copied()
            .find(|interval| interval.label == label)
    }

    pub fn fixed_seconds(self) -> Option<i64> {
        if let Some(seconds) = self.aggregate_seconds {
            return Some(seconds);
        }
        Resolution::from(self).seconds()
    }

    pub(crate) const fn yfinance_range(self) -> &'static str {
        self.yfinance_range
    }

    pub(crate) const fn yfinance_interval(self) -> &'static str {
        self.yfinance_interval
    }

    pub(crate) const fn binance_interval(self) -> &'static str {
        self.binance_interval
    }

    pub(crate) const fn aggregate_seconds(self) -> Option<i64> {
        self.aggregate_seconds
    }

    pub(crate) fn from_resolution(resolution: Resolution) -> Option<Self> {
        TIMEFRAMES
            .iter()
            .copied()
            .find(|interval| Resolution::from(*interval) == resolution)
    }
}

impl From<BarInterval> for Resolution {
    fn from(interval: BarInterval) -> Self {
        match interval.label {
            "1m" => Self::Minutes(1),
            "3m" => Self::Minutes(3),
            "5m" => Self::Minutes(5),
            "15m" => Self::Minutes(15),
            "1h" => Self::Hours(1),
            "4h" => Self::Hours(4),
            "D" => Self::Days(1),
            "W" => Self::Weeks(1),
            "M" => Self::Months(1),
            "3M" => Self::Months(3),
            "6M" => Self::Months(6),
            "12M" => Self::Months(12),
            _ => Self::Minutes(1),
        }
    }
}

pub const TIMEFRAMES: &[BarInterval] = &[
    BarInterval::new("1m", "5d", "1m", "1m", None),
    BarInterval::new("3m", "5d", "1m", "3m", Some(3 * 60)),
    BarInterval::new("5m", "1mo", "5m", "5m", None),
    BarInterval::new("15m", "60d", "15m", "15m", None),
    BarInterval::new("1h", "730d", "60m", "1h", None),
    BarInterval::new("4h", "730d", "60m", "4h", Some(4 * 60 * 60)),
    BarInterval::new("D", "10y", "1d", "1d", None),
    BarInterval::new("W", "10y", "1wk", "1w", None),
    BarInterval::new("M", "max", "1mo", "1M", None),
    BarInterval::new("3M", "max", "3mo", "1M", None),
    BarInterval::new("6M", "max", "3mo", "1M", None),
    BarInterval::new("12M", "max", "3mo", "1M", None),
];

pub const DEFAULT_INTERVAL: BarInterval = TIMEFRAMES[2];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarketSource {
    YFinance,
    Massive,
    Binance,
    CoinGecko,
}

impl MarketSource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::YFinance => "YFinance",
            Self::Massive => "Massive",
            Self::Binance => "Binance",
            Self::CoinGecko => "CoinGecko",
        }
    }

    pub const fn stable_key(self) -> &'static str {
        match self {
            Self::YFinance => "yfinance",
            Self::Massive => "massive",
            Self::Binance => "binance",
            Self::CoinGecko => "coingecko",
        }
    }

    pub fn from_stable_key(value: &str) -> Option<Self> {
        match value {
            "yfinance" | "YFinance" => Some(Self::YFinance),
            "massive" | "Massive" => Some(Self::Massive),
            "binance" | "Binance" => Some(Self::Binance),
            "coingecko" | "CoinGecko" => Some(Self::CoinGecko),
            _ => None,
        }
    }
}

pub fn normalized_symbol(source: MarketSource, symbol: &str) -> String {
    match source {
        MarketSource::YFinance => adapters::yfinance::normalized_symbol(symbol),
        MarketSource::Massive => adapters::massive::normalized_symbol(symbol),
        MarketSource::Binance => adapters::binance::normalized_symbol(symbol),
        MarketSource::CoinGecko => adapters::coingecko::normalized_symbol(symbol),
    }
}

pub fn fetch_bars(
    source: MarketSource,
    symbol: &str,
    interval: BarInterval,
) -> Result<Vec<OhlcBar>, String> {
    let symbol = normalized_symbol(source, symbol);
    let request = HistoryRequest {
        symbol: symbol_info(source, &symbol, interval),
        resolution: interval.into(),
        from: None,
        to: None,
        count_back: Some(DEFAULT_HISTORY_LIMIT),
        first_data_request: true,
    };

    let response = match source {
        MarketSource::YFinance => YFinanceFeed.get_bars(request),
        MarketSource::Massive => MassiveFeed.get_bars(request),
        MarketSource::Binance => BinanceFeed::new().get_bars(request),
        MarketSource::CoinGecko => CoinGeckoFeed.get_bars(request),
    }
    .map_err(|error| error.to_string())?;

    Ok(response.bars)
}

pub fn history_request(
    source: MarketSource,
    symbol: &str,
    interval: BarInterval,
) -> HistoryRequest {
    let symbol = normalized_symbol(source, symbol);
    HistoryRequest {
        symbol: symbol_info(source, &symbol, interval),
        resolution: interval.into(),
        from: None,
        to: None,
        count_back: Some(DEFAULT_HISTORY_LIMIT),
        first_data_request: true,
    }
}

pub fn fetch_yfinance_bars(symbol: &str, interval: BarInterval) -> Result<Vec<OhlcBar>, String> {
    adapters::yfinance::fetch_bars(symbol, interval).map_err(|error| error.to_string())
}

pub fn fetch_massive_bars(symbol: &str, interval: BarInterval) -> Result<Vec<OhlcBar>, String> {
    adapters::massive::fetch_bars(symbol, interval).map_err(|error| error.to_string())
}

pub fn fetch_binance_bars(symbol: &str, interval: BarInterval) -> Result<Vec<OhlcBar>, String> {
    adapters::binance::fetch_bars(symbol, interval).map_err(|error| error.to_string())
}

pub fn fetch_coingecko_bars(symbol: &str, interval: BarInterval) -> Result<Vec<OhlcBar>, String> {
    adapters::coingecko::fetch_bars(symbol, interval).map_err(|error| error.to_string())
}

pub fn format_interval_status(
    source: MarketSource,
    interval: BarInterval,
    bar_count: usize,
) -> String {
    format!(
        "Loaded {bar_count} {} bars from {}",
        interval.label(),
        source.label()
    )
}

pub(crate) fn symbol_info(source: MarketSource, symbol: &str, interval: BarInterval) -> SymbolInfo {
    SymbolInfo {
        name: normalized_symbol(source, symbol),
        ticker: normalized_symbol(source, symbol),
        description: symbol.trim().to_ascii_uppercase(),
        exchange: source.label().to_string(),
        listed_exchange: source.label().to_string(),
        symbol_type: match source {
            MarketSource::YFinance | MarketSource::Massive => SymbolType::Stock,
            MarketSource::Binance | MarketSource::CoinGecko => SymbolType::Crypto,
        },
        session: SessionSpec::default(),
        timezone: "Etc/UTC".to_string(),
        minmov: 1,
        pricescale: 100,
        price_format: PriceFormat::Price,
        volume_precision: 0,
        has_intraday: true,
        supported_resolutions: TIMEFRAMES.iter().map(|item| (*item).into()).collect(),
        has_volume: source != MarketSource::CoinGecko,
        default_resolution: interval.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BarInterval, MarketSource, Resolution, TIMEFRAMES, adapters, format_interval_status,
        normalized_symbol,
    };
    use crate::chart_core::OhlcBar;

    #[test]
    fn aggregate_bars_builds_larger_ohlcv_buckets() {
        let bars = vec![
            OhlcBar {
                time: 0,
                open: 10.0,
                high: 12.0,
                low: 9.0,
                close: 11.0,
                volume: 100.0,
            },
            OhlcBar {
                time: 60,
                open: 11.0,
                high: 13.0,
                low: 10.0,
                close: 12.0,
                volume: 150.0,
            },
            OhlcBar {
                time: 180,
                open: 12.0,
                high: 14.0,
                low: 11.0,
                close: 13.0,
                volume: 200.0,
            },
        ];

        let aggregated = adapters::common::aggregate_bars(bars, 180);

        assert_eq!(aggregated.len(), 2);
        assert_eq!(aggregated[0].open, 10.0);
        assert_eq!(aggregated[0].high, 13.0);
        assert_eq!(aggregated[0].low, 9.0);
        assert_eq!(aggregated[0].close, 12.0);
        assert_eq!(aggregated[0].volume, 250.0);
    }

    #[test]
    fn status_mentions_selected_interval() {
        let interval = BarInterval::new("5m", "1mo", "5m", "5m", None);

        assert_eq!(
            format_interval_status(MarketSource::YFinance, interval, 10),
            "Loaded 10 5m bars from YFinance"
        );
    }

    #[test]
    fn binance_symbols_allow_common_separators_in_search() {
        assert_eq!(
            normalized_symbol(MarketSource::Binance, "btc/usdt"),
            "BTCUSDT"
        );
        assert_eq!(
            normalized_symbol(MarketSource::YFinance, " brk-b "),
            "BRK-B"
        );
        assert_eq!(normalized_symbol(MarketSource::Massive, " brk.b "), "BRK.B");
    }

    #[test]
    fn coingecko_symbol_info_marks_missing_volume() {
        let info = super::symbol_info(MarketSource::CoinGecko, "BTC", TIMEFRAMES[2]);

        assert!(!info.has_volume);
        assert_eq!(info.symbol_type, super::SymbolType::Crypto);
    }

    #[test]
    fn massive_symbol_info_is_stock_data_with_volume() {
        let info = super::symbol_info(MarketSource::Massive, "AAPL", TIMEFRAMES[2]);

        assert!(info.has_volume);
        assert_eq!(info.symbol_type, super::SymbolType::Stock);
        assert_eq!(info.exchange, "Massive");
    }

    #[test]
    fn intervals_map_to_contract_resolutions() {
        assert_eq!(Resolution::from(TIMEFRAMES[0]), Resolution::Minutes(1));
        assert_eq!(Resolution::from(TIMEFRAMES[8]), Resolution::Months(1));
        assert_eq!(
            BarInterval::from_resolution(Resolution::Hours(4)).map(BarInterval::label),
            Some("4h")
        );
    }
}
