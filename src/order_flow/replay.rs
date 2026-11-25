use super::{book::OrderBookEngine, mbo::MboEvent};

#[derive(Debug, Clone)]
pub struct ReplayCheckpoint {
    pub event_index: u64,
    pub timestamp_ns: u64,
    pub engine: OrderBookEngine,
}

#[derive(Debug, Clone)]
pub struct ReplayCheckpointStore {
    checkpoint_every_events: u64,
    checkpoints: Vec<ReplayCheckpoint>,
}

impl ReplayCheckpointStore {
    pub fn new(checkpoint_every_events: u64) -> Self {
        assert!(
            checkpoint_every_events > 0,
            "checkpoint cadence must be non-zero"
        );
        Self {
            checkpoint_every_events,
            checkpoints: Vec::new(),
        }
    }

    pub const fn checkpoint_every_events(&self) -> u64 {
        self.checkpoint_every_events
    }

    pub fn maybe_record(&mut self, event_index: u64, event: &MboEvent, engine: &OrderBookEngine) {
        if event_index == 0 || !event_index.is_multiple_of(self.checkpoint_every_events) {
            return;
        }
        self.checkpoints.push(ReplayCheckpoint {
            event_index,
            timestamp_ns: event.ts_event,
            engine: engine.clone(),
        });
    }

    pub fn nearest_before_event(&self, event_index: u64) -> Option<&ReplayCheckpoint> {
        self.checkpoints
            .iter()
            .rev()
            .find(|checkpoint| checkpoint.event_index <= event_index)
    }

    pub fn nearest_before_time(&self, timestamp_ns: u64) -> Option<&ReplayCheckpoint> {
        self.checkpoints
            .iter()
            .rev()
            .find(|checkpoint| checkpoint.timestamp_ns <= timestamp_ns)
    }

    pub fn checkpoints(&self) -> &[ReplayCheckpoint] {
        &self.checkpoints
    }

    pub fn checkpoint_count(&self) -> usize {
        self.checkpoints.len()
    }

    pub fn take_checkpoints(&mut self) -> Vec<ReplayCheckpoint> {
        std::mem::take(&mut self.checkpoints)
    }
}

#[cfg(test)]
mod tests {
    use crate::order_flow::{
        FIXED_PRICE_SCALE, MboAction, MboEvent, MboPrice, MboSide, MboSourceInfo, OrderBookEngine,
        ReplayCheckpointStore,
    };

    fn source() -> MboSourceInfo {
        MboSourceInfo::new("GLBX.MDP3", "NQ.FUT", "parent", "instrument_id")
    }

    fn event(index: u64, order_id: u64, price: i64) -> MboEvent {
        let mut event = MboEvent::book_event(
            source(),
            1,
            order_id,
            MboAction::Add,
            MboSide::Bid,
            MboPrice::from_fixed(price * FIXED_PRICE_SCALE),
            1,
        );
        event.ts_event = index * 1_000;
        event
    }

    #[test]
    fn checkpoint_store_records_on_configured_event_cadence() {
        let mut engine = OrderBookEngine::new();
        let mut checkpoints = ReplayCheckpointStore::new(2);

        for index in 1..=5 {
            let event = event(index, index, 100 + index as i64);
            engine.apply(&event).expect("apply");
            checkpoints.maybe_record(index, &event, &engine);
        }

        assert_eq!(checkpoints.checkpoints().len(), 2);
        assert_eq!(checkpoints.checkpoints()[0].event_index, 2);
        assert_eq!(checkpoints.checkpoints()[1].event_index, 4);
        assert_eq!(
            checkpoints.checkpoints()[1]
                .engine
                .snapshot(1)
                .expect("snapshot")
                .order_count,
            4
        );
    }

    #[test]
    fn checkpoint_store_finds_nearest_before_event_or_time() {
        let mut engine = OrderBookEngine::new();
        let mut checkpoints = ReplayCheckpointStore::new(2);

        for index in 1..=4 {
            let event = event(index, index, 100 + index as i64);
            engine.apply(&event).expect("apply");
            checkpoints.maybe_record(index, &event, &engine);
        }

        assert_eq!(
            checkpoints
                .nearest_before_event(3)
                .expect("event checkpoint")
                .event_index,
            2
        );
        assert_eq!(
            checkpoints
                .nearest_before_time(4_500)
                .expect("time checkpoint")
                .event_index,
            4
        );
        assert!(checkpoints.nearest_before_time(1_500).is_none());
    }
}
