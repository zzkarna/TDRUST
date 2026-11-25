use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    env,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::Duration,
};

use eframe::egui;

use crate::{
    order_flow::{
        AddOnEventRecord, ArtifactPriceTimeWindowQuery, ArtifactTimeWindowQuery,
        BOOKMAP_CANDLE_INTERVALS, BOOKMAP_HEATMAP_LODS, BboTransition, BboTransitionRecord,
        BookQuality, BookmapFrame, BookmapFrameInput, BookmapFrameSource,
        BookmapInstrumentSelection, BookmapLayerKind, BookmapLayerState, BookmapLiquidityBand,
        DepthFrame, DepthFrameKey, DepthLevelSample, DerivedCacheManifest, DerivedCachePlan,
        DomColumnLevel, DomColumnSnapshot, DomSnapshotQuery, DomSnapshotRecord, FIXED_PRICE_SCALE,
        FlowSummary, FootprintCell, FootprintCellKey, FootprintCellRecord, FootprintWindowQuery,
        HeatmapCellRecord, HeatmapViewportQuery, HistoricalDbnMboSource, LiquidityCell,
        LiquidityCellKey, MboCacheEntry, MboCacheRegistry, MboPrice, MboSide, OhlcvBarRecord,
        OhlcvWindowQuery, OrderBookSnapshot, OrderFlowReplayConfig, OrderFlowTimeline,
        OrderFlowTimelineConfig, OrderFlowTimelineRunReport, OrderFlowTimelineSnapshot, PriceLevel,
        PriceLevelRecord, ReplayCheckpointRecord, TopOfBook, VolumeBubble, VolumeBubbleKey,
        default_unsupported_add_ons, derived_cache_manifest_path_for_entry, heatmap_band_runs,
        query_add_on_events, query_bbo_transitions, query_bookmap_composite_activity_window,
        query_dom_snapshot_instrument_ids, query_dom_snapshots, query_footprint_cells,
        query_heatmap_cells, query_latest_dom_snapshot_instrument_id, query_ohlcv_bars,
        query_volume_bubble_activity_window, query_volume_bubbles, read_cache_artifact,
        replay_checkpoint_at_or_before_time,
    },
    tv_chart::{ChartTransform, LogicalRange, OhlcBar, PriceRange},
    widget_api::WidgetKind,
};

const DEFAULT_MANIFEST_DIR: &str = "E-mini/manifests";
const PREFERRED_DERIVED_CACHE_ROOT: &str = "E-mini/derived-compressed";
const DEFAULT_DERIVED_CACHE_ROOT: &str = "E-mini/derived";
const DERIVED_CACHE_ROOT_ENV: &str = "RUSTCHART_DERIVED_CACHE_ROOT";
const SECOND_NS: u64 = 1_000_000_000;
const DERIVED_INITIAL_VIEWPORT_NS: u64 = 30 * 60 * SECOND_NS;
const PREVIEW_EVENT_LIMIT: usize = 10_000;
const PREVIEW_BATCH_SIZE: usize = 1_000;
const BOOK_DEPTH_ROWS: usize = 8;
const BOOKMAP_FALLBACK_DOM_DEPTH_ROWS: usize = BOOK_DEPTH_ROWS * 3;
const BOOKMAP_DOM_TEXT_LEVEL_LIMIT: usize = BOOK_DEPTH_ROWS * 2;
const HEATMAP_DEPTH_ROWS: usize = 12;
const FOOTPRINT_ROWS: usize = 12;
const COMPOSITE_HEATMAP_CELLS: usize = 12_000;
const COMPOSITE_BUBBLES: usize = 800;
const COMPOSITE_BBO_POINTS: usize = 800;
const COMPOSITE_OHLCV_BARS: usize = 420;
const BOOKMAP_HEATMAP_TEXTURE_MAX_RECTS: usize = 26_000;
const COMPOSITE_SPREAD_MARKER_THRESHOLD: i64 = FIXED_PRICE_SCALE / 2;
const BOOKMAP_TERMINAL_MIN_VISIBLE_TICKS: i64 = 48;
const BOOKMAP_TERMINAL_MAX_VISIBLE_TICKS: i64 = 160;
const BOOKMAP_TERMINAL_PRICE_PADDING_TICKS: i64 = 16;
const BOOKMAP_CURRENT_PRICE_COMFORT_MIN: i64 = 34;
const BOOKMAP_CURRENT_PRICE_COMFORT_MAX: i64 = 66;
const BOOKMAP_PRICE_LADDER_LABEL_FONT_SIZE: f32 = 10.0;
const BOOKMAP_PRICE_LADDER_LABEL_MIN_GAP: f32 = 14.0;
const BOOKMAP_PRICE_LADDER_LABEL_EDGE_PAD: f32 = 7.0;
const DEFAULT_REPLAY_CURSOR_FRACTION: f32 = 0.82;
const BOOKMAP_VISIBLE_CURSOR_FRACTION: f32 = 0.78;
const REPLAY_SPEEDS: [f32; 4] = [0.25, 1.0, 2.0, 4.0];
const DEFAULT_REPLAY_WINDOW_INDEX: usize = 7;
const REPLAY_WINDOWS: [&str; 8] = [
    "None",
    "10 Seconds",
    "15 Seconds",
    "30 Seconds",
    "1 Minute",
    "2 Minutes",
    "5 Minutes",
    "30 Minutes",
];

#[derive(Debug, Default)]
pub struct OrderFlowPreviewService {
    preview: Option<OrderFlowPreview>,
    receiver: Option<Receiver<OrderFlowPreview>>,
    replay_window: Option<DerivedReplayWindowPreview>,
    replay_window_receiver: Option<Receiver<DerivedReplayWindowPreview>>,
    replay_window_request: Option<DerivedReplayWindowKey>,
    replay_controls: ReplayControlState,
    bookmap_layers: BookmapLayerState,
}

impl OrderFlowPreviewService {
    pub fn ensure_loading(&mut self) {
        self.poll_loader();
        self.poll_replay_window();
        if self.preview.is_none() && self.receiver.is_none() {
            self.start_load();
        }
    }

    fn reload(&mut self) {
        self.preview = None;
        self.receiver = None;
        self.replay_window = None;
        self.replay_window_receiver = None;
        self.replay_window_request = None;
        self.replay_controls = ReplayControlState::default();
        self.start_load();
    }

    fn is_loading(&self) -> bool {
        self.receiver.is_some()
    }

    fn poll_loader(&mut self) {
        let mut loaded = None;
        let mut disconnected = false;
        if let Some(receiver) = &self.receiver {
            match receiver.try_recv() {
                Ok(preview) => loaded = Some(preview),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => disconnected = true,
            }
        }

        if let Some(preview) = loaded {
            self.preview = Some(preview);
            self.receiver = None;
        } else if disconnected {
            self.preview = Some(OrderFlowPreview::Error(
                "MBO preview loader stopped before returning a result".to_string(),
            ));
            self.receiver = None;
        }
    }

    fn start_load(&mut self) {
        let (sender, receiver) = mpsc::channel();
        match thread::Builder::new()
            .name("order-flow-preview-loader".to_string())
            .spawn(move || {
                let _ = sender.send(load_default_preview());
            }) {
            Ok(_handle) => {
                self.receiver = Some(receiver);
            }
            Err(error) => {
                self.preview = Some(OrderFlowPreview::Error(format!(
                    "MBO preview loader could not be started: {error}"
                )));
                self.receiver = None;
            }
        }
    }

    fn poll_replay_window(&mut self) {
        let mut loaded = None;
        let mut disconnected = false;
        if let Some(receiver) = &self.replay_window_receiver {
            match receiver.try_recv() {
                Ok(preview) => loaded = Some(preview),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => disconnected = true,
            }
        }

        if let Some(preview) = loaded {
            self.replay_window = Some(preview);
            self.replay_window_receiver = None;
        } else if disconnected {
            if let Some(key) = self.replay_window_request {
                self.replay_window = Some(DerivedReplayWindowPreview::Error {
                    key,
                    message: "derived replay-window loader stopped before returning a result"
                        .to_string(),
                });
            }
            self.replay_window_receiver = None;
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ReplayControlState {
    cursor_fraction: f32,
    speed_index: usize,
    window_index: usize,
    playing: bool,
    follow: bool,
}

impl Default for ReplayControlState {
    fn default() -> Self {
        Self {
            cursor_fraction: DEFAULT_REPLAY_CURSOR_FRACTION,
            speed_index: 1,
            window_index: DEFAULT_REPLAY_WINDOW_INDEX,
            playing: false,
            follow: true,
        }
    }
}

#[derive(Debug, Clone)]
struct DerivedPreviewSource {
    root: PathBuf,
    manifest: DerivedCacheManifest,
    instrument_selection: BookmapInstrumentSelection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DerivedReplayWindowKey {
    start_ts: u64,
    end_ts: u64,
    cursor_ts: u64,
    checkpoint_event_index: Option<u64>,
}

#[derive(Debug, Clone)]
struct DerivedReplayWindowSnapshot {
    key: DerivedReplayWindowKey,
    checkpoint: Option<ReplayCheckpointRecord>,
    visible_liquidity_cells: Vec<(LiquidityCellKey, LiquidityCell)>,
    ohlcv_bars: Vec<OhlcvBarRecord>,
    volume_bubbles: Vec<(VolumeBubbleKey, VolumeBubble)>,
    bbo_transitions: Vec<BboTransition>,
    dom_snapshot: Option<DomColumnSnapshot>,
    top_footprint_cells: Vec<(FootprintCellKey, FootprintCell)>,
    footprint_poc: Option<(FootprintCellKey, FootprintCell)>,
    flow_summary: FlowSummary,
    add_on_events: Vec<AddOnEventRecord>,
}

#[derive(Debug, Clone)]
enum DerivedReplayWindowPreview {
    Ready(DerivedReplayWindowSnapshot),
    Error {
        key: DerivedReplayWindowKey,
        message: String,
    },
}

impl DerivedReplayWindowPreview {
    fn key(&self) -> DerivedReplayWindowKey {
        match self {
            Self::Ready(snapshot) => snapshot.key,
            Self::Error { key, .. } => *key,
        }
    }

    fn ready_for(&self, key: DerivedReplayWindowKey) -> Option<&DerivedReplayWindowSnapshot> {
        match self {
            Self::Ready(snapshot) if snapshot.key == key => Some(snapshot),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct HeuristicOrderFlowEvent {
    kind: &'static str,
    note: &'static str,
    interval_start_ns: u64,
    price: MboPrice,
    score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopsIcebergsBadgeShape {
    Circle,
    Square,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StopsIcebergsBadgeStyle {
    glyph: &'static str,
    shape: StopsIcebergsBadgeShape,
    color: egui::Color32,
}

#[derive(Debug, Clone, PartialEq)]
struct PulseMetric {
    label: &'static str,
    value: f32,
    detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PulseRowBias {
    Buy,
    Sell,
    Neutral,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq)]
struct PulseWidgetRow {
    label: String,
    value_label: String,
    value: Option<f32>,
    bias: PulseRowBias,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderStatus {
    Ready,
    Planned,
    Limited,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProviderCapability {
    label: &'static str,
    status: ProviderStatus,
    capabilities: &'static [&'static str],
    detail: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ReplayAuditSnapshot {
    min_ts: u64,
    max_ts: u64,
    cursor_ts: u64,
    window_start_ts: u64,
    window_end_ts: u64,
    checkpoint_event_index: Option<u64>,
    checkpoint_ts: Option<u64>,
    visible_cells: usize,
    total_cells: usize,
    visible_bubbles: usize,
    total_bubbles: usize,
    visible_bbo: usize,
    total_bbo: usize,
    dom_levels: usize,
    cursor_fraction: f32,
}

#[derive(Debug, Clone)]
enum OrderFlowPreview {
    Ready(OrderFlowPreviewSnapshot),
    Error(String),
}

#[derive(Debug, Clone)]
struct OrderFlowPreviewSnapshot {
    source_label: String,
    range_label: String,
    instrument_selection: BookmapInstrumentSelection,
    raw_path: PathBuf,
    derived_source: Option<DerivedPreviewSource>,
    report: OrderFlowTimelineRunReport,
    warning: Option<String>,
    event_count: u64,
    last_event_ts: Option<u64>,
    checkpoint_count: usize,
    replay_checkpoints: Vec<ReplayCheckpointRecord>,
    instrument_count: usize,
    book_snapshot: Option<OrderBookSnapshot>,
    bid_levels: Vec<PriceLevel>,
    ask_levels: Vec<PriceLevel>,
    flow_summary: FlowSummary,
    heatmap_frame_count: usize,
    depth_frames: Vec<DepthFrame>,
    latest_depth_levels: Vec<DepthLevelSample>,
    liquidity_cell_count: usize,
    visible_liquidity_cells: Vec<(LiquidityCellKey, LiquidityCell)>,
    ohlcv_bars: Vec<OhlcvBarRecord>,
    volume_bubbles: Vec<(VolumeBubbleKey, VolumeBubble)>,
    bbo_transitions: Vec<BboTransition>,
    dom_snapshot: Option<DomColumnSnapshot>,
    footprint_interval_ns: u64,
    footprint_cell_count: usize,
    footprint_poc: Option<(FootprintCellKey, FootprintCell)>,
    top_footprint_cells: Vec<(FootprintCellKey, FootprintCell)>,
    add_on_events: Vec<AddOnEventRecord>,
}

pub fn show_order_flow_widget(
    ui: &mut egui::Ui,
    kind: WidgetKind,
    service: &mut OrderFlowPreviewService,
) {
    debug_assert!(matches!(
        kind,
        WidgetKind::OrderBook
            | WidgetKind::LiquidityHeatmap
            | WidgetKind::Footprint
            | WidgetKind::BookmapOrderFlow
            | WidgetKind::NanosecondZoom
            | WidgetKind::RecordReplay
            | WidgetKind::DeveloperApi
            | WidgetKind::MarketDataConnections
            | WidgetKind::StopsIcebergs
            | WidgetKind::TradermapPro
            | WidgetKind::DomPro
            | WidgetKind::FilteredFlowMap
            | WidgetKind::HeuristicOrderFlowEvents
            | WidgetKind::MarketPulse
    ));

    service.ensure_loading();
    if !matches!(
        kind,
        WidgetKind::DeveloperApi | WidgetKind::MarketDataConnections
    ) {
        advance_replay_playback(ui, &mut service.replay_controls);
    }
    let mut reload_requested = false;
    egui::Frame::default()
        .fill(egui::Color32::from_rgb(13, 13, 13))
        .inner_margin(if kind == WidgetKind::BookmapOrderFlow {
            egui::Margin::same(0)
        } else {
            egui::Margin::same(10)
        })
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            match service.preview.as_ref() {
                Some(OrderFlowPreview::Ready(snapshot)) => {
                    if kind == WidgetKind::BookmapOrderFlow {
                        let audit = replay_audit(snapshot, &service.replay_controls);
                        let mut derived_window = None;
                        if let Some(audit) = &audit {
                            let replay_window_key = derived_replay_window_key(audit);
                            ensure_derived_replay_window_loading(
                                &mut service.replay_window,
                                &mut service.replay_window_receiver,
                                &mut service.replay_window_request,
                                snapshot.derived_source.clone(),
                                replay_window_key,
                            );
                            derived_window = service
                                .replay_window
                                .as_ref()
                                .and_then(|preview| preview.ready_for(replay_window_key));
                        }
                        show_bookmap_order_flow(
                            ui,
                            snapshot,
                            audit.as_ref(),
                            derived_window,
                            &mut service.replay_controls,
                            &mut service.bookmap_layers,
                        );
                        return;
                    }
                    show_order_flow_header(ui, kind, snapshot);
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(8.0);
                    let mut derived_window = None;
                    let audit = if !matches!(
                        kind,
                        WidgetKind::DeveloperApi | WidgetKind::MarketDataConnections
                    ) {
                        reload_requested |= show_replay_controls(ui, &mut service.replay_controls);
                        let audit = replay_audit(snapshot, &service.replay_controls);
                        if let Some(audit) = &audit {
                            let replay_window_key = derived_replay_window_key(audit);
                            ensure_derived_replay_window_loading(
                                &mut service.replay_window,
                                &mut service.replay_window_receiver,
                                &mut service.replay_window_request,
                                snapshot.derived_source.clone(),
                                replay_window_key,
                            );
                            let loaded_derived_window = service
                                .replay_window
                                .as_ref()
                                .and_then(|preview| preview.ready_for(replay_window_key));
                            derived_window = loaded_derived_window;
                            ui.add_space(8.0);
                            show_replay_audit(
                                ui,
                                snapshot,
                                audit,
                                loaded_derived_window,
                                service.replay_window.as_ref(),
                                service.replay_window_receiver.is_some(),
                            );
                            ui.add_space(8.0);
                        }
                        audit
                    } else {
                        None
                    };
                    match kind {
                        WidgetKind::OrderBook => {
                            show_order_book(ui, snapshot, audit.as_ref(), derived_window)
                        }
                        WidgetKind::LiquidityHeatmap => {
                            show_liquidity_heatmap(ui, snapshot, audit.as_ref(), derived_window)
                        }
                        WidgetKind::Footprint => {
                            show_footprint(ui, snapshot, audit.as_ref(), derived_window)
                        }
                        WidgetKind::BookmapOrderFlow => {
                            show_bookmap_order_flow(
                                ui,
                                snapshot,
                                audit.as_ref(),
                                derived_window,
                                &mut service.replay_controls,
                                &mut service.bookmap_layers,
                            );
                        }
                        WidgetKind::NanosecondZoom => {
                            show_nanosecond_zoom(ui, snapshot, audit.as_ref(), derived_window)
                        }
                        WidgetKind::RecordReplay => show_record_replay(
                            ui,
                            snapshot,
                            audit.as_ref(),
                            derived_window,
                            &service.replay_controls,
                        ),
                        WidgetKind::DeveloperApi => show_developer_api(ui, snapshot),
                        WidgetKind::MarketDataConnections => {
                            show_market_data_connections(ui, snapshot)
                        }
                        WidgetKind::StopsIcebergs => {
                            show_stops_icebergs(ui, snapshot, audit.as_ref(), derived_window)
                        }
                        WidgetKind::TradermapPro => {
                            show_tradermap_pro(ui, snapshot, audit.as_ref(), derived_window)
                        }
                        WidgetKind::DomPro => {
                            show_dom_pro(ui, snapshot, audit.as_ref(), derived_window)
                        }
                        WidgetKind::FilteredFlowMap => {
                            show_filtered_flow_map(ui, snapshot, audit.as_ref(), derived_window)
                        }
                        WidgetKind::HeuristicOrderFlowEvents => show_heuristic_order_flow_events(
                            ui,
                            snapshot,
                            audit.as_ref(),
                            derived_window,
                        ),
                        WidgetKind::MarketPulse => {
                            show_market_pulse(ui, snapshot, audit.as_ref(), derived_window)
                        }
                        _ => {}
                    }
                }
                Some(OrderFlowPreview::Error(error)) => show_order_flow_error(ui, kind, error),
                None if service.is_loading() => {
                    ui.ctx().request_repaint_after(Duration::from_millis(100));
                    show_order_flow_loading(ui, kind);
                }
                None => show_order_flow_error(ui, kind, "Order-flow preview is not loaded"),
            }
        });
    if reload_requested {
        service.reload();
    }
}

fn ensure_derived_replay_window_loading(
    replay_window: &mut Option<DerivedReplayWindowPreview>,
    replay_window_receiver: &mut Option<Receiver<DerivedReplayWindowPreview>>,
    replay_window_request: &mut Option<DerivedReplayWindowKey>,
    source: Option<DerivedPreviewSource>,
    key: DerivedReplayWindowKey,
) {
    let Some(source) = source else {
        *replay_window = None;
        *replay_window_receiver = None;
        *replay_window_request = None;
        return;
    };
    if replay_window
        .as_ref()
        .is_some_and(|preview| preview.key() == key)
        || replay_window_request
            .as_ref()
            .is_some_and(|request| *request == key)
    {
        return;
    }

    *replay_window = None;
    *replay_window_request = Some(key);
    let (sender, receiver) = mpsc::channel();
    match thread::Builder::new()
        .name("derived-replay-window-loader".to_string())
        .spawn(move || {
            let preview = match query_derived_replay_window(&source, key) {
                Ok(snapshot) => DerivedReplayWindowPreview::Ready(snapshot),
                Err(error) => DerivedReplayWindowPreview::Error {
                    key,
                    message: error,
                },
            };
            let _ = sender.send(preview);
        }) {
        Ok(_handle) => {
            *replay_window_receiver = Some(receiver);
        }
        Err(error) => {
            *replay_window = Some(DerivedReplayWindowPreview::Error {
                key,
                message: format!("derived replay-window loader could not be started: {error}"),
            });
            *replay_window_receiver = None;
        }
    }
}

fn load_default_preview() -> OrderFlowPreview {
    match load_preview_from_manifest_dir(Path::new(DEFAULT_MANIFEST_DIR)) {
        Ok(snapshot) => OrderFlowPreview::Ready(snapshot),
        Err(error) => OrderFlowPreview::Error(error),
    }
}

fn default_derived_cache_roots() -> Vec<PathBuf> {
    vec![
        PathBuf::from(PREFERRED_DERIVED_CACHE_ROOT),
        PathBuf::from(DEFAULT_DERIVED_CACHE_ROOT),
    ]
}

fn configured_derived_cache_roots() -> Vec<PathBuf> {
    if let Some(root) = env::var_os(DERIVED_CACHE_ROOT_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
    {
        return vec![root];
    }
    default_derived_cache_roots()
}

fn load_preview_from_manifest_dir(path: &Path) -> Result<OrderFlowPreviewSnapshot, String> {
    let derived_roots = configured_derived_cache_roots();
    load_preview_from_candidate_roots(path, &derived_roots)
}

#[cfg(test)]
fn load_preview_from_paths(
    manifest_dir: &Path,
    derived_root: &Path,
) -> Result<OrderFlowPreviewSnapshot, String> {
    load_preview_from_candidate_roots(manifest_dir, &[derived_root.to_path_buf()])
}

fn load_preview_from_candidate_roots(
    manifest_dir: &Path,
    derived_roots: &[PathBuf],
) -> Result<OrderFlowPreviewSnapshot, String> {
    let registry = MboCacheRegistry::from_manifest_dir(manifest_dir)
        .map_err(|error| format!("MBO cache registry could not be loaded: {error:?}"))?;
    let entry = choose_preview_entry(registry.entries()).ok_or_else(|| {
        "No local MBO cache entry with an existing raw DBN payload was found".to_string()
    })?;
    let mut warnings = Vec::new();
    for derived_root in derived_roots {
        match load_derived_preview_for_entry(entry, derived_root) {
            Ok(Some(snapshot)) => return Ok(snapshot),
            Ok(None) => {}
            Err(error) => warnings.push(format!("{}: {error}", derived_root.display())),
        }
    }
    let derived_warning = if warnings.is_empty() {
        None
    } else {
        Some(format!(
            "Derived cache unavailable ({}); using bounded DBN preview",
            warnings.join("; ")
        ))
    };
    load_bounded_preview_from_entry(entry, derived_warning)
}

fn load_bounded_preview_from_entry(
    entry: &MboCacheEntry,
    warning_prefix: Option<String>,
) -> Result<OrderFlowPreviewSnapshot, String> {
    let source = HistoricalDbnMboSource::from_cache_entry(entry)
        .map_err(|error| format!("Historical MBO source could not be opened: {error}"))?;
    let config = OrderFlowTimelineConfig {
        replay: OrderFlowReplayConfig {
            checkpoint_every_events: 1_000,
            heatmap_depth_limit: HEATMAP_DEPTH_ROWS,
            ..OrderFlowReplayConfig::default()
        },
        event_limit: PREVIEW_EVENT_LIMIT,
        batch_size: PREVIEW_BATCH_SIZE,
        book_depth_limit: BOOK_DEPTH_ROWS,
    };
    let mut timeline = OrderFlowTimeline::new(source, config);
    let report = timeline.run_to_event_limit();
    let snapshot = timeline.snapshot();

    let mut snapshot = OrderFlowPreviewSnapshot::from_timeline(entry, snapshot, report);
    if let Some(warning_prefix) = warning_prefix {
        snapshot.warning = Some(match snapshot.warning {
            Some(warning) => format!("{warning_prefix}; {warning}"),
            None => warning_prefix,
        });
    }
    Ok(snapshot)
}

fn choose_preview_entry(entries: &[MboCacheEntry]) -> Option<&MboCacheEntry> {
    let mut candidates = entries
        .iter()
        .filter(|entry| entry.raw_path.exists())
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| compare_preview_entry(left, right));
    candidates.into_iter().next()
}

fn compare_preview_entry(left: &MboCacheEntry, right: &MboCacheEntry) -> Ordering {
    let left_nq = left.symbol == "NQ.FUT";
    let right_nq = right.symbol == "NQ.FUT";
    right_nq
        .cmp(&left_nq)
        .then_with(|| {
            left.bytes_on_disk
                .unwrap_or(u64::MAX)
                .cmp(&right.bytes_on_disk.unwrap_or(u64::MAX))
        })
        .then_with(|| left.start.cmp(&right.start))
        .then_with(|| left.raw_path.cmp(&right.raw_path))
}

fn load_derived_preview_for_entry(
    entry: &MboCacheEntry,
    derived_root: &Path,
) -> Result<Option<OrderFlowPreviewSnapshot>, String> {
    let manifest_path = derived_cache_manifest_path_for_entry(derived_root, entry);
    if !manifest_path.exists() {
        return Ok(None);
    }
    let manifest = DerivedCacheManifest::read_from_path(&manifest_path)
        .map_err(|error| format!("derived manifest could not be read: {error:?}"))?;
    if manifest.source.source_key() != entry.source_key() {
        return Ok(None);
    }
    Ok(Some(OrderFlowPreviewSnapshot::from_derived_cache(
        entry,
        &manifest,
        derived_root,
    )?))
}

impl OrderFlowPreviewSnapshot {
    fn from_timeline(
        entry: &MboCacheEntry,
        snapshot: OrderFlowTimelineSnapshot,
        report: OrderFlowTimelineRunReport,
    ) -> Self {
        let warning = report.warning.clone();
        let range_label = format_cache_range(entry);
        let source_label = format!("{} / {} / {}", entry.dataset, entry.symbol, entry.schema);
        let instrument_selection =
            BookmapInstrumentSelection::from_cache_entry(entry, range_label.clone())
                .with_timeline_instrument(snapshot.latest_instrument_id);
        let footprint_snapshot = snapshot.footprint_snapshot.clone();
        let mut top_footprint_cells = footprint_snapshot
            .cells
            .iter()
            .map(|(key, cell)| (*key, *cell))
            .collect::<Vec<_>>();
        top_footprint_cells.sort_by(|(_, left), (_, right)| {
            right
                .total_volume()
                .cmp(&left.total_volume())
                .then_with(|| right.delta().cmp(&left.delta()))
        });
        top_footprint_cells.truncate(FOOTPRINT_ROWS);
        let mut visible_liquidity_cells = snapshot.visible_liquidity_cells;
        if visible_liquidity_cells.len() > COMPOSITE_HEATMAP_CELLS {
            visible_liquidity_cells
                .drain(0..visible_liquidity_cells.len() - COMPOSITE_HEATMAP_CELLS);
        }
        let mut volume_bubbles = snapshot.volume_bubbles;
        if volume_bubbles.len() > COMPOSITE_BUBBLES {
            volume_bubbles.drain(0..volume_bubbles.len() - COMPOSITE_BUBBLES);
        }
        let mut bbo_transitions = snapshot.bbo_transitions;
        if bbo_transitions.len() > COMPOSITE_BBO_POINTS {
            bbo_transitions.drain(0..bbo_transitions.len() - COMPOSITE_BBO_POINTS);
        }

        Self {
            source_label,
            range_label,
            instrument_selection,
            raw_path: entry.raw_path.clone(),
            derived_source: None,
            report,
            warning,
            event_count: snapshot.event_count,
            last_event_ts: snapshot.last_event_ts,
            checkpoint_count: snapshot.checkpoint_count,
            replay_checkpoints: Vec::new(),
            instrument_count: snapshot.instrument_count,
            book_snapshot: snapshot.book_snapshot,
            bid_levels: snapshot.bid_levels,
            ask_levels: snapshot.ask_levels,
            flow_summary: snapshot.flow_summary,
            heatmap_frame_count: snapshot.heatmap_frame_count,
            depth_frames: snapshot.depth_frames,
            latest_depth_levels: snapshot.latest_depth_levels,
            liquidity_cell_count: snapshot.liquidity_cell_count,
            visible_liquidity_cells,
            ohlcv_bars: Vec::new(),
            volume_bubbles,
            bbo_transitions,
            dom_snapshot: snapshot.dom_snapshot,
            footprint_interval_ns: footprint_snapshot.interval_ns,
            footprint_cell_count: footprint_snapshot.cells.len(),
            footprint_poc: footprint_snapshot.point_of_control(),
            top_footprint_cells,
            add_on_events: Vec::new(),
        }
    }

    fn from_derived_cache(
        entry: &MboCacheEntry,
        manifest: &DerivedCacheManifest,
        derived_root: &Path,
    ) -> Result<Self, String> {
        let range_label = format_cache_range(entry);
        let activity_window = initial_derived_activity_window(manifest, derived_root)?;
        let (start_ts, end_ts) = activity_window
            .map(|window| (window.start_ts, window.end_ts))
            .unwrap_or_else(|| initial_derived_viewport_window(manifest));
        let derived_catalog = derived_instrument_catalog(manifest, derived_root)?;
        let preferred_instrument_id = if let Some(window) = activity_window {
            Some(window.instrument_id)
        } else {
            preferred_derived_instrument_id(manifest, derived_root, end_ts)?
        };
        let instrument_selection =
            BookmapInstrumentSelection::from_cache_entry(entry, range_label.clone())
                .with_derived_catalog_preferred(derived_catalog, preferred_instrument_id);
        let instrument_id = instrument_selection.query_instrument_id()?;
        let heatmap_records = query_heatmap_cells(
            manifest,
            derived_root,
            HeatmapViewportQuery {
                instrument_id,
                start_ts,
                end_ts,
                min_price_fixed: None,
                max_price_fixed: None,
                limit: COMPOSITE_HEATMAP_CELLS,
            },
        )
        .map_err(|error| format!("heatmap artifact query failed: {error:?}"))?;
        let bubble_records = query_volume_bubbles(
            manifest,
            derived_root,
            ArtifactPriceTimeWindowQuery {
                instrument_id,
                start_ts,
                end_ts,
                min_price_fixed: None,
                max_price_fixed: None,
                limit: COMPOSITE_BUBBLES,
            },
        )
        .map_err(|error| format!("volume bubble artifact query failed: {error:?}"))?;
        let ohlcv_records = query_ohlcv_bars(
            manifest,
            derived_root,
            OhlcvWindowQuery {
                interval_ns: BOOKMAP_CANDLE_INTERVALS[0].interval_ns,
                instrument_id,
                start_ts,
                end_ts,
                limit: COMPOSITE_OHLCV_BARS,
            },
        )
        .map_err(|error| format!("OHLCV artifact query failed: {error:?}"))?;
        let bbo_records = query_bbo_transitions(
            manifest,
            derived_root,
            ArtifactTimeWindowQuery {
                instrument_id,
                start_ts,
                end_ts,
                limit: COMPOSITE_BBO_POINTS,
            },
        )
        .map_err(|error| format!("BBO artifact query failed: {error:?}"))?;
        let footprint_records = query_footprint_cells(
            manifest,
            derived_root,
            FootprintWindowQuery {
                start_ts,
                end_ts,
                min_price_fixed: None,
                max_price_fixed: None,
                limit: FOOTPRINT_ROWS,
            },
        )
        .map_err(|error| format!("footprint artifact query failed: {error:?}"))?;
        let dom_snapshot = query_dom_snapshots(
            manifest,
            derived_root,
            DomSnapshotQuery {
                instrument_id,
                at_or_before_ts: Some(end_ts),
                min_price_fixed: None,
                max_price_fixed: None,
                snapshot_limit: 1,
                level_limit: BOOK_DEPTH_ROWS * 2,
            },
        )
        .map_err(|error| format!("DOM artifact query failed: {error:?}"))?
        .into_iter()
        .last()
        .map(dom_snapshot_from_record);
        let visible_liquidity_cells = heatmap_records
            .into_iter()
            .map(liquidity_cell_from_record)
            .collect::<Vec<_>>();
        let volume_bubbles = bubble_records
            .into_iter()
            .map(volume_bubble_from_record)
            .collect::<Vec<_>>();
        let bbo_transitions = bbo_records
            .into_iter()
            .map(bbo_transition_from_record)
            .collect::<Vec<_>>();
        let mut top_footprint_cells = footprint_records
            .into_iter()
            .map(footprint_cell_from_record)
            .collect::<Vec<_>>();
        top_footprint_cells.sort_by(|(_, left), (_, right)| {
            right
                .total_volume()
                .cmp(&left.total_volume())
                .then_with(|| right.delta().cmp(&left.delta()))
        });
        top_footprint_cells.truncate(FOOTPRINT_ROWS);
        let footprint_poc = top_footprint_cells
            .iter()
            .copied()
            .max_by_key(|(_, cell)| cell.total_volume());
        let flow_summary = flow_summary_from_bubbles(&volume_bubbles);
        let add_on_events = query_add_on_events(
            manifest,
            derived_root,
            ArtifactTimeWindowQuery {
                instrument_id,
                start_ts,
                end_ts,
                limit: 512,
            },
        )
        .map_err(|error| format!("add-on event artifact query failed: {error:?}"))?;
        let (bid_levels, ask_levels, latest_depth_levels, depth_frames, book_snapshot) =
            depth_state_from_dom(dom_snapshot.as_ref(), instrument_id, end_ts);
        let replay_checkpoints =
            read_cache_artifact::<crate::order_flow::ReplayCheckpointArtifact>(
                manifest,
                derived_root,
                crate::order_flow::DerivedCacheArtifactKind::ReplayCheckpoints,
            )
            .map_err(|error| format!("replay checkpoint artifact query failed: {error:?}"))?
            .checkpoints;
        let checkpoint_count = replay_checkpoints.len();

        Ok(Self {
            source_label: format!(
                "{} / {} / {} / derived",
                entry.dataset, entry.symbol, entry.schema
            ),
            range_label,
            instrument_selection: instrument_selection.clone(),
            raw_path: entry.raw_path.clone(),
            derived_source: Some(DerivedPreviewSource {
                root: derived_root.to_path_buf(),
                manifest: manifest.clone(),
                instrument_selection,
            }),
            report: OrderFlowTimelineRunReport {
                batches: manifest.plan.event_partitions as usize,
                read_events: manifest.summary.events_read as usize,
                applied_book_events: manifest.summary.applied_book_events as usize,
                trade_prints: manifest.summary.trade_prints as usize,
                exhausted_source: true,
                reached_event_limit: false,
                warning: None,
            },
            warning: None,
            event_count: manifest.summary.events_read,
            last_event_ts: Some(end_ts),
            checkpoint_count,
            replay_checkpoints,
            instrument_count: manifest.summary.instrument_count,
            book_snapshot,
            bid_levels,
            ask_levels,
            flow_summary,
            heatmap_frame_count: visible_liquidity_cells
                .iter()
                .map(|(key, _)| (key.interval_start_ns, key.instrument_id))
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            depth_frames,
            latest_depth_levels,
            liquidity_cell_count: manifest.summary.heatmap_cells,
            visible_liquidity_cells,
            ohlcv_bars: ohlcv_records,
            volume_bubbles,
            bbo_transitions,
            dom_snapshot,
            footprint_interval_ns: OrderFlowReplayConfig::default().footprint_interval_ns,
            footprint_cell_count: manifest.summary.footprint_cells,
            footprint_poc,
            top_footprint_cells,
            add_on_events,
        })
    }
}

fn query_derived_replay_window(
    source: &DerivedPreviewSource,
    key: DerivedReplayWindowKey,
) -> Result<DerivedReplayWindowSnapshot, String> {
    let instrument_id = source.instrument_selection.query_instrument_id()?;
    let heatmap_records = query_heatmap_cells(
        &source.manifest,
        &source.root,
        HeatmapViewportQuery {
            instrument_id,
            start_ts: key.start_ts,
            end_ts: key.end_ts,
            min_price_fixed: None,
            max_price_fixed: None,
            limit: COMPOSITE_HEATMAP_CELLS,
        },
    )
    .map_err(|error| format!("derived replay heatmap query failed: {error:?}"))?;
    let bubble_records = query_volume_bubbles(
        &source.manifest,
        &source.root,
        ArtifactPriceTimeWindowQuery {
            instrument_id,
            start_ts: key.start_ts,
            end_ts: key.end_ts,
            min_price_fixed: None,
            max_price_fixed: None,
            limit: COMPOSITE_BUBBLES,
        },
    )
    .map_err(|error| format!("derived replay bubble query failed: {error:?}"))?;
    let ohlcv_records = query_ohlcv_bars(
        &source.manifest,
        &source.root,
        OhlcvWindowQuery {
            interval_ns: BOOKMAP_CANDLE_INTERVALS[0].interval_ns,
            instrument_id,
            start_ts: key.start_ts,
            end_ts: key.end_ts,
            limit: COMPOSITE_OHLCV_BARS,
        },
    )
    .map_err(|error| format!("derived replay OHLCV query failed: {error:?}"))?;
    let bbo_records = query_bbo_transitions(
        &source.manifest,
        &source.root,
        ArtifactTimeWindowQuery {
            instrument_id,
            start_ts: key.start_ts,
            end_ts: key.end_ts,
            limit: COMPOSITE_BBO_POINTS,
        },
    )
    .map_err(|error| format!("derived replay BBO query failed: {error:?}"))?;
    let footprint_records = query_footprint_cells(
        &source.manifest,
        &source.root,
        FootprintWindowQuery {
            start_ts: key.start_ts,
            end_ts: key.end_ts,
            min_price_fixed: None,
            max_price_fixed: None,
            limit: FOOTPRINT_ROWS,
        },
    )
    .map_err(|error| format!("derived replay footprint query failed: {error:?}"))?;
    let dom_snapshot = query_dom_snapshots(
        &source.manifest,
        &source.root,
        DomSnapshotQuery {
            instrument_id,
            at_or_before_ts: Some(key.cursor_ts),
            min_price_fixed: None,
            max_price_fixed: None,
            snapshot_limit: 1,
            level_limit: BOOK_DEPTH_ROWS * 2,
        },
    )
    .map_err(|error| format!("derived replay DOM query failed: {error:?}"))?
    .into_iter()
    .last()
    .map(dom_snapshot_from_record);
    let checkpoint =
        replay_checkpoint_at_or_before_time(&source.manifest, &source.root, key.cursor_ts)
            .map_err(|error| format!("derived replay checkpoint query failed: {error:?}"))?;
    let visible_liquidity_cells = heatmap_records
        .into_iter()
        .map(liquidity_cell_from_record)
        .collect::<Vec<_>>();
    let volume_bubbles = bubble_records
        .into_iter()
        .map(volume_bubble_from_record)
        .collect::<Vec<_>>();
    let bbo_transitions = bbo_records
        .into_iter()
        .map(bbo_transition_from_record)
        .collect::<Vec<_>>();
    let mut top_footprint_cells = footprint_records
        .into_iter()
        .map(footprint_cell_from_record)
        .collect::<Vec<_>>();
    top_footprint_cells.sort_by(|(_, left), (_, right)| {
        right
            .total_volume()
            .cmp(&left.total_volume())
            .then_with(|| right.delta().cmp(&left.delta()))
    });
    top_footprint_cells.truncate(FOOTPRINT_ROWS);
    let footprint_poc = top_footprint_cells
        .iter()
        .copied()
        .max_by_key(|(_, cell)| cell.total_volume());
    let flow_summary = flow_summary_from_bubbles(&volume_bubbles);
    let add_on_events = query_add_on_events(
        &source.manifest,
        &source.root,
        ArtifactTimeWindowQuery {
            instrument_id,
            start_ts: key.start_ts,
            end_ts: key.end_ts,
            limit: 256,
        },
    )
    .map_err(|error| format!("derived replay add-on event query failed: {error:?}"))?;

    Ok(DerivedReplayWindowSnapshot {
        key,
        checkpoint,
        visible_liquidity_cells,
        ohlcv_bars: ohlcv_records,
        volume_bubbles,
        bbo_transitions,
        dom_snapshot,
        top_footprint_cells,
        footprint_poc,
        flow_summary,
        add_on_events,
    })
}

fn derived_instrument_catalog(
    manifest: &DerivedCacheManifest,
    derived_root: &Path,
) -> Result<Vec<u32>, String> {
    query_dom_snapshot_instrument_ids(manifest, derived_root)
        .map_err(|error| format!("DOM instrument catalog could not be read: {error:?}"))
}

fn preferred_derived_instrument_id(
    manifest: &DerivedCacheManifest,
    derived_root: &Path,
    end_ts: u64,
) -> Result<Option<u32>, String> {
    query_latest_dom_snapshot_instrument_id(manifest, derived_root, Some(end_ts))
        .map_err(|error| format!("latest DOM instrument could not be read: {error:?}"))
}

fn initial_derived_activity_window(
    manifest: &DerivedCacheManifest,
    derived_root: &Path,
) -> Result<Option<crate::order_flow::DerivedActivityWindow>, String> {
    query_bookmap_composite_activity_window(manifest, derived_root, DERIVED_INITIAL_VIEWPORT_NS)
        .or_else(|_| {
            query_volume_bubble_activity_window(manifest, derived_root, DERIVED_INITIAL_VIEWPORT_NS)
        })
        .map_err(|error| format!("volume activity window could not be read: {error:?}"))
}

fn initial_derived_viewport_window(manifest: &DerivedCacheManifest) -> (u64, u64) {
    let end_ts = manifest
        .summary
        .last_event_ts
        .or(manifest.summary.first_event_ts)
        .unwrap_or_default();
    let earliest_artifact_ts = manifest
        .summary
        .first_event_ts
        .unwrap_or_default()
        .saturating_sub(SECOND_NS);
    let start_ts = end_ts
        .saturating_sub(DERIVED_INITIAL_VIEWPORT_NS)
        .max(earliest_artifact_ts);
    (start_ts, end_ts)
}

fn liquidity_cell_from_record(record: HeatmapCellRecord) -> (LiquidityCellKey, LiquidityCell) {
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

fn volume_bubble_from_record(
    record: crate::order_flow::VolumeBubbleRecord,
) -> (VolumeBubbleKey, VolumeBubble) {
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

fn bbo_transition_from_record(record: BboTransitionRecord) -> BboTransition {
    BboTransition {
        ts_event: record.ts_event,
        instrument_id: record.instrument_id,
        bid: record.bid.map(price_level_from_record),
        ask: record.ask.map(price_level_from_record),
        spread: record.spread,
        mid: record.mid,
    }
}

fn footprint_cell_from_record(record: FootprintCellRecord) -> (FootprintCellKey, FootprintCell) {
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

fn dom_snapshot_from_record(record: DomSnapshotRecord) -> DomColumnSnapshot {
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

fn price_level_from_record(record: PriceLevelRecord) -> PriceLevel {
    PriceLevel {
        price: MboPrice::from_fixed(record.price_fixed),
        size: record.size,
    }
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

fn depth_state_from_dom(
    dom: Option<&DomColumnSnapshot>,
    instrument_id: u32,
    timestamp_ns: u64,
) -> (
    Vec<PriceLevel>,
    Vec<PriceLevel>,
    Vec<DepthLevelSample>,
    Vec<DepthFrame>,
    Option<OrderBookSnapshot>,
) {
    let Some(dom) = dom else {
        return (Vec::new(), Vec::new(), Vec::new(), Vec::new(), None);
    };
    let bid_levels = dom
        .levels
        .iter()
        .filter(|level| level.bid_size > 0)
        .map(|level| PriceLevel {
            price: level.price,
            size: level.bid_size,
        })
        .collect::<Vec<_>>();
    let ask_levels = dom
        .levels
        .iter()
        .filter(|level| level.ask_size > 0)
        .map(|level| PriceLevel {
            price: level.price,
            size: level.ask_size,
        })
        .collect::<Vec<_>>();
    let mut latest_depth_levels = Vec::with_capacity(bid_levels.len() + ask_levels.len());
    latest_depth_levels.extend(bid_levels.iter().map(|level| DepthLevelSample {
        side: MboSide::Bid,
        price: level.price,
        size: level.size,
    }));
    latest_depth_levels.extend(ask_levels.iter().map(|level| DepthLevelSample {
        side: MboSide::Ask,
        price: level.price,
        size: level.size,
    }));
    let depth_frames = if latest_depth_levels.is_empty() {
        Vec::new()
    } else {
        vec![DepthFrame {
            key: DepthFrameKey {
                interval_start_ns: timestamp_ns,
                instrument_id,
            },
            levels: latest_depth_levels.clone(),
        }]
    };
    let book_snapshot = Some(OrderBookSnapshot {
        instrument_id,
        quality: BookQuality::Clean,
        order_count: 0,
        bid_levels: bid_levels.len(),
        ask_levels: ask_levels.len(),
        top: TopOfBook {
            bid: dom.best_bid,
            ask: dom.best_ask,
            spread: match (dom.best_bid, dom.best_ask) {
                (Some(bid), Some(ask)) => {
                    Some((ask.price.fixed() - bid.price.fixed()) as f64 / FIXED_PRICE_SCALE as f64)
                }
                _ => None,
            },
            mid: match (dom.best_bid, dom.best_ask) {
                (Some(bid), Some(ask)) => Some(
                    (ask.price.fixed() + bid.price.fixed()) as f64
                        / (2.0 * FIXED_PRICE_SCALE as f64),
                ),
                _ => None,
            },
        },
    });
    (
        bid_levels,
        ask_levels,
        latest_depth_levels,
        depth_frames,
        book_snapshot,
    )
}

fn side_from_derived_label(label: &str) -> MboSide {
    match label {
        "bid" | "BID" => MboSide::Bid,
        "ask" | "ASK" => MboSide::Ask,
        "none" | "--" => MboSide::None,
        _ => MboSide::None,
    }
}

fn show_order_flow_header(
    ui: &mut egui::Ui,
    kind: WidgetKind,
    snapshot: &OrderFlowPreviewSnapshot,
) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(kind.mnemonic())
                .monospace()
                .size(12.0)
                .color(egui::Color32::from_rgb(232, 178, 88)),
        );
        ui.label(
            egui::RichText::new(kind.label())
                .monospace()
                .size(17.0)
                .color(egui::Color32::from_rgb(230, 232, 235)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let status = if snapshot.warning.is_some() {
                "partial"
            } else {
                "ready"
            };
            ui.label(
                egui::RichText::new(status)
                    .monospace()
                    .size(11.0)
                    .color(order_flow_status_color(snapshot.warning.is_some())),
            );
        });
    });
    ui.add_space(6.0);
    compact_status_row(ui, "Source", &snapshot.source_label);
    compact_status_row(
        ui,
        "Contract",
        &snapshot.instrument_selection.source_contract_label(),
    );
    compact_status_row(
        ui,
        "Instrument",
        &snapshot.instrument_selection.instrument_label(),
    );
    compact_status_row(
        ui,
        "Selection",
        &snapshot.instrument_selection.selection_label(),
    );
    if let Some(available_ids) = snapshot.instrument_selection.available_ids_label() {
        compact_status_row(ui, "Available", &available_ids);
    }
    compact_status_row(ui, "Range", &snapshot.range_label);
    compact_status_row(ui, "Path", &snapshot.raw_path.display().to_string());
    compact_status_row(
        ui,
        "Replay",
        &format!(
            "{} events / {} book events / {} prints / {} batches / {} checkpoints",
            snapshot.report.read_events,
            snapshot.report.applied_book_events,
            snapshot.report.trade_prints,
            snapshot.report.batches,
            snapshot.checkpoint_count
        ),
    );
    compact_status_row(ui, "Timeline", timeline_status_label(&snapshot.report));
    if let Some(warning) = &snapshot.warning {
        compact_status_row(ui, "Warning", warning);
    }
}

fn show_order_book(
    ui: &mut egui::Ui,
    snapshot: &OrderFlowPreviewSnapshot,
    audit: Option<&ReplayAuditSnapshot>,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) {
    let (bid_levels, ask_levels, book) = replay_order_book_state(snapshot, audit, derived_window);
    let Some(book) = book else {
        empty_state(ui, "No reconstructed order book snapshot is available");
        return;
    };

    ui.horizontal_wrapped(|ui| {
        metric_label(ui, "Instrument", &book.instrument_id.to_string());
        if let Some(audit) = audit {
            metric_label(ui, "View ts", &audit.cursor_ts.to_string());
        }
        metric_label(ui, "Quality", book_quality_label(book.quality));
        metric_label(ui, "Orders", &book.order_count.to_string());
        metric_label(
            ui,
            "Spread",
            &book
                .top
                .spread
                .map(format_price_value)
                .unwrap_or_else(|| "--".to_string()),
        );
        metric_label(
            ui,
            "Mid",
            &book
                .top
                .mid
                .map(format_price_value)
                .unwrap_or_else(|| "--".to_string()),
        );
    });
    ui.add_space(8.0);
    ui.columns(2, |columns| {
        show_book_side(&mut columns[0], "ASK", &ask_levels, true);
        show_book_side(&mut columns[1], "BID", &bid_levels, false);
    });
}

fn show_liquidity_heatmap(
    ui: &mut egui::Ui,
    snapshot: &OrderFlowPreviewSnapshot,
    audit: Option<&ReplayAuditSnapshot>,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) {
    let depth_levels = replay_depth_levels(snapshot, audit, derived_window);
    ui.horizontal_wrapped(|ui| {
        metric_label(ui, "Frames", &snapshot.heatmap_frame_count.to_string());
        metric_label(ui, "Cells", &snapshot.liquidity_cell_count.to_string());
        metric_label(ui, "Events", &snapshot.event_count.to_string());
        metric_label(ui, "Instruments", &snapshot.instrument_count.to_string());
        metric_label(
            ui,
            "Last ts",
            &snapshot
                .last_event_ts
                .map(|value| value.to_string())
                .unwrap_or_else(|| "--".to_string()),
        );
    });
    ui.add_space(8.0);

    if depth_levels.is_empty() {
        empty_state(ui, "No depth-time frame is available");
        return;
    }

    let max_size = depth_levels
        .iter()
        .map(|level| level.size)
        .max()
        .unwrap_or(1);
    for level in &depth_levels {
        depth_sample_row(ui, level, max_size);
        ui.add_space(3.0);
    }
}

fn show_footprint(
    ui: &mut egui::Ui,
    snapshot: &OrderFlowPreviewSnapshot,
    audit: Option<&ReplayAuditSnapshot>,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) {
    let flow_summary = replay_flow_summary(snapshot, derived_window);
    ui.horizontal_wrapped(|ui| {
        metric_label(ui, "Cells", &snapshot.footprint_cell_count.to_string());
        metric_label(
            ui,
            "Interval",
            &format!("{}s", snapshot.footprint_interval_ns / 1_000_000_000),
        );
        metric_label(ui, "CVD", &flow_summary.cumulative_delta.to_string());
        metric_label(ui, "Volume", &flow_summary.total_volume().to_string());
        metric_label(ui, "Large", &flow_summary.large_trade_count.to_string());
    });
    let footprint_poc = derived_window
        .and_then(|window| window.footprint_poc)
        .or(snapshot.footprint_poc);
    if let Some((key, cell)) = footprint_poc {
        ui.add_space(6.0);
        compact_status_row(
            ui,
            "POC",
            &format!(
                "{} / {} vol / {} delta",
                format_mbo_price(key.price),
                cell.total_volume(),
                cell.delta()
            ),
        );
    }
    ui.add_space(8.0);

    let top_footprint_cells = if let Some(audit) = audit {
        replay_footprint_cells(snapshot, audit, derived_window)
    } else {
        derived_window
            .map(|window| window.top_footprint_cells.clone())
            .unwrap_or_else(|| snapshot.top_footprint_cells.clone())
    };
    if top_footprint_cells.is_empty() {
        empty_state(ui, "No trade or fill footprint cells are available");
        return;
    }

    for (key, cell) in top_footprint_cells {
        footprint_row(ui, key, cell);
        ui.add_space(3.0);
    }
}

fn show_bookmap_order_flow(
    ui: &mut egui::Ui,
    snapshot: &OrderFlowPreviewSnapshot,
    audit: Option<&ReplayAuditSnapshot>,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
    controls: &mut ReplayControlState,
    layers: &mut BookmapLayerState,
) {
    let Some(audit) = audit else {
        empty_state(ui, "No order-flow timeline is available");
        return;
    };
    let cells = replay_liquidity_cells(snapshot, audit, derived_window);
    let ohlcv_bars = replay_ohlcv_bars(snapshot, audit, derived_window);
    let bubbles = replay_volume_bubbles(snapshot, audit, derived_window);
    let bbo = replay_bbo_transitions(snapshot, audit, derived_window);
    let active_cursor_price = nearest_cursor_price(audit.cursor_ts, &ohlcv_bars, &bbo);
    let dom_snapshot = bookmap_aligned_dom_snapshot(
        replay_dom_snapshot(snapshot, derived_window).cloned(),
        &bbo,
        active_cursor_price,
    );
    let footprint_cells = replay_footprint_cells(snapshot, audit, derived_window);
    let frame = BookmapFrame::from_layers(BookmapFrameInput {
        source: BookmapFrameSource {
            source_label: snapshot.source_label.clone(),
            range_label: snapshot.range_label.clone(),
            instrument_selection: Some(snapshot.instrument_selection.clone()),
            derived: snapshot.derived_source.is_some(),
            event_count: snapshot.event_count,
            instrument_count: snapshot.instrument_count,
            warning: snapshot.warning.clone(),
        },
        instrument_id: snapshot.instrument_selection.instrument_id,
        start_ts: audit.window_start_ts,
        end_ts: audit.window_end_ts,
        cursor_ts: audit.cursor_ts,
        checkpoint: derived_window.and_then(|window| window.checkpoint),
        book_quality: snapshot.book_snapshot.as_ref().map(|book| book.quality),
        heatmap_cells: cells,
        volume_bubbles: bubbles,
        bbo_transitions: bbo,
        dom_snapshot,
        footprint_cells,
        footprint_poc: derived_window
            .and_then(|window| window.footprint_poc)
            .or(snapshot.footprint_poc),
        flow_summary: replay_flow_summary(snapshot, derived_window),
        add_on_events: replay_add_on_events(snapshot, derived_window),
        unsupported_add_ons: default_unsupported_add_ons(),
    });
    let min_ts = frame.viewport.start_ts;
    let max_ts = frame.viewport.end_ts;
    let visible_bubbles =
        bookmap_visible_volume_bubbles(&frame.layers.volume_bubbles, &frame.layers.bbo_transitions);
    let Some((min_price, max_price)) =
        bookmap_terminal_visible_price_range(&frame, &ohlcv_bars, &visible_bubbles)
    else {
        empty_state(
            ui,
            "No price ladder is available for the current replay cursor",
        );
        return;
    };

    let height = ui.available_height().max(520.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(7, 10, 12));

    let layout = bookmap_terminal_layout(rect);
    let chart_rect = layout.chart_rect;
    let axis_rect = layout.axis_rect;

    draw_bookmap_top_strip(
        &painter,
        layout.toolbar_rect,
        snapshot,
        audit,
        derived_window.is_some(),
        controls,
        layers,
    );
    handle_bookmap_terminal_interactions(ui, &layout, audit, controls, layers);
    draw_bookmap_left_rail(&painter, layout.left_rail_rect, layers);
    painter.rect_filled(chart_rect, 0.0, egui::Color32::from_rgb(8, 15, 18));
    if layers.is_enabled(BookmapLayerKind::LiquidityHeatmap) {
        draw_heatmap_surface(
            &painter,
            chart_rect,
            min_ts,
            max_ts,
            min_price,
            max_price,
            &frame.layers.heatmap_bands,
        );
        draw_current_book_continuation(
            &painter,
            chart_rect,
            min_ts,
            max_ts,
            min_price,
            max_price,
            &frame.layers.current_book_bands,
        );
    }
    draw_chart_grid(&painter, chart_rect, min_price, max_price);
    draw_bookmap_price_action(
        &painter,
        chart_rect,
        min_ts,
        max_ts,
        min_price,
        max_price,
        &ohlcv_bars,
        &frame.layers.bbo_transitions,
        &visible_bubbles,
    );
    draw_bookmap_volume_histogram(
        &painter,
        layout.volume_rect,
        min_ts,
        max_ts,
        &ohlcv_bars,
        &visible_bubbles,
    );
    if layers.is_enabled(BookmapLayerKind::BboPath) {
        draw_bbo_paths(
            &painter,
            chart_rect,
            min_ts,
            max_ts,
            min_price,
            max_price,
            &frame.layers.bbo_transitions,
        );
    }
    if layers.is_enabled(BookmapLayerKind::VolumeBubbles) {
        draw_volume_bubbles(
            &painter,
            chart_rect,
            min_ts,
            max_ts,
            min_price,
            max_price,
            &visible_bubbles,
        );
        if layers.is_enabled(BookmapLayerKind::BboPath) {
            draw_bbo_trade_markers(
                &painter,
                chart_rect,
                min_ts,
                max_ts,
                min_price,
                max_price,
                &visible_bubbles,
                &frame.layers.bbo_transitions,
            );
        }
    }
    if layers.is_enabled(BookmapLayerKind::StopsIcebergs) {
        let events = heuristic_overlay_events_from_addons(&frame.layers.add_on_events)
            .unwrap_or_else(|| {
                heuristic_order_flow_events_from_parts(
                    &frame.layers.heatmap_cells,
                    &frame.layers.volume_bubbles,
                )
            });
        draw_stops_icebergs_annotations(
            &painter, chart_rect, min_ts, max_ts, min_price, max_price, &events,
        );
    }
    if layers.is_enabled(BookmapLayerKind::ReplayCursor) {
        draw_replay_cursor(
            &painter,
            chart_rect,
            min_ts,
            max_ts,
            frame.viewport.cursor_ts,
        );
    }
    if layers.is_enabled(BookmapLayerKind::CobDomColumns) {
        draw_dom_columns(
            &painter,
            layout.price_ladder_rect,
            layout.cob_rect,
            layout.cvp_rect,
            layout.svp_rect,
            min_price,
            max_price,
            frame.layers.dom_snapshot.as_ref(),
        );
    }
    if layers.is_enabled(BookmapLayerKind::TradermapSplit) {
        draw_bookmap_tradermap_split_overlay(
            &painter,
            chart_rect,
            min_ts,
            max_ts,
            min_price,
            max_price,
            &frame.layers.heatmap_cells,
            &frame.layers.bbo_transitions,
            &frame.layers.volume_bubbles,
        );
    }
    if layers.is_enabled(BookmapLayerKind::DomProPanel) {
        if let Some(dom) = frame.layers.dom_snapshot.as_ref() {
            draw_bookmap_dom_pro_panel(&painter, chart_rect, min_price, max_price, dom);
        }
    }
    if layers.is_enabled(BookmapLayerKind::MarketPulse) {
        let metrics = pulse_metrics_for_replay_window(snapshot, audit, derived_window);
        draw_market_pulse_overlay_widgets(&painter, chart_rect, &metrics);
    }
    draw_last_price_marker(
        &painter,
        chart_rect,
        layout.right_panel_rect,
        min_price,
        max_price,
        bookmap_active_price_marker(
            frame.viewport.cursor_ts,
            &ohlcv_bars,
            &frame.layers.bbo_transitions,
            &visible_bubbles,
            frame.layers.dom_snapshot.as_ref(),
        ),
    );
    draw_bookmap_time_axis(
        &painter,
        axis_rect,
        min_ts,
        max_ts,
        frame.viewport.cursor_ts,
    );
    draw_bookmap_cursor_tooltip(
        &painter,
        chart_rect,
        min_ts,
        max_ts,
        min_price,
        max_price,
        frame.viewport.cursor_ts,
        &ohlcv_bars,
        &frame.layers.bbo_transitions,
    );
    draw_bookmap_status_strip(&painter, layout.status_rect, snapshot, audit);
}

fn handle_bookmap_terminal_interactions(
    ui: &mut egui::Ui,
    layout: &BookmapTerminalLayout,
    audit: &ReplayAuditSnapshot,
    controls: &mut ReplayControlState,
    layers: &mut BookmapLayerState,
) {
    let replay_rects = bookmap_replay_control_rects(layout.toolbar_rect);
    let play_response = ui.interact(
        replay_rects.play_rect,
        ui.id().with("bookmap_replay_play"),
        egui::Sense::click(),
    );
    if play_response.clicked() {
        controls.playing = !controls.playing;
        if controls.playing && controls.cursor_fraction >= 1.0 {
            controls.cursor_fraction = 0.0;
        }
        ui.ctx().request_repaint();
    }
    play_response.on_hover_text("Play or pause replay");

    let step_response = ui.interact(
        replay_rects.step_rect,
        ui.id().with("bookmap_replay_step"),
        egui::Sense::click(),
    );
    if step_response.clicked() {
        controls.cursor_fraction = (controls.cursor_fraction + 0.025).min(1.0);
        ui.ctx().request_repaint();
    }
    step_response.on_hover_text("Step replay forward");

    let reset_response = ui.interact(
        replay_rects.reset_rect,
        ui.id().with("bookmap_replay_reset"),
        egui::Sense::click(),
    );
    if reset_response.clicked() {
        controls.cursor_fraction = 0.0;
        controls.playing = false;
        ui.ctx().request_repaint();
    }
    reset_response.on_hover_text("Reset replay cursor");

    let scrub_rect = layout.chart_rect.union(layout.axis_rect);
    let scrub_response = ui.interact(
        scrub_rect,
        ui.id().with("bookmap_replay_scrub"),
        egui::Sense::click_and_drag(),
    );
    if (scrub_response.clicked() || scrub_response.dragged())
        && let Some(pos) = scrub_response.interact_pointer_pos()
    {
        let timestamp = timestamp_for_x(
            layout.chart_rect,
            audit.window_start_ts,
            audit.window_end_ts,
            pos.x,
        );
        controls.cursor_fraction =
            cursor_fraction_for_timestamp(audit.min_ts, audit.max_ts, timestamp);
        controls.playing = false;
        ui.ctx().request_repaint();
    }
    scrub_response.on_hover_text("Click or drag to scrub replay");

    for (kind, rect) in bookmap_left_rail_layer_toggle_rects(layout.left_rail_rect) {
        let response = ui.interact(
            rect,
            ui.id().with(("bookmap_layer", kind.short_label())),
            egui::Sense::click(),
        );
        if response.clicked() {
            layers.toggle(kind);
            ui.ctx().request_repaint();
        }
        response.on_hover_text(format!("Toggle {}", kind.label()));
    }
}

fn timestamp_for_x(rect: egui::Rect, min_ts: u64, max_ts: u64, x: f32) -> u64 {
    if max_ts <= min_ts || rect.width() <= 0.0 {
        return min_ts;
    }
    let fraction = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
    min_ts.saturating_add(((max_ts - min_ts) as f32 * fraction) as u64)
}

fn cursor_fraction_for_timestamp(min_ts: u64, max_ts: u64, timestamp: u64) -> f32 {
    if max_ts <= min_ts {
        return 0.0;
    }
    (timestamp.saturating_sub(min_ts) as f32 / (max_ts - min_ts) as f32).clamp(0.0, 1.0)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct BookmapTerminalLayout {
    toolbar_rect: egui::Rect,
    left_rail_rect: egui::Rect,
    chart_rect: egui::Rect,
    volume_rect: egui::Rect,
    axis_rect: egui::Rect,
    status_rect: egui::Rect,
    right_panel_rect: egui::Rect,
    current_book_rect: egui::Rect,
    price_ladder_rect: egui::Rect,
    cob_rect: egui::Rect,
    cvp_rect: egui::Rect,
    svp_rect: egui::Rect,
}

fn bookmap_terminal_layout(rect: egui::Rect) -> BookmapTerminalLayout {
    let toolbar_height = (rect.height() * 0.13).clamp(74.0, 124.0);
    let axis_height = 24.0_f32.min(rect.height() * 0.06);
    let status_height = 22.0_f32.min(rect.height() * 0.05);
    let volume_height = (rect.height() * 0.075).clamp(42.0, 82.0);
    let left_rail_width = bookmap_left_rail_width(rect.width());
    let dom_width = bookmap_dom_width(rect.width());
    let gutter = 0.0;
    let toolbar_rect = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right(), rect.top() + toolbar_height),
    );
    let body_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left(), toolbar_rect.bottom()),
        egui::pos2(rect.right(), rect.bottom() - status_height),
    );
    let status_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left(), body_rect.bottom()),
        egui::pos2(rect.right(), rect.bottom()),
    );
    let left_rail_rect = egui::Rect::from_min_max(
        body_rect.min,
        egui::pos2(body_rect.left() + left_rail_width, body_rect.bottom()),
    );
    let chart_left = left_rail_rect.right() + gutter;
    let chart_right = body_rect.right() - dom_width - gutter;
    let axis_bottom = body_rect.bottom();
    let axis_top = axis_bottom - axis_height;
    let volume_bottom = axis_top;
    let volume_top = volume_bottom - volume_height;
    let chart_rect = egui::Rect::from_min_max(
        egui::pos2(chart_left, body_rect.top()),
        egui::pos2(chart_right.max(chart_left), volume_top.max(body_rect.top())),
    );
    let volume_rect = egui::Rect::from_min_max(
        egui::pos2(chart_rect.left(), chart_rect.bottom()),
        egui::pos2(chart_rect.right(), volume_bottom.max(chart_rect.bottom())),
    );
    let axis_rect = egui::Rect::from_min_max(
        egui::pos2(chart_rect.left(), volume_rect.bottom()),
        egui::pos2(chart_rect.right(), axis_bottom),
    );
    let right_panel_rect = egui::Rect::from_min_max(
        egui::pos2(chart_rect.right() + gutter, body_rect.top()),
        egui::pos2(body_rect.right(), axis_rect.bottom()),
    );
    let right_columns = dom_columns(
        right_panel_rect,
        if right_panel_rect.width() >= 260.0 {
            &[0.22, 0.28, 0.24, 0.26]
        } else {
            &[0.26, 0.30, 0.22, 0.22]
        },
    );
    let current_book_rect = egui::Rect::from_min_max(
        egui::pos2(
            chart_rect.right() - (chart_rect.width() * 0.28).clamp(90.0, 320.0),
            chart_rect.top(),
        ),
        chart_rect.max,
    );
    BookmapTerminalLayout {
        toolbar_rect,
        left_rail_rect,
        chart_rect,
        volume_rect,
        axis_rect,
        status_rect,
        right_panel_rect,
        current_book_rect,
        price_ladder_rect: right_columns[0],
        cob_rect: right_columns[1],
        cvp_rect: right_columns[2],
        svp_rect: right_columns[3],
    }
}

fn show_filtered_flow_map(
    ui: &mut egui::Ui,
    snapshot: &OrderFlowPreviewSnapshot,
    audit: Option<&ReplayAuditSnapshot>,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) {
    let cells = if let Some(audit) = audit {
        replay_liquidity_cells(snapshot, audit, derived_window)
    } else {
        derived_window
            .map(|window| window.visible_liquidity_cells.clone())
            .unwrap_or_else(|| snapshot.visible_liquidity_cells.clone())
    };
    let threshold = filtered_liquidity_threshold_for_cells(&cells);
    ui.horizontal_wrapped(|ui| {
        metric_label(ui, "Mode", "synthetic MBO filter");
        metric_label(ui, "Min size", &threshold.to_string());
        metric_label(ui, "Cells", &cells.len().to_string());
    });
    ui.add_space(8.0);
    let filtered = cells
        .iter()
        .rev()
        .filter(|(_, cell)| cell.displayed_size >= threshold && cell.displayed_size > 0)
        .take(18)
        .copied()
        .collect::<Vec<_>>();
    if filtered.is_empty() {
        empty_state(ui, "No liquidity cells match the current synthetic filter");
        return;
    }
    for (key, cell) in filtered {
        let value = format!(
            "{}  {}  size {}  intensity {:.2}",
            side_label(key.side),
            format_mbo_price(key.price),
            cell.displayed_size,
            cell.intensity
        );
        row_band(ui, &value, side_fill_color(key.side));
        ui.add_space(3.0);
    }
}

fn show_heuristic_order_flow_events(
    ui: &mut egui::Ui,
    snapshot: &OrderFlowPreviewSnapshot,
    audit: Option<&ReplayAuditSnapshot>,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) {
    let cells = if let Some(audit) = audit {
        replay_liquidity_cells(snapshot, audit, derived_window)
    } else {
        derived_window
            .map(|window| window.visible_liquidity_cells.clone())
            .unwrap_or_else(|| snapshot.visible_liquidity_cells.clone())
    };
    let bubbles = if let Some(audit) = audit {
        replay_volume_bubbles(snapshot, audit, derived_window)
    } else {
        derived_window
            .map(|window| window.volume_bubbles.clone())
            .unwrap_or_else(|| snapshot.volume_bubbles.clone())
    };
    ui.horizontal_wrapped(|ui| {
        metric_label(ui, "Mode", "heuristic");
        metric_label(ui, "Bubbles", &bubbles.len().to_string());
        metric_label(ui, "Heat cells", &cells.len().to_string());
    });
    ui.add_space(6.0);
    empty_state(
        ui,
        "Events are pattern annotations, not confirmed hidden orders or stop executions.",
    );
    ui.add_space(8.0);

    let events = heuristic_order_flow_events_from_parts(&cells, &bubbles);
    if events.is_empty() {
        empty_state(
            ui,
            "No heuristic events were detected in the bounded replay snapshot",
        );
        return;
    }
    for event in events {
        let value = format!(
            "{}  {}  score {:.2}  ts {}  {}",
            event.kind,
            format_mbo_price(event.price),
            event.score,
            event.interval_start_ns,
            event.note
        );
        row_band(ui, &value, egui::Color32::from_rgb(232, 178, 88));
        ui.add_space(3.0);
    }
}

fn show_market_pulse(
    ui: &mut egui::Ui,
    snapshot: &OrderFlowPreviewSnapshot,
    audit: Option<&ReplayAuditSnapshot>,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) {
    let metrics = if let Some(audit) = audit {
        pulse_metrics_for_replay_window(snapshot, audit, derived_window)
    } else {
        pulse_metrics_at_cursor(snapshot, None)
    };
    ui.horizontal_wrapped(|ui| {
        metric_label(ui, "Mode", "historical MBO pulse");
        metric_label(ui, "Metrics", &metrics.len().to_string());
        metric_label(ui, "Events", &snapshot.event_count.to_string());
    });
    ui.add_space(8.0);
    if metrics.is_empty() {
        empty_state(ui, "No pulse metrics are available");
        return;
    }
    let cells = if let Some(audit) = audit {
        replay_liquidity_cells(snapshot, audit, derived_window)
    } else {
        snapshot.visible_liquidity_cells.clone()
    };
    let bubbles = if let Some(audit) = audit {
        replay_volume_bubbles(snapshot, audit, derived_window)
    } else {
        snapshot.volume_bubbles.clone()
    };
    let bbo = if let Some(audit) = audit {
        replay_bbo_transitions(snapshot, audit, derived_window)
    } else {
        snapshot.bbo_transitions.clone()
    };
    let time_range = audit
        .map(|audit| (audit.window_start_ts, audit.window_end_ts))
        .or_else(|| composite_time_range(snapshot));
    let price_range = composite_price_range(
        &cells,
        &bubbles,
        &bbo,
        replay_dom_snapshot(snapshot, derived_window),
    );
    let height = ui.available_height().clamp(320.0, 560.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(7, 10, 12));

    if let (Some((min_ts, max_ts)), Some((min_price, max_price))) = (time_range, price_range) {
        draw_chart_grid(&painter, rect, min_price, max_price);
        draw_heatmap_cell_surface(&painter, rect, min_ts, max_ts, min_price, max_price, &cells);
        draw_bbo_paths(&painter, rect, min_ts, max_ts, min_price, max_price, &bbo);
        draw_volume_bubbles(
            &painter, rect, min_ts, max_ts, min_price, max_price, &bubbles,
        );
        if let Some(audit) = audit {
            draw_replay_cursor(&painter, rect, min_ts, max_ts, audit.cursor_ts);
        }
    }
    draw_market_pulse_overlay_widgets(&painter, rect, &metrics);
}

fn show_nanosecond_zoom(
    ui: &mut egui::Ui,
    snapshot: &OrderFlowPreviewSnapshot,
    audit: Option<&ReplayAuditSnapshot>,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) {
    let Some(audit) = audit else {
        empty_state(ui, "No event-level replay cursor is available");
        return;
    };
    let (view_min_ts, view_max_ts) = nanosecond_zoom_window(audit);
    let cells = filter_liquidity_cells_to_time_window(
        replay_liquidity_cells(snapshot, audit, derived_window),
        view_min_ts,
        view_max_ts,
    );
    let bubbles = filter_volume_bubbles_to_time_window(
        replay_volume_bubbles(snapshot, audit, derived_window),
        view_min_ts,
        view_max_ts,
    );
    let bbo = filter_bbo_to_time_window(
        replay_bbo_transitions(snapshot, audit, derived_window),
        view_min_ts,
        view_max_ts,
    );
    let dom_snapshot = replay_dom_snapshot(snapshot, derived_window);

    let Some((min_price, max_price)) = composite_price_range(&[], &[], &bbo, dom_snapshot)
        .or_else(|| composite_price_range(&cells, &bubbles, &bbo, dom_snapshot))
    else {
        empty_state(
            ui,
            "No event-level price ladder is available at this cursor",
        );
        return;
    };

    let height = ui.available_height().clamp(280.0, 520.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(5, 8, 11));

    let header_height = 30.0_f32.min(rect.height() * 0.14);
    let axis_height = 42.0_f32.min(rect.height() * 0.18);
    let header_rect = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.right(), rect.top() + header_height),
    );
    let plot_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left(), header_rect.bottom()),
        egui::pos2(rect.right(), rect.bottom() - axis_height),
    );
    let axis_rect = egui::Rect::from_min_max(egui::pos2(rect.left(), plot_rect.bottom()), rect.max);

    draw_nanosecond_zoom_header(&painter, header_rect, audit, &bbo, &bubbles);
    draw_nanosecond_zoom_grid(&painter, plot_rect);
    draw_heatmap_cell_surface(
        &painter,
        plot_rect,
        view_min_ts,
        view_max_ts,
        min_price,
        max_price,
        &cells,
    );
    draw_nanosecond_bbo_paths(
        &painter,
        plot_rect,
        view_min_ts,
        view_max_ts,
        min_price,
        max_price,
        &bbo,
    );
    draw_micro_event_ticks(
        &painter,
        plot_rect,
        view_min_ts,
        view_max_ts,
        &bbo,
        &bubbles,
    );
    draw_replay_cursor(
        &painter,
        plot_rect,
        view_min_ts,
        view_max_ts,
        audit.cursor_ts,
    );
    draw_nanosecond_zoom_axis(
        &painter,
        axis_rect,
        view_min_ts,
        view_max_ts,
        audit.cursor_ts,
    );
}

fn nanosecond_zoom_window(audit: &ReplayAuditSnapshot) -> (u64, u64) {
    let view_span = audit
        .window_end_ts
        .saturating_sub(audit.window_start_ts)
        .max(1);
    let half_span = view_span / 2;
    let view_min_ts = audit
        .cursor_ts
        .saturating_sub(half_span)
        .max(audit.window_start_ts);
    let view_max_ts = audit
        .cursor_ts
        .saturating_add(half_span)
        .min(audit.window_end_ts)
        .max(view_min_ts.saturating_add(1));
    (view_min_ts, view_max_ts)
}

fn filter_liquidity_cells_to_time_window(
    cells: Vec<(LiquidityCellKey, LiquidityCell)>,
    min_ts: u64,
    max_ts: u64,
) -> Vec<(LiquidityCellKey, LiquidityCell)> {
    cells
        .into_iter()
        .filter(|(key, _)| key.interval_start_ns >= min_ts && key.interval_start_ns <= max_ts)
        .collect()
}

fn filter_volume_bubbles_to_time_window(
    bubbles: Vec<(VolumeBubbleKey, VolumeBubble)>,
    min_ts: u64,
    max_ts: u64,
) -> Vec<(VolumeBubbleKey, VolumeBubble)> {
    bubbles
        .into_iter()
        .filter(|(key, _)| key.interval_start_ns >= min_ts && key.interval_start_ns <= max_ts)
        .collect()
}

fn filter_bbo_to_time_window(
    bbo: Vec<BboTransition>,
    min_ts: u64,
    max_ts: u64,
) -> Vec<BboTransition> {
    bbo.into_iter()
        .filter(|transition| transition.ts_event >= min_ts && transition.ts_event <= max_ts)
        .collect()
}

fn show_record_replay(
    ui: &mut egui::Ui,
    snapshot: &OrderFlowPreviewSnapshot,
    audit: Option<&ReplayAuditSnapshot>,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
    controls: &ReplayControlState,
) {
    let Some(audit) = audit else {
        empty_state(ui, "No replay timeline is available");
        return;
    };
    ui.horizontal_wrapped(|ui| {
        metric_label(ui, "Mode", "recorded historical MBO");
        metric_label(ui, "Preview cap", &PREVIEW_EVENT_LIMIT.to_string());
        metric_label(ui, "Batches", &snapshot.report.batches.to_string());
        metric_label(ui, "Checkpoints", &snapshot.checkpoint_count.to_string());
    });
    ui.add_space(8.0);
    draw_replay_timeline_panel(ui, snapshot, audit, derived_window, controls);
    ui.add_space(8.0);
    compact_status_row(
        ui,
        "Replay source",
        &snapshot.raw_path.display().to_string(),
    );
    compact_status_row(ui, "Status", timeline_status_label(&snapshot.report));
    if let Some(warning) = &snapshot.warning {
        compact_status_row(ui, "Warning", warning);
    }
}

fn show_developer_api(ui: &mut egui::Ui, snapshot: &OrderFlowPreviewSnapshot) {
    let derived_plan = DerivedCachePlan::for_estimated_events(snapshot.event_count.max(1));
    ui.horizontal_wrapped(|ui| {
        metric_label(ui, "Mode", "RustChart event contracts");
        metric_label(ui, "Loaded events", &snapshot.event_count.to_string());
        metric_label(
            ui,
            "Candle intervals",
            &BOOKMAP_CANDLE_INTERVALS.len().to_string(),
        );
        metric_label(ui, "Heatmap LODs", &BOOKMAP_HEATMAP_LODS.len().to_string());
    });
    ui.add_space(8.0);
    ui.columns(2, |columns| {
        feature_contract_row(
            &mut columns[0],
            "DepthUpdate",
            "MBO add/modify/cancel normalized into book state and depth frames",
            true,
        );
        feature_contract_row(
            &mut columns[0],
            "MboOrderLifecycleEvent",
            "order id, side, price, size, action, flags, event and receive timestamps",
            true,
        );
        feature_contract_row(
            &mut columns[0],
            "TradePrint",
            "trade/fill prints with aggressor side, signed size, delta, and volume",
            true,
        );
        feature_contract_row(
            &mut columns[0],
            "BboEvent",
            "best bid/offer transition path with spread and mid-price",
            true,
        );
        feature_contract_row(
            &mut columns[0],
            "HeatmapCell",
            "price/time/depth bucket with displayed size and intensity",
            true,
        );
        feature_contract_row(
            &mut columns[1],
            "VolumeBubble",
            "buy, sell, unknown volume, total volume, imbalance, trade/fill count",
            true,
        );
        feature_contract_row(
            &mut columns[1],
            "StopEvent",
            "heuristic one-sided burst label; not a confirmed hidden order event",
            false,
        );
        feature_contract_row(
            &mut columns[1],
            "IcebergEvent",
            "heuristic absorption label using volume into persistent displayed liquidity",
            false,
        );
        feature_contract_row(
            &mut columns[1],
            "MarketPulseEvent",
            "flow, book pressure, spread tightness, and price drift metrics",
            true,
        );
        feature_contract_row(
            &mut columns[1],
            "DOMColumnSnapshot",
            "COB/CVP/SVP-style ladder projection from reconstructed book state",
            true,
        );
    });
    ui.add_space(8.0);
    compact_status_row(
        ui,
        "Derived cache plan",
        &format!(
            "{} partitions every {} events",
            derived_plan.event_partitions, derived_plan.checkpoint_every_events
        ),
    );
    compact_status_row(
        ui,
        "Intervals",
        &BOOKMAP_CANDLE_INTERVALS
            .iter()
            .map(|interval| interval.label)
            .collect::<Vec<_>>()
            .join(", "),
    );
    compact_status_row(
        ui,
        "Heatmap LODs",
        &BOOKMAP_HEATMAP_LODS
            .iter()
            .map(|lod| lod.label)
            .collect::<Vec<_>>()
            .join(", "),
    );
}

fn show_market_data_connections(ui: &mut egui::Ui, snapshot: &OrderFlowPreviewSnapshot) {
    ui.horizontal_wrapped(|ui| {
        metric_label(ui, "Mode", "connection hub");
        metric_label(ui, "Active local feed", &snapshot.source_label);
        metric_label(ui, "Schema", "mbo");
    });
    ui.add_space(8.0);
    let providers = provider_capability_registry(snapshot.derived_source.is_some());
    let height = ui.available_height().clamp(300.0, 520.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    draw_market_data_connection_hub(&painter, rect, &providers, snapshot);
    ui.add_space(8.0);
    compact_status_row(ui, "Loaded path", &snapshot.raw_path.display().to_string());
    compact_status_row(ui, "Range", &snapshot.range_label);
}

fn provider_capability_registry(derived_cache_ready: bool) -> Vec<ProviderCapability> {
    vec![
        ProviderCapability {
            label: "Databento Historical MBO",
            status: ProviderStatus::Ready,
            capabilities: &["MBO", "MBP", "TRD", "BBO", "HIST", "REPLAY"],
            detail: "local DBN cache through normalized MBO events",
        },
        ProviderCapability {
            label: "Derived Cache",
            status: if derived_cache_ready {
                ProviderStatus::Ready
            } else {
                ProviderStatus::Planned
            },
            capabilities: &["HEATMAP", "BUBBLES", "BBO", "DOM", "LOD"],
            detail: "compressed replay artifacts for Bookmap surfaces",
        },
        ProviderCapability {
            label: "Databento Live MBO",
            status: ProviderStatus::Planned,
            capabilities: &["MBO", "MBP", "TRD", "BBO", "LIVE"],
            detail: "same event contract; network adapter not enabled",
        },
        ProviderCapability {
            label: "Legacy Bar Feeds",
            status: ProviderStatus::Limited,
            capabilities: &["OHLCV", "HIST"],
            detail: "YFinance, Massive, Binance, and CoinGecko compatibility",
        },
        ProviderCapability {
            label: "Broker / Own Orders",
            status: ProviderStatus::Unavailable,
            capabilities: &["READONLY"],
            detail: "order-entry and own-order state are out of scope",
        },
    ]
}

fn draw_market_data_connection_hub(
    painter: &egui::Painter,
    rect: egui::Rect,
    providers: &[ProviderCapability],
    snapshot: &OrderFlowPreviewSnapshot,
) {
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(7, 10, 13));
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(42, 48, 55)),
        egui::StrokeKind::Inside,
    );

    let hub = market_data_hub_rect(rect);
    let node_rects = market_data_connection_node_rects(rect, providers.len());
    for (provider, node_rect) in providers.iter().zip(node_rects.iter().copied()) {
        let color = provider_status_color(provider.status);
        painter.line_segment(
            [node_rect.center(), hub.center()],
            egui::Stroke::new(1.2, color.gamma_multiply(0.75)),
        );
    }

    painter.rect_filled(hub, 5.0, egui::Color32::from_rgb(18, 24, 31));
    painter.rect_stroke(
        hub,
        5.0,
        egui::Stroke::new(1.5, egui::Color32::from_rgb(84, 146, 198)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        hub.center_top() + egui::vec2(0.0, 20.0),
        egui::Align2::CENTER_CENTER,
        "RUSTCHART",
        egui::FontId::monospace(17.0),
        egui::Color32::from_rgb(226, 232, 238),
    );
    painter.text(
        hub.center() + egui::vec2(0.0, 8.0),
        egui::Align2::CENTER_CENTER,
        "BOOKMAP HUB",
        egui::FontId::monospace(11.0),
        egui::Color32::from_rgb(132, 188, 226),
    );
    painter.text(
        hub.center_bottom() - egui::vec2(0.0, 14.0),
        egui::Align2::CENTER_CENTER,
        format!("{} events", snapshot.event_count),
        egui::FontId::monospace(10.0),
        egui::Color32::from_rgb(150, 158, 168),
    );

    for (provider, node_rect) in providers.iter().zip(node_rects) {
        draw_provider_node(painter, node_rect, provider);
    }
}

fn market_data_hub_rect(rect: egui::Rect) -> egui::Rect {
    let width = (rect.width() * 0.22).clamp(136.0, 190.0);
    let height = 78.0_f32.min(rect.height() * 0.35);
    egui::Rect::from_center_size(rect.center(), egui::vec2(width, height))
}

fn market_data_connection_node_rects(rect: egui::Rect, count: usize) -> Vec<egui::Rect> {
    if count == 0 {
        return Vec::new();
    }
    let node_width = (rect.width() * 0.30).clamp(150.0, 240.0);
    let node_height = 58.0_f32.min(rect.height() * 0.20);
    let side_padding = 12.0;
    let vertical_min = rect.top() + 14.0;
    let vertical_max = (rect.bottom() - node_height - 14.0).max(vertical_min);
    let left_total = count.div_ceil(2);
    let right_total = count / 2;
    let mut left_index = 0;
    let mut right_index = 0;
    let mut rects = Vec::with_capacity(count);

    for index in 0..count {
        let left_side = index % 2 == 0;
        let (side_index, side_total) = if left_side {
            let value = (left_index, left_total);
            left_index += 1;
            value
        } else {
            let value = (right_index, right_total.max(1));
            right_index += 1;
            value
        };
        let y_fraction = (side_index + 1) as f32 / (side_total + 1) as f32;
        let y = egui::lerp(vertical_min..=vertical_max, y_fraction);
        let x = if left_side {
            rect.left() + side_padding
        } else {
            rect.right() - side_padding - node_width
        };
        rects.push(egui::Rect::from_min_size(
            egui::pos2(x, y),
            egui::vec2(node_width, node_height),
        ));
    }
    rects
}

fn draw_provider_node(painter: &egui::Painter, rect: egui::Rect, provider: &ProviderCapability) {
    let status_color = provider_status_color(provider.status);
    painter.rect_filled(rect, 4.0, egui::Color32::from_rgb(15, 19, 24));
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, status_color.gamma_multiply(0.9)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        egui::pos2(rect.left() + 8.0, rect.top() + 13.0),
        egui::Align2::LEFT_CENTER,
        provider.label,
        egui::FontId::monospace(11.0),
        egui::Color32::from_rgb(226, 230, 236),
    );
    painter.text(
        egui::pos2(rect.right() - 8.0, rect.top() + 13.0),
        egui::Align2::RIGHT_CENTER,
        provider_status_label(provider.status),
        egui::FontId::monospace(9.0),
        status_color,
    );
    painter.text(
        egui::pos2(rect.left() + 8.0, rect.top() + 31.0),
        egui::Align2::LEFT_CENTER,
        provider.detail,
        egui::FontId::monospace(9.0),
        egui::Color32::from_rgb(142, 150, 160),
    );

    let mut x = rect.left() + 8.0;
    let y = rect.bottom() - 11.0;
    for capability in provider.capabilities.iter().take(5) {
        let chip_width = capability.len() as f32 * 6.0 + 10.0;
        if x + chip_width > rect.right() - 6.0 {
            break;
        }
        let chip = egui::Rect::from_min_size(egui::pos2(x, y - 8.0), egui::vec2(chip_width, 14.0));
        painter.rect_filled(chip, 2.0, status_color.gamma_multiply(0.18));
        painter.rect_stroke(
            chip,
            2.0,
            egui::Stroke::new(1.0, status_color.gamma_multiply(0.55)),
            egui::StrokeKind::Inside,
        );
        painter.text(
            chip.center(),
            egui::Align2::CENTER_CENTER,
            *capability,
            egui::FontId::monospace(8.0),
            egui::Color32::from_rgb(210, 216, 222),
        );
        x += chip_width + 4.0;
    }
}

fn provider_status_label(status: ProviderStatus) -> &'static str {
    match status {
        ProviderStatus::Ready => "ready",
        ProviderStatus::Planned => "planned",
        ProviderStatus::Limited => "limited",
        ProviderStatus::Unavailable => "unavailable",
    }
}

fn provider_status_color(status: ProviderStatus) -> egui::Color32 {
    match status {
        ProviderStatus::Ready => egui::Color32::from_rgb(74, 205, 126),
        ProviderStatus::Planned => egui::Color32::from_rgb(232, 178, 88),
        ProviderStatus::Limited => egui::Color32::from_rgb(126, 156, 190),
        ProviderStatus::Unavailable => egui::Color32::from_rgb(132, 136, 144),
    }
}

fn show_stops_icebergs(
    ui: &mut egui::Ui,
    snapshot: &OrderFlowPreviewSnapshot,
    audit: Option<&ReplayAuditSnapshot>,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) {
    ui.horizontal_wrapped(|ui| {
        metric_label(ui, "Mode", "heuristic stops and icebergs");
        metric_label(ui, "Confidence", "derived from MBO only");
        metric_label(ui, "Confirmed hidden orders", "no");
    });
    ui.add_space(6.0);
    compact_status_row(
        ui,
        "Heuristic status",
        "Labels are RustChart heuristics and must not be presented as confirmed exchange event types.",
    );
    ui.add_space(8.0);
    let cells = if let Some(audit) = audit {
        replay_liquidity_cells(snapshot, audit, derived_window)
    } else {
        snapshot.visible_liquidity_cells.clone()
    };
    let bubbles = if let Some(audit) = audit {
        replay_volume_bubbles(snapshot, audit, derived_window)
    } else {
        snapshot.volume_bubbles.clone()
    };
    let bbo = if let Some(audit) = audit {
        replay_bbo_transitions(snapshot, audit, derived_window)
    } else {
        snapshot.bbo_transitions.clone()
    };
    let events = heuristic_order_flow_events_from_parts(&cells, &bubbles);
    if events.is_empty() {
        empty_state(
            ui,
            "No heuristic stop or iceberg annotations are available in this replay window",
        );
        return;
    }
    let Some((min_ts, max_ts)) = audit
        .map(|audit| (audit.window_start_ts, audit.window_end_ts))
        .or_else(|| composite_time_range(snapshot))
    else {
        empty_state(ui, "No time range is available for S&I annotations");
        return;
    };
    let Some((min_price, max_price)) = composite_price_range(&cells, &bubbles, &bbo, None) else {
        empty_state(ui, "No price range is available for S&I annotations");
        return;
    };
    let height = ui.available_height().clamp(320.0, 580.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(7, 10, 12));
    draw_chart_grid(&painter, rect, min_price, max_price);
    draw_heatmap_cell_surface(&painter, rect, min_ts, max_ts, min_price, max_price, &cells);
    draw_bbo_paths(&painter, rect, min_ts, max_ts, min_price, max_price, &bbo);
    draw_volume_bubbles(
        &painter, rect, min_ts, max_ts, min_price, max_price, &bubbles,
    );
    draw_stops_icebergs_annotations(
        &painter, rect, min_ts, max_ts, min_price, max_price, &events,
    );
}

fn draw_stops_icebergs_annotations(
    painter: &egui::Painter,
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    min_price: i64,
    max_price: i64,
    events: &[HeuristicOrderFlowEvent],
) {
    let visible_events = events
        .iter()
        .filter(|event| event.interval_start_ns >= min_ts && event.interval_start_ns <= max_ts)
        .take(6)
        .collect::<Vec<_>>();
    for event in &visible_events {
        let x = x_for_timestamp(rect, min_ts, max_ts, event.interval_start_ns);
        let y = y_for_price(rect, min_price, max_price, event.price);
        let is_iceberg = event.kind.contains("Iceberg");
        let color = if is_iceberg {
            egui::Color32::from_rgb(54, 176, 226)
        } else {
            egui::Color32::from_rgb(235, 28, 36)
        };
        draw_dotted_vertical_guide(painter, rect, x, color);
        painter.circle_filled(egui::pos2(x, y), 7.0, color);
        painter.circle_stroke(
            egui::pos2(x, y),
            7.0,
            egui::Stroke::new(1.5, egui::Color32::from_rgb(8, 10, 12)),
        );
        draw_stops_icebergs_label(
            painter,
            egui::pos2(
                (x + 8.0).min(rect.right() - 62.0),
                (y - 28.0).max(rect.top() + 10.0),
            ),
            event,
            color,
        );
    }
    for (index, event) in visible_events.iter().take(2).enumerate() {
        let end = egui::pos2(
            x_for_timestamp(rect, min_ts, max_ts, event.interval_start_ns),
            y_for_price(rect, min_price, max_price, event.price),
        );
        let start = if index == 0 {
            egui::pos2(
                rect.left() + rect.width() * 0.14,
                rect.top() + rect.height() * 0.34,
            )
        } else {
            egui::pos2(
                rect.right() - rect.width() * 0.26,
                rect.top() + rect.height() * 0.36,
            )
        };
        draw_white_callout_arrow(painter, start, end);
    }
}

fn draw_dotted_vertical_guide(
    painter: &egui::Painter,
    rect: egui::Rect,
    x: f32,
    color: egui::Color32,
) {
    let mut y = rect.top();
    while y < rect.bottom() {
        painter.line_segment(
            [
                egui::pos2(x, y),
                egui::pos2(x, (y + 8.0).min(rect.bottom())),
            ],
            egui::Stroke::new(1.5, color.gamma_multiply(0.75)),
        );
        y += 14.0;
    }
}

fn draw_stops_icebergs_label(
    painter: &egui::Painter,
    pos: egui::Pos2,
    event: &HeuristicOrderFlowEvent,
    color: egui::Color32,
) {
    let text = stops_icebergs_event_label(event);
    let style = stops_icebergs_badge_style(event);
    let rect = egui::Rect::from_min_size(pos, egui::vec2(72.0, 24.0));
    painter.rect_filled(
        rect,
        5.0,
        egui::Color32::from_rgba_premultiplied(8, 10, 12, 222),
    );
    let badge_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 14.0, rect.center().y),
        egui::vec2(15.0, 15.0),
    );
    match style.shape {
        StopsIcebergsBadgeShape::Circle => {
            painter.circle_filled(badge_rect.center(), 7.5, style.color);
        }
        StopsIcebergsBadgeShape::Square => {
            painter.rect_filled(badge_rect, 2.0, style.color);
        }
    }
    painter.text(
        badge_rect.center(),
        egui::Align2::CENTER_CENTER,
        style.glyph,
        egui::FontId::monospace(8.0),
        egui::Color32::from_rgb(245, 248, 252),
    );
    painter.rect_stroke(
        rect,
        5.0,
        egui::Stroke::new(1.0, color.gamma_multiply(0.85)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        egui::pos2(rect.left() + 29.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::monospace(15.0),
        egui::Color32::from_rgb(238, 242, 246),
    );
}

fn draw_white_callout_arrow(painter: &egui::Painter, start: egui::Pos2, end: egui::Pos2) {
    painter.line_segment(
        [start, end],
        egui::Stroke::new(9.0, egui::Color32::from_rgba_premultiplied(0, 0, 0, 150)),
    );
    painter.line_segment(
        [start, end],
        egui::Stroke::new(5.0, egui::Color32::from_rgb(245, 247, 250)),
    );
    let direction = (end - start).normalized();
    let normal = egui::vec2(-direction.y, direction.x);
    let head_a = end - direction * 24.0 + normal * 14.0;
    let head_b = end - direction * 24.0 - normal * 14.0;
    painter.line_segment(
        [head_a, end],
        egui::Stroke::new(5.0, egui::Color32::from_rgb(245, 247, 250)),
    );
    painter.line_segment(
        [head_b, end],
        egui::Stroke::new(5.0, egui::Color32::from_rgb(245, 247, 250)),
    );
}

fn stops_icebergs_event_label(event: &HeuristicOrderFlowEvent) -> String {
    let score = (event.score.clamp(0.0, 1.0) * 100.0).round() as u32;
    if event.kind.contains("Iceberg") {
        format!("E:{score}")
    } else {
        score.to_string()
    }
}

fn stops_icebergs_badge_style(event: &HeuristicOrderFlowEvent) -> StopsIcebergsBadgeStyle {
    if event.kind.contains("Iceberg") {
        StopsIcebergsBadgeStyle {
            glyph: "E",
            shape: StopsIcebergsBadgeShape::Square,
            color: egui::Color32::from_rgb(54, 176, 226),
        }
    } else {
        StopsIcebergsBadgeStyle {
            glyph: "S",
            shape: StopsIcebergsBadgeShape::Circle,
            color: egui::Color32::from_rgb(235, 28, 36),
        }
    }
}

fn show_tradermap_pro(
    ui: &mut egui::Ui,
    snapshot: &OrderFlowPreviewSnapshot,
    audit: Option<&ReplayAuditSnapshot>,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) {
    let Some(audit) = audit else {
        empty_state(ui, "No filtered flow map timeline is available");
        return;
    };
    let cells = replay_liquidity_cells(snapshot, audit, derived_window);
    let threshold = filtered_liquidity_threshold_for_cells(&cells);
    let filtered_cells = cells
        .iter()
        .copied()
        .filter(|(_, cell)| cell.displayed_size >= threshold && cell.displayed_size > 0)
        .collect::<Vec<_>>();
    let bbo = replay_bbo_transitions(snapshot, audit, derived_window);
    let bubbles = replay_volume_bubbles(snapshot, audit, derived_window);
    let dom_snapshot = replay_dom_snapshot(snapshot, derived_window);
    let Some((min_price, max_price)) = composite_price_range(&cells, &bubbles, &bbo, dom_snapshot)
    else {
        empty_state(ui, "No price ladder is available for the filtered map");
        return;
    };

    ui.horizontal_wrapped(|ui| {
        metric_label(ui, "Mode", "filtered synthetic instruments");
        metric_label(ui, "Threshold", &threshold.to_string());
        metric_label(ui, "Shown cells", &filtered_cells.len().to_string());
    });
    ui.add_space(8.0);
    let height = ui.available_height().clamp(300.0, 620.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(5, 8, 11));
    let (left_rect, profile_rect, right_rect) = tradermap_layout_rects(rect);
    draw_labeled_flow_panel(
        &painter,
        left_rect,
        "RAW MBO LIQUIDITY",
        audit.window_start_ts,
        audit.window_end_ts,
        min_price,
        max_price,
        &cells,
        &bbo,
        &bubbles,
    );
    if let Some(dom) = dom_snapshot {
        draw_dom_pro_surface(&painter, profile_rect, min_price, max_price, dom);
    } else {
        painter.rect_filled(profile_rect, 0.0, egui::Color32::from_rgb(10, 14, 16));
    }
    draw_labeled_flow_panel(
        &painter,
        right_rect,
        "FILTERED TRADERMAP",
        audit.window_start_ts,
        audit.window_end_ts,
        min_price,
        max_price,
        &filtered_cells,
        &bbo,
        &bubbles,
    );
    draw_tradermap_callout(&painter, left_rect, "No filter", false);
    draw_tradermap_callout(
        &painter,
        right_rect,
        &format!("orders size {threshold}+ only"),
        true,
    );
}

fn tradermap_layout_rects(rect: egui::Rect) -> (egui::Rect, egui::Rect, egui::Rect) {
    let gutter = 8.0;
    let profile_width = tradermap_profile_width(rect.width());
    let panel_width = ((rect.width() - profile_width - gutter * 2.0) * 0.5).max(80.0);
    let left_rect = egui::Rect::from_min_max(
        rect.min,
        egui::pos2((rect.left() + panel_width).min(rect.right()), rect.bottom()),
    );
    let profile_left = (left_rect.right() + gutter).min(rect.right());
    let profile_rect = egui::Rect::from_min_max(
        egui::pos2(profile_left, rect.top()),
        egui::pos2(
            (profile_left + profile_width).min(rect.right()),
            rect.bottom(),
        ),
    );
    let right_left = (profile_rect.right() + gutter).min(rect.right());
    let right_rect = egui::Rect::from_min_max(egui::pos2(right_left, rect.top()), rect.max);
    (left_rect, profile_rect, right_rect)
}

fn tradermap_profile_width(container_width: f32) -> f32 {
    (container_width * 0.16).clamp(96.0, 150.0)
}

fn draw_tradermap_callout(
    painter: &egui::Painter,
    rect: egui::Rect,
    label: &str,
    right_side: bool,
) {
    let label_width = (label.len() as f32 * 7.0 + 18.0).clamp(72.0, rect.width() * 0.72);
    let label_rect = if right_side {
        egui::Rect::from_min_size(
            egui::pos2(rect.right() - label_width - 10.0, rect.top() + 34.0),
            egui::vec2(label_width, 22.0),
        )
    } else {
        egui::Rect::from_min_size(
            egui::pos2(rect.left() + 10.0, rect.top() + 34.0),
            egui::vec2(label_width, 22.0),
        )
    };
    painter.rect_filled(
        label_rect,
        2.0,
        egui::Color32::from_rgba_premultiplied(238, 242, 246, 226),
    );
    painter.text(
        label_rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::monospace(10.0),
        egui::Color32::from_rgb(16, 18, 20),
    );
    let anchor = if right_side {
        egui::pos2(label_rect.left() + 8.0, label_rect.bottom())
    } else {
        egui::pos2(label_rect.right() - 8.0, label_rect.bottom())
    };
    let target = if right_side {
        egui::pos2(rect.center().x - 18.0, rect.center().y)
    } else {
        egui::pos2(rect.center().x + 18.0, rect.center().y)
    };
    painter.line_segment(
        [anchor, target],
        egui::Stroke::new(2.0, egui::Color32::from_rgb(238, 242, 246)),
    );
    painter.circle_filled(target, 3.5, egui::Color32::from_rgb(238, 242, 246));
}

fn show_dom_pro(
    ui: &mut egui::Ui,
    snapshot: &OrderFlowPreviewSnapshot,
    audit: Option<&ReplayAuditSnapshot>,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) {
    let Some(dom) = replay_dom_snapshot(snapshot, derived_window) else {
        empty_state(ui, "No DOM snapshot is available");
        return;
    };
    ui.horizontal_wrapped(|ui| {
        metric_label(ui, "Mode", "dense DOM ladder");
        metric_label(ui, "Levels", &dom.levels.len().to_string());
        metric_label(ui, "Quality", book_quality_label(dom.quality));
        if let Some(audit) = audit {
            metric_label(ui, "Cursor ts", &audit.cursor_ts.to_string());
        }
    });
    ui.add_space(8.0);
    let Some((min_price, max_price)) = composite_price_range(&[], &[], &[], Some(dom)) else {
        empty_state(ui, "No DOM price range is available");
        return;
    };
    let height = ui.available_height().clamp(340.0, 700.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    draw_dom_pro_surface(&painter, rect, min_price, max_price, dom);
}

fn show_replay_controls(ui: &mut egui::Ui, controls: &mut ReplayControlState) -> bool {
    let mut reload_requested = false;
    ui.horizontal_wrapped(|ui| {
        let label = if controls.playing { "Pause" } else { "Play" };
        if ui.button(label).clicked() {
            controls.playing = !controls.playing;
            if controls.playing && controls.cursor_fraction >= 1.0 {
                controls.cursor_fraction = 0.0;
            }
        }
        if ui.button("Step").clicked() {
            controls.cursor_fraction = (controls.cursor_fraction + 0.025).min(1.0);
        }
        if ui.button("Reset").clicked() {
            controls.cursor_fraction = 0.0;
            controls.playing = false;
        }
        if ui.button("Reload DBN").clicked() {
            reload_requested = true;
        }
        egui::ComboBox::from_id_salt("order_flow_replay_speed")
            .selected_text(format!(
                "{:.2}x",
                REPLAY_SPEEDS
                    .get(controls.speed_index)
                    .copied()
                    .unwrap_or(1.0)
            ))
            .show_ui(ui, |ui| {
                for (index, speed) in REPLAY_SPEEDS.iter().copied().enumerate() {
                    ui.selectable_value(&mut controls.speed_index, index, format!("{speed:.2}x"));
                }
            });
        egui::ComboBox::from_id_salt("order_flow_replay_window")
            .selected_text(REPLAY_WINDOWS[controls.window_index.min(REPLAY_WINDOWS.len() - 1)])
            .show_ui(ui, |ui| {
                for (index, label) in REPLAY_WINDOWS.iter().copied().enumerate() {
                    ui.selectable_value(&mut controls.window_index, index, label);
                }
            });
        ui.checkbox(&mut controls.follow, "Follow");
        ui.add(
            egui::Slider::new(&mut controls.cursor_fraction, 0.0..=1.0)
                .show_value(false)
                .text("cursor"),
        );
    });
    reload_requested
}

fn advance_replay_playback(ui: &egui::Ui, controls: &mut ReplayControlState) {
    if !controls.playing {
        return;
    }
    let dt = ui.input(|input| input.stable_dt).clamp(0.0, 0.25);
    let speed = REPLAY_SPEEDS
        .get(controls.speed_index)
        .copied()
        .unwrap_or(1.0);
    controls.cursor_fraction = (controls.cursor_fraction + dt * speed * 0.04).min(1.0);
    if controls.cursor_fraction >= 1.0 {
        controls.playing = false;
    }
    ui.ctx().request_repaint();
}

fn replay_audit(
    snapshot: &OrderFlowPreviewSnapshot,
    controls: &ReplayControlState,
) -> Option<ReplayAuditSnapshot> {
    let (min_ts, max_ts) = composite_time_range(snapshot)?;
    let cursor_ts = cursor_timestamp(min_ts, max_ts, controls.cursor_fraction);
    let (window_start_ts, window_end_ts) =
        replay_window_bounds(min_ts, max_ts, cursor_ts, controls);
    let checkpoint = replay_checkpoint_for_cursor(snapshot, cursor_ts);
    Some(ReplayAuditSnapshot {
        min_ts,
        max_ts,
        cursor_ts,
        window_start_ts,
        window_end_ts,
        checkpoint_event_index: checkpoint.map(|checkpoint| checkpoint.event_index),
        checkpoint_ts: checkpoint.map(|checkpoint| checkpoint.timestamp_ns),
        visible_cells: snapshot
            .visible_liquidity_cells
            .iter()
            .filter(|(key, _)| {
                key.interval_start_ns >= window_start_ts && key.interval_start_ns <= cursor_ts
            })
            .count(),
        total_cells: snapshot.visible_liquidity_cells.len(),
        visible_bubbles: snapshot
            .volume_bubbles
            .iter()
            .filter(|(key, _)| {
                key.interval_start_ns >= window_start_ts && key.interval_start_ns <= cursor_ts
            })
            .count(),
        total_bubbles: snapshot.volume_bubbles.len(),
        visible_bbo: snapshot
            .bbo_transitions
            .iter()
            .filter(|transition| {
                transition.ts_event >= window_start_ts && transition.ts_event <= cursor_ts
            })
            .count(),
        total_bbo: snapshot.bbo_transitions.len(),
        dom_levels: snapshot
            .dom_snapshot
            .as_ref()
            .map(|dom| dom.levels.len())
            .unwrap_or_default(),
        cursor_fraction: controls.cursor_fraction.clamp(0.0, 1.0),
    })
}

fn derived_replay_window_key(audit: &ReplayAuditSnapshot) -> DerivedReplayWindowKey {
    DerivedReplayWindowKey {
        start_ts: audit.window_start_ts,
        end_ts: audit.window_end_ts,
        cursor_ts: audit.cursor_ts,
        checkpoint_event_index: audit.checkpoint_event_index,
    }
}

fn replay_window_bounds(
    min_ts: u64,
    max_ts: u64,
    cursor_ts: u64,
    controls: &ReplayControlState,
) -> (u64, u64) {
    let Some(span) = replay_window_span_ns(controls) else {
        return (min_ts, max_ts);
    };
    let available_span = max_ts.saturating_sub(min_ts);
    if available_span <= span || available_span <= 1 {
        return (min_ts, max_ts);
    }
    let (before_cursor, after_cursor) = replay_window_cursor_offsets(span);
    let mut start_ts = cursor_ts.saturating_sub(before_cursor);
    let mut end_ts = cursor_ts.saturating_add(after_cursor);
    if start_ts < min_ts {
        let shift = min_ts - start_ts;
        start_ts = min_ts;
        end_ts = end_ts.saturating_add(shift).min(max_ts);
    }
    if end_ts > max_ts {
        let shift = end_ts - max_ts;
        end_ts = max_ts;
        start_ts = start_ts.saturating_sub(shift).max(min_ts);
    }
    if end_ts <= start_ts {
        end_ts = start_ts.saturating_add(1).min(max_ts);
    }
    (start_ts, end_ts)
}

fn replay_window_cursor_offsets(span: u64) -> (u64, u64) {
    if span <= 1 {
        return (1, 1);
    }
    let before_cursor = ((span as f64) * BOOKMAP_VISIBLE_CURSOR_FRACTION as f64).round() as u64;
    let before_cursor = before_cursor.clamp(1, span - 1);
    let after_cursor = span.saturating_sub(before_cursor).max(1);
    (before_cursor, after_cursor)
}

fn replay_window_span_ns(controls: &ReplayControlState) -> Option<u64> {
    match controls.window_index {
        1 => Some(10 * SECOND_NS),
        2 => Some(15 * SECOND_NS),
        3 => Some(30 * SECOND_NS),
        4 => Some(60 * SECOND_NS),
        5 => Some(120 * SECOND_NS),
        6 => Some(5 * 60 * SECOND_NS),
        7 => Some(30 * 60 * SECOND_NS),
        _ => None,
    }
}

fn replay_checkpoint_for_cursor(
    snapshot: &OrderFlowPreviewSnapshot,
    cursor_ts: u64,
) -> Option<ReplayCheckpointRecord> {
    snapshot
        .replay_checkpoints
        .iter()
        .copied()
        .filter(|checkpoint| checkpoint.timestamp_ns <= cursor_ts)
        .max_by_key(|checkpoint| checkpoint.timestamp_ns)
}

fn show_replay_audit(
    ui: &mut egui::Ui,
    snapshot: &OrderFlowPreviewSnapshot,
    audit: &ReplayAuditSnapshot,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
    derived_window_preview: Option<&DerivedReplayWindowPreview>,
    derived_window_loading: bool,
) {
    ui.horizontal_wrapped(|ui| {
        metric_label(ui, "Load", timeline_status_label(&snapshot.report));
        metric_label(ui, "Events", &snapshot.report.read_events.to_string());
        metric_label(ui, "Book", &snapshot.report.applied_book_events.to_string());
        metric_label(ui, "Prints", &snapshot.report.trade_prints.to_string());
        metric_label(ui, "Frames", &snapshot.heatmap_frame_count.to_string());
        metric_label(ui, "Checkpoints", &snapshot.checkpoint_count.to_string());
    });
    ui.add_space(5.0);
    ui.horizontal_wrapped(|ui| {
        metric_label(
            ui,
            "Cursor",
            &format!("{:.1}%", audit.cursor_fraction * 100.0),
        );
        metric_label(ui, "ts", &audit.cursor_ts.to_string());
        metric_label(
            ui,
            "Window",
            &format!("{}..{}", audit.window_start_ts, audit.window_end_ts),
        );
        if let Some(event_index) = audit.checkpoint_event_index {
            metric_label(ui, "Ckpt evt", &event_index.to_string());
        }
        if let Some(timestamp_ns) = audit.checkpoint_ts {
            metric_label(ui, "Ckpt ts", &timestamp_ns.to_string());
        }
        metric_label(
            ui,
            "Cells",
            &format!("{}/{}", audit.visible_cells, audit.total_cells),
        );
        metric_label(
            ui,
            "Bubbles",
            &format!("{}/{}", audit.visible_bubbles, audit.total_bubbles),
        );
        metric_label(
            ui,
            "BBO",
            &format!("{}/{}", audit.visible_bbo, audit.total_bbo),
        );
        metric_label(ui, "DOM", &audit.dom_levels.to_string());
        metric_label(
            ui,
            "CVD",
            &snapshot.flow_summary.cumulative_delta.to_string(),
        );
    });
    if let Some(window) = derived_window {
        ui.add_space(5.0);
        ui.horizontal_wrapped(|ui| {
            metric_label(ui, "Artifact", "derived window");
            metric_label(
                ui,
                "Cells",
                &window.visible_liquidity_cells.len().to_string(),
            );
            metric_label(ui, "Bubbles", &window.volume_bubbles.len().to_string());
            metric_label(ui, "BBO", &window.bbo_transitions.len().to_string());
            metric_label(
                ui,
                "DOM",
                &window
                    .dom_snapshot
                    .as_ref()
                    .map(|dom| dom.levels.len())
                    .unwrap_or_default()
                    .to_string(),
            );
            metric_label(
                ui,
                "Footprint",
                &window.top_footprint_cells.len().to_string(),
            );
            metric_label(
                ui,
                "Delta",
                &window.flow_summary.cumulative_delta.to_string(),
            );
            if let Some(checkpoint) = window.checkpoint {
                metric_label(ui, "Artifact ckpt", &checkpoint.event_index.to_string());
            }
        });
        if let Some((key, cell)) = window.footprint_poc {
            compact_status_row(
                ui,
                "Artifact POC",
                &format!(
                    "{} / {} vol / {} delta",
                    format_mbo_price(key.price),
                    cell.total_volume(),
                    cell.delta()
                ),
            );
        }
    } else if derived_window_loading {
        ui.add_space(5.0);
        compact_status_row(ui, "Artifact window", "loading derived cursor window");
    } else if let Some(DerivedReplayWindowPreview::Error { key, message }) = derived_window_preview
    {
        if *key == derived_replay_window_key(audit) {
            ui.add_space(5.0);
            compact_status_row(ui, "Artifact window", message);
        }
    }
    ui.add(
        egui::ProgressBar::new(audit.cursor_fraction)
            .show_percentage()
            .fill(egui::Color32::from_rgb(66, 116, 158)),
    );
}

fn draw_replay_timeline_panel(
    ui: &mut egui::Ui,
    snapshot: &OrderFlowPreviewSnapshot,
    audit: &ReplayAuditSnapshot,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
    controls: &ReplayControlState,
) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), 154.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(8, 11, 14));
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(44, 51, 58)),
        egui::StrokeKind::Inside,
    );
    let title_rect =
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.top() + 30.0));
    let controls_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 10.0, title_rect.bottom() + 6.0),
        egui::pos2(rect.right() - 10.0, title_rect.bottom() + 42.0),
    );
    let track_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 10.0, controls_rect.bottom() + 12.0),
        egui::pos2(rect.right() - 10.0, rect.bottom() - 12.0),
    );
    let cells = replay_liquidity_cells(snapshot, audit, derived_window);
    let bubbles = replay_volume_bubbles(snapshot, audit, derived_window);
    let bbo = replay_bbo_transitions(snapshot, audit, derived_window);
    let body_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left(), title_rect.bottom()),
        egui::pos2(rect.right(), rect.bottom()),
    );
    draw_replay_market_context(&painter, body_rect, audit, &cells, &bubbles, &bbo);
    draw_replay_panel_title(&painter, title_rect, snapshot, audit);
    draw_replay_controls_visual(&painter, controls_rect, controls, audit.cursor_fraction);
    painter.rect_filled(
        track_rect,
        0.0,
        egui::Color32::from_rgba_premultiplied(16, 18, 21, 230),
    );

    let max_volume = bubbles
        .iter()
        .map(|(_, bubble)| bubble.total_volume())
        .max()
        .unwrap_or(1);

    for (key, cell) in &cells {
        let x = x_for_timestamp(
            track_rect,
            audit.window_start_ts,
            audit.window_end_ts,
            key.interval_start_ns,
        );
        let height = (track_rect.height() * cell.intensity.clamp(0.05, 1.0)).max(2.0);
        let y = track_rect.bottom() - height;
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x, y), egui::pos2(x + 1.5, track_rect.bottom())),
            0.0,
            heatmap_color(key.side, cell.intensity),
        );
    }
    for (key, bubble) in &bubbles {
        let x = x_for_timestamp(
            track_rect,
            audit.window_start_ts,
            audit.window_end_ts,
            key.interval_start_ns,
        );
        let radius = bubble.radius(max_volume, 2.0, 7.0);
        painter.circle_filled(
            egui::pos2(x, track_rect.center().y),
            radius,
            bubble_color(*bubble),
        );
    }
    draw_replay_cursor(
        &painter,
        track_rect,
        audit.window_start_ts,
        audit.window_end_ts,
        audit.cursor_ts,
    );
}

fn draw_replay_market_context(
    painter: &egui::Painter,
    rect: egui::Rect,
    audit: &ReplayAuditSnapshot,
    cells: &[(LiquidityCellKey, LiquidityCell)],
    bubbles: &[(VolumeBubbleKey, VolumeBubble)],
    bbo: &[BboTransition],
) {
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(7, 10, 13));
    let Some((min_price, max_price)) = replay_market_context_price_range(cells, bubbles, bbo)
    else {
        return;
    };
    draw_heatmap_cell_surface(
        painter,
        rect,
        audit.window_start_ts,
        audit.window_end_ts,
        min_price,
        max_price,
        cells,
    );
    draw_bbo_paths(
        painter,
        rect,
        audit.window_start_ts,
        audit.window_end_ts,
        min_price,
        max_price,
        bbo,
    );
}

fn replay_market_context_price_range(
    cells: &[(LiquidityCellKey, LiquidityCell)],
    bubbles: &[(VolumeBubbleKey, VolumeBubble)],
    bbo: &[BboTransition],
) -> Option<(i64, i64)> {
    composite_price_range(cells, bubbles, bbo, None)
}

fn draw_replay_panel_title(
    painter: &egui::Painter,
    rect: egui::Rect,
    snapshot: &OrderFlowPreviewSnapshot,
    audit: &ReplayAuditSnapshot,
) {
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(25, 35, 40));
    painter.line_segment(
        [
            egui::pos2(rect.left(), rect.bottom() - 0.5),
            egui::pos2(rect.right(), rect.bottom() - 0.5),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(58, 68, 76)),
    );
    painter.text(
        egui::pos2(rect.left() + 10.0, rect.top() + 12.0),
        egui::Align2::LEFT_CENTER,
        "Record & Replay",
        egui::FontId::monospace(12.0),
        egui::Color32::from_rgb(196, 199, 205),
    );
    painter.text(
        egui::pos2(rect.right() - 10.0, rect.top() + 12.0),
        egui::Align2::RIGHT_CENTER,
        format!(
            "{}  checkpoints {}  window {} -> {}",
            timeline_status_label(&snapshot.report),
            snapshot.checkpoint_count,
            format_relative_ns(audit.window_start_ts.saturating_sub(audit.min_ts)),
            format_relative_ns(audit.window_end_ts.saturating_sub(audit.min_ts))
        ),
        egui::FontId::monospace(10.0),
        egui::Color32::from_rgb(118, 122, 128),
    );
}

fn draw_replay_controls_visual(
    painter: &egui::Painter,
    rect: egui::Rect,
    controls: &ReplayControlState,
    cursor_fraction: f32,
) {
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(28, 36, 42));
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(49, 61, 70)),
        egui::StrokeKind::Inside,
    );
    let menu_rect = egui::Rect::from_min_size(rect.min, egui::vec2(126.0, rect.height()));
    painter.rect_filled(menu_rect, 0.0, egui::Color32::from_rgb(42, 55, 64));
    painter.circle_stroke(
        egui::pos2(menu_rect.left() + 16.0, menu_rect.center().y),
        7.0,
        egui::Stroke::new(1.5, egui::Color32::from_rgb(178, 191, 201)),
    );
    painter.text(
        egui::pos2(menu_rect.left() + 32.0, menu_rect.center().y),
        egui::Align2::LEFT_CENTER,
        replay_window_visual_label(controls.window_index),
        egui::FontId::monospace(11.0),
        egui::Color32::from_rgb(218, 224, 230),
    );

    let center_y = rect.center().y;
    let mut x = menu_rect.right() + 22.0;
    for glyph in ["[]", "<<"] {
        painter.text(
            egui::pos2(x, center_y),
            egui::Align2::CENTER_CENTER,
            glyph,
            egui::FontId::monospace(18.0),
            egui::Color32::from_rgb(204, 211, 218),
        );
        x += 44.0;
    }
    let speed_label = replay_speed_visual_label(controls.speed_index);
    painter.text(
        egui::pos2(x, center_y),
        egui::Align2::CENTER_CENTER,
        speed_label,
        egui::FontId::monospace(15.0),
        egui::Color32::from_rgb(232, 236, 240),
    );
    x += 54.0;
    for glyph in [">>", "->", ">"] {
        painter.text(
            egui::pos2(x, center_y),
            egui::Align2::CENTER_CENTER,
            glyph,
            egui::FontId::monospace(18.0),
            egui::Color32::from_rgb(204, 211, 218),
        );
        x += 44.0;
    }

    let slider_rect = egui::Rect::from_min_max(
        egui::pos2(x + 4.0, center_y - 4.0),
        egui::pos2(rect.right() - 16.0, center_y + 4.0),
    );
    painter.rect_filled(slider_rect, 4.0, egui::Color32::from_rgb(63, 78, 88));
    let fill_rect = egui::Rect::from_min_max(
        slider_rect.min,
        egui::pos2(
            egui::lerp(
                slider_rect.left()..=slider_rect.right(),
                cursor_fraction.clamp(0.0, 1.0),
            ),
            slider_rect.bottom(),
        ),
    );
    painter.rect_filled(fill_rect, 4.0, egui::Color32::from_rgb(16, 172, 226));
    painter.circle_filled(
        egui::pos2(fill_rect.right(), slider_rect.center().y),
        6.0,
        egui::Color32::from_rgb(50, 184, 226),
    );
}

fn replay_speed_visual_label(speed_index: usize) -> String {
    let speed = REPLAY_SPEEDS.get(speed_index).copied().unwrap_or(1.0);
    if speed < 1.0 {
        format!("x{speed:.2}")
    } else if (speed.fract()).abs() < f32::EPSILON {
        format!("x{speed:.0}")
    } else {
        format!("x{speed:.1}")
    }
}

fn replay_window_visual_label(window_index: usize) -> &'static str {
    REPLAY_WINDOWS
        .get(window_index)
        .copied()
        .unwrap_or(REPLAY_WINDOWS[0])
}

fn feature_contract_row(ui: &mut egui::Ui, name: &str, detail: &str, ready: bool) {
    let row_height = 44.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), row_height),
        egui::Sense::hover(),
    );
    let color = if ready {
        egui::Color32::from_rgb(76, 205, 126)
    } else {
        egui::Color32::from_rgb(232, 178, 88)
    };
    ui.painter()
        .rect_filled(rect, 0.0, egui::Color32::from_rgb(18, 18, 18));
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, egui::vec2(4.0, row_height)),
        0.0,
        color,
    );
    ui.painter().text(
        egui::pos2(rect.left() + 10.0, rect.top() + 14.0),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::monospace(12.0),
        egui::Color32::from_rgb(230, 232, 235),
    );
    ui.painter().text(
        egui::pos2(rect.left() + 10.0, rect.bottom() - 13.0),
        egui::Align2::LEFT_CENTER,
        detail,
        egui::FontId::monospace(10.0),
        egui::Color32::from_rgb(150, 154, 160),
    );
    ui.painter().text(
        egui::pos2(rect.right() - 8.0, rect.top() + 14.0),
        egui::Align2::RIGHT_CENTER,
        if ready { "ready" } else { "planned" },
        egui::FontId::monospace(10.0),
        color,
    );
    ui.add_space(4.0);
}

fn replay_liquidity_cells(
    snapshot: &OrderFlowPreviewSnapshot,
    audit: &ReplayAuditSnapshot,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) -> Vec<(LiquidityCellKey, LiquidityCell)> {
    if let Some(window) = derived_window {
        window
            .visible_liquidity_cells
            .iter()
            .copied()
            .filter(|(key, _)| {
                key.interval_start_ns >= audit.window_start_ts
                    && key.interval_start_ns <= audit.cursor_ts
            })
            .collect()
    } else {
        snapshot
            .visible_liquidity_cells
            .iter()
            .copied()
            .filter(|(key, _)| {
                key.interval_start_ns >= audit.window_start_ts
                    && key.interval_start_ns <= audit.cursor_ts
            })
            .collect()
    }
}

fn replay_volume_bubbles(
    snapshot: &OrderFlowPreviewSnapshot,
    audit: &ReplayAuditSnapshot,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) -> Vec<(VolumeBubbleKey, VolumeBubble)> {
    if let Some(window) = derived_window {
        window
            .volume_bubbles
            .iter()
            .copied()
            .filter(|(key, _)| {
                key.interval_start_ns >= audit.window_start_ts
                    && key.interval_start_ns <= audit.cursor_ts
            })
            .collect()
    } else {
        snapshot
            .volume_bubbles
            .iter()
            .copied()
            .filter(|(key, _)| {
                key.interval_start_ns >= audit.window_start_ts
                    && key.interval_start_ns <= audit.cursor_ts
            })
            .collect()
    }
}

fn replay_ohlcv_bars(
    snapshot: &OrderFlowPreviewSnapshot,
    audit: &ReplayAuditSnapshot,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) -> Vec<OhlcvBarRecord> {
    if let Some(window) = derived_window {
        window
            .ohlcv_bars
            .iter()
            .filter(|bar| {
                bar.interval_start_ns >= audit.window_start_ts
                    && bar.interval_start_ns <= audit.cursor_ts
            })
            .cloned()
            .collect()
    } else {
        snapshot
            .ohlcv_bars
            .iter()
            .filter(|bar| {
                bar.interval_start_ns >= audit.window_start_ts
                    && bar.interval_start_ns <= audit.cursor_ts
            })
            .cloned()
            .collect()
    }
}

fn replay_bbo_transitions(
    snapshot: &OrderFlowPreviewSnapshot,
    audit: &ReplayAuditSnapshot,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) -> Vec<BboTransition> {
    if let Some(window) = derived_window {
        window
            .bbo_transitions
            .iter()
            .copied()
            .filter(|transition| {
                transition.ts_event >= audit.window_start_ts
                    && transition.ts_event <= audit.cursor_ts
            })
            .collect()
    } else {
        snapshot
            .bbo_transitions
            .iter()
            .copied()
            .filter(|transition| {
                transition.ts_event >= audit.window_start_ts
                    && transition.ts_event <= audit.cursor_ts
            })
            .collect()
    }
}

fn replay_footprint_cells(
    snapshot: &OrderFlowPreviewSnapshot,
    audit: &ReplayAuditSnapshot,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) -> Vec<(FootprintCellKey, FootprintCell)> {
    if let Some(window) = derived_window {
        window
            .top_footprint_cells
            .iter()
            .copied()
            .filter(|(key, _)| {
                key.interval_start_ns >= audit.window_start_ts
                    && key.interval_start_ns <= audit.cursor_ts
            })
            .collect()
    } else {
        snapshot
            .top_footprint_cells
            .iter()
            .copied()
            .filter(|(key, _)| {
                key.interval_start_ns >= audit.window_start_ts
                    && key.interval_start_ns <= audit.cursor_ts
            })
            .collect()
    }
}

fn replay_dom_snapshot<'a>(
    snapshot: &'a OrderFlowPreviewSnapshot,
    derived_window: Option<&'a DerivedReplayWindowSnapshot>,
) -> Option<&'a DomColumnSnapshot> {
    derived_window
        .and_then(|window| window.dom_snapshot.as_ref())
        .or(snapshot.dom_snapshot.as_ref())
}

fn bookmap_aligned_dom_snapshot(
    dom: Option<DomColumnSnapshot>,
    bbo: &[BboTransition],
    active_price: Option<MboPrice>,
) -> Option<DomColumnSnapshot> {
    let Some(latest_bbo) = bbo
        .iter()
        .rev()
        .copied()
        .find(|transition| transition.bid.is_some() || transition.ask.is_some())
    else {
        return dom;
    };
    let Some(bbo_midpoint) = midpoint_or_single_price(latest_bbo.bid, latest_bbo.ask) else {
        return dom;
    };
    let anchor = active_price
        .filter(|price| price.is_defined())
        .unwrap_or(bbo_midpoint);
    let allowed_distance = bookmap_dom_alignment_tolerance_fixed(latest_bbo);

    if let Some(dom) = dom {
        if let Some(dom_midpoint) = dom_midpoint_price(&dom) {
            if dom_midpoint.fixed().abs_diff(anchor.fixed()) <= allowed_distance as u64 {
                return Some(dom);
            }
        }
    }

    if bbo_midpoint.fixed().abs_diff(anchor.fixed()) > allowed_distance as u64 {
        return dom_snapshot_from_anchor_price(anchor, latest_bbo);
    }
    dom_snapshot_from_bbo_transition(latest_bbo)
}

fn bookmap_dom_alignment_tolerance_fixed(latest_bbo: BboTransition) -> i64 {
    let spread = match (latest_bbo.bid, latest_bbo.ask) {
        (Some(bid), Some(ask)) => ask.price.fixed().saturating_sub(bid.price.fixed()),
        _ => 0,
    };
    spread
        .max(COMPOSITE_SPREAD_MARKER_THRESHOLD)
        .clamp(COMPOSITE_SPREAD_MARKER_THRESHOLD, FIXED_PRICE_SCALE)
}

fn dom_midpoint_price(dom: &DomColumnSnapshot) -> Option<MboPrice> {
    midpoint_or_single_price(dom.best_bid, dom.best_ask).or_else(|| {
        let min = dom.levels.iter().map(|level| level.price.fixed()).min()?;
        let max = dom.levels.iter().map(|level| level.price.fixed()).max()?;
        Some(MboPrice::from_fixed(min.saturating_add(max) / 2))
    })
}

fn dom_snapshot_from_bbo_transition(transition: BboTransition) -> Option<DomColumnSnapshot> {
    let mut price_points = Vec::new();
    if let Some(level) = transition.bid {
        push_defined_price(&mut price_points, level.price);
    }
    if let Some(level) = transition.ask {
        push_defined_price(&mut price_points, level.price);
    }
    if price_points.is_empty() {
        return None;
    }
    price_points.sort_unstable();
    price_points.dedup();
    let tick = bookmap_inferred_tick_size(&price_points);
    let max_size = transition
        .bid
        .into_iter()
        .chain(transition.ask)
        .map(|level| level.size)
        .max()
        .unwrap_or(1)
        .max(1);
    let mut levels = Vec::new();
    if let Some(bid) = transition.bid {
        for offset in 0..BOOKMAP_FALLBACK_DOM_DEPTH_ROWS {
            let decay = (BOOKMAP_FALLBACK_DOM_DEPTH_ROWS - offset) as u64;
            levels.push(DomColumnLevel {
                price: MboPrice::from_fixed(
                    bid.price
                        .fixed()
                        .saturating_sub(tick.saturating_mul(offset as i64)),
                ),
                bid_size: bid.size.saturating_mul(decay) / BOOKMAP_FALLBACK_DOM_DEPTH_ROWS as u64,
                ask_size: 0,
                total_size: bid.size.saturating_mul(decay) / BOOKMAP_FALLBACK_DOM_DEPTH_ROWS as u64,
                side_hint: MboSide::Bid,
            });
        }
    }
    if let Some(ask) = transition.ask {
        for offset in 0..BOOKMAP_FALLBACK_DOM_DEPTH_ROWS {
            let decay = (BOOKMAP_FALLBACK_DOM_DEPTH_ROWS - offset) as u64;
            levels.push(DomColumnLevel {
                price: MboPrice::from_fixed(
                    ask.price
                        .fixed()
                        .saturating_add(tick.saturating_mul(offset as i64)),
                ),
                bid_size: 0,
                ask_size: ask.size.saturating_mul(decay) / BOOKMAP_FALLBACK_DOM_DEPTH_ROWS as u64,
                total_size: ask.size.saturating_mul(decay) / BOOKMAP_FALLBACK_DOM_DEPTH_ROWS as u64,
                side_hint: MboSide::Ask,
            });
        }
    }
    levels.sort_by(|left, right| right.price.fixed().cmp(&left.price.fixed()));
    Some(DomColumnSnapshot {
        instrument_id: transition.instrument_id,
        quality: BookQuality::Suspect,
        best_bid: transition.bid,
        best_ask: transition.ask,
        max_size,
        levels,
    })
}

fn dom_snapshot_from_anchor_price(
    anchor: MboPrice,
    source_transition: BboTransition,
) -> Option<DomColumnSnapshot> {
    if !anchor.is_defined() {
        return None;
    }
    let tick = match (source_transition.bid, source_transition.ask) {
        (Some(bid), Some(ask)) => ask.price.fixed().saturating_sub(bid.price.fixed()),
        _ => 0,
    }
    .max(FIXED_PRICE_SCALE / 4)
    .clamp(FIXED_PRICE_SCALE / 100, FIXED_PRICE_SCALE);
    let bid_size = source_transition
        .bid
        .map(|level| level.size)
        .unwrap_or(1)
        .max(1);
    let ask_size = source_transition
        .ask
        .map(|level| level.size)
        .unwrap_or(bid_size)
        .max(1);
    let bid = PriceLevel {
        price: anchor,
        size: bid_size,
    };
    let ask = PriceLevel {
        price: MboPrice::from_fixed(anchor.fixed().saturating_add(tick)),
        size: ask_size,
    };
    dom_snapshot_from_bbo_transition(BboTransition {
        ts_event: source_transition.ts_event,
        instrument_id: source_transition.instrument_id,
        bid: Some(bid),
        ask: Some(ask),
        spread: Some(tick as f64 / FIXED_PRICE_SCALE as f64),
        mid: anchor.as_f64(),
    })
}

fn replay_order_book_state(
    snapshot: &OrderFlowPreviewSnapshot,
    audit: Option<&ReplayAuditSnapshot>,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) -> (Vec<PriceLevel>, Vec<PriceLevel>, Option<OrderBookSnapshot>) {
    if let Some(dom) = derived_window.and_then(|window| window.dom_snapshot.as_ref()) {
        let (bid_levels, ask_levels, _, _, book_snapshot) = depth_state_from_dom(
            Some(dom),
            dom.instrument_id,
            audit.map(|audit| audit.cursor_ts).unwrap_or_default(),
        );
        return (bid_levels, ask_levels, book_snapshot);
    }

    let (bid_levels, ask_levels) = audit
        .and_then(|audit| price_levels_at_cursor(snapshot, audit.cursor_ts))
        .unwrap_or_else(|| (snapshot.bid_levels.clone(), snapshot.ask_levels.clone()));
    (bid_levels, ask_levels, snapshot.book_snapshot.clone())
}

fn replay_depth_levels(
    snapshot: &OrderFlowPreviewSnapshot,
    audit: Option<&ReplayAuditSnapshot>,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) -> Vec<DepthLevelSample> {
    if let Some(dom) = derived_window.and_then(|window| window.dom_snapshot.as_ref()) {
        return depth_levels_from_dom(dom);
    }
    audit
        .and_then(|audit| depth_levels_at_cursor(snapshot, audit.cursor_ts))
        .unwrap_or_else(|| snapshot.latest_depth_levels.clone())
}

fn depth_levels_from_dom(dom: &DomColumnSnapshot) -> Vec<DepthLevelSample> {
    let mut levels = Vec::with_capacity(dom.levels.len().saturating_mul(2));
    levels.extend(
        dom.levels
            .iter()
            .filter(|level| level.bid_size > 0)
            .map(|level| DepthLevelSample {
                side: MboSide::Bid,
                price: level.price,
                size: level.bid_size,
            }),
    );
    levels.extend(
        dom.levels
            .iter()
            .filter(|level| level.ask_size > 0)
            .map(|level| DepthLevelSample {
                side: MboSide::Ask,
                price: level.price,
                size: level.ask_size,
            }),
    );
    levels
}

fn replay_flow_summary(
    snapshot: &OrderFlowPreviewSnapshot,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) -> FlowSummary {
    derived_window
        .map(|window| window.flow_summary)
        .unwrap_or(snapshot.flow_summary)
}

fn replay_add_on_events(
    snapshot: &OrderFlowPreviewSnapshot,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) -> Vec<AddOnEventRecord> {
    if let Some(window) = derived_window {
        if !window.add_on_events.is_empty() {
            return window.add_on_events.clone();
        }
    }
    snapshot.add_on_events.clone()
}

fn draw_labeled_flow_panel(
    painter: &egui::Painter,
    rect: egui::Rect,
    label: &str,
    min_ts: u64,
    max_ts: u64,
    min_price: i64,
    max_price: i64,
    cells: &[(LiquidityCellKey, LiquidityCell)],
    bbo: &[BboTransition],
    bubbles: &[(VolumeBubbleKey, VolumeBubble)],
) {
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(8, 11, 14));
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(44, 51, 58)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        egui::pos2(rect.left() + 8.0, rect.top() + 14.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::monospace(12.0),
        egui::Color32::from_rgb(196, 199, 205),
    );
    let plot_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 4.0, rect.top() + 28.0),
        egui::pos2(rect.right() - 4.0, rect.bottom() - 4.0),
    );
    draw_chart_grid(painter, plot_rect, min_price, max_price);
    draw_heatmap_cell_surface(
        painter, plot_rect, min_ts, max_ts, min_price, max_price, cells,
    );
    draw_bbo_paths(
        painter, plot_rect, min_ts, max_ts, min_price, max_price, bbo,
    );
    draw_volume_bubbles(
        painter, plot_rect, min_ts, max_ts, min_price, max_price, bubbles,
    );
    draw_bbo_trade_markers(
        painter, plot_rect, min_ts, max_ts, min_price, max_price, bubbles, bbo,
    );
}

fn draw_nanosecond_zoom_header(
    painter: &egui::Painter,
    rect: egui::Rect,
    audit: &ReplayAuditSnapshot,
    bbo: &[BboTransition],
    bubbles: &[(VolumeBubbleKey, VolumeBubble)],
) {
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(18, 24, 28));
    painter.line_segment(
        [
            egui::pos2(rect.left(), rect.bottom() - 0.5),
            egui::pos2(rect.right(), rect.bottom() - 0.5),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(54, 62, 68)),
    );
    painter.text(
        egui::pos2(rect.left() + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "Nanosecond Zoom",
        egui::FontId::monospace(13.0),
        egui::Color32::from_rgb(210, 216, 222),
    );
    let detail = format!(
        "cursor {}  bbo {}  prints {}",
        format_nanosecond_axis_label(audit.cursor_ts),
        bbo.len(),
        bubbles.len()
    );
    painter.text(
        egui::pos2(rect.right() - 10.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        detail,
        egui::FontId::monospace(11.0),
        egui::Color32::from_rgb(138, 148, 158),
    );
}

fn draw_nanosecond_zoom_grid(painter: &egui::Painter, rect: egui::Rect) {
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(22, 31, 35));
    for index in 1..4 {
        let x = egui::lerp(rect.left()..=rect.right(), index as f32 / 4.0);
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(35, 45, 50)),
        );
    }
}

fn draw_nanosecond_bbo_paths(
    painter: &egui::Painter,
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    min_price: i64,
    max_price: i64,
    bbo: &[BboTransition],
) {
    let bid_points = bbo
        .iter()
        .filter_map(|transition| {
            transition.bid.map(|level| {
                egui::pos2(
                    x_for_timestamp(rect, min_ts, max_ts, transition.ts_event),
                    y_for_price(rect, min_price, max_price, level.price),
                )
            })
        })
        .collect::<Vec<_>>();
    let ask_points = bbo
        .iter()
        .filter_map(|transition| {
            transition.ask.map(|level| {
                egui::pos2(
                    x_for_timestamp(rect, min_ts, max_ts, transition.ts_event),
                    y_for_price(rect, min_price, max_price, level.price),
                )
            })
        })
        .collect::<Vec<_>>();
    let bid_steps = step_points(&bid_points);
    let ask_steps = step_points(&ask_points);
    draw_polyline(
        painter,
        &ask_steps,
        egui::Stroke::new(7.0, egui::Color32::from_rgba_premultiplied(132, 36, 36, 72)),
    );
    draw_polyline(
        painter,
        &bid_steps,
        egui::Stroke::new(7.0, egui::Color32::from_rgba_premultiplied(36, 132, 54, 72)),
    );
    draw_polyline(
        painter,
        &ask_steps,
        egui::Stroke::new(3.5, egui::Color32::from_rgb(190, 58, 56)),
    );
    draw_polyline(
        painter,
        &bid_steps,
        egui::Stroke::new(3.5, egui::Color32::from_rgb(62, 189, 86)),
    );
}

fn draw_nanosecond_zoom_axis(
    painter: &egui::Painter,
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    cursor_ts: u64,
) {
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(42, 56, 64));
    painter.line_segment(
        [
            egui::pos2(rect.left(), rect.top() + 0.5),
            egui::pos2(rect.right(), rect.top() + 0.5),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(64, 76, 84)),
    );
    for fraction in [0.0_f32, 1.0 / 3.0, 2.0 / 3.0, 1.0] {
        let x = egui::lerp(rect.left()..=rect.right(), fraction);
        painter.line_segment(
            [
                egui::pos2(x, rect.top()),
                egui::pos2(x, rect.bottom() - 5.0),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(72, 86, 96)),
        );
        let ts = min_ts
            .saturating_add(((max_ts.saturating_sub(min_ts)) as f32 * fraction).round() as u64);
        painter.text(
            egui::pos2(x + 6.0, rect.center().y + 2.0),
            egui::Align2::LEFT_CENTER,
            format_nanosecond_axis_label(ts),
            egui::FontId::monospace(19.0),
            egui::Color32::from_rgb(180, 190, 198),
        );
    }
    let cursor_x = x_for_timestamp(rect, min_ts, max_ts, cursor_ts);
    painter.circle_filled(
        egui::pos2(cursor_x, rect.top() + 5.0),
        4.0,
        egui::Color32::from_rgb(78, 190, 222),
    );
}

fn format_nanosecond_axis_label(ts: u64) -> String {
    let minute_ns = ts % 60_000_000_000;
    let seconds = minute_ns / 1_000_000_000;
    let nanos = minute_ns % 1_000_000_000;
    let millis = nanos / 1_000_000;
    let micros = nanos % 1_000_000;
    format!("{seconds}.{millis:03},{micros:06}")
}

fn draw_micro_event_ticks(
    painter: &egui::Painter,
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    bbo: &[BboTransition],
    bubbles: &[(VolumeBubbleKey, VolumeBubble)],
) {
    let top_lane = egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.top() + 28.0));
    let bottom_lane =
        egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 28.0), rect.max);
    for transition in bbo {
        let x = x_for_timestamp(rect, min_ts, max_ts, transition.ts_event);
        painter.line_segment(
            [
                egui::pos2(x, top_lane.top()),
                egui::pos2(x, top_lane.bottom()),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(90, 171, 206)),
        );
    }
    let max_volume = bubbles
        .iter()
        .map(|(_, bubble)| bubble.total_volume())
        .max()
        .unwrap_or(1);
    for (key, bubble) in bubbles {
        let x = x_for_timestamp(rect, min_ts, max_ts, key.interval_start_ns);
        let height = bottom_lane.height()
            * (bubble.total_volume() as f32 / max_volume as f32)
                .sqrt()
                .clamp(0.05, 1.0);
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x - 1.0, bottom_lane.bottom() - height),
                egui::pos2(x + 1.0, bottom_lane.bottom()),
            ),
            0.0,
            bubble_color(*bubble),
        );
    }
}

fn draw_dom_pro_surface(
    painter: &egui::Painter,
    rect: egui::Rect,
    min_price: i64,
    max_price: i64,
    dom: &DomColumnSnapshot,
) {
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(8, 8, 8));
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(44, 51, 58)),
        egui::StrokeKind::Inside,
    );
    let header_height = 26.0;
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + header_height),
        rect.max,
    );
    let headers = ["PRICE", "COB", "CVP", "SVP", "LTQ", "PIQ"];
    let fractions = [0.22, 0.18, 0.18, 0.18, 0.12, 0.12];
    let columns = dom_columns(rect, &fractions);
    for (index, header) in headers.iter().enumerate() {
        painter.text(
            columns[index].center_top() + egui::vec2(0.0, 14.0),
            egui::Align2::CENTER_CENTER,
            *header,
            egui::FontId::monospace(11.0),
            egui::Color32::from_rgb(196, 199, 205),
        );
        if index > 0 {
            painter.line_segment(
                [
                    egui::pos2(columns[index].left(), body.top()),
                    egui::pos2(columns[index].left(), body.bottom()),
                ],
                egui::Stroke::new(1.0, egui::Color32::from_rgb(28, 34, 38)),
            );
        }
    }

    let max_size = dom.max_size.max(1);
    for level in &dom.levels {
        let y = y_for_price(body, min_price, max_price, level.price);
        let row_height = price_row_height(body, min_price, max_price).clamp(14.0, 24.0);
        let row = egui::Rect::from_center_size(
            egui::pos2(body.center().x, y),
            egui::vec2(body.width(), row_height),
        )
        .intersect(body);
        painter.rect_filled(row, 0.0, egui::Color32::from_rgb(15, 15, 15));
        painter.text(
            egui::pos2(columns[0].center().x, row.center().y),
            egui::Align2::CENTER_CENTER,
            format_mbo_price(level.price),
            egui::FontId::monospace(10.0),
            egui::Color32::from_rgb(230, 232, 235),
        );
        draw_dom_size_cell(
            painter,
            columns[1].intersect(row),
            level.bid_size,
            level.ask_size,
            max_size,
            true,
        );
        draw_dom_profile_cell(
            painter,
            columns[2].intersect(row),
            level.total_size,
            max_size,
            level.side_hint,
            "CVP",
            true,
        );
        draw_dom_profile_cell(
            painter,
            columns[3].intersect(row),
            level.total_size,
            max_size,
            level.side_hint,
            "SVP",
            true,
        );
        draw_dom_text_cell(
            painter,
            columns[4].intersect(row),
            &(level.total_size / 3).max(1).to_string(),
        );
        draw_dom_text_cell(painter, columns[5].intersect(row), "--");
    }
}

fn dom_columns(rect: egui::Rect, fractions: &[f32]) -> Vec<egui::Rect> {
    let mut columns = Vec::with_capacity(fractions.len());
    let mut left = rect.left();
    for fraction in fractions {
        let right = (left + rect.width() * *fraction).min(rect.right());
        columns.push(egui::Rect::from_min_max(
            egui::pos2(left, rect.top()),
            egui::pos2(right, rect.bottom()),
        ));
        left = right;
    }
    if let Some(last) = columns.last_mut() {
        last.max.x = rect.right();
    }
    columns
}

fn draw_dom_size_cell(
    painter: &egui::Painter,
    rect: egui::Rect,
    bid_size: u64,
    ask_size: u64,
    max_size: u64,
    show_text: bool,
) {
    let center_x = rect.center().x;
    let bid_width =
        (rect.width() * 0.48 * bid_size as f32 / max_size as f32).clamp(0.0, rect.width() * 0.48);
    let ask_width =
        (rect.width() * 0.48 * ask_size as f32 / max_size as f32).clamp(0.0, rect.width() * 0.48);
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(center_x - bid_width, rect.top()),
            egui::pos2(center_x, rect.bottom()),
        ),
        0.0,
        egui::Color32::from_rgba_premultiplied(44, 205, 122, 214),
    );
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(center_x, rect.top()),
            egui::pos2(center_x + ask_width, rect.bottom()),
        ),
        0.0,
        egui::Color32::from_rgba_premultiplied(234, 62, 72, 214),
    );
    if show_text && bookmap_dom_cell_text_fits(rect) {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            format!("{bid_size}|{ask_size}"),
            egui::FontId::monospace(9.0),
            egui::Color32::from_rgb(230, 232, 235),
        );
    }
}

fn draw_dom_profile_cell(
    painter: &egui::Painter,
    rect: egui::Rect,
    value: u64,
    max_size: u64,
    side: MboSide,
    label: &str,
    show_text: bool,
) {
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(21, 25, 28));
    let fill_rect = dom_profile_fill_rect(rect, value, max_size, side);
    let normalized = (value as f32 / max_size.max(1) as f32)
        .sqrt()
        .clamp(0.0, 1.0);
    let color = dom_profile_fill_color(side, normalized);
    painter.rect_filled(fill_rect, 0.0, color);
    if normalized > 0.32 {
        let cap_width = (fill_rect.width() * 0.22).clamp(1.0, 6.0);
        let cap = match side {
            MboSide::Bid => egui::Rect::from_min_max(
                egui::pos2(fill_rect.right() - cap_width, fill_rect.top()),
                fill_rect.right_bottom(),
            ),
            MboSide::Ask => egui::Rect::from_min_max(
                fill_rect.left_top(),
                egui::pos2(fill_rect.left() + cap_width, fill_rect.bottom()),
            ),
            MboSide::None | MboSide::Unknown(_) => fill_rect.shrink2(egui::vec2(
                (fill_rect.width() * 0.38).min(fill_rect.width() * 0.5),
                0.0,
            )),
        };
        painter.rect_filled(
            cap,
            0.0,
            egui::Color32::from_rgba_premultiplied(245, 248, 250, 74),
        );
    }
    painter.line_segment(
        [
            egui::pos2(rect.center().x, rect.top()),
            egui::pos2(rect.center().x, rect.bottom()),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(42, 47, 52)),
    );
    if show_text && bookmap_dom_cell_text_fits(rect) {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            if label.is_empty() {
                value.to_string()
            } else {
                format!("{label} {value}")
            },
            egui::FontId::monospace(9.0),
            egui::Color32::from_rgb(196, 199, 205),
        );
    }
}

fn dom_profile_fill_color(side: MboSide, normalized: f32) -> egui::Color32 {
    let alpha = (114.0 + 118.0 * normalized).round().clamp(96.0, 236.0) as u8;
    match side {
        MboSide::Bid => egui::Color32::from_rgba_premultiplied(34, 216, 116, alpha),
        MboSide::Ask => egui::Color32::from_rgba_premultiplied(235, 62, 76, alpha),
        MboSide::None | MboSide::Unknown(_) => {
            egui::Color32::from_rgba_premultiplied(126, 132, 140, alpha)
        }
    }
}

fn dom_profile_fill_rect(rect: egui::Rect, value: u64, max_size: u64, side: MboSide) -> egui::Rect {
    if value == 0 || max_size == 0 {
        return egui::Rect::from_min_max(rect.center(), rect.center());
    }
    let half_width =
        (rect.width() * 0.48 * value as f32 / max_size as f32).clamp(0.0, rect.width() * 0.48);
    match side {
        MboSide::Bid => egui::Rect::from_min_max(
            egui::pos2(rect.center().x, rect.top()),
            egui::pos2(rect.center().x + half_width, rect.bottom()),
        ),
        MboSide::Ask => egui::Rect::from_min_max(
            egui::pos2(rect.center().x - half_width, rect.top()),
            egui::pos2(rect.center().x, rect.bottom()),
        ),
        MboSide::None | MboSide::Unknown(_) => {
            let width = (rect.width() * value as f32 / max_size as f32).clamp(0.0, rect.width());
            egui::Rect::from_center_size(rect.center(), egui::vec2(width, rect.height()))
        }
    }
}

fn draw_dom_text_cell(painter: &egui::Painter, rect: egui::Rect, text: &str) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::monospace(9.0),
        egui::Color32::from_rgb(150, 154, 160),
    );
}

fn depth_levels_at_cursor(
    snapshot: &OrderFlowPreviewSnapshot,
    cursor_ts: u64,
) -> Option<Vec<DepthLevelSample>> {
    snapshot
        .depth_frames
        .iter()
        .filter(|frame| frame.key.interval_start_ns <= cursor_ts)
        .max_by_key(|frame| frame.key.interval_start_ns)
        .map(|frame| frame.levels.clone())
}

fn price_levels_at_cursor(
    snapshot: &OrderFlowPreviewSnapshot,
    cursor_ts: u64,
) -> Option<(Vec<PriceLevel>, Vec<PriceLevel>)> {
    let levels = depth_levels_at_cursor(snapshot, cursor_ts)?;
    let bids = levels
        .iter()
        .filter(|level| level.side == MboSide::Bid && level.size > 0)
        .map(|level| PriceLevel {
            price: level.price,
            size: level.size,
        })
        .collect::<Vec<_>>();
    let asks = levels
        .iter()
        .filter(|level| level.side == MboSide::Ask && level.size > 0)
        .map(|level| PriceLevel {
            price: level.price,
            size: level.size,
        })
        .collect::<Vec<_>>();
    Some((bids, asks))
}

fn draw_bookmap_top_strip(
    painter: &egui::Painter,
    rect: egui::Rect,
    snapshot: &OrderFlowPreviewSnapshot,
    audit: &ReplayAuditSnapshot,
    using_derived: bool,
    controls: &ReplayControlState,
    layers: &BookmapLayerState,
) {
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(51, 64, 70));
    painter.line_segment(
        [
            egui::pos2(rect.left(), rect.bottom() - 0.5),
            egui::pos2(rect.right(), rect.bottom() - 0.5),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(28, 36, 40)),
    );

    let clip = rect.shrink2(egui::vec2(8.0, 0.0));
    let clipped = painter.with_clip_rect(clip);
    let menu_height = 24.0;
    let toolbar_height = 45.0_f32.min((rect.height() - menu_height).max(28.0) * 0.58);
    let tab_height = (rect.height() - menu_height - toolbar_height).max(22.0);
    let menu_rect =
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.top() + menu_height));
    let toolbar_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left(), menu_rect.bottom()),
        egui::pos2(rect.right(), menu_rect.bottom() + toolbar_height),
    );
    let tab_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left(), toolbar_rect.bottom()),
        egui::pos2(rect.right(), rect.bottom()),
    );

    painter.rect_filled(menu_rect, 0.0, egui::Color32::from_rgb(61, 76, 83));
    painter.rect_filled(toolbar_rect, 0.0, egui::Color32::from_rgb(48, 61, 67));
    painter.rect_filled(tab_rect, 0.0, egui::Color32::from_rgb(55, 68, 74));
    painter.line_segment(
        [
            egui::pos2(rect.left(), toolbar_rect.bottom()),
            egui::pos2(rect.right(), toolbar_rect.bottom()),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(36, 48, 54)),
    );

    let menu_y = menu_rect.center().y;
    let mut x = rect.left() + 10.0;
    for item in ["File", "Connections", "Settings", "Help"] {
        clipped.text(
            egui::pos2(x, menu_y),
            egui::Align2::LEFT_CENTER,
            item,
            egui::FontId::proportional(13.0),
            egui::Color32::from_rgb(220, 225, 228),
        );
        x += item.len() as f32 * 8.0 + 18.0;
    }

    clipped.text(
        egui::pos2(rect.right() - 108.0, menu_y),
        egui::Align2::LEFT_CENTER,
        "BOOKMAP",
        egui::FontId::proportional(18.0),
        egui::Color32::from_rgb(232, 235, 238),
    );

    clipped.text(
        egui::pos2(rect.right() - 196.0, menu_y),
        egui::Align2::LEFT_CENTER,
        "Feedback",
        egui::FontId::proportional(12.0),
        egui::Color32::from_rgb(216, 222, 226),
    );

    let mut icon_x = toolbar_rect.left() + 14.0;
    for (index, glyph) in ["+", "o", "<", "#", "x", "/", "p", "@", "+", "t", "b", "L"]
        .iter()
        .enumerate()
    {
        let center = egui::pos2(icon_x + 12.0, toolbar_rect.center().y);
        draw_toolbar_icon(&clipped, center, glyph, index % 5 == 2 || index % 7 == 0);
        icon_x += if index == 5 { 44.0 } else { 34.0 };
    }

    let legend_rect = egui::Rect::from_min_size(
        egui::pos2(icon_x + 10.0, toolbar_rect.top() + 8.0),
        egui::vec2(168.0, 14.0),
    )
    .intersect(toolbar_rect);
    draw_bookmap_heat_legend(&clipped, legend_rect);
    draw_bookmap_slider(
        &clipped,
        egui::Rect::from_min_size(
            egui::pos2(legend_rect.left(), legend_rect.bottom() + 8.0),
            egui::vec2(168.0, 5.0),
        )
        .intersect(toolbar_rect),
        egui::Color32::from_rgb(67, 174, 232),
    );

    let replay_rects = bookmap_replay_control_rects(rect);
    draw_bookmap_replay_control_chip(
        &clipped,
        replay_rects.play_rect,
        if controls.playing { "PAUSE" } else { "PLAY" },
        controls.playing,
    );
    draw_bookmap_replay_control_chip(&clipped, replay_rects.step_rect, "STEP", false);
    draw_bookmap_replay_control_chip(&clipped, replay_rects.reset_rect, "RST", false);
    clipped.text(
        egui::pos2(
            replay_rects.reset_rect.right() + 10.0,
            toolbar_rect.center().y,
        ),
        egui::Align2::LEFT_CENTER,
        format!(
            "{} {:.0}%",
            replay_window_visual_label(controls.window_index),
            (audit.cursor_fraction * 100.0).round()
        ),
        egui::FontId::monospace(11.0),
        egui::Color32::from_rgb(126, 190, 226),
    );

    let status = if using_derived { "DERIVED" } else { "PREVIEW" };
    let data_label = format!(
        "{}  {}  events {}  inst {}  layers {}",
        status,
        snapshot.instrument_selection.chrome_label(),
        snapshot.event_count,
        snapshot.instrument_count,
        layers.enabled_count()
    );
    draw_top_strip_meter(
        &clipped,
        egui::Rect::from_min_size(
            egui::pos2(rect.right() - 306.0, toolbar_rect.top() + 12.0),
            egui::vec2(70.0, 15.0),
        ),
        "BUY",
        egui::Color32::from_rgb(52, 184, 111),
    );
    draw_top_strip_meter(
        &clipped,
        egui::Rect::from_min_size(
            egui::pos2(rect.right() - 228.0, toolbar_rect.top() + 12.0),
            egui::vec2(70.0, 15.0),
        ),
        "SELL",
        egui::Color32::from_rgb(210, 66, 82),
    );

    let warning_rect = egui::Rect::from_min_size(
        egui::pos2(tab_rect.left() + 224.0, tab_rect.top() + 4.0),
        egui::vec2(
            (tab_rect.width() * 0.48).clamp(260.0, 620.0),
            tab_height - 7.0,
        ),
    )
    .intersect(tab_rect);
    if snapshot.warning.is_some() || !using_derived {
        painter.rect_filled(
            warning_rect,
            0.0,
            egui::Color32::from_rgba_premultiplied(78, 99, 107, 215),
        );
        clipped.text(
            warning_rect.left_center() + egui::vec2(10.0, 0.0),
            egui::Align2::LEFT_CENTER,
            "Market data is replayed/derived; live execution is disabled",
            egui::FontId::monospace(12.0),
            egui::Color32::from_rgb(214, 44, 52),
        );
    }

    let tab_labels = [
        snapshot.instrument_selection.chrome_label(),
        "ESM6.CME@BMD".to_string(),
        "AAPL@DXFEED".to_string(),
    ];
    let mut tab_x = tab_rect.left();
    for (index, label) in tab_labels.iter().enumerate() {
        let width = (if index == 0 { 250.0_f32 } else { 205.0_f32 }).min(tab_rect.right() - tab_x);
        if width <= 24.0 {
            break;
        }
        let tab = egui::Rect::from_min_max(
            egui::pos2(tab_x, tab_rect.top()),
            egui::pos2(tab_x + width, tab_rect.bottom()),
        );
        painter.rect_filled(
            tab,
            0.0,
            if index == 0 {
                egui::Color32::from_rgb(48, 62, 69)
            } else {
                egui::Color32::from_rgb(54, 67, 73)
            },
        );
        painter.line_segment(
            [
                egui::pos2(tab.right(), tab.top()),
                egui::pos2(tab.right(), tab.bottom()),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(38, 48, 54)),
        );
        clipped.text(
            tab.left_center() + egui::vec2(18.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::monospace(13.0),
            egui::Color32::from_rgb(216, 222, 226),
        );
        if index == 0 {
            painter.line_segment(
                [
                    egui::pos2(tab.left(), tab.bottom() - 2.0),
                    egui::pos2(tab.right(), tab.bottom() - 2.0),
                ],
                egui::Stroke::new(2.0, egui::Color32::from_rgb(45, 166, 224)),
            );
        }
        tab_x += width;
    }

    clipped.text(
        egui::pos2(rect.right() - 470.0, tab_rect.center().y),
        egui::Align2::LEFT_CENTER,
        data_label,
        egui::FontId::monospace(10.0),
        egui::Color32::from_rgb(205, 214, 218),
    );
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct BookmapReplayControlRects {
    play_rect: egui::Rect,
    step_rect: egui::Rect,
    reset_rect: egui::Rect,
}

fn bookmap_replay_control_rects(rect: egui::Rect) -> BookmapReplayControlRects {
    let menu_height = 24.0;
    let toolbar_height = 45.0_f32.min((rect.height() - menu_height).max(28.0) * 0.58);
    let toolbar_rect = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + menu_height),
        egui::pos2(rect.right(), rect.top() + menu_height + toolbar_height),
    );
    let height = 22.0_f32.min(toolbar_rect.height() - 8.0).max(18.0);
    let y = toolbar_rect.center().y - height * 0.5;
    let play_rect = egui::Rect::from_min_size(
        egui::pos2(rect.right() - 538.0, y),
        egui::vec2(58.0, height),
    );
    let step_rect = egui::Rect::from_min_size(
        egui::pos2(play_rect.right() + 4.0, y),
        egui::vec2(52.0, height),
    );
    let reset_rect = egui::Rect::from_min_size(
        egui::pos2(step_rect.right() + 4.0, y),
        egui::vec2(46.0, height),
    );
    BookmapReplayControlRects {
        play_rect,
        step_rect,
        reset_rect,
    }
}

fn draw_bookmap_replay_control_chip(
    painter: &egui::Painter,
    rect: egui::Rect,
    label: &str,
    active: bool,
) {
    let fill = if active {
        egui::Color32::from_rgb(48, 96, 78)
    } else {
        egui::Color32::from_rgb(42, 55, 62)
    };
    painter.rect_filled(rect, 2.0, fill);
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(73, 93, 104)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::monospace(10.0),
        egui::Color32::from_rgb(221, 230, 235),
    );
}

fn draw_bookmap_left_rail(painter: &egui::Painter, rect: egui::Rect, layers: &BookmapLayerState) {
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(50, 64, 70));
    painter.line_segment(
        [
            egui::pos2(rect.right(), rect.top()),
            egui::pos2(rect.right(), rect.bottom()),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 55, 60)),
    );
    painter.text(
        egui::pos2(rect.center().x, rect.top() + 14.0),
        egui::Align2::CENTER_CENTER,
        "TCP",
        egui::FontId::proportional(13.0),
        egui::Color32::from_rgb(218, 225, 230),
    );

    let mut y = rect.top() + 32.0;
    draw_rail_label_value(painter, rect, &mut y, "Account", "None");
    draw_rail_button(painter, rect, &mut y, "TRADING: OFF", false, 36.0);
    draw_rail_label_value(painter, rect, &mut y, "<NONE>", "");
    draw_rail_button(painter, rect, &mut y, "START", false, 28.0);
    draw_rail_label_value(painter, rect, &mut y, "Round:", "TO");
    for label in ["P:", "A:", "U:", "T:", "V:"] {
        draw_rail_metric(painter, rect, &mut y, label, "-");
    }
    y += 8.0;
    draw_rail_label_value(painter, rect, &mut y, "Size:", "0");
    draw_rail_button_grid(
        painter,
        rect,
        &mut y,
        &["0.001", "0.005", "0.01", "0.02", "0.05", "0.1"],
    );
    draw_rail_button(painter, rect, &mut y, "Unknown", false, 28.0);
    draw_rail_button(painter, rect, &mut y, "CANCEL ALL", false, 28.0);
    draw_rail_button(painter, rect, &mut y, "Unknown", false, 28.0);
    draw_rail_button(painter, rect, &mut y, "FLATTEN", false, 30.0);
    draw_rail_button_grid(
        painter,
        rect,
        &mut y,
        &["MKT", "MKT", "ASK", "ASK", "BID", "BID"],
    );
    draw_rail_button(painter, rect, &mut y, "REVERSE", false, 28.0);
    for label in ["OCO", "Brackets", "TP:", "SL:", "TS:"] {
        draw_rail_checkbox(painter, rect, &mut y, label);
    }

    for (kind, row) in bookmap_left_rail_layer_toggle_rects(rect) {
        let active = layers.is_enabled(kind);
        painter.rect_filled(
            row,
            1.0,
            if active {
                egui::Color32::from_rgb(42, 93, 76)
            } else {
                egui::Color32::from_rgb(43, 55, 61)
            },
        );
        painter.text(
            row.center(),
            egui::Align2::CENTER_CENTER,
            kind.short_label(),
            egui::FontId::monospace(8.0),
            egui::Color32::from_rgb(178, 190, 196),
        );
    }
}

fn bookmap_left_rail_layer_toggle_rects(rect: egui::Rect) -> Vec<(BookmapLayerKind, egui::Rect)> {
    let mut layer_y = (rect.bottom() - 138.0).max(rect.top() + 32.0);
    let mut rects = Vec::new();
    for kind in BookmapLayerKind::ALL.iter().take(5).copied() {
        if layer_y > rect.bottom() - 8.0 {
            break;
        }
        let row = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 6.0, layer_y),
            egui::vec2(rect.width() - 12.0, 18.0),
        );
        rects.push((kind, row));
        layer_y += 20.0;
    }
    rects
}

fn draw_toolbar_icon(painter: &egui::Painter, center: egui::Pos2, glyph: &str, active: bool) {
    let rect = egui::Rect::from_center_size(center, egui::vec2(27.0, 27.0));
    if active {
        painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(59, 85, 99));
        painter.rect_stroke(
            rect,
            2.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(68, 179, 238)),
            egui::StrokeKind::Inside,
        );
    }
    painter.text(
        center,
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::monospace(15.0),
        egui::Color32::from_rgb(205, 214, 220),
    );
}

fn draw_bookmap_slider(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    if rect.width() <= 2.0 || rect.height() <= 2.0 {
        return;
    }
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(85, 104, 112));
    let fill = egui::Rect::from_min_max(
        rect.min,
        egui::pos2(rect.left() + rect.width() * 0.62, rect.bottom()),
    );
    painter.rect_filled(fill, 0.0, color);
    painter.circle_filled(
        egui::pos2(fill.right(), rect.center().y),
        5.0,
        egui::Color32::from_rgb(65, 170, 226),
    );
}

fn draw_rail_label_value(
    painter: &egui::Painter,
    rect: egui::Rect,
    y: &mut f32,
    label: &str,
    value: &str,
) {
    let row = rail_row_rect(rect, *y, 26.0);
    painter.text(
        row.left_center() + egui::vec2(6.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::monospace(10.0),
        egui::Color32::from_rgb(150, 164, 172),
    );
    let value_rect = egui::Rect::from_min_max(
        egui::pos2(row.left() + row.width() * 0.40, row.top() + 2.0),
        egui::pos2(row.right() - 6.0, row.bottom() - 2.0),
    );
    painter.rect_filled(value_rect, 1.0, egui::Color32::from_rgb(59, 75, 82));
    painter.rect_stroke(
        value_rect,
        1.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(86, 103, 111)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        value_rect.center(),
        egui::Align2::CENTER_CENTER,
        value,
        egui::FontId::monospace(10.0),
        egui::Color32::from_rgb(213, 221, 226),
    );
    *y += 28.0;
}

fn draw_rail_metric(
    painter: &egui::Painter,
    rect: egui::Rect,
    y: &mut f32,
    label: &str,
    value: &str,
) {
    let row = rail_row_rect(rect, *y, 18.0);
    painter.line_segment(
        [row.left_bottom(), row.right_bottom()],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(64, 79, 86)),
    );
    painter.text(
        row.left_center() + egui::vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::monospace(10.0),
        egui::Color32::from_rgb(214, 222, 226),
    );
    painter.text(
        row.right_center() - egui::vec2(8.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        value,
        egui::FontId::monospace(10.0),
        egui::Color32::from_rgb(214, 222, 226),
    );
    *y += 18.0;
}

fn draw_rail_button(
    painter: &egui::Painter,
    rect: egui::Rect,
    y: &mut f32,
    label: &str,
    enabled: bool,
    height: f32,
) {
    let row = rail_row_rect(rect, *y, height);
    let fill = if enabled {
        egui::Color32::from_rgb(58, 103, 83)
    } else {
        egui::Color32::from_rgb(69, 87, 95)
    };
    painter.rect_filled(row.shrink2(egui::vec2(4.0, 2.0)), 1.0, fill);
    painter.text(
        row.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::monospace(10.0),
        if enabled {
            egui::Color32::from_rgb(226, 236, 230)
        } else {
            egui::Color32::from_rgb(145, 159, 167)
        },
    );
    *y += height;
}

fn draw_rail_button_grid(painter: &egui::Painter, rect: egui::Rect, y: &mut f32, labels: &[&str]) {
    let cols = 2;
    let cell_h = 27.0;
    let cell_w = (rect.width() - 8.0) / cols as f32;
    for (index, label) in labels.iter().enumerate() {
        let col = index % cols;
        let row = index / cols;
        let cell = egui::Rect::from_min_size(
            egui::pos2(
                rect.left() + 4.0 + col as f32 * cell_w,
                *y + row as f32 * cell_h,
            ),
            egui::vec2(cell_w - 1.0, cell_h - 1.0),
        );
        painter.rect_filled(cell, 0.0, egui::Color32::from_rgb(63, 81, 89));
        painter.text(
            cell.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            egui::FontId::monospace(10.0),
            egui::Color32::from_rgb(150, 165, 172),
        );
    }
    *y += ((labels.len() + cols - 1) / cols) as f32 * cell_h;
}

fn draw_rail_checkbox(painter: &egui::Painter, rect: egui::Rect, y: &mut f32, label: &str) {
    let row = rail_row_rect(rect, *y, 22.0);
    let box_rect = egui::Rect::from_min_size(
        row.left_center() + egui::vec2(6.0, -6.0),
        egui::vec2(12.0, 12.0),
    );
    painter.rect_stroke(
        box_rect,
        1.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(79, 96, 104)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        row.left_center() + egui::vec2(26.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::monospace(10.0),
        egui::Color32::from_rgb(141, 157, 165),
    );
    *y += 22.0;
}

fn rail_row_rect(rect: egui::Rect, y: f32, height: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(rect.left(), y),
        egui::pos2(rect.right(), (y + height).min(rect.bottom())),
    )
}

fn draw_bookmap_heat_legend(painter: &egui::Painter, rect: egui::Rect) {
    if rect.width() <= 2.0 || rect.height() <= 2.0 {
        return;
    }
    let segments = 30;
    for index in 0..segments {
        let t0 = index as f32 / segments as f32;
        let t1 = (index + 1) as f32 / segments as f32;
        let x0 = egui::lerp(rect.left()..=rect.right(), t0);
        let x1 = egui::lerp(rect.left()..=rect.right(), t1);
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.bottom())),
            0.0,
            heatmap_color(MboSide::None, t0),
        );
    }
    painter.rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 84, 88)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.left_top() + egui::vec2(0.0, -4.0),
        egui::Align2::LEFT_BOTTOM,
        "LIQUIDITY",
        egui::FontId::monospace(9.0),
        egui::Color32::from_rgb(140, 145, 152),
    );
}

fn draw_top_strip_meter(
    painter: &egui::Painter,
    rect: egui::Rect,
    label: &str,
    color: egui::Color32,
) {
    painter.rect_filled(rect, 1.0, egui::Color32::from_rgb(28, 30, 32));
    let fill = egui::Rect::from_min_max(rect.min, egui::pos2(rect.center().x, rect.bottom()));
    painter.rect_filled(fill, 1.0, color.gamma_multiply(0.65));
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::monospace(9.0),
        egui::Color32::from_rgb(222, 225, 230),
    );
}

fn draw_bookmap_time_axis(
    painter: &egui::Painter,
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    cursor_ts: u64,
) {
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(9, 11, 13));
    painter.line_segment(
        [
            egui::pos2(rect.left(), rect.top() + 0.5),
            egui::pos2(rect.right(), rect.top() + 0.5),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(54, 58, 64)),
    );
    for index in 0..=6 {
        let fraction = index as f32 / 6.0;
        let x = egui::lerp(rect.left()..=rect.right(), fraction);
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.top() + 7.0)],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(82, 86, 92)),
        );
        let ts = min_ts
            .saturating_add(((max_ts.saturating_sub(min_ts)) as f32 * fraction).round() as u64);
        painter.text(
            egui::pos2(x + 3.0, rect.center().y + 2.0),
            egui::Align2::LEFT_CENTER,
            format!("+{}", format_relative_ns(ts.saturating_sub(min_ts))),
            egui::FontId::monospace(10.0),
            egui::Color32::from_rgb(144, 151, 160),
        );
    }
    let cursor_x = x_for_timestamp(rect, min_ts, max_ts, cursor_ts);
    painter.line_segment(
        [
            egui::pos2(cursor_x, rect.top()),
            egui::pos2(cursor_x, rect.bottom()),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(235, 235, 70)),
    );
}

fn draw_bookmap_cursor_tooltip(
    painter: &egui::Painter,
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    min_price: i64,
    max_price: i64,
    cursor_ts: u64,
    bars: &[OhlcvBarRecord],
    bbo: &[BboTransition],
) {
    if rect.width() <= 120.0 || rect.height() <= 80.0 {
        return;
    }
    let x = x_for_timestamp(rect, min_ts, max_ts, cursor_ts);
    let marker_price = nearest_cursor_price(cursor_ts, bars, bbo)
        .unwrap_or_else(|| MboPrice::from_fixed((min_price + max_price) / 2));
    let y = y_for_price(rect, min_price, max_price, marker_price);
    let tooltip = egui::Rect::from_min_size(
        egui::pos2(
            (x + 18.0).min(rect.right() - 168.0),
            (y - 56.0).max(rect.top() + 8.0),
        ),
        egui::vec2(164.0, 50.0),
    )
    .intersect(rect.shrink(6.0));
    painter.rect_filled(
        tooltip,
        4.0,
        egui::Color32::from_rgba_premultiplied(83, 98, 106, 222),
    );
    let side_price = nearest_bbo_transition(bbo, cursor_ts)
        .and_then(|transition| transition.ask.or(transition.bid))
        .map(|level| format_mbo_price(level.price))
        .unwrap_or_else(|| format_mbo_price(marker_price));
    painter.text(
        tooltip.left_top() + egui::vec2(10.0, 10.0),
        egui::Align2::LEFT_TOP,
        format!("{}  {}", replay_time_label(cursor_ts), side_price),
        egui::FontId::monospace(11.0),
        egui::Color32::from_rgb(236, 241, 244),
    );
    let detail = nearest_ohlcv_bar(cursor_ts, bars)
        .map(|bar| format!("VOL {}  DELTA {}", bar.volume, bar.delta))
        .unwrap_or_else(|| "ASK/BID replay cursor".to_string());
    painter.text(
        tooltip.left_bottom() + egui::vec2(10.0, -10.0),
        egui::Align2::LEFT_BOTTOM,
        detail,
        egui::FontId::monospace(10.0),
        egui::Color32::from_rgb(222, 229, 233),
    );
}

fn draw_bookmap_status_strip(
    painter: &egui::Painter,
    rect: egui::Rect,
    snapshot: &OrderFlowPreviewSnapshot,
    audit: &ReplayAuditSnapshot,
) {
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(49, 65, 72));
    painter.line_segment(
        [rect.left_top(), rect.right_top()],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(34, 45, 50)),
    );
    let left = format!(
        "{}  {}  cursor {}",
        replay_time_label(audit.cursor_ts),
        snapshot.instrument_selection.chrome_label(),
        format_relative_ns(audit.cursor_ts.saturating_sub(audit.min_ts))
    );
    painter.text(
        rect.left_center() + egui::vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        left,
        egui::FontId::monospace(11.0),
        egui::Color32::from_rgb(229, 235, 238),
    );
    painter.text(
        rect.right_center() - egui::vec2(8.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        "BMD/DX     Data: Replayed     Trading: Simulated",
        egui::FontId::monospace(11.0),
        egui::Color32::from_rgb(229, 235, 238),
    );
}

fn nearest_cursor_price(
    cursor_ts: u64,
    bars: &[OhlcvBarRecord],
    bbo: &[BboTransition],
) -> Option<MboPrice> {
    nearest_ohlcv_bar(cursor_ts, bars)
        .map(|bar| MboPrice::from_fixed(bar.close_fixed))
        .or_else(|| {
            nearest_bbo_transition(bbo, cursor_ts)
                .and_then(|transition| midpoint_or_single_price(transition.bid, transition.ask))
        })
}

fn nearest_ohlcv_bar(cursor_ts: u64, bars: &[OhlcvBarRecord]) -> Option<&OhlcvBarRecord> {
    bars.iter()
        .min_by_key(|bar| bar.interval_start_ns.abs_diff(cursor_ts))
}

fn replay_time_label(timestamp_ns: u64) -> String {
    let seconds = timestamp_ns / SECOND_NS;
    let minutes = (seconds / 60) % 60;
    let hours = (seconds / 3_600) % 24;
    let millis = (timestamp_ns % SECOND_NS) / 1_000_000;
    format!("{hours:02}:{minutes:02}:{millis:03}")
}

fn format_relative_ns(ns: u64) -> String {
    if ns < 1_000 {
        format!("{ns}ns")
    } else if ns < 1_000_000 {
        format!("{:.1}us", ns as f64 / 1_000.0)
    } else if ns < 1_000_000_000 {
        format!("{:.1}ms", ns as f64 / 1_000_000.0)
    } else if ns < 60_000_000_000 {
        format!("{:.1}s", ns as f64 / 1_000_000_000.0)
    } else {
        format!("{:.1}m", ns as f64 / 60_000_000_000.0)
    }
}

fn bookmap_terminal_visible_price_range(
    frame: &BookmapFrame,
    ohlcv_bars: &[OhlcvBarRecord],
    visible_bubbles: &[(VolumeBubbleKey, VolumeBubble)],
) -> Option<(i64, i64)> {
    let fallback = frame
        .viewport
        .price_range
        .map(|range| (range.min.fixed(), range.max.fixed()));
    let mut prices = Vec::new();

    if let Some(dom) = frame.layers.dom_snapshot.as_ref() {
        for level in &dom.levels {
            push_defined_price(&mut prices, level.price);
        }
        if let Some(level) = dom.best_bid {
            push_defined_price(&mut prices, level.price);
        }
        if let Some(level) = dom.best_ask {
            push_defined_price(&mut prices, level.price);
        }
    }

    for transition in &frame.layers.bbo_transitions {
        if let Some(level) = transition.bid {
            push_defined_price(&mut prices, level.price);
        }
        if let Some(level) = transition.ask {
            push_defined_price(&mut prices, level.price);
        }
        if let Some(price) = midpoint_or_single_price(transition.bid, transition.ask) {
            push_defined_price(&mut prices, price);
        }
    }

    for (key, _) in visible_bubbles {
        push_defined_price(&mut prices, key.price);
    }

    for bar in ohlcv_bars {
        prices.extend([
            bar.open_fixed,
            bar.high_fixed,
            bar.low_fixed,
            bar.close_fixed,
        ]);
    }

    for band in &frame.layers.current_book_bands {
        push_defined_price(&mut prices, band.price);
    }

    let anchor = nearest_cursor_price(
        frame.viewport.cursor_ts,
        ohlcv_bars,
        &frame.layers.bbo_transitions,
    )
    .or_else(|| {
        composite_last_price_marker(
            visible_bubbles,
            &frame.layers.bbo_transitions,
            frame.layers.dom_snapshot.as_ref(),
        )
    })
    .map(|price| price.fixed());

    bookmap_compact_visible_price_range(prices, anchor).or(fallback)
}

fn push_defined_price(prices: &mut Vec<i64>, price: MboPrice) {
    if price.is_defined() {
        prices.push(price.fixed());
    }
}

fn bookmap_compact_visible_price_range(
    mut prices: Vec<i64>,
    anchor: Option<i64>,
) -> Option<(i64, i64)> {
    if prices.is_empty() {
        return None;
    }
    prices.sort_unstable();
    prices.dedup();
    let tick = bookmap_inferred_tick_size(&prices);
    let low = sorted_percentile(&prices, 3, 100);
    let high = sorted_percentile(&prices, 97, 100);
    let anchor = anchor
        .filter(|value| *value != MboPrice::UNDEFINED.fixed())
        .unwrap_or_else(|| low.saturating_add(high.saturating_sub(low) / 2));
    let observed_span = high.saturating_sub(low).max(tick);
    let min_span = tick.saturating_mul(BOOKMAP_TERMINAL_MIN_VISIBLE_TICKS);
    let max_span = tick
        .saturating_mul(BOOKMAP_TERMINAL_MAX_VISIBLE_TICKS)
        .max(min_span);
    let padded_span = observed_span
        .saturating_add(tick.saturating_mul(BOOKMAP_TERMINAL_PRICE_PADDING_TICKS))
        .clamp(min_span, max_span);

    let (mut min_price, mut max_price) = if observed_span <= padded_span {
        let pad = padded_span.saturating_sub(observed_span) / 2;
        (low.saturating_sub(pad), high.saturating_add(pad))
    } else {
        let half = padded_span / 2;
        (anchor.saturating_sub(half), anchor.saturating_add(half))
    };

    (min_price, max_price) = bookmap_anchor_price_comfort_range(min_price, max_price, anchor);

    if max_price <= min_price {
        max_price = min_price.saturating_add(tick.max(1));
    }
    Some((min_price, max_price))
}

fn bookmap_anchor_price_comfort_range(
    mut min_price: i64,
    mut max_price: i64,
    anchor: i64,
) -> (i64, i64) {
    if max_price <= min_price {
        return (min_price, max_price);
    }
    let span = max_price.saturating_sub(min_price);
    let comfort_min =
        min_price.saturating_add(span.saturating_mul(BOOKMAP_CURRENT_PRICE_COMFORT_MIN) / 100);
    let comfort_max =
        min_price.saturating_add(span.saturating_mul(BOOKMAP_CURRENT_PRICE_COMFORT_MAX) / 100);
    if anchor < comfort_min {
        let shift = comfort_min.saturating_sub(anchor);
        min_price = min_price.saturating_sub(shift);
        max_price = max_price.saturating_sub(shift);
    } else if anchor > comfort_max {
        let shift = anchor.saturating_sub(comfort_max);
        min_price = min_price.saturating_add(shift);
        max_price = max_price.saturating_add(shift);
    }
    (min_price, max_price)
}

fn bookmap_inferred_tick_size(sorted_prices: &[i64]) -> i64 {
    sorted_prices
        .windows(2)
        .filter_map(|pair| {
            let diff = pair[1].saturating_sub(pair[0]);
            (diff > 0).then_some(diff)
        })
        .min()
        .unwrap_or(FIXED_PRICE_SCALE / 4)
        .clamp(FIXED_PRICE_SCALE / 100, FIXED_PRICE_SCALE)
}

fn sorted_percentile(sorted_values: &[i64], numerator: usize, denominator: usize) -> i64 {
    debug_assert!(!sorted_values.is_empty());
    let denominator = denominator.max(1);
    let index =
        ((sorted_values.len().saturating_sub(1)) * numerator.min(denominator)) / denominator;
    sorted_values[index]
}

fn bookmap_dom_width(container_width: f32) -> f32 {
    if container_width <= 520.0 {
        return (container_width * 0.30).clamp(112.0, 136.0);
    }
    (container_width * 0.17).clamp(170.0, 330.0)
}

fn bookmap_left_rail_width(container_width: f32) -> f32 {
    if container_width <= 700.0 {
        return (container_width * 0.16).clamp(76.0, 140.0);
    }
    (container_width * 0.12).clamp(172.0, 230.0)
}

fn draw_chart_grid(painter: &egui::Painter, rect: egui::Rect, min_price: i64, max_price: i64) {
    let minor_time_stroke =
        egui::Stroke::new(1.0, egui::Color32::from_rgba_premultiplied(36, 44, 48, 72));
    for x in bookmap_minor_time_marker_positions(rect) {
        painter.line_segment(
            [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
            minor_time_stroke,
        );
    }

    for (index, y) in bookmap_price_ladder_y_positions(rect, min_price, max_price)
        .into_iter()
        .enumerate()
    {
        let color = if index % 4 == 3 {
            egui::Color32::from_rgba_premultiplied(58, 66, 70, 92)
        } else {
            egui::Color32::from_rgba_premultiplied(24, 32, 36, 70)
        };
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            egui::Stroke::new(1.0, color),
        );
    }

    let major_stroke = egui::Stroke::new(
        1.0,
        egui::Color32::from_rgba_premultiplied(94, 102, 108, 150),
    );
    for x in bookmap_major_time_marker_positions(rect) {
        draw_dotted_vertical_line(painter, rect, x, major_stroke);
    }
}

fn bookmap_minor_time_marker_positions(rect: egui::Rect) -> Vec<f32> {
    let divisions = if rect.width() >= 900.0 {
        16
    } else if rect.width() >= 600.0 {
        12
    } else {
        8
    };
    (1..divisions)
        .map(|index| egui::lerp(rect.left()..=rect.right(), index as f32 / divisions as f32))
        .collect()
}

fn bookmap_major_time_marker_positions(rect: egui::Rect) -> Vec<f32> {
    let divisions = if rect.width() >= 900.0 {
        8
    } else if rect.width() >= 600.0 {
        6
    } else {
        4
    };
    (1..divisions)
        .map(|index| egui::lerp(rect.left()..=rect.right(), index as f32 / divisions as f32))
        .collect()
}

fn bookmap_price_ladder_row_count(rect: egui::Rect, min_price: i64, max_price: i64) -> usize {
    if rect.height() <= 0.0 {
        return 0;
    }
    let row_height = price_row_height(rect, min_price, max_price).clamp(5.0, 14.0);
    (rect.height() / row_height).floor().clamp(6.0, 96.0) as usize
}

fn bookmap_price_ladder_y_positions(rect: egui::Rect, min_price: i64, max_price: i64) -> Vec<f32> {
    let rows = bookmap_price_ladder_row_count(rect, min_price, max_price);
    (1..rows)
        .map(|index| egui::lerp(rect.top()..=rect.bottom(), index as f32 / rows as f32))
        .collect()
}

fn draw_dotted_vertical_line(
    painter: &egui::Painter,
    rect: egui::Rect,
    x: f32,
    stroke: egui::Stroke,
) {
    let dash = 4.0;
    let gap = 5.0;
    let mut y = rect.top();
    while y < rect.bottom() {
        let y_end = (y + dash).min(rect.bottom());
        painter.line_segment([egui::pos2(x, y), egui::pos2(x, y_end)], stroke);
        y += dash + gap;
    }
}

fn draw_heatmap_surface(
    painter: &egui::Painter,
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    min_price: i64,
    max_price: i64,
    bands: &[BookmapLiquidityBand],
) {
    draw_persistent_heatmap_backdrop(painter, rect, min_price, max_price, bands);
    let bucket_width = heatmap_bucket_width(rect, min_ts, max_ts, bands);
    let row_height = price_row_height(rect, min_price, max_price);
    for band in bands {
        if band.displayed_size == 0 {
            continue;
        }
        let x = x_for_timestamp(rect, min_ts, max_ts, band.start_ts);
        let x1 = x_for_timestamp(rect, min_ts, max_ts, band.end_ts)
            .max(x + bucket_width)
            .min(rect.right());
        let y = y_for_price(rect, min_price, max_price, band.price);
        let cell_rect = egui::Rect::from_min_max(
            egui::pos2(x, y - row_height * 0.5),
            egui::pos2(x1, y + row_height * 0.5),
        )
        .intersect(rect);
        painter.rect_filled(
            cell_rect,
            0.0,
            heatmap_event_color(band.side, band.normalized_intensity),
        );
    }
    for cell in bookmap_heatmap_texture_rects(
        rect,
        min_ts,
        max_ts,
        min_price,
        max_price,
        bands,
        BOOKMAP_HEATMAP_TEXTURE_MAX_RECTS,
    ) {
        painter.rect_filled(cell.rect, 0.0, cell.color);
    }
}

#[derive(Debug, Clone, Copy)]
struct BookmapHeatmapTextureRect {
    rect: egui::Rect,
    color: egui::Color32,
    #[allow(dead_code)]
    source_price: MboPrice,
    #[allow(dead_code)]
    source_ts: u64,
}

fn bookmap_heatmap_texture_rects(
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    min_price: i64,
    max_price: i64,
    bands: &[BookmapLiquidityBand],
    limit: usize,
) -> Vec<BookmapHeatmapTextureRect> {
    if limit == 0 || rect.width() <= 2.0 || rect.height() <= 2.0 || max_ts <= min_ts {
        return Vec::new();
    }
    let mut source_bands = bands
        .iter()
        .enumerate()
        .filter(|(_, band)| band.displayed_size > 0 && band.price.is_defined())
        .collect::<Vec<_>>();
    source_bands.sort_by(|(left_index, left), (right_index, right)| {
        bookmap_heatmap_texture_priority(right)
            .partial_cmp(&bookmap_heatmap_texture_priority(left))
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.start_ts.cmp(&right.start_ts))
            .then_with(|| left.price.fixed().cmp(&right.price.fixed()))
            .then_with(|| left_index.cmp(right_index))
    });
    let source_limit = (limit / 2).clamp(256, 8_000).min(source_bands.len());
    if source_bands.len() > source_limit {
        let priority_take = (source_limit / 2).max(1);
        let mut selected = BTreeSet::<usize>::new();
        for (index, _) in source_bands.iter().take(priority_take) {
            selected.insert(*index);
        }

        let coverage_target = source_limit.saturating_sub(selected.len()).max(1);
        let mut coverage_sources = source_bands.clone();
        coverage_sources.sort_by(|(_, left), (_, right)| {
            left.start_ts
                .cmp(&right.start_ts)
                .then_with(|| left.price.fixed().cmp(&right.price.fixed()))
                .then_with(|| side_sort_key(left.side).cmp(&side_sort_key(right.side)))
        });
        let stride = coverage_sources.len() as f32 / coverage_target as f32;
        for sample in 0..coverage_target {
            let source_index = (sample as f32 * stride).floor() as usize;
            if let Some((index, _)) =
                coverage_sources.get(source_index.min(coverage_sources.len() - 1))
            {
                selected.insert(*index);
            }
        }
        source_bands.retain(|(index, _)| selected.contains(index));
    }
    source_bands.sort_by(|(_, left), (_, right)| {
        left.start_ts
            .cmp(&right.start_ts)
            .then_with(|| left.price.fixed().cmp(&right.price.fixed()))
            .then_with(|| side_sort_key(left.side).cmp(&side_sort_key(right.side)))
    });

    let max_size = source_bands
        .iter()
        .map(|(_, band)| band.displayed_size)
        .max()
        .unwrap_or(1)
        .max(1);
    let row_height = price_row_height(rect, min_price, max_price).clamp(2.2, 11.0);
    let step_width = bookmap_heatmap_texture_step_width(rect);
    let mut cells = Vec::with_capacity(limit.min(2_048));
    let source_count = source_bands.len().max(1);
    let base_band_budget = (limit / source_count).clamp(1, 36);
    for (_, band) in source_bands {
        if cells.len() >= limit {
            break;
        }
        let x0 =
            x_for_timestamp(rect, min_ts, max_ts, band.start_ts).clamp(rect.left(), rect.right());
        let x1 = x_for_timestamp(rect, min_ts, max_ts, band.end_ts)
            .max(x0 + step_width)
            .min(rect.right());
        if x1 <= x0 {
            continue;
        }
        let y = y_for_price(rect, min_price, max_price, band.price);
        let span = x1 - x0;
        let steps = ((span / step_width).ceil() as usize).clamp(1, 224);
        let density = bookmap_heatmap_texture_density(band, max_size);
        let band_budget = (base_band_budget as f32 * (0.55 + 0.65 * density))
            .round()
            .clamp(1.0, 64.0) as usize;
        let step_stride = ((steps as f32 / band_budget as f32).ceil() as usize).max(1);
        for step in (0..steps).step_by(step_stride) {
            if cells.len() >= limit {
                break;
            }
            let hash = bookmap_heatmap_texture_hash(band, step as u64);
            let keep = ((hash & 0x3ff) as f32) / 1023.0;
            let keep_threshold = (0.56 + 0.42 * density).clamp(0.58, 0.97);
            if keep > keep_threshold {
                continue;
            }
            let jitter_x = (((hash >> 10) & 0xff) as f32) / 255.0;
            let jitter_y = (((hash >> 18) & 0xff) as f32) / 255.0;
            let jitter_w = (((hash >> 26) & 0xff) as f32) / 255.0;
            let jitter_row = (((hash >> 34) & 0x7) as f32 - 3.0) / 3.0;
            let x_left = x0 + span * step as f32 / steps as f32 + step_width * jitter_x * 0.28;
            let x_right = (x_left + step_width * (0.52 + 0.84 * jitter_w))
                .min(x1)
                .max(x_left + 1.2);
            let center_y =
                y + (jitter_y - 0.5) * row_height * 0.70 + jitter_row * row_height * 0.34;
            let intensity = (band.normalized_intensity * (0.76 + 0.44 * jitter_w) + 0.10 * density)
                .clamp(0.04, 1.0);
            let height =
                (row_height * (0.28 + 1.08 * intensity.sqrt())).clamp(1.2, row_height * 1.45);
            let cell_rect = egui::Rect::from_min_max(
                egui::pos2(x_left, center_y - height * 0.5),
                egui::pos2(x_right, center_y + height * 0.5),
            )
            .intersect(rect);
            if cell_rect.width() <= 0.0 || cell_rect.height() <= 0.0 {
                continue;
            }
            cells.push(BookmapHeatmapTextureRect {
                rect: cell_rect,
                color: bookmap_heatmap_texture_color(band.side, intensity, density),
                source_price: band.price,
                source_ts: band.start_ts,
            });
        }
    }
    cells
}

fn bookmap_heatmap_texture_step_width(rect: egui::Rect) -> f32 {
    if rect.width() >= 1_200.0 {
        2.6
    } else if rect.width() >= 800.0 {
        3.2
    } else {
        4.2
    }
}

fn bookmap_heatmap_texture_priority(band: &BookmapLiquidityBand) -> f32 {
    let size = (band.displayed_size.max(1) as f32).ln_1p();
    size * (0.35 + 0.65 * band.normalized_intensity.clamp(0.0, 1.0))
}

fn bookmap_heatmap_texture_density(band: &BookmapLiquidityBand, max_size: u64) -> f32 {
    let size_t = (band.displayed_size as f32 / max_size.max(1) as f32)
        .sqrt()
        .clamp(0.0, 1.0);
    (0.36 + 0.46 * band.normalized_intensity.sqrt() + 0.18 * size_t).clamp(0.42, 0.96)
}

fn bookmap_heatmap_texture_hash(band: &BookmapLiquidityBand, segment: u64) -> u64 {
    let mut value = band.start_ts
        ^ band.end_ts.rotate_left(17)
        ^ (band.price.fixed() as u64).rotate_left(29)
        ^ band.displayed_size.rotate_left(7)
        ^ ((side_sort_key(band.side) as u64) << 56)
        ^ segment.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    value ^= value >> 30;
    value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn bookmap_heatmap_texture_color(_side: MboSide, intensity: f32, density: f32) -> egui::Color32 {
    let t = (0.10 + 0.88 * intensity.clamp(0.0, 1.0) * (0.76 + 0.24 * density)).clamp(0.0, 1.0);
    let base = heatmap_color(MboSide::None, t);
    let alpha = (38.0 + 172.0 * intensity.sqrt() * density.clamp(0.2, 1.0))
        .round()
        .clamp(30.0, 218.0) as u8;
    egui::Color32::from_rgba_premultiplied(base.r(), base.g(), base.b(), alpha)
}

#[derive(Debug, Clone, Copy)]
struct PersistentHeatmapLevel {
    price: MboPrice,
    side: MboSide,
    displayed_size: u64,
    peak_intensity: f32,
}

fn draw_persistent_heatmap_backdrop(
    painter: &egui::Painter,
    rect: egui::Rect,
    min_price: i64,
    max_price: i64,
    bands: &[BookmapLiquidityBand],
) {
    let limit = if rect.height() >= 540.0 { 96 } else { 64 };
    let levels = persistent_heatmap_levels(bands, limit);
    let max_size = levels
        .iter()
        .map(|level| level.displayed_size)
        .max()
        .unwrap_or(1)
        .max(1);
    let row_height = price_row_height(rect, min_price, max_price).clamp(3.0, 12.0);
    for level in levels {
        let size_t = (level.displayed_size as f32 / max_size as f32)
            .sqrt()
            .clamp(0.0, 1.0);
        let t = (0.12 + 0.54 * size_t.max(level.peak_intensity * 0.66)).clamp(0.0, 0.82);
        let base = heatmap_color(MboSide::None, t);
        let alpha = (16.0 + 62.0 * size_t).round() as u8;
        let color = egui::Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), alpha);
        let y = y_for_price(rect, min_price, max_price, level.price);
        let height = row_height * (0.62 + 0.38 * size_t);
        let band_rect = egui::Rect::from_min_max(
            egui::pos2(rect.left(), y - height * 0.5),
            egui::pos2(rect.right(), y + height * 0.5),
        )
        .intersect(rect);
        painter.rect_filled(band_rect, 0.0, color);
        if size_t >= 0.58 || level.peak_intensity >= 0.72 {
            let hot_t = (0.46 + 0.48 * size_t.max(level.peak_intensity)).clamp(0.0, 1.0);
            let hot = heatmap_color(MboSide::None, hot_t);
            let hot_alpha = (28.0 + 76.0 * size_t).round().clamp(28.0, 112.0) as u8;
            let core_height = (row_height * (0.18 + 0.22 * size_t)).clamp(1.0, row_height * 0.48);
            let core_rect = egui::Rect::from_min_max(
                egui::pos2(rect.left(), y - core_height * 0.5),
                egui::pos2(rect.right(), y + core_height * 0.5),
            )
            .intersect(rect);
            painter.rect_filled(
                core_rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(hot.r(), hot.g(), hot.b(), hot_alpha),
            );
        }
    }
}

fn persistent_heatmap_levels(
    bands: &[BookmapLiquidityBand],
    limit: usize,
) -> Vec<PersistentHeatmapLevel> {
    let mut by_level = BTreeMap::<(i64, u8), (u64, f32, MboSide)>::new();
    for band in bands {
        if band.displayed_size == 0 {
            continue;
        }
        let entry = by_level
            .entry((band.price.fixed(), side_sort_key(band.side)))
            .or_insert((0, 0.0, band.side));
        entry.0 = entry.0.saturating_add(band.displayed_size);
        entry.1 = entry.1.max(band.normalized_intensity);
    }
    let mut levels = by_level
        .into_iter()
        .map(
            |((price, _), (displayed_size, peak_intensity, side))| PersistentHeatmapLevel {
                price: MboPrice::from_fixed(price),
                side,
                displayed_size,
                peak_intensity,
            },
        )
        .collect::<Vec<_>>();
    levels.sort_by(|left, right| {
        right
            .displayed_size
            .cmp(&left.displayed_size)
            .then_with(|| {
                right
                    .peak_intensity
                    .partial_cmp(&left.peak_intensity)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| left.price.fixed().cmp(&right.price.fixed()))
            .then_with(|| side_sort_key(left.side).cmp(&side_sort_key(right.side)))
    });
    levels.truncate(limit);
    levels
}

fn side_sort_key(side: MboSide) -> u8 {
    match side {
        MboSide::Bid => 0,
        MboSide::Ask => 1,
        MboSide::None => 2,
        MboSide::Unknown(_) => 3,
    }
}

fn draw_heatmap_cell_surface(
    painter: &egui::Painter,
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    min_price: i64,
    max_price: i64,
    cells: &[(LiquidityCellKey, LiquidityCell)],
) {
    let bands = heatmap_band_runs(cells, max_ts);
    draw_heatmap_surface(painter, rect, min_ts, max_ts, min_price, max_price, &bands);
}

fn draw_current_book_continuation(
    painter: &egui::Painter,
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    min_price: i64,
    max_price: i64,
    bands: &[BookmapLiquidityBand],
) {
    for record in
        current_book_continuation_band_rects(rect, min_ts, max_ts, min_price, max_price, bands)
    {
        let t = record.intensity;
        let base_color = current_book_continuation_color(record.side, t, 0.36);
        let profile_color = current_book_continuation_color(record.side, t, 0.88);
        painter.rect_filled(record.base_rect, 0.0, base_color);
        painter.rect_filled(record.profile_rect, 0.0, profile_color);
    }
    for cell in bookmap_heatmap_texture_rects(
        rect,
        min_ts,
        max_ts,
        min_price,
        max_price,
        bands,
        BOOKMAP_HEATMAP_TEXTURE_MAX_RECTS / 4,
    ) {
        painter.rect_filled(cell.rect, 0.0, cell.color);
    }
}

#[derive(Debug, Clone, Copy)]
struct CurrentBookContinuationBandRect {
    base_rect: egui::Rect,
    profile_rect: egui::Rect,
    side: MboSide,
    intensity: f32,
}

fn current_book_continuation_band_rects(
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    min_price: i64,
    max_price: i64,
    bands: &[BookmapLiquidityBand],
) -> Vec<CurrentBookContinuationBandRect> {
    if rect.width() <= 2.0 || rect.height() <= 2.0 || max_ts <= min_ts {
        return Vec::new();
    }
    let row_height = price_row_height(rect, min_price, max_price).clamp(2.5, 11.0);
    let mut records = bands
        .iter()
        .filter(|band| band.displayed_size > 0 && band.price.is_defined())
        .filter_map(|band| {
            let y = y_for_price(rect, min_price, max_price, band.price);
            let t = band.normalized_intensity.clamp(0.0, 1.0);
            let height = (row_height * (0.54 + 0.26 * t)).clamp(2.0, row_height);
            let x0 = x_for_timestamp(rect, min_ts, max_ts, band.start_ts)
                .clamp(rect.left(), rect.right());
            let x1 = x_for_timestamp(rect, min_ts, max_ts, band.end_ts)
                .max(x0 + 4.0)
                .min(rect.right());
            if x1 <= x0 {
                return None;
            }
            let base_rect = egui::Rect::from_min_max(
                egui::pos2(x0, y - height * 0.5),
                egui::pos2(x1, y + height * 0.5),
            )
            .intersect(rect);
            if base_rect.is_negative() || base_rect.width() <= 0.0 || base_rect.height() <= 0.0 {
                return None;
            }
            let profile_fraction = (0.16 + 0.84 * t.sqrt()).clamp(0.16, 1.0);
            let profile_right = (x0 + (x1 - x0) * profile_fraction).max(x0 + 4.0).min(x1);
            let profile_rect = egui::Rect::from_min_max(
                egui::pos2(x0, base_rect.top()),
                egui::pos2(profile_right, base_rect.bottom()),
            )
            .intersect(rect);
            Some(CurrentBookContinuationBandRect {
                base_rect,
                profile_rect,
                side: band.side,
                intensity: t,
            })
        })
        .collect::<Vec<_>>();
    records.sort_by(|left, right| {
        left.base_rect
            .top()
            .partial_cmp(&right.base_rect.top())
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                left.intensity
                    .partial_cmp(&right.intensity)
                    .unwrap_or(Ordering::Equal)
            })
    });
    records
}

fn current_book_continuation_color(
    _side: MboSide,
    intensity: f32,
    opacity_scale: f32,
) -> egui::Color32 {
    let t = intensity.clamp(0.0, 1.0);
    let palette_t = (0.20 + 0.76 * t.sqrt()).clamp(0.0, 1.0);
    let color = heatmap_color(MboSide::None, palette_t);
    let alpha = ((34.0 + 178.0 * t.sqrt()) * opacity_scale)
        .round()
        .clamp(10.0, 218.0) as u8;
    egui::Color32::from_rgba_premultiplied(color.r(), color.g(), color.b(), alpha)
}

fn draw_bookmap_price_action(
    painter: &egui::Painter,
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    min_price: i64,
    max_price: i64,
    bars: &[OhlcvBarRecord],
    bbo: &[BboTransition],
    bubbles: &[(VolumeBubbleKey, VolumeBubble)],
) {
    let (price_action_records, source) = bookmap_price_action_records(bars, bbo, bubbles);
    if price_action_records.is_empty() {
        return;
    }
    let Some(foundation) = BookmapChartFoundation::new(
        rect,
        min_ts,
        max_ts,
        min_price,
        max_price,
        &price_action_records,
        source,
    ) else {
        return;
    };
    let mode = bookmap_price_action_mode(&foundation);
    let width = bookmap_price_action_body_width(&foundation);
    if mode == BookmapPriceActionMode::CompactPath {
        draw_bookmap_compact_price_action_ribbon(painter, &foundation);
        draw_bookmap_close_price_path(painter, &foundation);
        return;
    }
    for (index, chart_bar) in foundation.bars.iter().copied().enumerate() {
        let x = foundation.transform.logical_to_x(index as f64);
        let open_y = foundation.transform.price_to_y(chart_bar.open);
        let close_y = foundation.transform.price_to_y(chart_bar.close);
        let high_y = foundation.transform.price_to_y(chart_bar.high);
        let low_y = foundation.transform.price_to_y(chart_bar.low);
        let up = chart_bar.close >= chart_bar.open;
        let color = if up {
            egui::Color32::from_rgba_premultiplied(55, 215, 118, 205)
        } else {
            egui::Color32::from_rgba_premultiplied(226, 72, 74, 205)
        };
        let wick_stroke = egui::Stroke::new(1.5, egui::Color32::from_rgb(238, 242, 244));
        painter.line_segment([egui::pos2(x, high_y), egui::pos2(x, low_y)], wick_stroke);
        let min_body_height = 2.0;
        let body = egui::Rect::from_min_max(
            egui::pos2(x - width * 0.5, open_y.min(close_y)),
            egui::pos2(
                x + width * 0.5,
                open_y
                    .max(close_y)
                    .max(open_y.min(close_y) + min_body_height),
            ),
        )
        .intersect(rect);
        let radius = 1.0;
        painter.rect_filled(body, radius, color);
        painter.rect_stroke(
            body,
            radius,
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_premultiplied(245, 247, 248, 155),
            ),
            egui::StrokeKind::Inside,
        );
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BookmapPriceActionMode {
    DenseCandles,
    CompactPath,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BookmapPriceActionSource {
    Ohlcv,
    TradePrints,
    BboMidpoint,
}

#[derive(Clone, Debug)]
struct BookmapChartFoundation {
    transform: ChartTransform,
    records: Vec<OhlcvBarRecord>,
    bars: Vec<OhlcBar>,
    source: BookmapPriceActionSource,
}

impl BookmapChartFoundation {
    fn new(
        rect: egui::Rect,
        min_ts: u64,
        max_ts: u64,
        min_price: i64,
        max_price: i64,
        records: &[OhlcvBarRecord],
        source: BookmapPriceActionSource,
    ) -> Option<Self> {
        if rect.width() <= 0.0 || rect.height() <= 0.0 || records.is_empty() || max_ts <= min_ts {
            return None;
        }
        let mut records = records.to_vec();
        records.sort_by_key(|record| record.interval_start_ns);
        records.retain(|record| bookmap_ohlcv_record_to_chart_bar(record).is_valid());
        if records.is_empty() {
            return None;
        }
        let interval_ns = bookmap_ohlcv_interval_ns(&records);
        let bars = records
            .iter()
            .map(bookmap_ohlcv_record_to_chart_bar)
            .collect::<Vec<_>>();
        let time = bookmap_visible_logical_range(min_ts, max_ts, &records, interval_ns)?;
        let price = PriceRange::new(
            fixed_price_to_chart_value(min_price),
            fixed_price_to_chart_value(max_price),
        )?;
        Some(Self {
            transform: ChartTransform::new(rect, time, price),
            records,
            bars,
            source,
        })
    }

    fn bar_spacing(&self) -> f32 {
        self.transform.plot.width() / self.transform.time.span() as f32
    }
}

fn bookmap_price_action_mode(foundation: &BookmapChartFoundation) -> BookmapPriceActionMode {
    if foundation.source != BookmapPriceActionSource::Ohlcv
        || foundation.records.len() < 64
        || bookmap_price_action_raw_body_width(foundation) > 10.0
    {
        BookmapPriceActionMode::CompactPath
    } else {
        BookmapPriceActionMode::DenseCandles
    }
}

fn bookmap_price_action_body_width(foundation: &BookmapChartFoundation) -> f32 {
    let width = bookmap_price_action_raw_body_width(foundation);
    match bookmap_price_action_mode(foundation) {
        BookmapPriceActionMode::CompactPath
            if foundation.source == BookmapPriceActionSource::Ohlcv =>
        {
            width.clamp(2.0, 6.0)
        }
        BookmapPriceActionMode::CompactPath => width.clamp(1.0, 3.0),
        BookmapPriceActionMode::DenseCandles => width.clamp(2.0, 10.0),
    }
}

fn bookmap_price_action_raw_body_width(foundation: &BookmapChartFoundation) -> f32 {
    (foundation.bar_spacing() * 0.72).clamp(3.0, 18.0)
}

#[derive(Clone, Copy, Debug)]
struct BookmapMicroPriceActionBar {
    x: f32,
    open_y: f32,
    close_y: f32,
    high_y: f32,
    low_y: f32,
    up: bool,
}

fn draw_bookmap_compact_price_action_ribbon(
    painter: &egui::Painter,
    foundation: &BookmapChartFoundation,
) {
    let micro_bars = bookmap_micro_price_action_bars(foundation);
    if micro_bars.is_empty() {
        return;
    }
    let body_width = (foundation.transform.plot.width() / 300.0).clamp(2.6, 6.6);
    for bar in micro_bars {
        let color = if bar.up {
            egui::Color32::from_rgba_premultiplied(50, 226, 112, 164)
        } else {
            egui::Color32::from_rgba_premultiplied(232, 62, 66, 164)
        };
        painter.line_segment(
            [egui::pos2(bar.x, bar.high_y), egui::pos2(bar.x, bar.low_y)],
            egui::Stroke::new(
                1.05,
                egui::Color32::from_rgba_premultiplied(245, 248, 250, 188),
            ),
        );
        let top = bar.open_y.min(bar.close_y);
        let bottom = bar.open_y.max(bar.close_y).max(top + 1.8);
        let body = egui::Rect::from_min_max(
            egui::pos2(bar.x - body_width * 0.5, top),
            egui::pos2(bar.x + body_width * 0.5, bottom),
        )
        .intersect(foundation.transform.plot);
        painter.rect_filled(body, 0.0, color);
        painter.rect_stroke(
            body,
            0.0,
            egui::Stroke::new(
                0.8,
                egui::Color32::from_rgba_premultiplied(246, 248, 250, 150),
            ),
            egui::StrokeKind::Inside,
        );
    }
}

fn bookmap_micro_price_action_bars(
    foundation: &BookmapChartFoundation,
) -> Vec<BookmapMicroPriceActionBar> {
    if foundation.bars.is_empty() {
        return Vec::new();
    }
    let plot = foundation.transform.plot;
    let spacing = (plot.width() / 280.0).clamp(3.5, 8.0);
    let mut micro_bars = Vec::with_capacity(foundation.bars.len().saturating_mul(8));
    for (index, chart_bar) in foundation.bars.iter().copied().enumerate() {
        let x0 = foundation
            .transform
            .logical_to_x(index as f64)
            .clamp(plot.left(), plot.right());
        let x1 = if index + 1 < foundation.bars.len() {
            foundation
                .transform
                .logical_to_x(index as f64 + 1.0)
                .clamp(plot.left(), plot.right())
        } else {
            (x0 + foundation.bar_spacing()).clamp(plot.left(), plot.right())
        };
        let span = x1 - x0;
        if span <= 1.0 {
            continue;
        }
        let steps = ((span / spacing).ceil() as usize).clamp(1, 96);
        let anchors = bookmap_micro_price_anchors(chart_bar);
        for step in 0..steps {
            let t0 = step as f32 / steps as f32;
            let t1 = (step + 1) as f32 / steps as f32;
            let open = bookmap_micro_price_at(&anchors, t0);
            let close = bookmap_micro_price_at(&anchors, t1);
            let (low, high) = bookmap_micro_price_extrema(&anchors, t0, t1);
            let x = x0 + span * (step as f32 + 0.5) / steps as f32;
            micro_bars.push(BookmapMicroPriceActionBar {
                x,
                open_y: foundation
                    .transform
                    .price_to_y(open)
                    .clamp(plot.top(), plot.bottom()),
                close_y: foundation
                    .transform
                    .price_to_y(close)
                    .clamp(plot.top(), plot.bottom()),
                high_y: foundation
                    .transform
                    .price_to_y(high)
                    .clamp(plot.top(), plot.bottom()),
                low_y: foundation
                    .transform
                    .price_to_y(low)
                    .clamp(plot.top(), plot.bottom()),
                up: close >= open,
            });
        }
    }
    micro_bars
}

fn bookmap_micro_price_anchors(bar: OhlcBar) -> [(f32, f64); 4] {
    if bar.close >= bar.open {
        [
            (0.0, bar.open),
            (0.28, bar.low),
            (0.68, bar.high),
            (1.0, bar.close),
        ]
    } else {
        [
            (0.0, bar.open),
            (0.28, bar.high),
            (0.68, bar.low),
            (1.0, bar.close),
        ]
    }
}

fn bookmap_micro_price_at(anchors: &[(f32, f64); 4], t: f32) -> f64 {
    let t = t.clamp(0.0, 1.0);
    for pair in anchors.windows(2) {
        let (left_t, left_price) = pair[0];
        let (right_t, right_price) = pair[1];
        if t <= right_t {
            let span = (right_t - left_t).max(f32::EPSILON);
            let fraction = ((t - left_t) / span).clamp(0.0, 1.0) as f64;
            return left_price + (right_price - left_price) * fraction;
        }
    }
    anchors.last().map(|(_, price)| *price).unwrap_or_default()
}

fn bookmap_micro_price_extrema(anchors: &[(f32, f64); 4], t0: f32, t1: f32) -> (f64, f64) {
    let mut low = bookmap_micro_price_at(anchors, t0).min(bookmap_micro_price_at(anchors, t1));
    let mut high = bookmap_micro_price_at(anchors, t0).max(bookmap_micro_price_at(anchors, t1));
    for (anchor_t, price) in anchors {
        if *anchor_t > t0 && *anchor_t < t1 {
            low = low.min(*price);
            high = high.max(*price);
        }
    }
    (low, high)
}

fn draw_bookmap_close_price_path(painter: &egui::Painter, foundation: &BookmapChartFoundation) {
    let points = bookmap_close_price_points(foundation);
    if points.len() < 2 {
        return;
    }
    for (pos, up) in bookmap_sampled_close_price_ticks(foundation) {
        let color = if up {
            egui::Color32::from_rgba_premultiplied(58, 222, 118, 118)
        } else {
            egui::Color32::from_rgba_premultiplied(230, 74, 78, 118)
        };
        let tick_rect = egui::Rect::from_center_size(pos, egui::vec2(3.8, 9.0))
            .intersect(foundation.transform.plot);
        painter.rect_filled(tick_rect, 0.0, color);
    }
    let point_positions = step_points(&points.iter().map(|(pos, _)| *pos).collect::<Vec<_>>());
    draw_polyline_segments(
        painter,
        &point_positions,
        egui::Stroke::new(3.2, egui::Color32::from_rgba_premultiplied(0, 0, 0, 128)),
    );
    draw_polyline_segments(
        painter,
        &point_positions,
        egui::Stroke::new(
            1.15,
            egui::Color32::from_rgba_premultiplied(245, 248, 250, 150),
        ),
    );
    for pair in step_points_with_direction(&points).windows(2) {
        let [(left, _), (right, up)] = pair else {
            continue;
        };
        let stroke = if *up {
            egui::Stroke::new(
                1.55,
                egui::Color32::from_rgba_premultiplied(54, 224, 118, 220),
            )
        } else {
            egui::Stroke::new(
                1.55,
                egui::Color32::from_rgba_premultiplied(230, 66, 70, 220),
            )
        };
        painter.line_segment([*left, *right], stroke);
    }
}

fn bookmap_sampled_close_price_ticks(
    foundation: &BookmapChartFoundation,
) -> Vec<(egui::Pos2, bool)> {
    let points = bookmap_close_price_points(foundation);
    if points.len() < 2 {
        return points;
    }
    let spacing = (foundation.transform.plot.width() / 180.0).clamp(5.0, 13.0);
    let mut ticks = Vec::with_capacity(points.len().saturating_mul(4));
    for pair in points.windows(2) {
        let [(left, left_up), (right, right_up)] = pair else {
            continue;
        };
        ticks.push((*left, *left_up));
        let dx = right.x - left.x;
        if dx <= spacing {
            continue;
        }
        let count = (dx / spacing).floor() as usize;
        for index in 1..count {
            let x = left.x + spacing * index as f32;
            ticks.push((egui::pos2(x, left.y), *right_up));
        }
    }
    if let Some(last) = points.last().copied() {
        ticks.push(last);
    }
    ticks
}

fn step_points_with_direction(points: &[(egui::Pos2, bool)]) -> Vec<(egui::Pos2, bool)> {
    let Some(first) = points.first().copied() else {
        return Vec::new();
    };
    let mut stepped = Vec::with_capacity(points.len().saturating_mul(2));
    stepped.push(first);
    let mut previous = first;
    for point in points.iter().copied().skip(1) {
        stepped.push((egui::pos2(point.0.x, previous.0.y), point.1));
        stepped.push(point);
        previous = point;
    }
    stepped
}

fn bookmap_close_price_points(foundation: &BookmapChartFoundation) -> Vec<(egui::Pos2, bool)> {
    foundation
        .bars
        .iter()
        .enumerate()
        .map(|(index, bar)| {
            (
                egui::pos2(
                    foundation.transform.logical_to_x(index as f64),
                    foundation.transform.price_to_y(bar.close),
                ),
                bar.close >= bar.open,
            )
        })
        .collect()
}

fn bookmap_price_action_records(
    ohlcv_bars: &[OhlcvBarRecord],
    bbo: &[BboTransition],
    bubbles: &[(VolumeBubbleKey, VolumeBubble)],
) -> (Vec<OhlcvBarRecord>, BookmapPriceActionSource) {
    let threshold = bookmap_dense_price_action_threshold(ohlcv_bars.len());
    let trade_source = if bbo.is_empty() {
        bubbles.to_vec()
    } else {
        bookmap_visible_volume_bubbles(bubbles, bbo)
    };
    let trade_records = bookmap_trade_price_action_records(&trade_source);
    if !trade_records.is_empty() && (ohlcv_bars.is_empty() || trade_records.len() >= threshold) {
        return (trade_records, BookmapPriceActionSource::TradePrints);
    }

    let bbo_records = bookmap_bbo_price_action_records(bbo);
    if !bbo_records.is_empty() && (ohlcv_bars.is_empty() || bbo_records.len() >= threshold) {
        return (bbo_records, BookmapPriceActionSource::BboMidpoint);
    }

    (ohlcv_bars.to_vec(), BookmapPriceActionSource::Ohlcv)
}

fn bookmap_dense_price_action_threshold(ohlcv_len: usize) -> usize {
    if ohlcv_len < 32 {
        ohlcv_len.saturating_add(4).max(8)
    } else {
        ohlcv_len.saturating_mul(2).max(32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BookmapPriceActionPoint {
    ts: u64,
    instrument_id: u32,
    price_fixed: i64,
    volume: u64,
    delta: i64,
}

fn bookmap_trade_price_action_records(
    bubbles: &[(VolumeBubbleKey, VolumeBubble)],
) -> Vec<OhlcvBarRecord> {
    let mut by_ts = BTreeMap::<u64, (i128, u64, i64, u32)>::new();
    for (key, bubble) in bubbles {
        let volume = bubble.total_volume();
        if volume == 0 || !key.price.is_defined() {
            continue;
        }
        let entry = by_ts
            .entry(key.interval_start_ns)
            .or_insert((0, 0, 0, key.instrument_id));
        entry.0 = entry
            .0
            .saturating_add(key.price.fixed() as i128 * volume as i128);
        entry.1 = entry.1.saturating_add(volume);
        entry.2 = entry.2.saturating_add(bubble.delta());
        entry.3 = key.instrument_id;
    }
    let points = by_ts
        .into_iter()
        .filter_map(|(ts, (weighted_price, volume, delta, instrument_id))| {
            (volume > 0).then_some(BookmapPriceActionPoint {
                ts,
                instrument_id,
                price_fixed: (weighted_price / volume as i128) as i64,
                volume,
                delta,
            })
        })
        .collect::<Vec<_>>();
    bookmap_price_action_records_from_points(points)
}

fn bookmap_bbo_price_action_records(bbo: &[BboTransition]) -> Vec<OhlcvBarRecord> {
    let points = bbo
        .iter()
        .filter_map(|transition| {
            midpoint_or_single_price(transition.bid, transition.ask).map(|price| {
                BookmapPriceActionPoint {
                    ts: transition.ts_event,
                    instrument_id: transition.instrument_id,
                    price_fixed: price.fixed(),
                    volume: transition
                        .bid
                        .map(|level| level.size)
                        .unwrap_or_default()
                        .saturating_add(transition.ask.map(|level| level.size).unwrap_or_default()),
                    delta: 0,
                }
            })
        })
        .collect::<Vec<_>>();
    bookmap_price_action_records_from_points(points)
}

fn bookmap_price_action_records_from_points(
    mut points: Vec<BookmapPriceActionPoint>,
) -> Vec<OhlcvBarRecord> {
    points.sort_by_key(|point| point.ts);
    points.dedup_by_key(|point| point.ts);
    if points.is_empty() {
        return Vec::new();
    }
    let interval_ns = points
        .windows(2)
        .filter_map(|pair| {
            let left = pair[0].ts;
            let right = pair[1].ts;
            (right > left).then_some(right - left)
        })
        .min()
        .unwrap_or(SECOND_NS)
        .max(1);
    let mut previous_price = None::<i64>;
    points
        .into_iter()
        .map(|point| {
            let open_fixed = previous_price.unwrap_or(point.price_fixed);
            previous_price = Some(point.price_fixed);
            let high_fixed = open_fixed.max(point.price_fixed);
            let low_fixed = open_fixed.min(point.price_fixed);
            let delta = if point.delta != 0 {
                point.delta
            } else {
                point.price_fixed.saturating_sub(open_fixed)
            };
            OhlcvBarRecord {
                interval_ns,
                interval_start_ns: point.ts,
                instrument_id: point.instrument_id,
                open_fixed,
                high_fixed,
                low_fixed,
                close_fixed: point.price_fixed,
                volume: point.volume.max(1),
                buy_volume: if delta >= 0 { point.volume.max(1) } else { 0 },
                sell_volume: if delta < 0 { point.volume.max(1) } else { 0 },
                unknown_volume: 0,
                trade_count: 1,
                fill_count: 1,
                delta,
            }
        })
        .collect()
}

fn bookmap_ohlcv_record_to_chart_bar(record: &OhlcvBarRecord) -> OhlcBar {
    OhlcBar {
        time: record.interval_start_ns.min(i64::MAX as u64) as i64,
        open: fixed_price_to_chart_value(record.open_fixed),
        high: fixed_price_to_chart_value(record.high_fixed),
        low: fixed_price_to_chart_value(record.low_fixed),
        close: fixed_price_to_chart_value(record.close_fixed),
        volume: record.volume as f64,
    }
}

fn fixed_price_to_chart_value(fixed: i64) -> f64 {
    fixed as f64 / FIXED_PRICE_SCALE as f64
}

fn bookmap_ohlcv_interval_ns(records: &[OhlcvBarRecord]) -> u64 {
    records
        .windows(2)
        .filter_map(|pair| {
            let left = pair[0].interval_start_ns;
            let right = pair[1].interval_start_ns;
            (right > left).then_some(right - left)
        })
        .min()
        .or_else(|| records.first().map(|record| record.interval_ns))
        .unwrap_or(SECOND_NS)
        .max(1)
}

fn bookmap_visible_logical_range(
    min_ts: u64,
    max_ts: u64,
    records: &[OhlcvBarRecord],
    interval_ns: u64,
) -> Option<LogicalRange> {
    let from = bookmap_timestamp_logical(min_ts, records, interval_ns);
    let mut to = bookmap_timestamp_logical(max_ts, records, interval_ns);
    if to <= from {
        to = from + 1.0;
    }
    LogicalRange::new(from, to)
}

fn bookmap_timestamp_logical(
    timestamp_ns: u64,
    records: &[OhlcvBarRecord],
    interval_ns: u64,
) -> f64 {
    let Some(first) = records.first() else {
        return 0.0;
    };
    let interval = interval_ns.max(1) as f64;
    if timestamp_ns <= first.interval_start_ns {
        return -((first.interval_start_ns - timestamp_ns) as f64 / interval);
    }
    for (index, pair) in records.windows(2).enumerate() {
        let left = pair[0].interval_start_ns;
        let right = pair[1].interval_start_ns;
        if timestamp_ns <= right {
            let span = right.saturating_sub(left).max(1) as f64;
            return index as f64 + (timestamp_ns.saturating_sub(left) as f64 / span);
        }
    }
    let last = records.last().expect("records has first");
    records.len().saturating_sub(1) as f64
        + (timestamp_ns.saturating_sub(last.interval_start_ns) as f64 / interval)
}

fn draw_bookmap_volume_histogram(
    painter: &egui::Painter,
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    bars: &[OhlcvBarRecord],
    bubbles: &[(VolumeBubbleKey, VolumeBubble)],
) {
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(12, 20, 23));
    painter.line_segment(
        [rect.left_top(), rect.right_top()],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 58, 64)),
    );
    if !bars.is_empty() {
        let max_volume = bars.iter().map(|bar| bar.volume).max().unwrap_or(1).max(1);
        let width = ohlcv_bar_width(rect, min_ts, max_ts, bars).max(2.0);
        for bar in bars {
            let x = x_for_timestamp(rect, min_ts, max_ts, bar.interval_start_ns);
            let height = rect.height() * (bar.volume as f32 / max_volume as f32).sqrt();
            let fill = if bar.delta >= 0 {
                egui::Color32::from_rgba_premultiplied(68, 205, 124, 205)
            } else {
                egui::Color32::from_rgba_premultiplied(220, 73, 76, 205)
            };
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x - width * 0.42, rect.bottom() - height),
                    egui::pos2(x + width * 0.42, rect.bottom()),
                )
                .intersect(rect),
                0.0,
                fill,
            );
        }
        return;
    }
    let max_volume = bubbles
        .iter()
        .map(|(_, bubble)| bubble.total_volume())
        .max()
        .unwrap_or(1)
        .max(1);
    for (key, bubble) in bubbles {
        let x = x_for_timestamp(rect, min_ts, max_ts, key.interval_start_ns);
        let height = rect.height() * (bubble.total_volume() as f32 / max_volume as f32).sqrt();
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x - 2.0, rect.bottom() - height),
                egui::pos2(x + 2.0, rect.bottom()),
            )
            .intersect(rect),
            0.0,
            bubble_color(*bubble),
        );
    }
}

fn ohlcv_bar_width(rect: egui::Rect, min_ts: u64, max_ts: u64, bars: &[OhlcvBarRecord]) -> f32 {
    let interval = bars
        .windows(2)
        .filter_map(|pair| {
            let left = pair[0].interval_start_ns;
            let right = pair[1].interval_start_ns;
            (right > left).then_some(right - left)
        })
        .min()
        .unwrap_or_else(|| bars.first().map(|bar| bar.interval_ns).unwrap_or(SECOND_NS));
    if max_ts <= min_ts {
        return 4.0;
    }
    (rect.width() * interval as f32 / (max_ts - min_ts) as f32 * 0.72).clamp(3.0, 18.0)
}

fn draw_volume_bubbles(
    painter: &egui::Painter,
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    min_price: i64,
    max_price: i64,
    bubbles: &[(VolumeBubbleKey, VolumeBubble)],
) {
    let max_volume = bubbles
        .iter()
        .map(|(_, bubble)| bubble.total_volume())
        .max()
        .unwrap_or_default();
    for (key, bubble) in bubbles {
        let radius = composite_volume_bubble_radius(*bubble, max_volume, rect);
        if radius <= 0.0 {
            continue;
        }
        let center = egui::pos2(
            x_for_timestamp(rect, min_ts, max_ts, key.interval_start_ns),
            y_for_price(rect, min_price, max_price, key.price),
        );
        draw_split_volume_bubble(painter, rect, center, radius, *bubble);
        painter.circle_stroke(
            center,
            radius,
            egui::Stroke::new(1.2, egui::Color32::from_rgb(235, 237, 240)),
        );
    }
}

fn bookmap_visible_volume_bubbles(
    bubbles: &[(VolumeBubbleKey, VolumeBubble)],
    bbo: &[BboTransition],
) -> Vec<(VolumeBubbleKey, VolumeBubble)> {
    if bbo.is_empty() {
        return bubbles
            .iter()
            .copied()
            .take(COMPOSITE_BUBBLES / 4)
            .collect();
    }
    let max_distance_fixed = bookmap_bbo_neighborhood_fixed(bbo);
    bubbles
        .iter()
        .copied()
        .filter(|(key, _)| bubble_is_near_bbo_spread(key, bbo, max_distance_fixed))
        .take(COMPOSITE_BUBBLES / 4)
        .collect()
}

fn composite_volume_bubble_radius(bubble: VolumeBubble, max_volume: u64, rect: egui::Rect) -> f32 {
    let max_radius = bookmap_composite_bubble_max_radius(rect);
    bubble.radius(max_volume, 2.0, max_radius)
}

fn bookmap_composite_bubble_max_radius(rect: egui::Rect) -> f32 {
    (rect.width().min(rect.height()) * 0.014).clamp(4.0, 9.0)
}

fn draw_bbo_trade_markers(
    painter: &egui::Painter,
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    min_price: i64,
    max_price: i64,
    bubbles: &[(VolumeBubbleKey, VolumeBubble)],
    bbo: &[BboTransition],
) {
    let max_volume = bubbles
        .iter()
        .map(|(_, bubble)| bubble.total_volume())
        .max()
        .unwrap_or_default();
    if max_volume == 0 {
        return;
    }
    for (key, bubble) in bubbles {
        let total = bubble.total_volume();
        if total == 0 {
            continue;
        }
        if !bubble_is_near_bbo_spread(key, bbo, bookmap_bbo_neighborhood_fixed(bbo)) {
            continue;
        }
        let center = egui::pos2(
            x_for_timestamp(rect, min_ts, max_ts, key.interval_start_ns),
            y_for_price(rect, min_price, max_price, key.price),
        );
        let radius = bbo_trade_marker_radius(total, max_volume);
        let fill = if bubble.delta() >= 0 {
            egui::Color32::from_rgba_premultiplied(74, 216, 112, 210)
        } else {
            egui::Color32::from_rgba_premultiplied(226, 48, 54, 210)
        };
        painter.circle_filled(center, radius, fill);
        painter.circle_stroke(
            center,
            radius,
            egui::Stroke::new(1.0, egui::Color32::from_rgba_premultiplied(12, 14, 16, 160)),
        );
    }
}

fn bbo_trade_marker_radius(total_volume: u64, max_volume: u64) -> f32 {
    if total_volume == 0 || max_volume == 0 {
        return 0.0;
    }
    let normalized = (total_volume as f32 / max_volume as f32)
        .sqrt()
        .clamp(0.0, 1.0);
    1.8 + 4.2 * normalized
}

fn bubble_is_near_bbo_spread(
    key: &VolumeBubbleKey,
    bbo: &[BboTransition],
    max_distance_fixed: i64,
) -> bool {
    if bbo.is_empty() || !key.price.is_defined() {
        return true;
    }
    let Some(nearest) = nearest_bbo_transition(bbo, key.interval_start_ns) else {
        return true;
    };
    [nearest.bid, nearest.ask]
        .into_iter()
        .flatten()
        .any(|level| price_distance_fixed(key.price, level.price) <= max_distance_fixed as u64)
}

fn bookmap_bbo_neighborhood_fixed(bbo: &[BboTransition]) -> i64 {
    let max_observed_spread = bbo
        .iter()
        .filter_map(|transition| match (transition.bid, transition.ask) {
            (Some(bid), Some(ask)) => Some(ask.price.fixed().saturating_sub(bid.price.fixed())),
            _ => None,
        })
        .filter(|spread| *spread > 0)
        .max()
        .unwrap_or(COMPOSITE_SPREAD_MARKER_THRESHOLD);
    max_observed_spread.clamp(COMPOSITE_SPREAD_MARKER_THRESHOLD, FIXED_PRICE_SCALE * 4)
}

fn nearest_bbo_transition(bbo: &[BboTransition], ts_event: u64) -> Option<BboTransition> {
    bbo.iter()
        .copied()
        .min_by_key(|transition| transition.ts_event.abs_diff(ts_event))
}

fn price_distance_fixed(left: MboPrice, right: MboPrice) -> u64 {
    left.fixed().abs_diff(right.fixed())
}

fn draw_bbo_paths(
    painter: &egui::Painter,
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    min_price: i64,
    max_price: i64,
    bbo: &[BboTransition],
) {
    let bid_points = bbo_price_points(
        rect,
        min_ts,
        max_ts,
        min_price,
        max_price,
        bbo,
        MboSide::Bid,
    );
    let ask_points = bbo_price_points(
        rect,
        min_ts,
        max_ts,
        min_price,
        max_price,
        bbo,
        MboSide::Ask,
    );
    let bid_steps = step_points(&bid_points);
    let ask_steps = step_points(&ask_points);
    let halo = egui::Stroke::new(4.0, egui::Color32::from_rgba_premultiplied(0, 0, 0, 150));
    draw_polyline_segments(painter, &bid_steps, halo);
    draw_polyline_segments(painter, &ask_steps, halo);
    draw_polyline(
        painter,
        &bid_steps,
        egui::Stroke::new(1.45, egui::Color32::from_rgb(68, 178, 110)),
    );
    draw_polyline(
        painter,
        &ask_steps,
        egui::Stroke::new(1.45, egui::Color32::from_rgb(196, 70, 88)),
    );
}

fn bbo_price_points(
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    min_price: i64,
    max_price: i64,
    bbo: &[BboTransition],
    side: MboSide,
) -> Vec<egui::Pos2> {
    let mut points = Vec::new();
    let mut last_price = None::<i64>;
    for transition in bbo {
        let level = match side {
            MboSide::Bid => transition.bid,
            MboSide::Ask => transition.ask,
            MboSide::None | MboSide::Unknown(_) => None,
        };
        let Some(level) = level else {
            continue;
        };
        let price = level.price.fixed();
        if last_price == Some(price) {
            continue;
        }
        points.push(egui::pos2(
            x_for_timestamp(rect, min_ts, max_ts, transition.ts_event),
            y_for_price(rect, min_price, max_price, level.price),
        ));
        last_price = Some(price);
    }
    points
}

fn step_points(points: &[egui::Pos2]) -> Vec<egui::Pos2> {
    let Some(first) = points.first().copied() else {
        return Vec::new();
    };
    let mut stepped = Vec::with_capacity(points.len().saturating_mul(2));
    stepped.push(first);
    let mut previous = first;
    for point in points.iter().copied().skip(1) {
        stepped.push(egui::pos2(point.x, previous.y));
        stepped.push(point);
        previous = point;
    }
    stepped
}

fn draw_polyline(painter: &egui::Painter, points: &[egui::Pos2], stroke: egui::Stroke) {
    draw_polyline_segments(painter, points, stroke);
    if let Some(point) = points.last() {
        painter.circle_filled(*point, 2.5, stroke.color);
    }
}

fn draw_polyline_segments(painter: &egui::Painter, points: &[egui::Pos2], stroke: egui::Stroke) {
    for pair in points.windows(2) {
        painter.line_segment([pair[0], pair[1]], stroke);
    }
}

fn draw_replay_cursor(
    painter: &egui::Painter,
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    cursor_ts: u64,
) {
    let x = x_for_timestamp(rect, min_ts, max_ts, cursor_ts);
    painter.line_segment(
        [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(235, 235, 70)),
    );
}

fn draw_dom_columns(
    painter: &egui::Painter,
    price_ladder_rect: egui::Rect,
    cob_rect: egui::Rect,
    cvp_rect: egui::Rect,
    svp_rect: egui::Rect,
    min_price: i64,
    max_price: i64,
    dom: Option<&DomColumnSnapshot>,
) {
    let rect = price_ladder_rect
        .union(cob_rect)
        .union(cvp_rect)
        .union(svp_rect);
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(48, 64, 70));
    painter.line_segment(
        [
            egui::pos2(rect.left(), rect.top()),
            egui::pos2(rect.left(), rect.bottom()),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(34, 45, 50)),
    );
    let header_height = 24.0_f32.min(rect.height() * 0.08);
    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), rect.top() + header_height),
        rect.max,
    );
    let price_body = egui::Rect::from_min_max(
        egui::pos2(price_ladder_rect.left(), body.top()),
        egui::pos2(price_ladder_rect.right(), body.bottom()),
    );
    let cob_body = egui::Rect::from_min_max(
        egui::pos2(cob_rect.left(), body.top()),
        egui::pos2(cob_rect.right(), body.bottom()),
    );
    let cvp_body = egui::Rect::from_min_max(
        egui::pos2(cvp_rect.left(), body.top()),
        egui::pos2(cvp_rect.right(), body.bottom()),
    );
    let svp_body = egui::Rect::from_min_max(
        egui::pos2(svp_rect.left(), body.top()),
        egui::pos2(svp_rect.right(), body.bottom()),
    );
    for column in [price_ladder_rect, cob_rect, cvp_rect, svp_rect] {
        painter.line_segment(
            [
                egui::pos2(column.left(), rect.top()),
                egui::pos2(column.left(), rect.bottom()),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(39, 51, 57)),
        );
    }
    for (column, header) in [
        (price_ladder_rect, ""),
        (cob_rect, "COB"),
        (cvp_rect, "CVP"),
        (svp_rect, "SVP"),
    ] {
        painter.text(
            column.center_top() + egui::vec2(0.0, 14.0),
            egui::Align2::CENTER_CENTER,
            header,
            egui::FontId::monospace(10.0),
            egui::Color32::from_rgb(210, 218, 222),
        );
    }
    let Some(dom) = dom else {
        draw_bookmap_price_ladder_labels(painter, price_body, min_price, max_price);
        return;
    };
    let max_size = dom.max_size.max(1);
    let metrics = bookmap_dom_column_row_metrics(price_body, min_price, max_price);
    let show_level_text = bookmap_dom_show_level_text(metrics, dom.levels.len());
    for level in &dom.levels {
        let y = y_for_price(price_body, min_price, max_price, level.price);
        let row_height = metrics.row_height;
        let price_row = egui::Rect::from_center_size(
            egui::pos2(price_body.center().x, y),
            egui::vec2(price_body.width(), row_height),
        )
        .intersect(price_body);
        painter.rect_filled(price_row, 0.0, egui::Color32::from_rgb(48, 63, 69));
        let cob_row = row_at_y(cob_body, y, row_height);
        draw_dom_size_cell(
            painter,
            cob_row,
            level.bid_size,
            level.ask_size,
            max_size,
            show_level_text,
        );
        let cvp_value = level.total_size.saturating_add(level.bid_size / 2);
        let svp_value = level.total_size.saturating_add(level.ask_size / 2);
        draw_dom_profile_cell(
            painter,
            row_at_y(cvp_body, y, row_height),
            cvp_value,
            max_size,
            level.side_hint,
            "",
            show_level_text,
        );
        draw_dom_profile_cell(
            painter,
            row_at_y(svp_body, y, row_height),
            svp_value,
            max_size,
            level.side_hint,
            "",
            show_level_text,
        );
    }
    draw_bookmap_price_ladder_labels(painter, price_body, min_price, max_price);
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct BookmapDomColumnRowMetrics {
    row_height: f32,
    show_row_text: bool,
}

fn bookmap_dom_column_row_metrics(
    rect: egui::Rect,
    min_price: i64,
    max_price: i64,
) -> BookmapDomColumnRowMetrics {
    let row_height = price_row_height(rect, min_price, max_price).clamp(8.0, 18.0);
    BookmapDomColumnRowMetrics {
        row_height,
        show_row_text: row_height >= 13.5 && rect.width() >= 40.0,
    }
}

fn bookmap_dom_show_level_text(metrics: BookmapDomColumnRowMetrics, level_count: usize) -> bool {
    metrics.show_row_text && level_count <= BOOKMAP_DOM_TEXT_LEVEL_LIMIT
}

fn bookmap_dom_cell_text_fits(rect: egui::Rect) -> bool {
    rect.width() >= 32.0 && rect.height() >= 12.0
}

fn draw_bookmap_price_ladder_labels(
    painter: &egui::Painter,
    rect: egui::Rect,
    min_price: i64,
    max_price: i64,
) {
    for (price, y) in bookmap_price_ladder_labels(rect, min_price, max_price) {
        painter.text(
            egui::pos2(rect.right() - 5.0, y),
            egui::Align2::RIGHT_CENTER,
            format_mbo_price(price),
            egui::FontId::monospace(BOOKMAP_PRICE_LADDER_LABEL_FONT_SIZE),
            egui::Color32::from_rgb(179, 190, 197),
        );
    }
}

fn bookmap_price_ladder_labels(
    rect: egui::Rect,
    min_price: i64,
    max_price: i64,
) -> Vec<(MboPrice, f32)> {
    if rect.height() <= 0.0 || max_price <= min_price {
        return Vec::new();
    }
    let min_gap = BOOKMAP_PRICE_LADDER_LABEL_MIN_GAP;
    let top_limit = rect.top() + BOOKMAP_PRICE_LADDER_LABEL_EDGE_PAD;
    let bottom_limit = rect.bottom() - BOOKMAP_PRICE_LADDER_LABEL_EDGE_PAD;
    let mut labels = Vec::new();
    let mut last_y = f32::NEG_INFINITY;
    for y in bookmap_price_ladder_y_positions(rect, min_price, max_price) {
        if y < top_limit || y > bottom_limit {
            continue;
        }
        if y - last_y < min_gap {
            continue;
        }
        let fraction_from_bottom = ((rect.bottom() - y) / rect.height()).clamp(0.0, 1.0);
        let fixed = min_price as f64 + (max_price - min_price) as f64 * fraction_from_bottom as f64;
        labels.push((MboPrice::from_fixed(fixed.round() as i64), y));
        last_y = y;
    }
    labels
}

fn row_at_y(rect: egui::Rect, y: f32, height: f32) -> egui::Rect {
    egui::Rect::from_center_size(
        egui::pos2(rect.center().x, y),
        egui::vec2(rect.width(), height),
    )
    .intersect(rect)
}

fn draw_last_price_marker(
    painter: &egui::Painter,
    chart_rect: egui::Rect,
    dom_rect: egui::Rect,
    min_price: i64,
    max_price: i64,
    price: Option<MboPrice>,
) {
    let Some(price) = price else {
        return;
    };
    if !price.is_defined() {
        return;
    }
    let y = y_for_price(chart_rect, min_price, max_price, price);
    let left = chart_rect.left();
    let right = dom_rect.right();
    let halo = egui::Stroke::new(3.0, egui::Color32::from_rgba_premultiplied(0, 0, 0, 170));
    let line = egui::Stroke::new(1.4, egui::Color32::from_rgb(246, 232, 82));
    painter.line_segment([egui::pos2(left, y), egui::pos2(right, y)], halo);
    painter.line_segment([egui::pos2(left, y), egui::pos2(right, y)], line);

    let label_rect = egui::Rect::from_center_size(
        egui::pos2(dom_rect.right() - 34.0, y),
        egui::vec2(64.0, 17.0),
    )
    .intersect(dom_rect.expand2(egui::vec2(0.0, 8.0)));
    painter.rect_filled(
        label_rect,
        1.0,
        egui::Color32::from_rgba_premultiplied(40, 35, 10, 238),
    );
    painter.rect_stroke(
        label_rect,
        1.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(246, 232, 82)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        label_rect.center(),
        egui::Align2::CENTER_CENTER,
        format_mbo_price(price),
        egui::FontId::monospace(9.0),
        egui::Color32::from_rgb(255, 248, 190),
    );
}

fn composite_last_price_marker(
    bubbles: &[(VolumeBubbleKey, VolumeBubble)],
    bbo: &[BboTransition],
    dom: Option<&DomColumnSnapshot>,
) -> Option<MboPrice> {
    if let Some((key, _)) = bubbles
        .iter()
        .filter(|(_, bubble)| bubble.total_volume() > 0)
        .max_by_key(|(key, _)| key.interval_start_ns)
    {
        return Some(key.price);
    }

    if let Some(transition) = bbo.iter().max_by_key(|transition| transition.ts_event) {
        if let Some(price) = midpoint_or_single_price(transition.bid, transition.ask) {
            return Some(price);
        }
    }

    dom.and_then(|dom| midpoint_or_single_price(dom.best_bid, dom.best_ask))
}

fn bookmap_active_price_marker(
    cursor_ts: u64,
    bars: &[OhlcvBarRecord],
    bbo: &[BboTransition],
    bubbles: &[(VolumeBubbleKey, VolumeBubble)],
    dom: Option<&DomColumnSnapshot>,
) -> Option<MboPrice> {
    nearest_cursor_price(cursor_ts, bars, bbo)
        .or_else(|| composite_last_price_marker(bubbles, bbo, dom))
}

fn midpoint_or_single_price(bid: Option<PriceLevel>, ask: Option<PriceLevel>) -> Option<MboPrice> {
    match (bid, ask) {
        (Some(bid), Some(ask)) => Some(MboPrice::from_fixed(
            bid.price.fixed().saturating_add(ask.price.fixed()) / 2,
        )),
        (Some(bid), None) => Some(bid.price),
        (None, Some(ask)) => Some(ask.price),
        (None, None) => None,
    }
}

fn composite_time_range(snapshot: &OrderFlowPreviewSnapshot) -> Option<(u64, u64)> {
    let mut min_ts = None::<u64>;
    let mut max_ts = None::<u64>;
    for (key, _) in &snapshot.visible_liquidity_cells {
        observe_time(&mut min_ts, &mut max_ts, key.interval_start_ns);
    }
    for (key, _) in &snapshot.volume_bubbles {
        observe_time(&mut min_ts, &mut max_ts, key.interval_start_ns);
    }
    for transition in &snapshot.bbo_transitions {
        observe_time(&mut min_ts, &mut max_ts, transition.ts_event);
    }
    if let Some(last_event_ts) = snapshot.last_event_ts {
        observe_time(&mut min_ts, &mut max_ts, last_event_ts);
    }
    min_ts.zip(max_ts).map(|(min, max)| {
        if min == max {
            (min, max.saturating_add(1))
        } else {
            (min, max)
        }
    })
}

fn observe_time(min_ts: &mut Option<u64>, max_ts: &mut Option<u64>, value: u64) {
    *min_ts = Some(min_ts.map_or(value, |current| current.min(value)));
    *max_ts = Some(max_ts.map_or(value, |current| current.max(value)));
}

fn composite_price_range(
    cells: &[(LiquidityCellKey, LiquidityCell)],
    bubbles: &[(VolumeBubbleKey, VolumeBubble)],
    bbo: &[BboTransition],
    dom: Option<&DomColumnSnapshot>,
) -> Option<(i64, i64)> {
    let mut min_price = None::<i64>;
    let mut max_price = None::<i64>;
    for (key, _) in cells {
        observe_price(&mut min_price, &mut max_price, key.price);
    }
    for (key, _) in bubbles {
        observe_price(&mut min_price, &mut max_price, key.price);
    }
    for transition in bbo {
        if let Some(level) = transition.bid {
            observe_price(&mut min_price, &mut max_price, level.price);
        }
        if let Some(level) = transition.ask {
            observe_price(&mut min_price, &mut max_price, level.price);
        }
    }
    if let Some(dom) = dom {
        for level in &dom.levels {
            observe_price(&mut min_price, &mut max_price, level.price);
        }
    }
    min_price.zip(max_price).map(|(min, max)| {
        let span = (max - min).abs();
        let padding = (span / 10).max(FIXED_PRICE_SCALE / 4);
        (min.saturating_sub(padding), max.saturating_add(padding))
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

fn cursor_timestamp(min_ts: u64, max_ts: u64, cursor_fraction: f32) -> u64 {
    let span = max_ts.saturating_sub(min_ts);
    min_ts.saturating_add((span as f32 * cursor_fraction.clamp(0.0, 1.0)) as u64)
}

fn x_for_timestamp(rect: egui::Rect, min_ts: u64, max_ts: u64, ts: u64) -> f32 {
    if max_ts <= min_ts {
        return rect.left();
    }
    let fraction = (ts.saturating_sub(min_ts) as f64 / (max_ts - min_ts) as f64).clamp(0.0, 1.0);
    (rect.left() as f64 + rect.width() as f64 * fraction) as f32
}

fn y_for_price(rect: egui::Rect, min_price: i64, max_price: i64, price: MboPrice) -> f32 {
    let Some(value) = price.as_f64() else {
        return rect.center().y;
    };
    let min_value = min_price as f64 / FIXED_PRICE_SCALE as f64;
    let max_value = max_price as f64 / FIXED_PRICE_SCALE as f64;
    if max_value <= min_value {
        return rect.center().y;
    }
    let fraction = ((value - min_value) / (max_value - min_value)).clamp(0.0, 1.0) as f32;
    egui::lerp(rect.bottom()..=rect.top(), fraction)
}

fn heatmap_bucket_width(
    rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    bands: &[BookmapLiquidityBand],
) -> f32 {
    let min_interval = bands
        .windows(2)
        .filter_map(|pair| {
            let left = pair[0].start_ts;
            let right = pair[1].start_ts;
            (right > left).then_some(right - left)
        })
        .min()
        .unwrap_or_else(|| max_ts.saturating_sub(min_ts).max(1));
    if max_ts <= min_ts {
        return 4.0;
    }
    (rect.width() * min_interval as f32 / (max_ts - min_ts) as f32).clamp(2.0, 18.0)
}

fn price_row_height(rect: egui::Rect, min_price: i64, max_price: i64) -> f32 {
    let span = max_price.saturating_sub(min_price).max(1);
    (rect.height() * FIXED_PRICE_SCALE as f32 / span as f32).clamp(2.0, 14.0)
}

fn heatmap_color(side: MboSide, intensity: f32) -> egui::Color32 {
    let t = intensity.clamp(0.0, 1.0);
    let (mut red, mut green, mut blue) = if t < 0.16 {
        color_lerp((5, 9, 20), (0, 58, 96), t / 0.16)
    } else if t < 0.34 {
        color_lerp((0, 58, 96), (0, 166, 214), (t - 0.16) / 0.18)
    } else if t < 0.52 {
        color_lerp((0, 166, 214), (250, 230, 54), (t - 0.34) / 0.18)
    } else if t < 0.68 {
        color_lerp((250, 230, 54), (255, 126, 18), (t - 0.52) / 0.16)
    } else if t < 0.82 {
        color_lerp((255, 126, 18), (226, 38, 24), (t - 0.68) / 0.14)
    } else if t < 0.94 {
        color_lerp((226, 38, 24), (255, 214, 72), (t - 0.82) / 0.12)
    } else {
        color_lerp((255, 214, 72), (255, 250, 235), (t - 0.94) / 0.06)
    };
    match side {
        MboSide::Ask => {
            red = red.saturating_add(16);
            green = green.saturating_sub(4);
            blue = blue.saturating_sub(10);
        }
        MboSide::Bid => {
            green = green.saturating_add(8);
            blue = blue.saturating_add(8);
        }
        MboSide::None | MboSide::Unknown(_) => {}
    }
    let alpha = (65.0 + 185.0 * t) as u8;
    egui::Color32::from_rgba_premultiplied(red, green, blue, alpha)
}

fn heatmap_event_color(side: MboSide, intensity: f32) -> egui::Color32 {
    let t = intensity.clamp(0.0, 1.0);
    let base_t = (0.08 + 0.34 * t).clamp(0.0, 1.0);
    let base = heatmap_color(side, base_t);
    let alpha = (8.0 + 38.0 * t.sqrt()).round() as u8;
    egui::Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), alpha)
}

fn color_lerp(from: (u8, u8, u8), to: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    (
        lerp_u8(from.0, to.0, t),
        lerp_u8(from.1, to.1, t),
        lerp_u8(from.2, to.2, t),
    )
}

fn lerp_u8(from: u8, to: u8, t: f32) -> u8 {
    (from as f32 + (to as f32 - from as f32) * t).round() as u8
}

fn bubble_color(bubble: VolumeBubble) -> egui::Color32 {
    if bubble.delta() > 0 {
        egui::Color32::from_rgba_premultiplied(62, 176, 108, 150)
    } else if bubble.delta() < 0 {
        egui::Color32::from_rgba_premultiplied(190, 74, 91, 150)
    } else {
        egui::Color32::from_rgba_premultiplied(160, 164, 170, 135)
    }
}

fn draw_split_volume_bubble(
    painter: &egui::Painter,
    clip_rect: egui::Rect,
    center: egui::Pos2,
    radius: f32,
    bubble: VolumeBubble,
) {
    let total = bubble.total_volume();
    if total == 0 {
        return;
    }
    let bubble_rect = egui::Rect::from_center_size(center, egui::vec2(radius * 2.0, radius * 2.0));
    painter.circle_filled(
        center + egui::vec2(1.0, 1.5),
        radius + 1.0,
        egui::Color32::from_rgba_premultiplied(0, 0, 0, 70),
    );
    if bubble.unknown_volume > 0 {
        painter.circle_filled(
            center,
            radius,
            egui::Color32::from_rgba_premultiplied(145, 150, 158, 92),
        );
    }
    let buy_fraction = bubble.buy_volume as f32 / total as f32;
    let split_y = volume_bubble_buy_split_y(bubble_rect, buy_fraction);
    if bubble.buy_volume > 0 {
        painter
            .with_clip_rect(
                egui::Rect::from_min_max(bubble_rect.min, egui::pos2(bubble_rect.right(), split_y))
                    .intersect(clip_rect),
            )
            .circle_filled(
                center,
                radius,
                egui::Color32::from_rgba_premultiplied(54, 192, 113, 180),
            );
    }
    if bubble.sell_volume > 0 {
        painter
            .with_clip_rect(
                egui::Rect::from_min_max(egui::pos2(bubble_rect.left(), split_y), bubble_rect.max)
                    .intersect(clip_rect),
            )
            .circle_filled(
                center,
                radius,
                egui::Color32::from_rgba_premultiplied(211, 64, 82, 180),
            );
    }
    painter.circle_filled(
        center + egui::vec2(-radius * 0.28, -radius * 0.34),
        (radius * 0.24).clamp(1.0, 4.5),
        egui::Color32::from_rgba_premultiplied(255, 255, 255, 38),
    );
}

fn volume_bubble_buy_split_y(bubble_rect: egui::Rect, buy_fraction: f32) -> f32 {
    egui::lerp(
        bubble_rect.top()..=bubble_rect.bottom(),
        buy_fraction.clamp(0.0, 1.0),
    )
}

fn filtered_liquidity_threshold_for_cells(cells: &[(LiquidityCellKey, LiquidityCell)]) -> u64 {
    let mut sizes = cells
        .iter()
        .filter_map(|(_, cell)| (cell.displayed_size > 0).then_some(cell.displayed_size))
        .collect::<Vec<_>>();
    if sizes.is_empty() {
        return 1;
    }
    sizes.sort_unstable();
    sizes[sizes.len() * 3 / 4].max(1)
}

#[cfg(test)]
fn heuristic_order_flow_events(
    snapshot: &OrderFlowPreviewSnapshot,
) -> Vec<HeuristicOrderFlowEvent> {
    heuristic_overlay_events_from_addons(&snapshot.add_on_events).unwrap_or_else(|| {
        heuristic_order_flow_events_from_parts(
            &snapshot.visible_liquidity_cells,
            &snapshot.volume_bubbles,
        )
    })
}

fn heuristic_overlay_events_from_addons(
    add_on_events: &[AddOnEventRecord],
) -> Option<Vec<HeuristicOrderFlowEvent>> {
    let mut events = add_on_events
        .iter()
        .filter_map(heuristic_overlay_event_from_addon)
        .collect::<Vec<_>>();
    if events.is_empty() {
        return None;
    }
    events.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.interval_start_ns.cmp(&right.interval_start_ns))
    });
    events.truncate(64);
    Some(events)
}

fn heuristic_overlay_event_from_addon(event: &AddOnEventRecord) -> Option<HeuristicOrderFlowEvent> {
    let price = MboPrice::from_fixed(event.price_fixed?);
    let (kind, note) = if event.kind.contains("iceberg_like") {
        (
            "Iceberg-like",
            "persisted RustChart heuristic iceberg event",
        )
    } else if event.kind.contains("sweep_like") {
        ("Sweep-like", "persisted RustChart heuristic sweep event")
    } else if event.kind.contains("stop_run_like") {
        (
            "Stop-run-like",
            "persisted RustChart heuristic stop-run event",
        )
    } else {
        return None;
    };
    Some(HeuristicOrderFlowEvent {
        kind,
        note,
        interval_start_ns: event.timestamp_ns,
        price,
        score: event.confidence_bps as f32 / 10_000.0,
    })
}

fn heuristic_order_flow_events_from_parts(
    cells: &[(LiquidityCellKey, LiquidityCell)],
    bubbles: &[(VolumeBubbleKey, VolumeBubble)],
) -> Vec<HeuristicOrderFlowEvent> {
    let max_volume = bubbles
        .iter()
        .map(|(_, bubble)| bubble.total_volume())
        .max()
        .unwrap_or_default();
    if max_volume == 0 {
        return Vec::new();
    }
    let volume_threshold = (max_volume / 2).max(1);
    let liquidity_threshold = filtered_liquidity_threshold_for_cells(cells);
    let mut events = Vec::new();

    for (key, bubble) in bubbles {
        let total = bubble.total_volume();
        if total < volume_threshold {
            continue;
        }
        let imbalance = bubble.imbalance().abs() as f32;
        if imbalance >= 0.65 {
            events.push(HeuristicOrderFlowEvent {
                kind: "Stop-run-like",
                note: "large one-sided volume burst",
                interval_start_ns: key.interval_start_ns,
                price: key.price,
                score: imbalance,
            });
        }
        if total >= volume_threshold
            && cells.iter().any(|(cell_key, cell)| {
                cell_key.price == key.price
                    && cell.displayed_size >= liquidity_threshold
                    && cell.displayed_size > 0
            })
        {
            events.push(HeuristicOrderFlowEvent {
                kind: "Iceberg-like",
                note: "large volume into persistent displayed liquidity",
                interval_start_ns: key.interval_start_ns,
                price: key.price,
                score: (total as f32 / max_volume as f32).clamp(0.0, 1.0),
            });
        }
    }

    events.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.interval_start_ns.cmp(&right.interval_start_ns))
    });
    events.truncate(16);
    events
}

fn pulse_metrics_at_cursor(
    snapshot: &OrderFlowPreviewSnapshot,
    cursor_ts: Option<u64>,
) -> Vec<PulseMetric> {
    let mut metrics = Vec::new();
    let flow_summary = flow_summary_at_cursor(snapshot, cursor_ts);
    let total_side_volume = flow_summary.buy_volume + flow_summary.sell_volume;
    if total_side_volume > 0 {
        let pressure = flow_summary.buy_volume as f32 / total_side_volume as f32;
        metrics.push(PulseMetric {
            label: "Volume pressure",
            value: pressure,
            detail: format!(
                "buy {} / sell {}",
                flow_summary.buy_volume, flow_summary.sell_volume
            ),
        });
    }

    let (bid_size, ask_size) = if let Some(cursor_ts) = cursor_ts {
        depth_levels_at_cursor(snapshot, cursor_ts)
            .map(|levels| {
                let bid_size = levels
                    .iter()
                    .filter(|level| level.side == MboSide::Bid)
                    .map(|level| level.size)
                    .sum::<u64>();
                let ask_size = levels
                    .iter()
                    .filter(|level| level.side == MboSide::Ask)
                    .map(|level| level.size)
                    .sum::<u64>();
                (bid_size, ask_size)
            })
            .unwrap_or_default()
    } else {
        snapshot
            .dom_snapshot
            .as_ref()
            .map(|dom| {
                (
                    dom.levels.iter().map(|level| level.bid_size).sum::<u64>(),
                    dom.levels.iter().map(|level| level.ask_size).sum::<u64>(),
                )
            })
            .unwrap_or_default()
    };
    let total_size = bid_size + ask_size;
    if total_size > 0 {
        metrics.push(PulseMetric {
            label: "Book pressure",
            value: bid_size as f32 / total_size as f32,
            detail: format!("bid depth {bid_size} / ask depth {ask_size}"),
        });
    }

    if let Some(book) = &snapshot.book_snapshot
        && let Some(spread) = book.top.spread
    {
        let normalized = (1.0 / (1.0 + spread.max(0.0))) as f32;
        metrics.push(PulseMetric {
            label: "Spread tightness",
            value: normalized.clamp(0.0, 1.0),
            detail: format!("spread {spread:.2}"),
        });
    }

    let first_mid = snapshot
        .bbo_transitions
        .iter()
        .filter(|transition| cursor_ts.is_none_or(|cursor_ts| transition.ts_event <= cursor_ts))
        .find_map(|transition| transition.mid);
    let last_mid = snapshot
        .bbo_transitions
        .iter()
        .rev()
        .filter(|transition| cursor_ts.is_none_or(|cursor_ts| transition.ts_event <= cursor_ts))
        .find_map(|transition| transition.mid);
    if let (Some(first_mid), Some(last_mid)) = (first_mid, last_mid) {
        let drift = last_mid - first_mid;
        let value = (0.5 + drift.signum() as f32 * 0.25).clamp(0.0, 1.0);
        metrics.push(PulseMetric {
            label: "Price change",
            value,
            detail: format!("mid drift {drift:.2}"),
        });
    }

    metrics
}

fn pulse_metrics_for_replay_window(
    snapshot: &OrderFlowPreviewSnapshot,
    audit: &ReplayAuditSnapshot,
    derived_window: Option<&DerivedReplayWindowSnapshot>,
) -> Vec<PulseMetric> {
    let bubbles = replay_volume_bubbles(snapshot, audit, derived_window);
    let flow_summary = flow_summary_from_bubbles(&bubbles);
    let dom = replay_dom_snapshot(snapshot, derived_window);
    let bbo = replay_bbo_transitions(snapshot, audit, derived_window);
    let mut metrics = Vec::new();

    let total_side_volume = flow_summary.buy_volume + flow_summary.sell_volume;
    if total_side_volume > 0 {
        let pressure = flow_summary.buy_volume as f32 / total_side_volume as f32;
        metrics.push(PulseMetric {
            label: "Volume pressure",
            value: pressure,
            detail: format!(
                "buy {} / sell {}",
                flow_summary.buy_volume, flow_summary.sell_volume
            ),
        });
    }

    let (bid_size, ask_size) = dom
        .map(|dom| {
            (
                dom.levels.iter().map(|level| level.bid_size).sum::<u64>(),
                dom.levels.iter().map(|level| level.ask_size).sum::<u64>(),
            )
        })
        .unwrap_or_else(|| {
            let levels = replay_depth_levels(snapshot, Some(audit), derived_window);
            (
                levels
                    .iter()
                    .filter(|level| level.side == MboSide::Bid)
                    .map(|level| level.size)
                    .sum::<u64>(),
                levels
                    .iter()
                    .filter(|level| level.side == MboSide::Ask)
                    .map(|level| level.size)
                    .sum::<u64>(),
            )
        });
    let total_size = bid_size + ask_size;
    if total_size > 0 {
        metrics.push(PulseMetric {
            label: "Book pressure",
            value: bid_size as f32 / total_size as f32,
            detail: format!("bid depth {bid_size} / ask depth {ask_size}"),
        });
    }

    let spread = dom
        .and_then(|dom| match (dom.best_bid, dom.best_ask) {
            (Some(bid), Some(ask)) => {
                Some((ask.price.fixed() - bid.price.fixed()) as f64 / FIXED_PRICE_SCALE as f64)
            }
            _ => None,
        })
        .or_else(|| {
            snapshot
                .book_snapshot
                .as_ref()
                .and_then(|book| book.top.spread)
        });
    if let Some(spread) = spread {
        let normalized = (1.0 / (1.0 + spread.max(0.0))) as f32;
        metrics.push(PulseMetric {
            label: "Spread tightness",
            value: normalized.clamp(0.0, 1.0),
            detail: format!("spread {spread:.2}"),
        });
    }

    let first_mid = bbo.iter().find_map(|transition| transition.mid);
    let last_mid = bbo.iter().rev().find_map(|transition| transition.mid);
    if let (Some(first_mid), Some(last_mid)) = (first_mid, last_mid) {
        let drift = last_mid - first_mid;
        let value = (0.5 + drift.signum() as f32 * 0.25).clamp(0.0, 1.0);
        metrics.push(PulseMetric {
            label: "Price change",
            value,
            detail: format!("mid drift {drift:.2}"),
        });
    }

    metrics
}

fn flow_summary_at_cursor(
    snapshot: &OrderFlowPreviewSnapshot,
    cursor_ts: Option<u64>,
) -> FlowSummary {
    let Some(cursor_ts) = cursor_ts else {
        return snapshot.flow_summary;
    };

    let mut summary = FlowSummary::default();
    for (_, bubble) in snapshot
        .volume_bubbles
        .iter()
        .filter(|(key, _)| key.interval_start_ns <= cursor_ts)
    {
        summary.buy_volume += bubble.buy_volume;
        summary.sell_volume += bubble.sell_volume;
        summary.unknown_volume += bubble.unknown_volume;
        summary.trade_count += bubble.trade_count;
        summary.fill_count += bubble.fill_count;
        summary.cumulative_delta += bubble.delta();
    }
    summary
}

fn draw_market_pulse_overlay_widgets(
    painter: &egui::Painter,
    rect: egui::Rect,
    metrics: &[PulseMetric],
) {
    let panel_size = egui::vec2(172.0_f32.min(rect.width() * 0.40), 112.0);
    let gap = 12.0;
    let total_width = panel_size.x * 2.0 + gap;
    let left = (rect.center().x - total_width * 0.5).max(rect.left() + 12.0);
    let top =
        (rect.center().y - panel_size.y * 0.18).clamp(rect.top() + 26.0, rect.bottom() - 120.0);
    let first_rect = egui::Rect::from_min_size(egui::pos2(left, top), panel_size);
    let second_rect =
        egui::Rect::from_min_size(egui::pos2(left + panel_size.x + gap, top), panel_size);
    let split = metrics.len().div_ceil(2).max(1);
    draw_market_pulse_widget(
        painter,
        first_rect,
        "10min Training",
        &metrics[..split.min(metrics.len())],
    );
    if metrics.len() > split {
        draw_market_pulse_widget(
            painter,
            second_rect,
            "2h Training Period",
            &metrics[split..],
        );
    } else {
        draw_market_pulse_widget(painter, second_rect, "2h Training Period", metrics);
    }
    draw_market_pulse_callout(painter, rect, second_rect);
}

fn draw_bookmap_dom_pro_panel(
    painter: &egui::Painter,
    chart_rect: egui::Rect,
    min_price: i64,
    max_price: i64,
    dom: &DomColumnSnapshot,
) {
    let panel_width = (chart_rect.width() * 0.24).clamp(160.0, 260.0);
    let panel_height = (chart_rect.height() * 0.58).clamp(220.0, 380.0);
    let panel_rect = egui::Rect::from_min_size(
        egui::pos2(
            chart_rect.right() - panel_width - 12.0,
            chart_rect.top() + 42.0,
        ),
        egui::vec2(panel_width, panel_height),
    )
    .intersect(chart_rect.shrink(8.0));
    draw_dom_pro_surface(painter, panel_rect, min_price, max_price, dom);
}

fn draw_bookmap_tradermap_split_overlay(
    painter: &egui::Painter,
    chart_rect: egui::Rect,
    min_ts: u64,
    max_ts: u64,
    min_price: i64,
    max_price: i64,
    cells: &[(LiquidityCellKey, LiquidityCell)],
    bbo: &[BboTransition],
    bubbles: &[(VolumeBubbleKey, VolumeBubble)],
) {
    let threshold = filtered_liquidity_threshold_for_cells(cells);
    let filtered_cells = cells
        .iter()
        .copied()
        .filter(|(_, cell)| cell.displayed_size >= threshold && cell.displayed_size > 0)
        .collect::<Vec<_>>();
    let right_rect = egui::Rect::from_min_max(
        egui::pos2(chart_rect.center().x, chart_rect.top()),
        chart_rect.max,
    );
    painter.rect_filled(
        right_rect,
        0.0,
        egui::Color32::from_rgba_premultiplied(4, 8, 10, 172),
    );
    painter.line_segment(
        [
            egui::pos2(right_rect.left(), right_rect.top()),
            egui::pos2(right_rect.left(), right_rect.bottom()),
        ],
        egui::Stroke::new(2.0, egui::Color32::from_rgb(90, 171, 206)),
    );
    draw_heatmap_cell_surface(
        painter,
        right_rect,
        min_ts,
        max_ts,
        min_price,
        max_price,
        &filtered_cells,
    );
    draw_bbo_paths(
        painter, right_rect, min_ts, max_ts, min_price, max_price, bbo,
    );
    draw_volume_bubbles(
        painter, right_rect, min_ts, max_ts, min_price, max_price, bubbles,
    );
    let label = format!("TRADERMAP FILTER  size {threshold}+");
    let label_rect = egui::Rect::from_min_size(
        egui::pos2(right_rect.left() + 10.0, right_rect.top() + 10.0),
        egui::vec2(
            (label.len() as f32 * 7.0 + 18.0).min(right_rect.width() - 20.0),
            24.0,
        ),
    );
    painter.rect_filled(
        label_rect,
        2.0,
        egui::Color32::from_rgba_premultiplied(238, 242, 246, 226),
    );
    painter.text(
        label_rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::monospace(10.0),
        egui::Color32::from_rgb(16, 18, 20),
    );
}

fn draw_market_pulse_widget(
    painter: &egui::Painter,
    rect: egui::Rect,
    title: &str,
    metrics: &[PulseMetric],
) {
    painter.rect_filled(
        rect.translate(egui::vec2(2.0, 3.0)),
        2.0,
        egui::Color32::from_rgba_premultiplied(0, 0, 0, 80),
    );
    painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(53, 68, 74));
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(83, 99, 108)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        egui::pos2(rect.left() + 8.0, rect.top() + 13.0),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::monospace(10.0),
        egui::Color32::from_rgb(230, 234, 238),
    );
    painter.text(
        egui::pos2(rect.right() - 8.0, rect.top() + 13.0),
        egui::Align2::RIGHT_CENTER,
        "x",
        egui::FontId::monospace(13.0),
        egui::Color32::from_rgb(232, 112, 86),
    );
    let rows = market_pulse_widget_rows(metrics);
    for (index, row_model) in rows.iter().take(3).enumerate() {
        let row = egui::Rect::from_min_size(
            egui::pos2(rect.left() + 10.0, rect.top() + 32.0 + index as f32 * 23.0),
            egui::vec2(rect.width() - 20.0, 18.0),
        );
        draw_market_pulse_row(painter, row, row_model);
    }
}

fn market_pulse_widget_rows(metrics: &[PulseMetric]) -> Vec<PulseWidgetRow> {
    let mut rows = Vec::new();
    for metric in metrics {
        match metric.label {
            "Volume pressure" => {
                rows.push(pulse_row("Agg buys", metric.value, PulseRowBias::Buy));
                rows.push(pulse_row(
                    "Agg sells",
                    1.0 - metric.value,
                    PulseRowBias::Sell,
                ));
            }
            "Book pressure" => {
                rows.push(pulse_row("Bid depth", metric.value, PulseRowBias::Buy));
                rows.push(pulse_row(
                    "Ask depth",
                    1.0 - metric.value,
                    PulseRowBias::Sell,
                ));
            }
            "Spread tightness" => {
                rows.push(pulse_row("Spread", metric.value, PulseRowBias::Neutral));
            }
            "Price change" => {
                let bias = if metric.value >= 0.5 {
                    PulseRowBias::Buy
                } else {
                    PulseRowBias::Sell
                };
                rows.push(pulse_row("Price move", metric.value, bias));
            }
            _ => rows.push(pulse_row(metric.label, metric.value, PulseRowBias::Neutral)),
        }
    }
    if rows.len() < 3 {
        rows.push(PulseWidgetRow {
            label: "External algos".to_string(),
            value_label: "n/a".to_string(),
            value: None,
            bias: PulseRowBias::Unavailable,
        });
    }
    rows
}

fn pulse_row(label: &str, value: f32, bias: PulseRowBias) -> PulseWidgetRow {
    let value = value.clamp(0.0, 1.0);
    PulseWidgetRow {
        label: label.to_string(),
        value_label: format!("{value:.2}"),
        value: Some(value),
        bias,
    }
}

fn draw_market_pulse_row(painter: &egui::Painter, rect: egui::Rect, row: &PulseWidgetRow) {
    painter.text(
        egui::pos2(rect.left(), rect.top() - 2.0),
        egui::Align2::LEFT_BOTTOM,
        &row.label,
        egui::FontId::monospace(9.0),
        egui::Color32::from_rgb(230, 234, 238),
    );
    let value_width = 50.0_f32.min(rect.width() * 0.42);
    let bar_rect =
        egui::Rect::from_min_max(egui::pos2(rect.right() - value_width, rect.top()), rect.max);
    painter.rect_filled(bar_rect, 2.0, egui::Color32::from_rgb(74, 88, 96));
    if let Some(value) = row.value {
        let fill_rect = egui::Rect::from_min_max(
            bar_rect.min,
            egui::pos2(
                egui::lerp(bar_rect.left()..=bar_rect.right(), value),
                bar_rect.bottom(),
            ),
        );
        painter.rect_filled(fill_rect, 2.0, market_pulse_row_color(row.bias, value));
    }
    painter.text(
        bar_rect.center(),
        egui::Align2::CENTER_CENTER,
        &row.value_label,
        egui::FontId::monospace(12.0),
        egui::Color32::from_rgb(246, 248, 250),
    );
}

fn draw_market_pulse_callout(painter: &egui::Painter, rect: egui::Rect, target: egui::Rect) {
    let start = egui::pos2(
        rect.right() - rect.width() * 0.22,
        rect.top() + rect.height() * 0.24,
    );
    let end = egui::pos2(target.center().x, target.top() - 8.0);
    painter.line_segment(
        [start, end],
        egui::Stroke::new(6.0, egui::Color32::from_rgba_premultiplied(0, 0, 0, 150)),
    );
    painter.line_segment(
        [start, end],
        egui::Stroke::new(3.0, egui::Color32::from_rgb(245, 247, 250)),
    );
    painter.circle_filled(end, 4.0, egui::Color32::from_rgb(245, 247, 250));
}

fn market_pulse_bar_color(value: f32) -> egui::Color32 {
    if value >= 0.5 {
        egui::Color32::from_rgb(106, 220, 155)
    } else {
        egui::Color32::from_rgb(226, 112, 104)
    }
}

fn market_pulse_row_color(bias: PulseRowBias, value: f32) -> egui::Color32 {
    match bias {
        PulseRowBias::Buy => egui::Color32::from_rgb(106, 220, 155),
        PulseRowBias::Sell => egui::Color32::from_rgb(226, 112, 104),
        PulseRowBias::Neutral => market_pulse_bar_color(value),
        PulseRowBias::Unavailable => egui::Color32::from_rgb(118, 122, 128),
    }
}

fn show_order_flow_error(ui: &mut egui::Ui, kind: WidgetKind, error: &str) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(kind.mnemonic())
                .monospace()
                .size(12.0)
                .color(egui::Color32::from_rgb(232, 178, 88)),
        );
        ui.label(
            egui::RichText::new(kind.label())
                .monospace()
                .size(17.0)
                .color(egui::Color32::from_rgb(230, 232, 235)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new("offline")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(242, 92, 102)),
            );
        });
    });
    ui.add_space(10.0);
    ui.separator();
    ui.add_space(8.0);
    empty_state(ui, error);
}

fn show_order_flow_loading(ui: &mut egui::Ui, kind: WidgetKind) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(kind.mnemonic())
                .monospace()
                .size(12.0)
                .color(egui::Color32::from_rgb(232, 178, 88)),
        );
        ui.label(
            egui::RichText::new(kind.label())
                .monospace()
                .size(17.0)
                .color(egui::Color32::from_rgb(230, 232, 235)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new("loading")
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(232, 178, 88)),
            );
        });
    });
    ui.add_space(10.0);
    ui.separator();
    ui.add_space(8.0);
    empty_state(ui, "Loading local historical MBO cache in the background");
}

fn show_book_side(ui: &mut egui::Ui, label: &str, levels: &[PriceLevel], reverse: bool) {
    ui.label(
        egui::RichText::new(label)
            .monospace()
            .size(12.0)
            .color(egui::Color32::from_rgb(150, 154, 160)),
    );
    ui.add_space(4.0);

    let mut rows = levels.iter().copied().collect::<Vec<_>>();
    if reverse {
        rows.reverse();
    }
    if rows.is_empty() {
        empty_state(ui, "--");
        return;
    }

    for level in rows {
        let value = format!("{}  {}", format_mbo_price(level.price), level.size);
        row_band(ui, &value, side_color(label));
        ui.add_space(3.0);
    }
}

fn depth_sample_row(ui: &mut egui::Ui, level: &DepthLevelSample, max_size: u64) {
    let row_height = 24.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), row_height),
        egui::Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, 0.0, egui::Color32::from_rgb(20, 20, 20));
    let width_factor = if max_size == 0 {
        0.0
    } else {
        level.size as f32 / max_size as f32
    };
    let fill_rect = egui::Rect::from_min_size(
        rect.min,
        egui::vec2(rect.width() * width_factor.clamp(0.0, 1.0), row_height),
    );
    ui.painter().rect_filled(
        fill_rect,
        0.0,
        side_fill_color(level.side).linear_multiply(0.45),
    );
    ui.painter().text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        side_label(level.side),
        egui::FontId::monospace(12.0),
        egui::Color32::from_rgb(230, 232, 235),
    );
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        format_mbo_price(level.price),
        egui::FontId::monospace(12.0),
        egui::Color32::from_rgb(230, 232, 235),
    );
    ui.painter().text(
        egui::pos2(rect.right() - 8.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        level.size.to_string(),
        egui::FontId::monospace(12.0),
        egui::Color32::from_rgb(230, 232, 235),
    );
}

fn footprint_row(ui: &mut egui::Ui, key: FootprintCellKey, cell: FootprintCell) {
    let value = format!(
        "{}  B {}  S {}  D {}",
        format_mbo_price(key.price),
        cell.buy_volume,
        cell.sell_volume,
        cell.delta()
    );
    let color = if cell.delta() >= 0 {
        egui::Color32::from_rgb(52, 147, 96)
    } else {
        egui::Color32::from_rgb(150, 57, 70)
    };
    row_band(ui, &value, color);
}

fn row_band(ui: &mut egui::Ui, text: &str, color: egui::Color32) {
    let row_height = 24.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), row_height),
        egui::Sense::hover(),
    );
    ui.painter()
        .rect_filled(rect, 0.0, egui::Color32::from_rgb(20, 20, 20));
    ui.painter().rect_filled(
        egui::Rect::from_min_size(rect.min, egui::vec2(3.0, row_height)),
        0.0,
        color,
    );
    ui.painter().text(
        egui::pos2(rect.left() + 9.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::monospace(12.0),
        egui::Color32::from_rgb(230, 232, 235),
    );
}

fn metric_label(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(
        egui::RichText::new(format!("{label} {value}"))
            .monospace()
            .size(12.0)
            .color(egui::Color32::from_rgb(196, 199, 205)),
    );
}

fn compact_status_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(118, 122, 128)),
        );
        ui.label(
            egui::RichText::new(value)
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(196, 199, 205)),
        );
    });
}

fn empty_state(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .monospace()
            .size(12.0)
            .color(egui::Color32::from_rgb(150, 154, 160)),
    );
}

fn format_cache_range(entry: &MboCacheEntry) -> String {
    match (&entry.start, &entry.end_exclusive) {
        (Some(start), Some(end)) => format!("{start} -> {end}"),
        (Some(start), None) => format!("{start} -> --"),
        (None, Some(end)) => format!("-- -> {end}"),
        (None, None) => "--".to_string(),
    }
}

fn format_mbo_price(price: MboPrice) -> String {
    price
        .as_f64()
        .map(format_price_value)
        .unwrap_or_else(|| "--".to_string())
}

fn format_price_value(value: f64) -> String {
    format!("{value:.2}")
}

fn book_quality_label(quality: BookQuality) -> &'static str {
    match quality {
        BookQuality::Clean => "clean",
        BookQuality::Suspect => "suspect",
    }
}

fn order_flow_status_color(is_partial: bool) -> egui::Color32 {
    if is_partial {
        egui::Color32::from_rgb(232, 178, 88)
    } else {
        egui::Color32::from_rgb(76, 205, 126)
    }
}

fn timeline_status_label(report: &OrderFlowTimelineRunReport) -> &'static str {
    if report.warning.is_some() {
        "partial"
    } else if report.reached_event_limit {
        "bounded"
    } else if report.exhausted_source {
        "exhausted"
    } else {
        "ready"
    }
}

fn side_label(side: MboSide) -> &'static str {
    match side {
        MboSide::Bid => "BID",
        MboSide::Ask => "ASK",
        MboSide::None | MboSide::Unknown(_) => "--",
    }
}

fn side_color(label: &str) -> egui::Color32 {
    match label {
        "BID" => egui::Color32::from_rgb(52, 147, 96),
        "ASK" => egui::Color32::from_rgb(150, 57, 70),
        _ => egui::Color32::from_rgb(118, 122, 128),
    }
}

fn side_fill_color(side: MboSide) -> egui::Color32 {
    match side {
        MboSide::Bid => egui::Color32::from_rgb(52, 147, 96),
        MboSide::Ask => egui::Color32::from_rgb(150, 57, 70),
        MboSide::None | MboSide::Unknown(_) => egui::Color32::from_rgb(118, 122, 128),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use eframe::egui;

    use super::{
        BOOKMAP_DOM_TEXT_LEVEL_LIMIT, BOOKMAP_FALLBACK_DOM_DEPTH_ROWS,
        BOOKMAP_PRICE_LADDER_LABEL_EDGE_PAD, BOOKMAP_PRICE_LADDER_LABEL_MIN_GAP,
        BOOKMAP_VISIBLE_CURSOR_FRACTION, BookmapChartFoundation, BookmapPriceActionMode,
        BookmapPriceActionSource, DEFAULT_DERIVED_CACHE_ROOT, DEFAULT_REPLAY_CURSOR_FRACTION,
        DEFAULT_REPLAY_WINDOW_INDEX, DERIVED_INITIAL_VIEWPORT_NS, DerivedReplayWindowKey,
        DerivedReplayWindowSnapshot, HeuristicOrderFlowEvent, OrderFlowPreviewSnapshot,
        PREFERRED_DERIVED_CACHE_ROOT, ProviderStatus, PulseMetric, PulseRowBias,
        ReplayAuditSnapshot, ReplayControlState, SECOND_NS, StopsIcebergsBadgeShape,
        bbo_trade_marker_radius, bookmap_active_price_marker, bookmap_aligned_dom_snapshot,
        bookmap_anchor_price_comfort_range, bookmap_bbo_price_action_records,
        bookmap_close_price_points, bookmap_dom_cell_text_fits, bookmap_dom_column_row_metrics,
        bookmap_dom_show_level_text, bookmap_dom_width, bookmap_heatmap_texture_rects,
        bookmap_left_rail_layer_toggle_rects, bookmap_left_rail_width,
        bookmap_major_time_marker_positions, bookmap_micro_price_action_bars,
        bookmap_minor_time_marker_positions, bookmap_price_action_body_width,
        bookmap_price_action_mode, bookmap_price_action_records, bookmap_price_ladder_labels,
        bookmap_price_ladder_row_count, bookmap_replay_control_rects,
        bookmap_sampled_close_price_ticks, bookmap_terminal_layout,
        bookmap_terminal_visible_price_range, bookmap_timestamp_logical,
        bookmap_trade_price_action_records, bookmap_visible_volume_bubbles,
        bubble_is_near_bbo_spread, choose_preview_entry, composite_last_price_marker,
        composite_volume_bubble_radius, current_book_continuation_band_rects,
        cursor_fraction_for_timestamp, default_derived_cache_roots, derived_replay_window_key,
        dom_profile_fill_rect, filter_liquidity_cells_to_time_window,
        filtered_liquidity_threshold_for_cells, format_mbo_price, format_nanosecond_axis_label,
        format_relative_ns, heatmap_color, heuristic_order_flow_events,
        heuristic_order_flow_events_from_parts, heuristic_overlay_events_from_addons,
        initial_derived_viewport_window, load_derived_preview_for_entry,
        load_preview_from_manifest_dir, load_preview_from_paths, market_data_connection_node_rects,
        market_data_hub_rect, market_pulse_bar_color, market_pulse_widget_rows,
        nanosecond_zoom_window, ohlcv_bar_width, persistent_heatmap_levels, price_row_height,
        provider_capability_registry, pulse_metrics_at_cursor, pulse_metrics_for_replay_window,
        query_derived_replay_window, replay_add_on_events, replay_audit, replay_depth_levels,
        replay_liquidity_cells, replay_market_context_price_range, replay_ohlcv_bars,
        replay_speed_visual_label, replay_window_bounds, replay_window_span_ns,
        replay_window_visual_label, stops_icebergs_badge_style, stops_icebergs_event_label,
        timestamp_for_x, tradermap_layout_rects, volume_bubble_buy_split_y, x_for_timestamp,
    };
    use crate::order_flow::{
        AddOnEventRecord, BboTransition, BookQuality, BookmapFrame, BookmapFrameInput,
        BookmapFrameSource, BookmapInstrumentSelection, BookmapLiquidityBand,
        DerivedCacheBuildConfig, DerivedCacheBuildSummary, DerivedCacheManifest, DomColumnLevel,
        DomColumnSnapshot, FIXED_PRICE_SCALE, FlowSummary, LiquidityCell, LiquidityCellKey,
        MboAction, MboCacheEntry, MboEvent, MboPrice, MboSide, MboSourceInfo, OhlcvBarRecord,
        OrderBookSnapshot, OrderFlowReplayConfig, OrderFlowTimelineRunReport, PriceLevel,
        ReplayCheckpointRecord, TopOfBook, VecMboEventSource, VolumeBubble, VolumeBubbleKey,
        build_derived_cache_artifacts, heatmap_band_runs,
    };

    fn entry(symbol: &str, raw_path: &str, bytes_on_disk: Option<u64>) -> MboCacheEntry {
        MboCacheEntry {
            dataset: "GLBX.MDP3".to_string(),
            schema: "mbo".to_string(),
            symbol: symbol.to_string(),
            stype_in: "parent".to_string(),
            stype_out: "instrument_id".to_string(),
            start: Some("2024-11-29T00:00:00Z".to_string()),
            end_exclusive: Some("2024-12-03T00:00:00Z".to_string()),
            raw_path: PathBuf::from(raw_path),
            estimated_record_count: None,
            estimated_cost_usd: None,
            estimated_billable_size_bytes: None,
            bytes_on_disk,
        }
    }

    fn price(value: i64) -> MboPrice {
        MboPrice::from_fixed(value * FIXED_PRICE_SCALE)
    }

    #[test]
    fn bookmap_heatmap_ramp_reaches_yellow_white_peak_bands() {
        let cold = heatmap_color(MboSide::None, 0.04);
        let cyan = heatmap_color(MboSide::None, 0.35);
        let warm = heatmap_color(MboSide::None, 0.68);
        let red_orange = heatmap_color(MboSide::None, 0.78);
        let peak = heatmap_color(MboSide::None, 0.98);

        assert!(cold.b() > cold.r(), "cold liquidity should stay blue");
        assert!(
            cyan.g() > cyan.r() && cyan.b() > cyan.r(),
            "mid liquidity should pass through cyan"
        );
        assert!(
            warm.r() > warm.b() && warm.g() > warm.b(),
            "warm liquidity should pass through yellow/orange"
        );
        assert!(
            red_orange.r() > red_orange.g() && red_orange.r() > red_orange.b(),
            "hot transition liquidity should pass through red/orange"
        );
        assert!(
            peak.r() >= 250 && peak.g() >= 225 && peak.b() >= 180,
            "peak liquidity should reach the PDF-like yellow/white class"
        );
    }

    #[test]
    fn heatmap_band_helpers_extend_to_next_matching_level_and_normalize() {
        let first = liquidity_cell_at(10, 100, 10);
        let different_price = liquidity_cell_at(20, 101, 10);
        let different_side = (
            LiquidityCellKey {
                interval_start_ns: 30,
                instrument_id: 1,
                price: price(100),
                side: MboSide::Ask,
            },
            LiquidityCell {
                displayed_size: 10,
                max_displayed_size: 10,
                samples: 1,
                intensity: 0.25,
            },
        );
        let next_same_level = liquidity_cell_at(40, 100, 10);
        let cells = vec![first, different_price, different_side, next_same_level];
        let bands = heatmap_band_runs(&cells, 99);

        assert_eq!(bands[0].end_ts, 40);
        assert_eq!(bands[1].end_ts, 99);
        assert!((bands[2].normalized_intensity - 0.5).abs() < 0.001);
        assert_eq!(bands[0].normalized_intensity, 1.0);
    }

    #[test]
    fn persistent_heatmap_levels_rank_repeated_price_liquidity() {
        let cells = vec![
            liquidity_cell_at(10, 100, 3),
            liquidity_cell_at(20, 101, 8),
            liquidity_cell_at(30, 100, 9),
            (
                LiquidityCellKey {
                    interval_start_ns: 40,
                    instrument_id: 1,
                    price: price(100),
                    side: MboSide::Ask,
                },
                LiquidityCell {
                    displayed_size: 7,
                    max_displayed_size: 10,
                    samples: 1,
                    intensity: 0.49,
                },
            ),
        ];
        let bands = heatmap_band_runs(&cells, 60);
        let levels = persistent_heatmap_levels(&bands, 2);

        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0].price, price(100));
        assert_eq!(levels[0].side, MboSide::Bid);
        assert_eq!(levels[0].displayed_size, 12);
        assert_eq!(levels[1].price, price(101));
    }

    #[test]
    fn bookmap_heatmap_texture_rects_are_bounded_and_inside_viewport() {
        let rect = egui::Rect::from_min_size(egui::pos2(20.0, 40.0), egui::vec2(1_100.0, 520.0));
        let bands = (0..160)
            .map(|index| BookmapLiquidityBand {
                instrument_id: 1,
                start_ts: index * 5,
                end_ts: index * 5 + 120,
                price: price(100 + (index % 18) as i64),
                side: if index % 2 == 0 {
                    MboSide::Bid
                } else {
                    MboSide::Ask
                },
                displayed_size: 10 + index,
                normalized_intensity: (0.18 + (index % 10) as f32 * 0.08).min(1.0),
            })
            .collect::<Vec<_>>();

        let cells = bookmap_heatmap_texture_rects(
            rect,
            0,
            1_200,
            price(95).fixed(),
            price(125).fixed(),
            &bands,
            128,
        );

        assert!(!cells.is_empty());
        assert!(cells.len() <= 128);
        assert!(cells.iter().all(|cell| rect.contains_rect(cell.rect)));
        assert!(cells.iter().all(|cell| cell.source_price.is_defined()));
    }

    #[test]
    fn bookmap_heatmap_texture_rects_weight_stronger_liquidity() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 400.0));
        let weak = BookmapLiquidityBand {
            instrument_id: 1,
            start_ts: 0,
            end_ts: 1_000,
            price: price(100),
            side: MboSide::Bid,
            displayed_size: 2,
            normalized_intensity: 0.12,
        };
        let strong = BookmapLiquidityBand {
            instrument_id: 1,
            start_ts: 0,
            end_ts: 1_000,
            price: price(101),
            side: MboSide::Ask,
            displayed_size: 200,
            normalized_intensity: 0.96,
        };

        let cells = bookmap_heatmap_texture_rects(
            rect,
            0,
            1_000,
            price(95).fixed(),
            price(110).fixed(),
            &[weak, strong],
            512,
        );
        let weak_count = cells
            .iter()
            .filter(|cell| cell.source_price == weak.price)
            .count();
        let strong_count = cells
            .iter()
            .filter(|cell| cell.source_price == strong.price)
            .count();

        assert!(strong_count > weak_count);
        assert!(
            strong_count > 32,
            "strong liquidity should create visible texture"
        );
    }

    #[test]
    fn bookmap_heatmap_texture_rects_cover_the_visible_history() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(1_600.0, 640.0));
        let bands = (0..720)
            .map(|index| {
                let time_bucket = index / 24;
                let price_bucket = index % 24;
                BookmapLiquidityBand {
                    instrument_id: 1,
                    start_ts: time_bucket * 60,
                    end_ts: time_bucket * 60 + 160,
                    price: price(100 + price_bucket as i64),
                    side: if price_bucket % 2 == 0 {
                        MboSide::Bid
                    } else {
                        MboSide::Ask
                    },
                    displayed_size: 20 + ((index * 13) % 220) as u64,
                    normalized_intensity: (0.20 + (price_bucket % 9) as f32 * 0.085).min(1.0),
                }
            })
            .collect::<Vec<_>>();

        let cells = bookmap_heatmap_texture_rects(
            rect,
            0,
            30 * 60,
            price(96).fixed(),
            price(130).fixed(),
            &bands,
            2_400,
        );
        let mut quarters = [false; 4];
        for cell in &cells {
            let center_x = cell.rect.center().x;
            let fraction = ((center_x - rect.left()) / rect.width()).clamp(0.0, 0.999);
            quarters[(fraction * 4.0) as usize] = true;
        }

        assert!(!cells.is_empty());
        assert!(cells.len() <= 2_400);
        assert!(
            quarters.iter().all(|covered| *covered),
            "texture should not collapse into one localized region"
        );
        assert!(cells.iter().all(|cell| rect.contains_rect(cell.rect)));
    }

    #[test]
    fn current_book_continuation_rects_are_price_aligned_and_size_scaled() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(1_000.0, 600.0));
        let min_ts = 0;
        let max_ts = 100;
        let bands = vec![
            BookmapLiquidityBand {
                instrument_id: 1,
                start_ts: 76,
                end_ts: 100,
                price: price(101),
                side: MboSide::Bid,
                displayed_size: 25,
                normalized_intensity: 0.25,
            },
            BookmapLiquidityBand {
                instrument_id: 1,
                start_ts: 76,
                end_ts: 100,
                price: price(103),
                side: MboSide::Ask,
                displayed_size: 100,
                normalized_intensity: 1.0,
            },
        ];

        let draw_records = current_book_continuation_band_rects(
            rect,
            min_ts,
            max_ts,
            price(95).fixed(),
            price(110).fixed(),
            &bands,
        );

        assert_eq!(draw_records.len(), 2);
        assert!(draw_records.iter().all(|record| {
            record.base_rect.left() >= rect.left()
                && record.base_rect.right() <= rect.right()
                && record.profile_rect.left() == record.base_rect.left()
                && record.profile_rect.right() <= record.base_rect.right()
                && record.base_rect.height()
                    <= price_row_height(rect, price(95).fixed(), price(110).fixed())
        }));
        assert!(
            draw_records[0].base_rect.center().y < draw_records[1].base_rect.center().y,
            "higher ask price should map higher on the shared price scale"
        );
        assert!(
            draw_records[0].profile_rect.width() > draw_records[1].profile_rect.width(),
            "larger current-book size should produce a longer profile segment"
        );
        assert_eq!(draw_records[0].base_rect.right(), rect.right());
    }

    #[test]
    fn current_book_continuation_rects_ignore_empty_or_undefined_levels() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(600.0, 300.0));
        let bands = vec![
            BookmapLiquidityBand {
                instrument_id: 1,
                start_ts: 50,
                end_ts: 100,
                price: price(101),
                side: MboSide::Bid,
                displayed_size: 0,
                normalized_intensity: 1.0,
            },
            BookmapLiquidityBand {
                instrument_id: 1,
                start_ts: 50,
                end_ts: 100,
                price: MboPrice::UNDEFINED,
                side: MboSide::Ask,
                displayed_size: 10,
                normalized_intensity: 1.0,
            },
            BookmapLiquidityBand {
                instrument_id: 1,
                start_ts: 50,
                end_ts: 100,
                price: price(102),
                side: MboSide::Ask,
                displayed_size: 10,
                normalized_intensity: 0.5,
            },
        ];

        let draw_records = current_book_continuation_band_rects(
            rect,
            0,
            100,
            price(95).fixed(),
            price(110).fixed(),
            &bands,
        );

        assert_eq!(draw_records.len(), 1);
        assert_eq!(draw_records[0].side, MboSide::Ask);
    }

    #[test]
    fn bookmap_grid_planner_uses_dense_minor_and_major_markers() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 420.0));

        let minor = bookmap_minor_time_marker_positions(rect);
        let major = bookmap_major_time_marker_positions(rect);
        let rows =
            bookmap_price_ladder_row_count(rect, 100 * FIXED_PRICE_SCALE, 120 * FIXED_PRICE_SCALE);

        assert_eq!(minor.len(), 11);
        assert_eq!(major.len(), 5);
        assert!(minor.iter().all(|x| *x > rect.left() && *x < rect.right()));
        assert!(major.iter().all(|x| *x > rect.left() && *x < rect.right()));
        assert!(rows >= 24, "Bookmap grid should expose dense price rows");
    }

    #[test]
    fn bookmap_dom_width_scales_with_composite_viewport() {
        assert!((bookmap_dom_width(420.0) - 126.0).abs() < 0.001);
        assert!((bookmap_dom_width(1_000.0) - 170.0).abs() < 0.001);
        assert!((bookmap_dom_width(1_400.0) - 238.0).abs() < 0.001);
    }

    #[test]
    fn bookmap_price_ladder_labels_provide_continuous_row_context() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(72.0, 620.0));
        let labels =
            bookmap_price_ladder_labels(rect, 100 * FIXED_PRICE_SCALE, 130 * FIXED_PRICE_SCALE);

        assert!(labels.len() >= 24);
        assert!(labels.windows(2).all(|pair| pair[0].1 < pair[1].1));
        assert!(
            labels
                .windows(2)
                .all(|pair| pair[0].0.fixed() > pair[1].0.fixed())
        );
    }

    #[test]
    fn bookmap_price_ladder_labels_avoid_edge_clipping_and_overlap() {
        let rect = egui::Rect::from_min_size(egui::pos2(4.0, 8.0), egui::vec2(72.0, 180.0));
        let labels =
            bookmap_price_ladder_labels(rect, 100 * FIXED_PRICE_SCALE, 130 * FIXED_PRICE_SCALE);

        assert!(!labels.is_empty());
        assert!(labels.iter().all(|(_, y)| {
            *y >= rect.top() + BOOKMAP_PRICE_LADDER_LABEL_EDGE_PAD
                && *y <= rect.bottom() - BOOKMAP_PRICE_LADDER_LABEL_EDGE_PAD
        }));
        assert!(
            labels
                .windows(2)
                .all(|pair| pair[1].1 - pair[0].1 >= BOOKMAP_PRICE_LADDER_LABEL_MIN_GAP - 0.001)
        );
    }

    #[test]
    fn bookmap_dom_column_metrics_suppress_text_when_rows_are_too_dense() {
        let wide_body = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(62.0, 620.0));
        let dense = bookmap_dom_column_row_metrics(
            wide_body,
            100 * FIXED_PRICE_SCALE,
            170 * FIXED_PRICE_SCALE,
        );
        let readable = bookmap_dom_column_row_metrics(
            wide_body,
            100 * FIXED_PRICE_SCALE,
            130 * FIXED_PRICE_SCALE,
        );
        let narrow_body = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(28.0, 620.0));
        let narrow = bookmap_dom_column_row_metrics(
            narrow_body,
            100 * FIXED_PRICE_SCALE,
            130 * FIXED_PRICE_SCALE,
        );

        assert!(!dense.show_row_text);
        assert!(dense.row_height < 13.5);
        assert!(readable.show_row_text);
        assert!(bookmap_dom_show_level_text(
            readable,
            BOOKMAP_DOM_TEXT_LEVEL_LIMIT
        ));
        assert!(!bookmap_dom_show_level_text(
            readable,
            BOOKMAP_DOM_TEXT_LEVEL_LIMIT + 1
        ));
        assert!(!narrow.show_row_text);
        assert!(bookmap_dom_cell_text_fits(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(48.0, 14.0)
        )));
        assert!(!bookmap_dom_cell_text_fits(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(31.0, 14.0)
        )));
        assert!(!bookmap_dom_cell_text_fits(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(48.0, 11.0)
        )));
    }

    #[test]
    fn bookmap_left_rail_width_matches_actual_terminal_density() {
        assert!((bookmap_left_rail_width(600.0) - 96.0).abs() < 0.001);
        assert!((bookmap_left_rail_width(1_200.0) - 172.0).abs() < 0.001);
        assert!((bookmap_left_rail_width(2_400.0) - 230.0).abs() < 0.001);
    }

    #[test]
    fn bookmap_terminal_layout_keeps_terminal_regions_non_overlapping() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1_200.0, 700.0));
        let layout = bookmap_terminal_layout(rect);

        assert_eq!(layout.toolbar_rect.top(), rect.top());
        assert!(layout.left_rail_rect.left() >= rect.left());
        assert!(layout.left_rail_rect.right() <= layout.chart_rect.left());
        assert!(layout.chart_rect.right() <= layout.price_ladder_rect.left());
        assert_eq!(layout.price_ladder_rect.right(), layout.cob_rect.left());
        assert_eq!(layout.cob_rect.right(), layout.cvp_rect.left());
        assert_eq!(layout.cvp_rect.right(), layout.svp_rect.left());
        assert_eq!(layout.axis_rect.left(), layout.chart_rect.left());
        assert_eq!(layout.axis_rect.right(), layout.chart_rect.right());
        assert_eq!(layout.volume_rect.top(), layout.chart_rect.bottom());
        assert_eq!(layout.axis_rect.top(), layout.volume_rect.bottom());
        assert_eq!(layout.status_rect.top(), layout.left_rail_rect.bottom());
        assert!(layout.right_panel_rect.right() <= rect.right());
        assert!(layout.chart_rect.height() > layout.axis_rect.height());
    }

    #[test]
    fn bookmap_interaction_rects_stay_inside_terminal_chrome() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1_200.0, 700.0));
        let layout = bookmap_terminal_layout(rect);
        let replay = bookmap_replay_control_rects(layout.toolbar_rect);
        let layer_rects = bookmap_left_rail_layer_toggle_rects(layout.left_rail_rect);

        assert!(layout.toolbar_rect.contains_rect(replay.play_rect));
        assert!(layout.toolbar_rect.contains_rect(replay.step_rect));
        assert!(layout.toolbar_rect.contains_rect(replay.reset_rect));
        assert_eq!(layer_rects.len(), 5);
        assert!(
            layer_rects
                .iter()
                .all(|(_, row)| layout.left_rail_rect.contains_rect(*row))
        );
    }

    #[test]
    fn bookmap_scrub_math_maps_chart_x_to_replay_fraction() {
        let rect = egui::Rect::from_min_size(egui::pos2(100.0, 0.0), egui::vec2(400.0, 100.0));

        let timestamp = timestamp_for_x(rect, 1_000, 2_000, 300.0);

        assert_eq!(timestamp, 1_500);
        assert!((cursor_fraction_for_timestamp(1_000, 2_000, timestamp) - 0.5).abs() < 0.001);
        assert_eq!(timestamp_for_x(rect, 1_000, 2_000, 50.0), 1_000);
        assert_eq!(cursor_fraction_for_timestamp(1_000, 2_000, 3_000), 1.0);
    }

    #[test]
    fn composite_volume_bubbles_use_compact_viewport_relative_cap() {
        let bubble = VolumeBubble {
            buy_volume: 100,
            sell_volume: 0,
            unknown_volume: 0,
            trade_count: 1,
            fill_count: 1,
        };
        let small_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, 300.0));
        let large_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1_000.0, 500.0));

        assert!((composite_volume_bubble_radius(bubble, 100, small_rect) - 4.2).abs() < 0.001);
        assert!((composite_volume_bubble_radius(bubble, 100, large_rect) - 7.0).abs() < 0.001);
    }

    #[test]
    fn ohlcv_bar_width_tracks_visible_time_scale() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(600.0, 80.0));
        let bars = vec![ohlcv_bar(0, 100), ohlcv_bar(10, 101), ohlcv_bar(20, 102)];

        assert_eq!(ohlcv_bar_width(rect, 0, 60, &bars), 18.0);
        assert_eq!(ohlcv_bar_width(rect, 0, 1_000, &bars), 4.32);
    }

    #[test]
    fn sparse_bookmap_ohlcv_uses_compact_price_action_path() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1_400.0, 700.0));
        let bars = (0..24)
            .map(|index| ohlcv_bar(index * SECOND_NS * 60, 100 + index as i64))
            .collect::<Vec<_>>();
        let foundation = BookmapChartFoundation::new(
            rect,
            0,
            30 * 60 * SECOND_NS,
            price(90).fixed(),
            price(130).fixed(),
            &bars,
            BookmapPriceActionSource::Ohlcv,
        )
        .expect("bookmap chart foundation");

        assert_eq!(
            bookmap_price_action_mode(&foundation),
            BookmapPriceActionMode::CompactPath
        );
        assert_eq!(bookmap_price_action_body_width(&foundation), 6.0);

        let points = bookmap_close_price_points(&foundation);
        assert_eq!(points.len(), bars.len());
        assert!(points.windows(2).all(|pair| pair[0].0.x < pair[1].0.x));
    }

    #[test]
    fn sparse_bookmap_price_action_samples_held_path_into_dense_terminal_ticks() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1_200.0, 600.0));
        let bars = vec![
            ohlcv_bar(0, 100),
            ohlcv_bar(10 * 60 * SECOND_NS, 104),
            ohlcv_bar(20 * 60 * SECOND_NS, 101),
        ];
        let foundation = BookmapChartFoundation::new(
            rect,
            0,
            30 * 60 * SECOND_NS,
            price(90).fixed(),
            price(110).fixed(),
            &bars,
            BookmapPriceActionSource::Ohlcv,
        )
        .expect("bookmap chart foundation");

        let ticks = bookmap_sampled_close_price_ticks(&foundation);

        assert!(ticks.len() > bars.len() * 4);
        assert!(ticks.windows(2).all(|pair| pair[0].0.x <= pair[1].0.x));
    }

    #[test]
    fn sparse_bookmap_price_action_expands_to_dense_micro_candles() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1_200.0, 600.0));
        let mut down_bar = ohlcv_bar(20 * 60 * SECOND_NS, 101);
        down_bar.open_fixed = price(106).fixed();
        down_bar.high_fixed = price(107).fixed();
        down_bar.low_fixed = price(100).fixed();
        down_bar.close_fixed = price(101).fixed();
        down_bar.delta = -5;
        let bars = vec![
            ohlcv_bar(0, 100),
            ohlcv_bar(10 * 60 * SECOND_NS, 104),
            down_bar,
        ];
        let foundation = BookmapChartFoundation::new(
            rect,
            0,
            30 * 60 * SECOND_NS,
            price(90).fixed(),
            price(110).fixed(),
            &bars,
            BookmapPriceActionSource::Ohlcv,
        )
        .expect("bookmap chart foundation");

        let micro_bars = bookmap_micro_price_action_bars(&foundation);

        assert!(micro_bars.len() > bars.len() * 8);
        assert!(micro_bars.windows(2).all(|pair| pair[0].x <= pair[1].x));
        assert!(micro_bars.iter().all(|bar| bar.high_y <= bar.low_y));
        assert!(micro_bars.iter().any(|bar| bar.up));
        assert!(micro_bars.iter().any(|bar| !bar.up));
        assert!(
            (micro_bars.first().expect("first micro bar").open_y
                - foundation.transform.price_to_y(foundation.bars[0].open))
            .abs()
                < 0.001
        );
    }

    #[test]
    fn bookmap_chart_foundation_maps_timestamps_through_chart_transform() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(600.0, 300.0));
        let bars = vec![
            ohlcv_bar(60 * SECOND_NS, 100),
            ohlcv_bar(120 * SECOND_NS, 101),
            ohlcv_bar(180 * SECOND_NS, 102),
        ];
        let foundation = BookmapChartFoundation::new(
            rect,
            0,
            240 * SECOND_NS,
            price(95).fixed(),
            price(110).fixed(),
            &bars,
            BookmapPriceActionSource::Ohlcv,
        )
        .expect("bookmap chart foundation");

        assert_eq!(
            bookmap_timestamp_logical(120 * SECOND_NS, &bars, 60 * SECOND_NS),
            1.0
        );
        assert!(
            (foundation.transform.logical_to_x(bookmap_timestamp_logical(
                120 * SECOND_NS,
                &bars,
                60 * SECOND_NS,
            )) - foundation.transform.logical_to_x(1.0))
            .abs()
                < 0.001
        );
        assert_eq!(foundation.bars[1].close, 101.0);
    }

    #[test]
    fn dense_bookmap_ohlcv_keeps_candle_mode_with_narrow_bodies() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(600.0, 240.0));
        let bars = (0..120)
            .map(|index| ohlcv_bar(index * 100, 100 + (index % 3) as i64))
            .collect::<Vec<_>>();
        let foundation = BookmapChartFoundation::new(
            rect,
            0,
            20_000,
            price(95).fixed(),
            price(110).fixed(),
            &bars,
            BookmapPriceActionSource::Ohlcv,
        )
        .expect("bookmap chart foundation");

        assert_eq!(
            bookmap_price_action_mode(&foundation),
            BookmapPriceActionMode::DenseCandles
        );
        assert!(bookmap_price_action_body_width(&foundation) <= 10.0);
    }

    #[test]
    fn bookmap_price_action_prefers_dense_trade_points_over_sparse_ohlcv() {
        let ohlcv = vec![ohlcv_bar(0, 100), ohlcv_bar(60 * SECOND_NS, 101)];
        let bubbles = (0..36)
            .map(|index| {
                (
                    VolumeBubbleKey {
                        interval_start_ns: index * SECOND_NS,
                        instrument_id: 1,
                        price: price(100 + (index % 4) as i64),
                    },
                    VolumeBubble {
                        buy_volume: if index % 2 == 0 { 8 } else { 2 },
                        sell_volume: if index % 2 == 0 { 2 } else { 8 },
                        unknown_volume: 0,
                        trade_count: 1,
                        fill_count: 1,
                    },
                )
            })
            .collect::<Vec<_>>();

        let (records, source) = bookmap_price_action_records(&ohlcv, &[], &bubbles);

        assert_eq!(source, BookmapPriceActionSource::TradePrints);
        assert!(records.len() > ohlcv.len());
        assert!(
            records
                .windows(2)
                .all(|pair| { pair[0].interval_start_ns < pair[1].interval_start_ns })
        );
    }

    #[test]
    fn bookmap_price_action_uses_bbo_midpoint_when_trade_points_are_not_dense() {
        let ohlcv = vec![ohlcv_bar(0, 100), ohlcv_bar(60 * SECOND_NS, 101)];
        let bbo = (0..36)
            .map(|index| BboTransition {
                ts_event: index * SECOND_NS,
                instrument_id: 1,
                bid: Some(PriceLevel {
                    price: price(100 + (index % 3) as i64),
                    size: 10,
                }),
                ask: Some(PriceLevel {
                    price: price(101 + (index % 3) as i64),
                    size: 12,
                }),
                spread: Some(1.0),
                mid: None,
            })
            .collect::<Vec<_>>();

        let (records, source) = bookmap_price_action_records(&ohlcv, &bbo, &[]);

        assert_eq!(source, BookmapPriceActionSource::BboMidpoint);
        assert_eq!(records.len(), bookmap_bbo_price_action_records(&bbo).len());
        assert_eq!(
            records[0].close_fixed,
            price(100).fixed() + FIXED_PRICE_SCALE / 2
        );
    }

    #[test]
    fn bookmap_price_action_uses_bbo_when_ohlcv_is_sparse_and_bbo_has_more_context() {
        let ohlcv = (0..10)
            .map(|index| ohlcv_bar(index * 60 * SECOND_NS, 100 + index as i64))
            .collect::<Vec<_>>();
        let bbo = (0..14)
            .map(|index| BboTransition {
                ts_event: index * 10 * SECOND_NS,
                instrument_id: 1,
                bid: Some(PriceLevel {
                    price: price(100 + (index % 2) as i64),
                    size: 10,
                }),
                ask: Some(PriceLevel {
                    price: price(101 + (index % 2) as i64),
                    size: 12,
                }),
                spread: Some(1.0),
                mid: None,
            })
            .collect::<Vec<_>>();

        let (records, source) = bookmap_price_action_records(&ohlcv, &bbo, &[]);

        assert_eq!(source, BookmapPriceActionSource::BboMidpoint);
        assert_eq!(records.len(), 14);
    }

    #[test]
    fn sparse_bookmap_trade_points_do_not_replace_ohlcv_source() {
        let ohlcv = vec![ohlcv_bar(0, 100), ohlcv_bar(60 * SECOND_NS, 101)];
        let bubbles = vec![(
            VolumeBubbleKey {
                interval_start_ns: 10,
                instrument_id: 1,
                price: price(100),
            },
            VolumeBubble {
                buy_volume: 8,
                sell_volume: 0,
                unknown_volume: 0,
                trade_count: 1,
                fill_count: 1,
            },
        )];

        let (records, source) = bookmap_price_action_records(&ohlcv, &[], &bubbles);

        assert_eq!(source, BookmapPriceActionSource::Ohlcv);
        assert_eq!(records, ohlcv);
        assert_eq!(bookmap_trade_price_action_records(&bubbles).len(), 1);
    }

    #[test]
    fn volume_bubble_split_uses_top_buy_bottom_sell_hemispheres() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 10.0), egui::vec2(20.0, 40.0));

        let balanced = volume_bubble_buy_split_y(rect, 0.5);
        let buy_heavy = volume_bubble_buy_split_y(rect, 0.75);
        let sell_heavy = volume_bubble_buy_split_y(rect, 0.25);

        assert!((balanced - 30.0).abs() < f32::EPSILON);
        assert!(
            buy_heavy > balanced,
            "more buy volume should grow the top half"
        );
        assert!(
            sell_heavy < balanced,
            "more sell volume should grow the bottom half"
        );
    }

    #[test]
    fn relative_time_labels_choose_readable_units() {
        assert_eq!(format_relative_ns(42), "42ns");
        assert_eq!(format_relative_ns(42_000), "42.0us");
        assert_eq!(format_relative_ns(42_000_000), "42.0ms");
        assert_eq!(format_relative_ns(42_000_000_000), "42.0s");
    }

    #[test]
    fn nanosecond_axis_label_matches_pdf_style_precision() {
        assert_eq!(
            format_nanosecond_axis_label(19_668_070_000),
            "19.668,070000"
        );
        assert_eq!(
            format_nanosecond_axis_label(79_668_070_250),
            "19.668,070250"
        );
    }

    #[test]
    fn nanosecond_zoom_window_filters_heatmap_cells_to_micro_view() {
        let audit = ReplayAuditSnapshot {
            min_ts: 0,
            max_ts: 300,
            cursor_ts: 125,
            window_start_ts: 100,
            window_end_ts: 200,
            checkpoint_event_index: None,
            checkpoint_ts: None,
            visible_cells: 0,
            total_cells: 0,
            visible_bubbles: 0,
            total_bubbles: 0,
            visible_bbo: 0,
            total_bbo: 0,
            dom_levels: 0,
            cursor_fraction: 0.25,
        };
        let (min_ts, max_ts) = nanosecond_zoom_window(&audit);
        let cells = vec![
            liquidity_cell_at(99, 100, 10),
            liquidity_cell_at(100, 100, 10),
            liquidity_cell_at(150, 100, 10),
            liquidity_cell_at(176, 100, 10),
        ];

        let visible = filter_liquidity_cells_to_time_window(cells, min_ts, max_ts);

        assert_eq!((min_ts, max_ts), (100, 175));
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].0.interval_start_ns, 100);
        assert_eq!(visible[1].0.interval_start_ns, 150);
    }

    #[test]
    fn timestamp_to_x_mapping_preserves_order_on_tiny_windows() {
        let rect = egui::Rect::from_min_size(egui::pos2(50.0, 0.0), egui::vec2(1_000.0, 200.0));
        let min_ts = 1_735_000_000_000_000_001;
        let max_ts = min_ts + 10;

        let left = x_for_timestamp(rect, min_ts, max_ts, min_ts);
        let one_ns = x_for_timestamp(rect, min_ts, max_ts, min_ts + 1);
        let two_ns = x_for_timestamp(rect, min_ts, max_ts, min_ts + 2);
        let right = x_for_timestamp(rect, min_ts, max_ts, max_ts);

        assert_eq!(left, rect.left());
        assert!(one_ns > left);
        assert!(two_ns > one_ns);
        assert_eq!(right, rect.right());
    }

    #[test]
    fn replay_visual_labels_follow_control_indices() {
        assert_eq!(replay_speed_visual_label(0), "x0.25");
        assert_eq!(replay_speed_visual_label(3), "x4");
        assert_eq!(replay_speed_visual_label(99), "x1");
        assert_eq!(replay_window_visual_label(0), "None");
        assert_eq!(replay_window_visual_label(4), "1 Minute");
        assert_eq!(replay_window_visual_label(7), "30 Minutes");
        assert_eq!(replay_window_visual_label(99), "None");
    }

    #[test]
    fn replay_market_context_can_use_bbo_when_heatmap_is_sparse() {
        let bbo = vec![BboTransition {
            ts_event: 10,
            instrument_id: 1,
            bid: Some(PriceLevel {
                price: price(100),
                size: 10,
            }),
            ask: Some(PriceLevel {
                price: price(102),
                size: 12,
            }),
            spread: Some(2.0),
            mid: Some(101.0),
        }];

        let range = replay_market_context_price_range(&[], &[], &bbo).expect("bbo range");

        assert!(range.0 < price(100).fixed());
        assert!(range.1 > price(102).fixed());
    }

    #[test]
    fn market_pulse_bar_color_switches_at_neutral_threshold() {
        let weak = market_pulse_bar_color(0.49);
        let strong = market_pulse_bar_color(0.5);

        assert!(weak.r() > weak.g(), "below neutral should be red");
        assert!(strong.g() > strong.r(), "neutral and above should be green");
    }

    #[test]
    fn market_pulse_rows_pair_side_pressure_and_unavailable_sources() {
        let rows = market_pulse_widget_rows(&[PulseMetric {
            label: "Volume pressure",
            value: 0.75,
            detail: "buy 75 / sell 25".to_string(),
        }]);

        assert_eq!(rows[0].label, "Agg buys");
        assert_eq!(rows[0].value_label, "0.75");
        assert_eq!(rows[0].bias, PulseRowBias::Buy);
        assert_eq!(rows[1].label, "Agg sells");
        assert_eq!(rows[1].value_label, "0.25");
        assert_eq!(rows[1].bias, PulseRowBias::Sell);
        assert_eq!(rows[2].label, "External algos");
        assert_eq!(rows[2].value_label, "n/a");
        assert_eq!(rows[2].value, None);
        assert_eq!(rows[2].bias, PulseRowBias::Unavailable);
    }

    #[test]
    fn provider_capability_registry_marks_databento_and_legacy_statuses() {
        let registry = provider_capability_registry(true);
        let historical = registry
            .iter()
            .find(|provider| provider.label == "Databento Historical MBO")
            .expect("historical provider");
        let derived = registry
            .iter()
            .find(|provider| provider.label == "Derived Cache")
            .expect("derived provider");
        let live = registry
            .iter()
            .find(|provider| provider.label == "Databento Live MBO")
            .expect("live provider");
        let legacy = registry
            .iter()
            .find(|provider| provider.label == "Legacy Bar Feeds")
            .expect("legacy provider");
        let broker = registry
            .iter()
            .find(|provider| provider.label == "Broker / Own Orders")
            .expect("broker provider");

        assert_eq!(historical.status, ProviderStatus::Ready);
        assert!(historical.capabilities.contains(&"MBO"));
        assert!(historical.capabilities.contains(&"REPLAY"));
        assert_eq!(derived.status, ProviderStatus::Ready);
        assert_eq!(live.status, ProviderStatus::Planned);
        assert_eq!(legacy.status, ProviderStatus::Limited);
        assert_eq!(broker.status, ProviderStatus::Unavailable);
    }

    #[test]
    fn provider_capability_registry_does_not_embed_secret_values() {
        for provider in provider_capability_registry(true) {
            let mut rendered = format!("{} {}", provider.label, provider.detail);
            rendered.push_str(&provider.capabilities.join(" "));
            let rendered = rendered.to_ascii_lowercase();

            assert!(!rendered.contains("databento_api_key"));
            assert!(!rendered.contains("api_key"));
            assert!(!rendered.contains("secret"));
            assert!(!rendered.contains('='));
        }
    }

    #[test]
    fn market_data_connection_nodes_stay_inside_hub_viewport() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 360.0));
        let hub = market_data_hub_rect(rect);
        let nodes = market_data_connection_node_rects(rect, 5);

        assert_eq!(nodes.len(), 5);
        assert!(rect.contains_rect(hub));
        assert!(nodes.iter().all(|node| rect.contains_rect(*node)));
        assert!(nodes.iter().all(|node| !node.intersects(hub)));
    }

    #[test]
    fn stops_icebergs_labels_distinguish_stop_and_iceberg_events() {
        let stop = HeuristicOrderFlowEvent {
            kind: "Stop-run-like",
            note: "large one-sided volume burst",
            interval_start_ns: 1,
            price: price(100),
            score: 0.91,
        };
        let iceberg = HeuristicOrderFlowEvent {
            kind: "Iceberg-like",
            note: "large volume into persistent displayed liquidity",
            interval_start_ns: 1,
            price: price(100),
            score: 0.30,
        };

        assert_eq!(stops_icebergs_event_label(&stop), "91");
        assert_eq!(stops_icebergs_event_label(&iceberg), "E:30");
    }

    #[test]
    fn stops_icebergs_badges_use_distinct_stop_and_iceberg_shapes() {
        let stop = HeuristicOrderFlowEvent {
            kind: "Stop-run-like",
            note: "large one-sided volume burst",
            interval_start_ns: 1,
            price: price(100),
            score: 0.91,
        };
        let iceberg = HeuristicOrderFlowEvent {
            kind: "Iceberg-like",
            note: "large volume into persistent displayed liquidity",
            interval_start_ns: 1,
            price: price(100),
            score: 0.30,
        };

        let stop_style = stops_icebergs_badge_style(&stop);
        let iceberg_style = stops_icebergs_badge_style(&iceberg);

        assert_eq!(stop_style.glyph, "S");
        assert_eq!(stop_style.shape, StopsIcebergsBadgeShape::Circle);
        assert_eq!(iceberg_style.glyph, "E");
        assert_eq!(iceberg_style.shape, StopsIcebergsBadgeShape::Square);
        assert_ne!(stop_style.color, iceberg_style.color);
    }

    #[test]
    fn bbo_trade_marker_radius_uses_volume_area_scale() {
        let small = bbo_trade_marker_radius(25, 100);
        let large = bbo_trade_marker_radius(100, 100);

        assert!(small > 1.8);
        assert!(large > small);
        assert!((large - 6.0).abs() < 0.001);
    }

    #[test]
    fn bbo_trade_markers_only_include_bubbles_near_spread() {
        let bbo = vec![BboTransition {
            ts_event: 10,
            instrument_id: 1,
            bid: Some(PriceLevel {
                price: price(100),
                size: 10,
            }),
            ask: Some(PriceLevel {
                price: price(101),
                size: 12,
            }),
            spread: Some(1.0),
            mid: Some(100.5),
        }];
        let near_bid = VolumeBubbleKey {
            interval_start_ns: 12,
            instrument_id: 1,
            price: price(100),
        };
        let near_ask = VolumeBubbleKey {
            interval_start_ns: 12,
            instrument_id: 1,
            price: price(101),
        };
        let away = VolumeBubbleKey {
            interval_start_ns: 12,
            instrument_id: 1,
            price: price(104),
        };

        assert!(bubble_is_near_bbo_spread(
            &near_bid,
            &bbo,
            FIXED_PRICE_SCALE / 2
        ));
        assert!(bubble_is_near_bbo_spread(
            &near_ask,
            &bbo,
            FIXED_PRICE_SCALE / 2
        ));
        assert!(!bubble_is_near_bbo_spread(
            &away,
            &bbo,
            FIXED_PRICE_SCALE / 2
        ));
    }

    #[test]
    fn bookmap_visible_volume_bubbles_filter_to_bbo_neighborhood() {
        let bbo = vec![BboTransition {
            ts_event: 10,
            instrument_id: 1,
            bid: Some(PriceLevel {
                price: price(100),
                size: 10,
            }),
            ask: Some(PriceLevel {
                price: price(101),
                size: 12,
            }),
            spread: Some(1.0),
            mid: Some(100.5),
        }];
        let near_bid = VolumeBubbleKey {
            interval_start_ns: 12,
            instrument_id: 1,
            price: price(100),
        };
        let away = VolumeBubbleKey {
            interval_start_ns: 12,
            instrument_id: 1,
            price: price(104),
        };
        let bubble = VolumeBubble {
            buy_volume: 10,
            sell_volume: 0,
            unknown_volume: 0,
            trade_count: 1,
            fill_count: 1,
        };

        let visible = bookmap_visible_volume_bubbles(&[(near_bid, bubble), (away, bubble)], &bbo);

        assert_eq!(visible, vec![(near_bid, bubble)]);
    }

    #[test]
    fn bookmap_price_range_keeps_current_anchor_inside_comfort_band() {
        let span = 1_000;

        let (low_min, low_max) = bookmap_anchor_price_comfort_range(1_000, 2_000, 1_050);
        let low_percent = (1_050 - low_min) * 100 / (low_max - low_min);

        assert_eq!(low_max - low_min, span);
        assert_eq!(low_percent, 34);

        let (high_min, high_max) = bookmap_anchor_price_comfort_range(1_000, 2_000, 1_950);
        let high_percent = (1_950 - high_min) * 100 / (high_max - high_min);

        assert_eq!(high_max - high_min, span);
        assert_eq!(high_percent, 66);

        assert_eq!(
            bookmap_anchor_price_comfort_range(1_000, 2_000, 1_500),
            (1_000, 2_000)
        );
    }

    #[test]
    fn bookmap_terminal_price_range_ignores_historical_heatmap_and_trade_outliers() {
        let p = |whole: i64, quarters: i64| {
            MboPrice::from_fixed(whole * FIXED_PRICE_SCALE + quarters * FIXED_PRICE_SCALE / 4)
        };
        let bbo = vec![BboTransition {
            ts_event: 50,
            instrument_id: 1,
            bid: Some(PriceLevel {
                price: p(100, 1),
                size: 12,
            }),
            ask: Some(PriceLevel {
                price: p(100, 2),
                size: 14,
            }),
            spread: Some(0.25),
            mid: Some(100.375),
        }];
        let bubble = VolumeBubble {
            buy_volume: 12,
            sell_volume: 3,
            unknown_volume: 0,
            trade_count: 1,
            fill_count: 1,
        };
        let frame = BookmapFrame::from_layers(BookmapFrameInput {
            source: BookmapFrameSource {
                source_label: "GLBX.MDP3 / NQ.FUT / mbo".to_string(),
                range_label: "test".to_string(),
                instrument_selection: None,
                derived: true,
                event_count: 0,
                instrument_count: 1,
                warning: None,
            },
            instrument_id: Some(1),
            start_ts: 0,
            end_ts: 60,
            cursor_ts: 50,
            checkpoint: None,
            book_quality: Some(BookQuality::Clean),
            heatmap_cells: vec![
                (
                    LiquidityCellKey {
                        interval_start_ns: 10,
                        instrument_id: 1,
                        price: p(80, 0),
                        side: MboSide::Bid,
                    },
                    LiquidityCell {
                        displayed_size: 400,
                        max_displayed_size: 500,
                        samples: 4,
                        intensity: 0.8,
                    },
                ),
                (
                    LiquidityCellKey {
                        interval_start_ns: 20,
                        instrument_id: 1,
                        price: p(200, 0),
                        side: MboSide::Ask,
                    },
                    LiquidityCell {
                        displayed_size: 500,
                        max_displayed_size: 500,
                        samples: 4,
                        intensity: 1.0,
                    },
                ),
            ],
            volume_bubbles: vec![
                (
                    VolumeBubbleKey {
                        interval_start_ns: 45,
                        instrument_id: 1,
                        price: p(100, 2),
                    },
                    bubble,
                ),
                (
                    VolumeBubbleKey {
                        interval_start_ns: 46,
                        instrument_id: 1,
                        price: p(180, 0),
                    },
                    bubble,
                ),
            ],
            bbo_transitions: bbo,
            dom_snapshot: Some(DomColumnSnapshot {
                instrument_id: 1,
                quality: BookQuality::Clean,
                best_bid: Some(PriceLevel {
                    price: p(100, 1),
                    size: 12,
                }),
                best_ask: Some(PriceLevel {
                    price: p(100, 2),
                    size: 14,
                }),
                max_size: 20,
                levels: vec![
                    DomColumnLevel {
                        price: p(100, 0),
                        bid_size: 8,
                        ask_size: 0,
                        total_size: 8,
                        side_hint: MboSide::Bid,
                    },
                    DomColumnLevel {
                        price: p(100, 1),
                        bid_size: 12,
                        ask_size: 1,
                        total_size: 13,
                        side_hint: MboSide::Bid,
                    },
                    DomColumnLevel {
                        price: p(100, 2),
                        bid_size: 1,
                        ask_size: 14,
                        total_size: 15,
                        side_hint: MboSide::Ask,
                    },
                    DomColumnLevel {
                        price: p(100, 3),
                        bid_size: 0,
                        ask_size: 7,
                        total_size: 7,
                        side_hint: MboSide::Ask,
                    },
                ],
            }),
            footprint_cells: Vec::new(),
            footprint_poc: None,
            flow_summary: FlowSummary::default(),
            add_on_events: Vec::new(),
            unsupported_add_ons: Vec::new(),
        });
        let visible_bubbles = bookmap_visible_volume_bubbles(
            &frame.layers.volume_bubbles,
            &frame.layers.bbo_transitions,
        );

        let (min_price, max_price) =
            bookmap_terminal_visible_price_range(&frame, &[], &visible_bubbles)
                .expect("terminal price range");

        assert_eq!(visible_bubbles.len(), 1);
        assert!(
            min_price > p(90, 0).fixed(),
            "range should drop low heatmap outlier"
        );
        assert!(
            max_price < p(112, 0).fixed(),
            "range should drop high heatmap/trade outliers"
        );
    }

    #[test]
    fn bookmap_aligned_dom_snapshot_replaces_stale_dom_with_bbo_ladder() {
        let bbo = vec![BboTransition {
            ts_event: 100,
            instrument_id: 7,
            bid: Some(PriceLevel {
                price: price(100),
                size: 80,
            }),
            ask: Some(PriceLevel {
                price: MboPrice::from_fixed(100 * FIXED_PRICE_SCALE + FIXED_PRICE_SCALE / 4),
                size: 64,
            }),
            spread: Some(0.25),
            mid: Some(100.125),
        }];
        let stale_dom = DomColumnSnapshot {
            instrument_id: 7,
            quality: BookQuality::Clean,
            best_bid: Some(PriceLevel {
                price: price(90),
                size: 12,
            }),
            best_ask: Some(PriceLevel {
                price: price(91),
                size: 14,
            }),
            max_size: 14,
            levels: vec![DomColumnLevel {
                price: price(90),
                bid_size: 12,
                ask_size: 0,
                total_size: 12,
                side_hint: MboSide::Bid,
            }],
        };

        let aligned =
            bookmap_aligned_dom_snapshot(Some(stale_dom), &bbo, None).expect("aligned DOM");

        assert_eq!(aligned.quality, BookQuality::Suspect);
        assert_eq!(aligned.best_bid.expect("bid").price, price(100));
        assert!(aligned.levels.len() >= BOOKMAP_FALLBACK_DOM_DEPTH_ROWS * 2);
        assert!(
            aligned
                .levels
                .iter()
                .any(|level| level.price.fixed() < price(96).fixed())
        );
    }

    #[test]
    fn bookmap_aligned_dom_snapshot_replaces_near_but_off_market_dom() {
        let bbo = vec![BboTransition {
            ts_event: 100,
            instrument_id: 7,
            bid: Some(PriceLevel {
                price: price(100),
                size: 80,
            }),
            ask: Some(PriceLevel {
                price: MboPrice::from_fixed(100 * FIXED_PRICE_SCALE + FIXED_PRICE_SCALE / 4),
                size: 64,
            }),
            spread: Some(0.25),
            mid: Some(100.125),
        }];
        let off_market_dom = DomColumnSnapshot {
            instrument_id: 7,
            quality: BookQuality::Clean,
            best_bid: Some(PriceLevel {
                price: price(102),
                size: 12,
            }),
            best_ask: Some(PriceLevel {
                price: MboPrice::from_fixed(102 * FIXED_PRICE_SCALE + FIXED_PRICE_SCALE / 4),
                size: 14,
            }),
            max_size: 14,
            levels: vec![DomColumnLevel {
                price: price(102),
                bid_size: 12,
                ask_size: 0,
                total_size: 12,
                side_hint: MboSide::Bid,
            }],
        };

        let aligned =
            bookmap_aligned_dom_snapshot(Some(off_market_dom), &bbo, None).expect("aligned DOM");

        assert_eq!(aligned.quality, BookQuality::Suspect);
        assert_eq!(aligned.best_bid.expect("bid").price, price(100));
    }

    #[test]
    fn bookmap_aligned_dom_snapshot_anchors_fallback_to_active_cursor_price() {
        let bbo = vec![BboTransition {
            ts_event: 100,
            instrument_id: 7,
            bid: Some(PriceLevel {
                price: price(90),
                size: 80,
            }),
            ask: Some(PriceLevel {
                price: MboPrice::from_fixed(90 * FIXED_PRICE_SCALE + FIXED_PRICE_SCALE / 4),
                size: 64,
            }),
            spread: Some(0.25),
            mid: Some(90.125),
        }];

        let aligned = bookmap_aligned_dom_snapshot(None, &bbo, Some(price(100)))
            .expect("active anchored DOM");

        assert_eq!(aligned.quality, BookQuality::Suspect);
        assert_eq!(aligned.best_bid.expect("bid").price, price(100));
        assert!(
            aligned
                .levels
                .iter()
                .any(|level| level.price.fixed() >= price(100).fixed())
        );
    }

    #[test]
    fn composite_last_price_prefers_latest_trade_then_bbo_midpoint() {
        let older_key = VolumeBubbleKey {
            interval_start_ns: 10,
            instrument_id: 1,
            price: price(100),
        };
        let newer_key = VolumeBubbleKey {
            interval_start_ns: 20,
            instrument_id: 1,
            price: price(105),
        };
        let bubble = VolumeBubble {
            buy_volume: 1,
            sell_volume: 0,
            unknown_volume: 0,
            trade_count: 1,
            fill_count: 1,
        };
        let bbo = vec![BboTransition {
            ts_event: 30,
            instrument_id: 1,
            bid: Some(PriceLevel {
                price: price(101),
                size: 10,
            }),
            ask: Some(PriceLevel {
                price: price(103),
                size: 10,
            }),
            spread: Some(2.0),
            mid: Some(102.0),
        }];

        let from_trade =
            composite_last_price_marker(&[(older_key, bubble), (newer_key, bubble)], &bbo, None)
                .expect("trade marker");
        let from_bbo = composite_last_price_marker(&[], &bbo, None).expect("bbo marker");

        assert_eq!(from_trade.fixed(), price(105).fixed());
        assert_eq!(from_bbo.fixed(), price(102).fixed());
    }

    #[test]
    fn bookmap_active_price_marker_prefers_cursor_price_over_stale_trade() {
        let trade_key = VolumeBubbleKey {
            interval_start_ns: 10,
            instrument_id: 1,
            price: price(90),
        };
        let bubble = VolumeBubble {
            buy_volume: 1,
            sell_volume: 0,
            unknown_volume: 0,
            trade_count: 1,
            fill_count: 1,
        };
        let bbo = vec![BboTransition {
            ts_event: 60 * SECOND_NS,
            instrument_id: 1,
            bid: Some(PriceLevel {
                price: price(110),
                size: 10,
            }),
            ask: Some(PriceLevel {
                price: price(112),
                size: 10,
            }),
            spread: Some(2.0),
            mid: Some(111.0),
        }];
        let bars = vec![ohlcv_bar(60 * SECOND_NS, 111)];

        let marker =
            bookmap_active_price_marker(60 * SECOND_NS, &bars, &bbo, &[(trade_key, bubble)], None)
                .expect("active marker");

        assert_eq!(marker.fixed(), price(111).fixed());
    }

    #[test]
    fn dom_profile_fill_rect_extends_from_price_center_by_side() {
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(100.0, 18.0));
        let bid = dom_profile_fill_rect(rect, 50, 100, MboSide::Bid);
        let ask = dom_profile_fill_rect(rect, 50, 100, MboSide::Ask);

        assert_eq!(bid.left(), rect.center().x);
        assert!(bid.right() > rect.center().x);
        assert_eq!(ask.right(), rect.center().x);
        assert!(ask.left() < rect.center().x);
    }

    fn source_info() -> MboSourceInfo {
        MboSourceInfo::new("GLBX.MDP3", "NQ.FUT", "parent", "instrument_id")
    }

    fn event(
        ts_event: u64,
        order_id: u64,
        action: MboAction,
        side: MboSide,
        price_value: i64,
        size: u32,
    ) -> MboEvent {
        let mut event = MboEvent::book_event(
            source_info(),
            1,
            order_id,
            action,
            side,
            price(price_value),
            size,
        );
        event.ts_event = ts_event;
        event
    }

    fn snapshot() -> OrderFlowPreviewSnapshot {
        OrderFlowPreviewSnapshot {
            source_label: "GLBX.MDP3 / NQ.FUT / mbo".to_string(),
            range_label: "--".to_string(),
            instrument_selection: BookmapInstrumentSelection::from_cache_entry(
                &entry("NQ.FUT", "", None),
                "--",
            )
            .with_timeline_instrument(Some(1)),
            raw_path: PathBuf::new(),
            derived_source: None,
            report: OrderFlowTimelineRunReport::default(),
            warning: None,
            event_count: 0,
            last_event_ts: None,
            checkpoint_count: 0,
            replay_checkpoints: Vec::new(),
            instrument_count: 1,
            book_snapshot: None,
            bid_levels: Vec::new(),
            ask_levels: Vec::new(),
            flow_summary: FlowSummary::default(),
            heatmap_frame_count: 0,
            depth_frames: Vec::new(),
            latest_depth_levels: Vec::new(),
            liquidity_cell_count: 0,
            visible_liquidity_cells: Vec::new(),
            ohlcv_bars: Vec::new(),
            volume_bubbles: Vec::new(),
            bbo_transitions: Vec::new(),
            dom_snapshot: None,
            footprint_interval_ns: 1,
            footprint_cell_count: 0,
            footprint_poc: None,
            top_footprint_cells: Vec::new(),
            add_on_events: Vec::new(),
        }
    }

    fn liquidity_cell(price_value: i64, size: u64) -> (LiquidityCellKey, LiquidityCell) {
        liquidity_cell_at(10, price_value, size)
    }

    fn liquidity_cell_at(
        interval_start_ns: u64,
        price_value: i64,
        size: u64,
    ) -> (LiquidityCellKey, LiquidityCell) {
        (
            LiquidityCellKey {
                interval_start_ns,
                instrument_id: 1,
                price: price(price_value),
                side: MboSide::Bid,
            },
            LiquidityCell {
                displayed_size: size,
                max_displayed_size: size,
                samples: 1,
                intensity: 1.0,
            },
        )
    }

    fn ohlcv_bar(interval_start_ns: u64, close_value: i64) -> OhlcvBarRecord {
        OhlcvBarRecord {
            interval_ns: 10,
            interval_start_ns,
            instrument_id: 1,
            open_fixed: close_value * FIXED_PRICE_SCALE - FIXED_PRICE_SCALE,
            high_fixed: close_value * FIXED_PRICE_SCALE + FIXED_PRICE_SCALE,
            low_fixed: close_value * FIXED_PRICE_SCALE - 2 * FIXED_PRICE_SCALE,
            close_fixed: close_value * FIXED_PRICE_SCALE,
            volume: 10,
            buy_volume: 6,
            sell_volume: 4,
            unknown_volume: 0,
            trade_count: 1,
            fill_count: 1,
            delta: 2,
        }
    }

    #[test]
    fn preview_entry_prefers_existing_nq_payload() {
        let missing_nq = entry("NQ.FUT", "missing-nq.dbn.zst", Some(1));
        let existing_repo_file = entry("ES.FUT", "Cargo.toml", Some(1));
        let existing_nq_repo_file = entry("NQ.FUT", "Cargo.lock", Some(10));
        let entries = vec![missing_nq, existing_repo_file, existing_nq_repo_file];

        let selected = choose_preview_entry(&entries).expect("preview entry");

        assert_eq!(selected.symbol, "NQ.FUT");
        assert_eq!(selected.raw_path, PathBuf::from("Cargo.lock"));
    }

    #[test]
    fn default_derived_roots_prefer_completed_compressed_cache() {
        assert_eq!(
            default_derived_cache_roots(),
            vec![
                PathBuf::from(PREFERRED_DERIVED_CACHE_ROOT),
                PathBuf::from(DEFAULT_DERIVED_CACHE_ROOT),
            ]
        );
    }

    #[test]
    fn initial_derived_viewport_uses_latest_bounded_window() {
        let mut manifest = DerivedCacheManifest::from_entry(
            &entry("NQ.FUT", "Cargo.toml", Some(1)),
            DerivedCacheBuildSummary {
                events_read: 100,
                applied_book_events: 80,
                trade_prints: 20,
                first_event_ts: Some(1_000),
                last_event_ts: Some(1_000 + DERIVED_INITIAL_VIEWPORT_NS + 25),
                instrument_count: 1,
                checkpoint_count: 1,
                ohlcv_bars: 1,
                footprint_cells: 1,
                heatmap_cells: 1,
                volume_bubbles: 1,
                bbo_transitions: 1,
                add_on_events: 0,
            },
        );

        assert_eq!(
            initial_derived_viewport_window(&manifest),
            (1_025, 1_000 + DERIVED_INITIAL_VIEWPORT_NS + 25)
        );

        manifest.summary.last_event_ts = Some(1_500);
        assert_eq!(initial_derived_viewport_window(&manifest), (0, 1_500));
    }

    #[test]
    fn derived_cache_preview_loads_widget_snapshot_from_artifacts() {
        let dir = std::env::temp_dir().join(format!(
            "rustchart-widget-derived-preview-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let entry = entry("NQ.FUT", "Cargo.toml", Some(1));
        let events = vec![
            event(1, 1, MboAction::Add, MboSide::Bid, 100, 3),
            event(2, 2, MboAction::Add, MboSide::Ask, 102, 4),
            event(3, 2, MboAction::Trade, MboSide::Bid, 102, 5),
            event(4, 2, MboAction::Cancel, MboSide::Ask, 102, 4),
        ];
        let config = DerivedCacheBuildConfig {
            replay: OrderFlowReplayConfig {
                checkpoint_every_events: 2,
                large_trade_threshold: 5,
                footprint_interval_ns: 10,
                heatmap_interval_ns: 1,
                heatmap_depth_limit: 2,
            },
            batch_size: 2,
            max_events: None,
        };
        build_derived_cache_artifacts(
            &entry,
            VecMboEventSource::new(source_info(), events),
            &dir,
            config,
        )
        .expect("build derived artifacts");

        let snapshot = load_derived_preview_for_entry(&entry, &dir)
            .expect("derived preview")
            .expect("source-matched snapshot");

        assert!(snapshot.source_label.ends_with("derived"));
        assert_eq!(snapshot.event_count, 4);
        assert_eq!(snapshot.checkpoint_count, 2);
        assert_eq!(snapshot.replay_checkpoints.len(), 2);
        assert_eq!(snapshot.instrument_selection.instrument_id, Some(1));
        assert_eq!(
            snapshot.instrument_selection.available_instrument_ids,
            vec![1]
        );
        assert!(
            snapshot
                .instrument_selection
                .chrome_label()
                .contains("instrument_id 1")
        );
        assert!(snapshot.derived_source.is_some());
        assert_eq!(
            snapshot
                .derived_source
                .as_ref()
                .expect("derived source")
                .instrument_selection
                .instrument_id,
            Some(1)
        );
        assert_eq!(snapshot.volume_bubbles.len(), 1);
        assert!(!snapshot.visible_liquidity_cells.is_empty());
        assert!(!snapshot.add_on_events.is_empty());
        assert!(!snapshot.bbo_transitions.is_empty());
        assert!(snapshot.dom_snapshot.is_some());
        assert_eq!(snapshot.flow_summary.total_volume(), 5);
        assert_eq!(snapshot.footprint_poc.expect("poc").1.total_volume(), 5);

        std::fs::remove_dir_all(dir).expect("cleanup derived preview temp dir");
    }

    #[test]
    fn preview_loader_uses_supplied_derived_cache_root() {
        let dir = std::env::temp_dir().join(format!(
            "rustchart-widget-derived-root-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let manifest_dir = dir.join("manifests");
        let derived_root = dir.join("clean-derived");
        fs::create_dir_all(&manifest_dir).expect("manifest dir");

        let raw_path = "Cargo.toml";
        let entry = entry("NQ.FUT", raw_path, Some(1));
        let manifest = serde_json::json!({
            "dataset": "GLBX.MDP3",
            "schema": "mbo",
            "symbol": "NQ.FUT",
            "stype_in": "parent",
            "stype_out": "instrument_id",
            "start": "2024-11-29T00:00:00Z",
            "end_exclusive": "2024-12-03T00:00:00Z",
            "raw_path": raw_path,
            "estimated_record_count": 4,
            "bytes_on_disk": 1
        });
        fs::write(
            manifest_dir.join("nq.json"),
            serde_json::to_string_pretty(&manifest).expect("manifest json"),
        )
        .expect("write manifest");

        let events = vec![
            event(1, 1, MboAction::Add, MboSide::Bid, 100, 3),
            event(2, 2, MboAction::Add, MboSide::Ask, 102, 4),
            event(3, 2, MboAction::Trade, MboSide::Bid, 102, 5),
            event(4, 2, MboAction::Cancel, MboSide::Ask, 102, 4),
        ];
        let config = DerivedCacheBuildConfig {
            replay: OrderFlowReplayConfig {
                checkpoint_every_events: 2,
                large_trade_threshold: 5,
                footprint_interval_ns: 10,
                heatmap_interval_ns: 1,
                heatmap_depth_limit: 2,
            },
            batch_size: 2,
            max_events: None,
        };
        build_derived_cache_artifacts(
            &entry,
            VecMboEventSource::new(source_info(), events),
            &derived_root,
            config,
        )
        .expect("build derived artifacts");

        let snapshot =
            load_preview_from_paths(&manifest_dir, &derived_root).expect("preview from paths");

        assert!(snapshot.source_label.ends_with("derived"));
        assert_eq!(snapshot.event_count, 4);
        assert!(snapshot.derived_source.is_some());

        fs::remove_dir_all(dir).expect("cleanup derived root temp dir");
    }

    #[test]
    fn derived_replay_window_queries_artifacts_around_cursor() {
        let dir = std::env::temp_dir().join(format!(
            "rustchart-derived-replay-window-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let entry = entry("NQ.FUT", "Cargo.toml", Some(1));
        let minute = 60 * SECOND_NS;
        let events = vec![
            event(16 * minute, 1, MboAction::Add, MboSide::Bid, 100, 3),
            event(17 * minute, 2, MboAction::Add, MboSide::Ask, 102, 4),
            event(18 * minute, 2, MboAction::Trade, MboSide::Bid, 102, 5),
            event(19 * minute, 2, MboAction::Cancel, MboSide::Ask, 102, 4),
        ];
        let config = DerivedCacheBuildConfig {
            replay: OrderFlowReplayConfig {
                checkpoint_every_events: 2,
                large_trade_threshold: 5,
                footprint_interval_ns: 10,
                heatmap_interval_ns: 10,
                heatmap_depth_limit: 2,
            },
            batch_size: 2,
            max_events: None,
        };
        build_derived_cache_artifacts(
            &entry,
            VecMboEventSource::new(source_info(), events),
            &dir,
            config,
        )
        .expect("build derived artifacts");
        let snapshot = load_derived_preview_for_entry(&entry, &dir)
            .expect("derived preview")
            .expect("source-matched snapshot");
        let controls = ReplayControlState {
            cursor_fraction: 1.0,
            ..ReplayControlState::default()
        };
        let audit = replay_audit(&snapshot, &controls).expect("audit");
        let key = derived_replay_window_key(&audit);
        let source = snapshot.derived_source.clone().expect("derived source");

        let window = query_derived_replay_window(&source, key).expect("derived replay window");

        assert_eq!(source.instrument_selection.query_instrument_id(), Ok(1));
        assert_eq!(window.key, key);
        assert_eq!(
            window.checkpoint.map(|checkpoint| checkpoint.event_index),
            audit.checkpoint_event_index
        );
        assert_eq!(window.volume_bubbles.len(), 1);
        assert!(!window.visible_liquidity_cells.is_empty());
        assert!(!window.add_on_events.is_empty());
        assert!(!window.bbo_transitions.is_empty());
        assert!(window.dom_snapshot.is_some());
        assert_eq!(window.flow_summary.total_volume(), 5);
        assert_eq!(
            window.footprint_poc.expect("window poc").1.total_volume(),
            5
        );

        std::fs::remove_dir_all(dir).expect("cleanup derived replay temp dir");
    }

    #[test]
    fn derived_replay_window_drives_secondary_surface_helpers() {
        let mut snapshot = snapshot();
        snapshot.visible_liquidity_cells = vec![liquidity_cell(90, 1)];
        snapshot.volume_bubbles = vec![(
            VolumeBubbleKey {
                interval_start_ns: 10,
                instrument_id: 1,
                price: price(90),
            },
            VolumeBubble {
                sell_volume: 1,
                ..VolumeBubble::default()
            },
        )];
        let key = DerivedReplayWindowKey {
            start_ts: 20,
            end_ts: 40,
            cursor_ts: 30,
            checkpoint_event_index: Some(2),
        };
        let derived_cells = vec![(
            LiquidityCellKey {
                interval_start_ns: 30,
                instrument_id: 1,
                price: price(101),
                side: MboSide::Bid,
            },
            LiquidityCell {
                displayed_size: 24,
                max_displayed_size: 24,
                samples: 1,
                intensity: 1.0,
            },
        )];
        let derived_bubbles = vec![(
            VolumeBubbleKey {
                interval_start_ns: 30,
                instrument_id: 1,
                price: price(101),
            },
            VolumeBubble {
                buy_volume: 12,
                sell_volume: 1,
                trade_count: 2,
                ..VolumeBubble::default()
            },
        )];
        let derived_window = DerivedReplayWindowSnapshot {
            key,
            checkpoint: Some(ReplayCheckpointRecord {
                event_index: 2,
                timestamp_ns: 30,
                instrument_count: 1,
            }),
            visible_liquidity_cells: derived_cells.clone(),
            ohlcv_bars: Vec::new(),
            volume_bubbles: derived_bubbles.clone(),
            bbo_transitions: vec![
                BboTransition {
                    ts_event: 20,
                    instrument_id: 1,
                    bid: None,
                    ask: None,
                    spread: Some(1.0),
                    mid: Some(100.0),
                },
                BboTransition {
                    ts_event: 30,
                    instrument_id: 1,
                    bid: None,
                    ask: None,
                    spread: Some(1.0),
                    mid: Some(102.0),
                },
            ],
            dom_snapshot: Some(DomColumnSnapshot {
                instrument_id: 1,
                quality: BookQuality::Clean,
                best_bid: Some(PriceLevel {
                    price: price(100),
                    size: 10,
                }),
                best_ask: Some(PriceLevel {
                    price: price(102),
                    size: 2,
                }),
                max_size: 24,
                levels: vec![DomColumnLevel {
                    price: price(101),
                    bid_size: 24,
                    ask_size: 2,
                    total_size: 26,
                    side_hint: MboSide::Bid,
                }],
            }),
            top_footprint_cells: Vec::new(),
            footprint_poc: None,
            flow_summary: FlowSummary::default(),
            add_on_events: Vec::new(),
        };
        let audit = ReplayAuditSnapshot {
            min_ts: 10,
            max_ts: 40,
            cursor_ts: 30,
            window_start_ts: 20,
            window_end_ts: 40,
            checkpoint_event_index: Some(2),
            checkpoint_ts: Some(30),
            visible_cells: 0,
            total_cells: 0,
            visible_bubbles: 0,
            total_bubbles: 0,
            visible_bbo: 0,
            total_bbo: 0,
            dom_levels: 0,
            cursor_fraction: 0.5,
        };

        let cells = replay_liquidity_cells(&snapshot, &audit, Some(&derived_window));
        let depth_levels = replay_depth_levels(&snapshot, Some(&audit), Some(&derived_window));
        let events = heuristic_order_flow_events_from_parts(&cells, &derived_bubbles);
        let metrics = pulse_metrics_for_replay_window(&snapshot, &audit, Some(&derived_window));

        assert_eq!(cells, derived_cells);
        assert_eq!(depth_levels.len(), 2);
        assert!(depth_levels.iter().any(|level| level.size == 24));
        assert!(events.iter().any(|event| event.kind == "Stop-run-like"));
        assert!(events.iter().any(|event| event.kind == "Iceberg-like"));
        assert!(
            metrics
                .iter()
                .any(|metric| metric.label == "Book pressure" && metric.value > 0.9)
        );
        assert!(metrics.iter().any(|metric| metric.label == "Price change"));
    }

    #[test]
    fn replay_ohlcv_bars_prefers_derived_window_and_filters_to_cursor() {
        let mut snapshot = snapshot();
        snapshot.ohlcv_bars = vec![ohlcv_bar(10, 100), ohlcv_bar(20, 101), ohlcv_bar(40, 102)];
        let audit = ReplayAuditSnapshot {
            min_ts: 0,
            max_ts: 60,
            cursor_ts: 35,
            window_start_ts: 15,
            window_end_ts: 60,
            checkpoint_event_index: None,
            checkpoint_ts: None,
            visible_cells: 0,
            total_cells: 0,
            visible_bubbles: 0,
            total_bubbles: 0,
            visible_bbo: 0,
            total_bbo: 0,
            dom_levels: 0,
            cursor_fraction: 0.5,
        };

        let snapshot_bars = replay_ohlcv_bars(&snapshot, &audit, None);

        assert_eq!(
            snapshot_bars
                .iter()
                .map(|bar| bar.interval_start_ns)
                .collect::<Vec<_>>(),
            vec![20]
        );

        let derived_window = DerivedReplayWindowSnapshot {
            key: DerivedReplayWindowKey {
                start_ts: 15,
                end_ts: 60,
                cursor_ts: 35,
                checkpoint_event_index: None,
            },
            checkpoint: None,
            visible_liquidity_cells: Vec::new(),
            ohlcv_bars: vec![ohlcv_bar(16, 200), ohlcv_bar(36, 201)],
            volume_bubbles: Vec::new(),
            bbo_transitions: Vec::new(),
            dom_snapshot: None,
            top_footprint_cells: Vec::new(),
            footprint_poc: None,
            flow_summary: FlowSummary::default(),
            add_on_events: Vec::new(),
        };

        let derived_bars = replay_ohlcv_bars(&snapshot, &audit, Some(&derived_window));

        assert_eq!(derived_bars.len(), 1);
        assert_eq!(derived_bars[0].interval_start_ns, 16);
        assert_eq!(derived_bars[0].close_fixed, 200 * FIXED_PRICE_SCALE);
    }

    #[test]
    fn persisted_addon_events_drive_stops_icebergs_overlay() {
        let add_on_events = vec![
            AddOnEventRecord {
                kind: "rustchart.heuristic.stop_run_like".to_string(),
                timestamp_ns: 10,
                instrument_id: 1,
                price_fixed: Some(100 * FIXED_PRICE_SCALE),
                side: Some("buy".to_string()),
                confidence_bps: 9100,
                source: "fixture".to_string(),
                display_label: "Stop-run-like".to_string(),
                note: "fixture".to_string(),
            },
            AddOnEventRecord {
                kind: "rustchart.heuristic.iceberg_like".to_string(),
                timestamp_ns: 20,
                instrument_id: 1,
                price_fixed: Some(101 * FIXED_PRICE_SCALE),
                side: Some("sell".to_string()),
                confidence_bps: 6300,
                source: "fixture".to_string(),
                display_label: "Iceberg-like".to_string(),
                note: "fixture".to_string(),
            },
        ];
        let events = heuristic_overlay_events_from_addons(&add_on_events).expect("overlay events");

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, "Stop-run-like");
        assert_eq!(events[0].score, 0.91);
        assert!(events.iter().any(|event| event.kind == "Iceberg-like"));

        let mut snapshot = snapshot();
        snapshot.add_on_events = add_on_events.clone();
        let derived_window = DerivedReplayWindowSnapshot {
            key: DerivedReplayWindowKey {
                start_ts: 10,
                end_ts: 20,
                cursor_ts: 20,
                checkpoint_event_index: None,
            },
            checkpoint: None,
            visible_liquidity_cells: Vec::new(),
            ohlcv_bars: Vec::new(),
            volume_bubbles: Vec::new(),
            bbo_transitions: Vec::new(),
            dom_snapshot: None,
            top_footprint_cells: Vec::new(),
            footprint_poc: None,
            flow_summary: FlowSummary::default(),
            add_on_events: add_on_events[1..].to_vec(),
        };

        assert_eq!(
            heuristic_order_flow_events(&snapshot)[0].kind,
            "Stop-run-like"
        );
        assert_eq!(
            replay_add_on_events(&snapshot, Some(&derived_window)),
            add_on_events[1..].to_vec()
        );
    }

    #[test]
    fn mbo_price_formats_fixed_scale() {
        let price = MboPrice::from_fixed(18_250 * FIXED_PRICE_SCALE + 250_000_000);

        assert_eq!(format_mbo_price(price), "18250.25");
    }

    #[test]
    fn filtered_flow_threshold_uses_upper_liquidity_quartile() {
        let mut snapshot = snapshot();
        snapshot.visible_liquidity_cells = vec![
            liquidity_cell(100, 2),
            liquidity_cell(101, 4),
            liquidity_cell(102, 8),
            liquidity_cell(103, 16),
        ];

        assert_eq!(
            filtered_liquidity_threshold_for_cells(&snapshot.visible_liquidity_cells),
            16
        );
    }

    #[test]
    fn tradermap_layout_includes_center_profile_lane() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1_000.0, 420.0));
        let (left, profile, right) = tradermap_layout_rects(rect);

        assert!(left.left() >= rect.left());
        assert!(left.right() < profile.left());
        assert!(profile.right() < right.left());
        assert!(right.right() <= rect.right());
        assert!(profile.width() >= 96.0);
        assert!(left.width() > profile.width());
        assert!(right.width() > profile.width());
    }

    #[test]
    fn tradermap_threshold_can_use_replay_window_cells_only() {
        let replay_cells = vec![
            liquidity_cell_at(20, 100, 2),
            liquidity_cell_at(21, 101, 4),
            liquidity_cell_at(22, 102, 80),
        ];
        let full_snapshot_cells = vec![
            liquidity_cell_at(10, 100, 2),
            liquidity_cell_at(11, 101, 4),
            liquidity_cell_at(12, 102, 80),
            liquidity_cell_at(13, 103, 10_000),
        ];

        assert_eq!(filtered_liquidity_threshold_for_cells(&replay_cells), 80);
        assert_eq!(
            filtered_liquidity_threshold_for_cells(&full_snapshot_cells),
            10_000
        );
    }

    #[test]
    fn heuristic_events_detect_one_sided_volume_and_absorption_like_patterns() {
        let mut snapshot = snapshot();
        snapshot.visible_liquidity_cells = vec![liquidity_cell(100, 20)];
        snapshot.volume_bubbles = vec![(
            VolumeBubbleKey {
                interval_start_ns: 10,
                instrument_id: 1,
                price: price(100),
            },
            VolumeBubble {
                buy_volume: 20,
                sell_volume: 0,
                unknown_volume: 0,
                trade_count: 1,
                fill_count: 0,
            },
        )];

        let events = heuristic_order_flow_events(&snapshot);

        assert!(events.iter().any(|event| event.kind == "Stop-run-like"));
        assert!(events.iter().any(|event| event.kind == "Iceberg-like"));
    }

    #[test]
    fn market_pulse_metrics_cover_flow_book_spread_and_price_change() {
        let mut snapshot = snapshot();
        snapshot.flow_summary = FlowSummary {
            buy_volume: 9,
            sell_volume: 3,
            ..FlowSummary::default()
        };
        snapshot.dom_snapshot = Some(DomColumnSnapshot {
            instrument_id: 1,
            quality: BookQuality::Clean,
            best_bid: Some(PriceLevel {
                price: price(100),
                size: 3,
            }),
            best_ask: Some(PriceLevel {
                price: price(101),
                size: 1,
            }),
            max_size: 3,
            levels: vec![DomColumnLevel {
                price: price(100),
                bid_size: 3,
                ask_size: 1,
                total_size: 4,
                side_hint: MboSide::Bid,
            }],
        });
        snapshot.book_snapshot = Some(OrderBookSnapshot {
            instrument_id: 1,
            quality: BookQuality::Clean,
            order_count: 2,
            bid_levels: 1,
            ask_levels: 1,
            top: TopOfBook {
                bid: Some(PriceLevel {
                    price: price(100),
                    size: 3,
                }),
                ask: Some(PriceLevel {
                    price: price(101),
                    size: 1,
                }),
                spread: Some(1.0),
                mid: Some(100.5),
            },
        });
        snapshot.bbo_transitions = vec![
            BboTransition {
                ts_event: 1,
                instrument_id: 1,
                bid: None,
                ask: None,
                spread: None,
                mid: Some(100.0),
            },
            BboTransition {
                ts_event: 2,
                instrument_id: 1,
                bid: None,
                ask: None,
                spread: None,
                mid: Some(101.0),
            },
        ];

        let metrics = pulse_metrics_at_cursor(&snapshot, None);

        assert!(
            metrics
                .iter()
                .any(|metric| metric.label == "Volume pressure" && metric.value == 0.75)
        );
        assert!(metrics.iter().any(|metric| metric.label == "Book pressure"));
        assert!(
            metrics
                .iter()
                .any(|metric| metric.label == "Spread tightness")
        );
        assert!(metrics.iter().any(|metric| metric.label == "Price change"));
    }

    #[test]
    fn replay_controls_default_to_recent_terminal_context() {
        let controls = ReplayControlState::default();

        assert_eq!(controls.cursor_fraction, DEFAULT_REPLAY_CURSOR_FRACTION);
        assert_eq!(controls.window_index, DEFAULT_REPLAY_WINDOW_INDEX);
        assert_eq!(
            replay_window_span_ns(&controls),
            Some(DERIVED_INITIAL_VIEWPORT_NS)
        );
        assert!(!controls.playing);
    }

    #[test]
    fn replay_window_places_cursor_near_right_side_of_working_view() {
        let controls = ReplayControlState::default();
        let cursor_ts = 30 * 60 * SECOND_NS;

        let (start_ts, end_ts) = replay_window_bounds(0, 60 * 60 * SECOND_NS, cursor_ts, &controls);

        assert_eq!(end_ts - start_ts, DERIVED_INITIAL_VIEWPORT_NS);
        let cursor_position = (cursor_ts - start_ts) as f32 / (end_ts - start_ts) as f32;
        assert!((cursor_position - BOOKMAP_VISIBLE_CURSOR_FRACTION).abs() < 0.01);
    }

    #[test]
    fn replay_audit_counts_visible_surfaces_at_cursor() {
        let mut snapshot = snapshot();
        snapshot.visible_liquidity_cells = vec![liquidity_cell(100, 2), liquidity_cell(101, 4)];
        snapshot.visible_liquidity_cells[1].0.interval_start_ns = 20;
        snapshot.volume_bubbles = vec![
            (
                VolumeBubbleKey {
                    interval_start_ns: 10,
                    instrument_id: 1,
                    price: price(100),
                },
                VolumeBubble {
                    buy_volume: 2,
                    ..VolumeBubble::default()
                },
            ),
            (
                VolumeBubbleKey {
                    interval_start_ns: 30,
                    instrument_id: 1,
                    price: price(101),
                },
                VolumeBubble {
                    sell_volume: 4,
                    ..VolumeBubble::default()
                },
            ),
        ];
        snapshot.bbo_transitions = vec![
            BboTransition {
                ts_event: 10,
                instrument_id: 1,
                bid: None,
                ask: None,
                spread: None,
                mid: Some(100.0),
            },
            BboTransition {
                ts_event: 30,
                instrument_id: 1,
                bid: None,
                ask: None,
                spread: None,
                mid: Some(101.0),
            },
        ];
        snapshot.replay_checkpoints = vec![
            ReplayCheckpointRecord {
                event_index: 1,
                timestamp_ns: 10,
                instrument_count: 1,
            },
            ReplayCheckpointRecord {
                event_index: 2,
                timestamp_ns: 20,
                instrument_count: 1,
            },
        ];
        snapshot.dom_snapshot = Some(DomColumnSnapshot {
            instrument_id: 1,
            quality: BookQuality::Clean,
            best_bid: None,
            best_ask: None,
            max_size: 1,
            levels: vec![DomColumnLevel {
                price: price(100),
                bid_size: 1,
                ask_size: 0,
                total_size: 1,
                side_hint: MboSide::Bid,
            }],
        });
        let controls = ReplayControlState {
            cursor_fraction: 0.5,
            ..ReplayControlState::default()
        };

        let audit = replay_audit(&snapshot, &controls).expect("audit");

        assert_eq!(audit.min_ts, 10);
        assert_eq!(audit.max_ts, 30);
        assert_eq!(audit.cursor_ts, 20);
        assert_eq!(audit.window_start_ts, 10);
        assert_eq!(audit.window_end_ts, 30);
        assert_eq!(audit.checkpoint_event_index, Some(2));
        assert_eq!(audit.checkpoint_ts, Some(20));
        assert_eq!(audit.visible_cells, 2);
        assert_eq!(audit.total_cells, 2);
        assert_eq!(audit.visible_bubbles, 1);
        assert_eq!(audit.total_bubbles, 2);
        assert_eq!(audit.visible_bbo, 1);
        assert_eq!(audit.total_bbo, 2);
        assert_eq!(audit.dom_levels, 1);
    }

    #[test]
    #[ignore = "requires local ignored E-mini MBO DBN cache"]
    fn local_order_flow_preview_loads_bounded_cache_snapshot() {
        let snapshot =
            load_preview_from_manifest_dir(Path::new("E-mini/manifests")).expect("preview");

        assert!(snapshot.source_label.contains("NQ.FUT"));
        assert!(snapshot.event_count > 0);
        assert!(!snapshot.raw_path.as_os_str().is_empty());
    }
}
