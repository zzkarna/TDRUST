use std::collections::BTreeMap;

use super::{
    flow::{AggressorSide, MboTradePrint},
    mbo::{MboEvent, MboPrice},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FootprintCellKey {
    pub interval_start_ns: u64,
    pub price: MboPrice,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FootprintCell {
    pub buy_volume: u64,
    pub sell_volume: u64,
    pub unknown_volume: u64,
}

impl FootprintCell {
    pub const fn total_volume(self) -> u64 {
        self.buy_volume + self.sell_volume + self.unknown_volume
    }

    pub fn delta(self) -> i64 {
        self.buy_volume as i64 - self.sell_volume as i64
    }

    pub fn imbalance_ratio(self) -> Option<f64> {
        let smaller = self.buy_volume.min(self.sell_volume);
        let larger = self.buy_volume.max(self.sell_volume);
        (smaller > 0).then_some(larger as f64 / smaller as f64)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FootprintSnapshot {
    pub interval_ns: u64,
    pub cells: BTreeMap<FootprintCellKey, FootprintCell>,
}

impl FootprintSnapshot {
    pub fn point_of_control(&self) -> Option<(FootprintCellKey, FootprintCell)> {
        self.cells
            .iter()
            .max_by_key(|(_, cell)| cell.total_volume())
            .map(|(key, cell)| (*key, *cell))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FootprintAggregator {
    interval_ns: u64,
    cells: BTreeMap<FootprintCellKey, FootprintCell>,
}

impl FootprintAggregator {
    pub fn new(interval_ns: u64) -> Self {
        assert!(interval_ns > 0, "footprint interval must be non-zero");
        Self {
            interval_ns,
            cells: BTreeMap::new(),
        }
    }

    pub const fn interval_ns(&self) -> u64 {
        self.interval_ns
    }

    pub fn apply_event(&mut self, event: &MboEvent) -> Option<FootprintCellKey> {
        let print = MboTradePrint::from_event(event)?;
        Some(self.apply_print(&print))
    }

    pub fn apply_print(&mut self, print: &MboTradePrint) -> FootprintCellKey {
        let key = FootprintCellKey {
            interval_start_ns: bucket_start(print.ts_event, self.interval_ns),
            price: print.price,
        };
        let cell = self.cells.entry(key).or_default();
        match print.side {
            AggressorSide::Buy => cell.buy_volume += u64::from(print.size),
            AggressorSide::Sell => cell.sell_volume += u64::from(print.size),
            AggressorSide::Unknown => cell.unknown_volume += u64::from(print.size),
        }
        key
    }

    pub fn cell(&self, key: FootprintCellKey) -> Option<FootprintCell> {
        self.cells.get(&key).copied()
    }

    pub fn snapshot(&self) -> FootprintSnapshot {
        FootprintSnapshot {
            interval_ns: self.interval_ns,
            cells: self.cells.clone(),
        }
    }

    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    pub fn drain_ready_cells(
        &mut self,
        cutoff_ts: u64,
        include_open_bucket: bool,
    ) -> FootprintSnapshot {
        let cutoff_bucket = bucket_start(cutoff_ts, self.interval_ns);
        let keys = self
            .cells
            .keys()
            .copied()
            .filter(|key| include_open_bucket || key.interval_start_ns < cutoff_bucket)
            .collect::<Vec<_>>();
        let cells = keys
            .into_iter()
            .filter_map(|key| self.cells.remove(&key).map(|cell| (key, cell)))
            .collect();
        FootprintSnapshot {
            interval_ns: self.interval_ns,
            cells,
        }
    }
}

const fn bucket_start(timestamp_ns: u64, interval_ns: u64) -> u64 {
    timestamp_ns / interval_ns * interval_ns
}

#[cfg(test)]
mod tests {
    use crate::order_flow::{
        FIXED_PRICE_SCALE, FootprintAggregator, FootprintCell, MboAction, MboEvent, MboPrice,
        MboSide, MboSourceInfo,
    };

    const SECOND_NS: u64 = 1_000_000_000;

    fn source() -> MboSourceInfo {
        MboSourceInfo::new("GLBX.MDP3", "NQ.FUT", "parent", "instrument_id")
    }

    fn event(ts_event: u64, action: MboAction, side: MboSide, price: i64, size: u32) -> MboEvent {
        let mut event = MboEvent::book_event(
            source(),
            1,
            99,
            action,
            side,
            MboPrice::from_fixed(price * FIXED_PRICE_SCALE),
            size,
        );
        event.ts_event = ts_event;
        event
    }

    #[test]
    fn footprint_buckets_trades_by_interval_and_price() {
        let mut footprint = FootprintAggregator::new(60 * SECOND_NS);

        let first_key = footprint
            .apply_event(&event(
                61 * SECOND_NS,
                MboAction::Trade,
                MboSide::Bid,
                100,
                3,
            ))
            .expect("first trade");
        let second_key = footprint
            .apply_event(&event(
                62 * SECOND_NS,
                MboAction::Trade,
                MboSide::Ask,
                100,
                5,
            ))
            .expect("second trade");

        assert_eq!(first_key, second_key);
        assert_eq!(first_key.interval_start_ns, 60 * SECOND_NS);
        assert_eq!(
            footprint.cell(first_key),
            Some(FootprintCell {
                buy_volume: 3,
                sell_volume: 5,
                unknown_volume: 0,
            })
        );
    }

    #[test]
    fn footprint_ignores_non_trade_events() {
        let mut footprint = FootprintAggregator::new(60 * SECOND_NS);

        assert!(
            footprint
                .apply_event(&event(60 * SECOND_NS, MboAction::Add, MboSide::Bid, 100, 3))
                .is_none()
        );
        assert!(footprint.snapshot().cells.is_empty());
    }

    #[test]
    fn footprint_snapshot_reports_delta_ratio_and_point_of_control() {
        let mut footprint = FootprintAggregator::new(60 * SECOND_NS);

        let first = footprint
            .apply_event(&event(
                60 * SECOND_NS,
                MboAction::Trade,
                MboSide::Bid,
                100,
                10,
            ))
            .expect("first");
        footprint.apply_event(&event(
            60 * SECOND_NS,
            MboAction::Trade,
            MboSide::Ask,
            100,
            2,
        ));
        let second = footprint
            .apply_event(&event(
                60 * SECOND_NS,
                MboAction::Trade,
                MboSide::Ask,
                101,
                6,
            ))
            .expect("second");

        let snapshot = footprint.snapshot();
        let first_cell = snapshot.cells.get(&first).copied().expect("first cell");
        assert_eq!(first_cell.total_volume(), 12);
        assert_eq!(first_cell.delta(), 8);
        assert_eq!(first_cell.imbalance_ratio(), Some(5.0));
        assert_eq!(
            snapshot.point_of_control(),
            Some((first, first_cell)),
            "higher total-volume cell should be POC"
        );
        assert_eq!(
            snapshot
                .cells
                .get(&second)
                .copied()
                .expect("second cell")
                .delta(),
            -6
        );
    }
}
