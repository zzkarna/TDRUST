use std::collections::BTreeMap;

use super::mbo::{MboAction, MboEvent, MboPrice, MboSide};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggressorSide {
    Buy,
    Sell,
    Unknown,
}

impl AggressorSide {
    pub const fn from_mbo_side(side: MboSide) -> Self {
        match side {
            MboSide::Bid => Self::Buy,
            MboSide::Ask => Self::Sell,
            MboSide::None | MboSide::Unknown(_) => Self::Unknown,
        }
    }

    pub const fn signed_size(self, size: u32) -> i64 {
        match self {
            Self::Buy => size as i64,
            Self::Sell => -(size as i64),
            Self::Unknown => 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MboTradePrint {
    pub instrument_id: u32,
    pub order_id: u64,
    pub action: MboAction,
    pub side: AggressorSide,
    pub price: MboPrice,
    pub size: u32,
    pub ts_event: u64,
    pub ts_recv: u64,
    pub sequence: u32,
}

impl MboTradePrint {
    pub fn from_event(event: &MboEvent) -> Option<Self> {
        if !matches!(event.action, MboAction::Trade | MboAction::Fill)
            || !event.price.is_defined()
            || event.size == 0
        {
            return None;
        }

        Some(Self {
            instrument_id: event.instrument_id,
            order_id: event.order_id,
            action: event.action,
            side: AggressorSide::from_mbo_side(event.side),
            price: event.price,
            size: event.size,
            ts_event: event.ts_event,
            ts_recv: event.ts_recv,
            sequence: event.sequence,
        })
    }

    pub fn signed_size(&self) -> i64 {
        self.side.signed_size(self.size)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FlowSummary {
    pub trade_count: u64,
    pub fill_count: u64,
    pub buy_volume: u64,
    pub sell_volume: u64,
    pub unknown_volume: u64,
    pub cumulative_delta: i64,
    pub large_trade_count: u64,
}

impl FlowSummary {
    pub const fn total_volume(self) -> u64 {
        self.buy_volume + self.sell_volume + self.unknown_volume
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeFlowAccumulator {
    large_trade_threshold: u32,
    summary: FlowSummary,
    prints: Vec<MboTradePrint>,
}

impl TradeFlowAccumulator {
    pub fn new(large_trade_threshold: u32) -> Self {
        Self {
            large_trade_threshold,
            summary: FlowSummary::default(),
            prints: Vec::new(),
        }
    }

    pub fn apply_event(&mut self, event: &MboEvent) -> Option<MboTradePrint> {
        let print = MboTradePrint::from_event(event)?;

        match print.action {
            MboAction::Trade => self.summary.trade_count += 1,
            MboAction::Fill => self.summary.fill_count += 1,
            _ => {}
        }

        match print.side {
            AggressorSide::Buy => self.summary.buy_volume += u64::from(print.size),
            AggressorSide::Sell => self.summary.sell_volume += u64::from(print.size),
            AggressorSide::Unknown => self.summary.unknown_volume += u64::from(print.size),
        }
        self.summary.cumulative_delta += print.signed_size();
        if self.large_trade_threshold > 0 && print.size >= self.large_trade_threshold {
            self.summary.large_trade_count += 1;
        }

        self.prints.push(print.clone());
        Some(print)
    }

    pub const fn summary(&self) -> FlowSummary {
        self.summary
    }

    pub fn prints(&self) -> &[MboTradePrint] {
        &self.prints
    }

    pub fn clear_prints(&mut self) {
        self.prints.clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VolumeBubbleKey {
    pub interval_start_ns: u64,
    pub instrument_id: u32,
    pub price: MboPrice,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VolumeBubble {
    pub buy_volume: u64,
    pub sell_volume: u64,
    pub unknown_volume: u64,
    pub trade_count: u64,
    pub fill_count: u64,
}

impl VolumeBubble {
    pub const fn total_volume(self) -> u64 {
        self.buy_volume + self.sell_volume + self.unknown_volume
    }

    pub fn delta(self) -> i64 {
        self.buy_volume as i64 - self.sell_volume as i64
    }

    pub fn imbalance(self) -> f64 {
        let total = self.total_volume();
        if total == 0 {
            0.0
        } else {
            self.delta() as f64 / total as f64
        }
    }

    pub fn radius(self, max_volume: u64, min_radius: f32, max_radius: f32) -> f32 {
        if max_volume == 0 || self.total_volume() == 0 {
            return 0.0;
        }
        let normalized_area = self.total_volume() as f32 / max_volume as f32;
        min_radius + (max_radius - min_radius) * normalized_area.sqrt().clamp(0.0, 1.0)
    }

    fn apply_print(&mut self, print: &MboTradePrint) {
        match print.side {
            AggressorSide::Buy => self.buy_volume += u64::from(print.size),
            AggressorSide::Sell => self.sell_volume += u64::from(print.size),
            AggressorSide::Unknown => self.unknown_volume += u64::from(print.size),
        }
        match print.action {
            MboAction::Trade => self.trade_count += 1,
            MboAction::Fill => self.fill_count += 1,
            _ => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeBubbleAggregator {
    interval_ns: u64,
    max_volume: u64,
    bubbles: BTreeMap<VolumeBubbleKey, VolumeBubble>,
}

impl VolumeBubbleAggregator {
    pub fn new(interval_ns: u64) -> Self {
        assert!(interval_ns > 0, "volume bubble interval must be non-zero");
        Self {
            interval_ns,
            max_volume: 0,
            bubbles: BTreeMap::new(),
        }
    }

    pub const fn interval_ns(&self) -> u64 {
        self.interval_ns
    }

    pub const fn max_volume(&self) -> u64 {
        self.max_volume
    }

    pub fn apply_event(&mut self, event: &MboEvent) -> Option<VolumeBubbleKey> {
        let print = MboTradePrint::from_event(event)?;
        Some(self.apply_print(&print))
    }

    pub fn apply_print(&mut self, print: &MboTradePrint) -> VolumeBubbleKey {
        let key = VolumeBubbleKey {
            interval_start_ns: bucket_start(print.ts_event, self.interval_ns),
            instrument_id: print.instrument_id,
            price: print.price,
        };
        let bubble = self.bubbles.entry(key).or_default();
        bubble.apply_print(print);
        self.max_volume = self.max_volume.max(bubble.total_volume());
        key
    }

    pub fn bubble(&self, key: VolumeBubbleKey) -> Option<VolumeBubble> {
        self.bubbles.get(&key).copied()
    }

    pub fn bubbles(&self) -> impl Iterator<Item = (VolumeBubbleKey, VolumeBubble)> + '_ {
        self.bubbles.iter().map(|(key, bubble)| (*key, *bubble))
    }

    pub fn bubble_count(&self) -> usize {
        self.bubbles.len()
    }

    pub fn drain_ready_bubbles(
        &mut self,
        cutoff_ts: u64,
        include_open_bucket: bool,
    ) -> Vec<(VolumeBubbleKey, VolumeBubble)> {
        let cutoff_bucket = bucket_start(cutoff_ts, self.interval_ns);
        let keys = self
            .bubbles
            .keys()
            .copied()
            .filter(|key| include_open_bucket || key.interval_start_ns < cutoff_bucket)
            .collect::<Vec<_>>();
        keys.into_iter()
            .filter_map(|key| self.bubbles.remove(&key).map(|bubble| (key, bubble)))
            .collect()
    }

    pub fn visible_bubbles_for_instrument(
        &self,
        instrument_id: u32,
        limit: usize,
    ) -> Vec<(VolumeBubbleKey, VolumeBubble)> {
        let mut bubbles = self
            .bubbles
            .iter()
            .filter(|(key, bubble)| key.instrument_id == instrument_id && bubble.total_volume() > 0)
            .map(|(key, bubble)| (*key, *bubble))
            .collect::<Vec<_>>();
        bubbles.sort_by(|(left_key, _), (right_key, _)| {
            left_key
                .interval_start_ns
                .cmp(&right_key.interval_start_ns)
                .then_with(|| left_key.price.cmp(&right_key.price))
        });
        if bubbles.len() > limit {
            bubbles.drain(0..bubbles.len() - limit);
        }
        bubbles
    }
}

const fn bucket_start(timestamp_ns: u64, interval_ns: u64) -> u64 {
    timestamp_ns / interval_ns * interval_ns
}

#[cfg(test)]
mod tests {
    use crate::order_flow::{
        AggressorSide, FIXED_PRICE_SCALE, MboAction, MboEvent, MboPrice, MboSide, MboSourceInfo,
        TradeFlowAccumulator, VolumeBubbleAggregator, VolumeBubbleKey,
    };

    fn source() -> MboSourceInfo {
        MboSourceInfo::new("GLBX.MDP3", "NQ.FUT", "parent", "instrument_id")
    }

    fn event(action: MboAction, side: MboSide, size: u32) -> MboEvent {
        MboEvent::book_event(
            source(),
            1,
            42,
            action,
            side,
            MboPrice::from_fixed(18_250 * FIXED_PRICE_SCALE),
            size,
        )
    }

    fn timed_event(ts_event: u64, action: MboAction, side: MboSide, size: u32) -> MboEvent {
        let mut event = event(action, side, size);
        event.ts_event = ts_event;
        event
    }

    #[test]
    fn trade_print_extractor_accepts_trade_and_fill_only() {
        assert!(
            super::MboTradePrint::from_event(&event(MboAction::Trade, MboSide::Bid, 3)).is_some()
        );
        assert!(
            super::MboTradePrint::from_event(&event(MboAction::Fill, MboSide::Ask, 2)).is_some()
        );
        assert!(
            super::MboTradePrint::from_event(&event(MboAction::Add, MboSide::Bid, 3)).is_none()
        );
    }

    #[test]
    fn aggressor_side_maps_bid_to_buy_and_ask_to_sell() {
        let buy = super::MboTradePrint::from_event(&event(MboAction::Trade, MboSide::Bid, 3))
            .expect("buy print");
        let sell = super::MboTradePrint::from_event(&event(MboAction::Trade, MboSide::Ask, 2))
            .expect("sell print");
        let unknown = super::MboTradePrint::from_event(&event(MboAction::Trade, MboSide::None, 5))
            .expect("unknown print");

        assert_eq!(buy.side, AggressorSide::Buy);
        assert_eq!(buy.signed_size(), 3);
        assert_eq!(sell.side, AggressorSide::Sell);
        assert_eq!(sell.signed_size(), -2);
        assert_eq!(unknown.side, AggressorSide::Unknown);
        assert_eq!(unknown.signed_size(), 0);
    }

    #[test]
    fn accumulator_tracks_cvd_volume_and_large_trades() {
        let mut accumulator = TradeFlowAccumulator::new(4);

        accumulator.apply_event(&event(MboAction::Trade, MboSide::Bid, 3));
        accumulator.apply_event(&event(MboAction::Trade, MboSide::Ask, 5));
        accumulator.apply_event(&event(MboAction::Fill, MboSide::Bid, 4));
        accumulator.apply_event(&event(MboAction::Add, MboSide::Bid, 10));

        let summary = accumulator.summary();
        assert_eq!(summary.trade_count, 2);
        assert_eq!(summary.fill_count, 1);
        assert_eq!(summary.buy_volume, 7);
        assert_eq!(summary.sell_volume, 5);
        assert_eq!(summary.total_volume(), 12);
        assert_eq!(summary.cumulative_delta, 2);
        assert_eq!(summary.large_trade_count, 2);
        assert_eq!(accumulator.prints().len(), 3);
    }

    #[test]
    fn volume_bubbles_aggregate_side_and_bucket() {
        let mut bubbles = VolumeBubbleAggregator::new(10);

        bubbles.apply_event(&timed_event(11, MboAction::Trade, MboSide::Bid, 3));
        bubbles.apply_event(&timed_event(12, MboAction::Trade, MboSide::Ask, 5));
        bubbles.apply_event(&timed_event(19, MboAction::Fill, MboSide::None, 2));
        bubbles.apply_event(&timed_event(20, MboAction::Trade, MboSide::Bid, 7));

        let first = bubbles
            .bubble(VolumeBubbleKey {
                interval_start_ns: 10,
                instrument_id: 1,
                price: MboPrice::from_fixed(18_250 * FIXED_PRICE_SCALE),
            })
            .expect("first bubble");

        assert_eq!(first.buy_volume, 3);
        assert_eq!(first.sell_volume, 5);
        assert_eq!(first.unknown_volume, 2);
        assert_eq!(first.total_volume(), 10);
        assert_eq!(first.delta(), -2);
        assert!((first.imbalance() + 0.2).abs() < f64::EPSILON);
        assert_eq!(first.trade_count, 2);
        assert_eq!(first.fill_count, 1);
        assert_eq!(bubbles.max_volume(), 10);
        assert!(first.radius(10, 2.0, 12.0) > first.radius(20, 2.0, 12.0));
        assert_eq!(bubbles.visible_bubbles_for_instrument(1, 1).len(), 1);
    }
}
