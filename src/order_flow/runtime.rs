use super::{
    bbo::BboPath,
    book::OrderBookEngine,
    derived_cache::MultiResolutionOhlcvCache,
    flow::{FlowSummary, TradeFlowAccumulator, VolumeBubbleAggregator},
    footprint::{FootprintAggregator, FootprintSnapshot},
    heatmap::{DepthTimeCache, LiquidityHeatmapSurface},
    mbo::MboEventSource,
    replay::ReplayCheckpointStore,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderFlowReplayConfig {
    pub checkpoint_every_events: u64,
    pub large_trade_threshold: u32,
    pub footprint_interval_ns: u64,
    pub heatmap_interval_ns: u64,
    pub heatmap_depth_limit: usize,
}

impl Default for OrderFlowReplayConfig {
    fn default() -> Self {
        Self {
            checkpoint_every_events: 10_000,
            large_trade_threshold: 25,
            footprint_interval_ns: 60_000_000_000,
            heatmap_interval_ns: 1_000_000_000,
            heatmap_depth_limit: 20,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OrderFlowReplayState {
    pub event_count: u64,
    pub first_event_ts: Option<u64>,
    pub last_event_ts: Option<u64>,
    pub engine: OrderBookEngine,
    pub checkpoints: ReplayCheckpointStore,
    pub flow: TradeFlowAccumulator,
    pub ohlcv: MultiResolutionOhlcvCache,
    pub footprint: FootprintAggregator,
    pub heatmap: DepthTimeCache,
    pub liquidity_surface: LiquidityHeatmapSurface,
    pub volume_bubbles: VolumeBubbleAggregator,
    pub bbo_path: BboPath,
}

impl OrderFlowReplayState {
    pub fn new(config: OrderFlowReplayConfig) -> Self {
        Self {
            event_count: 0,
            first_event_ts: None,
            last_event_ts: None,
            engine: OrderBookEngine::new(),
            checkpoints: ReplayCheckpointStore::new(config.checkpoint_every_events),
            flow: TradeFlowAccumulator::new(config.large_trade_threshold),
            ohlcv: MultiResolutionOhlcvCache::default(),
            footprint: FootprintAggregator::new(config.footprint_interval_ns),
            heatmap: DepthTimeCache::new(config.heatmap_interval_ns, config.heatmap_depth_limit),
            liquidity_surface: LiquidityHeatmapSurface::new(
                config.heatmap_interval_ns,
                config.heatmap_depth_limit,
            ),
            volume_bubbles: VolumeBubbleAggregator::new(config.footprint_interval_ns),
            bbo_path: BboPath::new(),
        }
    }

    pub fn flow_summary(&self) -> FlowSummary {
        self.flow.summary()
    }

    pub fn footprint_snapshot(&self) -> FootprintSnapshot {
        self.footprint.snapshot()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReplayBatchReport {
    pub read_events: usize,
    pub applied_book_events: usize,
    pub trade_prints: usize,
}

pub struct OrderFlowReplayRuntime<S> {
    source: S,
    state: OrderFlowReplayState,
}

impl<S> OrderFlowReplayRuntime<S>
where
    S: MboEventSource,
{
    pub fn new(source: S, config: OrderFlowReplayConfig) -> Self {
        Self {
            source,
            state: OrderFlowReplayState::new(config),
        }
    }

    pub fn state(&self) -> &OrderFlowReplayState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut OrderFlowReplayState {
        &mut self.state
    }

    pub fn process_next_batch(&mut self, max_events: usize) -> Result<ReplayBatchReport, String> {
        let events = self.source.next_batch(max_events)?;
        let mut report = ReplayBatchReport {
            read_events: events.len(),
            ..ReplayBatchReport::default()
        };

        for event in events {
            self.state.event_count += 1;
            if self.state.first_event_ts.is_none() {
                self.state.first_event_ts = Some(event.ts_event);
            }
            self.state.last_event_ts = Some(event.ts_event);

            if let Some(print) = self.state.flow.apply_event(&event) {
                report.trade_prints += 1;
                self.state.ohlcv.apply_print(&print);
                self.state.footprint.apply_print(&print);
                self.state.volume_bubbles.apply_print(&print);
            } else {
                self.state.footprint.apply_event(&event);
            }

            if event.action.mutates_book() {
                let apply_report = self
                    .state
                    .engine
                    .apply(&event)
                    .map_err(|error| format!("order book apply failed: {error:?}"))?;
                if apply_report.mutated_book {
                    report.applied_book_events += 1;
                    if let Some(book) = self.state.engine.book(event.instrument_id) {
                        self.state.heatmap.record_book(event.ts_event, book);
                        self.state
                            .liquidity_surface
                            .record_book(event.ts_event, book);
                        self.state.bbo_path.record_book(event.ts_event, book);
                    }
                }
            }

            self.state
                .checkpoints
                .maybe_record(self.state.event_count, &event, &self.state.engine);
        }

        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use crate::order_flow::{
        FIXED_PRICE_SCALE, MboAction, MboEvent, MboPrice, MboSide, MboSourceInfo,
        OrderFlowReplayConfig, OrderFlowReplayRuntime, VecMboEventSource,
    };

    fn source_info() -> MboSourceInfo {
        MboSourceInfo::new("GLBX.MDP3", "NQ.FUT", "parent", "instrument_id")
    }

    fn event(
        ts_event: u64,
        order_id: u64,
        action: MboAction,
        side: MboSide,
        price: i64,
        size: u32,
    ) -> MboEvent {
        let mut event = MboEvent::book_event(
            source_info(),
            1,
            order_id,
            action,
            side,
            MboPrice::from_fixed(price * FIXED_PRICE_SCALE),
            size,
        );
        event.ts_event = ts_event;
        event
    }

    #[test]
    fn replay_runtime_processes_source_into_book_flow_and_derived_state() {
        let events = vec![
            event(1, 1, MboAction::Add, MboSide::Bid, 100, 3),
            event(2, 2, MboAction::Add, MboSide::Ask, 102, 4),
            event(3, 2, MboAction::Trade, MboSide::Bid, 102, 5),
            event(4, 2, MboAction::Cancel, MboSide::Ask, 102, 4),
        ];
        let source = VecMboEventSource::new(source_info(), events);
        let config = OrderFlowReplayConfig {
            checkpoint_every_events: 2,
            large_trade_threshold: 5,
            footprint_interval_ns: 10,
            heatmap_interval_ns: 1,
            heatmap_depth_limit: 1,
        };
        let mut runtime = OrderFlowReplayRuntime::new(source, config);

        let report = runtime.process_next_batch(10).expect("process");
        let state = runtime.state();

        assert_eq!(report.read_events, 4);
        assert_eq!(report.applied_book_events, 3);
        assert_eq!(report.trade_prints, 1);
        assert_eq!(state.event_count, 4);
        assert_eq!(state.last_event_ts, Some(4));
        assert_eq!(state.flow_summary().cumulative_delta, 5);
        assert_eq!(state.flow_summary().large_trade_count, 1);
        assert_eq!(
            state
                .engine
                .snapshot(1)
                .expect("snapshot")
                .top
                .bid
                .expect("bid")
                .size,
            3
        );
        assert_eq!(state.footprint_snapshot().cells.len(), 1);
        assert_eq!(state.heatmap.frames().count(), 3);
        assert_eq!(state.liquidity_surface.cell_count(), 4);
        assert_eq!(
            state
                .volume_bubbles
                .visible_bubbles_for_instrument(1, 10)
                .len(),
            1
        );
        assert_eq!(state.bbo_path.visible_for_instrument(1, 10).len(), 3);
        assert_eq!(state.checkpoints.checkpoints().len(), 2);
    }
}
