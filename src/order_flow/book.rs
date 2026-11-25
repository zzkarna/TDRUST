use std::collections::{BTreeMap, HashMap};

use super::mbo::{MboAction, MboEvent, MboFlags, MboPrice, MboSide};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookQuality {
    Clean,
    Suspect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookApplyError {
    InvalidBookSide(MboSide),
    InvalidBookPrice(MboPrice),
    MissingOrder(u64),
    DuplicateOrder(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookApplyReport {
    pub instrument_id: u32,
    pub action: MboAction,
    pub mutated_book: bool,
    pub quality: BookQuality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BookOrder {
    side: MboSide,
    price: MboPrice,
    size: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceLevel {
    pub price: MboPrice,
    pub size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TopOfBook {
    pub bid: Option<PriceLevel>,
    pub ask: Option<PriceLevel>,
    pub spread: Option<f64>,
    pub mid: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderBookSnapshot {
    pub instrument_id: u32,
    pub quality: BookQuality,
    pub order_count: usize,
    pub bid_levels: usize,
    pub ask_levels: usize,
    pub top: TopOfBook,
}

#[derive(Debug, Clone)]
pub struct InstrumentOrderBook {
    instrument_id: u32,
    orders: HashMap<u64, BookOrder>,
    bids: BTreeMap<MboPrice, u64>,
    asks: BTreeMap<MboPrice, u64>,
    quality: BookQuality,
    last_sequence: Option<u32>,
}

impl InstrumentOrderBook {
    pub fn new(instrument_id: u32) -> Self {
        Self {
            instrument_id,
            orders: HashMap::new(),
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            quality: BookQuality::Clean,
            last_sequence: None,
        }
    }

    pub fn apply(&mut self, event: &MboEvent) -> Result<BookApplyReport, BookApplyError> {
        self.observe_quality(event.flags, event.sequence);

        let mutated_book = match event.action {
            MboAction::Add => {
                self.add_order(event.order_id, event.side, event.price, event.size)?;
                true
            }
            MboAction::Modify => {
                self.modify_order(event.order_id, event.side, event.price, event.size)?;
                true
            }
            MboAction::Cancel => {
                self.cancel_order(event.order_id, event.size)?;
                true
            }
            MboAction::Clear => {
                self.clear();
                true
            }
            MboAction::Trade | MboAction::Fill | MboAction::None | MboAction::Unknown(_) => false,
        };

        Ok(BookApplyReport {
            instrument_id: self.instrument_id,
            action: event.action,
            mutated_book,
            quality: self.quality,
        })
    }

    pub fn snapshot(&self) -> OrderBookSnapshot {
        OrderBookSnapshot {
            instrument_id: self.instrument_id,
            quality: self.quality,
            order_count: self.orders.len(),
            bid_levels: self.bids.len(),
            ask_levels: self.asks.len(),
            top: self.top_of_book(),
        }
    }

    pub fn top_of_book(&self) -> TopOfBook {
        let bid = self
            .bids
            .iter()
            .next_back()
            .map(|(price, size)| PriceLevel {
                price: *price,
                size: *size,
            });
        let ask = self.asks.iter().next().map(|(price, size)| PriceLevel {
            price: *price,
            size: *size,
        });

        let spread = match (bid, ask) {
            (Some(bid), Some(ask)) => Some(
                (ask.price.fixed() - bid.price.fixed()) as f64
                    / super::mbo::FIXED_PRICE_SCALE as f64,
            ),
            _ => None,
        };
        let mid = match (bid, ask) {
            (Some(bid), Some(ask)) => Some(
                (ask.price.fixed() + bid.price.fixed()) as f64
                    / (2.0 * super::mbo::FIXED_PRICE_SCALE as f64),
            ),
            _ => None,
        };

        TopOfBook {
            bid,
            ask,
            spread,
            mid,
        }
    }

    pub fn depth_at(&self, side: MboSide, price: MboPrice) -> Option<u64> {
        match side {
            MboSide::Bid => self.bids.get(&price).copied(),
            MboSide::Ask => self.asks.get(&price).copied(),
            _ => None,
        }
    }

    pub fn levels(&self, side: MboSide, limit: usize) -> Vec<PriceLevel> {
        match side {
            MboSide::Bid => self
                .bids
                .iter()
                .rev()
                .take(limit)
                .map(|(price, size)| PriceLevel {
                    price: *price,
                    size: *size,
                })
                .collect(),
            MboSide::Ask => self
                .asks
                .iter()
                .take(limit)
                .map(|(price, size)| PriceLevel {
                    price: *price,
                    size: *size,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    fn add_order(
        &mut self,
        order_id: u64,
        side: MboSide,
        price: MboPrice,
        size: u32,
    ) -> Result<(), BookApplyError> {
        validate_book_order(side, price)?;
        if self.orders.contains_key(&order_id) {
            return Err(BookApplyError::DuplicateOrder(order_id));
        }

        self.orders
            .insert(order_id, BookOrder { side, price, size });
        self.add_level(side, price, u64::from(size));
        Ok(())
    }

    fn modify_order(
        &mut self,
        order_id: u64,
        side: MboSide,
        price: MboPrice,
        size: u32,
    ) -> Result<(), BookApplyError> {
        validate_book_order(side, price)?;
        let previous = self
            .orders
            .get(&order_id)
            .copied()
            .ok_or(BookApplyError::MissingOrder(order_id))?;

        self.remove_level(previous.side, previous.price, u64::from(previous.size));
        self.orders
            .insert(order_id, BookOrder { side, price, size });
        self.add_level(side, price, u64::from(size));
        Ok(())
    }

    fn cancel_order(&mut self, order_id: u64, size: u32) -> Result<(), BookApplyError> {
        let previous = self
            .orders
            .get(&order_id)
            .copied()
            .ok_or(BookApplyError::MissingOrder(order_id))?;
        let cancel_size = if size == 0 {
            previous.size
        } else {
            size.min(previous.size)
        };

        self.remove_level(previous.side, previous.price, u64::from(cancel_size));
        if cancel_size >= previous.size {
            self.orders.remove(&order_id);
        } else if let Some(order) = self.orders.get_mut(&order_id) {
            order.size -= cancel_size;
        }
        Ok(())
    }

    fn clear(&mut self) {
        self.orders.clear();
        self.bids.clear();
        self.asks.clear();
    }

    fn add_level(&mut self, side: MboSide, price: MboPrice, size: u64) {
        let levels = self.levels_mut(side);
        *levels.entry(price).or_insert(0) += size;
    }

    fn remove_level(&mut self, side: MboSide, price: MboPrice, size: u64) {
        let levels = self.levels_mut(side);
        if let Some(level_size) = levels.get_mut(&price) {
            *level_size = level_size.saturating_sub(size);
            if *level_size == 0 {
                levels.remove(&price);
            }
        }
    }

    fn levels_mut(&mut self, side: MboSide) -> &mut BTreeMap<MboPrice, u64> {
        match side {
            MboSide::Bid => &mut self.bids,
            MboSide::Ask => &mut self.asks,
            _ => unreachable!("book side already validated"),
        }
    }

    fn observe_quality(&mut self, flags: MboFlags, sequence: u32) {
        if flags.is_maybe_bad_book() {
            self.quality = BookQuality::Suspect;
        }
        if let Some(last_sequence) = self.last_sequence
            && sequence != 0
            && last_sequence != 0
            && sequence < last_sequence
        {
            self.quality = BookQuality::Suspect;
        }
        self.last_sequence = Some(sequence);
    }
}

#[derive(Debug, Default, Clone)]
pub struct OrderBookEngine {
    books: BTreeMap<u32, InstrumentOrderBook>,
}

impl OrderBookEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, event: &MboEvent) -> Result<BookApplyReport, BookApplyError> {
        self.books
            .entry(event.instrument_id)
            .or_insert_with(|| InstrumentOrderBook::new(event.instrument_id))
            .apply(event)
    }

    pub fn book(&self, instrument_id: u32) -> Option<&InstrumentOrderBook> {
        self.books.get(&instrument_id)
    }

    pub fn books(&self) -> impl Iterator<Item = &InstrumentOrderBook> {
        self.books.values()
    }

    pub fn snapshot(&self, instrument_id: u32) -> Option<OrderBookSnapshot> {
        self.book(instrument_id).map(InstrumentOrderBook::snapshot)
    }

    pub fn instrument_count(&self) -> usize {
        self.books.len()
    }
}

fn validate_book_order(side: MboSide, price: MboPrice) -> Result<(), BookApplyError> {
    if !side.is_book_side() {
        return Err(BookApplyError::InvalidBookSide(side));
    }
    if !price.is_defined() {
        return Err(BookApplyError::InvalidBookPrice(price));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{BookApplyError, BookQuality, OrderBookEngine};
    use crate::order_flow::{
        FIXED_PRICE_SCALE, MboAction, MboEvent, MboFlags, MboPrice, MboSide, MboSourceInfo,
    };

    fn source() -> MboSourceInfo {
        MboSourceInfo::new("GLBX.MDP3", "NQ.FUT", "parent", "instrument_id")
    }

    fn price(value: i64) -> MboPrice {
        MboPrice::from_fixed(value * FIXED_PRICE_SCALE)
    }

    fn event(
        instrument_id: u32,
        order_id: u64,
        action: MboAction,
        side: MboSide,
        price: MboPrice,
        size: u32,
    ) -> MboEvent {
        MboEvent::book_event(source(), instrument_id, order_id, action, side, price, size)
    }

    #[test]
    fn add_orders_builds_top_of_book() {
        let mut engine = OrderBookEngine::new();

        engine
            .apply(&event(10, 1, MboAction::Add, MboSide::Bid, price(100), 3))
            .expect("bid add");
        engine
            .apply(&event(10, 2, MboAction::Add, MboSide::Ask, price(102), 5))
            .expect("ask add");

        let snapshot = engine.snapshot(10).expect("snapshot");

        assert_eq!(snapshot.order_count, 2);
        assert_eq!(snapshot.top.bid.expect("bid").price, price(100));
        assert_eq!(snapshot.top.ask.expect("ask").price, price(102));
        assert_eq!(snapshot.top.spread, Some(2.0));
        assert_eq!(snapshot.top.mid, Some(101.0));
    }

    #[test]
    fn modify_moves_order_between_price_levels() {
        let mut engine = OrderBookEngine::new();

        engine
            .apply(&event(10, 1, MboAction::Add, MboSide::Bid, price(100), 3))
            .expect("add");
        engine
            .apply(&event(
                10,
                1,
                MboAction::Modify,
                MboSide::Bid,
                price(101),
                4,
            ))
            .expect("modify");

        let book = engine.book(10).expect("book");

        assert_eq!(book.depth_at(MboSide::Bid, price(100)), None);
        assert_eq!(book.depth_at(MboSide::Bid, price(101)), Some(4));
        assert_eq!(book.snapshot().order_count, 1);
    }

    #[test]
    fn levels_are_bounded_and_side_ordered() {
        let mut engine = OrderBookEngine::new();

        engine
            .apply(&event(10, 1, MboAction::Add, MboSide::Bid, price(99), 1))
            .expect("bid 99");
        engine
            .apply(&event(10, 2, MboAction::Add, MboSide::Bid, price(101), 2))
            .expect("bid 101");
        engine
            .apply(&event(10, 3, MboAction::Add, MboSide::Ask, price(103), 3))
            .expect("ask 103");
        engine
            .apply(&event(10, 4, MboAction::Add, MboSide::Ask, price(102), 4))
            .expect("ask 102");

        let book = engine.book(10).expect("book");
        let bids = book.levels(MboSide::Bid, 1);
        let asks = book.levels(MboSide::Ask, 2);

        assert_eq!(bids.len(), 1);
        assert_eq!(bids[0].price, price(101));
        assert_eq!(asks[0].price, price(102));
        assert_eq!(asks[1].price, price(103));
    }

    #[test]
    fn cancel_reduces_then_removes_order() {
        let mut engine = OrderBookEngine::new();

        engine
            .apply(&event(10, 1, MboAction::Add, MboSide::Ask, price(102), 5))
            .expect("add");
        engine
            .apply(&event(
                10,
                1,
                MboAction::Cancel,
                MboSide::Ask,
                price(102),
                2,
            ))
            .expect("partial cancel");

        assert_eq!(
            engine
                .book(10)
                .expect("book")
                .depth_at(MboSide::Ask, price(102)),
            Some(3)
        );

        engine
            .apply(&event(
                10,
                1,
                MboAction::Cancel,
                MboSide::Ask,
                price(102),
                3,
            ))
            .expect("full cancel");

        let snapshot = engine.snapshot(10).expect("snapshot");
        assert_eq!(snapshot.order_count, 0);
        assert_eq!(snapshot.ask_levels, 0);
    }

    #[test]
    fn clear_resets_only_one_instrument() {
        let mut engine = OrderBookEngine::new();

        engine
            .apply(&event(10, 1, MboAction::Add, MboSide::Bid, price(100), 1))
            .expect("add first");
        engine
            .apply(&event(11, 2, MboAction::Add, MboSide::Bid, price(200), 1))
            .expect("add second");
        engine
            .apply(&event(
                10,
                0,
                MboAction::Clear,
                MboSide::None,
                MboPrice::UNDEFINED,
                0,
            ))
            .expect("clear");

        assert_eq!(engine.snapshot(10).expect("first").order_count, 0);
        assert_eq!(engine.snapshot(11).expect("second").order_count, 1);
        assert_eq!(engine.instrument_count(), 2);
    }

    #[test]
    fn trade_and_fill_do_not_mutate_book() {
        let mut engine = OrderBookEngine::new();

        engine
            .apply(&event(10, 1, MboAction::Add, MboSide::Bid, price(100), 2))
            .expect("add");
        let trade_report = engine
            .apply(&event(10, 1, MboAction::Trade, MboSide::Ask, price(100), 1))
            .expect("trade");
        let fill_report = engine
            .apply(&event(10, 1, MboAction::Fill, MboSide::Bid, price(100), 1))
            .expect("fill");

        assert!(!trade_report.mutated_book);
        assert!(!fill_report.mutated_book);
        assert_eq!(engine.snapshot(10).expect("snapshot").order_count, 1);
    }

    #[test]
    fn invalid_add_side_is_rejected() {
        let mut engine = OrderBookEngine::new();
        let error = engine
            .apply(&event(10, 1, MboAction::Add, MboSide::None, price(100), 2))
            .expect_err("invalid side");

        assert_eq!(error, BookApplyError::InvalidBookSide(MboSide::None));
    }

    #[test]
    fn maybe_bad_book_flag_marks_quality_suspect() {
        let mut engine = OrderBookEngine::new();
        let mut add = event(10, 1, MboAction::Add, MboSide::Bid, price(100), 2);
        add.flags = MboFlags::new(MboFlags::MAYBE_BAD_BOOK);

        let report = engine.apply(&add).expect("apply");

        assert_eq!(report.quality, BookQuality::Suspect);
        assert_eq!(
            engine.snapshot(10).expect("snapshot").quality,
            BookQuality::Suspect
        );
    }
}
