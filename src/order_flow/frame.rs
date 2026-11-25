use super::{
    AddOnEventRecord, BboTransition, BboTransitionRecord, BookQuality, DomColumnLevel,
    DomColumnSnapshot, DomSnapshotRecord, FlowSummary, FootprintCell, FootprintCellKey,
    FootprintCellRecord, HeatmapCellRecord, LiquidityCell, LiquidityCellKey, MboPrice, MboSide,
    OrderFlowTimelineRunReport, OrderFlowTimelineSnapshot, PriceLevel, PriceLevelRecord,
    ReplayCheckpointRecord, VolumeBubble, VolumeBubbleKey, VolumeBubbleRecord,
    instrument::BookmapInstrumentSelection,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookmapFrameLimits {
    pub heatmap_cells: usize,
    pub volume_bubbles: usize,
    pub bbo_transitions: usize,
    pub footprint_cells: usize,
}

impl Default for BookmapFrameLimits {
    fn default() -> Self {
        Self {
            heatmap_cells: 2_400,
            volume_bubbles: 800,
            bbo_transitions: 800,
            footprint_cells: 12,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmapFrameSource {
    pub source_label: String,
    pub range_label: String,
    pub instrument_selection: Option<BookmapInstrumentSelection>,
    pub derived: bool,
    pub event_count: u64,
    pub instrument_count: usize,
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookmapPriceRange {
    pub min: MboPrice,
    pub max: MboPrice,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BookmapFrameViewport {
    pub instrument_id: Option<u32>,
    pub start_ts: u64,
    pub end_ts: u64,
    pub cursor_ts: u64,
    pub price_range: Option<BookmapPriceRange>,
    pub checkpoint: Option<ReplayCheckpointRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookmapFrameQuality {
    pub book_quality: Option<BookQuality>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmapUnsupportedAddOn {
    pub label: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BookmapLiquidityBand {
    pub instrument_id: u32,
    pub start_ts: u64,
    pub end_ts: u64,
    pub price: MboPrice,
    pub side: MboSide,
    pub displayed_size: u64,
    pub normalized_intensity: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BookmapFrameLayers {
    pub heatmap_cells: Vec<(LiquidityCellKey, LiquidityCell)>,
    pub heatmap_bands: Vec<BookmapLiquidityBand>,
    pub current_book_bands: Vec<BookmapLiquidityBand>,
    pub volume_bubbles: Vec<(VolumeBubbleKey, VolumeBubble)>,
    pub bbo_transitions: Vec<BboTransition>,
    pub dom_snapshot: Option<DomColumnSnapshot>,
    pub footprint_cells: Vec<(FootprintCellKey, FootprintCell)>,
    pub footprint_poc: Option<(FootprintCellKey, FootprintCell)>,
    pub flow_summary: FlowSummary,
    pub add_on_events: Vec<AddOnEventRecord>,
    pub unsupported_add_ons: Vec<BookmapUnsupportedAddOn>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BookmapFrame {
    pub source: BookmapFrameSource,
    pub viewport: BookmapFrameViewport,
    pub quality: BookmapFrameQuality,
    pub layers: BookmapFrameLayers,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BookmapFrameInput {
    pub source: BookmapFrameSource,
    pub instrument_id: Option<u32>,
    pub start_ts: u64,
    pub end_ts: u64,
    pub cursor_ts: u64,
    pub checkpoint: Option<ReplayCheckpointRecord>,
    pub book_quality: Option<BookQuality>,
    pub heatmap_cells: Vec<(LiquidityCellKey, LiquidityCell)>,
    pub volume_bubbles: Vec<(VolumeBubbleKey, VolumeBubble)>,
    pub bbo_transitions: Vec<BboTransition>,
    pub dom_snapshot: Option<DomColumnSnapshot>,
    pub footprint_cells: Vec<(FootprintCellKey, FootprintCell)>,
    pub footprint_poc: Option<(FootprintCellKey, FootprintCell)>,
    pub flow_summary: FlowSummary,
    pub add_on_events: Vec<AddOnEventRecord>,
    pub unsupported_add_ons: Vec<BookmapUnsupportedAddOn>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BookmapDerivedFrameInput {
    pub source: BookmapFrameSource,
    pub instrument_id: u32,
    pub start_ts: u64,
    pub end_ts: u64,
    pub cursor_ts: u64,
    pub checkpoint: Option<ReplayCheckpointRecord>,
    pub heatmap_records: Vec<HeatmapCellRecord>,
    pub volume_bubble_records: Vec<VolumeBubbleRecord>,
    pub bbo_records: Vec<BboTransitionRecord>,
    pub dom_snapshot: Option<DomSnapshotRecord>,
    pub footprint_records: Vec<FootprintCellRecord>,
    pub add_on_events: Vec<AddOnEventRecord>,
    pub unsupported_add_ons: Vec<BookmapUnsupportedAddOn>,
    pub limits: BookmapFrameLimits,
}

impl BookmapFrame {
    pub fn from_layers(input: BookmapFrameInput) -> Self {
        let heatmap_bands = heatmap_band_runs(&input.heatmap_cells, input.end_ts);
        let current_book_bands = current_book_continuation_bands(
            input.cursor_ts,
            input.end_ts,
            input.dom_snapshot.as_ref(),
        );
        let price_range = frame_price_range(
            &input.heatmap_cells,
            &input.volume_bubbles,
            &input.bbo_transitions,
            input.dom_snapshot.as_ref(),
            &input.footprint_cells,
            &input.add_on_events,
        );
        let book_quality = input
            .book_quality
            .or_else(|| input.dom_snapshot.as_ref().map(|dom| dom.quality));
        let instrument_id = input
            .instrument_id
            .or_else(|| {
                input
                    .source
                    .instrument_selection
                    .as_ref()
                    .and_then(|selection| selection.instrument_id)
            })
            .or_else(|| {
                input
                    .dom_snapshot
                    .as_ref()
                    .map(|dom| dom.instrument_id)
                    .or_else(|| {
                        input
                            .bbo_transitions
                            .last()
                            .map(|transition| transition.instrument_id)
                    })
                    .or_else(|| input.heatmap_cells.last().map(|(key, _)| key.instrument_id))
            });
        Self {
            source: input.source,
            viewport: BookmapFrameViewport {
                instrument_id,
                start_ts: input.start_ts,
                end_ts: input.end_ts,
                cursor_ts: input.cursor_ts,
                price_range,
                checkpoint: input.checkpoint,
            },
            quality: BookmapFrameQuality { book_quality },
            layers: BookmapFrameLayers {
                heatmap_cells: input.heatmap_cells,
                heatmap_bands,
                current_book_bands,
                volume_bubbles: input.volume_bubbles,
                bbo_transitions: input.bbo_transitions,
                dom_snapshot: input.dom_snapshot,
                footprint_cells: input.footprint_cells,
                footprint_poc: input.footprint_poc,
                flow_summary: input.flow_summary,
                add_on_events: input.add_on_events,
                unsupported_add_ons: input.unsupported_add_ons,
            },
        }
    }

    pub fn from_timeline_snapshot(
        source: BookmapFrameSource,
        snapshot: OrderFlowTimelineSnapshot,
        report: OrderFlowTimelineRunReport,
        limits: BookmapFrameLimits,
    ) -> Self {
        let mut heatmap_cells = snapshot.visible_liquidity_cells;
        retain_latest(&mut heatmap_cells, limits.heatmap_cells);

        let mut volume_bubbles = snapshot.volume_bubbles;
        retain_latest(&mut volume_bubbles, limits.volume_bubbles);

        let mut bbo_transitions = snapshot.bbo_transitions;
        retain_latest(&mut bbo_transitions, limits.bbo_transitions);

        let mut footprint_cells = snapshot
            .footprint_snapshot
            .cells
            .iter()
            .map(|(key, cell)| (*key, *cell))
            .collect::<Vec<_>>();
        sort_footprint_cells(&mut footprint_cells);
        retain_latest(&mut footprint_cells, limits.footprint_cells);
        let footprint_poc = snapshot.footprint_snapshot.point_of_control();

        let (start_ts, end_ts) = frame_time_range(
            &heatmap_cells,
            &volume_bubbles,
            &bbo_transitions,
            snapshot.last_event_ts,
        );
        let cursor_ts = snapshot.last_event_ts.unwrap_or(end_ts);
        let warning = source.warning.or(report.warning);
        let source = BookmapFrameSource { warning, ..source };

        Self::from_layers(BookmapFrameInput {
            source,
            instrument_id: snapshot.latest_instrument_id,
            start_ts,
            end_ts,
            cursor_ts,
            checkpoint: None,
            book_quality: snapshot.book_snapshot.as_ref().map(|book| book.quality),
            heatmap_cells,
            volume_bubbles,
            bbo_transitions,
            dom_snapshot: snapshot.dom_snapshot,
            footprint_cells,
            footprint_poc,
            flow_summary: snapshot.flow_summary,
            add_on_events: Vec::new(),
            unsupported_add_ons: default_unsupported_add_ons(),
        })
    }

    pub fn from_derived_records(input: BookmapDerivedFrameInput) -> Self {
        let mut heatmap_cells = input
            .heatmap_records
            .into_iter()
            .map(liquidity_cell_from_record)
            .collect::<Vec<_>>();
        retain_latest(&mut heatmap_cells, input.limits.heatmap_cells);

        let mut volume_bubbles = input
            .volume_bubble_records
            .into_iter()
            .map(volume_bubble_from_record)
            .collect::<Vec<_>>();
        retain_latest(&mut volume_bubbles, input.limits.volume_bubbles);

        let mut bbo_transitions = input
            .bbo_records
            .into_iter()
            .map(bbo_transition_from_record)
            .collect::<Vec<_>>();
        retain_latest(&mut bbo_transitions, input.limits.bbo_transitions);

        let mut footprint_cells = input
            .footprint_records
            .into_iter()
            .map(footprint_cell_from_record)
            .collect::<Vec<_>>();
        sort_footprint_cells(&mut footprint_cells);
        retain_latest(&mut footprint_cells, input.limits.footprint_cells);
        let footprint_poc = footprint_cells
            .iter()
            .copied()
            .max_by_key(|(_, cell)| cell.total_volume());

        let flow_summary = flow_summary_from_bubbles(&volume_bubbles);
        let dom_snapshot = input.dom_snapshot.map(dom_snapshot_from_record);

        Self::from_layers(BookmapFrameInput {
            source: input.source,
            instrument_id: Some(input.instrument_id),
            start_ts: input.start_ts,
            end_ts: input.end_ts,
            cursor_ts: input.cursor_ts,
            checkpoint: input.checkpoint,
            book_quality: dom_snapshot.as_ref().map(|dom| dom.quality),
            heatmap_cells,
            volume_bubbles,
            bbo_transitions,
            dom_snapshot,
            footprint_cells,
            footprint_poc,
            flow_summary,
            add_on_events: input.add_on_events,
            unsupported_add_ons: input.unsupported_add_ons,
        })
    }
}

pub fn default_unsupported_add_ons() -> Vec<BookmapUnsupportedAddOn> {
    vec![
        BookmapUnsupportedAddOn {
            label: "Proprietary Stops/Icebergs parity".to_string(),
            reason: "Bookmap detection internals are not public; RustChart output is heuristic"
                .to_string(),
        },
        BookmapUnsupportedAddOn {
            label: "External Market Pulse sources".to_string(),
            reason: "options, liquidation, and external-source feeds are not connected".to_string(),
        },
    ]
}

pub fn heatmap_band_runs(
    cells: &[(LiquidityCellKey, LiquidityCell)],
    fallback_end_ts: u64,
) -> Vec<BookmapLiquidityBand> {
    cells
        .iter()
        .map(|(key, cell)| BookmapLiquidityBand {
            instrument_id: key.instrument_id,
            start_ts: key.interval_start_ns,
            end_ts: next_heatmap_band_end_ts(cells, key, fallback_end_ts)
                .max(key.interval_start_ns),
            price: key.price,
            side: key.side,
            displayed_size: cell.displayed_size,
            normalized_intensity: normalized_band_intensity(cell.intensity),
        })
        .collect()
}

pub fn current_book_continuation_bands(
    cursor_ts: u64,
    end_ts: u64,
    dom: Option<&DomColumnSnapshot>,
) -> Vec<BookmapLiquidityBand> {
    let Some(dom) = dom else {
        return Vec::new();
    };
    let max_size = dom.max_size.max(1) as f32;
    dom.levels
        .iter()
        .filter(|level| level.total_size > 0)
        .map(|level| {
            let t = (level.total_size as f32 / max_size).sqrt().clamp(0.0, 1.0);
            BookmapLiquidityBand {
                instrument_id: dom.instrument_id,
                start_ts: cursor_ts,
                end_ts: end_ts.max(cursor_ts),
                price: level.price,
                side: level.side_hint,
                displayed_size: level.total_size,
                normalized_intensity: t,
            }
        })
        .collect()
}

fn next_heatmap_band_end_ts(
    cells: &[(LiquidityCellKey, LiquidityCell)],
    key: &LiquidityCellKey,
    fallback_end_ts: u64,
) -> u64 {
    cells
        .iter()
        .filter(|(candidate, _)| {
            candidate.instrument_id == key.instrument_id
                && candidate.price == key.price
                && candidate.side == key.side
                && candidate.interval_start_ns > key.interval_start_ns
        })
        .map(|(candidate, _)| candidate.interval_start_ns)
        .min()
        .unwrap_or(fallback_end_ts)
}

fn normalized_band_intensity(intensity: f32) -> f32 {
    intensity.clamp(0.0, 1.0).sqrt()
}

pub fn liquidity_cell_from_record(record: HeatmapCellRecord) -> (LiquidityCellKey, LiquidityCell) {
    (
        LiquidityCellKey {
            interval_start_ns: record.interval_start_ns,
            instrument_id: record.instrument_id,
            price: MboPrice::from_fixed(record.price_fixed),
            side: side_from_derived_label(&record.side),
        },
        LiquidityCell {
            displayed_size: record.displayed_size,
            max_displayed_size: record.max_displayed_size,
            samples: record.samples,
            intensity: record.intensity,
        },
    )
}

pub fn volume_bubble_from_record(record: VolumeBubbleRecord) -> (VolumeBubbleKey, VolumeBubble) {
    (
        VolumeBubbleKey {
            interval_start_ns: record.interval_start_ns,
            instrument_id: record.instrument_id,
            price: MboPrice::from_fixed(record.price_fixed),
        },
        VolumeBubble {
            buy_volume: record.buy_volume,
            sell_volume: record.sell_volume,
            unknown_volume: record.unknown_volume,
            trade_count: record.trade_count,
            fill_count: record.fill_count,
        },
    )
}

pub fn bbo_transition_from_record(record: BboTransitionRecord) -> BboTransition {
    BboTransition {
        ts_event: record.ts_event,
        instrument_id: record.instrument_id,
        bid: record.bid.map(price_level_from_record),
        ask: record.ask.map(price_level_from_record),
        spread: record.spread,
        mid: record.mid,
    }
}

pub fn footprint_cell_from_record(
    record: FootprintCellRecord,
) -> (FootprintCellKey, FootprintCell) {
    (
        FootprintCellKey {
            interval_start_ns: record.interval_start_ns,
            price: MboPrice::from_fixed(record.price_fixed),
        },
        FootprintCell {
            buy_volume: record.buy_volume,
            sell_volume: record.sell_volume,
            unknown_volume: record.unknown_volume,
        },
    )
}

pub fn dom_snapshot_from_record(record: DomSnapshotRecord) -> DomColumnSnapshot {
    let levels = record
        .levels
        .into_iter()
        .map(|level| DomColumnLevel {
            price: MboPrice::from_fixed(level.price_fixed),
            bid_size: level.bid_size,
            ask_size: level.ask_size,
            total_size: level.total_size,
            side_hint: side_from_derived_label(&level.side_hint),
        })
        .collect::<Vec<_>>();
    DomColumnSnapshot {
        instrument_id: record.instrument_id,
        quality: BookQuality::Clean,
        best_bid: record.best_bid.map(price_level_from_record),
        best_ask: record.best_ask.map(price_level_from_record),
        max_size: record.max_size,
        levels,
    }
}

pub fn price_level_from_record(record: PriceLevelRecord) -> PriceLevel {
    PriceLevel {
        price: MboPrice::from_fixed(record.price_fixed),
        size: record.size,
    }
}

pub fn side_from_derived_label(label: &str) -> MboSide {
    match label {
        "bid" => MboSide::Bid,
        "ask" => MboSide::Ask,
        "none" => MboSide::None,
        other => MboSide::Unknown(other.as_bytes().first().copied().unwrap_or_default()),
    }
}

fn retain_latest<T>(values: &mut Vec<T>, limit: usize) {
    if limit == 0 {
        values.clear();
    } else if values.len() > limit {
        values.drain(0..values.len() - limit);
    }
}

fn sort_footprint_cells(cells: &mut [(FootprintCellKey, FootprintCell)]) {
    cells.sort_by(|(_, left), (_, right)| {
        right
            .total_volume()
            .cmp(&left.total_volume())
            .then_with(|| right.delta().cmp(&left.delta()))
    });
}

fn flow_summary_from_bubbles(bubbles: &[(VolumeBubbleKey, VolumeBubble)]) -> FlowSummary {
    let mut summary = FlowSummary::default();
    for (_, bubble) in bubbles {
        summary.buy_volume += bubble.buy_volume;
        summary.sell_volume += bubble.sell_volume;
        summary.unknown_volume += bubble.unknown_volume;
        summary.trade_count += bubble.trade_count;
        summary.fill_count += bubble.fill_count;
    }
    summary.cumulative_delta = summary.buy_volume as i64 - summary.sell_volume as i64;
    summary
}

fn frame_time_range(
    heatmap_cells: &[(LiquidityCellKey, LiquidityCell)],
    volume_bubbles: &[(VolumeBubbleKey, VolumeBubble)],
    bbo_transitions: &[BboTransition],
    fallback_end_ts: Option<u64>,
) -> (u64, u64) {
    let mut min_ts = None::<u64>;
    let mut max_ts = None::<u64>;
    for (key, _) in heatmap_cells {
        observe_ts(&mut min_ts, &mut max_ts, key.interval_start_ns);
    }
    for (key, _) in volume_bubbles {
        observe_ts(&mut min_ts, &mut max_ts, key.interval_start_ns);
    }
    for transition in bbo_transitions {
        observe_ts(&mut min_ts, &mut max_ts, transition.ts_event);
    }
    if let Some(end_ts) = fallback_end_ts {
        observe_ts(&mut min_ts, &mut max_ts, end_ts);
    }
    let start = min_ts.unwrap_or_default();
    let end = max_ts.unwrap_or(start);
    (start, end.max(start))
}

fn observe_ts(min_ts: &mut Option<u64>, max_ts: &mut Option<u64>, value: u64) {
    *min_ts = Some(min_ts.map_or(value, |current| current.min(value)));
    *max_ts = Some(max_ts.map_or(value, |current| current.max(value)));
}

fn frame_price_range(
    heatmap_cells: &[(LiquidityCellKey, LiquidityCell)],
    volume_bubbles: &[(VolumeBubbleKey, VolumeBubble)],
    bbo_transitions: &[BboTransition],
    dom_snapshot: Option<&DomColumnSnapshot>,
    footprint_cells: &[(FootprintCellKey, FootprintCell)],
    add_on_events: &[AddOnEventRecord],
) -> Option<BookmapPriceRange> {
    let mut min_price = None::<i64>;
    let mut max_price = None::<i64>;
    for (key, _) in heatmap_cells {
        observe_price(&mut min_price, &mut max_price, key.price);
    }
    for (key, _) in volume_bubbles {
        observe_price(&mut min_price, &mut max_price, key.price);
    }
    for transition in bbo_transitions {
        if let Some(level) = transition.bid {
            observe_price(&mut min_price, &mut max_price, level.price);
        }
        if let Some(level) = transition.ask {
            observe_price(&mut min_price, &mut max_price, level.price);
        }
    }
    if let Some(dom) = dom_snapshot {
        for level in &dom.levels {
            observe_price(&mut min_price, &mut max_price, level.price);
        }
    }
    for (key, _) in footprint_cells {
        observe_price(&mut min_price, &mut max_price, key.price);
    }
    for event in add_on_events {
        if let Some(price) = event.price_fixed {
            observe_price(&mut min_price, &mut max_price, MboPrice::from_fixed(price));
        }
    }
    Some(BookmapPriceRange {
        min: MboPrice::from_fixed(min_price?),
        max: MboPrice::from_fixed(max_price?),
    })
}

fn observe_price(min_price: &mut Option<i64>, max_price: &mut Option<i64>, price: MboPrice) {
    if !price.is_defined() {
        return;
    }
    let fixed = price.fixed();
    *min_price = Some(min_price.map_or(fixed, |current| current.min(fixed)));
    *max_price = Some(max_price.map_or(fixed, |current| current.max(fixed)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order_flow::{FootprintSnapshot, OrderBookSnapshot, TopOfBook};

    fn price(value: i64) -> MboPrice {
        MboPrice::from_fixed(value)
    }

    fn source() -> BookmapFrameSource {
        BookmapFrameSource {
            source_label: "GLBX.MDP3 / NQ.FUT / mbo".to_string(),
            range_label: "test range".to_string(),
            instrument_selection: None,
            derived: false,
            event_count: 3,
            instrument_count: 1,
            warning: None,
        }
    }

    #[test]
    fn frame_prefers_explicit_source_selection_over_layer_inference() {
        let selected = BookmapInstrumentSelection {
            instrument_id: Some(3),
            available_instrument_ids: vec![3, 9],
            ..BookmapInstrumentSelection::from_cache_entry(
                &crate::order_flow::MboCacheEntry {
                    dataset: "GLBX.MDP3".to_string(),
                    schema: "mbo".to_string(),
                    symbol: "NQ.FUT".to_string(),
                    stype_in: "parent".to_string(),
                    stype_out: "instrument_id".to_string(),
                    start: None,
                    end_exclusive: None,
                    raw_path: std::path::PathBuf::from("sample.dbn.zst"),
                    estimated_record_count: None,
                    estimated_cost_usd: None,
                    estimated_billable_size_bytes: None,
                    bytes_on_disk: None,
                },
                "range",
            )
        };
        let frame = BookmapFrame::from_layers(BookmapFrameInput {
            source: BookmapFrameSource {
                instrument_selection: Some(selected),
                ..source()
            },
            instrument_id: None,
            start_ts: 10,
            end_ts: 20,
            cursor_ts: 20,
            checkpoint: None,
            book_quality: None,
            heatmap_cells: vec![(
                LiquidityCellKey {
                    interval_start_ns: 10,
                    instrument_id: 9,
                    price: price(100),
                    side: MboSide::Bid,
                },
                LiquidityCell {
                    displayed_size: 10,
                    max_displayed_size: 10,
                    samples: 1,
                    intensity: 1.0,
                },
            )],
            volume_bubbles: Vec::new(),
            bbo_transitions: Vec::new(),
            dom_snapshot: None,
            footprint_cells: Vec::new(),
            footprint_poc: None,
            flow_summary: FlowSummary::default(),
            add_on_events: Vec::new(),
            unsupported_add_ons: Vec::new(),
        });

        assert_eq!(frame.viewport.instrument_id, Some(3));
    }

    #[test]
    fn frame_from_timeline_keeps_layers_on_one_viewport() {
        let snapshot = OrderFlowTimelineSnapshot {
            event_count: 3,
            last_event_ts: Some(30),
            checkpoint_count: 0,
            instrument_count: 1,
            latest_instrument_id: Some(7),
            book_snapshot: Some(OrderBookSnapshot {
                instrument_id: 7,
                quality: BookQuality::Clean,
                order_count: 2,
                bid_levels: 1,
                ask_levels: 1,
                top: TopOfBook {
                    bid: Some(PriceLevel {
                        price: price(100),
                        size: 4,
                    }),
                    ask: Some(PriceLevel {
                        price: price(110),
                        size: 6,
                    }),
                    spread: Some(10.0),
                    mid: Some(105.0),
                },
            }),
            bid_levels: vec![],
            ask_levels: vec![],
            flow_summary: FlowSummary::default(),
            heatmap_frame_count: 1,
            depth_frames: vec![],
            latest_depth_levels: vec![],
            liquidity_cell_count: 1,
            visible_liquidity_cells: vec![(
                LiquidityCellKey {
                    interval_start_ns: 10,
                    instrument_id: 7,
                    price: price(100),
                    side: MboSide::Bid,
                },
                LiquidityCell {
                    displayed_size: 9,
                    max_displayed_size: 10,
                    samples: 1,
                    intensity: 0.9,
                },
            )],
            volume_bubbles: vec![(
                VolumeBubbleKey {
                    interval_start_ns: 20,
                    instrument_id: 7,
                    price: price(105),
                },
                VolumeBubble {
                    buy_volume: 5,
                    sell_volume: 2,
                    unknown_volume: 0,
                    trade_count: 2,
                    fill_count: 2,
                },
            )],
            bbo_transitions: vec![BboTransition {
                ts_event: 30,
                instrument_id: 7,
                bid: Some(PriceLevel {
                    price: price(100),
                    size: 4,
                }),
                ask: Some(PriceLevel {
                    price: price(110),
                    size: 6,
                }),
                spread: Some(10.0),
                mid: Some(105.0),
            }],
            dom_snapshot: None,
            footprint_snapshot: FootprintSnapshot {
                interval_ns: 1_000_000_000,
                cells: Default::default(),
            },
        };

        let frame = BookmapFrame::from_timeline_snapshot(
            source(),
            snapshot,
            OrderFlowTimelineRunReport::default(),
            BookmapFrameLimits::default(),
        );

        assert_eq!(frame.viewport.instrument_id, Some(7));
        assert_eq!(frame.viewport.start_ts, 10);
        assert_eq!(frame.viewport.end_ts, 30);
        assert_eq!(frame.viewport.cursor_ts, 30);
        assert_eq!(frame.layers.heatmap_cells.len(), 1);
        assert_eq!(frame.layers.heatmap_bands.len(), 1);
        assert!(frame.layers.current_book_bands.is_empty());
        assert_eq!(frame.layers.volume_bubbles.len(), 1);
        assert_eq!(frame.layers.bbo_transitions.len(), 1);
        assert_eq!(
            frame.viewport.price_range.expect("price range").min,
            price(100)
        );
        assert!(!frame.layers.unsupported_add_ons.is_empty());
    }

    #[test]
    fn frame_from_derived_records_converts_all_core_layers() {
        let frame = BookmapFrame::from_derived_records(BookmapDerivedFrameInput {
            source: BookmapFrameSource {
                derived: true,
                event_count: 100,
                ..source()
            },
            instrument_id: 9,
            start_ts: 100,
            end_ts: 200,
            cursor_ts: 175,
            checkpoint: Some(ReplayCheckpointRecord {
                event_index: 42,
                timestamp_ns: 170,
                instrument_count: 1,
            }),
            heatmap_records: vec![HeatmapCellRecord {
                interval_start_ns: 120,
                instrument_id: 9,
                price_fixed: 101,
                side: "bid".to_string(),
                displayed_size: 20,
                max_displayed_size: 30,
                samples: 2,
                intensity: 0.7,
            }],
            volume_bubble_records: vec![VolumeBubbleRecord {
                interval_start_ns: 130,
                instrument_id: 9,
                price_fixed: 102,
                buy_volume: 8,
                sell_volume: 3,
                unknown_volume: 1,
                total_volume: 12,
                delta: 5,
                trade_count: 4,
                fill_count: 4,
            }],
            bbo_records: vec![BboTransitionRecord {
                ts_event: 140,
                instrument_id: 9,
                bid: Some(PriceLevelRecord {
                    price_fixed: 100,
                    size: 10,
                }),
                ask: Some(PriceLevelRecord {
                    price_fixed: 103,
                    size: 11,
                }),
                spread: Some(3.0),
                mid: Some(101.5),
            }],
            dom_snapshot: Some(DomSnapshotRecord {
                timestamp_ns: Some(175),
                instrument_id: 9,
                best_bid: None,
                best_ask: None,
                max_size: 15,
                levels: vec![crate::order_flow::DomLevelRecord {
                    price_fixed: 100,
                    bid_size: 10,
                    ask_size: 0,
                    total_size: 10,
                    side_hint: "bid".to_string(),
                }],
            }),
            footprint_records: vec![FootprintCellRecord {
                interval_start_ns: 150,
                price_fixed: 102,
                buy_volume: 6,
                sell_volume: 2,
                unknown_volume: 0,
                total_volume: 8,
                delta: 4,
            }],
            add_on_events: vec![AddOnEventRecord {
                kind: "rustchart.stop_like".to_string(),
                timestamp_ns: 160,
                instrument_id: 9,
                price_fixed: Some(104),
                side: Some("buy".to_string()),
                confidence_bps: 7000,
                source: "fixture".to_string(),
                display_label: "Stop-like".to_string(),
                note: "fixture".to_string(),
            }],
            unsupported_add_ons: vec![BookmapUnsupportedAddOn {
                label: "External liquidation feed".to_string(),
                reason: "not connected".to_string(),
            }],
            limits: BookmapFrameLimits::default(),
        });

        assert!(frame.source.derived);
        assert_eq!(frame.viewport.instrument_id, Some(9));
        assert_eq!(
            frame.viewport.checkpoint.expect("checkpoint").event_index,
            42
        );
        assert_eq!(frame.layers.flow_summary.buy_volume, 8);
        assert_eq!(frame.layers.flow_summary.sell_volume, 3);
        assert_eq!(frame.layers.heatmap_bands.len(), 1);
        assert_eq!(frame.layers.current_book_bands.len(), 1);
        assert_eq!(frame.layers.current_book_bands[0].start_ts, 175);
        assert_eq!(frame.layers.current_book_bands[0].end_ts, 200);
        assert_eq!(frame.layers.dom_snapshot.expect("dom").levels.len(), 1);
        assert_eq!(frame.layers.add_on_events.len(), 1);
        assert_eq!(
            frame.layers.unsupported_add_ons[0].label,
            "External liquidation feed"
        );
        assert_eq!(
            frame.viewport.price_range.expect("price range").max,
            price(104)
        );
    }

    #[test]
    fn frame_heatmap_band_runs_extend_to_next_same_price_side_level() {
        let cells = vec![
            (
                LiquidityCellKey {
                    interval_start_ns: 10,
                    instrument_id: 9,
                    price: price(100),
                    side: MboSide::Bid,
                },
                LiquidityCell {
                    displayed_size: 20,
                    max_displayed_size: 40,
                    samples: 1,
                    intensity: 0.25,
                },
            ),
            (
                LiquidityCellKey {
                    interval_start_ns: 20,
                    instrument_id: 9,
                    price: price(101),
                    side: MboSide::Bid,
                },
                LiquidityCell {
                    displayed_size: 10,
                    max_displayed_size: 40,
                    samples: 1,
                    intensity: 1.0,
                },
            ),
            (
                LiquidityCellKey {
                    interval_start_ns: 40,
                    instrument_id: 9,
                    price: price(100),
                    side: MboSide::Bid,
                },
                LiquidityCell {
                    displayed_size: 0,
                    max_displayed_size: 40,
                    samples: 1,
                    intensity: 0.0,
                },
            ),
        ];

        let bands = heatmap_band_runs(&cells, 99);

        assert_eq!(bands.len(), 3);
        assert_eq!(bands[0].start_ts, 10);
        assert_eq!(bands[0].end_ts, 40);
        assert!((bands[0].normalized_intensity - 0.5).abs() < 0.001);
        assert_eq!(bands[1].end_ts, 99);
    }
}
