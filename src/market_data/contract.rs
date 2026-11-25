#![allow(dead_code)]

use std::fmt;

use crate::chart_core::OhlcBar;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Resolution {
    Ticks(u32),
    Seconds(u32),
    Minutes(u32),
    Hours(u32),
    Days(u32),
    Weeks(u32),
    Months(u32),
}

impl Resolution {
    pub fn label(self) -> String {
        match self {
            Self::Ticks(value) => format!("{value}T"),
            Self::Seconds(value) => format!("{value}S"),
            Self::Minutes(value) => format!("{value}m"),
            Self::Hours(value) => format!("{value}h"),
            Self::Days(1) => "D".to_string(),
            Self::Days(value) => format!("{value}D"),
            Self::Weeks(1) => "W".to_string(),
            Self::Weeks(value) => format!("{value}W"),
            Self::Months(1) => "M".to_string(),
            Self::Months(value) => format!("{value}M"),
        }
    }

    pub fn seconds(self) -> Option<i64> {
        match self {
            Self::Ticks(_) => None,
            Self::Seconds(value) => Some(i64::from(value)),
            Self::Minutes(value) => Some(i64::from(value) * 60),
            Self::Hours(value) => Some(i64::from(value) * 3_600),
            Self::Days(value) => Some(i64::from(value) * 86_400),
            Self::Weeks(value) => Some(i64::from(value) * 7 * 86_400),
            Self::Months(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceFormat {
    Price,
    Volume,
    Percent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolType {
    Stock,
    Futures,
    Forex,
    Crypto,
    Index,
    Fund,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSpec {
    pub regular: String,
    pub holidays: Vec<String>,
    pub corrections: Vec<String>,
}

impl Default for SessionSpec {
    fn default() -> Self {
        Self {
            regular: "24x7".to_string(),
            holidays: Vec::new(),
            corrections: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolInfo {
    pub name: String,
    pub ticker: String,
    pub description: String,
    pub exchange: String,
    pub listed_exchange: String,
    pub symbol_type: SymbolType,
    pub session: SessionSpec,
    pub timezone: String,
    pub minmov: u32,
    pub pricescale: u32,
    pub price_format: PriceFormat,
    pub volume_precision: u32,
    pub has_intraday: bool,
    pub supported_resolutions: Vec<Resolution>,
    pub has_volume: bool,
    pub default_resolution: Resolution,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolSearch {
    pub query: String,
    pub exchange: Option<String>,
    pub symbol_type: Option<SymbolType>,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchSymbol {
    pub symbol: String,
    pub full_name: String,
    pub description: String,
    pub exchange: String,
    pub ticker: String,
    pub symbol_type: SymbolType,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRequest {
    pub symbol: SymbolInfo,
    pub resolution: Resolution,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub count_back: Option<usize>,
    pub first_data_request: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryResponse {
    pub bars: Vec<OhlcBar>,
    pub no_data: bool,
    pub next_time: Option<i64>,
}

impl HistoryResponse {
    pub fn from_bars(bars: Vec<OhlcBar>) -> Self {
        let bars = sorted_unique_valid_bars(bars);
        Self {
            no_data: bars.is_empty(),
            bars,
            next_time: None,
        }
    }

    pub fn for_request(bars: Vec<OhlcBar>, request: &HistoryRequest) -> Self {
        let bars = sorted_unique_valid_bars(bars);
        let upper = request
            .to
            .map_or(bars.len(), |to| bars.partition_point(|bar| bar.time < to));
        let mut lower = request
            .from
            .map_or(0, |from| bars.partition_point(|bar| bar.time < from))
            .min(upper);

        if let Some(count_back) = request.count_back {
            lower = upper.saturating_sub(count_back);
        }

        let bars = bars[lower..upper].to_vec();
        Self {
            no_data: bars.is_empty(),
            bars,
            next_time: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscribeRequest {
    pub symbol: SymbolInfo,
    pub resolution: Resolution,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub u64);

#[derive(Debug, Clone, PartialEq)]
pub struct BarUpdate {
    pub subscription_id: SubscriptionId,
    pub symbol: String,
    pub resolution: Resolution,
    pub bar: OhlcBar,
}

pub type BarUpdateSink = Box<dyn FnMut(BarUpdate) + Send + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatafeedErrorKind {
    Network,
    Parse,
    NoData,
    Unsupported,
    InvalidRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatafeedError {
    pub kind: DatafeedErrorKind,
    pub message: String,
}

impl DatafeedError {
    pub fn new(kind: DatafeedErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn parse(message: impl Into<String>) -> Self {
        Self::new(DatafeedErrorKind::Parse, message)
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self::new(DatafeedErrorKind::Network, message)
    }

    pub fn no_data(message: impl Into<String>) -> Self {
        Self::new(DatafeedErrorKind::NoData, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(DatafeedErrorKind::Unsupported, message)
    }
}

impl fmt::Display for DatafeedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

pub trait MarketDataFeed: Send + Sync {
    fn search_symbols(&self, query: SymbolSearch) -> Result<Vec<SearchSymbol>, DatafeedError>;
    fn resolve_symbol(&self, symbol: &str) -> Result<SymbolInfo, DatafeedError>;
    fn get_bars(&self, request: HistoryRequest) -> Result<HistoryResponse, DatafeedError>;
    fn subscribe_bars(
        &self,
        request: SubscribeRequest,
        sink: BarUpdateSink,
    ) -> Result<SubscriptionId, DatafeedError>;
    fn unsubscribe_bars(&self, id: SubscriptionId);
}

fn sorted_unique_valid_bars(mut bars: Vec<OhlcBar>) -> Vec<OhlcBar> {
    bars.retain(|bar| bar.is_valid());
    bars.sort_by_key(|bar| bar.time);

    let mut unique: Vec<OhlcBar> = Vec::with_capacity(bars.len());
    for bar in bars {
        if let Some(last) = unique.last_mut()
            && last.time == bar.time
        {
            *last = bar;
            continue;
        }
        unique.push(bar);
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::{HistoryResponse, Resolution};
    use crate::chart_core::OhlcBar;

    fn bar(time: i64, close: f64) -> OhlcBar {
        OhlcBar {
            time,
            open: close,
            high: close,
            low: close,
            close,
            volume: 1.0,
        }
    }

    fn invalid_bar(time: i64) -> OhlcBar {
        OhlcBar {
            time,
            open: 10.0,
            high: 9.0,
            low: 11.0,
            close: 10.0,
            volume: 1.0,
        }
    }

    #[test]
    fn history_response_sorts_and_deduplicates_bars() {
        let response = HistoryResponse::from_bars(vec![bar(2, 2.0), bar(1, 1.0), bar(2, 3.0)]);

        assert_eq!(response.bars.len(), 2);
        assert_eq!(response.bars[0].time, 1);
        assert_eq!(response.bars[1].time, 2);
        assert_eq!(response.bars[1].close, 3.0);
        assert!(!response.no_data);
    }

    #[test]
    fn history_response_uses_to_as_exclusive_upper_bound() {
        let request = request(Some(2), Some(4), None);

        let response = HistoryResponse::for_request(
            vec![bar(1, 1.0), bar(2, 2.0), bar(3, 3.0), bar(4, 4.0)],
            &request,
        );

        assert_eq!(
            response.bars.iter().map(|bar| bar.time).collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn history_response_backfills_count_back_before_from() {
        let request = request(Some(4), Some(6), Some(4));

        let response = HistoryResponse::for_request(
            vec![
                bar(1, 1.0),
                bar(2, 2.0),
                bar(3, 3.0),
                bar(4, 4.0),
                bar(5, 5.0),
                bar(6, 6.0),
            ],
            &request,
        );

        assert_eq!(
            response.bars.iter().map(|bar| bar.time).collect::<Vec<_>>(),
            vec![2, 3, 4, 5]
        );
    }

    #[test]
    fn history_response_count_back_uses_latest_when_range_is_open() {
        let request = request(None, None, Some(2));

        let response =
            HistoryResponse::for_request(vec![bar(1, 1.0), bar(2, 2.0), bar(3, 3.0)], &request);

        assert_eq!(
            response.bars.iter().map(|bar| bar.time).collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn history_response_no_data_is_true_for_empty_or_invalid_results() {
        let request = request(None, None, Some(2));

        let empty = HistoryResponse::for_request(Vec::new(), &request);
        let invalid_only = HistoryResponse::from_bars(vec![invalid_bar(1)]);

        assert!(empty.no_data);
        assert!(empty.bars.is_empty());
        assert!(invalid_only.no_data);
        assert!(invalid_only.bars.is_empty());
    }

    #[test]
    fn history_response_count_back_applies_after_sort_dedupe_and_exclusive_to() {
        let request = request(None, Some(4), Some(2));

        let response = HistoryResponse::for_request(
            vec![
                bar(4, 4.0),
                bar(2, 2.0),
                bar(1, 1.0),
                bar(3, 3.0),
                bar(2, 20.0),
            ],
            &request,
        );

        assert!(!response.no_data);
        assert_eq!(
            response
                .bars
                .iter()
                .map(|bar| (bar.time, bar.close))
                .collect::<Vec<_>>(),
            vec![(2, 20.0), (3, 3.0)]
        );
    }

    #[test]
    fn resolution_labels_and_seconds_are_stable() {
        assert_eq!(Resolution::Minutes(5).label(), "5m");
        assert_eq!(Resolution::Days(1).label(), "D");
        assert_eq!(Resolution::Hours(4).seconds(), Some(14_400));
        assert_eq!(Resolution::Months(1).seconds(), None);
    }

    fn request(
        from: Option<i64>,
        to: Option<i64>,
        count_back: Option<usize>,
    ) -> super::HistoryRequest {
        super::HistoryRequest {
            symbol: super::SymbolInfo {
                name: "TEST".to_string(),
                ticker: "TEST".to_string(),
                description: "TEST".to_string(),
                exchange: "TEST".to_string(),
                listed_exchange: "TEST".to_string(),
                symbol_type: super::SymbolType::Unknown,
                session: super::SessionSpec::default(),
                timezone: "Etc/UTC".to_string(),
                minmov: 1,
                pricescale: 100,
                price_format: super::PriceFormat::Price,
                volume_precision: 0,
                has_intraday: true,
                supported_resolutions: vec![Resolution::Minutes(1)],
                has_volume: true,
                default_resolution: Resolution::Minutes(1),
            },
            resolution: Resolution::Minutes(1),
            from,
            to,
            count_back,
            first_data_request: false,
        }
    }
}
