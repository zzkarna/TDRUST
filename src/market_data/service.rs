use std::collections::HashMap;

use crate::chart_core::{BarStore, OhlcBar, TailUpdate};

use super::{BarInterval, MarketSource, Resolution, normalized_symbol};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MarketDataKey {
    source: MarketSource,
    symbol: String,
    resolution: Resolution,
}

impl MarketDataKey {
    pub fn new(source: MarketSource, symbol: impl AsRef<str>, resolution: Resolution) -> Self {
        Self {
            source,
            symbol: normalized_symbol(source, symbol.as_ref()),
            resolution,
        }
    }

    pub fn from_interval(
        source: MarketSource,
        symbol: impl AsRef<str>,
        interval: BarInterval,
    ) -> Self {
        Self::new(source, symbol, interval.into())
    }

    pub const fn source(&self) -> MarketSource {
        self.source
    }

    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    pub const fn resolution(&self) -> Resolution {
        self.resolution
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketDataSnapshot {
    pub key: MarketDataKey,
    pub bar_count: usize,
    pub generation: u64,
    pub first_time: Option<i64>,
    pub last_time: Option<i64>,
}

impl MarketDataSnapshot {
    fn from_store(key: MarketDataKey, store: &BarStore) -> Self {
        Self {
            key,
            bar_count: store.len(),
            generation: store.generation(),
            first_time: store.front().map(|bar| bar.time),
            last_time: store.back().map(|bar| bar.time),
        }
    }
}

#[derive(Debug, Default)]
pub struct MarketDataService {
    stores: HashMap<MarketDataKey, BarStore>,
}

impl MarketDataService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn replace_all(
        &mut self,
        key: MarketDataKey,
        bars: impl IntoIterator<Item = OhlcBar>,
    ) -> MarketDataSnapshot {
        let store = self.stores.entry(key.clone()).or_default();
        store.replace_all(bars);
        MarketDataSnapshot::from_store(key, store)
    }

    pub fn prepend_history(
        &mut self,
        key: MarketDataKey,
        bars: impl IntoIterator<Item = OhlcBar>,
    ) -> (usize, MarketDataSnapshot) {
        let store = self.stores.entry(key.clone()).or_default();
        let added = store.prepend_history(bars);
        (added, MarketDataSnapshot::from_store(key, store))
    }

    pub fn apply_tail(
        &mut self,
        key: MarketDataKey,
        bar: OhlcBar,
    ) -> (TailUpdate, MarketDataSnapshot) {
        let store = self.stores.entry(key.clone()).or_default();
        let update = store.apply_tail(bar);
        (update, MarketDataSnapshot::from_store(key, store))
    }

    pub fn snapshot(&self, key: &MarketDataKey) -> Option<MarketDataSnapshot> {
        self.stores
            .get(key)
            .map(|store| MarketDataSnapshot::from_store(key.clone(), store))
    }

    pub fn store(&self, key: &MarketDataKey) -> Option<&BarStore> {
        self.stores.get(key)
    }

    pub fn remove(&mut self, key: &MarketDataKey) -> Option<BarStore> {
        self.stores.remove(key)
    }

    pub fn len(&self) -> usize {
        self.stores.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stores.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{MarketDataKey, MarketDataService};
    use crate::{
        chart_core::{OhlcBar, TailUpdate},
        market_data::{MarketSource, Resolution, TIMEFRAMES},
    };

    fn bar(time: i64, open: f64, close: f64) -> OhlcBar {
        OhlcBar {
            time,
            open,
            high: open.max(close) + 1.0,
            low: open.min(close) - 1.0,
            close,
            volume: 100.0,
        }
    }

    #[test]
    fn data_key_normalizes_symbol_by_source() {
        let binance =
            MarketDataKey::from_interval(MarketSource::Binance, "btc/usdt", TIMEFRAMES[0]);
        let yfinance = MarketDataKey::new(MarketSource::YFinance, " aapl ", Resolution::Days(1));

        assert_eq!(binance.symbol(), "BTCUSDT");
        assert_eq!(binance.resolution(), Resolution::Minutes(1));
        assert_eq!(yfinance.symbol(), "AAPL");
        assert_eq!(yfinance.source(), MarketSource::YFinance);
    }

    #[test]
    fn market_data_service_reuses_store_for_same_key() {
        let key = MarketDataKey::new(MarketSource::YFinance, "AAPL", Resolution::Minutes(5));
        let mut service = MarketDataService::new();

        let initial = service.replace_all(key.clone(), [bar(60, 10.0, 11.0)]);
        let (update, after_tail) = service.apply_tail(key.clone(), bar(120, 11.0, 12.0));

        assert_eq!(update, TailUpdate::Appended);
        assert_eq!(service.len(), 1);
        assert_eq!(initial.key, key);
        assert_eq!(after_tail.bar_count, 2);
        assert!(after_tail.generation > initial.generation);
        assert_eq!(service.store(&key).expect("shared store").len(), 2);
    }

    #[test]
    fn market_data_service_isolates_different_keys() {
        let five_minute =
            MarketDataKey::new(MarketSource::YFinance, "AAPL", Resolution::Minutes(5));
        let daily = MarketDataKey::new(MarketSource::YFinance, "AAPL", Resolution::Days(1));
        let mut service = MarketDataService::new();

        service.replace_all(five_minute.clone(), [bar(60, 10.0, 11.0)]);
        service.replace_all(
            daily.clone(),
            [bar(86_400, 12.0, 13.0), bar(172_800, 13.0, 14.0)],
        );

        assert_eq!(service.len(), 2);
        assert_eq!(
            service
                .snapshot(&five_minute)
                .expect("5m snapshot")
                .bar_count,
            1
        );
        assert_eq!(
            service.snapshot(&daily).expect("daily snapshot").bar_count,
            2
        );
    }

    #[test]
    fn market_data_service_prepends_history_by_key() {
        let key = MarketDataKey::new(MarketSource::Binance, "BTCUSDT", Resolution::Minutes(1));
        let mut service = MarketDataService::new();
        service.replace_all(key.clone(), [bar(120, 11.0, 12.0)]);

        let (added, snapshot) = service.prepend_history(key, [bar(60, 10.0, 11.0)]);

        assert_eq!(added, 1);
        assert_eq!(snapshot.bar_count, 2);
        assert_eq!(snapshot.first_time, Some(60));
        assert_eq!(snapshot.last_time, Some(120));
    }
}
