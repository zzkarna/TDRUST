use std::collections::BTreeMap;

use super::{
    book::{BookQuality, InstrumentOrderBook, PriceLevel},
    mbo::{MboPrice, MboSide},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DomColumnLevel {
    pub price: MboPrice,
    pub bid_size: u64,
    pub ask_size: u64,
    pub total_size: u64,
    pub side_hint: MboSide,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomColumnSnapshot {
    pub instrument_id: u32,
    pub quality: BookQuality,
    pub best_bid: Option<PriceLevel>,
    pub best_ask: Option<PriceLevel>,
    pub max_size: u64,
    pub levels: Vec<DomColumnLevel>,
}

impl DomColumnSnapshot {
    pub fn from_book(book: &InstrumentOrderBook, depth_limit: usize) -> Self {
        let snapshot = book.snapshot();
        let mut levels_by_price: BTreeMap<MboPrice, (u64, u64)> = BTreeMap::new();

        for level in book.levels(MboSide::Bid, depth_limit) {
            levels_by_price
                .entry(level.price)
                .and_modify(|sizes| sizes.0 = level.size)
                .or_insert((level.size, 0));
        }
        for level in book.levels(MboSide::Ask, depth_limit) {
            levels_by_price
                .entry(level.price)
                .and_modify(|sizes| sizes.1 = level.size)
                .or_insert((0, level.size));
        }

        let mut levels = levels_by_price
            .into_iter()
            .rev()
            .map(|(price, (bid_size, ask_size))| {
                let total_size = bid_size + ask_size;
                DomColumnLevel {
                    price,
                    bid_size,
                    ask_size,
                    total_size,
                    side_hint: if bid_size >= ask_size && bid_size > 0 {
                        MboSide::Bid
                    } else if ask_size > 0 {
                        MboSide::Ask
                    } else {
                        MboSide::None
                    },
                }
            })
            .collect::<Vec<_>>();

        if levels.len() > depth_limit.saturating_mul(2) {
            levels.truncate(depth_limit.saturating_mul(2));
        }
        let max_size = levels
            .iter()
            .map(|level| level.total_size)
            .max()
            .unwrap_or_default();

        Self {
            instrument_id: snapshot.instrument_id,
            quality: snapshot.quality,
            best_bid: snapshot.top.bid,
            best_ask: snapshot.top.ask,
            max_size,
            levels,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::order_flow::{
        DomColumnSnapshot, FIXED_PRICE_SCALE, MboAction, MboEvent, MboPrice, MboSide,
        MboSourceInfo, OrderBookEngine,
    };

    fn source() -> MboSourceInfo {
        MboSourceInfo::new("GLBX.MDP3", "NQ.FUT", "parent", "instrument_id")
    }

    fn price(value: i64) -> MboPrice {
        MboPrice::from_fixed(value * FIXED_PRICE_SCALE)
    }

    fn event(order_id: u64, side: MboSide, price_value: i64, size: u32) -> MboEvent {
        MboEvent::book_event(
            source(),
            1,
            order_id,
            MboAction::Add,
            side,
            price(price_value),
            size,
        )
    }

    #[test]
    fn dom_snapshot_extracts_bounded_ladder_columns() {
        let mut engine = OrderBookEngine::new();
        for (order_id, side, price, size) in [
            (1, MboSide::Bid, 100, 3),
            (2, MboSide::Bid, 99, 5),
            (3, MboSide::Ask, 101, 7),
            (4, MboSide::Ask, 102, 11),
        ] {
            engine
                .apply(&event(order_id, side, price, size))
                .expect("apply");
        }

        let snapshot = DomColumnSnapshot::from_book(engine.book(1).expect("book"), 1);

        assert_eq!(snapshot.instrument_id, 1);
        assert_eq!(snapshot.best_bid.expect("bid").price, price(100));
        assert_eq!(snapshot.best_ask.expect("ask").price, price(101));
        assert_eq!(snapshot.levels.len(), 2);
        assert_eq!(snapshot.max_size, 7);
        assert_eq!(snapshot.levels[0].price, price(101));
        assert_eq!(snapshot.levels[0].ask_size, 7);
        assert_eq!(snapshot.levels[1].price, price(100));
        assert_eq!(snapshot.levels[1].bid_size, 3);
    }
}
