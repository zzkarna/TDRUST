use std::collections::BTreeMap;

use super::{
    book::{InstrumentOrderBook, PriceLevel},
    mbo::{MboPrice, MboSide},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DepthFrameKey {
    pub interval_start_ns: u64,
    pub instrument_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepthLevelSample {
    pub side: MboSide,
    pub price: MboPrice,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepthFrame {
    pub key: DepthFrameKey,
    pub levels: Vec<DepthLevelSample>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DepthTimeCache {
    interval_ns: u64,
    depth_limit: usize,
    frames: BTreeMap<DepthFrameKey, DepthFrame>,
}

impl DepthTimeCache {
    pub fn new(interval_ns: u64, depth_limit: usize) -> Self {
        assert!(interval_ns > 0, "depth interval must be non-zero");
        Self {
            interval_ns,
            depth_limit,
            frames: BTreeMap::new(),
        }
    }

    pub const fn interval_ns(&self) -> u64 {
        self.interval_ns
    }

    pub const fn depth_limit(&self) -> usize {
        self.depth_limit
    }

    pub fn record_book(&mut self, timestamp_ns: u64, book: &InstrumentOrderBook) -> DepthFrameKey {
        let key = DepthFrameKey {
            interval_start_ns: bucket_start(timestamp_ns, self.interval_ns),
            instrument_id: book.snapshot().instrument_id,
        };
        let mut levels = Vec::with_capacity(self.depth_limit.saturating_mul(2));
        levels.extend(
            book.levels(MboSide::Bid, self.depth_limit)
                .into_iter()
                .map(|level| DepthLevelSample {
                    side: MboSide::Bid,
                    price: level.price,
                    size: level.size,
                }),
        );
        levels.extend(
            book.levels(MboSide::Ask, self.depth_limit)
                .into_iter()
                .map(|level| DepthLevelSample {
                    side: MboSide::Ask,
                    price: level.price,
                    size: level.size,
                }),
        );
        self.frames.insert(key, DepthFrame { key, levels });
        key
    }

    pub fn frame(&self, key: DepthFrameKey) -> Option<&DepthFrame> {
        self.frames.get(&key)
    }

    pub fn frames(&self) -> impl Iterator<Item = &DepthFrame> {
        self.frames.values()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LiquidityCellKey {
    pub interval_start_ns: u64,
    pub instrument_id: u32,
    pub price: MboPrice,
    pub side: MboSide,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiquidityCell {
    pub displayed_size: u64,
    pub max_displayed_size: u64,
    pub samples: u64,
    pub intensity: f32,
}

impl LiquidityCell {
    fn new(displayed_size: u64, max_visible_size: u64) -> Self {
        Self {
            displayed_size,
            max_displayed_size: displayed_size,
            samples: 1,
            intensity: intensity(displayed_size, max_visible_size),
        }
    }

    fn update(&mut self, displayed_size: u64, max_visible_size: u64) {
        self.displayed_size = displayed_size;
        self.max_displayed_size = self.max_displayed_size.max(displayed_size);
        self.samples += 1;
        self.intensity = intensity(displayed_size, max_visible_size);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LiquidityHeatmapSurface {
    interval_ns: u64,
    depth_limit: usize,
    max_visible_size: u64,
    cells: BTreeMap<LiquidityCellKey, LiquidityCell>,
    bucket_index: BTreeMap<DepthFrameKey, Vec<LiquidityCellKey>>,
}

impl LiquidityHeatmapSurface {
    pub fn new(interval_ns: u64, depth_limit: usize) -> Self {
        assert!(interval_ns > 0, "heatmap interval must be non-zero");
        Self {
            interval_ns,
            depth_limit,
            max_visible_size: 0,
            cells: BTreeMap::new(),
            bucket_index: BTreeMap::new(),
        }
    }

    pub const fn interval_ns(&self) -> u64 {
        self.interval_ns
    }

    pub const fn depth_limit(&self) -> usize {
        self.depth_limit
    }

    pub const fn max_visible_size(&self) -> u64 {
        self.max_visible_size
    }

    pub fn record_book(&mut self, timestamp_ns: u64, book: &InstrumentOrderBook) -> DepthFrameKey {
        let key = DepthFrameKey {
            interval_start_ns: bucket_start(timestamp_ns, self.interval_ns),
            instrument_id: book.snapshot().instrument_id,
        };

        self.zero_existing_bucket(key);
        for (side, level) in book
            .levels(MboSide::Bid, self.depth_limit)
            .into_iter()
            .map(|level| (MboSide::Bid, level))
            .chain(
                book.levels(MboSide::Ask, self.depth_limit)
                    .into_iter()
                    .map(|level| (MboSide::Ask, level)),
            )
        {
            self.record_level(key, side, level);
        }
        key
    }

    pub fn cell(&self, key: LiquidityCellKey) -> Option<LiquidityCell> {
        self.cells.get(&key).copied()
    }

    pub fn cells(&self) -> impl Iterator<Item = (LiquidityCellKey, LiquidityCell)> + '_ {
        self.cells.iter().map(|(key, cell)| (*key, *cell))
    }

    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    pub fn drain_ready_cells(
        &mut self,
        cutoff_ts: u64,
        include_open_bucket: bool,
    ) -> Vec<(LiquidityCellKey, LiquidityCell)> {
        let cutoff_bucket = bucket_start(cutoff_ts, self.interval_ns);
        let keys = self
            .cells
            .keys()
            .copied()
            .filter(|key| include_open_bucket || key.interval_start_ns < cutoff_bucket)
            .collect::<Vec<_>>();
        let bucket_keys = self
            .bucket_index
            .keys()
            .copied()
            .filter(|key| include_open_bucket || key.interval_start_ns < cutoff_bucket)
            .collect::<Vec<_>>();
        for bucket_key in bucket_keys {
            self.bucket_index.remove(&bucket_key);
        }
        keys.into_iter()
            .filter_map(|key| self.cells.remove(&key).map(|cell| (key, cell)))
            .collect()
    }

    pub fn visible_cells_for_instrument(
        &self,
        instrument_id: u32,
        limit: usize,
    ) -> Vec<(LiquidityCellKey, LiquidityCell)> {
        let mut cells = self
            .cells
            .iter()
            .filter(|(key, cell)| key.instrument_id == instrument_id && cell.displayed_size > 0)
            .map(|(key, cell)| (*key, *cell))
            .collect::<Vec<_>>();
        cells.sort_by(|(left_key, _), (right_key, _)| {
            left_key
                .interval_start_ns
                .cmp(&right_key.interval_start_ns)
                .then_with(|| left_key.price.cmp(&right_key.price))
                .then_with(|| left_key.side.cmp(&right_key.side))
        });
        if cells.len() > limit {
            cells.drain(0..cells.len() - limit);
        }
        cells
    }

    fn zero_existing_bucket(&mut self, key: DepthFrameKey) {
        if let Some(cell_keys) = self.bucket_index.get(&key) {
            for cell_key in cell_keys {
                let Some(cell) = self.cells.get_mut(cell_key) else {
                    continue;
                };
                cell.update(0, self.max_visible_size);
            }
        }
    }

    fn record_level(&mut self, key: DepthFrameKey, side: MboSide, level: PriceLevel) {
        self.max_visible_size = self.max_visible_size.max(level.size);
        let cell_key = LiquidityCellKey {
            interval_start_ns: key.interval_start_ns,
            instrument_id: key.instrument_id,
            price: level.price,
            side,
        };
        let is_new_cell = !self.cells.contains_key(&cell_key);
        self.cells
            .entry(cell_key)
            .and_modify(|cell| cell.update(level.size, self.max_visible_size))
            .or_insert_with(|| LiquidityCell::new(level.size, self.max_visible_size));
        if is_new_cell {
            self.bucket_index.entry(key).or_default().push(cell_key);
        }
    }
}

fn intensity(displayed_size: u64, max_visible_size: u64) -> f32 {
    if max_visible_size == 0 {
        0.0
    } else {
        (displayed_size as f32 / max_visible_size as f32).clamp(0.0, 1.0)
    }
}

const fn bucket_start(timestamp_ns: u64, interval_ns: u64) -> u64 {
    timestamp_ns / interval_ns * interval_ns
}

#[cfg(test)]
mod tests {
    use crate::order_flow::{
        DepthTimeCache, FIXED_PRICE_SCALE, LiquidityCellKey, LiquidityHeatmapSurface, MboAction,
        MboEvent, MboPrice, MboSide, MboSourceInfo, OrderBookEngine,
    };

    const SECOND_NS: u64 = 1_000_000_000;

    fn source() -> MboSourceInfo {
        MboSourceInfo::new("GLBX.MDP3", "NQ.FUT", "parent", "instrument_id")
    }

    fn price(value: i64) -> MboPrice {
        MboPrice::from_fixed(value * FIXED_PRICE_SCALE)
    }

    fn event(order_id: u64, side: MboSide, price: MboPrice, size: u32) -> MboEvent {
        MboEvent::book_event(source(), 1, order_id, MboAction::Add, side, price, size)
    }

    #[test]
    fn depth_cache_records_bounded_bid_and_ask_levels() {
        let mut engine = OrderBookEngine::new();
        for (order_id, side, price, size) in [
            (1, MboSide::Bid, price(100), 1),
            (2, MboSide::Bid, price(99), 2),
            (3, MboSide::Ask, price(101), 3),
            (4, MboSide::Ask, price(102), 4),
        ] {
            engine
                .apply(&event(order_id, side, price, size))
                .expect("apply event");
        }

        let mut cache = DepthTimeCache::new(60 * SECOND_NS, 1);
        let key = cache.record_book(61 * SECOND_NS, engine.book(1).expect("book"));
        let frame = cache.frame(key).expect("frame");

        assert_eq!(key.interval_start_ns, 60 * SECOND_NS);
        assert_eq!(frame.levels.len(), 2);
        assert_eq!(frame.levels[0].side, MboSide::Bid);
        assert_eq!(frame.levels[0].price, price(100));
        assert_eq!(frame.levels[1].side, MboSide::Ask);
        assert_eq!(frame.levels[1].price, price(101));
    }

    #[test]
    fn liquidity_surface_records_price_time_cells_and_intensity() {
        let mut engine = OrderBookEngine::new();
        for (order_id, side, price, size) in [
            (1, MboSide::Bid, price(100), 2),
            (2, MboSide::Bid, price(99), 4),
            (3, MboSide::Ask, price(101), 8),
        ] {
            engine
                .apply(&event(order_id, side, price, size))
                .expect("apply event");
        }

        let mut surface = LiquidityHeatmapSurface::new(SECOND_NS, 2);
        let frame_key = surface.record_book(2 * SECOND_NS + 10, engine.book(1).expect("book"));

        assert_eq!(frame_key.interval_start_ns, 2 * SECOND_NS);
        assert_eq!(surface.cell_count(), 3);
        assert_eq!(surface.max_visible_size(), 8);
        let cell = surface
            .cell(LiquidityCellKey {
                interval_start_ns: 2 * SECOND_NS,
                instrument_id: 1,
                price: price(101),
                side: MboSide::Ask,
            })
            .expect("ask cell");
        assert_eq!(cell.displayed_size, 8);
        assert_eq!(cell.intensity, 1.0);
    }

    #[test]
    fn liquidity_surface_tracks_modify_cancel_and_clear() {
        let mut engine = OrderBookEngine::new();
        let mut surface = LiquidityHeatmapSurface::new(SECOND_NS, 4);
        engine
            .apply(&event(1, MboSide::Bid, price(100), 6))
            .expect("add");
        surface.record_book(1, engine.book(1).expect("book"));

        engine
            .apply(&MboEvent::book_event(
                source(),
                1,
                1,
                MboAction::Modify,
                MboSide::Bid,
                price(101),
                3,
            ))
            .expect("modify");
        surface.record_book(2, engine.book(1).expect("book"));

        let old_cell = surface
            .cell(LiquidityCellKey {
                interval_start_ns: 0,
                instrument_id: 1,
                price: price(100),
                side: MboSide::Bid,
            })
            .expect("old cell");
        assert_eq!(old_cell.displayed_size, 0);
        let moved_cell = surface
            .cell(LiquidityCellKey {
                interval_start_ns: 0,
                instrument_id: 1,
                price: price(101),
                side: MboSide::Bid,
            })
            .expect("moved cell");
        assert_eq!(moved_cell.displayed_size, 3);

        engine
            .apply(&MboEvent::book_event(
                source(),
                1,
                1,
                MboAction::Cancel,
                MboSide::Bid,
                price(101),
                3,
            ))
            .expect("cancel");
        surface.record_book(3, engine.book(1).expect("book"));
        assert_eq!(
            surface
                .cell(LiquidityCellKey {
                    interval_start_ns: 0,
                    instrument_id: 1,
                    price: price(101),
                    side: MboSide::Bid,
                })
                .expect("cancelled cell")
                .displayed_size,
            0
        );

        engine
            .apply(&MboEvent::book_event(
                source(),
                1,
                0,
                MboAction::Clear,
                MboSide::None,
                MboPrice::UNDEFINED,
                0,
            ))
            .expect("clear");
        surface.record_book(4, engine.book(1).expect("book"));
        assert!(surface.visible_cells_for_instrument(1, 10).is_empty());
    }
}
