use super::{
    BboTransition, DomColumnSnapshot, LiquidityCell, LiquidityCellKey, PriceLevel, VolumeBubble,
    VolumeBubbleKey,
    book::OrderBookSnapshot,
    flow::FlowSummary,
    footprint::FootprintSnapshot,
    heatmap::{DepthFrame, DepthLevelSample},
    mbo::{MboEventSource, MboSide},
    runtime::{OrderFlowReplayConfig, OrderFlowReplayRuntime, ReplayBatchReport},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderFlowTimelineConfig {
    pub replay: OrderFlowReplayConfig,
    pub event_limit: usize,
    pub batch_size: usize,
    pub book_depth_limit: usize,
}

impl Default for OrderFlowTimelineConfig {
    fn default() -> Self {
        Self {
            replay: OrderFlowReplayConfig::default(),
            event_limit: 10_000,
            batch_size: 1_000,
            book_depth_limit: 8,
        }
    }
}

impl OrderFlowTimelineConfig {
    pub fn normalized(mut self) -> Self {
        self.event_limit = self.event_limit.max(1);
        self.batch_size = self.batch_size.max(1);
        self.book_depth_limit = self.book_depth_limit.max(1);
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OrderFlowTimelineRunReport {
    pub batches: usize,
    pub read_events: usize,
    pub applied_book_events: usize,
    pub trade_prints: usize,
    pub exhausted_source: bool,
    pub reached_event_limit: bool,
    pub warning: Option<String>,
}

impl OrderFlowTimelineRunReport {
    fn absorb_batch(&mut self, batch: ReplayBatchReport) {
        self.batches += 1;
        self.read_events += batch.read_events;
        self.applied_book_events += batch.applied_book_events;
        self.trade_prints += batch.trade_prints;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderFlowTimelineSnapshot {
    pub event_count: u64,
    pub last_event_ts: Option<u64>,
    pub checkpoint_count: usize,
    pub instrument_count: usize,
    pub latest_instrument_id: Option<u32>,
    pub book_snapshot: Option<OrderBookSnapshot>,
    pub bid_levels: Vec<PriceLevel>,
    pub ask_levels: Vec<PriceLevel>,
    pub flow_summary: FlowSummary,
    pub heatmap_frame_count: usize,
    pub depth_frames: Vec<DepthFrame>,
    pub latest_depth_levels: Vec<DepthLevelSample>,
    pub liquidity_cell_count: usize,
    pub visible_liquidity_cells: Vec<(LiquidityCellKey, LiquidityCell)>,
    pub volume_bubbles: Vec<(VolumeBubbleKey, VolumeBubble)>,
    pub bbo_transitions: Vec<BboTransition>,
    pub dom_snapshot: Option<DomColumnSnapshot>,
    pub footprint_snapshot: FootprintSnapshot,
}

pub struct OrderFlowTimeline<S>
where
    S: MboEventSource,
{
    runtime: OrderFlowReplayRuntime<S>,
    config: OrderFlowTimelineConfig,
}

impl<S> OrderFlowTimeline<S>
where
    S: MboEventSource,
{
    pub fn new(source: S, config: OrderFlowTimelineConfig) -> Self {
        let config = config.normalized();
        Self {
            runtime: OrderFlowReplayRuntime::new(source, config.replay),
            config,
        }
    }

    pub fn run_to_event_limit(&mut self) -> OrderFlowTimelineRunReport {
        let mut report = OrderFlowTimelineRunReport::default();

        while self.runtime.state().event_count < self.config.event_limit as u64 {
            let remaining = (self.config.event_limit as u64 - self.runtime.state().event_count)
                .min(usize::MAX as u64) as usize;
            let batch_limit = remaining.min(self.config.batch_size);
            match self.runtime.process_next_batch(batch_limit) {
                Ok(batch_report) if batch_report.read_events == 0 => {
                    report.exhausted_source = true;
                    break;
                }
                Ok(batch_report) => report.absorb_batch(batch_report),
                Err(error) => {
                    report.warning = Some(error);
                    break;
                }
            }
        }

        report.reached_event_limit =
            self.runtime.state().event_count >= self.config.event_limit as u64;
        report
    }

    pub fn snapshot(&self) -> OrderFlowTimelineSnapshot {
        let state = self.runtime.state();
        let latest_frame = state.heatmap.frames().last().cloned();
        let latest_instrument_id = latest_frame
            .as_ref()
            .map(|frame| frame.key.instrument_id)
            .or_else(|| {
                state
                    .bbo_path
                    .transitions()
                    .last()
                    .map(|transition| transition.instrument_id)
            });
        let book_snapshot = latest_instrument_id.and_then(|id| state.engine.snapshot(id));
        let (bid_levels, ask_levels) = latest_instrument_id
            .and_then(|id| state.engine.book(id))
            .map(|book| {
                (
                    book.levels(MboSide::Bid, self.config.book_depth_limit),
                    book.levels(MboSide::Ask, self.config.book_depth_limit),
                )
            })
            .unwrap_or_default();
        let visible_liquidity_cells = latest_instrument_id
            .map(|id| {
                state
                    .liquidity_surface
                    .visible_cells_for_instrument(id, self.config.event_limit)
            })
            .unwrap_or_default();
        let volume_bubbles = latest_instrument_id
            .map(|id| {
                state
                    .volume_bubbles
                    .visible_bubbles_for_instrument(id, self.config.event_limit)
            })
            .unwrap_or_default();
        let bbo_transitions = latest_instrument_id
            .map(|id| {
                state
                    .bbo_path
                    .visible_for_instrument(id, self.config.event_limit)
            })
            .unwrap_or_default();
        let dom_snapshot = latest_instrument_id
            .and_then(|id| state.engine.book(id))
            .map(|book| DomColumnSnapshot::from_book(book, self.config.book_depth_limit));

        OrderFlowTimelineSnapshot {
            event_count: state.event_count,
            last_event_ts: state.last_event_ts,
            checkpoint_count: state.checkpoints.checkpoints().len(),
            instrument_count: state.engine.instrument_count(),
            latest_instrument_id,
            book_snapshot,
            bid_levels,
            ask_levels,
            flow_summary: state.flow_summary(),
            heatmap_frame_count: state.heatmap.frames().count(),
            depth_frames: state.heatmap.frames().cloned().collect(),
            latest_depth_levels: latest_frame
                .map(|frame| frame.levels)
                .unwrap_or_else(Vec::new),
            liquidity_cell_count: state.liquidity_surface.cell_count(),
            visible_liquidity_cells,
            volume_bubbles,
            bbo_transitions,
            dom_snapshot,
            footprint_snapshot: state.footprint_snapshot(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::order_flow::{
        FIXED_PRICE_SCALE, MboAction, MboEvent, MboPrice, MboSide, MboSourceInfo,
        OrderFlowReplayConfig, OrderFlowTimeline, OrderFlowTimelineConfig, VecMboEventSource,
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

    fn config(event_limit: usize) -> OrderFlowTimelineConfig {
        OrderFlowTimelineConfig {
            replay: OrderFlowReplayConfig {
                checkpoint_every_events: 2,
                large_trade_threshold: 4,
                footprint_interval_ns: 10,
                heatmap_interval_ns: 1,
                heatmap_depth_limit: 2,
            },
            event_limit,
            batch_size: 2,
            book_depth_limit: 2,
        }
    }

    #[test]
    fn timeline_processes_source_into_bounded_snapshot() {
        let events = vec![
            event(1, 1, MboAction::Add, MboSide::Bid, 100, 3),
            event(2, 2, MboAction::Add, MboSide::Ask, 102, 4),
            event(3, 2, MboAction::Trade, MboSide::Bid, 102, 5),
            event(4, 3, MboAction::Add, MboSide::Bid, 99, 6),
        ];
        let source = VecMboEventSource::new(source_info(), events);
        let mut timeline = OrderFlowTimeline::new(source, config(3));

        let report = timeline.run_to_event_limit();
        let snapshot = timeline.snapshot();

        assert_eq!(report.batches, 2);
        assert_eq!(report.read_events, 3);
        assert_eq!(report.applied_book_events, 2);
        assert_eq!(report.trade_prints, 1);
        assert!(report.reached_event_limit);
        assert!(!report.exhausted_source);
        assert_eq!(snapshot.event_count, 3);
        assert_eq!(snapshot.checkpoint_count, 1);
        assert_eq!(snapshot.latest_instrument_id, Some(1));
        assert_eq!(snapshot.bid_levels.len(), 1);
        assert_eq!(snapshot.ask_levels.len(), 1);
        assert_eq!(snapshot.flow_summary.cumulative_delta, 5);
        assert_eq!(snapshot.heatmap_frame_count, 2);
        assert_eq!(snapshot.liquidity_cell_count, 3);
        assert!(!snapshot.visible_liquidity_cells.is_empty());
        assert_eq!(snapshot.volume_bubbles.len(), 1);
        assert_eq!(snapshot.bbo_transitions.len(), 2);
        assert!(snapshot.dom_snapshot.is_some());
        assert_eq!(snapshot.footprint_snapshot.cells.len(), 1);
    }

    #[test]
    fn timeline_reports_source_exhaustion_separately_from_event_limit() {
        let events = vec![event(1, 1, MboAction::Add, MboSide::Bid, 100, 3)];
        let source = VecMboEventSource::new(source_info(), events);
        let mut timeline = OrderFlowTimeline::new(source, config(10));

        let report = timeline.run_to_event_limit();
        let snapshot = timeline.snapshot();

        assert_eq!(report.read_events, 1);
        assert!(report.exhausted_source);
        assert!(!report.reached_event_limit);
        assert_eq!(snapshot.event_count, 1);
    }

    #[test]
    fn timeline_keeps_partial_state_and_warning_on_apply_error() {
        let events = vec![
            event(1, 1, MboAction::Add, MboSide::Bid, 100, 3),
            event(2, 99, MboAction::Cancel, MboSide::Bid, 100, 1),
        ];
        let source = VecMboEventSource::new(source_info(), events);
        let mut timeline = OrderFlowTimeline::new(source, config(10));

        let report = timeline.run_to_event_limit();
        let snapshot = timeline.snapshot();

        assert!(report.warning.is_some());
        assert_eq!(snapshot.event_count, 2);
        assert_eq!(snapshot.bid_levels.len(), 1);
    }
}
