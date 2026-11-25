use std::collections::BTreeMap;

use super::{
    book::{InstrumentOrderBook, PriceLevel},
    mbo::MboPrice,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BboTransition {
    pub ts_event: u64,
    pub instrument_id: u32,
    pub bid: Option<PriceLevel>,
    pub ask: Option<PriceLevel>,
    pub spread: Option<f64>,
    pub mid: Option<f64>,
}

impl BboTransition {
    pub fn bid_price(self) -> Option<MboPrice> {
        self.bid.map(|level| level.price)
    }

    pub fn ask_price(self) -> Option<MboPrice> {
        self.ask.map(|level| level.price)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BboPath {
    latest_by_instrument: BTreeMap<u32, BboTransition>,
    transitions: Vec<BboTransition>,
}

impl BboPath {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_book(
        &mut self,
        ts_event: u64,
        book: &InstrumentOrderBook,
    ) -> Option<BboTransition> {
        let snapshot = book.snapshot();
        let next = BboTransition {
            ts_event,
            instrument_id: snapshot.instrument_id,
            bid: snapshot.top.bid,
            ask: snapshot.top.ask,
            spread: snapshot.top.spread,
            mid: snapshot.top.mid,
        };

        if self
            .latest_by_instrument
            .get(&snapshot.instrument_id)
            .is_some_and(|previous| same_bbo(*previous, next))
        {
            return None;
        }

        self.latest_by_instrument
            .insert(snapshot.instrument_id, next);
        self.transitions.push(next);
        Some(next)
    }

    pub fn latest_for_instrument(&self, instrument_id: u32) -> Option<BboTransition> {
        self.latest_by_instrument.get(&instrument_id).copied()
    }

    pub fn transitions(&self) -> &[BboTransition] {
        &self.transitions
    }

    pub fn transition_count(&self) -> usize {
        self.transitions.len()
    }

    pub fn take_transitions(&mut self) -> Vec<BboTransition> {
        std::mem::take(&mut self.transitions)
    }

    pub fn visible_for_instrument(&self, instrument_id: u32, limit: usize) -> Vec<BboTransition> {
        let mut transitions = self
            .transitions
            .iter()
            .copied()
            .filter(|transition| transition.instrument_id == instrument_id)
            .collect::<Vec<_>>();
        transitions.sort_by_key(|transition| transition.ts_event);
        if transitions.len() > limit {
            transitions.drain(0..transitions.len() - limit);
        }
        transitions
    }
}

fn same_bbo(left: BboTransition, right: BboTransition) -> bool {
    left.bid == right.bid
        && left.ask == right.ask
        && left.spread == right.spread
        && left.mid == right.mid
}

#[cfg(test)]
mod tests {
    use crate::order_flow::{
        BboPath, FIXED_PRICE_SCALE, MboAction, MboEvent, MboPrice, MboSide, MboSourceInfo,
        OrderBookEngine,
    };

    fn source() -> MboSourceInfo {
        MboSourceInfo::new("GLBX.MDP3", "NQ.FUT", "parent", "instrument_id")
    }

    fn price(value: i64) -> MboPrice {
        MboPrice::from_fixed(value * FIXED_PRICE_SCALE)
    }

    fn event(
        order_id: u64,
        action: MboAction,
        side: MboSide,
        price_value: i64,
        size: u32,
    ) -> MboEvent {
        MboEvent::book_event(
            source(),
            1,
            order_id,
            action,
            side,
            price(price_value),
            size,
        )
    }

    #[test]
    fn bbo_path_records_top_changes_only() {
        let mut engine = OrderBookEngine::new();
        let mut path = BboPath::new();

        engine
            .apply(&event(1, MboAction::Add, MboSide::Bid, 100, 3))
            .expect("bid");
        assert!(path.record_book(1, engine.book(1).expect("book")).is_some());
        assert!(path.record_book(2, engine.book(1).expect("book")).is_none());

        engine
            .apply(&event(2, MboAction::Add, MboSide::Ask, 102, 4))
            .expect("ask");
        let transition = path
            .record_book(3, engine.book(1).expect("book"))
            .expect("new bbo");

        assert_eq!(transition.bid_price(), Some(price(100)));
        assert_eq!(transition.ask_price(), Some(price(102)));
        assert_eq!(transition.spread, Some(2.0));
        assert_eq!(transition.mid, Some(101.0));
        assert_eq!(path.transitions().len(), 2);
        assert_eq!(path.visible_for_instrument(1, 1).len(), 1);
    }

    #[test]
    fn bbo_path_records_empty_side_transitions() {
        let mut engine = OrderBookEngine::new();
        let mut path = BboPath::new();
        engine
            .apply(&event(1, MboAction::Add, MboSide::Ask, 102, 4))
            .expect("ask");
        path.record_book(1, engine.book(1).expect("book"));

        engine
            .apply(&event(1, MboAction::Cancel, MboSide::Ask, 102, 4))
            .expect("cancel");
        let transition = path
            .record_book(2, engine.book(1).expect("book"))
            .expect("empty ask transition");

        assert_eq!(transition.ask, None);
        assert_eq!(transition.spread, None);
        assert_eq!(path.transitions().len(), 2);
    }
}
