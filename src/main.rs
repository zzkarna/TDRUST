mod chart_core;
mod latency;
mod market_data;
mod order_flow;
mod order_flow_widgets;
mod persistence;
mod studies;
mod timeframe_widget;
mod tv_chart;
mod widget_api;
mod workspace_shell;

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use chart_core::{
    BarSeries, BarStore, ChartColor, ChartTheme, DrawingAnchor, DrawingBulkAction,
    DrawingBulkActionStatus, DrawingEditError, DrawingGroupId, DrawingId, DrawingKind,
    DrawingLineStyle, DrawingPayload, DrawingSpec, DrawingStore, DrawingStyle,
    FibRetracementLabelPosition, FibRetracementLabelSide, FibRetracementLevel,
    FibRetracementSettings, LogicalRange, OhlcBar, OhlcField, PaneId, PaneRect, PaneSpec,
    PositionDrawingSettings, PriceRange, PriceScaleId, PriceScaleValueMode, SeriesId, TailUpdate,
    normalize_timestamp_seconds,
};
use eframe::egui;
use latency::{
    LatencyMetricSummary, LatencyProbe, LatencySnapshot, input_event_is_latency_relevant,
};
use market_data::{
    BarInterval, BinanceFeed, CoinGeckoFeed, DEFAULT_INTERVAL, MarketDataFeed, MarketDataKey,
    MarketDataService, MarketDataSnapshot, MarketSource, MassiveFeed, Resolution, SubscribeRequest,
    SubscriptionId, SymbolType, TIMEFRAMES, YFinanceFeed, format_interval_status, history_request,
    load_csv, normalized_symbol,
};
use order_flow::{
    DerivedCacheBuildConfig, DerivedCacheFinalizationEvent, DerivedCacheFinalizationEventKind,
    DerivedCacheManifest, DerivedCacheProgressEvent, DerivedCacheProgressEventKind,
    DerivedCacheProgressManifest, HistoricalDbnMboSource, MboCacheEntry, MboCacheRegistry,
    build_derived_cache_resumable_with_progress, derived_cache_artifact_storage_report,
    derived_cache_manifest_path_for_entry, finalize_derived_cache_partitions_with_progress,
};
use order_flow_widgets::OrderFlowPreviewService;
use persistence::{
    ChartPresetDocument, ChartPresetDocumentEntry, DrawingDocument, DrawingDocumentScope,
    DrawingToolTemplateDocument, DrawingToolTemplatesDocument, FibPresetDocument,
    FibPresetDocumentEntry, LiveChartSessionDocument, LiveChartSessionDocumentEntry,
    PersistenceDocument, PersistenceError, PersistenceSchemaError, StoragePaths,
    UserSettingsDocument, WorkspaceLayoutDocument, read_json_document, write_json_document_atomic,
};
use studies::{
    EmaStudy, MacdStudy, RsiStudy, SmaStudy, StudyColor, StudyDataSource, StudyGuideStyle, StudyId,
    StudyOutputId, StudyOutputStyle, StudyPriceSource, StudyRuntime, StudyRuntimeUpdate,
    StudySmoothingKind, StudyValue,
};
use tv_chart::engine::prepare_chart_snapshot_with_studies;
use tv_chart::{
    ChartCrosshairMagnet, ChartCrosshairMagnetMode, ChartDrawingLayer, ChartSnapshot,
    ChartUiResponse, ChartWidgetSizing, DrawingHit, DrawingHitTarget, EguiChartRenderer,
    SceneLayout, ScenePoint, SceneRect, SceneSnapshot, SceneTransform, TvChartState,
    hit_test_drawings, scene::DrawingHitTestRequest, show_candlestick_chart_with_sizing,
};
use widget_api::{
    WidgetHostClass, WidgetHostMode, WidgetId, WidgetKind, WidgetRenderStatus, WidgetSnapshot,
};
use workspace_shell::{
    ShellCommandAction, WorkspaceLayoutRect, WorkspacePresetKind, WorkspaceShell,
};

const DEFAULT_SYMBOL: &str = "NQH6";
const BACKFILL_TRIGGER_BARS: f64 = 80.0;
const BACKFILL_BAR_COUNT: usize = 720;
const TOP_BAR_HEIGHT: f32 = 34.0;
const LIVE_CHART_HEADER_HOST_GAP: f32 = 8.0;
const LIVE_CHART_HOST_CONTROL_GAP: f32 = 6.0;
const STUDY_LEGEND_ROW_HEIGHT: f32 = 22.0;
const STUDY_LEGEND_ROW_GAP: f32 = 2.0;
const STUDY_LEGEND_CONTROL_SIZE: f32 = 18.0;
const STUDY_LEGEND_CONTROL_GAP: f32 = 3.0;
const DEFAULT_SMA_PERIOD: usize = 20;
const DEFAULT_EMA_PERIOD: usize = 20;
const DEFAULT_RSI_PERIOD: usize = 14;
const DEFAULT_MACD_FAST_PERIOD: usize = 12;
const DEFAULT_MACD_SLOW_PERIOD: usize = 26;
const DEFAULT_MACD_SIGNAL_PERIOD: usize = 9;
const SEARCH_DEBOUNCE_MS: u64 = 180;
const LATENCY_LOG_INTERVAL: Duration = Duration::from_secs(2);
const CHART_SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(3);
const MIN_STUDY_PERIOD: usize = 1;
const MAX_STUDY_PERIOD: usize = 500;
const MIN_STUDY_OFFSET: i32 = -500;
const MAX_STUDY_OFFSET: i32 = 500;
const MIN_STUDY_STYLE_WIDTH: f32 = 0.5;
const MAX_STUDY_STYLE_WIDTH: f32 = 5.0;
const APP_UNDO_LIMIT: usize = 64;
const DRAWING_TOOL_BUTTON_WIDTH: f32 = 30.0;
const DRAWING_TOOL_BUTTON_HEIGHT: f32 = 28.0;
const DRAWING_TOOL_BUTTON_GAP: f32 = 4.0;
const DRAWING_TOOL_GLOBAL_ACTION_HEIGHT: f32 = 24.0;
const DRAWING_TOOL_RAIL_WIDTH: f32 = 56.0;
const CHART_UTILITY_RAIL_WIDTH: f32 = 48.0;
const DRAWING_TOOLBAR_MARGIN: f32 = 4.0;
const DRAWING_TOOLBAR_TOP_OFFSET: f32 = 8.0;
const SELECTED_DRAWING_TOOLBAR_WIDTH: f32 = 480.0;
const SELECTED_DRAWING_TOOLBAR_HEIGHT: f32 = 38.0;
const SELECTED_DRAWING_TOOLBAR_TOP_OFFSET: f32 = 6.0;
const DRAWING_COLOR_PALETTE: [ChartColor; 48] = [
    ChartColor::rgb(242, 242, 242),
    ChartColor::rgb(210, 210, 210),
    ChartColor::rgb(174, 174, 174),
    ChartColor::rgb(140, 140, 140),
    ChartColor::rgb(92, 92, 92),
    ChartColor::rgb(24, 24, 24),
    ChartColor::rgb(247, 49, 67),
    ChartColor::rgb(255, 152, 0),
    ChartColor::rgb(255, 226, 51),
    ChartColor::rgb(74, 181, 86),
    ChartColor::rgb(14, 160, 137),
    ChartColor::rgb(18, 188, 204),
    ChartColor::rgb(45, 98, 255),
    ChartColor::rgb(105, 61, 190),
    ChartColor::rgb(170, 43, 190),
    ChartColor::rgb(235, 30, 96),
    ChartColor::rgb(248, 190, 193),
    ChartColor::rgb(255, 218, 171),
    ChartColor::rgb(255, 245, 187),
    ChartColor::rgb(190, 231, 195),
    ChartColor::rgb(157, 218, 207),
    ChartColor::rgb(158, 225, 232),
    ChartColor::rgb(169, 203, 239),
    ChartColor::rgb(198, 184, 229),
    ChartColor::rgb(222, 187, 224),
    ChartColor::rgb(235, 181, 198),
    ChartColor::rgb(255, 149, 158),
    ChartColor::rgb(255, 205, 129),
    ChartColor::rgb(255, 241, 143),
    ChartColor::rgb(163, 216, 169),
    ChartColor::rgb(107, 200, 185),
    ChartColor::rgb(126, 213, 224),
    ChartColor::rgb(132, 185, 241),
    ChartColor::rgb(181, 152, 222),
    ChartColor::rgb(203, 139, 211),
    ChartColor::rgb(232, 128, 171),
    ChartColor::rgb(199, 34, 49),
    ChartColor::rgb(243, 91, 0),
    ChartColor::rgb(255, 183, 43),
    ChartColor::rgb(45, 145, 58),
    ChartColor::rgb(0, 119, 96),
    ChartColor::rgb(0, 147, 156),
    ChartColor::rgb(27, 76, 206),
    ChartColor::rgb(78, 43, 164),
    ChartColor::rgb(133, 31, 171),
    ChartColor::rgb(190, 16, 89),
    ChartColor::rgb(150, 20, 32),
    ChartColor::rgb(145, 40, 0),
];
const DRAWING_MAGNET_HOLD_SECONDS: f64 = 0.45;
const MAGNET_WEAK_PRICE_DISTANCE_PX: f32 = 18.0;
const BRUSH_SAMPLE_MIN_DISTANCE_PX: f32 = 3.0;
const DEFAULT_SMA_STUDY_ID: StudyId = StudyId::new(1);
const DEFAULT_RSI_STUDY_ID: StudyId = StudyId::new(2);
const SEARCH_SOURCES: [MarketSource; 4] = [
    MarketSource::YFinance,
    MarketSource::Massive,
    MarketSource::Binance,
    MarketSource::CoinGecko,
];
const YFINANCE_SEARCH_TYPES: [SymbolType; 2] = [SymbolType::Stock, SymbolType::Futures];
const MASSIVE_SEARCH_TYPES: [SymbolType; 1] = [SymbolType::Stock];
const CRYPTO_SEARCH_TYPES: [SymbolType; 1] = [SymbolType::Crypto];

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
enum DrawingTool {
    HorizontalLine,
    HorizontalRay,
    VerticalLine,
    TrendLine,
    Ray,
    Brush,
    FibRetracement,
    LongPosition,
    ShortPosition,
    Rectangle,
    Text,
    PriceNote,
}

impl DrawingTool {
    const fn label(self) -> &'static str {
        match self {
            Self::HorizontalLine => "H",
            Self::HorizontalRay => "H>",
            Self::VerticalLine => "V",
            Self::TrendLine => "/",
            Self::Ray => "R",
            Self::Brush => "B",
            Self::FibRetracement => "Fib",
            Self::LongPosition => "L+",
            Self::ShortPosition => "S-",
            Self::Rectangle => "[]",
            Self::Text => "T",
            Self::PriceNote => "$",
        }
    }

    const fn tooltip(self) -> &'static str {
        match self {
            Self::HorizontalLine => "Horizontal line",
            Self::HorizontalRay => "Horizontal ray",
            Self::VerticalLine => "Vertical line",
            Self::TrendLine => "Trend line",
            Self::Ray => "Ray",
            Self::Brush => "Brush",
            Self::FibRetracement => "Fib retracement",
            Self::LongPosition => "Long position",
            Self::ShortPosition => "Short position",
            Self::Rectangle => "Rectangle",
            Self::Text => "Text label",
            Self::PriceNote => "Price note",
        }
    }

    const fn kind(self) -> DrawingKind {
        match self {
            Self::HorizontalLine => DrawingKind::HorizontalLine,
            Self::HorizontalRay => DrawingKind::HorizontalRay,
            Self::VerticalLine => DrawingKind::VerticalLine,
            Self::TrendLine => DrawingKind::TrendLine,
            Self::Ray => DrawingKind::Ray,
            Self::Brush => DrawingKind::Brush,
            Self::FibRetracement => DrawingKind::FibRetracement,
            Self::LongPosition => DrawingKind::LongPosition,
            Self::ShortPosition => DrawingKind::ShortPosition,
            Self::Rectangle => DrawingKind::Rectangle,
            Self::Text => DrawingKind::Text,
            Self::PriceNote => DrawingKind::PriceNote,
        }
    }

    const fn needs_two_clicks(self) -> bool {
        matches!(
            self,
            Self::TrendLine | Self::Ray | Self::FibRetracement | Self::Rectangle
        )
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
enum DrawingToolAccess {
    Available,
    Disabled,
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct DrawingToolPolicy {
    favorites: Vec<DrawingTool>,
    access_overrides: Vec<(DrawingTool, DrawingToolAccess)>,
}

impl Default for DrawingToolPolicy {
    fn default() -> Self {
        Self {
            favorites: vec![
                DrawingTool::HorizontalLine,
                DrawingTool::TrendLine,
                DrawingTool::Rectangle,
            ],
            access_overrides: Vec::new(),
        }
    }
}

impl DrawingToolPolicy {
    fn access(&self, tool: DrawingTool) -> DrawingToolAccess {
        self.access_overrides
            .iter()
            .find_map(|(candidate, access)| (*candidate == tool).then_some(*access))
            .unwrap_or(DrawingToolAccess::Available)
    }

    fn set_access(&mut self, tool: DrawingTool, access: DrawingToolAccess) {
        self.access_overrides
            .retain(|(candidate, _)| *candidate != tool);
        if access != DrawingToolAccess::Available {
            self.access_overrides.push((tool, access));
        }
    }

    fn is_favorite(&self, tool: DrawingTool) -> bool {
        self.favorites.contains(&tool)
    }

    fn set_favorite(&mut self, tool: DrawingTool, favorite: bool) {
        self.favorites.retain(|candidate| *candidate != tool);
        if favorite {
            self.favorites.push(tool);
            self.favorites.sort_by_key(|tool| drawing_tool_order(*tool));
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct DrawingToolTemplates {
    style_overrides: Vec<(DrawingTool, DrawingStyle)>,
    payload_overrides: Vec<(DrawingTool, DrawingPayload)>,
}

impl Default for DrawingToolTemplates {
    fn default() -> Self {
        Self {
            style_overrides: Vec::new(),
            payload_overrides: Vec::new(),
        }
    }
}

impl DrawingToolTemplates {
    fn style(&self, tool: DrawingTool) -> DrawingStyle {
        self.style_overrides
            .iter()
            .find_map(|(candidate, style)| (*candidate == tool).then(|| style.clone()))
            .unwrap_or_default()
    }

    fn set_style(&mut self, tool: DrawingTool, style: DrawingStyle) -> bool {
        let before = self.style_overrides.clone();
        self.style_overrides
            .retain(|(candidate, _)| *candidate != tool);
        if style != DrawingStyle::default() {
            self.style_overrides.push((tool, style));
        }
        self.style_overrides != before
    }

    fn payload(&self, tool: DrawingTool) -> Option<DrawingPayload> {
        self.payload_overrides
            .iter()
            .find_map(|(candidate, payload)| (*candidate == tool).then(|| payload.clone()))
    }

    fn set_payload(&mut self, tool: DrawingTool, payload: Option<DrawingPayload>) -> bool {
        let before = self.payload_overrides.clone();
        self.payload_overrides
            .retain(|(candidate, _)| *candidate != tool);
        if let Some(payload) = payload
            && drawing_tool_template_payload_is_valid(tool, &payload)
            && Some(&payload) != default_drawing_template_payload(tool).as_ref()
        {
            self.payload_overrides.push((tool, payload));
        }
        self.payload_overrides != before
    }

    fn from_document(document: DrawingToolTemplatesDocument) -> Self {
        let mut templates = Self::default();
        for tool in DRAWING_TOOLS {
            if let Some(template) = document.tools.get(&tool)
                && template.style.is_valid()
            {
                templates.set_style(tool, template.style.clone());
            }
            if let Some(template) = document.tools.get(&tool)
                && let Some(payload) = template.payload.clone()
            {
                templates.set_payload(tool, Some(payload));
            }
        }
        templates
    }

    fn to_document(&self) -> DrawingToolTemplatesDocument {
        let mut tools = std::collections::BTreeMap::new();
        for (tool, style) in &self.style_overrides {
            tools.insert(
                *tool,
                DrawingToolTemplateDocument {
                    style: style.clone(),
                    payload: None,
                },
            );
        }
        for (tool, payload) in &self.payload_overrides {
            tools
                .entry(*tool)
                .and_modify(|template| template.payload = Some(payload.clone()))
                .or_insert_with(|| DrawingToolTemplateDocument {
                    style: DrawingStyle::default(),
                    payload: Some(payload.clone()),
                });
        }
        DrawingToolTemplatesDocument {
            schema_version: persistence::DRAWING_TOOL_TEMPLATES_SCHEMA_VERSION,
            tools,
        }
    }
}

fn default_drawing_template_payload(tool: DrawingTool) -> Option<DrawingPayload> {
    match tool {
        DrawingTool::FibRetracement => Some(DrawingPayload::FibRetracement(
            FibRetracementSettings::default(),
        )),
        DrawingTool::LongPosition | DrawingTool::ShortPosition => {
            Some(DrawingPayload::Position(PositionDrawingSettings::default()))
        }
        _ => None,
    }
}

fn drawing_tool_template_payload_is_valid(tool: DrawingTool, payload: &DrawingPayload) -> bool {
    matches!(
        (tool, payload),
        (DrawingTool::FibRetracement, DrawingPayload::FibRetracement(settings))
            if settings.is_valid()
    ) || matches!(
        (tool, payload),
        (
            DrawingTool::LongPosition | DrawingTool::ShortPosition,
            DrawingPayload::Position(settings)
        ) if settings.is_valid()
    )
}

const DEFAULT_FIB_PRESET_NAME: &str = "Fib preset";
const DEFAULT_CHART_PRESET_NAME: &str = "Chart preset";

#[derive(Debug, Clone, PartialEq)]
struct FibPreset {
    id: String,
    name: String,
    settings: FibRetracementSettings,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
struct FibPresetCollection {
    active_preset_id: Option<String>,
    presets: Vec<FibPreset>,
}

impl FibPresetCollection {
    fn from_document(document: FibPresetDocument) -> Self {
        let mut collection = Self::default();
        for entry in document.presets {
            let id = entry.id.trim();
            if id.is_empty() || collection.has_preset(id) || !entry.settings.is_valid() {
                continue;
            }
            collection.presets.push(FibPreset {
                id: id.to_string(),
                name: normalize_fib_preset_name(&entry.name),
                settings: entry.settings,
                created_at: entry.created_at,
                updated_at: entry.updated_at.max(entry.created_at),
            });
        }
        if let Some(active_preset_id) = document.active_preset_id
            && collection.has_preset(&active_preset_id)
        {
            collection.active_preset_id = Some(active_preset_id);
        }
        collection
    }

    fn to_document(&self) -> FibPresetDocument {
        FibPresetDocument {
            schema_version: persistence::FIB_PRESETS_SCHEMA_VERSION,
            active_preset_id: self
                .active_preset_id
                .as_ref()
                .filter(|id| self.has_preset(id))
                .cloned(),
            presets: self
                .presets
                .iter()
                .filter(|preset| preset.settings.is_valid())
                .map(|preset| FibPresetDocumentEntry {
                    id: preset.id.clone(),
                    name: normalize_fib_preset_name(&preset.name),
                    settings: preset.settings.clone(),
                    created_at: preset.created_at,
                    updated_at: preset.updated_at.max(preset.created_at),
                })
                .collect(),
        }
    }

    fn active_preset(&self) -> Option<&FibPreset> {
        let active_id = self.active_preset_id.as_deref()?;
        self.presets.iter().find(|preset| preset.id == active_id)
    }

    fn active_settings(&self) -> Option<FibRetracementSettings> {
        self.active_preset().map(|preset| preset.settings.clone())
    }

    fn preset(&self, id: &str) -> Option<&FibPreset> {
        self.presets.iter().find(|preset| preset.id == id)
    }

    fn has_preset(&self, id: &str) -> bool {
        self.preset(id).is_some()
    }

    fn set_active_preset(&mut self, id: &str) -> bool {
        if !self.has_preset(id) {
            return false;
        }
        if self.active_preset_id.as_deref() == Some(id) {
            return false;
        }
        self.active_preset_id = Some(id.to_string());
        true
    }

    fn clear_active_preset(&mut self) -> bool {
        let had_active = self.active_preset_id.is_some();
        self.active_preset_id = None;
        had_active
    }

    fn save_new_preset(
        &mut self,
        name: impl AsRef<str>,
        settings: FibRetracementSettings,
        now: i64,
    ) -> Option<String> {
        if !settings.is_valid() {
            return None;
        }
        let name = normalize_fib_preset_name(name.as_ref());
        let id = self.next_preset_id(&name, now);
        self.presets.push(FibPreset {
            id: id.clone(),
            name,
            settings,
            created_at: now,
            updated_at: now,
        });
        self.active_preset_id = Some(id.clone());
        Some(id)
    }

    fn update_preset(&mut self, id: &str, settings: FibRetracementSettings, now: i64) -> bool {
        if !settings.is_valid() {
            return false;
        }
        let Some(preset) = self.presets.iter_mut().find(|preset| preset.id == id) else {
            return false;
        };
        preset.settings = settings;
        preset.updated_at = now.max(preset.created_at);
        true
    }

    fn delete_preset(&mut self, id: &str) -> bool {
        let before_len = self.presets.len();
        self.presets.retain(|preset| preset.id != id);
        if self.active_preset_id.as_deref() == Some(id) {
            self.active_preset_id = None;
        }
        self.presets.len() != before_len
    }

    fn next_preset_id(&self, name: &str, now: i64) -> String {
        let base = format!("fib-{}-{}", now.max(0), fib_preset_id_slug(name));
        let mut id = base.clone();
        let mut suffix = 2;
        while self.has_preset(&id) {
            id = format!("{base}-{suffix}");
            suffix += 1;
        }
        id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FibPresetUiAction {
    SaveAs,
    UpdateActive,
    DeleteActive,
    Apply(String),
    ClearActive,
}

fn normalize_fib_preset_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        DEFAULT_FIB_PRESET_NAME.to_string()
    } else {
        trimmed.chars().take(80).collect()
    }
}

fn fib_preset_id_slug(name: &str) -> String {
    let mut slug = String::new();
    let mut previous_separator = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_separator = false;
        } else if !slug.is_empty() && !previous_separator {
            slug.push('-');
            previous_separator = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "preset".to_string()
    } else {
        slug.chars().take(48).collect()
    }
}

fn current_unix_timestamp_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

fn chart_snapshot_file_name(symbol: &str, timeframe: &str, timestamp: i64) -> String {
    format!(
        "rustchart-{}-{}-{}.png",
        chart_snapshot_file_component(symbol),
        chart_snapshot_file_component(timeframe),
        timestamp.max(0)
    )
}

fn chart_snapshot_file_component(value: &str) -> String {
    let mut slug = String::new();
    let mut previous_separator = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_separator = false;
        } else if !slug.is_empty() && !previous_separator {
            slug.push('-');
            previous_separator = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "chart".to_string()
    } else {
        slug.chars().take(48).collect()
    }
}

fn chart_snapshot_image_region(
    image: &egui::ColorImage,
    region: egui::Rect,
    pixels_per_point: f32,
) -> Option<egui::ColorImage> {
    let pixels_per_point = pixels_per_point.max(0.01);
    let viewport_rect = egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(
            image.width() as f32 / pixels_per_point,
            image.height() as f32 / pixels_per_point,
        ),
    );
    let clipped = region.intersect(viewport_rect);
    if !clipped.is_positive() || clipped.width() < 1.0 || clipped.height() < 1.0 {
        return None;
    }
    Some(image.region(&clipped, Some(pixels_per_point)))
}

fn chart_snapshot_rgba_bytes(image: &egui::ColorImage) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(image.pixels.len() * 4);
    for pixel in &image.pixels {
        rgba.extend_from_slice(&pixel.to_srgba_unmultiplied());
    }
    rgba
}

fn chart_snapshot_output_dir(storage_paths: Option<&StoragePaths>) -> Result<PathBuf, String> {
    if let Some(home) = env::var_os("HOME") {
        let downloads = PathBuf::from(home).join("Downloads");
        if downloads.is_dir() {
            return Ok(downloads);
        }
    }
    let Some(paths) = storage_paths else {
        return Err("no Downloads folder or storage root is available".to_string());
    };
    Ok(paths.root().join("snapshots"))
}

fn save_chart_snapshot_png(
    image: &egui::ColorImage,
    storage_paths: Option<&StoragePaths>,
    request: &ChartSnapshotCaptureRequest,
) -> Result<Option<PathBuf>, String> {
    let output_dir = chart_snapshot_output_dir(storage_paths)?;
    let default_file_name = chart_snapshot_file_name(
        &request.source_label,
        &request.timeframe_label,
        request.requested_at,
    );
    let Some(output_path) = chart_snapshot_choose_output_path(&output_dir, &default_file_name)?
    else {
        return Ok(None);
    };
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create snapshot directory: {error}"))?;
    }
    let rgba = chart_snapshot_rgba_bytes(image);
    image::save_buffer_with_format(
        &output_path,
        &rgba,
        image.width() as u32,
        image.height() as u32,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(|error| format!("could not write snapshot PNG: {error}"))?;
    Ok(Some(output_path))
}

#[cfg(target_os = "macos")]
fn chart_snapshot_choose_output_path(
    default_dir: &Path,
    default_file_name: &str,
) -> Result<Option<PathBuf>, String> {
    fs::create_dir_all(default_dir)
        .map_err(|error| format!("could not create snapshot directory: {error}"))?;
    let script = format!(
        "set outputPath to POSIX path of (choose file name with prompt {} default name {} default location (POSIX file {}))\nreturn outputPath",
        applescript_string_literal("Save chart snapshot"),
        applescript_string_literal(default_file_name),
        applescript_string_literal(&default_dir.display().to_string())
    );
    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|error| format!("could not open save dialog: {error}"))?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if path.is_empty() {
            return Ok(None);
        }
        return Ok(Some(PathBuf::from(path)));
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("User canceled") || stderr.contains("-128") {
        return Ok(None);
    }
    Err(format!("save dialog failed: {}", stderr.trim()))
}

#[cfg(not(target_os = "macos"))]
fn chart_snapshot_choose_output_path(
    default_dir: &Path,
    default_file_name: &str,
) -> Result<Option<PathBuf>, String> {
    Ok(Some(default_dir.join(default_file_name)))
}

#[cfg(target_os = "macos")]
fn applescript_string_literal(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        if ch == '"' || ch == '\\' {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped.push('"');
    escaped
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChartPresetSnapshot {
    theme: ChartTheme,
    volume_visible: bool,
}

impl ChartPresetSnapshot {
    fn new(theme: ChartTheme, volume_visible: bool) -> Self {
        Self {
            theme,
            volume_visible,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChartPreset {
    id: String,
    name: String,
    snapshot: ChartPresetSnapshot,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ChartPresetCollection {
    active_preset_id: Option<String>,
    presets: Vec<ChartPreset>,
}

impl ChartPresetCollection {
    fn from_document(document: ChartPresetDocument) -> Self {
        let mut collection = Self::default();
        for entry in document.presets {
            let id = entry.id.trim();
            if id.is_empty() || collection.has_preset(id) {
                continue;
            }
            collection.presets.push(ChartPreset {
                id: id.to_string(),
                name: normalize_chart_preset_name(&entry.name),
                snapshot: ChartPresetSnapshot::new(entry.theme, entry.volume_visible),
                created_at: entry.created_at,
                updated_at: entry.updated_at.max(entry.created_at),
            });
        }
        if let Some(active_preset_id) = document.active_preset_id
            && collection.has_preset(&active_preset_id)
        {
            collection.active_preset_id = Some(active_preset_id);
        }
        collection
    }

    fn to_document(&self) -> ChartPresetDocument {
        ChartPresetDocument {
            schema_version: persistence::CHART_PRESETS_SCHEMA_VERSION,
            active_preset_id: self
                .active_preset_id
                .as_ref()
                .filter(|id| self.has_preset(id))
                .cloned(),
            presets: self
                .presets
                .iter()
                .map(|preset| ChartPresetDocumentEntry {
                    id: preset.id.clone(),
                    name: normalize_chart_preset_name(&preset.name),
                    theme: preset.snapshot.theme.clone(),
                    volume_visible: preset.snapshot.volume_visible,
                    created_at: preset.created_at,
                    updated_at: preset.updated_at.max(preset.created_at),
                })
                .collect(),
        }
    }

    fn active_preset(&self) -> Option<&ChartPreset> {
        let active_id = self.active_preset_id.as_deref()?;
        self.presets.iter().find(|preset| preset.id == active_id)
    }

    fn active_snapshot(&self) -> Option<ChartPresetSnapshot> {
        self.active_preset().map(|preset| preset.snapshot.clone())
    }

    fn preset(&self, id: &str) -> Option<&ChartPreset> {
        self.presets.iter().find(|preset| preset.id == id)
    }

    fn has_preset(&self, id: &str) -> bool {
        self.preset(id).is_some()
    }

    fn set_active_preset(&mut self, id: &str) -> bool {
        if !self.has_preset(id) {
            return false;
        }
        if self.active_preset_id.as_deref() == Some(id) {
            return false;
        }
        self.active_preset_id = Some(id.to_string());
        true
    }

    fn clear_active_preset(&mut self) -> bool {
        let had_active = self.active_preset_id.is_some();
        self.active_preset_id = None;
        had_active
    }

    fn save_new_preset(
        &mut self,
        name: impl AsRef<str>,
        snapshot: ChartPresetSnapshot,
        now: i64,
    ) -> String {
        let name = normalize_chart_preset_name(name.as_ref());
        let id = self.next_preset_id(&name, now);
        self.presets.push(ChartPreset {
            id: id.clone(),
            name,
            snapshot,
            created_at: now,
            updated_at: now,
        });
        self.active_preset_id = Some(id.clone());
        id
    }

    fn update_preset(&mut self, id: &str, snapshot: ChartPresetSnapshot, now: i64) -> bool {
        let Some(preset) = self.presets.iter_mut().find(|preset| preset.id == id) else {
            return false;
        };
        preset.snapshot = snapshot;
        preset.updated_at = now.max(preset.created_at);
        true
    }

    fn delete_preset(&mut self, id: &str) -> bool {
        let before_len = self.presets.len();
        self.presets.retain(|preset| preset.id != id);
        if self.active_preset_id.as_deref() == Some(id) {
            self.active_preset_id = None;
        }
        self.presets.len() != before_len
    }

    fn next_preset_id(&self, name: &str, now: i64) -> String {
        let base = format!("chart-{}-{}", now.max(0), chart_preset_id_slug(name));
        let mut id = base.clone();
        let mut suffix = 2;
        while self.has_preset(&id) {
            id = format!("{base}-{suffix}");
            suffix += 1;
        }
        id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ChartPresetUiAction {
    SaveAs,
    UpdateActive,
    DeleteActive,
    Apply(String),
    ClearActive,
}

fn normalize_chart_preset_name(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        DEFAULT_CHART_PRESET_NAME.to_string()
    } else {
        trimmed.chars().take(80).collect()
    }
}

fn chart_preset_id_slug(name: &str) -> String {
    let mut slug = String::new();
    let mut previous_separator = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            previous_separator = false;
        } else if !slug.is_empty() && !previous_separator {
            slug.push('-');
            previous_separator = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "preset".to_string()
    } else {
        slug.chars().take(48).collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrawingToolbarToolGroup {
    Favorites,
    Main,
}

const DRAWING_TOOLS: [DrawingTool; 12] = [
    DrawingTool::HorizontalLine,
    DrawingTool::HorizontalRay,
    DrawingTool::VerticalLine,
    DrawingTool::TrendLine,
    DrawingTool::Ray,
    DrawingTool::Brush,
    DrawingTool::FibRetracement,
    DrawingTool::LongPosition,
    DrawingTool::ShortPosition,
    DrawingTool::Rectangle,
    DrawingTool::Text,
    DrawingTool::PriceNote,
];

const fn drawing_kind_is_text_editable(kind: DrawingKind) -> bool {
    matches!(kind, DrawingKind::Text | DrawingKind::AnchoredText)
}

#[derive(Debug, Clone, PartialEq)]
struct AppUndoSnapshot {
    drawings: DrawingStore,
    selected_drawing_id: Option<DrawingId>,
    study_instances: Vec<StudyInstance>,
    selected_study_instance_id: Option<StudyId>,
    study_settings_instance_id: Option<StudyId>,
    selected_timeframe: BarInterval,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MeasurePoint {
    pane: PaneId,
    logical: f64,
    price: f64,
    time: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MeasureState {
    start: MeasurePoint,
    current: MeasurePoint,
    phase: MeasurePhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeasurePhase {
    Placing,
    Placed,
    Adjusting(MeasureDragTarget),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MeasureDragTarget {
    Start,
    Current,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MeasureStats {
    price_delta: f64,
    percent_delta: Option<f64>,
    bars_delta: i64,
    seconds_delta: Option<i64>,
    volume: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
struct DrawingCreationState {
    tool: DrawingTool,
    pane: PaneId,
    first_anchor: DrawingAnchor,
    preview_id: Option<DrawingId>,
    undo_snapshot: AppUndoSnapshot,
}

#[derive(Debug, Clone, PartialEq)]
struct DrawingDragState {
    drawing_id: DrawingId,
    kind: DrawingKind,
    pane: PaneId,
    start_position: egui::Pos2,
    start_anchors: Vec<DrawingAnchor>,
    mode: DrawingDragMode,
    undo_snapshot: AppUndoSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrawingDragMode {
    Move,
    Anchor(usize),
}

#[derive(Debug, Clone, PartialEq)]
struct DrawingTextEditState {
    drawing_id: DrawingId,
    draft: String,
    focus: bool,
    undo_snapshot: AppUndoSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrawingMagnetMode {
    Off,
    Weak,
    Strong,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MagnetSnap {
    time: i64,
    logical: f64,
    price: f64,
    field: OhlcField,
}

#[derive(Debug)]
enum MarketEvent {
    Loaded {
        widget_id: WidgetId,
        request_id: u64,
        source: MarketSource,
        symbol: String,
        bars: Vec<OhlcBar>,
    },
    LoadFailed {
        widget_id: WidgetId,
        request_id: u64,
        symbol: String,
        error: String,
    },
    LiveBar {
        widget_id: WidgetId,
        request_id: u64,
        bar: OhlcBar,
    },
    Backfilled {
        widget_id: WidgetId,
        request_id: u64,
        symbol: String,
        to: i64,
        bars: Vec<OhlcBar>,
        no_data: bool,
    },
    BackfillFailed {
        widget_id: WidgetId,
        request_id: u64,
        symbol: String,
        to: i64,
        error: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchSourceFilter {
    All,
    Source(MarketSource),
}

impl SearchSourceFilter {
    const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Source(source) => source.label(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchTypeFilter {
    All,
    Stock,
    Futures,
    Crypto,
}

impl SearchTypeFilter {
    const fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Stock => "Stocks",
            Self::Futures => "Futures",
            Self::Crypto => "Crypto",
        }
    }

    const fn symbol_type(self) -> Option<SymbolType> {
        match self {
            Self::All => None,
            Self::Stock => Some(SymbolType::Stock),
            Self::Futures => Some(SymbolType::Futures),
            Self::Crypto => Some(SymbolType::Crypto),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SearchCandidate {
    source: MarketSource,
    symbol_type: SymbolType,
    symbol: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TimeframeInputCandidate {
    interval: Option<BarInterval>,
    unit: TimeframeInputUnit,
    amount: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeframeInputUnit {
    Minutes,
    Hours,
    Days,
    Weeks,
    Months,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct WorkspaceWidgetHostResponse {
    open: bool,
    host_class: WidgetHostClass,
    focused: bool,
    layout_rect: Option<WorkspaceLayoutRect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChartWidgetDensity {
    Full,
    Compact,
    Mini,
    Tiny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChartSnapshotAction {
    DownloadImage,
    CopyImage,
}

#[derive(Debug, Clone)]
struct ChartSnapshotCaptureRequest {
    widget_id: WidgetId,
    viewport_id: egui::ViewportId,
    action: ChartSnapshotAction,
    capture_rect: egui::Rect,
    pixels_per_point: f32,
    source_label: String,
    timeframe_label: String,
    requested_at: i64,
}

struct LiveChartInstanceState {
    chart: TvChartState,
    chart_renderer: EguiChartRenderer,
    study_runtime: StudyRuntime,
    study_instances: Vec<StudyInstance>,
    next_study_instance_id: u32,
    indicators_open: bool,
    indicator_panel_rect: Option<egui::Rect>,
    chart_settings_open: bool,
    chart_settings_panel_rect: Option<egui::Rect>,
    object_tree_open: bool,
    object_tree_panel_rect: Option<egui::Rect>,
    chart_settings_tab: ChartSettingsTab,
    chart_context_menu: Option<ChartContextMenuState>,
    active_drawing_tool: Option<DrawingTool>,
    measure_active: bool,
    measure_state: Option<MeasureState>,
    drawing_tool_policy_menu_open: bool,
    app_undo_stack: Vec<AppUndoSnapshot>,
    app_redo_stack: Vec<AppUndoSnapshot>,
    preserve_app_history_on_load_request_id: Option<u64>,
    drawing_magnet_mode: DrawingMagnetMode,
    drawing_magnet_menu_open: bool,
    drawing_magnet_hold_started_at: Option<f64>,
    drawing_magnet_hold_opened: bool,
    drawing_creation: Option<DrawingCreationState>,
    drawing_drag: Option<DrawingDragState>,
    selected_drawing_id: Option<DrawingId>,
    selected_drawing_toolbar_position: Option<egui::Pos2>,
    selected_drawing_toolbar_drag_start: Option<egui::Pos2>,
    fib_settings_drawing_id: Option<DrawingId>,
    fib_settings_panel_rect: Option<egui::Rect>,
    position_settings_drawing_id: Option<DrawingId>,
    position_settings_panel_rect: Option<egui::Rect>,
    hovered_drawing_id: Option<DrawingId>,
    hovered_drawing_hit: Option<DrawingHit>,
    drawing_clipboard: Option<DrawingSpec>,
    text_editing: Option<DrawingTextEditState>,
    selected_study_instance_id: Option<StudyId>,
    study_settings_instance_id: Option<StudyId>,
    study_settings_tab: StudySettingsTab,
    active_data_key: Option<MarketDataKey>,
    bar_store: BarStore,
    source_label: String,
    search_input: String,
    search_open: bool,
    focus_search: bool,
    select_search_on_focus: bool,
    search_palette_rect: Option<egui::Rect>,
    search_source_filter: SearchSourceFilter,
    search_type_filter: SearchTypeFilter,
    search_results_query: String,
    search_results: Vec<SearchCandidate>,
    search_last_edited: Option<Instant>,
    timeframe_input_open: bool,
    timeframe_input: String,
    focus_timeframe_input: bool,
    timeframe_input_rect: Option<egui::Rect>,
    market_source: MarketSource,
    status: String,
    status_is_error: bool,
    loading: bool,
    selected_timeframe: BarInterval,
    request_id: u64,
    active_subscription: Option<SubscriptionId>,
    active_source: Option<MarketSource>,
    backfill_loading: bool,
    no_more_history: bool,
    last_backfill_to: Option<i64>,
    chart_preset_name_input: String,
    fib_preset_name_input: String,
}

impl LiveChartInstanceState {
    fn take_from_app(app: &mut ChartOnlyApp) -> Self {
        Self {
            chart: std::mem::take(&mut app.chart),
            chart_renderer: std::mem::take(&mut app.chart_renderer),
            study_runtime: std::mem::replace(&mut app.study_runtime, StudyRuntime::new()),
            study_instances: std::mem::take(&mut app.study_instances),
            next_study_instance_id: std::mem::replace(&mut app.next_study_instance_id, 1),
            indicators_open: std::mem::replace(&mut app.indicators_open, false),
            indicator_panel_rect: app.indicator_panel_rect.take(),
            chart_settings_open: std::mem::replace(&mut app.chart_settings_open, false),
            chart_settings_panel_rect: app.chart_settings_panel_rect.take(),
            object_tree_open: std::mem::replace(&mut app.object_tree_open, false),
            object_tree_panel_rect: app.object_tree_panel_rect.take(),
            chart_settings_tab: std::mem::take(&mut app.chart_settings_tab),
            chart_context_menu: app.chart_context_menu.take(),
            active_drawing_tool: app.active_drawing_tool.take(),
            measure_active: std::mem::replace(&mut app.measure_active, false),
            measure_state: app.measure_state.take(),
            drawing_tool_policy_menu_open: std::mem::replace(
                &mut app.drawing_tool_policy_menu_open,
                false,
            ),
            app_undo_stack: std::mem::take(&mut app.app_undo_stack),
            app_redo_stack: std::mem::take(&mut app.app_redo_stack),
            preserve_app_history_on_load_request_id: app
                .preserve_app_history_on_load_request_id
                .take(),
            drawing_magnet_mode: std::mem::replace(
                &mut app.drawing_magnet_mode,
                DrawingMagnetMode::Off,
            ),
            drawing_magnet_menu_open: std::mem::replace(&mut app.drawing_magnet_menu_open, false),
            drawing_magnet_hold_started_at: app.drawing_magnet_hold_started_at.take(),
            drawing_magnet_hold_opened: std::mem::replace(
                &mut app.drawing_magnet_hold_opened,
                false,
            ),
            drawing_creation: app.drawing_creation.take(),
            drawing_drag: app.drawing_drag.take(),
            selected_drawing_id: app.selected_drawing_id.take(),
            selected_drawing_toolbar_position: app.selected_drawing_toolbar_position.take(),
            selected_drawing_toolbar_drag_start: app.selected_drawing_toolbar_drag_start.take(),
            fib_settings_drawing_id: app.fib_settings_drawing_id.take(),
            fib_settings_panel_rect: app.fib_settings_panel_rect.take(),
            position_settings_drawing_id: app.position_settings_drawing_id.take(),
            position_settings_panel_rect: app.position_settings_panel_rect.take(),
            hovered_drawing_id: app.hovered_drawing_id.take(),
            hovered_drawing_hit: app.hovered_drawing_hit.take(),
            drawing_clipboard: app.drawing_clipboard.take(),
            text_editing: app.text_editing.take(),
            selected_study_instance_id: app.selected_study_instance_id.take(),
            study_settings_instance_id: app.study_settings_instance_id.take(),
            study_settings_tab: std::mem::replace(
                &mut app.study_settings_tab,
                StudySettingsTab::Inputs,
            ),
            active_data_key: app.active_data_key.take(),
            bar_store: std::mem::take(&mut app.bar_store),
            source_label: std::mem::take(&mut app.source_label),
            search_input: std::mem::take(&mut app.search_input),
            search_open: std::mem::replace(&mut app.search_open, false),
            focus_search: std::mem::replace(&mut app.focus_search, false),
            select_search_on_focus: std::mem::replace(&mut app.select_search_on_focus, false),
            search_palette_rect: app.search_palette_rect.take(),
            search_source_filter: std::mem::replace(
                &mut app.search_source_filter,
                SearchSourceFilter::All,
            ),
            search_type_filter: std::mem::replace(
                &mut app.search_type_filter,
                SearchTypeFilter::All,
            ),
            search_results_query: std::mem::take(&mut app.search_results_query),
            search_results: std::mem::take(&mut app.search_results),
            search_last_edited: app.search_last_edited.take(),
            timeframe_input_open: std::mem::replace(&mut app.timeframe_input_open, false),
            timeframe_input: std::mem::take(&mut app.timeframe_input),
            focus_timeframe_input: std::mem::replace(&mut app.focus_timeframe_input, false),
            timeframe_input_rect: app.timeframe_input_rect.take(),
            market_source: std::mem::replace(&mut app.market_source, MarketSource::YFinance),
            status: std::mem::take(&mut app.status),
            status_is_error: std::mem::replace(&mut app.status_is_error, false),
            loading: std::mem::replace(&mut app.loading, false),
            selected_timeframe: std::mem::replace(&mut app.selected_timeframe, DEFAULT_INTERVAL),
            request_id: std::mem::replace(&mut app.request_id, 0),
            active_subscription: app.active_subscription.take(),
            active_source: app.active_source.take(),
            backfill_loading: std::mem::replace(&mut app.backfill_loading, false),
            no_more_history: std::mem::replace(&mut app.no_more_history, false),
            last_backfill_to: app.last_backfill_to.take(),
            chart_preset_name_input: std::mem::take(&mut app.chart_preset_name_input),
            fib_preset_name_input: std::mem::take(&mut app.fib_preset_name_input),
        }
    }

    fn put_into_app(self, app: &mut ChartOnlyApp) {
        app.chart = self.chart;
        app.chart_renderer = self.chart_renderer;
        app.study_runtime = self.study_runtime;
        app.study_instances = self.study_instances;
        app.next_study_instance_id = self.next_study_instance_id;
        app.indicators_open = self.indicators_open;
        app.indicator_panel_rect = self.indicator_panel_rect;
        app.chart_settings_open = self.chart_settings_open;
        app.chart_settings_panel_rect = self.chart_settings_panel_rect;
        app.object_tree_open = self.object_tree_open;
        app.object_tree_panel_rect = self.object_tree_panel_rect;
        app.chart_settings_tab = self.chart_settings_tab;
        app.chart_context_menu = self.chart_context_menu;
        app.active_drawing_tool = self.active_drawing_tool;
        app.measure_active = self.measure_active;
        app.measure_state = self.measure_state;
        app.drawing_tool_policy_menu_open = self.drawing_tool_policy_menu_open;
        app.app_undo_stack = self.app_undo_stack;
        app.app_redo_stack = self.app_redo_stack;
        app.preserve_app_history_on_load_request_id = self.preserve_app_history_on_load_request_id;
        app.drawing_magnet_mode = self.drawing_magnet_mode;
        app.drawing_magnet_menu_open = self.drawing_magnet_menu_open;
        app.drawing_magnet_hold_started_at = self.drawing_magnet_hold_started_at;
        app.drawing_magnet_hold_opened = self.drawing_magnet_hold_opened;
        app.drawing_creation = self.drawing_creation;
        app.drawing_drag = self.drawing_drag;
        app.selected_drawing_id = self.selected_drawing_id;
        app.selected_drawing_toolbar_position = self.selected_drawing_toolbar_position;
        app.selected_drawing_toolbar_drag_start = self.selected_drawing_toolbar_drag_start;
        app.fib_settings_drawing_id = self.fib_settings_drawing_id;
        app.fib_settings_panel_rect = self.fib_settings_panel_rect;
        app.position_settings_drawing_id = self.position_settings_drawing_id;
        app.position_settings_panel_rect = self.position_settings_panel_rect;
        app.hovered_drawing_id = self.hovered_drawing_id;
        app.hovered_drawing_hit = self.hovered_drawing_hit;
        app.drawing_clipboard = self.drawing_clipboard;
        app.text_editing = self.text_editing;
        app.selected_study_instance_id = self.selected_study_instance_id;
        app.study_settings_instance_id = self.study_settings_instance_id;
        app.study_settings_tab = self.study_settings_tab;
        app.active_data_key = self.active_data_key;
        app.bar_store = self.bar_store;
        app.source_label = self.source_label;
        app.search_input = self.search_input;
        app.search_open = self.search_open;
        app.focus_search = self.focus_search;
        app.select_search_on_focus = self.select_search_on_focus;
        app.search_palette_rect = self.search_palette_rect;
        app.search_source_filter = self.search_source_filter;
        app.search_type_filter = self.search_type_filter;
        app.search_results_query = self.search_results_query;
        app.search_results = self.search_results;
        app.search_last_edited = self.search_last_edited;
        app.timeframe_input_open = self.timeframe_input_open;
        app.timeframe_input = self.timeframe_input;
        app.focus_timeframe_input = self.focus_timeframe_input;
        app.timeframe_input_rect = self.timeframe_input_rect;
        app.market_source = self.market_source;
        app.status = self.status;
        app.status_is_error = self.status_is_error;
        app.loading = self.loading;
        app.selected_timeframe = self.selected_timeframe;
        app.request_id = self.request_id;
        app.active_subscription = self.active_subscription;
        app.active_source = self.active_source;
        app.backfill_loading = self.backfill_loading;
        app.no_more_history = self.no_more_history;
        app.last_backfill_to = self.last_backfill_to;
        app.chart_preset_name_input = self.chart_preset_name_input;
        app.fib_preset_name_input = self.fib_preset_name_input;
    }
}

struct ChartOnlyApp {
    workspace_shell: WorkspaceShell,
    latency_probe: LatencyProbe,
    debug_latency_overlay_visible: bool,
    live_chart_instances: BTreeMap<WidgetId, LiveChartInstanceState>,
    live_chart_session_restores: BTreeMap<WidgetId, LiveChartSessionRestore>,
    order_flow_preview_service: OrderFlowPreviewService,
    loaded_live_chart_widget_id: Option<WidgetId>,
    chart: TvChartState,
    chart_renderer: EguiChartRenderer,
    study_runtime: StudyRuntime,
    study_instances: Vec<StudyInstance>,
    next_study_instance_id: u32,
    indicators_open: bool,
    indicator_panel_rect: Option<egui::Rect>,
    chart_settings_open: bool,
    chart_settings_panel_rect: Option<egui::Rect>,
    chart_snapshot_capture_rects: BTreeMap<WidgetId, egui::Rect>,
    pending_chart_snapshots: BTreeMap<WidgetId, ChartSnapshotCaptureRequest>,
    object_tree_open: bool,
    object_tree_panel_rect: Option<egui::Rect>,
    chart_settings_tab: ChartSettingsTab,
    chart_context_menu: Option<ChartContextMenuState>,
    drawings: DrawingStore,
    active_drawing_tool: Option<DrawingTool>,
    measure_active: bool,
    measure_state: Option<MeasureState>,
    drawing_tool_policy: DrawingToolPolicy,
    drawing_tool_templates: DrawingToolTemplates,
    fib_presets: FibPresetCollection,
    fib_preset_name_input: String,
    chart_presets: ChartPresetCollection,
    chart_preset_name_input: String,
    drawing_document_cache: BTreeMap<String, DrawingStore>,
    drawing_tool_policy_menu_open: bool,
    app_undo_stack: Vec<AppUndoSnapshot>,
    app_redo_stack: Vec<AppUndoSnapshot>,
    preserve_app_history_on_load_request_id: Option<u64>,
    storage_paths: Option<StoragePaths>,
    drawing_magnet_mode: DrawingMagnetMode,
    drawing_magnet_menu_open: bool,
    drawing_magnet_hold_started_at: Option<f64>,
    drawing_magnet_hold_opened: bool,
    drawing_creation: Option<DrawingCreationState>,
    drawing_drag: Option<DrawingDragState>,
    selected_drawing_id: Option<DrawingId>,
    selected_drawing_toolbar_position: Option<egui::Pos2>,
    selected_drawing_toolbar_drag_start: Option<egui::Pos2>,
    fib_settings_drawing_id: Option<DrawingId>,
    fib_settings_panel_rect: Option<egui::Rect>,
    position_settings_drawing_id: Option<DrawingId>,
    position_settings_panel_rect: Option<egui::Rect>,
    hovered_drawing_id: Option<DrawingId>,
    hovered_drawing_hit: Option<DrawingHit>,
    drawing_clipboard: Option<DrawingSpec>,
    text_editing: Option<DrawingTextEditState>,
    selected_study_instance_id: Option<StudyId>,
    study_settings_instance_id: Option<StudyId>,
    study_settings_tab: StudySettingsTab,
    market_data_service: MarketDataService,
    active_data_key: Option<MarketDataKey>,
    bar_store: BarStore,
    source_label: String,
    search_input: String,
    search_open: bool,
    focus_search: bool,
    select_search_on_focus: bool,
    search_palette_rect: Option<egui::Rect>,
    search_source_filter: SearchSourceFilter,
    search_type_filter: SearchTypeFilter,
    search_results_query: String,
    search_results: Vec<SearchCandidate>,
    search_last_edited: Option<Instant>,
    timeframe_input_open: bool,
    timeframe_input: String,
    focus_timeframe_input: bool,
    timeframe_input_rect: Option<egui::Rect>,
    market_source: MarketSource,
    status: String,
    status_is_error: bool,
    loading: bool,
    selected_timeframe: BarInterval,
    request_id: u64,
    sender: Sender<MarketEvent>,
    receiver: Receiver<MarketEvent>,
    yfinance_feed: Arc<YFinanceFeed>,
    massive_feed: Arc<MassiveFeed>,
    binance_feed: Arc<BinanceFeed>,
    coingecko_feed: Arc<CoinGeckoFeed>,
    active_subscription: Option<SubscriptionId>,
    active_source: Option<MarketSource>,
    backfill_loading: bool,
    no_more_history: bool,
    last_backfill_to: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct AppLaunchOptions {
    csv_path: Option<String>,
    open_widget: Option<WidgetKind>,
    reset_workspace: bool,
}

fn parse_app_launch_options(args: &[String]) -> Result<AppLaunchOptions, String> {
    let mut options = AppLaunchOptions::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--open-widget" => {
                let value = required_cli_value(args, &mut index)?;
                let Some(kind) = WidgetKind::from_stable_key(&value) else {
                    return Err(format!(
                        "unknown widget stable key for --open-widget: {value}"
                    ));
                };
                options.open_widget = Some(kind);
            }
            "--reset-workspace" => {
                options.reset_workspace = true;
                index += 1;
            }
            value if value.starts_with('-') => {
                return Err(format!("unknown app launch option: {value}"));
            }
            value => {
                if options.csv_path.is_some() {
                    return Err("only one positional CSV path is supported".to_string());
                }
                options.csv_path = Some(value.to_string());
                index += 1;
            }
        }
    }
    Ok(options)
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct StudyInstance {
    id: StudyId,
    visible: bool,
    config: StudyConfig,
    inputs: StudyInputConfig,
    style: StudyStyleConfig,
    visibility: StudyVisibilityConfig,
}

impl StudyInstance {
    fn new(id: StudyId, config: StudyConfig) -> Self {
        let inputs = StudyInputConfig::for_config(&config);
        let style = StudyStyleConfig::for_config(&config);
        let visibility = StudyVisibilityConfig::default();
        Self {
            id,
            visible: true,
            config,
            inputs,
            style,
            visibility,
        }
    }

    fn pane(&self) -> PaneId {
        match &self.config {
            StudyConfig::Sma { .. } | StudyConfig::Ema { .. } => PaneId::Main,
            StudyConfig::Rsi { .. } | StudyConfig::Macd { .. } => PaneId::Study(self.id.0),
        }
    }

    fn display_name(&self) -> String {
        self.config.display_name(&self.inputs)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StudyLegendEntry {
    id: StudyId,
    pane: PaneId,
    label: String,
    visible: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct StudyInstanceRowAction {
    changed: bool,
    settings: bool,
    remove: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StudyLegendAction {
    Settings(StudyId),
    ToggleVisibility(StudyId),
    Delete(StudyId),
    More(StudyId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StudyLegendIcon {
    Settings,
    Visibility,
    Delete,
    More,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StudySettingsTab {
    Inputs,
    Style,
    Visibility,
}

impl Default for StudySettingsTab {
    fn default() -> Self {
        Self::Inputs
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChartSettingsTab {
    Symbol,
    Appearance,
    Scales,
}

impl Default for ChartSettingsTab {
    fn default() -> Self {
        Self::Symbol
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ChartContextMenuState {
    position: egui::Pos2,
    target: ChartContextTarget,
    plot_width: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChartContextTarget {
    Drawing(DrawingId),
    Plot,
    PriceScale,
    TimeScale,
    Legend,
    Pane(PaneId),
    Chart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChartContextMenuAction {
    ChartSettings,
    Indicators,
    ResetPriceScale,
    FitContent,
    Drawing(DrawingId, DrawingAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrawingAction {
    EditText,
    OpenSettings,
    ToggleLock,
    Hide,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrawingActionEffect {
    EditText,
    OpenSettings,
    LockChanged(bool),
    Hidden,
    Deleted,
    Noop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrawingObjectTreeAction {
    Select(DrawingId),
    ToggleVisible(DrawingId),
    ToggleLock(DrawingId),
    Delete(DrawingId),
    BringForward(DrawingId),
    SendBackward(DrawingId),
    BringToFront(DrawingId),
    SendToBack(DrawingId),
    GroupWithSelected(DrawingId),
    Ungroup(DrawingGroupId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DrawingObjectTreeEntry {
    id: DrawingId,
    pane: PaneId,
    label: String,
    visible: bool,
    locked: bool,
    selectable: bool,
    group_id: Option<DrawingGroupId>,
    z_order: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrawingClipboardShortcut {
    Copy(DrawingId),
    Paste,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChartPriceModeChoice {
    Regular,
    Percent,
    Indexed,
    Logarithmic,
    Inverted,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum StudyConfig {
    Sma {
        period: usize,
    },
    Ema {
        period: usize,
    },
    Rsi {
        period: usize,
        upper_band: usize,
        middle_band: usize,
        lower_band: usize,
    },
    Macd {
        fast_period: usize,
        slow_period: usize,
        signal_period: usize,
    },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
enum StudyInputConfig {
    Sma {
        source: StudyPriceSource,
        offset: i32,
        calculation: StudyCalculationConfig,
    },
    Ema {
        source: StudyPriceSource,
        offset: i32,
        calculation: StudyCalculationConfig,
    },
    Rsi {
        source: StudyPriceSource,
        calculate_divergence: bool,
        smoothing: StudyRsiSmoothingConfig,
        calculation: StudyCalculationConfig,
    },
    Macd {
        source: StudyPriceSource,
        calculation: StudyCalculationConfig,
    },
}

impl StudyInputConfig {
    fn for_config(config: &StudyConfig) -> Self {
        let calculation = StudyCalculationConfig::default();
        match config {
            StudyConfig::Sma { .. } => Self::Sma {
                source: StudyPriceSource::Close,
                offset: 0,
                calculation,
            },
            StudyConfig::Ema { .. } => Self::Ema {
                source: StudyPriceSource::Close,
                offset: 0,
                calculation,
            },
            StudyConfig::Rsi { .. } => Self::Rsi {
                source: StudyPriceSource::Close,
                calculate_divergence: false,
                smoothing: StudyRsiSmoothingConfig::default(),
                calculation,
            },
            StudyConfig::Macd { .. } => Self::Macd {
                source: StudyPriceSource::Close,
                calculation,
            },
        }
    }

    fn source(&self) -> StudyPriceSource {
        match self {
            Self::Sma { source, .. }
            | Self::Ema { source, .. }
            | Self::Rsi { source, .. }
            | Self::Macd { source, .. } => *source,
        }
    }

    fn output_offset(&self, output: u32) -> i32 {
        match self {
            Self::Sma { offset, .. } | Self::Ema { offset, .. } if output == 0 => *offset,
            _ => 0,
        }
    }

    fn normalize(&mut self) {
        match self {
            Self::Sma { offset, .. } | Self::Ema { offset, .. } => {
                *offset = (*offset).clamp(MIN_STUDY_OFFSET, MAX_STUDY_OFFSET);
            }
            Self::Rsi { smoothing, .. } => smoothing.normalize(),
            Self::Macd { .. } => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct StudyCalculationConfig {
    timeframe: StudyCalculationTimeframe,
    wait_for_timeframe_close: bool,
}

impl Default for StudyCalculationConfig {
    fn default() -> Self {
        Self {
            timeframe: StudyCalculationTimeframe::Chart,
            wait_for_timeframe_close: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum StudyCalculationTimeframe {
    Chart,
}

impl StudyCalculationTimeframe {
    const fn label(self) -> &'static str {
        match self {
            Self::Chart => "Chart",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
struct StudyRsiSmoothingConfig {
    kind: StudySmoothingKind,
    length: usize,
}

impl Default for StudyRsiSmoothingConfig {
    fn default() -> Self {
        Self {
            kind: StudySmoothingKind::Sma,
            length: DEFAULT_RSI_PERIOD,
        }
    }
}

impl StudyRsiSmoothingConfig {
    fn normalize(&mut self) {
        self.length = clamp_study_period(self.length);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct StudyVisibilityConfig {
    ticks: bool,
    seconds: StudyVisibilityRange,
    minutes: StudyVisibilityRange,
    hours: StudyVisibilityRange,
    days: StudyVisibilityRange,
    weeks: StudyVisibilityRange,
    months: StudyVisibilityRange,
    ranges: bool,
}

impl Default for StudyVisibilityConfig {
    fn default() -> Self {
        Self {
            ticks: true,
            seconds: StudyVisibilityRange::new(true, 1, 59),
            minutes: StudyVisibilityRange::new(true, 1, 59),
            hours: StudyVisibilityRange::new(true, 1, 24),
            days: StudyVisibilityRange::new(true, 1, 366),
            weeks: StudyVisibilityRange::new(true, 1, 52),
            months: StudyVisibilityRange::new(true, 1, 12),
            ranges: true,
        }
    }
}

impl StudyVisibilityConfig {
    fn allows_interval(&self, interval: BarInterval) -> bool {
        match Resolution::from(interval) {
            Resolution::Ticks(_) => self.ticks,
            Resolution::Seconds(value) => self.seconds.allows(value),
            Resolution::Minutes(value) => self.minutes.allows(value),
            Resolution::Hours(value) => self.hours.allows(value),
            Resolution::Days(value) => self.days.allows(value),
            Resolution::Weeks(value) => self.weeks.allows(value),
            Resolution::Months(value) => self.months.allows(value),
        }
    }

    fn normalize(&mut self) {
        self.seconds.normalize();
        self.minutes.normalize();
        self.hours.normalize();
        self.days.normalize();
        self.weeks.normalize();
        self.months.normalize();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct StudyVisibilityRange {
    enabled: bool,
    min: u32,
    max: u32,
}

impl StudyVisibilityRange {
    const fn new(enabled: bool, min: u32, max: u32) -> Self {
        Self { enabled, min, max }
    }

    fn allows(self, value: u32) -> bool {
        self.enabled && (self.min..=self.max).contains(&value)
    }

    fn normalize(&mut self) {
        self.min = self.min.max(1);
        self.max = self.max.max(self.min);
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
enum StudyStyleConfig {
    Sma {
        line: StudyPlotStyle,
    },
    Ema {
        line: StudyPlotStyle,
    },
    Rsi {
        line: StudyPlotStyle,
        smoothing: StudyPlotStyle,
        upper_band: StudyPlotStyle,
        middle_band: StudyPlotStyle,
        lower_band: StudyPlotStyle,
    },
    Macd {
        macd: StudyPlotStyle,
        signal: StudyPlotStyle,
        histogram: StudyHistogramStyle,
    },
}

impl StudyStyleConfig {
    fn for_config(config: &StudyConfig) -> Self {
        match config {
            StudyConfig::Sma { .. } => Self::Sma {
                line: StudyPlotStyle::new(true, StudyColor::rgb(68, 152, 255), 1.5),
            },
            StudyConfig::Ema { .. } => Self::Ema {
                line: StudyPlotStyle::new(true, StudyColor::rgb(41, 98, 255), 1.5),
            },
            StudyConfig::Rsi { .. } => Self::Rsi {
                line: StudyPlotStyle::new(true, StudyColor::rgb(126, 87, 194), 1.5),
                smoothing: StudyPlotStyle::new(true, StudyColor::rgb(255, 214, 48), 1.5),
                upper_band: StudyPlotStyle::new(true, StudyColor::rgba(132, 145, 158, 150), 1.0),
                middle_band: StudyPlotStyle::new(true, StudyColor::rgba(132, 145, 158, 85), 1.0),
                lower_band: StudyPlotStyle::new(true, StudyColor::rgba(132, 145, 158, 150), 1.0),
            },
            StudyConfig::Macd { .. } => Self::Macd {
                macd: StudyPlotStyle::new(true, StudyColor::rgb(41, 98, 255), 1.5),
                signal: StudyPlotStyle::new(true, StudyColor::rgb(255, 214, 48), 1.5),
                histogram: StudyHistogramStyle::new(
                    true,
                    StudyColor::rgba(76, 175, 80, 140),
                    StudyColor::rgba(242, 54, 69, 140),
                ),
            },
        }
    }

    fn output_style(&self, output: u32) -> Option<StudyOutputStyle> {
        match self {
            Self::Sma { line } | Self::Ema { line } => {
                (output == 0).then(|| line.to_output_style())
            }
            Self::Rsi {
                line, smoothing, ..
            } => match output {
                0 => Some(line.to_output_style()),
                1 => Some(smoothing.to_output_style()),
                _ => None,
            },
            Self::Macd {
                macd,
                signal,
                histogram,
            } => match output {
                0 => Some(macd.to_output_style()),
                1 => Some(signal.to_output_style()),
                2 => Some(histogram.to_output_style()),
                _ => None,
            },
        }
    }

    fn normalize(&mut self) {
        match self {
            Self::Sma { line } | Self::Ema { line } => {
                line.normalize();
            }
            Self::Rsi {
                line,
                smoothing,
                upper_band,
                middle_band,
                lower_band,
            } => {
                line.normalize();
                smoothing.normalize();
                upper_band.normalize();
                middle_band.normalize();
                lower_band.normalize();
            }
            Self::Macd {
                macd,
                signal,
                histogram: _,
            } => {
                macd.normalize();
                signal.normalize();
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
struct StudyPlotStyle {
    visible: bool,
    color: StudyColor,
    width: f32,
}

impl StudyPlotStyle {
    const fn new(visible: bool, color: StudyColor, width: f32) -> Self {
        Self {
            visible,
            color,
            width,
        }
    }

    const fn to_output_style(self) -> StudyOutputStyle {
        StudyOutputStyle::line(self.visible, self.color, self.width)
    }

    const fn to_guide_style(self, level: f64) -> StudyGuideStyle {
        StudyGuideStyle::new(level, self.visible, self.color, self.width)
    }

    fn normalize(&mut self) {
        self.width = if self.width.is_finite() {
            self.width
                .clamp(MIN_STUDY_STYLE_WIDTH, MAX_STUDY_STYLE_WIDTH)
        } else {
            1.0
        };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
struct StudyHistogramStyle {
    visible: bool,
    positive_color: StudyColor,
    negative_color: StudyColor,
}

impl StudyHistogramStyle {
    const fn new(visible: bool, positive_color: StudyColor, negative_color: StudyColor) -> Self {
        Self {
            visible,
            positive_color,
            negative_color,
        }
    }

    const fn to_output_style(self) -> StudyOutputStyle {
        StudyOutputStyle::histogram(self.visible, self.positive_color, self.negative_color)
    }
}

impl StudyConfig {
    fn display_name(&self, inputs: &StudyInputConfig) -> String {
        let source = inputs.source().label().to_ascii_lowercase();
        match self {
            Self::Sma { period } => format!("SMA {period} {source}"),
            Self::Ema { period } => format!("EMA {period} {source}"),
            Self::Rsi { period, .. } => format!("RSI {period} {source}"),
            Self::Macd {
                fast_period,
                slow_period,
                signal_period,
            } => format!("MACD {fast_period} {slow_period} {source} {signal_period}"),
        }
    }

    const fn kind_label(&self) -> &'static str {
        match self {
            Self::Sma { .. } => "SMA",
            Self::Ema { .. } => "EMA",
            Self::Rsi { .. } => "RSI",
            Self::Macd { .. } => "MACD",
        }
    }

    const fn matches_inputs(&self, inputs: &StudyInputConfig) -> bool {
        matches!(
            (self, inputs),
            (Self::Sma { .. }, StudyInputConfig::Sma { .. })
                | (Self::Ema { .. }, StudyInputConfig::Ema { .. })
                | (Self::Rsi { .. }, StudyInputConfig::Rsi { .. })
                | (Self::Macd { .. }, StudyInputConfig::Macd { .. })
        )
    }

    const fn matches_style(&self, style: &StudyStyleConfig) -> bool {
        matches!(
            (self, style),
            (Self::Sma { .. }, StudyStyleConfig::Sma { .. })
                | (Self::Ema { .. }, StudyStyleConfig::Ema { .. })
                | (Self::Rsi { .. }, StudyStyleConfig::Rsi { .. })
                | (Self::Macd { .. }, StudyStyleConfig::Macd { .. })
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AppStatusMessage {
    text: String,
    is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UserSettingsState {
    drawing_tool_policy: DrawingToolPolicy,
    volume_visible: bool,
    status: Option<AppStatusMessage>,
}

struct DrawingDocumentState {
    drawings: DrawingStore,
    status: Option<AppStatusMessage>,
}

#[derive(Debug, Clone, PartialEq)]
struct DrawingToolTemplatesState {
    templates: DrawingToolTemplates,
    status: Option<AppStatusMessage>,
}

#[derive(Debug, Clone, PartialEq)]
struct FibPresetsState {
    presets: FibPresetCollection,
    status: Option<AppStatusMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChartPresetsState {
    presets: ChartPresetCollection,
    status: Option<AppStatusMessage>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct LayoutDocument {
    schema_version: u32,
    layout_id: String,
    study_instances: Vec<StudyInstance>,
}

impl LayoutDocument {
    fn from_study_instances(study_instances: &[StudyInstance]) -> Self {
        let mut study_instances = study_instances.to_vec();
        normalize_persisted_study_instances(&mut study_instances);
        Self {
            schema_version: persistence::LAYOUT_DOCUMENT_SCHEMA_VERSION,
            layout_id: persistence::DEFAULT_LAYOUT_ID.to_string(),
            study_instances,
        }
    }

    fn validate_schema(&self) -> Result<(), PersistenceSchemaError> {
        if self.schema_version == persistence::LAYOUT_DOCUMENT_SCHEMA_VERSION {
            Ok(())
        } else {
            Err(PersistenceSchemaError::UnsupportedVersion {
                document: "layout-document",
                found: self.schema_version,
                expected: persistence::LAYOUT_DOCUMENT_SCHEMA_VERSION,
            })
        }
    }
}

impl PersistenceDocument for LayoutDocument {
    fn validate_schema(&self) -> Result<(), PersistenceSchemaError> {
        LayoutDocument::validate_schema(self)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct LayoutDocumentState {
    study_instances: Vec<StudyInstance>,
    status: Option<AppStatusMessage>,
}

#[derive(Debug, Clone, PartialEq)]
struct WorkspaceLayoutState {
    restore_input: Option<workspace_shell::WorkspaceLayoutRestoreInput>,
    status: Option<AppStatusMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LiveChartSessionRestore {
    source: MarketSource,
    symbol: String,
    selected_timeframe: BarInterval,
}

#[derive(Debug, Clone, PartialEq)]
struct LiveChartSessionsState {
    restores: BTreeMap<WidgetId, LiveChartSessionRestore>,
    status: Option<AppStatusMessage>,
}

fn load_user_settings_state(
    paths: &StoragePaths,
    default_policy: DrawingToolPolicy,
    default_volume_visible: bool,
) -> UserSettingsState {
    let path = paths.user_settings_path();
    match read_json_document::<UserSettingsDocument>(&path) {
        Ok(document) => UserSettingsState {
            drawing_tool_policy: normalize_drawing_tool_policy(
                document.drawing_tool_policy.to_policy(),
            ),
            volume_visible: document.volume_visible,
            status: None,
        },
        Err(PersistenceError::MissingFile { .. }) => UserSettingsState {
            drawing_tool_policy: default_policy,
            volume_visible: default_volume_visible,
            status: None,
        },
        Err(error) => UserSettingsState {
            drawing_tool_policy: default_policy,
            volume_visible: default_volume_visible,
            status: Some(AppStatusMessage {
                text: format!("User settings could not be loaded: {error}"),
                is_error: true,
            }),
        },
    }
}

fn save_user_settings_state(
    paths: &StoragePaths,
    policy: &DrawingToolPolicy,
    volume_visible: bool,
) -> Result<(), PersistenceError> {
    let document = UserSettingsDocument::from_drawing_tool_policy(policy, volume_visible);
    write_json_document_atomic(paths.user_settings_path(), &document)
}

fn load_drawing_tool_templates_state(paths: &StoragePaths) -> DrawingToolTemplatesState {
    let path = paths.drawing_tool_templates_path();
    match read_json_document::<DrawingToolTemplatesDocument>(&path) {
        Ok(document) => DrawingToolTemplatesState {
            templates: DrawingToolTemplates::from_document(document),
            status: None,
        },
        Err(PersistenceError::MissingFile { .. }) => DrawingToolTemplatesState {
            templates: DrawingToolTemplates::default(),
            status: None,
        },
        Err(error) => DrawingToolTemplatesState {
            templates: DrawingToolTemplates::default(),
            status: Some(AppStatusMessage {
                text: format!("Drawing templates could not be loaded: {error}"),
                is_error: true,
            }),
        },
    }
}

fn save_drawing_tool_templates_state(
    paths: &StoragePaths,
    templates: &DrawingToolTemplates,
) -> Result<(), PersistenceError> {
    write_json_document_atomic(
        paths.drawing_tool_templates_path(),
        &templates.to_document(),
    )
}

fn load_fib_presets_state(paths: &StoragePaths) -> FibPresetsState {
    let path = paths.fib_presets_path();
    match read_json_document::<FibPresetDocument>(&path) {
        Ok(document) => FibPresetsState {
            presets: FibPresetCollection::from_document(document),
            status: None,
        },
        Err(PersistenceError::MissingFile { .. }) => FibPresetsState {
            presets: FibPresetCollection::default(),
            status: None,
        },
        Err(error) => FibPresetsState {
            presets: FibPresetCollection::default(),
            status: Some(AppStatusMessage {
                text: format!("Fib presets could not be loaded: {error}"),
                is_error: true,
            }),
        },
    }
}

fn save_fib_presets_state(
    paths: &StoragePaths,
    presets: &FibPresetCollection,
) -> Result<(), PersistenceError> {
    write_json_document_atomic(paths.fib_presets_path(), &presets.to_document())
}

fn load_chart_presets_state(paths: &StoragePaths) -> ChartPresetsState {
    let path = paths.chart_presets_path();
    match read_json_document::<ChartPresetDocument>(&path) {
        Ok(document) => ChartPresetsState {
            presets: ChartPresetCollection::from_document(document),
            status: None,
        },
        Err(PersistenceError::MissingFile { .. }) => ChartPresetsState {
            presets: ChartPresetCollection::default(),
            status: None,
        },
        Err(error) => ChartPresetsState {
            presets: ChartPresetCollection::default(),
            status: Some(AppStatusMessage {
                text: format!("Chart presets could not be loaded: {error}"),
                is_error: true,
            }),
        },
    }
}

fn save_chart_presets_state(
    paths: &StoragePaths,
    presets: &ChartPresetCollection,
) -> Result<(), PersistenceError> {
    write_json_document_atomic(paths.chart_presets_path(), &presets.to_document())
}

fn load_layout_state(
    paths: &StoragePaths,
    mut default_study_instances: Vec<StudyInstance>,
) -> LayoutDocumentState {
    normalize_persisted_study_instances(&mut default_study_instances);
    let path = paths.default_layout_path();
    match read_json_document::<LayoutDocument>(&path) {
        Ok(mut document) if document.layout_id == persistence::DEFAULT_LAYOUT_ID => {
            normalize_persisted_study_instances(&mut document.study_instances);
            LayoutDocumentState {
                study_instances: document.study_instances,
                status: None,
            }
        }
        Ok(document) => LayoutDocumentState {
            study_instances: default_study_instances,
            status: Some(AppStatusMessage {
                text: format!(
                    "Layout document id mismatch: expected {} but found {}",
                    persistence::DEFAULT_LAYOUT_ID,
                    document.layout_id
                ),
                is_error: true,
            }),
        },
        Err(PersistenceError::MissingFile { .. }) => LayoutDocumentState {
            study_instances: default_study_instances,
            status: None,
        },
        Err(error) => LayoutDocumentState {
            study_instances: default_study_instances,
            status: Some(AppStatusMessage {
                text: format!("Layout document could not be loaded: {error}"),
                is_error: true,
            }),
        },
    }
}

fn save_layout_state(
    paths: &StoragePaths,
    study_instances: &[StudyInstance],
) -> Result<(), PersistenceError> {
    let document = LayoutDocument::from_study_instances(study_instances);
    write_json_document_atomic(paths.default_layout_path(), &document)
}

fn load_workspace_layout_state(paths: &StoragePaths) -> WorkspaceLayoutState {
    let path = paths.workspace_layout_path();
    match read_json_document::<WorkspaceLayoutDocument>(&path) {
        Ok(document) if document.layout_id == persistence::DEFAULT_WORKSPACE_LAYOUT_ID => {
            match document.to_restore_input() {
                Ok(restore_input) => WorkspaceLayoutState {
                    restore_input: Some(restore_input),
                    status: None,
                },
                Err(error) => WorkspaceLayoutState {
                    restore_input: None,
                    status: Some(AppStatusMessage {
                        text: format!("Workspace layout could not be restored: {error}"),
                        is_error: true,
                    }),
                },
            }
        }
        Ok(document) => WorkspaceLayoutState {
            restore_input: None,
            status: Some(AppStatusMessage {
                text: format!(
                    "Workspace layout id mismatch: expected {} but found {}",
                    persistence::DEFAULT_WORKSPACE_LAYOUT_ID,
                    document.layout_id
                ),
                is_error: true,
            }),
        },
        Err(PersistenceError::MissingFile { .. }) => WorkspaceLayoutState {
            restore_input: None,
            status: None,
        },
        Err(error) => WorkspaceLayoutState {
            restore_input: None,
            status: Some(AppStatusMessage {
                text: format!("Workspace layout could not be loaded: {error}"),
                is_error: true,
            }),
        },
    }
}

fn save_workspace_layout_state(
    paths: &StoragePaths,
    layout: &workspace_shell::WorkspaceLayout,
) -> Result<(), PersistenceError> {
    let document = WorkspaceLayoutDocument::from_layout(layout);
    write_json_document_atomic(paths.workspace_layout_path(), &document)
}

fn reset_workspace_layout_state(paths: &StoragePaths) -> Result<(), PersistenceError> {
    let layout = workspace_shell::WorkspaceLayout {
        next_widget_id: 1,
        next_z_order: 1,
        active_widget_id: None,
        widgets: Vec::new(),
    };
    save_workspace_layout_state(paths, &layout)
}

fn load_live_chart_sessions_state(paths: &StoragePaths) -> LiveChartSessionsState {
    let path = paths.live_chart_sessions_path();
    match read_json_document::<LiveChartSessionDocument>(&path) {
        Ok(document) => {
            let mut restores = BTreeMap::new();
            for entry in document.sessions {
                let Some(restore) = live_chart_session_restore_from_entry(&entry) else {
                    return LiveChartSessionsState {
                        restores: BTreeMap::new(),
                        status: Some(AppStatusMessage {
                            text: format!(
                                "Live Chart session could not be restored for widget {}",
                                entry.widget_id
                            ),
                            is_error: true,
                        }),
                    };
                };
                restores.insert(WidgetId::new(entry.widget_id), restore);
            }
            LiveChartSessionsState {
                restores,
                status: None,
            }
        }
        Err(PersistenceError::MissingFile { .. }) => LiveChartSessionsState {
            restores: BTreeMap::new(),
            status: None,
        },
        Err(error) => LiveChartSessionsState {
            restores: BTreeMap::new(),
            status: Some(AppStatusMessage {
                text: format!("Live Chart sessions could not be loaded: {error}"),
                is_error: true,
            }),
        },
    }
}

fn save_live_chart_sessions_state(
    paths: &StoragePaths,
    sessions: Vec<LiveChartSessionDocumentEntry>,
) -> Result<(), PersistenceError> {
    let document = LiveChartSessionDocument::from_entries(sessions);
    write_json_document_atomic(paths.live_chart_sessions_path(), &document)
}

fn live_chart_session_restore_from_entry(
    entry: &LiveChartSessionDocumentEntry,
) -> Option<LiveChartSessionRestore> {
    let source = MarketSource::from_stable_key(&entry.source)?;
    let selected_timeframe = BarInterval::from_label(&entry.timeframe)?;
    let symbol = normalized_symbol(source, &entry.symbol);
    (!symbol.is_empty()).then_some(LiveChartSessionRestore {
        source,
        symbol,
        selected_timeframe,
    })
}

fn live_chart_session_entry(
    widget_id: WidgetId,
    source: MarketSource,
    symbol: &str,
    selected_timeframe: BarInterval,
) -> LiveChartSessionDocumentEntry {
    LiveChartSessionDocumentEntry {
        widget_id: widget_id.get(),
        source: source.stable_key().to_string(),
        symbol: normalized_symbol(source, symbol),
        timeframe: selected_timeframe.label().to_string(),
    }
}

fn load_drawing_document_state(
    paths: &StoragePaths,
    scope: &DrawingDocumentScope,
) -> DrawingDocumentState {
    let path = match paths.drawing_document_path(scope) {
        Ok(path) => path,
        Err(error) => {
            return DrawingDocumentState {
                drawings: DrawingStore::new(),
                status: Some(AppStatusMessage {
                    text: format!("Drawing storage path could not be built: {error}"),
                    is_error: true,
                }),
            };
        }
    };
    match read_json_document::<DrawingDocument>(&path) {
        Ok(document) if document.scope == *scope => {
            match DrawingStore::from_persisted_parts(
                document.drawings,
                document.groups,
                document.next_drawing_id,
                document.next_group_id,
            ) {
                Ok(drawings) => DrawingDocumentState {
                    drawings,
                    status: None,
                },
                Err(error) => DrawingDocumentState {
                    drawings: DrawingStore::new(),
                    status: Some(AppStatusMessage {
                        text: format!("Drawing document could not be loaded: {error:?}"),
                        is_error: true,
                    }),
                },
            }
        }
        Ok(document) => DrawingDocumentState {
            drawings: DrawingStore::new(),
            status: Some(AppStatusMessage {
                text: format!(
                    "Drawing document scope mismatch: expected {} but found {}",
                    scope.symbol_key, document.scope.symbol_key
                ),
                is_error: true,
            }),
        },
        Err(PersistenceError::MissingFile { .. }) => DrawingDocumentState {
            drawings: DrawingStore::new(),
            status: None,
        },
        Err(error) => DrawingDocumentState {
            drawings: DrawingStore::new(),
            status: Some(AppStatusMessage {
                text: format!("Drawing document could not be loaded: {error}"),
                is_error: true,
            }),
        },
    }
}

fn save_drawing_document_state(
    paths: &StoragePaths,
    scope: DrawingDocumentScope,
    drawings: &DrawingStore,
) -> Result<(), PersistenceError> {
    let path = paths.drawing_document_path(&scope)?;
    let symbol_key = scope.symbol_key.clone();
    let document = DrawingDocument {
        schema_version: persistence::DRAWING_DOCUMENT_SCHEMA_VERSION,
        scope,
        next_drawing_id: drawings.next_drawing_id(),
        next_group_id: drawings.next_group_id(),
        drawings: drawings
            .drawings()
            .iter()
            .filter(|drawing| drawing_matches_symbol_key(drawing, &symbol_key))
            .cloned()
            .collect(),
        groups: drawings
            .groups()
            .iter()
            .filter(|group| group.symbol.as_deref() == Some(symbol_key.as_str()))
            .cloned()
            .collect(),
    };
    write_json_document_atomic(path, &document)
}

fn drawing_document_scope(source: MarketSource, symbol: &str) -> DrawingDocumentScope {
    let symbol_key = drawing_symbol_key(source, symbol);
    DrawingDocumentScope::default_chart(source.label(), symbol_key)
}

fn live_chart_drawing_cache_key(source: MarketSource, symbol: &str) -> String {
    let scope = drawing_document_scope(source, symbol);
    format!("{}:{}:{}", scope.source, scope.symbol_key, scope.scope_id)
}

fn merge_startup_status(
    status: &mut String,
    status_is_error: &mut bool,
    message: AppStatusMessage,
) {
    if *status_is_error {
        status.push_str("; ");
        status.push_str(&message.text);
    } else {
        *status = message.text;
    }
    *status_is_error = message.is_error;
}

fn normalize_drawing_tool_policy(policy: DrawingToolPolicy) -> DrawingToolPolicy {
    let favorites = DRAWING_TOOLS
        .into_iter()
        .filter(|tool| policy.favorites.contains(tool))
        .collect();
    let access_overrides = DRAWING_TOOLS
        .into_iter()
        .filter_map(|tool| {
            let access = policy.access(tool);
            (access != DrawingToolAccess::Available).then_some((tool, access))
        })
        .collect();
    DrawingToolPolicy {
        favorites,
        access_overrides,
    }
}

fn replace_chart_bars_from_service(
    service: &mut MarketDataService,
    chart_store: &mut BarStore,
    key: MarketDataKey,
    bars: Vec<OhlcBar>,
) -> MarketDataSnapshot {
    let snapshot = service.replace_all(key, bars.iter().copied());
    chart_store.replace_all(bars);
    debug_assert_chart_store_matches_snapshot(chart_store, &snapshot);
    snapshot
}

fn prepend_chart_history_from_service(
    service: &mut MarketDataService,
    chart_store: &mut BarStore,
    key: MarketDataKey,
    bars: Vec<OhlcBar>,
) -> (usize, MarketDataSnapshot) {
    let (added, snapshot) = service.prepend_history(key, bars.iter().copied());
    let chart_added = chart_store.prepend_history(bars);
    debug_assert_eq!(chart_added, added);
    debug_assert_chart_store_matches_snapshot(chart_store, &snapshot);
    (added, snapshot)
}

fn apply_chart_tail_from_service(
    service: &mut MarketDataService,
    chart_store: &mut BarStore,
    key: MarketDataKey,
    bar: OhlcBar,
) -> (TailUpdate, MarketDataSnapshot) {
    let (update, snapshot) = service.apply_tail(key, bar);
    let chart_update = chart_store.apply_tail(bar);
    debug_assert_eq!(chart_update, update);
    debug_assert_chart_store_matches_snapshot(chart_store, &snapshot);
    (update, snapshot)
}

fn debug_assert_chart_store_matches_snapshot(
    chart_store: &BarStore,
    snapshot: &MarketDataSnapshot,
) {
    debug_assert_eq!(chart_store.len(), snapshot.bar_count);
    debug_assert_eq!(chart_store.front().map(|bar| bar.time), snapshot.first_time);
    debug_assert_eq!(chart_store.back().map(|bar| bar.time), snapshot.last_time);
}

impl ChartOnlyApp {
    fn new(_cc: &eframe::CreationContext<'_>, launch_options: AppLaunchOptions) -> Self {
        let (sender, receiver) = mpsc::channel();
        let (bars, source_label, mut status, mut status_is_error) =
            match launch_options.csv_path.as_deref() {
                Some(path) => match load_csv(path) {
                    Ok(bars) if !bars.is_empty() => {
                        let label = Path::new(path)
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or(path)
                            .to_string();
                        let status = format!("Loaded {} bars from CSV", bars.len());
                        (bars, label, status, false)
                    }
                    Ok(_) => (
                        sample_bars(),
                        DEFAULT_SYMBOL.to_string(),
                        format!("{path} did not contain any valid OHLCV rows"),
                        true,
                    ),
                    Err(error) => (
                        sample_bars(),
                        DEFAULT_SYMBOL.to_string(),
                        format!("Could not load {path}: {error}"),
                        true,
                    ),
                },
                None => (
                    sample_bars(),
                    DEFAULT_SYMBOL.to_string(),
                    "Sample OHLCV data".to_string(),
                    false,
                ),
            };

        let mut bar_store = BarStore::unbounded();
        let initial_data_key =
            MarketDataKey::from_interval(MarketSource::YFinance, &source_label, DEFAULT_INTERVAL);
        let mut market_data_service = MarketDataService::new();
        replace_chart_bars_from_service(
            &mut market_data_service,
            &mut bar_store,
            initial_data_key.clone(),
            bars,
        );
        let mut chart = TvChartState::default();
        let mut chart_renderer = EguiChartRenderer::default();
        let storage_paths = match StoragePaths::resolve() {
            Ok(paths) => Some(paths),
            Err(error) => {
                merge_startup_status(
                    &mut status,
                    &mut status_is_error,
                    AppStatusMessage {
                        text: format!("Persistence storage is unavailable: {error}"),
                        is_error: true,
                    },
                );
                None
            }
        };
        let mut workspace_shell = WorkspaceShell::default();
        let mut live_chart_session_restores = BTreeMap::new();
        if let Some(paths) = &storage_paths {
            let workspace_layout_state = load_workspace_layout_state(paths);
            if let Some(restore_input) = workspace_layout_state.restore_input {
                workspace_shell.restore_workspace_layout(restore_input);
            }
            if let Some(workspace_status) = workspace_layout_state.status {
                merge_startup_status(&mut status, &mut status_is_error, workspace_status);
            }
            let live_chart_sessions_state = load_live_chart_sessions_state(paths);
            live_chart_session_restores = live_chart_sessions_state.restores;
            if let Some(session_status) = live_chart_sessions_state.status {
                merge_startup_status(&mut status, &mut status_is_error, session_status);
            }
        }
        if launch_options.reset_workspace {
            workspace_shell.reset_workspace_layout();
            live_chart_session_restores.clear();
        }
        let mut study_instances = default_study_instances();
        if let Some(paths) = &storage_paths {
            let layout_state = load_layout_state(paths, study_instances);
            study_instances = layout_state.study_instances;
            if let Some(layout_status) = layout_state.status {
                merge_startup_status(&mut status, &mut status_is_error, layout_status);
            }
        }
        let next_study_instance_id = next_study_instance_id(&study_instances);
        let mut study_runtime = build_study_runtime(&study_instances);
        apply_indicator_layout(&mut chart, &study_instances);
        study_runtime.register_output_series(&mut chart.series);
        study_runtime.replay(&bar_store);
        chart_renderer.set_study_generation(study_runtime.generation());
        let default_drawing_tool_policy = DrawingToolPolicy::default();
        let mut drawing_tool_policy = default_drawing_tool_policy.clone();
        if let Some(paths) = &storage_paths {
            let settings_state = load_user_settings_state(
                paths,
                default_drawing_tool_policy,
                chart.volume_visible(),
            );
            drawing_tool_policy = settings_state.drawing_tool_policy;
            chart.set_volume_visible(settings_state.volume_visible);
            if let Some(settings_status) = settings_state.status {
                merge_startup_status(&mut status, &mut status_is_error, settings_status);
            }
        }
        let mut chart_presets = ChartPresetCollection::default();
        if let Some(paths) = &storage_paths {
            let presets_state = load_chart_presets_state(paths);
            chart_presets = presets_state.presets;
            if let Some(active_snapshot) = chart_presets.active_snapshot() {
                if chart.set_theme(active_snapshot.theme) {
                    chart_renderer.set_theme_generation(chart.theme_generation());
                }
                chart.set_volume_visible(active_snapshot.volume_visible);
            }
            if let Some(presets_status) = presets_state.status {
                merge_startup_status(&mut status, &mut status_is_error, presets_status);
            }
        }
        let mut drawing_tool_templates = DrawingToolTemplates::default();
        if let Some(paths) = &storage_paths {
            let templates_state = load_drawing_tool_templates_state(paths);
            drawing_tool_templates = templates_state.templates;
            if let Some(templates_status) = templates_state.status {
                merge_startup_status(&mut status, &mut status_is_error, templates_status);
            }
        }
        let mut fib_presets = FibPresetCollection::default();
        if let Some(paths) = &storage_paths {
            let presets_state = load_fib_presets_state(paths);
            fib_presets = presets_state.presets;
            if let Some(presets_status) = presets_state.status {
                merge_startup_status(&mut status, &mut status_is_error, presets_status);
            }
        }
        let mut drawings = DrawingStore::new();
        if let Some(paths) = &storage_paths {
            let drawing_scope = drawing_document_scope(MarketSource::YFinance, &source_label);
            let drawing_state = load_drawing_document_state(paths, &drawing_scope);
            drawings = drawing_state.drawings;
            if let Some(drawing_status) = drawing_state.status {
                merge_startup_status(&mut status, &mut status_is_error, drawing_status);
            }
        }

        let mut app = Self {
            workspace_shell,
            latency_probe: LatencyProbe::default(),
            debug_latency_overlay_visible: false,
            live_chart_instances: BTreeMap::new(),
            live_chart_session_restores,
            order_flow_preview_service: OrderFlowPreviewService::default(),
            loaded_live_chart_widget_id: None,
            chart,
            chart_renderer,
            study_runtime,
            study_instances,
            next_study_instance_id,
            indicators_open: false,
            indicator_panel_rect: None,
            chart_settings_open: false,
            chart_settings_panel_rect: None,
            chart_snapshot_capture_rects: BTreeMap::new(),
            pending_chart_snapshots: BTreeMap::new(),
            object_tree_open: false,
            object_tree_panel_rect: None,
            chart_settings_tab: ChartSettingsTab::default(),
            chart_context_menu: None,
            drawings,
            active_drawing_tool: None,
            measure_active: false,
            measure_state: None,
            drawing_tool_policy,
            drawing_tool_templates,
            fib_presets,
            fib_preset_name_input: DEFAULT_FIB_PRESET_NAME.to_string(),
            chart_presets,
            chart_preset_name_input: DEFAULT_CHART_PRESET_NAME.to_string(),
            drawing_document_cache: BTreeMap::new(),
            drawing_tool_policy_menu_open: false,
            app_undo_stack: Vec::new(),
            app_redo_stack: Vec::new(),
            preserve_app_history_on_load_request_id: None,
            storage_paths,
            drawing_magnet_mode: DrawingMagnetMode::Off,
            drawing_magnet_menu_open: false,
            drawing_magnet_hold_started_at: None,
            drawing_magnet_hold_opened: false,
            drawing_creation: None,
            drawing_drag: None,
            selected_drawing_id: None,
            selected_drawing_toolbar_position: None,
            selected_drawing_toolbar_drag_start: None,
            fib_settings_drawing_id: None,
            fib_settings_panel_rect: None,
            position_settings_drawing_id: None,
            position_settings_panel_rect: None,
            hovered_drawing_id: None,
            hovered_drawing_hit: None,
            drawing_clipboard: None,
            text_editing: None,
            selected_study_instance_id: None,
            study_settings_instance_id: None,
            study_settings_tab: StudySettingsTab::Inputs,
            market_data_service,
            active_data_key: Some(initial_data_key),
            bar_store,
            source_label: source_label.clone(),
            search_input: source_label,
            search_open: false,
            focus_search: false,
            select_search_on_focus: false,
            search_palette_rect: None,
            search_source_filter: SearchSourceFilter::All,
            search_type_filter: SearchTypeFilter::All,
            search_results_query: String::new(),
            search_results: Vec::new(),
            search_last_edited: None,
            timeframe_input_open: false,
            timeframe_input: String::new(),
            focus_timeframe_input: false,
            timeframe_input_rect: None,
            market_source: MarketSource::YFinance,
            status,
            status_is_error,
            loading: false,
            selected_timeframe: DEFAULT_INTERVAL,
            request_id: 0,
            sender,
            receiver,
            yfinance_feed: Arc::new(YFinanceFeed),
            massive_feed: Arc::new(MassiveFeed),
            binance_feed: Arc::new(BinanceFeed::new()),
            coingecko_feed: Arc::new(CoinGeckoFeed),
            active_subscription: None,
            active_source: None,
            backfill_loading: false,
            no_more_history: false,
            last_backfill_to: None,
        };
        if let Some(kind) = launch_options.open_widget
            && let Some(action) = app.workspace_shell.spawn_widget(kind)
        {
            app.apply_shell_command_action(action);
        }
        app
    }

    fn create_live_chart_instance(&mut self, widget_id: WidgetId) {
        if self.loaded_live_chart_widget_id == Some(widget_id)
            || self.live_chart_instances.contains_key(&widget_id)
        {
            return;
        }

        let session_restore = self.live_chart_session_restores.remove(&widget_id);
        if session_restore.is_none()
            && self.loaded_live_chart_widget_id.is_none()
            && self.live_chart_instances.is_empty()
        {
            self.cache_current_drawing_document();
            let state = LiveChartInstanceState::take_from_app(self);
            self.live_chart_instances.insert(widget_id, state);
            return;
        }

        let state = self.default_live_chart_instance_state(session_restore);
        self.live_chart_instances.insert(widget_id, state);
    }

    fn default_live_chart_instance_state(
        &mut self,
        session_restore: Option<LiveChartSessionRestore>,
    ) -> LiveChartInstanceState {
        let source = session_restore
            .as_ref()
            .map(|restore| restore.source)
            .unwrap_or(MarketSource::YFinance);
        let symbol = session_restore
            .as_ref()
            .map(|restore| restore.symbol.clone())
            .unwrap_or_else(|| DEFAULT_SYMBOL.to_string());
        let selected_timeframe = session_restore
            .as_ref()
            .map(|restore| restore.selected_timeframe)
            .unwrap_or(DEFAULT_INTERVAL);
        let active_data_key = MarketDataKey::from_interval(source, &symbol, selected_timeframe);
        let mut bar_store = if let Some(store) = self.market_data_service.store(&active_data_key) {
            store.clone()
        } else {
            let bars = sample_bars();
            let mut store = BarStore::unbounded();
            replace_chart_bars_from_service(
                &mut self.market_data_service,
                &mut store,
                active_data_key.clone(),
                bars,
            );
            store
        };
        if bar_store.is_empty() {
            replace_chart_bars_from_service(
                &mut self.market_data_service,
                &mut bar_store,
                active_data_key.clone(),
                sample_bars(),
            );
        }

        let mut chart = TvChartState::default();
        let mut chart_renderer = EguiChartRenderer::default();
        if let Some(active_snapshot) = self.chart_presets.active_snapshot() {
            if chart.set_theme(active_snapshot.theme) {
                chart_renderer.set_theme_generation(chart.theme_generation());
            }
            chart.set_volume_visible(active_snapshot.volume_visible);
        }
        let mut study_instances = default_study_instances();
        if let Some(paths) = &self.storage_paths {
            study_instances = load_layout_state(paths, study_instances).study_instances;
        }
        let next_study_instance_id = next_study_instance_id(&study_instances);
        let mut study_runtime = build_study_runtime(&study_instances);
        apply_indicator_layout(&mut chart, &study_instances);
        study_runtime.register_output_series(&mut chart.series);
        study_runtime.replay(&bar_store);
        chart_renderer.set_study_generation(study_runtime.generation());

        LiveChartInstanceState {
            chart,
            chart_renderer,
            study_runtime,
            study_instances,
            next_study_instance_id,
            indicators_open: false,
            indicator_panel_rect: None,
            chart_settings_open: false,
            chart_settings_panel_rect: None,
            object_tree_open: false,
            object_tree_panel_rect: None,
            chart_settings_tab: ChartSettingsTab::default(),
            chart_context_menu: None,
            active_drawing_tool: None,
            measure_active: false,
            measure_state: None,
            drawing_tool_policy_menu_open: false,
            app_undo_stack: Vec::new(),
            app_redo_stack: Vec::new(),
            preserve_app_history_on_load_request_id: None,
            drawing_magnet_mode: DrawingMagnetMode::Off,
            drawing_magnet_menu_open: false,
            drawing_magnet_hold_started_at: None,
            drawing_magnet_hold_opened: false,
            drawing_creation: None,
            drawing_drag: None,
            selected_drawing_id: None,
            selected_drawing_toolbar_position: None,
            selected_drawing_toolbar_drag_start: None,
            fib_settings_drawing_id: None,
            fib_settings_panel_rect: None,
            position_settings_drawing_id: None,
            position_settings_panel_rect: None,
            hovered_drawing_id: None,
            hovered_drawing_hit: None,
            drawing_clipboard: None,
            text_editing: None,
            selected_study_instance_id: None,
            study_settings_instance_id: None,
            study_settings_tab: StudySettingsTab::Inputs,
            active_data_key: Some(active_data_key),
            bar_store,
            source_label: symbol.clone(),
            search_input: symbol,
            search_open: false,
            focus_search: false,
            select_search_on_focus: false,
            search_palette_rect: None,
            search_source_filter: SearchSourceFilter::All,
            search_type_filter: SearchTypeFilter::All,
            search_results_query: String::new(),
            search_results: Vec::new(),
            search_last_edited: None,
            timeframe_input_open: false,
            timeframe_input: String::new(),
            focus_timeframe_input: false,
            timeframe_input_rect: None,
            market_source: source,
            status: "Sample OHLCV data".to_string(),
            status_is_error: false,
            loading: false,
            selected_timeframe,
            request_id: 0,
            active_subscription: None,
            active_source: Some(source),
            backfill_loading: false,
            no_more_history: false,
            last_backfill_to: None,
            chart_preset_name_input: DEFAULT_CHART_PRESET_NAME.to_string(),
            fib_preset_name_input: DEFAULT_FIB_PRESET_NAME.to_string(),
        }
    }

    fn load_live_chart_instance(&mut self, widget_id: WidgetId) {
        if self.loaded_live_chart_widget_id == Some(widget_id) {
            return;
        }
        self.save_loaded_live_chart_instance();
        self.create_live_chart_instance(widget_id);
        if let Some(state) = self.live_chart_instances.remove(&widget_id) {
            state.put_into_app(self);
            self.loaded_live_chart_widget_id = Some(widget_id);
            self.restore_drawings_for_current_chart();
        }
    }

    fn save_loaded_live_chart_instance(&mut self) {
        let Some(widget_id) = self.loaded_live_chart_widget_id.take() else {
            return;
        };
        self.cache_current_drawing_document();
        let state = LiveChartInstanceState::take_from_app(self);
        self.live_chart_instances.insert(widget_id, state);
    }

    fn remove_live_chart_instance(&mut self, widget_id: WidgetId) {
        if self.loaded_live_chart_widget_id == Some(widget_id) {
            if let Some(id) = self.active_subscription.take() {
                self.binance_feed.unsubscribe_bars(id);
            }
            self.cache_current_drawing_document();
            self.loaded_live_chart_widget_id = None;
            let _ = LiveChartInstanceState::take_from_app(self);
        }
        if let Some(mut state) = self.live_chart_instances.remove(&widget_id)
            && let Some(id) = state.active_subscription.take()
        {
            self.binance_feed.unsubscribe_bars(id);
        }
        self.chart_snapshot_capture_rects.remove(&widget_id);
        self.pending_chart_snapshots.remove(&widget_id);
    }

    fn with_live_chart_instance<R>(
        &mut self,
        widget_id: WidgetId,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.load_live_chart_instance(widget_id);
        let result = f(self);
        self.save_loaded_live_chart_instance();
        result
    }

    fn current_live_chart_widget_id(&self) -> Option<WidgetId> {
        self.loaded_live_chart_widget_id
    }

    fn cache_current_drawing_document(&mut self) {
        if self.source_label.is_empty() {
            return;
        }
        let key = live_chart_drawing_cache_key(
            self.active_source.unwrap_or(self.market_source),
            &self.source_label,
        );
        self.drawing_document_cache
            .insert(key, self.drawings.clone());
    }

    fn restore_drawings_for_current_chart(&mut self) {
        if self.source_label.is_empty() {
            self.drawings = DrawingStore::new();
            return;
        }
        let source = self.active_source.unwrap_or(self.market_source);
        let key = live_chart_drawing_cache_key(source, &self.source_label);
        if let Some(drawings) = self.drawing_document_cache.get(&key).cloned() {
            self.drawings = drawings;
            return;
        }
        let Some(paths) = &self.storage_paths else {
            self.drawings = DrawingStore::new();
            self.drawing_document_cache
                .insert(key, self.drawings.clone());
            return;
        };
        let scope = drawing_document_scope(source, &self.source_label);
        let drawing_state = load_drawing_document_state(paths, &scope);
        self.drawings = drawing_state.drawings;
        if let Some(status) = drawing_state.status {
            self.status = status.text;
            self.status_is_error = status.is_error;
        }
        self.drawing_document_cache
            .insert(key, self.drawings.clone());
    }

    fn poll_market_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.receiver.try_recv() {
            match event {
                MarketEvent::Loaded {
                    widget_id,
                    request_id,
                    source,
                    symbol,
                    bars,
                } => self.with_live_chart_instance(widget_id, |app| {
                    if request_id != app.request_id {
                        return;
                    }
                    app.loading = false;
                    app.status_is_error = false;
                    app.source_label = symbol.clone();
                    app.search_input = symbol.clone();
                    app.search_results_query.clear();
                    app.search_results.clear();
                    app.selected_drawing_id = None;
                    app.hovered_drawing_id = None;
                    app.hovered_drawing_hit = None;
                    app.drawing_drag = None;
                    app.text_editing = None;
                    let data_key =
                        MarketDataKey::from_interval(source, &symbol, app.selected_timeframe);
                    let snapshot = replace_chart_bars_from_service(
                        &mut app.market_data_service,
                        &mut app.bar_store,
                        data_key.clone(),
                        bars,
                    );
                    app.active_data_key = Some(data_key);
                    app.latency_probe.record_data_ingest(Instant::now());
                    reset_chart_for_loaded_market_data(&mut app.chart, &app.study_instances);
                    app.study_runtime
                        .register_output_series(&mut app.chart.series);
                    let study_update = app.study_runtime.replay(&app.bar_store);
                    app.sync_study_generation(study_update);
                    app.chart_renderer.clear_cache();
                    app.active_source = Some(source);
                    app.backfill_loading = false;
                    app.no_more_history = false;
                    app.last_backfill_to = None;
                    let bar_count = snapshot.bar_count;

                    if source == MarketSource::Binance {
                        app.status = format!(
                            "Loaded {} {} bars from Binance; opening public WebSocket",
                            bar_count,
                            app.selected_timeframe.label()
                        );
                        app.start_market_subscription(
                            ctx,
                            widget_id,
                            request_id,
                            source,
                            symbol.clone(),
                            app.selected_timeframe,
                        );
                    } else {
                        app.status =
                            format_interval_status(source, app.selected_timeframe, bar_count);
                    }
                    let preserve_app_history =
                        app.preserve_app_history_on_load_request_id == Some(request_id);
                    app.preserve_app_history_on_load_request_id = None;
                    app.load_drawing_document(source, &symbol, preserve_app_history);
                }),
                MarketEvent::LoadFailed {
                    widget_id,
                    request_id,
                    symbol,
                    error,
                } => self.with_live_chart_instance(widget_id, |app| {
                    if request_id != app.request_id {
                        return;
                    }
                    if app.preserve_app_history_on_load_request_id == Some(request_id) {
                        app.preserve_app_history_on_load_request_id = None;
                    }
                    app.loading = false;
                    app.status_is_error = true;
                    app.status = format!("{symbol}: {error}");
                }),
                MarketEvent::LiveBar {
                    widget_id,
                    request_id,
                    bar,
                } => self.with_live_chart_instance(widget_id, |app| {
                    if request_id != app.request_id {
                        return;
                    }
                    let tail_update = if let Some(data_key) = app.active_data_key.clone() {
                        apply_chart_tail_from_service(
                            &mut app.market_data_service,
                            &mut app.bar_store,
                            data_key,
                            bar,
                        )
                        .0
                    } else {
                        app.bar_store.apply_tail(bar)
                    };
                    if matches!(tail_update, TailUpdate::Appended | TailUpdate::ReplacedTail) {
                        app.latency_probe.record_data_ingest(Instant::now());
                    }
                    let study_update = app.study_runtime.apply_tail(&app.bar_store, tail_update);
                    app.sync_study_generation(study_update);
                    app.status = format!(
                        "Streaming {} {} via Binance public WebSocket",
                        app.source_label,
                        app.selected_timeframe.label()
                    );
                    app.status_is_error = false;
                    app.loading = false;
                    app.chart.follow_realtime = true;
                }),
                MarketEvent::Backfilled {
                    widget_id,
                    request_id,
                    symbol,
                    to,
                    bars,
                    no_data,
                } => self.with_live_chart_instance(widget_id, |app| {
                    if request_id != app.request_id || symbol != app.source_label {
                        return;
                    }
                    app.backfill_loading = false;
                    let added = if let Some(data_key) = app.active_data_key.clone() {
                        prepend_chart_history_from_service(
                            &mut app.market_data_service,
                            &mut app.bar_store,
                            data_key,
                            bars,
                        )
                        .0
                    } else {
                        app.bar_store.prepend_history(bars)
                    };
                    if added > 0 {
                        app.latency_probe.record_data_ingest(Instant::now());
                        let study_update = app.study_runtime.replay(&app.bar_store);
                        app.sync_study_generation(study_update);
                        app.chart
                            .time_scale
                            .pan_by_bars(added as f64, app.bar_store.len());
                        app.chart.data_len = app.bar_store.len();
                        app.status = format!(
                            "Loaded {added} older {} bars for {symbol}",
                            app.selected_timeframe.label()
                        );
                        app.status_is_error = false;
                    } else if no_data || app.bar_store.front().is_some_and(|bar| bar.time >= to) {
                        app.no_more_history = true;
                    }
                }),
                MarketEvent::BackfillFailed {
                    widget_id,
                    request_id,
                    symbol,
                    to,
                    error,
                } => self.with_live_chart_instance(widget_id, |app| {
                    if request_id != app.request_id || symbol != app.source_label {
                        return;
                    }
                    app.backfill_loading = false;
                    app.no_more_history = true;
                    app.status = format!("{symbol}: older history before {to} failed: {error}");
                    app.status_is_error = true;
                }),
            }
        }
    }

    fn submit_search(&mut self, ctx: &egui::Context) {
        self.submit_search_with_options(ctx, false);
    }

    fn submit_search_preserving_app_history(&mut self, ctx: &egui::Context) {
        self.submit_search_with_options(ctx, true);
    }

    fn submit_search_with_options(&mut self, ctx: &egui::Context, preserve_app_history: bool) {
        let Some(widget_id) = self.current_live_chart_widget_id() else {
            self.status = "No active Live Chart widget for symbol search".to_string();
            self.status_is_error = true;
            return;
        };
        let symbol = normalized_symbol(self.market_source, &self.search_input);
        if symbol.is_empty() {
            self.status = "Enter a ticker or symbol".to_string();
            self.status_is_error = true;
            return;
        }

        self.cancel_drawing_creation();
        self.save_current_drawing_document();
        self.cancel_subscription();
        self.request_id = self.request_id.wrapping_add(1);
        let request_id = self.request_id;
        self.preserve_app_history_on_load_request_id = preserve_app_history.then_some(request_id);
        let source = self.market_source;
        let interval = self.selected_timeframe;
        let request = history_request(source, &symbol, interval);
        let feed = self.feed_for_source(source);
        self.loading = true;
        self.backfill_loading = false;
        self.no_more_history = false;
        self.last_backfill_to = None;
        self.active_source = None;
        self.active_data_key = None;
        self.status_is_error = false;
        self.status = format!(
            "Loading {symbol} {} from {}",
            interval.label(),
            source.label()
        );
        self.source_label = symbol.clone();
        self.search_input = symbol.clone();
        self.selected_drawing_id = None;
        self.hovered_drawing_id = None;
        self.hovered_drawing_hit = None;
        self.drawing_drag = None;
        self.text_editing = None;
        self.search_open = false;
        self.search_palette_rect = None;
        self.search_results_query.clear();
        self.search_results.clear();
        self.search_last_edited = None;
        self.save_live_chart_sessions();

        let sender = self.sender.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let event = match feed.get_bars(request) {
                Ok(response) => MarketEvent::Loaded {
                    widget_id,
                    request_id,
                    source,
                    symbol,
                    bars: response.bars,
                },
                Err(error) => MarketEvent::LoadFailed {
                    widget_id,
                    request_id,
                    symbol,
                    error: error.to_string(),
                },
            };
            let _ = sender.send(event);
            ctx.request_repaint();
        });
    }

    fn start_market_subscription(
        &mut self,
        ctx: &egui::Context,
        widget_id: WidgetId,
        request_id: u64,
        source: MarketSource,
        symbol: String,
        interval: BarInterval,
    ) {
        self.cancel_subscription();
        if source != MarketSource::Binance {
            return;
        }

        let sender = self.sender.clone();
        let ctx = ctx.clone();
        let request = history_request(source, &symbol, interval);
        let subscribe_request = SubscribeRequest {
            symbol: request.symbol,
            resolution: request.resolution,
        };
        match self.binance_feed.subscribe_bars(
            subscribe_request,
            Box::new(move |update| {
                let _ = sender.send(MarketEvent::LiveBar {
                    widget_id,
                    request_id,
                    bar: update.bar,
                });
                ctx.request_repaint();
            }),
        ) {
            Ok(id) => {
                self.active_subscription = Some(id);
            }
            Err(error) => {
                self.status = format!("Binance stream failed: {error}");
                self.status_is_error = true;
            }
        }
    }

    fn cancel_subscription(&mut self) {
        if let Some(id) = self.active_subscription.take() {
            self.binance_feed.unsubscribe_bars(id);
        }
    }

    fn feed_for_source(&self, source: MarketSource) -> Arc<dyn MarketDataFeed> {
        match source {
            MarketSource::YFinance => self.yfinance_feed.clone(),
            MarketSource::Massive => self.massive_feed.clone(),
            MarketSource::Binance => self.binance_feed.clone(),
            MarketSource::CoinGecko => self.coingecko_feed.clone(),
        }
    }

    fn sync_study_generation(&mut self, update: StudyRuntimeUpdate) {
        if update != StudyRuntimeUpdate::Unchanged {
            self.chart_renderer
                .set_study_generation(self.study_runtime.generation());
        }
    }

    fn rebuild_studies(&mut self) {
        normalize_study_instances(&mut self.study_instances);
        self.study_runtime = build_study_runtime(&self.study_instances);
        let volume_visible = self.chart.volume_visible();
        self.chart.series = Default::default();
        self.chart.set_volume_visible(volume_visible);
        apply_indicator_layout(&mut self.chart, &self.study_instances);
        self.study_runtime
            .register_output_series(&mut self.chart.series);
        self.study_runtime.replay(&self.bar_store);
        self.chart_renderer
            .set_study_generation(self.study_runtime.generation());
        self.chart_renderer.clear_cache();
    }

    fn apply_study_instances_with_undo(&mut self, mut instances: Vec<StudyInstance>) {
        normalize_study_instances(&mut instances);
        if instances == self.study_instances {
            return;
        }
        let undo_snapshot = self.app_undo_snapshot();
        self.study_instances = instances;
        self.clear_missing_selected_study();
        self.push_app_undo_snapshot_if_changed(undo_snapshot);
        self.rebuild_studies();
        self.save_layout();
    }

    fn clear_missing_selected_study(&mut self) {
        if !self
            .study_instances
            .iter()
            .any(|instance| Some(instance.id) == self.selected_study_instance_id)
        {
            self.selected_study_instance_id = None;
        }
        if !self
            .study_instances
            .iter()
            .any(|instance| Some(instance.id) == self.study_settings_instance_id)
        {
            self.study_settings_instance_id = None;
        }
    }

    fn apply_timeframe_with_undo(&mut self, ctx: &egui::Context, interval: BarInterval) {
        if self.selected_timeframe == interval {
            return;
        }
        let undo_snapshot = self.app_undo_snapshot();
        self.selected_timeframe = interval;
        self.push_app_undo_snapshot_if_changed(undo_snapshot);
        self.submit_search_preserving_app_history(ctx);
    }

    fn save_user_settings(&mut self) {
        let Some(paths) = &self.storage_paths else {
            self.status =
                "User settings could not be saved: storage root is unavailable".to_string();
            self.status_is_error = true;
            return;
        };
        if let Err(error) = save_user_settings_state(
            paths,
            &self.drawing_tool_policy,
            self.chart.volume_visible(),
        ) {
            self.status = format!("User settings could not be saved: {error}");
            self.status_is_error = true;
        }
    }

    fn save_drawing_tool_templates(&mut self) {
        let Some(paths) = &self.storage_paths else {
            self.status =
                "Drawing templates could not be saved: storage root is unavailable".to_string();
            self.status_is_error = true;
            return;
        };
        if let Err(error) = save_drawing_tool_templates_state(paths, &self.drawing_tool_templates) {
            self.status = format!("Drawing templates could not be saved: {error}");
            self.status_is_error = true;
        }
    }

    fn save_fib_presets(&mut self) {
        let Some(paths) = &self.storage_paths else {
            self.status = "Fib presets could not be saved: storage root is unavailable".to_string();
            self.status_is_error = true;
            return;
        };
        if let Err(error) = save_fib_presets_state(paths, &self.fib_presets) {
            self.status = format!("Fib presets could not be saved: {error}");
            self.status_is_error = true;
        }
    }

    fn save_chart_presets(&mut self) {
        let Some(paths) = &self.storage_paths else {
            self.status =
                "Chart presets could not be saved: storage root is unavailable".to_string();
            self.status_is_error = true;
            return;
        };
        if let Err(error) = save_chart_presets_state(paths, &self.chart_presets) {
            self.status = format!("Chart presets could not be saved: {error}");
            self.status_is_error = true;
        }
    }

    fn save_layout(&mut self) {
        let Some(paths) = &self.storage_paths else {
            self.status = "Layout could not be saved: storage root is unavailable".to_string();
            self.status_is_error = true;
            return;
        };
        if let Err(error) = save_layout_state(paths, &self.study_instances) {
            self.status = format!("Layout document could not be saved: {error}");
            self.status_is_error = true;
        }
    }

    fn save_workspace_layout(&mut self) {
        if let Err(error) = self.save_workspace_layout_result() {
            self.status = format!("Workspace layout could not be saved: {error}");
            self.status_is_error = true;
        }
    }

    fn save_workspace_layout_with_status(&mut self) {
        match self.save_workspace_layout_result() {
            Ok(()) => {
                self.status = "Workspace layout saved".to_string();
                self.status_is_error = false;
            }
            Err(error) => {
                self.status = format!("Workspace layout could not be saved: {error}");
                self.status_is_error = true;
            }
        }
    }

    fn save_workspace_layout_result(&self) -> Result<(), String> {
        let Some(paths) = &self.storage_paths else {
            return Err("storage root is unavailable".to_string());
        };
        save_workspace_layout_state(paths, &self.workspace_shell.workspace_layout_snapshot())
            .map_err(|error| error.to_string())?;
        save_live_chart_sessions_state(paths, self.live_chart_session_entries())
            .map_err(|error| error.to_string())
    }

    fn live_chart_session_entries(&self) -> Vec<LiveChartSessionDocumentEntry> {
        let layout = self.workspace_shell.workspace_layout_snapshot();
        let mut entries = Vec::new();
        for widget in layout
            .widgets
            .iter()
            .filter(|widget| widget.kind == WidgetKind::LiveChart)
        {
            if Some(widget.id) == self.loaded_live_chart_widget_id {
                entries.push(live_chart_session_entry(
                    widget.id,
                    self.active_source.unwrap_or(self.market_source),
                    &self.source_label,
                    self.selected_timeframe,
                ));
            } else if let Some(state) = self.live_chart_instances.get(&widget.id) {
                entries.push(live_chart_session_entry(
                    widget.id,
                    state.active_source.unwrap_or(state.market_source),
                    &state.source_label,
                    state.selected_timeframe,
                ));
            } else if let Some(restore) = self.live_chart_session_restores.get(&widget.id) {
                entries.push(live_chart_session_entry(
                    widget.id,
                    restore.source,
                    &restore.symbol,
                    restore.selected_timeframe,
                ));
            }
        }
        entries.sort_by_key(|entry| entry.widget_id);
        entries
    }

    fn save_live_chart_sessions(&mut self) {
        let Some(paths) = &self.storage_paths else {
            self.status =
                "Live Chart sessions could not be saved: storage root is unavailable".to_string();
            self.status_is_error = true;
            return;
        };
        if let Err(error) = save_live_chart_sessions_state(paths, self.live_chart_session_entries())
        {
            self.status = format!("Live Chart sessions could not be saved: {error}");
            self.status_is_error = true;
        }
    }

    fn restore_workspace_layout(&mut self) {
        let Some(paths) = &self.storage_paths else {
            self.status =
                "Workspace layout could not be restored: storage root is unavailable".to_string();
            self.status_is_error = true;
            return;
        };
        let state = load_workspace_layout_state(paths);
        if let Some(status) = state.status {
            self.status = status.text;
            self.status_is_error = status.is_error;
            return;
        }
        let Some(restore_input) = state.restore_input else {
            self.status = "No saved workspace layout found".to_string();
            self.status_is_error = false;
            return;
        };
        let live_chart_sessions_state = load_live_chart_sessions_state(paths);
        self.remove_workspace_live_chart_instances();
        self.live_chart_session_restores = live_chart_sessions_state.restores;
        self.workspace_shell.restore_workspace_layout(restore_input);
        if let Some(status) = live_chart_sessions_state.status {
            self.status = status.text;
            self.status_is_error = status.is_error;
        } else {
            self.status = "Workspace layout restored".to_string();
            self.status_is_error = false;
        }
    }

    fn apply_workspace_preset(&mut self, preset: WorkspacePresetKind) {
        self.remove_workspace_live_chart_instances();
        self.live_chart_session_restores.clear();
        let descriptors = self.workspace_shell.apply_workspace_preset(preset);
        if let Some(descriptor) = descriptors
            .iter()
            .rev()
            .find(|descriptor| descriptor.kind == WidgetKind::LiveChart)
        {
            self.create_live_chart_instance(descriptor.id);
            self.load_live_chart_instance(descriptor.id);
        }
        match self.save_workspace_layout_result() {
            Ok(()) => {
                self.status = format!("Workspace preset applied: {}", preset.label());
                self.status_is_error = false;
            }
            Err(error) => {
                self.status = format!("Workspace preset applied but could not be saved: {error}");
                self.status_is_error = true;
            }
        }
    }

    fn remove_workspace_live_chart_instances(&mut self) {
        let live_chart_widget_ids: Vec<_> = self
            .workspace_shell
            .workspace_layout_snapshot()
            .widgets
            .iter()
            .filter(|widget| widget.kind == WidgetKind::LiveChart)
            .map(|widget| widget.id)
            .collect();
        for widget_id in live_chart_widget_ids {
            self.remove_live_chart_instance(widget_id);
        }
    }

    fn reset_workspace_layout(&mut self) {
        self.remove_workspace_live_chart_instances();
        self.workspace_shell.reset_workspace_layout();
        let Some(paths) = &self.storage_paths else {
            self.status = "Workspace reset; persistence storage is unavailable".to_string();
            self.status_is_error = true;
            return;
        };
        match reset_workspace_layout_state(paths) {
            Ok(()) => {
                let live_result = save_live_chart_sessions_state(paths, Vec::new());
                match live_result {
                    Ok(()) => {
                        self.live_chart_session_restores.clear();
                        self.status = "Workspace layout reset".to_string();
                        self.status_is_error = false;
                    }
                    Err(error) => {
                        self.status =
                            format!("Workspace layout reset but sessions remain: {error}");
                        self.status_is_error = true;
                    }
                }
            }
            Err(error) => {
                self.status = format!("Workspace layout could not be reset: {error}");
                self.status_is_error = true;
            }
        }
    }

    fn current_drawing_document_scope(&self) -> DrawingDocumentScope {
        drawing_document_scope(
            self.active_source.unwrap_or(self.market_source),
            &self.source_label,
        )
    }

    fn save_current_drawing_document(&mut self) {
        self.cache_current_drawing_document();
        let Some(paths) = &self.storage_paths else {
            return;
        };
        if let Err(error) = save_drawing_document_state(
            paths,
            self.current_drawing_document_scope(),
            &self.drawings,
        ) {
            self.status = format!("Drawing document could not be saved: {error}");
            self.status_is_error = true;
        }
    }

    fn save_drawings_if_generation_changed(&mut self, generation_before: u64) {
        if self.drawings.generation() != generation_before {
            self.save_current_drawing_document();
        }
    }

    fn app_undo_snapshot(&self) -> AppUndoSnapshot {
        AppUndoSnapshot {
            drawings: self.drawings.clone(),
            selected_drawing_id: self.selected_drawing_id,
            study_instances: self.study_instances.clone(),
            selected_study_instance_id: self.selected_study_instance_id,
            study_settings_instance_id: self.study_settings_instance_id,
            selected_timeframe: self.selected_timeframe,
        }
    }

    fn push_app_undo_snapshot_if_changed(&mut self, snapshot: AppUndoSnapshot) {
        if self.app_undo_snapshot_matches_current(&snapshot) {
            return;
        }
        push_app_snapshot_stack(&mut self.app_undo_stack, snapshot);
        self.app_redo_stack.clear();
    }

    fn app_undo_snapshot_matches_current(&self, snapshot: &AppUndoSnapshot) -> bool {
        drawing_store_state_matches(&self.drawings, &snapshot.drawings)
            && self.selected_drawing_id == snapshot.selected_drawing_id
            && self.study_instances == snapshot.study_instances
            && self.selected_study_instance_id == snapshot.selected_study_instance_id
            && self.study_settings_instance_id == snapshot.study_settings_instance_id
            && self.selected_timeframe == snapshot.selected_timeframe
    }

    fn undo_last_app_action(&mut self, ctx: &egui::Context) -> bool {
        let Some(snapshot) = self.app_undo_stack.pop() else {
            self.status = "Nothing to undo".to_string();
            self.status_is_error = false;
            return false;
        };
        let redo_snapshot = self.app_undo_snapshot();
        self.apply_app_snapshot(ctx, snapshot, "Undo chart action");
        push_app_snapshot_stack(&mut self.app_redo_stack, redo_snapshot);
        true
    }

    fn redo_last_app_action(&mut self, ctx: &egui::Context) -> bool {
        let Some(snapshot) = self.app_redo_stack.pop() else {
            self.status = "Nothing to redo".to_string();
            self.status_is_error = false;
            return false;
        };
        let undo_snapshot = self.app_undo_snapshot();
        self.apply_app_snapshot(ctx, snapshot, "Redo chart action");
        push_app_snapshot_stack(&mut self.app_undo_stack, undo_snapshot);
        true
    }

    fn apply_app_snapshot(
        &mut self,
        ctx: &egui::Context,
        snapshot: AppUndoSnapshot,
        status: &'static str,
    ) {
        let AppUndoSnapshot {
            drawings,
            selected_drawing_id,
            study_instances,
            selected_study_instance_id,
            study_settings_instance_id,
            selected_timeframe,
        } = snapshot;
        let drawings_changed = !drawing_store_state_matches(&self.drawings, &drawings);
        let studies_changed = self.study_instances != study_instances;
        let timeframe_changed = self.selected_timeframe != selected_timeframe;
        if drawings_changed {
            self.drawings.restore_from(drawings);
        }
        self.study_instances = study_instances;
        self.selected_drawing_id = selected_drawing_id;
        self.selected_study_instance_id = selected_study_instance_id;
        self.study_settings_instance_id = study_settings_instance_id;
        self.selected_timeframe = selected_timeframe;
        self.next_study_instance_id = self
            .next_study_instance_id
            .max(next_study_instance_id(&self.study_instances));
        self.selected_drawing_toolbar_position = None;
        self.selected_drawing_toolbar_drag_start = None;
        self.fib_settings_drawing_id = None;
        self.fib_settings_panel_rect = None;
        self.position_settings_drawing_id = None;
        self.position_settings_panel_rect = None;
        self.hovered_drawing_id = None;
        self.hovered_drawing_hit = None;
        self.drawing_creation = None;
        self.drawing_drag = None;
        self.text_editing = None;
        self.status = status.to_string();
        self.status_is_error = false;
        if studies_changed {
            self.rebuild_studies();
            self.save_layout();
        }
        if drawings_changed {
            self.save_current_drawing_document();
        }
        if timeframe_changed {
            self.submit_search_preserving_app_history(ctx);
        }
    }

    fn load_drawing_document(
        &mut self,
        source: MarketSource,
        symbol: &str,
        preserve_app_history: bool,
    ) {
        let key = live_chart_drawing_cache_key(source, symbol);
        if let Some(drawings) = self.drawing_document_cache.get(&key).cloned() {
            self.drawings = drawings;
            if !preserve_app_history {
                self.app_undo_stack.clear();
                self.app_redo_stack.clear();
            }
            self.clear_all_drawing_interaction_state();
            return;
        }
        let Some(paths) = &self.storage_paths else {
            self.drawings = DrawingStore::new();
            self.drawing_document_cache
                .insert(key, self.drawings.clone());
            return;
        };
        let scope = drawing_document_scope(source, symbol);
        let drawing_state = load_drawing_document_state(paths, &scope);
        self.drawings = drawing_state.drawings;
        self.drawing_document_cache
            .insert(key, self.drawings.clone());
        if !preserve_app_history {
            self.app_undo_stack.clear();
            self.app_redo_stack.clear();
        }
        self.clear_all_drawing_interaction_state();
        if let Some(status) = drawing_state.status {
            self.status = status.text;
            self.status_is_error = status.is_error;
        }
    }

    fn maybe_request_history_backfill(&mut self, ctx: &egui::Context) {
        let Some(widget_id) = self.current_live_chart_widget_id() else {
            return;
        };
        if !should_request_backfill(
            self.chart.time_scale.visible_logical_range(),
            self.bar_store.front().map(|bar| bar.time),
            self.loading || self.backfill_loading,
            self.no_more_history,
            self.last_backfill_to,
        ) {
            return;
        }

        let Some(source) = self.active_source else {
            return;
        };
        let Some(first) = self.bar_store.front().copied() else {
            return;
        };

        let symbol = self.source_label.clone();
        let to = first.time;
        self.backfill_loading = true;
        self.last_backfill_to = Some(to);

        let mut request = history_request(source, &symbol, self.selected_timeframe);
        request.to = Some(to);
        request.count_back = Some(BACKFILL_BAR_COUNT);
        request.first_data_request = false;

        let feed = self.feed_for_source(source);
        let sender = self.sender.clone();
        let request_id = self.request_id;
        let ctx = ctx.clone();
        thread::spawn(move || {
            let event = match feed.get_bars(request) {
                Ok(response) => MarketEvent::Backfilled {
                    widget_id,
                    request_id,
                    symbol,
                    to,
                    bars: response.bars,
                    no_data: response.no_data,
                },
                Err(error) => MarketEvent::BackfillFailed {
                    widget_id,
                    request_id,
                    symbol,
                    to,
                    error: error.to_string(),
                },
            };
            let _ = sender.send(event);
            ctx.request_repaint();
        });
    }

    fn open_search_on_click(&mut self, response: egui::Response) {
        if response.clicked() {
            self.search_open = true;
            self.focus_search = true;
            self.select_search_on_focus = true;
            self.search_input = self.source_label.clone();
            self.search_source_filter = SearchSourceFilter::All;
            self.search_type_filter = SearchTypeFilter::All;
            self.refresh_search_results();
        }
    }

    fn handle_type_to_search_shortcut(&mut self, ctx: &egui::Context) {
        if self.search_open
            || self.workspace_shell.command_open()
            || self.timeframe_input_open
            || self.text_editing.is_some()
            || ctx.egui_wants_keyboard_input()
        {
            return;
        }
        let typed_timeframe = ctx.input(|input| {
            if input.modifiers.command || input.modifiers.ctrl || input.modifiers.alt {
                None
            } else {
                typed_timeframe_prefix(&input.events)
            }
        });
        if let Some(typed) = typed_timeframe {
            self.open_timeframe_input(typed);
            return;
        }

        let typed = ctx.input(|input| {
            if input.modifiers.command || input.modifiers.ctrl || input.modifiers.alt {
                None
            } else {
                typed_search_prefix(&input.events)
            }
        });
        let Some(typed) = typed else {
            return;
        };

        self.search_open = true;
        self.focus_search = true;
        self.select_search_on_focus = false;
        self.search_input = typed;
        self.search_source_filter = SearchSourceFilter::All;
        self.search_type_filter = SearchTypeFilter::All;
        self.refresh_search_results();
    }

    fn open_timeframe_input(&mut self, typed: String) {
        self.timeframe_input_open = true;
        self.focus_timeframe_input = true;
        self.timeframe_input = typed;
    }

    fn apply_shell_command_action(&mut self, action: ShellCommandAction) {
        match action {
            ShellCommandAction::SpawnWidget(descriptor) => {
                if descriptor.kind == WidgetKind::LiveChart {
                    self.create_live_chart_instance(descriptor.id);
                    self.load_live_chart_instance(descriptor.id);
                }
                self.status = format!("{} launched", descriptor.title());
                self.status_is_error = false;
                self.save_workspace_layout();
            }
            ShellCommandAction::MinimizeWidget(descriptor) => {
                self.status = format!("{} minimized", descriptor.title());
                self.status_is_error = false;
                self.save_workspace_layout();
            }
            ShellCommandAction::RestoreWidget(descriptor) => {
                self.status = format!("{} restored", descriptor.title());
                self.status_is_error = false;
                self.save_workspace_layout();
            }
            ShellCommandAction::TearOutWidget(descriptor) => {
                self.status = format!("{} torn out", descriptor.title());
                self.status_is_error = false;
                self.save_workspace_layout();
            }
            ShellCommandAction::ReturnWidgetToShell(descriptor) => {
                self.status = format!("{} returned to shell", descriptor.title());
                self.status_is_error = false;
                self.save_workspace_layout();
            }
            ShellCommandAction::SaveWorkspaceLayout => {
                self.save_workspace_layout_with_status();
            }
            ShellCommandAction::RestoreWorkspaceLayout => {
                self.restore_workspace_layout();
            }
            ShellCommandAction::ResetWorkspaceLayout => {
                self.reset_workspace_layout();
            }
            ShellCommandAction::ApplyWorkspacePreset(preset) => {
                self.apply_workspace_preset(preset);
            }
        }
    }

    fn show_workspace_widget_host(
        &mut self,
        ctx: &egui::Context,
        widget: WidgetSnapshot,
        display_title: &str,
        show_overlay_areas: bool,
    ) -> WorkspaceWidgetHostResponse {
        match widget.host.mode {
            WidgetHostMode::InShellWorkspaceSurface => self.show_workspace_widget_in_shell_surface(
                ctx,
                widget,
                display_title,
                show_overlay_areas,
            ),
            WidgetHostMode::DetachedNativeViewport => self.show_workspace_widget_detached_viewport(
                ctx,
                widget,
                display_title,
                show_overlay_areas,
            ),
        }
    }

    fn show_workspace_widget_detached_viewport(
        &mut self,
        ctx: &egui::Context,
        widget: WidgetSnapshot,
        display_title: &str,
        show_overlay_areas: bool,
    ) -> WorkspaceWidgetHostResponse {
        let mut open = true;
        let mut focused = false;
        let mut host_class = WidgetHostClass::NativeViewport;
        let viewport_id = widget.descriptor.viewport_id();
        let builder = detached_widget_viewport_builder(widget, display_title);

        ctx.show_viewport_immediate(viewport_id, builder, |ui, viewport_class| {
            self.handle_chart_snapshot_events(ui.ctx());
            host_class = viewport_host_class(viewport_class);
            focused = ui.input(|input| input.focused);
            if ui.input(|input| input.viewport().close_requested()) {
                open = false;
                return;
            }

            egui::CentralPanel::default()
                .frame(egui::Frame::default().fill(egui::Color32::from_rgb(13, 13, 13)))
                .show_inside(ui, |ui| {
                    ui.push_id(
                        ("workspace_widget_viewport_scope", widget.id().get()),
                        |ui| {
                            self.show_workspace_widget_contents(ui, widget, show_overlay_areas);
                        },
                    );
                });
        });

        WorkspaceWidgetHostResponse {
            open,
            host_class,
            focused,
            layout_rect: None,
        }
    }

    fn show_workspace_widget_in_shell_surface(
        &mut self,
        ctx: &egui::Context,
        widget: WidgetSnapshot,
        display_title: &str,
        show_overlay_areas: bool,
    ) -> WorkspaceWidgetHostResponse {
        let mut open = true;
        let mut content_focused = false;
        let shown = egui::Window::new(display_title)
            .id(egui::Id::new(("workspace_widget", widget.id().get())))
            .default_pos(widget.host.default_pos)
            .default_size(widget.host.default_size)
            .min_width(widget.host.in_shell_min_size.x)
            .min_height(widget.host.in_shell_min_size.y)
            .constrain(true)
            .resizable(true)
            .movable(true)
            .collapsible(false)
            .frame(
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(13, 13, 13))
                    .stroke(egui::Stroke::new(0.5, egui::Color32::from_rgb(36, 36, 36)))
                    .inner_margin(egui::Margin::same(2)),
            )
            .open(&mut open)
            .show(ctx, |ui| {
                let content_rect = ui.max_rect();
                content_focused = ui.rect_contains_pointer(content_rect)
                    && ui.input(|input| input.pointer.any_pressed() || input.pointer.any_down());
                ui.push_id(("workspace_widget_scope", widget.id().get()), |ui| {
                    self.show_workspace_widget_contents(ui, widget, show_overlay_areas);
                });
            });
        let focused = shown.as_ref().is_some_and(|inner| {
            inner.response.clicked() || inner.response.dragged() || content_focused
        });
        let layout_rect = shown.as_ref().map(|inner| {
            let rect = inner.response.rect;
            WorkspaceLayoutRect::new(rect.left(), rect.top(), rect.width(), rect.height())
        });
        WorkspaceWidgetHostResponse {
            open,
            host_class: WidgetHostClass::InShellWorkspaceSurface,
            focused,
            layout_rect,
        }
    }

    fn show_workspace_widget_contents(
        &mut self,
        ui: &mut egui::Ui,
        widget: WidgetSnapshot,
        show_overlay_areas: bool,
    ) {
        match widget.kind() {
            WidgetKind::LiveChart => {
                self.load_live_chart_instance(widget.id());
                self.show_live_chart_widget_contents(ui, widget, show_overlay_areas)
            }
            WidgetKind::SyncedTimeframes => {
                if let Some(interval) = timeframe_widget::show_synced_timeframes_widget(
                    ui,
                    &self.bar_store,
                    self.selected_timeframe,
                    &self.source_label,
                ) {
                    self.apply_timeframe_with_undo(ui.ctx(), interval);
                }
            }
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
            | WidgetKind::MarketPulse => order_flow_widgets::show_order_flow_widget(
                ui,
                widget.kind(),
                &mut self.order_flow_preview_service,
            ),
            _ => self.show_placeholder_widget_contents(ui, widget),
        }
    }

    fn show_placeholder_widget_contents(&mut self, ui: &mut egui::Ui, widget: WidgetSnapshot) {
        debug_assert!(widget.kind().is_placeholder());
        let fill = egui::Color32::from_rgb(13, 13, 13);
        egui::Frame::default()
            .fill(fill)
            .inner_margin(egui::Margin::same(14))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(widget.kind().mnemonic())
                            .monospace()
                            .size(12.0)
                            .color(egui::Color32::from_rgb(232, 178, 88)),
                    );
                    ui.label(
                        egui::RichText::new(widget.kind().label())
                            .monospace()
                            .size(17.0)
                            .color(egui::Color32::from_rgb(230, 232, 235)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new("placeholder")
                                .monospace()
                                .size(11.0)
                                .color(egui::Color32::from_rgb(118, 122, 128)),
                        );
                    });
                });
                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);

                for (label, value) in placeholder_widget_rows(widget.kind()) {
                    let row_height = 28.0;
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), row_height),
                        egui::Sense::hover(),
                    );
                    ui.painter()
                        .rect_filled(rect, 0.0, egui::Color32::from_rgb(20, 20, 20));
                    ui.painter().text(
                        egui::pos2(rect.left() + 8.0, rect.center().y),
                        egui::Align2::LEFT_CENTER,
                        label,
                        egui::FontId::monospace(12.0),
                        egui::Color32::from_rgb(150, 154, 160),
                    );
                    ui.painter().text(
                        egui::pos2(rect.right() - 8.0, rect.center().y),
                        egui::Align2::RIGHT_CENTER,
                        value,
                        egui::FontId::monospace(12.0),
                        egui::Color32::from_rgb(210, 212, 216),
                    );
                    ui.add_space(4.0);
                }
            });
    }

    fn show_live_chart_widget_contents(
        &mut self,
        ui: &mut egui::Ui,
        widget: WidgetSnapshot,
        show_overlay_areas: bool,
    ) {
        let widget_id = widget.id();
        if show_overlay_areas {
            self.handle_type_to_search_shortcut(ui.ctx());
        }
        let density = chart_widget_density(ui.available_size());
        let show_header = density != ChartWidgetDensity::Tiny || ui.available_height() >= 132.0;
        if show_header {
            match density {
                ChartWidgetDensity::Full => self.show_tradingview_header(ui, widget),
                ChartWidgetDensity::Compact
                | ChartWidgetDensity::Mini
                | ChartWidgetDensity::Tiny => {
                    self.show_live_chart_widget_compact_header(ui, density, widget);
                }
            }
        }

        let panel_fill = chart_color_to_egui(self.chart.theme().appearance.frame_background);
        egui::Frame::default().fill(panel_fill).show(ui, |ui| {
            if self.status_is_error {
                ui.colored_label(egui::Color32::from_rgb(232, 178, 88), &self.status);
                ui.add_space(4.0);
            }
            let content_size = ui.available_size();
            ui.allocate_ui_with_layout(
                content_size,
                egui::Layout::left_to_right(egui::Align::Min),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let show_left_rail = density == ChartWidgetDensity::Full;
                    let show_right_rail = density == ChartWidgetDensity::Full;
                    if show_left_rail {
                        self.show_drawing_toolbar(ui);
                    }
                    let right_rail_width = if show_right_rail {
                        CHART_UTILITY_RAIL_WIDTH
                    } else {
                        0.0
                    };
                    let chart_min = match density {
                        ChartWidgetDensity::Full => egui::vec2(180.0, 150.0),
                        ChartWidgetDensity::Compact => egui::vec2(160.0, 120.0),
                        ChartWidgetDensity::Mini => egui::vec2(120.0, 96.0),
                        ChartWidgetDensity::Tiny => egui::vec2(120.0, 96.0),
                    };
                    let chart_width = (ui.available_width() - right_rail_width).max(chart_min.x);
                    let chart_height = ui.available_height().max(chart_min.y);
                    ui.allocate_ui_with_layout(
                        egui::vec2(chart_width, chart_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            let chart_response = {
                                let study_source = AppStudyDataSource {
                                    runtime: &self.study_runtime,
                                    instances: &self.study_instances,
                                    active_interval: self.selected_timeframe,
                                };
                                let drawing_symbol_filter = self.current_drawing_symbol_key();
                                let hovered_drawing_claims_pointer =
                                    self.hovered_drawing_id.is_some()
                                        && ui.ctx().input(|input| {
                                            input.pointer.primary_down()
                                                || input.pointer.primary_pressed()
                                        });
                                let drawing_layer = ChartDrawingLayer {
                                    drawings: &self.drawings,
                                    selected: self.selected_drawing_id,
                                    hovered: self.hovered_drawing_id,
                                    symbol_filter: Some(&drawing_symbol_filter),
                                    interval_seconds: self.current_drawing_interval_seconds(),
                                    crosshair_magnet: self.drawing_crosshair_magnet(ui.ctx()),
                                    blocks_chart_input: drawing_layer_blocks_chart_input(
                                        self.drawing_creation.is_some(),
                                        self.drawing_drag.is_some(),
                                        self.text_editing.is_some(),
                                        self.measure_state
                                            .is_some_and(measure_state_blocks_chart_input),
                                        hovered_drawing_claims_pointer,
                                    ),
                                };
                                show_candlestick_chart_with_sizing(
                                    ui,
                                    &mut self.chart,
                                    &mut self.chart_renderer,
                                    &self.source_label,
                                    &self.bar_store,
                                    Some(&study_source),
                                    Some(drawing_layer),
                                    ChartWidgetSizing::embedded_shell_widget(),
                                )
                            };
                            self.chart_snapshot_capture_rects
                                .insert(widget_id, chart_response.outer);
                            let chart_snapshot = {
                                let study_source = AppStudyDataSource {
                                    runtime: &self.study_runtime,
                                    instances: &self.study_instances,
                                    active_interval: self.selected_timeframe,
                                };
                                prepare_chart_snapshot_with_studies(
                                    &self.chart,
                                    &self.bar_store,
                                    chart_response.plot,
                                    Some(&study_source),
                                )
                            };
                            let search_dismissed =
                                self.handle_search_palette_chart_dismiss(ui.ctx(), &chart_response);
                            let floating_panel_dismissed =
                                self.handle_floating_panel_chart_dismiss(ui.ctx(), &chart_response);
                            if !search_dismissed && !floating_panel_dismissed {
                                if self.measure_active {
                                    self.handle_measure_interaction(
                                        ui.ctx(),
                                        &chart_response,
                                        &chart_snapshot,
                                    );
                                } else {
                                    self.handle_drawing_interaction(
                                        ui.ctx(),
                                        &chart_response,
                                        &chart_snapshot,
                                    );
                                }
                            }
                            self.show_measure_overlay(ui.ctx(), &chart_response, &chart_snapshot);
                            self.show_selected_drawing_toolbar(ui.ctx(), &chart_response);
                            self.show_text_edit_overlay(ui.ctx(), &chart_response, &chart_snapshot);
                            if !search_dismissed && !floating_panel_dismissed {
                                self.handle_chart_context_trigger(
                                    ui.ctx(),
                                    &chart_response,
                                    &chart_snapshot,
                                );
                            }
                            if show_overlay_areas && density == ChartWidgetDensity::Full {
                                self.show_study_legend_rows(ui.ctx(), &chart_response);
                            }
                            self.maybe_request_history_backfill(ui.ctx());
                        },
                    );
                    if show_right_rail {
                        self.show_chart_utility_rail(ui);
                    }
                },
            );
        });
        if show_overlay_areas {
            self.show_live_chart_widget_floating_surfaces(ui.ctx());
            self.show_latency_overlay(ui.ctx());
        }
    }

    fn show_live_chart_widget_floating_surfaces(&mut self, ctx: &egui::Context) {
        self.show_search_palette(ctx);
        self.show_timeframe_input_palette(ctx);
        self.show_indicator_panel(ctx);
        self.show_chart_settings_surface(ctx);
        self.show_object_tree_panel(ctx);
        self.show_fib_settings_surface(ctx);
        self.show_position_settings_surface(ctx);
        self.show_chart_context_menu(ctx);
        self.show_study_settings_surface(ctx);
    }

    fn handle_chart_snapshot_events(&mut self, ctx: &egui::Context) {
        let captures: Vec<_> = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| {
                    let egui::Event::Screenshot {
                        viewport_id,
                        user_data,
                        image,
                    } = event
                    else {
                        return None;
                    };
                    let request = user_data
                        .data
                        .as_ref()?
                        .downcast_ref::<ChartSnapshotCaptureRequest>()?;
                    Some((*viewport_id, request.clone(), Arc::clone(image)))
                })
                .collect()
        });

        for (viewport_id, request, image) in captures {
            if viewport_id != request.viewport_id {
                continue;
            }
            self.pending_chart_snapshots.remove(&request.widget_id);
            self.with_live_chart_instance(request.widget_id, |app| {
                app.complete_chart_snapshot_capture(ctx, request, image.as_ref());
            });
        }
        self.expire_chart_snapshot_requests();
    }

    fn expire_chart_snapshot_requests(&mut self) {
        let now = current_unix_timestamp_seconds();
        let timeout_secs = CHART_SNAPSHOT_TIMEOUT.as_secs() as i64;
        let expired = self
            .pending_chart_snapshots
            .iter()
            .filter_map(|(&widget_id, request)| {
                (now.saturating_sub(request.requested_at) > timeout_secs).then_some(widget_id)
            })
            .collect::<Vec<_>>();

        for widget_id in expired {
            let Some(request) = self.pending_chart_snapshots.remove(&widget_id) else {
                continue;
            };
            self.with_live_chart_instance(widget_id, |app| {
                app.status = format!(
                    "Chart snapshot failed: no native screenshot event for {} {}",
                    request.source_label, request.timeframe_label
                );
                app.status_is_error = true;
            });
        }
    }

    fn complete_chart_snapshot_capture(
        &mut self,
        ctx: &egui::Context,
        request: ChartSnapshotCaptureRequest,
        image: &egui::ColorImage,
    ) {
        let Some(cropped) =
            chart_snapshot_image_region(image, request.capture_rect, request.pixels_per_point)
        else {
            self.status =
                "Chart snapshot failed: capture region was outside the viewport".to_string();
            self.status_is_error = true;
            return;
        };

        match request.action {
            ChartSnapshotAction::CopyImage => {
                ctx.copy_image(cropped);
                self.status = format!(
                    "Copied {} {} chart snapshot",
                    request.source_label, request.timeframe_label
                );
                self.status_is_error = false;
            }
            ChartSnapshotAction::DownloadImage => {
                match save_chart_snapshot_png(&cropped, self.storage_paths.as_ref(), &request) {
                    Ok(Some(path)) => {
                        self.status = format!("Saved chart snapshot to {}", path.display());
                        self.status_is_error = false;
                    }
                    Ok(None) => {
                        self.status = "Chart snapshot download canceled".to_string();
                        self.status_is_error = false;
                    }
                    Err(error) => {
                        self.status = format!("Chart snapshot failed: {error}");
                        self.status_is_error = true;
                    }
                }
            }
        }
    }

    fn request_chart_snapshot(
        &mut self,
        ctx: &egui::Context,
        widget_id: WidgetId,
        action: ChartSnapshotAction,
    ) {
        let Some(capture_rect) = self.chart_snapshot_capture_rects.get(&widget_id).copied() else {
            self.status = "Chart snapshot failed: chart region is not ready".to_string();
            self.status_is_error = true;
            return;
        };
        let request = ChartSnapshotCaptureRequest {
            widget_id,
            viewport_id: ctx.viewport_id(),
            action,
            capture_rect,
            pixels_per_point: ctx.pixels_per_point(),
            source_label: self.source_label.clone(),
            timeframe_label: self.selected_timeframe.label().to_string(),
            requested_at: current_unix_timestamp_seconds(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
            request.clone(),
        )));
        self.pending_chart_snapshots.insert(widget_id, request);
        self.status = match action {
            ChartSnapshotAction::DownloadImage => "Capturing chart snapshot for download",
            ChartSnapshotAction::CopyImage => "Capturing chart snapshot for clipboard",
        }
        .to_string();
        self.status_is_error = false;
        ctx.request_repaint();
    }

    fn show_chart_snapshot_button(&mut self, ui: &mut egui::Ui, scale: f32, widget_id: WidgetId) {
        let response = show_toolbar_camera_button(ui, scale).on_hover_text("Chart snapshot");
        egui::Popup::menu(&response)
            .id(egui::Id::new(("chart_snapshot_menu", widget_id.get())))
            .width(286.0)
            .show(|ui| {
                self.show_chart_snapshot_menu(ui, widget_id);
            });
    }

    fn show_chart_snapshot_menu(&mut self, ui: &mut egui::Ui, widget_id: WidgetId) {
        ui.set_min_width(272.0);
        ui.label(
            egui::RichText::new("CHART SNAPSHOT")
                .monospace()
                .size(12.0)
                .color(egui::Color32::from_rgb(146, 150, 156)),
        );
        ui.add_space(5.0);
        if show_snapshot_menu_row(ui, "DL", "Download image", true)
            .on_hover_text("Save the current Live Chart widget as a PNG")
            .clicked()
        {
            self.request_chart_snapshot(ui.ctx(), widget_id, ChartSnapshotAction::DownloadImage);
            ui.close();
        }
        if show_snapshot_menu_row(ui, "CP", "Copy image", true)
            .on_hover_text("Copy the current Live Chart widget image")
            .clicked()
        {
            self.request_chart_snapshot(ui.ctx(), widget_id, ChartSnapshotAction::CopyImage);
            ui.close();
        }
        ui.separator();
        ui.label(
            egui::RichText::new("Links and social sharing require a snapshot publish API.")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(126, 132, 140)),
        );
    }

    fn show_indicator_section(&mut self, ui: &mut egui::Ui, scale: f32) {
        let response =
            show_toolbar_toggle(ui, "fx", self.indicators_open, scale).on_hover_text("Indicators");
        if response.clicked() {
            self.indicators_open = !self.indicators_open;
        }
    }

    fn show_chart_settings_button(&mut self, ui: &mut egui::Ui, scale: f32) {
        let response = show_toolbar_settings_button(ui, self.chart_settings_open, scale)
            .on_hover_text("Chart settings");
        if response.clicked() {
            self.chart_settings_open = !self.chart_settings_open;
            self.chart_settings_tab = ChartSettingsTab::Symbol;
        }
    }

    fn handle_chart_context_trigger(
        &mut self,
        ctx: &egui::Context,
        chart: &ChartUiResponse,
        snapshot: &ChartSnapshot,
    ) {
        if self.active_drawing_tool.is_some() {
            return;
        }
        if !chart.response.secondary_clicked() {
            return;
        }
        let position = ctx
            .input(|input| input.pointer.interact_pos())
            .unwrap_or(chart.outer.center());
        if chart.outer.contains(position) {
            let drawing_hit = self.drawing_hit_at_position(chart, snapshot, position);
            if let Some(hit) = drawing_hit {
                self.selected_drawing_id = Some(hit.drawing_id);
                self.hovered_drawing_id = Some(hit.drawing_id);
                self.hovered_drawing_hit = Some(hit);
            }
            self.chart_context_menu = Some(ChartContextMenuState {
                position,
                target: chart_context_target_for_drawing_hit(
                    chart_context_target(chart, position),
                    drawing_hit,
                ),
                plot_width: chart.plot.width(),
            });
        }
    }

    fn show_chart_context_menu(&mut self, ctx: &egui::Context) {
        let Some(menu) = self.chart_context_menu else {
            return;
        };
        let mut action = None;
        let mut keep_open = true;
        let area = egui::Area::new(egui::Id::new("rustchart_chart_context_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(menu.position)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(178.0);
                    if let ChartContextTarget::Drawing(id) = menu.target
                        && let Some(drawing) = self.drawings.get(id)
                    {
                        ui.label(
                            egui::RichText::new(drawing_kind_label(drawing.kind))
                                .size(12.0)
                                .color(egui::Color32::from_rgb(162, 168, 176)),
                        );
                        if drawing_kind_is_text_editable(drawing.kind)
                            && show_context_menu_button_enabled(ui, "Edit text", !drawing.locked)
                        {
                            action =
                                Some(ChartContextMenuAction::Drawing(id, DrawingAction::EditText));
                        }
                        if let Some(settings_label) = drawing_settings_action_label(drawing.kind)
                            && show_context_menu_button(ui, settings_label)
                        {
                            action = Some(ChartContextMenuAction::Drawing(
                                id,
                                DrawingAction::OpenSettings,
                            ));
                        }
                        if show_context_menu_button(ui, drawing_lock_action_label(drawing.locked)) {
                            action = Some(ChartContextMenuAction::Drawing(
                                id,
                                DrawingAction::ToggleLock,
                            ));
                        }
                        if show_context_menu_button(ui, "Hide drawing") {
                            action = Some(ChartContextMenuAction::Drawing(id, DrawingAction::Hide));
                        }
                        if show_context_menu_button_enabled(ui, "Delete drawing", !drawing.locked) {
                            action =
                                Some(ChartContextMenuAction::Drawing(id, DrawingAction::Delete));
                        }
                        ui.separator();
                    }
                    if show_context_menu_button(ui, "Chart settings") {
                        action = Some(ChartContextMenuAction::ChartSettings);
                    }
                    if show_context_menu_button(ui, "Indicators") {
                        action = Some(ChartContextMenuAction::Indicators);
                    }
                    ui.separator();
                    if chart_context_has_price_scale(menu.target)
                        && show_context_menu_button(ui, "Reset price scale")
                    {
                        action = Some(ChartContextMenuAction::ResetPriceScale);
                    }
                    if chart_context_has_time_scale(menu.target)
                        && show_context_menu_button(ui, "Fit content")
                    {
                        action = Some(ChartContextMenuAction::FitContent);
                    }
                });
            });

        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            keep_open = false;
        }
        if ctx.input(|input| input.pointer.any_pressed()) && !area.response.hovered() {
            keep_open = false;
        }

        if let Some(action) = action {
            self.apply_chart_context_menu_action(action);
            self.chart_context_menu = None;
        } else if !keep_open {
            self.chart_context_menu = None;
        }
    }

    fn apply_chart_context_menu_action(&mut self, action: ChartContextMenuAction) {
        match action {
            ChartContextMenuAction::ChartSettings => {
                self.chart_settings_open = true;
                self.chart_settings_tab = ChartSettingsTab::Symbol;
            }
            ChartContextMenuAction::Indicators => {
                self.indicators_open = true;
            }
            ChartContextMenuAction::ResetPriceScale => {
                self.chart.price_scales.reset(PriceScaleId::Right);
            }
            ChartContextMenuAction::FitContent => {
                if let Some(menu) = self.chart_context_menu {
                    self.chart
                        .time_scale
                        .reset(self.bar_store.len(), f64::from(menu.plot_width));
                    self.chart.follow_realtime = true;
                }
            }
            ChartContextMenuAction::Drawing(id, action) => {
                self.apply_drawing_action(id, action);
            }
        }
    }

    fn show_study_legend_rows(&mut self, ctx: &egui::Context, chart: &ChartUiResponse) {
        let entries = study_legend_entries(&self.study_instances);
        let mut pane_counts = Vec::<(PaneId, usize)>::new();
        let mut actions = Vec::new();

        for entry in entries {
            let Some(pane_rect) = chart.pane_rect(entry.pane) else {
                continue;
            };
            let row_index = next_study_legend_row_index(&mut pane_counts, entry.pane);
            let position = study_legend_row_position(pane_rect, row_index);
            if let Some(action) = show_study_legend_entry(ctx, chart.outer, &entry, position) {
                actions.push(action);
            }
        }

        for action in actions {
            self.handle_study_legend_action(action);
        }
    }

    fn handle_study_legend_action(&mut self, action: StudyLegendAction) {
        match action {
            StudyLegendAction::Settings(id) | StudyLegendAction::More(id) => {
                self.selected_study_instance_id = Some(id);
                self.study_settings_instance_id = Some(id);
                self.study_settings_tab = StudySettingsTab::Inputs;
            }
            StudyLegendAction::ToggleVisibility(_) | StudyLegendAction::Delete(_) => {
                let mut instances = self.study_instances.clone();
                if apply_study_legend_action(&mut instances, action) {
                    self.apply_study_instances_with_undo(instances);
                }
            }
        }
    }

    fn show_search_palette(&mut self, ctx: &egui::Context) {
        if !self.search_open {
            self.search_palette_rect = None;
            return;
        }

        self.refresh_search_results_if_ready(ctx);
        let mut open = true;
        let mut selected = None;
        let mut submit_first = false;
        let mut cancel = false;

        let window_response = egui::Window::new("Symbol Search")
            .id(egui::Id::new("symbol_search_palette"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .default_width(520.0)
            .open(&mut open)
            .show(ctx, |ui| {
                let edit = egui::TextEdit::singleline(&mut self.search_input)
                    .desired_width(ui.available_width())
                    .hint_text("Ticker or symbol");
                let mut output = edit.show(ui);
                let response = output.response;
                if self.focus_search {
                    response.request_focus();
                    let end = egui::text::CCursor::new(self.search_input.chars().count());
                    let range = if self.select_search_on_focus {
                        egui::text::CCursorRange::two(egui::text::CCursor::default(), end)
                    } else {
                        egui::text::CCursorRange::one(end)
                    };
                    output.state.cursor.set_char_range(Some(range));
                    output.state.store(ctx, response.id);
                    self.focus_search = false;
                    self.select_search_on_focus = false;
                }
                if response.changed() {
                    self.search_last_edited = Some(Instant::now());
                    ctx.request_repaint_after(Duration::from_millis(SEARCH_DEBOUNCE_MS));
                }

                ui.add_space(6.0);
                let mut filter_changed = false;
                ui.horizontal(|ui| {
                    ui.label("Source");
                    for filter in [
                        SearchSourceFilter::All,
                        SearchSourceFilter::Source(MarketSource::YFinance),
                        SearchSourceFilter::Source(MarketSource::Massive),
                        SearchSourceFilter::Source(MarketSource::Binance),
                        SearchSourceFilter::Source(MarketSource::CoinGecko),
                    ] {
                        filter_changed |= ui
                            .selectable_value(
                                &mut self.search_source_filter,
                                filter,
                                filter.label(),
                            )
                            .changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Type");
                    for filter in [
                        SearchTypeFilter::All,
                        SearchTypeFilter::Stock,
                        SearchTypeFilter::Futures,
                        SearchTypeFilter::Crypto,
                    ] {
                        filter_changed |= ui
                            .selectable_value(&mut self.search_type_filter, filter, filter.label())
                            .changed();
                    }
                });
                if filter_changed {
                    self.refresh_search_results();
                }

                ui.add_space(6.0);
                if self.search_results.is_empty() {
                    ui.label(if self.search_input.trim().is_empty() {
                        "Enter a symbol"
                    } else {
                        "No matching source/type"
                    });
                } else {
                    for candidate in self.search_results.iter().cloned() {
                        let label = format!(
                            "{}  {}  {}",
                            candidate.symbol,
                            candidate.source.label(),
                            symbol_type_label(candidate.symbol_type)
                        );
                        if ui.button(label).clicked() {
                            selected = Some(candidate);
                        }
                    }
                }

                submit_first = ui.input(|input| input.key_pressed(egui::Key::Enter));
                cancel = ui.input(|input| input.key_pressed(egui::Key::Escape));
            });
        self.search_palette_rect = window_response
            .as_ref()
            .map(|response| response.response.rect);

        if !open || cancel {
            self.cancel_search_palette();
            return;
        }
        if submit_first {
            selected = self.best_search_candidate();
        }
        if let Some(candidate) = selected {
            self.submit_search_candidate(ctx, candidate);
        }
    }

    fn show_timeframe_input_palette(&mut self, ctx: &egui::Context) {
        if !self.timeframe_input_open {
            self.timeframe_input_rect = None;
            return;
        }

        let mut open = true;
        let mut submit = false;
        let mut cancel = false;
        let candidate = parse_timeframe_input(&self.timeframe_input);
        let preview = timeframe_input_preview(candidate, &self.timeframe_input);

        let window_response = egui::Window::new("Change interval")
            .id(egui::Id::new("timeframe_input_palette"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .default_width(360.0)
            .open(&mut open)
            .show(ctx, |ui| {
                let edit = egui::TextEdit::singleline(&mut self.timeframe_input)
                    .desired_width(ui.available_width())
                    .hint_text("Interval");
                let mut output = edit.show(ui);
                let response = output.response;
                if self.focus_timeframe_input {
                    response.request_focus();
                    let end = egui::text::CCursor::new(self.timeframe_input.chars().count());
                    output
                        .state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::one(end)));
                    output.state.store(ctx, response.id);
                    self.focus_timeframe_input = false;
                }
                ui.add_space(6.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new(preview).size(16.0).color(
                        if candidate.and_then(|candidate| candidate.interval).is_some() {
                            egui::Color32::from_rgb(180, 184, 192)
                        } else {
                            egui::Color32::from_rgb(232, 178, 88)
                        },
                    ));
                });
                submit = ui.input(|input| input.key_pressed(egui::Key::Enter));
                cancel = ui.input(|input| input.key_pressed(egui::Key::Escape));
            });
        self.timeframe_input_rect = window_response
            .as_ref()
            .map(|response| response.response.rect);

        if !open || cancel {
            self.cancel_timeframe_input();
            return;
        }
        if submit {
            if let Some(interval) = candidate.and_then(|candidate| candidate.interval) {
                self.cancel_timeframe_input();
                self.apply_timeframe_with_undo(ctx, interval);
            } else {
                self.status = format!("Unsupported interval: {}", self.timeframe_input.trim());
                self.status_is_error = true;
            }
        }
    }

    fn refresh_search_results_if_ready(&mut self, ctx: &egui::Context) {
        if self.search_results_query == self.search_input {
            return;
        }
        if let Some(last_edited) = self.search_last_edited {
            let debounce = Duration::from_millis(SEARCH_DEBOUNCE_MS);
            if last_edited.elapsed() < debounce {
                ctx.request_repaint_after(debounce - last_edited.elapsed());
                return;
            }
        }
        self.refresh_search_results();
    }

    fn refresh_search_results(&mut self) {
        self.search_results = search_candidates(
            &self.search_input,
            self.search_source_filter,
            self.search_type_filter,
        );
        self.search_results_query = self.search_input.clone();
        self.search_last_edited = None;
    }

    fn best_search_candidate(&self) -> Option<SearchCandidate> {
        best_search_candidate(
            &self.search_input,
            self.market_source,
            self.search_source_filter,
            self.search_type_filter,
        )
    }

    fn submit_search_candidate(&mut self, ctx: &egui::Context, candidate: SearchCandidate) {
        self.market_source = candidate.source;
        self.search_input = candidate.symbol;
        self.submit_search(ctx);
    }

    fn cancel_search_palette(&mut self) {
        self.search_open = false;
        self.search_palette_rect = None;
        self.select_search_on_focus = false;
        self.search_input = self.source_label.clone();
        self.search_last_edited = None;
    }

    fn cancel_timeframe_input(&mut self) {
        self.timeframe_input_open = false;
        self.timeframe_input.clear();
        self.focus_timeframe_input = false;
        self.timeframe_input_rect = None;
    }

    fn handle_search_palette_chart_dismiss(
        &mut self,
        ctx: &egui::Context,
        chart: &ChartUiResponse,
    ) -> bool {
        let should_cancel = ctx.input(|input| {
            search_palette_chart_click_should_cancel(
                self.search_open,
                self.search_palette_rect,
                chart.outer,
                input.pointer.primary_clicked(),
                input.pointer.interact_pos(),
            )
        });
        if !should_cancel {
            return false;
        }

        self.cancel_search_palette();
        ctx.request_repaint();
        true
    }

    fn handle_floating_panel_chart_dismiss(
        &mut self,
        ctx: &egui::Context,
        chart: &ChartUiResponse,
    ) -> bool {
        let dismissible_panel_open = self.indicators_open
            || self.chart_settings_open
            || self.fib_settings_drawing_id.is_some()
            || self.position_settings_drawing_id.is_some()
            || self.timeframe_input_open;
        let blocking_panel_open = dismissible_panel_open || self.object_tree_open;
        let blocking_panel_rects = [
            self.indicator_panel_rect,
            self.chart_settings_panel_rect,
            self.object_tree_panel_rect,
            self.fib_settings_panel_rect,
            self.position_settings_panel_rect,
            self.timeframe_input_rect,
        ];
        let popup_open = egui::Popup::is_any_open(ctx);
        let should_block = ctx.input(|input| {
            floating_panels_chart_click_should_block_chart_input(
                blocking_panel_open,
                &blocking_panel_rects,
                chart.outer,
                input.pointer.primary_clicked(),
                input.pointer.interact_pos(),
                popup_open,
            )
        });
        if should_block {
            return true;
        }

        let should_cancel = ctx.input(|input| {
            floating_panels_chart_click_should_cancel(
                dismissible_panel_open,
                &[
                    self.indicator_panel_rect,
                    self.chart_settings_panel_rect,
                    self.fib_settings_panel_rect,
                    self.position_settings_panel_rect,
                    self.timeframe_input_rect,
                ],
                chart.outer,
                input.pointer.primary_clicked(),
                input.pointer.interact_pos(),
            )
        });
        if !should_cancel {
            return false;
        }

        self.indicators_open = false;
        self.indicator_panel_rect = None;
        self.chart_settings_open = false;
        self.chart_settings_panel_rect = None;
        self.fib_settings_drawing_id = None;
        self.fib_settings_panel_rect = None;
        self.position_settings_drawing_id = None;
        self.position_settings_panel_rect = None;
        self.cancel_timeframe_input();
        ctx.request_repaint();
        true
    }

    fn show_indicator_panel(&mut self, ctx: &egui::Context) {
        if !self.indicators_open {
            self.indicator_panel_rect = None;
            return;
        }

        let mut open = true;
        let mut instances = self.study_instances.clone();
        let mut remove_index = None;
        let mut settings_id = None;
        let mut add_configs = Vec::new();
        let mut changed = false;
        let selected_study_instance_id = self.selected_study_instance_id;

        let window_response = egui::Window::new("Indicators")
            .id(egui::Id::new("indicator_config_panel"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .default_width(430.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("On chart")
                        .size(12.0)
                        .color(egui::Color32::from_rgb(162, 168, 176)),
                );
                ui.add_space(4.0);
                if instances.is_empty() {
                    ui.label("No indicators");
                } else {
                    for (index, instance) in instances.iter_mut().enumerate() {
                        let selected = Some(instance.id) == selected_study_instance_id;
                        let action = show_study_instance_row(ui, instance, selected);
                        if action.changed {
                            changed = true;
                        }
                        if action.settings {
                            settings_id = Some(instance.id);
                        }
                        if action.remove {
                            remove_index = Some(index);
                        }
                        ui.add_space(4.0);
                    }
                }

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("Add indicator")
                        .size(12.0)
                        .color(egui::Color32::from_rgb(162, 168, 176)),
                );
                ui.add_space(4.0);
                add_configs.extend(show_add_indicator_choices(ui));
            });
        self.indicator_panel_rect = window_response
            .as_ref()
            .map(|response| response.response.rect);

        if !open {
            self.indicators_open = false;
            self.indicator_panel_rect = None;
        }
        if let Some(id) = settings_id {
            self.selected_study_instance_id = Some(id);
            self.study_settings_instance_id = Some(id);
            self.study_settings_tab = StudySettingsTab::Inputs;
        }
        if let Some(index) = remove_index {
            instances.remove(index);
            changed = true;
        }
        for config in add_configs {
            instances.push(StudyInstance::new(
                self.allocate_study_instance_id(),
                config,
            ));
            changed = true;
        }
        normalize_study_instances(&mut instances);
        if changed && instances != self.study_instances {
            self.apply_study_instances_with_undo(instances);
        }
    }

    fn show_chart_settings_surface(&mut self, ctx: &egui::Context) {
        if !self.chart_settings_open {
            self.chart_settings_panel_rect = None;
            return;
        }

        let mut open = true;
        let mut tab = self.chart_settings_tab;
        let mut theme = self.chart.theme().clone();
        let mut theme_changed = false;
        let mut volume_visible = self.chart.volume_visible();
        let mut chart_preset_name_input = self.chart_preset_name_input.clone();
        let chart_presets_snapshot = self.chart_presets.clone();
        let mut preset_action = None;
        let window_response = egui::Window::new("Chart settings")
            .id(egui::Id::new("chart_settings_panel"))
            .collapsible(false)
            .resizable(false)
            .default_pos(egui::pos2(456.0, TOP_BAR_HEIGHT + 8.0))
            .default_width(380.0)
            .open(&mut open)
            .show(ctx, |ui| {
                if let Some(action) = show_chart_preset_controls(
                    ui,
                    &chart_presets_snapshot,
                    &mut chart_preset_name_input,
                ) {
                    preset_action = Some(action);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(matches!(tab, ChartSettingsTab::Symbol), "Symbol")
                        .clicked()
                    {
                        tab = ChartSettingsTab::Symbol;
                    }
                    if ui
                        .selectable_label(matches!(tab, ChartSettingsTab::Appearance), "Appearance")
                        .clicked()
                    {
                        tab = ChartSettingsTab::Appearance;
                    }
                    if ui
                        .selectable_label(matches!(tab, ChartSettingsTab::Scales), "Scales")
                        .clicked()
                    {
                        tab = ChartSettingsTab::Scales;
                    }
                });
                ui.separator();
                ui.add_space(6.0);

                match tab {
                    ChartSettingsTab::Symbol => {
                        theme_changed |= show_chart_symbol_settings(ui, &mut theme);
                    }
                    ChartSettingsTab::Appearance => {
                        theme_changed |=
                            show_chart_appearance_settings(ui, &mut theme, &mut volume_visible);
                    }
                    ChartSettingsTab::Scales => {
                        show_chart_scale_settings(ui, &mut self.chart);
                    }
                }
            });
        self.chart_preset_name_input = chart_preset_name_input;
        self.chart_settings_panel_rect = window_response
            .as_ref()
            .map(|response| response.response.rect);
        self.chart_settings_tab = tab;
        if let Some(action) = preset_action {
            match action {
                ChartPresetUiAction::SaveAs => {
                    let now = current_unix_timestamp_seconds();
                    self.chart_presets.save_new_preset(
                        &self.chart_preset_name_input,
                        ChartPresetSnapshot::new(theme.clone(), volume_visible),
                        now,
                    );
                    self.save_chart_presets();
                }
                ChartPresetUiAction::UpdateActive => {
                    if let Some(id) = self.chart_presets.active_preset_id.clone() {
                        let now = current_unix_timestamp_seconds();
                        if self.chart_presets.update_preset(
                            &id,
                            ChartPresetSnapshot::new(theme.clone(), volume_visible),
                            now,
                        ) {
                            self.save_chart_presets();
                        }
                    }
                }
                ChartPresetUiAction::DeleteActive => {
                    if let Some(id) = self.chart_presets.active_preset_id.clone()
                        && self.chart_presets.delete_preset(&id)
                    {
                        self.save_chart_presets();
                    }
                }
                ChartPresetUiAction::Apply(id) => {
                    if let Some(snapshot) = self
                        .chart_presets
                        .preset(&id)
                        .map(|preset| preset.snapshot.clone())
                    {
                        if self.chart_presets.set_active_preset(&id) {
                            self.save_chart_presets();
                        }
                        if theme != snapshot.theme {
                            theme = snapshot.theme;
                            theme_changed = true;
                        }
                        volume_visible = snapshot.volume_visible;
                    }
                }
                ChartPresetUiAction::ClearActive => {
                    if self.chart_presets.clear_active_preset() {
                        self.save_chart_presets();
                    }
                }
            }
        }
        if self.chart.volume_visible() != volume_visible {
            self.chart.set_volume_visible(volume_visible);
            self.save_user_settings();
        }
        if theme_changed && self.chart.set_theme(theme) {
            self.chart_renderer
                .set_theme_generation(self.chart.theme_generation());
        }
        if !open {
            self.chart_settings_open = false;
            self.chart_settings_panel_rect = None;
        }
    }

    fn show_fib_settings_surface(&mut self, ctx: &egui::Context) {
        let Some(id) = self.fib_settings_drawing_id else {
            self.fib_settings_panel_rect = None;
            return;
        };
        let Some(drawing) = self.drawings.get(id).cloned() else {
            self.fib_settings_drawing_id = None;
            self.fib_settings_panel_rect = None;
            return;
        };
        if drawing.kind != DrawingKind::FibRetracement {
            self.fib_settings_drawing_id = None;
            self.fib_settings_panel_rect = None;
            return;
        }

        let mut open = true;
        let mut settings = drawing.fib_retracement_settings();
        let mut style = drawing.style.clone();
        let mut anchors = drawing.anchors.clone();
        let locked = drawing.locked;
        let mut settings_changed = false;
        let mut style_changed = false;
        let mut coordinates_changed = false;
        let interval_seconds = self.current_drawing_interval_seconds();
        let mut fib_preset_name_input = self.fib_preset_name_input.clone();
        let fib_presets_snapshot = self.fib_presets.clone();
        let mut preset_action = None;

        let window_response = egui::Window::new("Fib Retracement")
            .id(egui::Id::new(("fib_retracement_settings", id.0)))
            .collapsible(false)
            .resizable(false)
            .default_pos(egui::pos2(404.0, TOP_BAR_HEIGHT + 8.0))
            .default_width(360.0)
            .open(&mut open)
            .show(ctx, |ui| {
                if locked {
                    ui.label(
                        egui::RichText::new("Locked")
                            .size(12.0)
                            .color(egui::Color32::from_rgb(220, 176, 96)),
                    );
                    ui.add_space(4.0);
                }
                ui.add_enabled_ui(!locked, |ui| {
                    let changed = show_fib_retracement_settings_editor(
                        ui,
                        &mut settings,
                        &mut style,
                        &mut anchors,
                        drawing.kind,
                        &self.bar_store,
                        interval_seconds,
                    );
                    settings_changed |= changed.settings;
                    style_changed |= changed.style;
                    coordinates_changed |= changed.coordinates;
                    if let Some(action) = show_fib_preset_controls(
                        ui,
                        &fib_presets_snapshot,
                        &mut fib_preset_name_input,
                        settings.is_valid(),
                    ) {
                        preset_action = Some(action);
                    }
                });
            });

        self.fib_preset_name_input = fib_preset_name_input;
        self.fib_settings_panel_rect = window_response
            .as_ref()
            .map(|response| response.response.rect);

        if !open {
            self.fib_settings_drawing_id = None;
            self.fib_settings_panel_rect = None;
        }

        if let Some(action) = preset_action {
            match action {
                FibPresetUiAction::SaveAs => {
                    let now = current_unix_timestamp_seconds();
                    if self
                        .fib_presets
                        .save_new_preset(&self.fib_preset_name_input, settings.clone(), now)
                        .is_some()
                    {
                        self.save_fib_presets();
                    } else {
                        self.status = "Fib preset could not be saved: invalid settings".to_string();
                        self.status_is_error = true;
                    }
                }
                FibPresetUiAction::UpdateActive => {
                    if let Some(id) = self.fib_presets.active_preset_id.clone() {
                        let now = current_unix_timestamp_seconds();
                        if self.fib_presets.update_preset(&id, settings.clone(), now) {
                            self.save_fib_presets();
                        }
                    }
                }
                FibPresetUiAction::DeleteActive => {
                    if let Some(id) = self.fib_presets.active_preset_id.clone()
                        && self.fib_presets.delete_preset(&id)
                    {
                        self.save_fib_presets();
                    }
                }
                FibPresetUiAction::Apply(id) => {
                    if let Some(preset_settings) = self
                        .fib_presets
                        .preset(&id)
                        .map(|preset| preset.settings.clone())
                    {
                        if self.fib_presets.set_active_preset(&id) {
                            self.save_fib_presets();
                        }
                        if settings != preset_settings {
                            settings = preset_settings;
                            settings_changed = true;
                        }
                    }
                }
                FibPresetUiAction::ClearActive => {
                    if self.fib_presets.clear_active_preset() {
                        self.save_fib_presets();
                    }
                }
            }
        }

        if settings_changed || style_changed || coordinates_changed {
            let generation_before = self.drawings.generation();
            let undo_snapshot = self.app_undo_snapshot();
            let mut error = None;
            let mut templates_changed = false;
            if coordinates_changed
                && let Err(update_error) = self.drawings.replace_anchors(id, anchors)
            {
                error = Some(update_error);
            }
            if error.is_none() && settings_changed {
                if let Err(update_error) = self
                    .drawings
                    .replace_payload(id, Some(DrawingPayload::FibRetracement(settings.clone())))
                {
                    error = Some(update_error);
                } else {
                    templates_changed |= self.drawing_tool_templates.set_payload(
                        DrawingTool::FibRetracement,
                        Some(DrawingPayload::FibRetracement(settings.clone())),
                    );
                }
            }
            if error.is_none() && style_changed {
                match self.drawings.replace_style(id, style.clone()) {
                    Ok(()) => {
                        templates_changed |= self
                            .drawing_tool_templates
                            .set_style(DrawingTool::FibRetracement, style);
                    }
                    Err(update_error) => error = Some(update_error),
                }
            }
            if let Some(error) = error {
                self.status = format!("Fib settings could not be updated: {error:?}");
                self.status_is_error = true;
            } else {
                if templates_changed {
                    self.save_drawing_tool_templates();
                }
                if self.drawings.generation() != generation_before {
                    self.push_app_undo_snapshot_if_changed(undo_snapshot);
                    self.save_current_drawing_document();
                }
            }
        }
    }

    fn show_position_settings_surface(&mut self, ctx: &egui::Context) {
        let Some(id) = self.position_settings_drawing_id else {
            self.position_settings_panel_rect = None;
            return;
        };
        let Some(drawing) = self.drawings.get(id).cloned() else {
            self.position_settings_drawing_id = None;
            self.position_settings_panel_rect = None;
            return;
        };
        if !matches!(
            drawing.kind,
            DrawingKind::LongPosition | DrawingKind::ShortPosition
        ) {
            self.position_settings_drawing_id = None;
            self.position_settings_panel_rect = None;
            return;
        }

        let mut open = true;
        let mut settings = drawing.position_settings();
        let mut style = drawing.style.clone();
        let mut anchors = drawing.anchors.clone();
        let locked = drawing.locked;
        let mut settings_changed = false;
        let mut style_changed = false;
        let mut coordinates_changed = false;
        let interval_seconds = self.current_drawing_interval_seconds();
        let title = match drawing.kind {
            DrawingKind::LongPosition => "Long Position",
            DrawingKind::ShortPosition => "Short Position",
            _ => "Position",
        };

        let window_response = egui::Window::new(title)
            .id(egui::Id::new(("position_settings", id.0)))
            .collapsible(false)
            .resizable(false)
            .default_pos(egui::pos2(404.0, TOP_BAR_HEIGHT + 8.0))
            .default_width(340.0)
            .open(&mut open)
            .show(ctx, |ui| {
                if locked {
                    ui.label(
                        egui::RichText::new("Locked")
                            .size(12.0)
                            .color(egui::Color32::from_rgb(220, 176, 96)),
                    );
                    ui.add_space(4.0);
                }
                ui.add_enabled_ui(!locked, |ui| {
                    let changed = show_position_settings_editor(
                        ui,
                        drawing.kind,
                        &mut settings,
                        &mut style,
                        &mut anchors,
                        &self.bar_store,
                        interval_seconds,
                    );
                    settings_changed |= changed.settings;
                    style_changed |= changed.style;
                    coordinates_changed |= changed.coordinates;
                });
            });

        self.position_settings_panel_rect = window_response
            .as_ref()
            .map(|response| response.response.rect);

        if !open {
            self.position_settings_drawing_id = None;
            self.position_settings_panel_rect = None;
        }

        if settings_changed || style_changed || coordinates_changed {
            let generation_before = self.drawings.generation();
            let undo_snapshot = self.app_undo_snapshot();
            let mut error = None;
            let mut templates_changed = false;
            if coordinates_changed
                && let Some(normalized) = normalize_position_drawing_anchors(
                    drawing.kind,
                    anchors.clone(),
                    &self.bar_store,
                    interval_seconds,
                )
            {
                anchors = normalized;
            }
            if coordinates_changed
                && let Err(update_error) = self.drawings.replace_anchors(id, anchors)
            {
                error = Some(update_error);
            }
            if error.is_none() && settings_changed {
                let payload = Some(DrawingPayload::Position(settings));
                if let Err(update_error) = self.drawings.replace_payload(id, payload.clone()) {
                    error = Some(update_error);
                } else if let Some(tool) = drawing_tool_for_kind(drawing.kind) {
                    templates_changed |= self.drawing_tool_templates.set_payload(tool, payload);
                }
            }
            if error.is_none() && style_changed {
                match self.drawings.replace_style(id, style.clone()) {
                    Ok(()) => {
                        if let Some(tool) = drawing_tool_for_kind(drawing.kind) {
                            templates_changed |= self.drawing_tool_templates.set_style(tool, style);
                        }
                    }
                    Err(update_error) => error = Some(update_error),
                }
            }
            if let Some(error) = error {
                self.status = format!("Position settings could not be updated: {error:?}");
                self.status_is_error = true;
            } else {
                if templates_changed {
                    self.save_drawing_tool_templates();
                }
                if self.drawings.generation() != generation_before {
                    self.push_app_undo_snapshot_if_changed(undo_snapshot);
                    self.save_current_drawing_document();
                }
            }
        }
    }

    fn show_object_tree_panel(&mut self, ctx: &egui::Context) {
        if !self.object_tree_open {
            self.object_tree_panel_rect = None;
            return;
        }

        let symbol_key = self.current_drawing_symbol_key();
        let entries = drawing_object_tree_entries(&self.drawings, &symbol_key);
        let selected = self.selected_drawing_id;
        let mut action = None;
        let mut open = true;

        let window_response = egui::Window::new("Drawing Object Tree")
            .id(egui::Id::new("drawing_object_tree_panel"))
            .collapsible(false)
            .resizable(true)
            .default_pos(egui::pos2(560.0, TOP_BAR_HEIGHT + 8.0))
            .default_width(380.0)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("Drawings")
                        .size(12.0)
                        .color(egui::Color32::from_rgb(162, 168, 176)),
                );
                ui.add_space(4.0);

                if entries.is_empty() {
                    ui.label("No drawings for this symbol");
                    return;
                }

                egui::ScrollArea::vertical()
                    .max_height(520.0)
                    .show(ui, |ui| {
                        let mut current_pane = None;
                        for entry in &entries {
                            if current_pane != Some(entry.pane) {
                                if current_pane.is_some() {
                                    ui.add_space(6.0);
                                }
                                current_pane = Some(entry.pane);
                                ui.label(
                                    egui::RichText::new(drawing_object_tree_pane_label(entry.pane))
                                        .size(12.0)
                                        .color(egui::Color32::from_rgb(162, 168, 176)),
                                );
                                ui.add_space(2.0);
                            }

                            let can_group = drawing_object_tree_can_group_with_selected(
                                &self.drawings,
                                selected,
                                entry.id,
                            );
                            if let Some(row_action) =
                                show_drawing_object_tree_row(ui, entry, selected, can_group)
                            {
                                action = Some(row_action);
                            }
                            ui.add_space(4.0);
                        }
                    });
            });
        self.object_tree_panel_rect = window_response
            .as_ref()
            .map(|response| response.response.rect);

        if !open {
            self.object_tree_open = false;
            self.object_tree_panel_rect = None;
        }
        if let Some(action) = action {
            let generation_before = self.drawings.generation();
            let undo_snapshot = self.app_undo_snapshot();
            self.apply_drawing_object_tree_action(action);
            if self.drawings.generation() != generation_before {
                self.push_app_undo_snapshot_if_changed(undo_snapshot);
                self.save_current_drawing_document();
                ctx.request_repaint();
            }
        }
    }

    fn apply_drawing_object_tree_action(&mut self, action: DrawingObjectTreeAction) {
        match action {
            DrawingObjectTreeAction::Select(id) => {
                let symbol_key = self.current_drawing_symbol_key();
                if self
                    .drawings
                    .get(id)
                    .is_some_and(|drawing| drawing_object_tree_can_select(drawing, &symbol_key))
                {
                    self.selected_drawing_id = Some(id);
                    self.hovered_drawing_id = Some(id);
                    self.hovered_drawing_hit = None;
                }
            }
            DrawingObjectTreeAction::ToggleVisible(id) => {
                let Some(visible) = self.drawings.get(id).map(|drawing| drawing.visible) else {
                    self.status =
                        format!("Drawing visibility could not be changed: missing {id:?}");
                    self.status_is_error = true;
                    return;
                };
                if self.drawings.set_visible(id, !visible) && visible {
                    self.clear_drawing_interaction_state(id);
                }
            }
            DrawingObjectTreeAction::ToggleLock(id) => {
                let Some(locked) = self.drawings.get(id).map(|drawing| drawing.locked) else {
                    self.status = format!("Drawing lock could not be changed: missing {id:?}");
                    self.status_is_error = true;
                    return;
                };
                self.drawings.set_locked(id, !locked);
                if !locked {
                    self.drawing_drag = None;
                    if self
                        .text_editing
                        .as_ref()
                        .is_some_and(|editing| editing.drawing_id == id)
                    {
                        self.text_editing = None;
                    }
                }
            }
            DrawingObjectTreeAction::Delete(id) => match self.drawings.remove(id) {
                Ok(()) => self.clear_drawing_interaction_state(id),
                Err(error) => {
                    self.status = format!("Drawing could not be deleted: {error:?}");
                    self.status_is_error = true;
                }
            },
            DrawingObjectTreeAction::BringForward(id) => {
                self.apply_drawing_object_tree_z_action("moved forward", |drawings| {
                    drawings.bring_forward(id)
                });
            }
            DrawingObjectTreeAction::SendBackward(id) => {
                self.apply_drawing_object_tree_z_action("moved backward", |drawings| {
                    drawings.send_backward(id)
                });
            }
            DrawingObjectTreeAction::BringToFront(id) => {
                self.apply_drawing_object_tree_z_action("moved to front", |drawings| {
                    drawings.bring_to_front(id)
                });
            }
            DrawingObjectTreeAction::SendToBack(id) => {
                self.apply_drawing_object_tree_z_action("moved to back", |drawings| {
                    drawings.send_to_back(id)
                });
            }
            DrawingObjectTreeAction::GroupWithSelected(id) => {
                let Some(selected) = self.selected_drawing_id else {
                    return;
                };
                if !drawing_object_tree_can_group_with_selected(&self.drawings, Some(selected), id)
                {
                    self.status = "Drawings cannot be grouped".to_string();
                    self.status_is_error = true;
                    return;
                }
                match self.drawings.create_group(vec![selected, id]) {
                    Ok(_) => {
                        self.status = "Drawings grouped".to_string();
                        self.status_is_error = false;
                    }
                    Err(error) => {
                        self.status = format!("Drawings could not be grouped: {error:?}");
                        self.status_is_error = true;
                    }
                }
            }
            DrawingObjectTreeAction::Ungroup(group_id) => match self.drawings.ungroup(group_id) {
                Ok(_) => {
                    self.status = "Drawings ungrouped".to_string();
                    self.status_is_error = false;
                }
                Err(error) => {
                    self.status = format!("Drawings could not be ungrouped: {error:?}");
                    self.status_is_error = true;
                }
            },
        }
    }

    fn apply_drawing_object_tree_z_action(
        &mut self,
        label: &'static str,
        apply: impl FnOnce(&mut DrawingStore) -> Result<bool, DrawingEditError>,
    ) {
        match apply(&mut self.drawings) {
            Ok(true) => {
                self.status = format!("Drawing {label}");
                self.status_is_error = false;
            }
            Ok(false) => {}
            Err(error) => {
                self.status = format!("Drawing could not be reordered: {error:?}");
                self.status_is_error = true;
            }
        }
    }

    fn show_drawing_toolbar(&mut self, ui: &mut egui::Ui) {
        let desired = egui::vec2(DRAWING_TOOL_RAIL_WIDTH, ui.available_height().max(160.0));
        let (rail_rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
        let painter = ui.painter_at(rail_rect);
        painter.rect_filled(rail_rect, 0.0, egui::Color32::from_rgb(19, 19, 19));
        painter.line_segment(
            [
                egui::pos2(rail_rect.right() - 1.0, rail_rect.top()),
                egui::pos2(rail_rect.right() - 1.0, rail_rect.bottom()),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(45, 45, 45)),
        );

        if self
            .active_drawing_tool
            .is_some_and(|tool| !drawing_tool_can_activate(&self.drawing_tool_policy, tool))
        {
            self.cancel_drawing_creation();
        }

        let toolbar_rect = drawing_toolbar_rect_from_rail(rail_rect);
        ui.scope_builder(egui::UiBuilder::new().max_rect(toolbar_rect), |ui| {
            ui.add_space(DRAWING_TOOLBAR_MARGIN);
            ui.vertical_centered(|ui| {
                let favorite_tools = drawing_toolbar_tools(
                    &self.drawing_tool_policy,
                    DrawingToolbarToolGroup::Favorites,
                );
                let main_tools =
                    drawing_toolbar_tools(&self.drawing_tool_policy, DrawingToolbarToolGroup::Main);
                if self.show_drawing_tool_group(ui, &favorite_tools) {
                    ui.separator();
                }
                self.show_drawing_tool_group(ui, &main_tools);
                if !favorite_tools.is_empty() || !main_tools.is_empty() {
                    ui.separator();
                }
                if show_measure_tool_button(ui, self.measure_active) {
                    self.cancel_drawing_creation();
                    self.drawing_drag = None;
                    self.active_drawing_tool = None;
                    self.measure_active = !self.measure_active;
                    if !self.measure_active {
                        self.measure_state = None;
                    }
                }
                ui.separator();
                if show_drawing_tool_policy_menu(
                    ui,
                    &mut self.drawing_tool_policy,
                    &mut self.drawing_tool_policy_menu_open,
                ) {
                    if self.active_drawing_tool.is_some_and(|tool| {
                        !drawing_tool_can_activate(&self.drawing_tool_policy, tool)
                    }) {
                        self.cancel_drawing_creation();
                    }
                    self.save_user_settings();
                }
                ui.separator();
                let modifier_inverts_magnet = ui.ctx().input(|input| {
                    drawing_magnet_modifier(input.modifiers)
                        && drawing_magnet_modifier_should_apply(
                            self.active_drawing_tool.is_some(),
                            self.drawing_creation.is_some(),
                            self.drawing_drag.is_some(),
                        )
                });
                show_drawing_magnet_control(
                    ui,
                    &mut self.drawing_magnet_mode,
                    modifier_inverts_magnet,
                    &mut self.drawing_magnet_menu_open,
                    &mut self.drawing_magnet_hold_started_at,
                    &mut self.drawing_magnet_hold_opened,
                );
                ui.separator();
                let symbol_key = self.current_drawing_symbol_key();
                let has_drawings = self
                    .drawings
                    .drawings()
                    .iter()
                    .any(|drawing| drawing_matches_symbol_key(drawing, &symbol_key));
                let has_visible_drawings = self.drawings.drawings().iter().any(|drawing| {
                    drawing.visible && drawing_matches_symbol_key(drawing, &symbol_key)
                });
                if show_drawing_visibility_action_button(ui, has_drawings, has_visible_drawings) {
                    self.set_current_symbol_drawings_visible(!has_visible_drawings);
                }
                if show_drawing_delete_action_button(ui, has_drawings) {
                    self.delete_current_symbol_drawings();
                }
            });
        });
    }

    fn show_drawing_tool_group(&mut self, ui: &mut egui::Ui, tools: &[DrawingTool]) -> bool {
        let mut shown_any = false;
        for &tool in tools {
            let response = show_drawing_tool_button(
                ui,
                tool,
                self.active_drawing_tool,
                self.drawing_tool_policy.access(tool),
                self.drawing_tool_policy.is_favorite(tool),
            );
            if response.clicked() && drawing_tool_can_activate(&self.drawing_tool_policy, tool) {
                let was_active = self.active_drawing_tool == Some(tool);
                self.cancel_drawing_creation();
                self.drawing_drag = None;
                if was_active {
                    self.active_drawing_tool = None;
                } else {
                    self.active_drawing_tool = Some(tool);
                    self.measure_active = false;
                    self.measure_state = None;
                }
            }
            shown_any = true;
        }
        shown_any
    }

    fn show_chart_utility_rail(&mut self, ui: &mut egui::Ui) {
        let desired = egui::vec2(CHART_UTILITY_RAIL_WIDTH, ui.available_height().max(160.0));
        let (rail_rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
        let painter = ui.painter_at(rail_rect);
        painter.rect_filled(rail_rect, 0.0, egui::Color32::from_rgb(19, 19, 19));
        painter.line_segment(
            [
                egui::pos2(rail_rect.left(), rail_rect.top()),
                egui::pos2(rail_rect.left(), rail_rect.bottom()),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(45, 45, 45)),
        );

        ui.scope_builder(
            egui::UiBuilder::new().max_rect(rail_rect.shrink2(egui::vec2(6.0, 8.0))),
            |ui| {
                ui.vertical_centered(|ui| {
                    let objects_response = show_toolbar_toggle(ui, "O", self.object_tree_open, 0.9)
                        .on_hover_text("Drawing object tree");
                    if objects_response.clicked() && !self.object_tree_open {
                        self.object_tree_open = true;
                    }
                });
            },
        );
    }

    fn show_selected_drawing_toolbar(&mut self, ctx: &egui::Context, chart: &ChartUiResponse) {
        if !self.selected_drawing_toolbar_visible() {
            self.selected_drawing_toolbar_drag_start = None;
            return;
        }
        let Some(id) = self.selected_drawing_id else {
            return;
        };
        let Some(drawing) = self.drawings.get(id).cloned() else {
            self.selected_drawing_id = None;
            return;
        };
        if !is_style_toolbar_drawing(drawing.kind) {
            return;
        }

        let mut style = drawing.style.clone();
        let mut style_changed = false;
        let mut action = None;
        let rect = selected_drawing_toolbar_rect(chart, self.selected_drawing_toolbar_position);
        if self.selected_drawing_toolbar_position.is_some() {
            self.selected_drawing_toolbar_position = Some(rect.min);
        }
        let mut toolbar_position = self.selected_drawing_toolbar_position;
        let mut toolbar_drag_start = self.selected_drawing_toolbar_drag_start;
        let (
            toolbar_primary_pressed,
            toolbar_primary_down,
            toolbar_primary_released,
            toolbar_press_origin,
            toolbar_drag_delta,
            toolbar_hover_pos,
        ) = ctx.input(|input| {
            (
                input.pointer.primary_pressed(),
                input.pointer.primary_down(),
                input.pointer.primary_released(),
                input.pointer.press_origin(),
                input.pointer.total_drag_delta(),
                input.pointer.hover_pos(),
            )
        });
        if toolbar_primary_pressed
            && toolbar_press_origin.is_some_and(|origin| rect.contains(origin))
        {
            toolbar_drag_start = Some(rect.min);
        }
        if let Some(start) = toolbar_drag_start {
            if let Some(delta) = toolbar_drag_delta {
                toolbar_position = Some(clamp_selected_drawing_toolbar_position(
                    start + delta,
                    chart.outer,
                    rect.size(),
                ));
            }
            let cursor = if toolbar_primary_down || toolbar_primary_released {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            };
            ctx.set_cursor_icon(cursor);
        } else if toolbar_hover_pos.is_some_and(|position| rect.contains(position)) {
            ctx.set_cursor_icon(egui::CursorIcon::Grab);
        }
        if !toolbar_primary_down {
            toolbar_drag_start = None;
        }
        egui::Area::new(egui::Id::new("rustchart_selected_drawing_toolbar"))
            .order(chart_local_overlay_order())
            .fixed_pos(rect.min)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_unmultiplied(32, 32, 32, 240))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(68, 68, 68)))
                    .corner_radius(4.0)
                    .inner_margin(egui::Margin::symmetric(7, 4))
                    .show(ui, |ui| {
                        ui.set_width(rect.width());
                        ui.set_height(rect.height() - 8.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(5.0, 0.0);
                            ui.add_enabled_ui(!drawing.locked, |ui| {
                                style_changed |=
                                    show_drawing_color_menu(ui, drawing.kind, &mut style);
                                if drawing.kind == DrawingKind::Rectangle {
                                    style_changed |= show_drawing_fill_color_menu(ui, &mut style);
                                }
                                if is_line_style_drawing(drawing.kind) {
                                    style_changed |= show_drawing_width_menu(ui, &mut style);
                                    style_changed |= show_drawing_line_style_menu(ui, &mut style);
                                }
                            });
                            if drawing_kind_is_text_editable(drawing.kind)
                                && show_selected_drawing_action_button(
                                    ui,
                                    "Text",
                                    "Edit text",
                                    !drawing.locked,
                                )
                            {
                                action = Some(DrawingAction::EditText);
                            }
                            if let Some(settings_label) =
                                drawing_settings_action_label(drawing.kind)
                                && show_selected_drawing_action_button(
                                    ui,
                                    drawing_settings_toolbar_label(drawing.kind),
                                    settings_label,
                                    true,
                                )
                            {
                                action = Some(DrawingAction::OpenSettings);
                            }
                            ui.separator();
                            if show_selected_drawing_action_button(
                                ui,
                                drawing_lock_toolbar_label(drawing.locked),
                                drawing_lock_action_label(drawing.locked),
                                true,
                            ) {
                                action = Some(DrawingAction::ToggleLock);
                            }
                            if show_selected_drawing_action_button(
                                ui,
                                "Hide",
                                "Hide this drawing",
                                true,
                            ) {
                                action = Some(DrawingAction::Hide);
                            }
                            if show_selected_drawing_action_button(
                                ui,
                                "Del",
                                "Delete this drawing",
                                !drawing.locked,
                            ) {
                                action = Some(DrawingAction::Delete);
                            }
                            ui.separator();
                            ui.add_space(2.0);
                            if drawing.locked {
                                ui.label(
                                    egui::RichText::new("Locked")
                                        .size(11.0)
                                        .color(egui::Color32::from_rgb(220, 176, 96)),
                                );
                            } else {
                                ui.label(
                                    egui::RichText::new(drawing_kind_label(drawing.kind))
                                        .size(11.0)
                                        .color(egui::Color32::from_rgb(170, 174, 182)),
                                );
                            }
                        });
                    });
            });

        self.selected_drawing_toolbar_position = toolbar_position;
        self.selected_drawing_toolbar_drag_start = toolbar_drag_start;
        if toolbar_position.is_some_and(|position| position != rect.min) {
            ctx.request_repaint();
        }
        let generation_before = self.drawings.generation();
        if style_changed {
            let undo_snapshot = self.app_undo_snapshot();
            match self.drawings.replace_style(id, style.clone()) {
                Ok(()) => {
                    self.push_app_undo_snapshot_if_changed(undo_snapshot);
                    if let Some(tool) = drawing_tool_for_kind(drawing.kind) {
                        self.drawing_tool_templates.set_style(tool, style);
                        self.save_drawing_tool_templates();
                    }
                }
                Err(error) => {
                    self.status = format!("Drawing style could not be updated: {error:?}");
                    self.status_is_error = true;
                }
            }
        }
        self.save_drawings_if_generation_changed(generation_before);
        if let Some(action) = action {
            self.apply_drawing_action(id, action);
        }
    }

    fn apply_drawing_action(&mut self, id: DrawingId, action: DrawingAction) {
        let generation_before = self.drawings.generation();
        let undo_snapshot = self.app_undo_snapshot();
        match apply_drawing_action_to_store(&mut self.drawings, id, action) {
            Ok(DrawingActionEffect::EditText) => {
                self.selected_drawing_id = Some(id);
                self.begin_text_edit(id, false);
            }
            Ok(DrawingActionEffect::OpenSettings) => {
                self.selected_drawing_id = Some(id);
                match self.drawings.get(id).map(|drawing| drawing.kind) {
                    Some(DrawingKind::FibRetracement) => {
                        self.fib_settings_drawing_id = Some(id);
                        self.position_settings_drawing_id = None;
                        self.position_settings_panel_rect = None;
                    }
                    Some(DrawingKind::LongPosition | DrawingKind::ShortPosition) => {
                        self.position_settings_drawing_id = Some(id);
                        self.fib_settings_drawing_id = None;
                        self.fib_settings_panel_rect = None;
                    }
                    _ => {}
                }
            }
            Ok(DrawingActionEffect::LockChanged(locked)) => {
                self.selected_drawing_id = Some(id);
                if locked {
                    self.drawing_drag = None;
                    if self
                        .text_editing
                        .as_ref()
                        .is_some_and(|editing| editing.drawing_id == id)
                    {
                        self.text_editing = None;
                    }
                }
            }
            Ok(DrawingActionEffect::Hidden | DrawingActionEffect::Deleted) => {
                self.clear_drawing_interaction_state(id);
            }
            Ok(DrawingActionEffect::Noop) => {}
            Err(error) => {
                self.status = drawing_action_error_message(action, error);
                self.status_is_error = true;
            }
        }
        self.push_app_undo_snapshot_if_changed(undo_snapshot);
        self.save_drawings_if_generation_changed(generation_before);
    }

    fn clear_drawing_interaction_state(&mut self, id: DrawingId) {
        if self.selected_drawing_id == Some(id) {
            self.selected_drawing_id = None;
            self.selected_drawing_toolbar_drag_start = None;
        }
        if self.fib_settings_drawing_id == Some(id) {
            self.fib_settings_drawing_id = None;
            self.fib_settings_panel_rect = None;
        }
        if self.position_settings_drawing_id == Some(id) {
            self.position_settings_drawing_id = None;
            self.position_settings_panel_rect = None;
        }
        if self.hovered_drawing_id == Some(id) {
            self.hovered_drawing_id = None;
        }
        if self
            .hovered_drawing_hit
            .is_some_and(|hit| hit.drawing_id == id)
        {
            self.hovered_drawing_hit = None;
        }
        if self
            .drawing_drag
            .as_ref()
            .is_some_and(|drag| drag.drawing_id == id)
        {
            self.drawing_drag = None;
        }
        if self
            .text_editing
            .as_ref()
            .is_some_and(|editing| editing.drawing_id == id)
        {
            self.text_editing = None;
        }
    }

    fn clear_all_drawing_interaction_state(&mut self) {
        self.active_drawing_tool = None;
        self.measure_active = false;
        self.measure_state = None;
        self.drawing_creation = None;
        self.drawing_drag = None;
        self.selected_drawing_id = None;
        self.selected_drawing_toolbar_position = None;
        self.selected_drawing_toolbar_drag_start = None;
        self.fib_settings_drawing_id = None;
        self.fib_settings_panel_rect = None;
        self.position_settings_drawing_id = None;
        self.position_settings_panel_rect = None;
        self.hovered_drawing_id = None;
        self.hovered_drawing_hit = None;
        self.text_editing = None;
    }

    fn drawing_hit_at_position(
        &self,
        chart: &ChartUiResponse,
        snapshot: &ChartSnapshot,
        position: egui::Pos2,
    ) -> Option<DrawingHit> {
        let scene_layout = scene_layout_from_chart_response(chart);
        let scene_snapshot = scene_snapshot_from_chart_snapshot(snapshot, &scene_layout);
        let symbol_filter = self.current_drawing_symbol_key();
        hit_test_drawings(DrawingHitTestRequest {
            layout: &scene_layout,
            snapshot: &scene_snapshot,
            drawings: &self.drawings,
            bars: &self.bar_store,
            point: egui_pos_to_scene_point(position),
            tolerance: 5.0,
            symbol_filter: Some(&symbol_filter),
            selected: self.selected_drawing_id,
            hovered: self.hovered_drawing_id,
            interval_seconds: self.current_drawing_interval_seconds(),
        })
    }

    fn selected_drawing_toolbar_visible(&self) -> bool {
        let Some(id) = self.selected_drawing_id else {
            return false;
        };
        let symbol_key = self.current_drawing_symbol_key();
        self.drawings
            .get(id)
            .is_some_and(|drawing| drawing_matches_symbol_key(drawing, &symbol_key))
            && self.active_drawing_tool.is_none()
            && !self.measure_active
            && self.drawing_creation.is_none()
    }

    fn current_drawing_symbol_key(&self) -> String {
        drawing_symbol_key(
            self.active_source.unwrap_or(self.market_source),
            &self.source_label,
        )
    }

    fn current_drawing_interval_seconds(&self) -> Option<i64> {
        self.selected_timeframe.fixed_seconds()
    }

    fn apply_drawing_template(&mut self, id: DrawingId, tool: DrawingTool) {
        let style = self.drawing_tool_templates.style(tool);
        if style != DrawingStyle::default()
            && let Err(error) = self.drawings.replace_style(id, style)
        {
            self.status = format!("Drawing template style could not be applied: {error:?}");
            self.status_is_error = true;
            return;
        }
        let payload = match tool {
            DrawingTool::FibRetracement => self
                .fib_presets
                .active_settings()
                .map(DrawingPayload::FibRetracement)
                .or_else(|| self.drawing_tool_templates.payload(tool)),
            _ => self.drawing_tool_templates.payload(tool),
        };
        if let Some(payload) = payload
            && let Err(error) = self.drawings.replace_payload(id, Some(payload))
        {
            self.status = format!("Drawing template settings could not be applied: {error:?}");
            self.status_is_error = true;
        }
    }

    fn handle_drawing_clipboard_shortcuts(
        &mut self,
        ctx: &egui::Context,
        chart: &ChartUiResponse,
        snapshot: &ChartSnapshot,
    ) -> bool {
        let keyboard_input_active = self.search_open || ctx.text_edit_focused();
        let shortcut = ctx.input(|input| {
            let (copy_event, paste_event) = drawing_clipboard_event_flags(&input.events);
            drawing_clipboard_shortcut(
                self.selected_drawing_id,
                self.drawing_clipboard.is_some(),
                self.text_editing.is_some(),
                keyboard_input_active,
                copy_event || (input.modifiers.command && input.key_pressed(egui::Key::C)),
                paste_event || (input.modifiers.command && input.key_pressed(egui::Key::V)),
            )
        });
        match shortcut {
            Some(DrawingClipboardShortcut::Copy(id)) => {
                self.copy_drawing_to_clipboard(id);
                true
            }
            Some(DrawingClipboardShortcut::Paste) => {
                self.paste_drawing_from_clipboard(ctx, chart, snapshot);
                true
            }
            None => false,
        }
    }

    fn copy_drawing_to_clipboard(&mut self, id: DrawingId) {
        let symbol_key = self.current_drawing_symbol_key();
        let Some(drawing) = self
            .drawings
            .get(id)
            .filter(|drawing| drawing_matches_symbol_key(drawing, &symbol_key))
            .cloned()
        else {
            return;
        };
        self.drawing_clipboard = Some(drawing);
        self.status = "Drawing copied".to_string();
        self.status_is_error = false;
    }

    fn paste_drawing_from_clipboard(
        &mut self,
        ctx: &egui::Context,
        chart: &ChartUiResponse,
        snapshot: &ChartSnapshot,
    ) {
        let Some(mut copy) = self.drawing_clipboard.clone() else {
            return;
        };
        copy.anchors = offset_drawing_anchors_for_paste(
            &copy,
            chart,
            snapshot,
            &self.bar_store,
            self.current_drawing_interval_seconds(),
        )
        .unwrap_or_else(|| copy.anchors.clone());
        let generation_before = self.drawings.generation();
        let undo_snapshot = self.app_undo_snapshot();
        match self.insert_drawing_copy(copy) {
            Ok(id) => {
                self.push_app_undo_snapshot_if_changed(undo_snapshot);
                self.selected_drawing_id = Some(id);
                self.hovered_drawing_id = Some(id);
                self.hovered_drawing_hit = None;
                self.status = "Drawing pasted".to_string();
                self.status_is_error = false;
                self.save_drawings_if_generation_changed(generation_before);
                ctx.request_repaint();
            }
            Err(error) => {
                self.status = format!("Drawing could not be pasted: {error:?}");
                self.status_is_error = true;
            }
        }
    }

    fn insert_drawing_copy(
        &mut self,
        mut drawing: DrawingSpec,
    ) -> Result<DrawingId, chart_core::DrawingValidationError> {
        drawing.symbol = Some(self.current_drawing_symbol_key());
        drawing.group_id = None;
        drawing.z_order = next_drawing_z_order(&self.drawings);
        self.drawings.insert_copy(drawing)
    }

    fn drawing_crosshair_magnet(&self, ctx: &egui::Context) -> Option<ChartCrosshairMagnet> {
        let active = self.active_drawing_tool.is_some()
            || self.drawing_creation.is_some()
            || self.drawing_drag.is_some();
        if !active {
            return None;
        }
        chart_crosshair_magnet(self.effective_drawing_magnet_mode(ctx))
    }

    fn show_text_edit_overlay(
        &mut self,
        ctx: &egui::Context,
        chart: &ChartUiResponse,
        snapshot: &ChartSnapshot,
    ) {
        let Some(editing_snapshot) = self.text_editing.clone() else {
            return;
        };
        let Some(drawing) = self.drawings.get(editing_snapshot.drawing_id).cloned() else {
            self.text_editing = None;
            return;
        };
        let Some(position) = drawing_anchor_overlay_position(
            &drawing,
            chart,
            snapshot,
            &self.bar_store,
            self.current_drawing_interval_seconds(),
        ) else {
            return;
        };

        let mut commit = false;
        let area_position = position - egui::vec2(4.0, 16.0);
        egui::Area::new(egui::Id::new((
            "rustchart_text_edit",
            editing_snapshot.drawing_id.0,
        )))
        .order(egui::Order::Tooltip)
        .fixed_pos(area_position)
        .show(ctx, |ui| {
            let Some(editing) = self.text_editing.as_mut() else {
                return;
            };
            let text_id = ui.id().with(("text_value", editing.drawing_id.0));
            let desired_width = (editing.draft.chars().count() as f32 * 12.0).max(120.0);
            let text_edit = egui::TextEdit::singleline(&mut editing.draft)
                .id(text_id)
                .desired_width(desired_width)
                .font(egui::TextStyle::Heading)
                .hint_text("Add text");
            let mut output = text_edit.show(ui);
            let had_focus_request = editing.focus;
            if had_focus_request {
                output.response.request_focus();
                output
                    .state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::default(),
                        egui::text::CCursor::new(editing.draft.chars().count()),
                    )));
                output.state.store(ctx, output.response.id);
                editing.focus = false;
            }
            if output.response.changed() {
                let value = Some(editing.draft.clone());
                let _ = self.drawings.set_text(editing.drawing_id, value);
            }
            if ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                commit = true;
            }
            let clicked_elsewhere = ui.input(|input| {
                !had_focus_request
                    && input.pointer.primary_clicked()
                    && input
                        .pointer
                        .interact_pos()
                        .is_some_and(|pos| !output.response.rect.expand(10.0).contains(pos))
            });
            if clicked_elsewhere {
                commit = true;
            }
        });
        if commit {
            self.commit_text_edit();
        }
    }

    fn set_current_symbol_drawings_visible(&mut self, visible: bool) {
        let symbol_key = self.current_drawing_symbol_key();
        let ids = self
            .drawings
            .drawings()
            .iter()
            .filter(|drawing| drawing_matches_symbol_key(drawing, &symbol_key))
            .map(|drawing| drawing.id)
            .collect::<Vec<_>>();
        let generation_before = self.drawings.generation();
        let undo_snapshot = self.app_undo_snapshot();
        self.drawings
            .apply_bulk_action(&ids, DrawingBulkAction::SetVisible(visible));
        if !visible {
            self.selected_drawing_id = None;
            self.hovered_drawing_id = None;
            self.drawing_drag = None;
            self.text_editing = None;
        }
        self.push_app_undo_snapshot_if_changed(undo_snapshot);
        self.save_drawings_if_generation_changed(generation_before);
    }

    fn delete_current_symbol_drawings(&mut self) {
        let symbol_key = self.current_drawing_symbol_key();
        let ids = self
            .drawings
            .drawings()
            .iter()
            .filter(|drawing| drawing_matches_symbol_key(drawing, &symbol_key))
            .map(|drawing| drawing.id)
            .collect::<Vec<_>>();
        let generation_before = self.drawings.generation();
        let undo_snapshot = self.app_undo_snapshot();
        let outcomes = self
            .drawings
            .apply_bulk_action(&ids, DrawingBulkAction::Delete);
        let deleted = outcomes
            .iter()
            .filter(|outcome| outcome.status == DrawingBulkActionStatus::Deleted)
            .count();
        let locked = outcomes
            .iter()
            .filter(|outcome| {
                matches!(
                    outcome.status,
                    DrawingBulkActionStatus::Error(DrawingEditError::LockedDrawing(_))
                )
            })
            .count();
        let failed = outcomes
            .iter()
            .filter(|outcome| matches!(outcome.status, DrawingBulkActionStatus::Error(_)))
            .count()
            .saturating_sub(locked);
        if locked > 0 || failed > 0 {
            self.status =
                format!("Deleted {deleted} drawings; skipped {locked} locked and {failed} failed");
            self.status_is_error = true;
        } else if deleted > 0 {
            self.status = format!("Deleted {deleted} drawings");
            self.status_is_error = false;
        }
        self.cancel_drawing_creation();
        self.drawing_drag = None;
        self.selected_drawing_id = None;
        self.hovered_drawing_id = None;
        self.hovered_drawing_hit = None;
        self.text_editing = None;
        self.push_app_undo_snapshot_if_changed(undo_snapshot);
        self.save_drawings_if_generation_changed(generation_before);
    }

    fn handle_drawing_interaction(
        &mut self,
        ctx: &egui::Context,
        chart: &ChartUiResponse,
        snapshot: &ChartSnapshot,
    ) {
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            if let Some(editing) = self.text_editing.take() {
                self.push_app_undo_snapshot_if_changed(editing.undo_snapshot);
                self.save_current_drawing_document();
            }
            self.cancel_drawing_creation();
            self.drawing_drag = None;
            self.measure_state = None;
            return;
        }
        if self.handle_drawing_delete_undo_shortcuts(ctx) {
            return;
        }
        if self.handle_drawing_clipboard_shortcuts(ctx, chart, snapshot) {
            return;
        }
        let egui_wants_keyboard_input = ctx.egui_wants_keyboard_input();
        let delete_shortcut_target = ctx.input(|input| {
            selected_drawing_delete_shortcut_target(
                self.selected_drawing_id,
                self.text_editing.is_some(),
                egui_wants_keyboard_input,
                input.key_pressed(egui::Key::Delete),
                input.key_pressed(egui::Key::Backspace),
            )
        });
        if let Some(id) = delete_shortcut_target {
            self.apply_drawing_action(id, DrawingAction::Delete);
            return;
        }

        let scene_layout = scene_layout_from_chart_response(chart);
        let scene_snapshot = scene_snapshot_from_chart_snapshot(snapshot, &scene_layout);
        let hover = ctx.input(|input| input.pointer.hover_pos());
        let symbol_filter = self.current_drawing_symbol_key();
        self.hovered_drawing_hit = hover.map(egui_pos_to_scene_point).and_then(|point| {
            hit_test_drawings(DrawingHitTestRequest {
                layout: &scene_layout,
                snapshot: &scene_snapshot,
                drawings: &self.drawings,
                bars: &self.bar_store,
                point,
                tolerance: 5.0,
                symbol_filter: Some(&symbol_filter),
                selected: self.selected_drawing_id,
                hovered: self.hovered_drawing_id,
                interval_seconds: self.current_drawing_interval_seconds(),
            })
        });
        self.hovered_drawing_id = self.hovered_drawing_hit.map(|hit| hit.drawing_id);

        if self.selected_drawing_toolbar_drag_start.is_some() {
            return;
        }

        let primary_clicked = ctx.input(|input| input.pointer.primary_clicked());
        let primary_pressed = ctx.input(|input| input.pointer.primary_pressed());
        let primary_down = ctx.input(|input| input.pointer.primary_down());
        let primary_released = ctx.input(|input| input.pointer.primary_released());
        let secondary_clicked = ctx.input(|input| input.pointer.secondary_clicked());
        let position = ctx.input(|input| input.pointer.interact_pos());

        let selected_toolbar_visible = self.selected_drawing_toolbar_visible();
        if position.is_some_and(|position| {
            drawing_controls_contain(
                chart,
                selected_toolbar_visible,
                self.selected_drawing_toolbar_position,
                position,
            )
        }) {
            return;
        }

        if self.active_drawing_tool.is_some() && secondary_clicked {
            self.cancel_drawing_creation();
            return;
        }

        let magnet_mode = self.effective_drawing_magnet_mode(ctx);

        if self.handle_brush_drawing_interaction(
            ctx,
            chart,
            snapshot,
            position,
            primary_pressed,
            primary_down,
            primary_released,
            magnet_mode,
        ) {
            return;
        }

        if primary_pressed
            && let Some(hit) = self.hovered_drawing_hit
            && self.should_begin_text_edit(hit)
        {
            self.begin_text_edit(hit.drawing_id, false);
            return;
        }

        if !primary_clicked && let Some(position) = position {
            if self.update_pending_drawing_preview(ctx, chart, snapshot, position, magnet_mode) {
                return;
            }
        }

        if self.handle_drawing_drag(
            ctx,
            chart,
            snapshot,
            position,
            primary_pressed,
            primary_down,
            primary_released,
            magnet_mode,
        ) {
            return;
        }

        if !primary_clicked {
            return;
        }

        let Some(position) = position else {
            return;
        };

        if let Some(tool) = self.active_drawing_tool {
            let generation_before = self.drawings.generation();
            let had_pending_creation = self.drawing_creation.is_some();
            self.apply_drawing_tool_click(tool, chart, snapshot, position, magnet_mode);
            let drawing_generation_changed = self.drawings.generation() != generation_before;
            let finalized_pending_creation =
                had_pending_creation && self.drawing_creation.is_none();
            if drawing_generation_changed || finalized_pending_creation {
                if self.drawing_creation.is_none() {
                    self.save_current_drawing_document();
                }
                ctx.request_repaint();
            }
        } else if chart.outer.contains(position) {
            if let Some(hit) = self.hovered_drawing_hit
                && self.should_begin_text_edit(hit)
            {
                self.begin_text_edit(hit.drawing_id, false);
                return;
            }
            if selected_drawing_blank_click_should_clear(
                selected_toolbar_visible,
                self.hovered_drawing_id,
                primary_clicked,
                chart.response.contains_pointer(),
            ) {
                self.selected_drawing_id = None;
                return;
            }
            if selected_toolbar_visible
                && self.hovered_drawing_id.is_none()
                && ctx.egui_wants_pointer_input()
            {
                return;
            }
            self.selected_drawing_id = self.hovered_drawing_id;
        }
    }

    fn handle_drawing_delete_undo_shortcuts(&mut self, ctx: &egui::Context) -> bool {
        let egui_wants_keyboard_input = ctx.egui_wants_keyboard_input();
        if ctx.input(|input| {
            app_redo_shortcut(
                self.text_editing.is_some(),
                egui_wants_keyboard_input,
                input.modifiers.command && input.modifiers.shift && input.key_pressed(egui::Key::Z),
                !self.app_redo_stack.is_empty(),
            )
        }) {
            self.redo_last_app_action(ctx);
            ctx.request_repaint();
            return true;
        }
        if ctx.input(|input| {
            app_undo_shortcut(
                self.text_editing.is_some(),
                egui_wants_keyboard_input,
                input.modifiers.command
                    && !input.modifiers.shift
                    && input.key_pressed(egui::Key::Z),
                !self.app_undo_stack.is_empty(),
            )
        }) {
            self.undo_last_app_action(ctx);
            ctx.request_repaint();
            return true;
        }
        let cut_shortcut_target = ctx.input(|input| {
            let cut_event = drawing_cut_event_flag(&input.events);
            selected_drawing_cut_shortcut_target(
                self.selected_drawing_id,
                self.text_editing.is_some(),
                egui_wants_keyboard_input,
                cut_event
                    || input.key_pressed(egui::Key::Cut)
                    || (input.modifiers.command && input.key_pressed(egui::Key::X)),
            )
        });
        if let Some(id) = cut_shortcut_target {
            self.apply_drawing_action(id, DrawingAction::Delete);
            ctx.request_repaint();
            return true;
        }
        false
    }

    fn handle_measure_interaction(
        &mut self,
        ctx: &egui::Context,
        chart: &ChartUiResponse,
        snapshot: &ChartSnapshot,
    ) {
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.measure_active = false;
            self.measure_state = None;
            return;
        }
        if self.handle_drawing_delete_undo_shortcuts(ctx) {
            return;
        }

        let (primary_pressed, primary_down, primary_released, interact_position, hover_position) =
            ctx.input(|input| {
                (
                    input.pointer.primary_pressed(),
                    input.pointer.primary_down(),
                    input.pointer.primary_released(),
                    input.pointer.interact_pos(),
                    input.pointer.hover_pos(),
                )
            });

        if primary_pressed && let Some(position) = interact_position {
            let point = measure_point_at_position(
                chart,
                snapshot,
                &self.bar_store,
                position,
                self.current_drawing_interval_seconds(),
            );
            let handle_target = self.measure_state.and_then(|state| {
                measure_drag_target_at_position(state, chart, snapshot, position)
            });
            let previous_state = self.measure_state;
            let next_state = measure_state_after_click(self.measure_state, point, handle_target);
            if next_state != self.measure_state {
                self.measure_state = next_state;
                if previous_state.is_some_and(measure_state_cleared_by_chart_click)
                    && next_state.is_none()
                {
                    self.measure_active = false;
                }
                ctx.request_repaint();
            }
            return;
        }

        if primary_down
            && let (Some(state), Some(position)) = (self.measure_state, interact_position)
            && let MeasurePhase::Adjusting(drag_target) = state.phase
            && let Some(point) = measure_point_at_position(
                chart,
                snapshot,
                &self.bar_store,
                position,
                self.current_drawing_interval_seconds(),
            )
            && point.pane == state.start.pane
        {
            let next_state = measure_state_after_adjust_drag(state, drag_target, point);
            if Some(next_state) != self.measure_state {
                self.measure_state = Some(next_state);
                ctx.request_repaint();
            }
            return;
        }

        if primary_released && let Some(state) = self.measure_state {
            let next_state = measure_state_after_release(state);
            if next_state != state {
                self.measure_state = Some(next_state);
                ctx.request_repaint();
            }
            return;
        }

        if let Some(position) = hover_position
            && let Some(state) = self.measure_state
            && let Some(point) = measure_point_at_position(
                chart,
                snapshot,
                &self.bar_store,
                position,
                self.current_drawing_interval_seconds(),
            )
        {
            let next_state = measure_state_after_hover(state, point);
            if next_state != state {
                self.measure_state = Some(next_state);
                ctx.request_repaint();
            }
        }
    }

    fn show_measure_overlay(
        &self,
        ctx: &egui::Context,
        chart: &ChartUiResponse,
        snapshot: &ChartSnapshot,
    ) {
        let Some(state) = self.measure_state else {
            return;
        };
        let Some(start) = measure_point_screen_position(state.start, chart, snapshot) else {
            return;
        };
        let Some(current) = measure_point_screen_position(state.current, chart, snapshot) else {
            return;
        };
        let stats = measure_stats(state.start, state.current, &self.bar_store);
        let layer = egui::LayerId::new(
            chart_local_overlay_order(),
            egui::Id::new("rustchart_measure_overlay"),
        );
        let painter = ctx.layer_painter(layer);
        let color = egui::Color32::from_rgb(86, 148, 255);
        let measure_rect = egui::Rect::from_two_pos(start, current);
        painter.rect_filled(
            measure_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(29, 82, 180, 55),
        );
        painter.rect_stroke(
            measure_rect,
            0.0,
            egui::Stroke::new(1.2, color),
            egui::StrokeKind::Inside,
        );
        let center = measure_rect.center();
        paint_measure_arrow_line(
            &painter,
            egui::pos2(start.x, center.y),
            egui::pos2(current.x, center.y),
            color,
        );
        paint_measure_arrow_line(
            &painter,
            egui::pos2(center.x, start.y),
            egui::pos2(center.x, current.y),
            color,
        );
        painter.circle_filled(start, 3.5, color);
        painter.circle_filled(current, 3.5, color);

        let lines = format_measure_stats_lines(stats);
        let font_id = egui::FontId::proportional(12.0);
        let galleys = lines
            .into_iter()
            .map(|line| painter.layout_no_wrap(line, font_id.clone(), egui::Color32::WHITE))
            .collect::<Vec<_>>();
        let text_width = galleys
            .iter()
            .map(|galley| galley.size().x)
            .fold(0.0, f32::max);
        let text_height = galleys.iter().map(|galley| galley.size().y).sum::<f32>()
            + (galleys.len().saturating_sub(1) as f32 * 4.0);
        let size = egui::vec2(text_width + 24.0, text_height + 16.0);
        let label_anchor = egui::pos2(center.x - size.x / 2.0, measure_rect.top() - size.y - 10.0);
        let mut position = label_anchor;
        position = clamp_label_position(position, size, chart.outer);
        let rect = egui::Rect::from_min_size(position, size);
        painter.rect_filled(rect, 7.0, egui::Color32::from_rgb(45, 101, 246));
        let mut y = rect.top() + 8.0;
        for galley in galleys {
            let x = rect.center().x - galley.size().x / 2.0;
            painter.galley(egui::pos2(x, y), galley.clone(), egui::Color32::WHITE);
            y += galley.size().y + 4.0;
        }
    }

    fn effective_drawing_magnet_mode(&self, ctx: &egui::Context) -> DrawingMagnetMode {
        let modifier_held = ctx.input(|input| {
            drawing_magnet_modifier(input.modifiers)
                && drawing_magnet_modifier_should_apply(
                    self.active_drawing_tool.is_some(),
                    self.drawing_creation.is_some(),
                    self.drawing_drag.is_some(),
                )
        });
        effective_drawing_magnet_mode(self.drawing_magnet_mode, modifier_held)
    }

    fn handle_brush_drawing_interaction(
        &mut self,
        ctx: &egui::Context,
        chart: &ChartUiResponse,
        snapshot: &ChartSnapshot,
        position: Option<egui::Pos2>,
        primary_pressed: bool,
        primary_down: bool,
        primary_released: bool,
        magnet_mode: DrawingMagnetMode,
    ) -> bool {
        let brush_creation_active = self
            .drawing_creation
            .as_ref()
            .is_some_and(|pending| pending.tool == DrawingTool::Brush);
        if !brush_creation_active && self.active_drawing_tool != Some(DrawingTool::Brush) {
            return false;
        }

        if primary_released && brush_creation_active {
            self.finish_brush_drawing(chart, snapshot, position, magnet_mode);
            ctx.request_repaint();
            return true;
        }

        if primary_pressed && self.active_drawing_tool == Some(DrawingTool::Brush) {
            if let Some(position) = position {
                self.begin_brush_drawing(chart, snapshot, position);
                ctx.request_repaint();
                return true;
            }
            return false;
        }

        if primary_down && brush_creation_active {
            if let Some(position) = position {
                self.update_brush_drawing(chart, snapshot, position, magnet_mode);
                ctx.request_repaint();
            }
            return true;
        }

        brush_creation_active
    }

    fn begin_brush_drawing(
        &mut self,
        chart: &ChartUiResponse,
        snapshot: &ChartSnapshot,
        position: egui::Pos2,
    ) {
        let tool = DrawingTool::Brush;
        if !drawing_tool_can_activate(&self.drawing_tool_policy, tool) {
            self.cancel_drawing_creation();
            self.status = format!("{} tool is unavailable", tool.tooltip());
            self.status_is_error = true;
            return;
        }
        let Some((pane, _)) = drawing_pane_at(chart, position) else {
            return;
        };
        let Some(transform) = drawing_scene_transform_for_pane(pane, chart, snapshot) else {
            return;
        };
        let Some(anchor) = brush_anchor_at_position(
            transform,
            position,
            &self.bar_store,
            self.current_drawing_interval_seconds(),
        ) else {
            return;
        };

        let undo_snapshot = self.app_undo_snapshot();
        match self.drawings.create_with_symbol(
            tool.kind(),
            pane,
            vec![anchor, anchor],
            Some(self.current_drawing_symbol_key()),
        ) {
            Ok(id) => {
                self.apply_drawing_template(id, tool);
                self.selected_drawing_id = Some(id);
                self.hovered_drawing_id = Some(id);
                self.drawing_creation = Some(DrawingCreationState {
                    tool,
                    pane,
                    first_anchor: anchor,
                    preview_id: Some(id),
                    undo_snapshot,
                });
            }
            Err(error) => {
                self.status = format!("Brush drawing could not be started: {error:?}");
                self.status_is_error = true;
            }
        }
    }

    fn update_brush_drawing(
        &mut self,
        chart: &ChartUiResponse,
        snapshot: &ChartSnapshot,
        position: egui::Pos2,
        magnet_mode: DrawingMagnetMode,
    ) {
        let Some(pending) = self.drawing_creation.clone() else {
            return;
        };
        if pending.tool != DrawingTool::Brush {
            return;
        }
        if !drawing_tool_can_activate(&self.drawing_tool_policy, pending.tool) {
            self.cancel_drawing_creation();
            return;
        }
        let Some(preview_id) = pending.preview_id else {
            return;
        };
        let Some(anchor) = brush_anchor_for_position(
            pending.pane,
            position,
            chart,
            snapshot,
            &self.bar_store,
            magnet_mode,
            self.current_drawing_interval_seconds(),
        ) else {
            return;
        };
        let Some(current) = self.drawings.get(preview_id) else {
            return;
        };
        let Some(next_anchors) = brush_sampled_anchors(
            &current.anchors,
            anchor,
            pending.pane,
            chart,
            snapshot,
            &self.bar_store,
            self.current_drawing_interval_seconds(),
        ) else {
            return;
        };
        self.replace_drawing_anchors(preview_id, next_anchors);
    }

    fn finish_brush_drawing(
        &mut self,
        chart: &ChartUiResponse,
        snapshot: &ChartSnapshot,
        position: Option<egui::Pos2>,
        magnet_mode: DrawingMagnetMode,
    ) {
        if let Some(position) = position {
            self.update_brush_drawing(chart, snapshot, position, magnet_mode);
        }

        let Some(pending) = self.drawing_creation.take() else {
            return;
        };
        if pending.tool != DrawingTool::Brush {
            self.drawing_creation = Some(pending);
            return;
        }
        let Some(id) = pending.preview_id else {
            if !drawing_tool_stays_active_after_creation(pending.tool) {
                self.active_drawing_tool = None;
            }
            return;
        };
        let compacted = self.drawings.get(id).and_then(|drawing| {
            brush_compacted_anchors(
                &drawing.anchors,
                pending.pane,
                chart,
                snapshot,
                &self.bar_store,
                self.current_drawing_interval_seconds(),
            )
        });
        let keep = compacted.as_ref().is_some_and(|anchors| {
            brush_has_visible_span(
                anchors,
                pending.pane,
                chart,
                snapshot,
                &self.bar_store,
                self.current_drawing_interval_seconds(),
            )
        });

        if keep {
            if let Some(anchors) = compacted {
                self.replace_drawing_anchors(id, anchors);
            }
            self.selected_drawing_id = Some(id);
            self.hovered_drawing_id = Some(id);
            self.push_app_undo_snapshot_if_changed(pending.undo_snapshot);
            self.save_current_drawing_document();
        } else {
            let _ = self.drawings.remove(id);
            self.clear_drawing_interaction_state(id);
        }
        if !drawing_tool_stays_active_after_creation(pending.tool) {
            self.active_drawing_tool = None;
        }
    }

    fn apply_drawing_tool_click(
        &mut self,
        tool: DrawingTool,
        chart: &ChartUiResponse,
        snapshot: &ChartSnapshot,
        position: egui::Pos2,
        magnet_mode: DrawingMagnetMode,
    ) {
        if !drawing_tool_can_activate(&self.drawing_tool_policy, tool) {
            self.cancel_drawing_creation();
            self.status = format!("{} tool is unavailable", tool.tooltip());
            self.status_is_error = true;
            return;
        }
        let Some((pane, _)) = drawing_pane_at(chart, position) else {
            return;
        };
        let Some(anchor) = drawing_anchor_for_tool(
            tool,
            pane,
            position,
            chart,
            snapshot,
            &self.bar_store,
            magnet_mode,
            self.current_drawing_interval_seconds(),
        ) else {
            return;
        };

        if matches!(tool, DrawingTool::LongPosition | DrawingTool::ShortPosition) {
            let undo_snapshot = self.app_undo_snapshot();
            let Some(anchors) = position_drawing_anchors_for_stage(
                tool,
                pane,
                anchor,
                None,
                None,
                chart,
                snapshot,
                &self.bar_store,
                self.current_drawing_interval_seconds(),
            ) else {
                return;
            };
            self.commit_drawing(tool, pane, anchors);
            self.push_app_undo_snapshot_if_changed(undo_snapshot);
            self.drawing_creation = None;
            if !drawing_tool_stays_active_after_creation(tool) {
                self.active_drawing_tool = None;
            }
            return;
        }

        if tool.needs_two_clicks() {
            match self.drawing_creation.clone() {
                Some(pending) if pending.tool == tool && pending.pane == pane => {
                    if let Some(id) = pending.preview_id {
                        self.replace_drawing_anchors(id, vec![pending.first_anchor, anchor]);
                        self.selected_drawing_id = Some(id);
                        self.hovered_drawing_id = Some(id);
                    } else {
                        self.commit_drawing(tool, pane, vec![pending.first_anchor, anchor]);
                    }
                    self.push_app_undo_snapshot_if_changed(pending.undo_snapshot);
                    self.drawing_creation = None;
                    if !drawing_tool_stays_active_after_creation(tool) {
                        self.active_drawing_tool = None;
                    }
                }
                _ => {
                    let undo_snapshot = self.app_undo_snapshot();
                    let preview_id = self.create_preview_drawing(tool, pane, anchor);
                    self.drawing_creation = Some(DrawingCreationState {
                        tool,
                        pane,
                        first_anchor: anchor,
                        preview_id,
                        undo_snapshot,
                    });
                }
            }
        } else {
            let undo_snapshot = self.app_undo_snapshot();
            self.commit_drawing(tool, pane, vec![anchor]);
            self.push_app_undo_snapshot_if_changed(undo_snapshot);
            self.drawing_creation = None;
            if !drawing_tool_stays_active_after_creation(tool) {
                self.active_drawing_tool = None;
            }
        }
    }

    fn cancel_drawing_creation(&mut self) {
        if let Some(pending) = self.drawing_creation.take() {
            if let Some(id) = pending.preview_id {
                let _ = self.drawings.remove(id);
                if self.selected_drawing_id == Some(id) {
                    self.selected_drawing_id = None;
                }
                if self.hovered_drawing_id == Some(id) {
                    self.hovered_drawing_id = None;
                }
            }
        }
        self.active_drawing_tool = None;
    }

    fn create_preview_drawing(
        &mut self,
        tool: DrawingTool,
        pane: PaneId,
        anchor: DrawingAnchor,
    ) -> Option<DrawingId> {
        if !drawing_tool_can_activate(&self.drawing_tool_policy, tool) {
            self.status = format!("{} tool is unavailable", tool.tooltip());
            self.status_is_error = true;
            return None;
        }
        match self.drawings.create_with_symbol(
            tool.kind(),
            pane,
            vec![anchor, anchor],
            Some(self.current_drawing_symbol_key()),
        ) {
            Ok(id) => {
                self.apply_drawing_template(id, tool);
                self.selected_drawing_id = Some(id);
                self.hovered_drawing_id = Some(id);
                Some(id)
            }
            Err(error) => {
                self.status = format!("Drawing preview could not be created: {error:?}");
                self.status_is_error = true;
                None
            }
        }
    }

    fn update_pending_drawing_preview(
        &mut self,
        ctx: &egui::Context,
        chart: &ChartUiResponse,
        snapshot: &ChartSnapshot,
        position: egui::Pos2,
        magnet_mode: DrawingMagnetMode,
    ) -> bool {
        let Some(pending) = self.drawing_creation.clone() else {
            return false;
        };
        if !drawing_tool_can_activate(&self.drawing_tool_policy, pending.tool) {
            self.cancel_drawing_creation();
            return true;
        }
        let Some(preview_id) = pending.preview_id else {
            return false;
        };
        let Some((pane, _)) = drawing_pane_at(chart, position) else {
            return false;
        };
        if pane != pending.pane {
            return false;
        }
        let Some(anchor) = drawing_anchor_for_tool(
            pending.tool,
            pending.pane,
            position,
            chart,
            snapshot,
            &self.bar_store,
            magnet_mode,
            self.current_drawing_interval_seconds(),
        ) else {
            return false;
        };

        let generation_before = self.drawings.generation();
        self.replace_drawing_anchors(preview_id, vec![pending.first_anchor, anchor]);
        if self.drawings.generation() != generation_before {
            ctx.request_repaint();
        }
        true
    }

    fn handle_drawing_drag(
        &mut self,
        ctx: &egui::Context,
        chart: &ChartUiResponse,
        snapshot: &ChartSnapshot,
        position: Option<egui::Pos2>,
        primary_pressed: bool,
        primary_down: bool,
        primary_released: bool,
        magnet_mode: DrawingMagnetMode,
    ) -> bool {
        if primary_released {
            let drag = self.drawing_drag.take();
            if let Some(drag) = drag {
                self.push_app_undo_snapshot_if_changed(drag.undo_snapshot);
                self.save_current_drawing_document();
                return true;
            }
            return false;
        }

        if let (Some(drag), Some(position)) = (self.drawing_drag.clone(), position) {
            if !primary_down {
                if let Some(drag) = self.drawing_drag.take() {
                    self.push_app_undo_snapshot_if_changed(drag.undo_snapshot);
                    self.save_current_drawing_document();
                }
                return true;
            }
            let anchors = match drag.mode {
                DrawingDragMode::Move => moved_drawing_anchors(
                    &drag.start_anchors,
                    drag.pane,
                    drag.start_position,
                    position,
                    chart,
                    snapshot,
                    &self.bar_store,
                    magnet_mode,
                    self.current_drawing_interval_seconds(),
                ),
                DrawingDragMode::Anchor(index) => resized_drawing_anchors(
                    &drag.start_anchors,
                    drag.kind,
                    index,
                    drag.pane,
                    position,
                    chart,
                    snapshot,
                    &self.bar_store,
                    magnet_mode,
                    self.current_drawing_interval_seconds(),
                ),
            };
            if let Some(anchors) = anchors {
                let generation_before = self.drawings.generation();
                self.replace_drawing_anchors(drag.drawing_id, anchors);
                if self.drawings.generation() != generation_before {
                    ctx.request_repaint();
                }
            }
            return true;
        }

        if self.active_drawing_tool.is_some() || !primary_pressed {
            return false;
        }

        let Some(id) = self.hovered_drawing_id else {
            return false;
        };
        let Some(position) = position else {
            return false;
        };
        let Some(drawing) = self.drawings.get(id).cloned() else {
            return false;
        };
        if drawing.locked {
            self.selected_drawing_id = Some(id);
            return false;
        }
        let command_drag_copy = ctx.input(|input| input.modifiers.command);
        if command_drag_copy {
            let undo_snapshot = self.app_undo_snapshot();
            match self.insert_drawing_copy(drawing.clone()) {
                Ok(copy_id) => {
                    let Some(copy) = self.drawings.get(copy_id).cloned() else {
                        return true;
                    };
                    self.selected_drawing_id = Some(copy_id);
                    self.hovered_drawing_id = Some(copy_id);
                    self.hovered_drawing_hit = Some(DrawingHit {
                        drawing_id: copy_id,
                        target: DrawingHitTarget::Body,
                        distance: 0.0,
                    });
                    self.drawing_drag = Some(DrawingDragState {
                        drawing_id: copy_id,
                        kind: copy.kind,
                        pane: copy.pane,
                        start_position: position,
                        start_anchors: copy.anchors.clone(),
                        mode: DrawingDragMode::Move,
                        undo_snapshot,
                    });
                    ctx.request_repaint();
                    return true;
                }
                Err(error) => {
                    self.status = format!("Drawing could not be copied: {error:?}");
                    self.status_is_error = true;
                    return true;
                }
            }
        }
        self.selected_drawing_id = Some(id);
        let undo_snapshot = self.app_undo_snapshot();
        let mode = match self.hovered_drawing_hit.map(|hit| hit.target) {
            Some(DrawingHitTarget::Anchor(index)) => DrawingDragMode::Anchor(index),
            _ => DrawingDragMode::Move,
        };
        self.drawing_drag = Some(DrawingDragState {
            drawing_id: id,
            kind: drawing.kind,
            pane: drawing.pane,
            start_position: position,
            start_anchors: drawing.anchors.clone(),
            mode,
            undo_snapshot,
        });
        true
    }

    fn replace_drawing_anchors(&mut self, id: DrawingId, anchors: Vec<DrawingAnchor>) {
        if let Err(error) = self.drawings.replace_anchors(id, anchors) {
            self.status = format!("Drawing could not be moved: {error:?}");
            self.status_is_error = true;
        }
    }

    fn commit_drawing(&mut self, tool: DrawingTool, pane: PaneId, anchors: Vec<DrawingAnchor>) {
        if !drawing_tool_can_activate(&self.drawing_tool_policy, tool) {
            self.status = format!("{} tool is unavailable", tool.tooltip());
            self.status_is_error = true;
            return;
        }
        match self.drawings.create_with_symbol(
            tool.kind(),
            pane,
            anchors,
            Some(self.current_drawing_symbol_key()),
        ) {
            Ok(id) => {
                self.apply_drawing_template(id, tool);
                self.selected_drawing_id = Some(id);
                self.hovered_drawing_id = Some(id);
                if tool == DrawingTool::Text {
                    self.begin_text_edit(id, true);
                }
            }
            Err(error) => {
                self.status = format!("Drawing could not be created: {error:?}");
                self.status_is_error = true;
            }
        }
    }

    fn begin_text_edit(&mut self, id: DrawingId, select_placeholder: bool) {
        if self.drawings.get(id).is_some_and(|drawing| drawing.locked) {
            self.status = "Unlock drawing before editing text".to_string();
            self.status_is_error = true;
            return;
        }
        let draft = self
            .drawings
            .get(id)
            .and_then(|drawing| drawing.text.clone())
            .unwrap_or_else(|| "Add text".to_string());
        let undo_snapshot = self.app_undo_snapshot();
        if select_placeholder {
            let _ = self.drawings.set_text(id, Some(draft.clone()));
        }
        self.text_editing = Some(DrawingTextEditState {
            drawing_id: id,
            draft,
            focus: true,
            undo_snapshot,
        });
    }

    fn commit_text_edit(&mut self) {
        let Some(editing) = self.text_editing.take() else {
            return;
        };
        let text = editing.draft.trim();
        let value = if text.is_empty() {
            Some("Add text".to_string())
        } else {
            Some(text.to_string())
        };
        if let Err(error) = self.drawings.set_text(editing.drawing_id, value) {
            self.status = format!("Drawing text could not be updated: {error:?}");
            self.status_is_error = true;
        }
        self.push_app_undo_snapshot_if_changed(editing.undo_snapshot);
        self.save_current_drawing_document();
    }

    fn should_begin_text_edit(&self, hit: DrawingHit) -> bool {
        self.selected_drawing_id == Some(hit.drawing_id)
            && matches!(
                hit.target,
                DrawingHitTarget::Text | DrawingHitTarget::Anchor(0)
            )
            && self.drawings.get(hit.drawing_id).is_some_and(|drawing| {
                !drawing.locked && drawing_kind_is_text_editable(drawing.kind)
            })
    }

    fn show_study_settings_surface(&mut self, ctx: &egui::Context) {
        let Some(id) = self.study_settings_instance_id else {
            return;
        };
        let Some(index) = self
            .study_instances
            .iter()
            .position(|instance| instance.id == id)
        else {
            self.selected_study_instance_id = None;
            self.study_settings_instance_id = None;
            return;
        };

        let mut open = true;
        let mut draft = self.study_instances[index].clone();
        let mut changed = false;
        let mut remove = false;
        let mut tab = self.study_settings_tab;
        let title = format!("{} Settings", draft.config.kind_label());

        egui::Window::new(title)
            .id(egui::Id::new(("study_instance_settings", id.0)))
            .collapsible(false)
            .resizable(false)
            .default_pos(egui::pos2(404.0, TOP_BAR_HEIGHT + 8.0))
            .default_width(320.0)
            .open(&mut open)
            .show(ctx, |ui| {
                let (settings_changed, settings_remove) =
                    show_study_settings_editor(ui, &mut draft, &mut tab);
                changed |= settings_changed;
                remove |= settings_remove;
            });

        self.study_settings_tab = tab;
        if !open {
            self.study_settings_instance_id = None;
        }
        if remove {
            let mut instances = self.study_instances.clone();
            if remove_study_instance(&mut instances, id) {
                self.apply_study_instances_with_undo(instances);
            }
            return;
        }
        normalize_study_instances(std::slice::from_mut(&mut draft));
        if changed {
            let mut instances = self.study_instances.clone();
            if apply_study_settings_update(&mut instances, draft) {
                self.apply_study_instances_with_undo(instances);
            }
        }
    }

    fn allocate_study_instance_id(&mut self) -> StudyId {
        let id = StudyId::new(self.next_study_instance_id);
        self.next_study_instance_id = self.next_study_instance_id.saturating_add(1);
        id
    }

    fn show_tradingview_header(&mut self, ui: &mut egui::Ui, widget: WidgetSnapshot) {
        let widget_id = widget.id();
        let desired = egui::vec2(ui.available_width(), TOP_BAR_HEIGHT);
        let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(19, 19, 19));
        painter.line_segment(
            [
                egui::pos2(rect.left(), rect.bottom() - 1.0),
                egui::pos2(rect.right(), rect.bottom() - 1.0),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(45, 45, 45)),
        );

        let inner_rect = rect.shrink2(egui::vec2(6.0, 0.0));
        let scale = toolbar_scale(rect.width());
        let host_width = live_chart_host_controls_width(widget.host.mode, scale);
        let (content_rect, host_rect) =
            live_chart_header_control_rects(inner_rect, host_width, scale);

        ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect), |ui| {
            ui.shrink_clip_rect(content_rect);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_height(TOP_BAR_HEIGHT);
                let response = show_symbol_pill(ui, &header_symbol(&self.source_label), scale);
                self.open_search_on_click(response);
                ui.add_space(8.0 * scale);
                show_vertical_divider(ui, 24.0 * scale);
                ui.add_space(7.0 * scale);
                if let Some(interval) = show_timeframe_buttons(ui, self.selected_timeframe, scale) {
                    self.apply_timeframe_with_undo(ui.ctx(), interval);
                }
                ui.add_space(8.0 * scale);
                show_vertical_divider(ui, 24.0 * scale);
                ui.add_space(7.0 * scale);
                self.show_live_chart_header_actions(ui, scale, widget, widget_id);
            });
        });
        ui.scope_builder(egui::UiBuilder::new().max_rect(host_rect), |ui| {
            ui.shrink_clip_rect(host_rect);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.set_height(TOP_BAR_HEIGHT);
                self.show_live_chart_host_controls(ui, scale, widget);
            });
        });
    }

    fn show_live_chart_widget_compact_header(
        &mut self,
        ui: &mut egui::Ui,
        density: ChartWidgetDensity,
        widget: WidgetSnapshot,
    ) {
        let height = match density {
            ChartWidgetDensity::Full => TOP_BAR_HEIGHT,
            ChartWidgetDensity::Compact => 30.0,
            ChartWidgetDensity::Mini | ChartWidgetDensity::Tiny => 26.0,
        };
        let desired = egui::vec2(ui.available_width(), height);
        let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(19, 19, 19));
        painter.line_segment(
            [
                egui::pos2(rect.left(), rect.bottom() - 1.0),
                egui::pos2(rect.right(), rect.bottom() - 1.0),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(45, 45, 45)),
        );

        let inner_rect = rect.shrink2(egui::vec2(6.0, 0.0));
        let host_scale = 0.82;
        let host_width = live_chart_host_controls_width(widget.host.mode, host_scale);
        let (content_rect, host_rect) =
            live_chart_header_control_rects(inner_rect, host_width, host_scale);

        ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect), |ui| {
            ui.shrink_clip_rect(content_rect);
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.set_height(height);
                ui.label(
                    egui::RichText::new(header_symbol(&self.source_label))
                        .monospace()
                        .size(if density == ChartWidgetDensity::Compact {
                            14.0
                        } else {
                            12.0
                        })
                        .color(egui::Color32::from_rgb(232, 232, 232)),
                );
                ui.label(
                    egui::RichText::new(self.selected_timeframe.label())
                        .monospace()
                        .size(12.0)
                        .color(egui::Color32::from_rgb(176, 182, 190)),
                );
                ui.menu_button("...", |ui| {
                    self.show_live_chart_widget_overflow_menu(ui, widget);
                });
            });
        });
        ui.scope_builder(egui::UiBuilder::new().max_rect(host_rect), |ui| {
            ui.shrink_clip_rect(host_rect);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.set_height(height);
                self.show_live_chart_host_controls(ui, host_scale, widget);
            });
        });
    }

    fn show_live_chart_header_actions(
        &mut self,
        ui: &mut egui::Ui,
        scale: f32,
        widget: WidgetSnapshot,
        widget_id: WidgetId,
    ) {
        let action_width = toolbar_toggle_width("fx", scale)
            + toolbar_icon_button_width(scale) * 2.0
            + 10.0 * scale;
        if ui.available_width() >= action_width {
            self.show_indicator_section(ui, scale);
            ui.add_space(5.0 * scale);
            self.show_chart_settings_button(ui, scale);
            ui.add_space(5.0 * scale);
            self.show_chart_snapshot_button(ui, scale, widget_id);
        } else if ui.available_width() >= toolbar_toggle_width("...", scale) {
            ui.menu_button("...", |ui| {
                self.show_live_chart_widget_overflow_menu(ui, widget);
            });
        }
    }

    fn show_live_chart_widget_overflow_menu(&mut self, ui: &mut egui::Ui, widget: WidgetSnapshot) {
        let widget_id = widget.id();
        ui.set_min_width(196.0);
        if ui.button("Minimize widget").clicked() {
            let action = self.workspace_shell.minimize_widget(widget_id);
            if let Some(action) = action {
                self.apply_shell_command_action(action);
            }
            ui.close();
        }
        match widget.host.mode {
            WidgetHostMode::InShellWorkspaceSurface => {
                if ui.button("Pop out widget").clicked() {
                    let action = self.workspace_shell.tear_out_widget(widget_id);
                    if let Some(action) = action {
                        self.apply_shell_command_action(action);
                    }
                    ui.close();
                }
            }
            WidgetHostMode::DetachedNativeViewport => {
                let fullscreen = current_viewport_fullscreen(ui);
                let label = if fullscreen {
                    "Exit fullscreen"
                } else {
                    "Fullscreen widget"
                };
                if ui.button(label).clicked() {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
                    ui.close();
                }
                if ui.button("Return to shell").clicked() {
                    let action = self.workspace_shell.return_widget_to_shell(widget_id);
                    if let Some(action) = action {
                        self.apply_shell_command_action(action);
                    }
                    ui.close();
                }
            }
        }
        ui.separator();
        if ui.button("Symbol search").clicked() {
            self.search_input = self.source_label.clone();
            self.search_open = true;
            self.focus_search = true;
            self.select_search_on_focus = true;
            ui.close();
        }
        if ui
            .button(format!("Timeframe {}", self.selected_timeframe.label()))
            .clicked()
        {
            self.open_timeframe_input(String::new());
            ui.close();
        }
        if ui.button("Indicators").clicked() {
            self.indicators_open = true;
            ui.close();
        }
        if ui.button("Chart settings").clicked() {
            self.chart_settings_open = true;
            ui.close();
        }
        if ui.button("Download snapshot").clicked() {
            self.request_chart_snapshot(ui.ctx(), widget_id, ChartSnapshotAction::DownloadImage);
            ui.close();
        }
        if ui.button("Copy snapshot").clicked() {
            self.request_chart_snapshot(ui.ctx(), widget_id, ChartSnapshotAction::CopyImage);
            ui.close();
        }
        if ui.button("Object tree").clicked() {
            self.object_tree_open = true;
            ui.close();
        }

        ui.separator();
        ui.label(
            egui::RichText::new("Drawing tools")
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(150, 156, 164)),
        );
        let favorite_tools = drawing_toolbar_tools(
            &self.drawing_tool_policy,
            DrawingToolbarToolGroup::Favorites,
        );
        let main_tools =
            drawing_toolbar_tools(&self.drawing_tool_policy, DrawingToolbarToolGroup::Main);
        self.show_drawing_tool_group(ui, &favorite_tools);
        self.show_drawing_tool_group(ui, &main_tools);
        if show_measure_tool_button(ui, self.measure_active) {
            self.cancel_drawing_creation();
            self.drawing_drag = None;
            self.active_drawing_tool = None;
            self.measure_active = !self.measure_active;
            if !self.measure_active {
                self.measure_state = None;
            }
            ui.close();
        }
        if ui.button("Tool policy").clicked() {
            self.drawing_tool_policy_menu_open = true;
            ui.close();
        }
    }

    fn show_live_chart_host_controls(
        &mut self,
        ui: &mut egui::Ui,
        scale: f32,
        widget: WidgetSnapshot,
    ) {
        ui.spacing_mut().item_spacing.x = LIVE_CHART_HOST_CONTROL_GAP * scale;
        match widget.host.mode {
            WidgetHostMode::InShellWorkspaceSurface => {
                if show_live_chart_host_button(ui, "OUT", "Pop out widget", scale).clicked() {
                    let action = self.workspace_shell.tear_out_widget(widget.id());
                    if let Some(action) = action {
                        self.apply_shell_command_action(action);
                    }
                }
            }
            WidgetHostMode::DetachedNativeViewport => {
                let fullscreen = current_viewport_fullscreen(ui);
                let label = if fullscreen { "WIN" } else { "FS" };
                let hover = if fullscreen {
                    "Exit fullscreen"
                } else {
                    "Fullscreen widget"
                };
                if show_live_chart_host_button(ui, label, hover, scale).clicked() {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fullscreen));
                }
                if show_live_chart_host_button(ui, "IN", "Return widget to shell", scale).clicked()
                {
                    let action = self.workspace_shell.return_widget_to_shell(widget.id());
                    if let Some(action) = action {
                        self.apply_shell_command_action(action);
                    }
                }
            }
        }
        if show_live_chart_host_button(ui, "MIN", "Minimize widget", scale).clicked() {
            let action = self.workspace_shell.minimize_widget(widget.id());
            if let Some(action) = action {
                self.apply_shell_command_action(action);
            }
        }
    }

    fn show_latency_overlay(&self, ctx: &egui::Context) {
        if !cfg!(debug_assertions) || !self.debug_latency_overlay_visible {
            return;
        }
        let snapshot = self.latency_probe.snapshot(Instant::now());
        egui::Area::new(egui::Id::new("debug_latency_overlay"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-10.0, 48.0))
            .order(egui::Order::Tooltip)
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_rgba_premultiplied(11, 13, 15, 220))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(55, 62, 70)))
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_min_width(270.0);
                        ui.label(
                            egui::RichText::new("DEBUG LATENCY")
                                .monospace()
                                .size(11.0)
                                .color(egui::Color32::from_rgb(160, 168, 176)),
                        );
                        show_latency_metric_row(ui, "update", snapshot.update);
                        show_latency_metric_row(ui, "repaint", snapshot.repaint_gap);
                        show_latency_metric_row(ui, "idle wake", snapshot.idle_wake_gap);
                        show_latency_metric_row(ui, "input->frame", snapshot.input_to_frame);
                        show_latency_metric_row(ui, "data->frame", snapshot.data_to_frame);
                        ui.separator();
                        show_latency_value_row(ui, "input age", snapshot.current_input_age_ms);
                        show_latency_value_row(ui, "data age", snapshot.current_data_age_ms);
                        ui.label(
                            egui::RichText::new(format!(
                                "frames {} / active {} / samples {} / idle wakes {}",
                                snapshot.frame_count,
                                snapshot.active_sample_count,
                                snapshot.sample_count,
                                snapshot.idle_wake_count
                            ))
                            .monospace()
                            .size(11.0)
                            .color(egui::Color32::from_rgb(130, 136, 144)),
                        );
                    });
            });
    }
}

impl Drop for ChartOnlyApp {
    fn drop(&mut self) {
        self.cancel_subscription();
    }
}

fn show_symbol_pill(ui: &mut egui::Ui, symbol: &str, scale: f32) -> egui::Response {
    let font = egui::FontId::proportional(17.0 * scale);
    let text_width = ui
        .painter()
        .layout_no_wrap(
            symbol.to_string(),
            font.clone(),
            egui::Color32::from_rgb(214, 214, 214),
        )
        .rect
        .width();
    let width = (text_width + 30.0 * scale).max(86.0 * scale);
    let desired = egui::vec2(width, 26.0 * scale);
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click());
    let fill = if response.hovered() {
        egui::Color32::from_rgb(76, 76, 76)
    } else {
        egui::Color32::from_rgb(64, 64, 64)
    };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 13.0 * scale, fill);
    painter.text(
        egui::pos2(rect.left() + 12.0 * scale, rect.center().y),
        egui::Align2::LEFT_CENTER,
        symbol,
        font,
        egui::Color32::from_rgb(218, 218, 218),
    );
    response.on_hover_text("Search ticker")
}

fn show_vertical_divider(ui: &mut egui::Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, height), egui::Sense::hover());
    ui.painter_at(rect).line_segment(
        [
            egui::pos2(rect.center().x, rect.top()),
            egui::pos2(rect.center().x, rect.bottom()),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_rgb(54, 54, 54)),
    );
}

fn show_timeframe_buttons(
    ui: &mut egui::Ui,
    selected: BarInterval,
    scale: f32,
) -> Option<BarInterval> {
    let mut clicked = None;
    for timeframe in TIMEFRAMES {
        let selected_now = selected == *timeframe;
        let width = timeframe_button_width(timeframe.label()) * scale;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, 27.0 * scale), egui::Sense::click());
        if response.clicked() {
            clicked = Some(*timeframe);
        }

        let painter = ui.painter_at(rect);
        if selected_now {
            painter.rect_filled(
                rect.shrink2(egui::vec2(0.0, 1.0)),
                5.0 * scale,
                egui::Color32::from_rgb(75, 75, 75),
            );
        }
        let color = if selected_now {
            egui::Color32::from_rgb(232, 232, 232)
        } else if response.hovered() {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_rgb(217, 217, 217)
        };
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            timeframe.label(),
            egui::FontId::proportional(17.0 * scale),
            color,
        );
    }
    clicked
}

fn show_toolbar_toggle(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    scale: f32,
) -> egui::Response {
    let width = toolbar_toggle_width(label, scale);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, 27.0 * scale), egui::Sense::click());
    let painter = ui.painter_at(rect);
    if selected || response.hovered() {
        painter.rect_filled(
            rect.shrink2(egui::vec2(0.0, 1.0)),
            5.0 * scale,
            if selected {
                egui::Color32::from_rgb(75, 75, 75)
            } else {
                egui::Color32::from_rgb(54, 54, 54)
            },
        );
    }
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(16.0 * scale),
        if selected || response.hovered() {
            egui::Color32::WHITE
        } else {
            egui::Color32::from_rgb(217, 217, 217)
        },
    );
    response
}

fn toolbar_toggle_width(label: &str, scale: f32) -> f32 {
    (label.len() as f32 * 9.0 + 22.0).max(34.0) * scale
}

fn toolbar_icon_button_width(scale: f32) -> f32 {
    34.0 * scale
}

fn current_viewport_fullscreen(ui: &egui::Ui) -> bool {
    ui.input(|input| input.viewport().fullscreen.unwrap_or(false))
}

fn show_live_chart_host_button(
    ui: &mut egui::Ui,
    label: &str,
    hover: &str,
    scale: f32,
) -> egui::Response {
    show_toolbar_toggle(ui, label, false, scale * 0.82).on_hover_text(hover)
}

fn show_toolbar_settings_button(ui: &mut egui::Ui, selected: bool, scale: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(toolbar_icon_button_width(scale), 27.0 * scale),
        egui::Sense::click(),
    );
    let painter = ui.painter_at(rect);
    if selected || response.hovered() {
        painter.rect_filled(
            rect.shrink2(egui::vec2(0.0, 1.0)),
            5.0 * scale,
            if selected {
                egui::Color32::from_rgb(75, 75, 75)
            } else {
                egui::Color32::from_rgb(54, 54, 54)
            },
        );
    }
    paint_toolbar_settings_icon(&painter, rect, selected || response.hovered(), scale);
    response
}

fn show_toolbar_camera_button(ui: &mut egui::Ui, scale: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(toolbar_icon_button_width(scale), 27.0 * scale),
        egui::Sense::click(),
    );
    let painter = ui.painter_at(rect);
    if response.hovered() {
        painter.rect_filled(
            rect.shrink2(egui::vec2(0.0, 1.0)),
            5.0 * scale,
            egui::Color32::from_rgb(54, 54, 54),
        );
    }
    paint_toolbar_camera_icon(&painter, rect, response.hovered(), scale);
    response
}

fn paint_toolbar_camera_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    highlighted: bool,
    scale: f32,
) {
    let color = if highlighted {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_rgb(217, 217, 217)
    };
    let stroke = egui::Stroke::new(1.3 * scale, color);
    let body = egui::Rect::from_center_size(
        rect.center() + egui::vec2(0.0, 1.0 * scale),
        egui::vec2(19.0 * scale, 13.0 * scale),
    );
    let lens_center = body.center() + egui::vec2(1.5 * scale, 0.5 * scale);
    let top = egui::Rect::from_min_size(
        egui::pos2(body.left() + 4.0 * scale, body.top() - 4.0 * scale),
        egui::vec2(8.0 * scale, 4.0 * scale),
    );
    painter.rect_stroke(body, 2.5 * scale, stroke, egui::StrokeKind::Inside);
    painter.line_segment([top.left_bottom(), top.left_top()], stroke);
    painter.line_segment([top.left_top(), top.right_top()], stroke);
    painter.line_segment([top.right_top(), top.right_bottom()], stroke);
    painter.circle_stroke(lens_center, 4.1 * scale, stroke);
    painter.circle_filled(
        egui::pos2(body.left() + 4.0 * scale, body.top() + 3.4 * scale),
        1.2 * scale,
        color,
    );
}

fn show_snapshot_menu_row(
    ui: &mut egui::Ui,
    icon: &str,
    label: &str,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let desired = egui::vec2(ui.available_width(), 34.0);
    let (rect, response) = ui.allocate_exact_size(desired, sense);
    let painter = ui.painter_at(rect);
    let text_color = if enabled {
        egui::Color32::from_rgb(226, 226, 226)
    } else {
        egui::Color32::from_rgb(112, 116, 122)
    };
    if enabled && response.hovered() {
        painter.rect_filled(rect, 4.0, egui::Color32::from_rgb(48, 48, 48));
    }
    let badge_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 22.0, rect.center().y),
        egui::vec2(28.0, 22.0),
    );
    painter.rect_stroke(
        badge_rect,
        3.0,
        egui::Stroke::new(1.0, text_color),
        egui::StrokeKind::Inside,
    );
    painter.text(
        badge_rect.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::monospace(11.0),
        text_color,
    );
    painter.text(
        egui::pos2(rect.left() + 46.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(17.0),
        text_color,
    );
    response
}

fn paint_toolbar_settings_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    highlighted: bool,
    scale: f32,
) {
    let center = rect.center();
    let color = if highlighted {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_rgb(217, 217, 217)
    };
    let stroke = egui::Stroke::new(1.2 * scale, color);
    let outer = 7.0 * scale;

    for index in 0..8 {
        let angle = index as f32 * std::f32::consts::TAU / 8.0;
        let direction = egui::vec2(angle.cos(), angle.sin());
        let from = center + direction * (outer - 1.8 * scale);
        let to = center + direction * (outer + 1.4 * scale);
        painter.line_segment([from, to], stroke);
    }
    painter.circle_stroke(center, 4.7 * scale, stroke);
    painter.circle_stroke(center, 2.3 * scale, stroke);
}

fn drawing_tool_order(tool: DrawingTool) -> usize {
    DRAWING_TOOLS
        .iter()
        .position(|candidate| *candidate == tool)
        .unwrap_or(DRAWING_TOOLS.len())
}

fn drawing_tool_visible(policy: &DrawingToolPolicy, tool: DrawingTool) -> bool {
    policy.access(tool) != DrawingToolAccess::Hidden
}

fn drawing_tool_can_activate(policy: &DrawingToolPolicy, tool: DrawingTool) -> bool {
    policy.access(tool) == DrawingToolAccess::Available
}

fn drawing_tool_stays_active_after_creation(tool: DrawingTool) -> bool {
    matches!(tool, DrawingTool::Brush)
}

fn drawing_toolbar_tools(
    policy: &DrawingToolPolicy,
    group: DrawingToolbarToolGroup,
) -> Vec<DrawingTool> {
    DRAWING_TOOLS
        .into_iter()
        .filter(|tool| drawing_tool_visible(policy, *tool))
        .filter(|tool| match group {
            DrawingToolbarToolGroup::Favorites => policy.is_favorite(*tool),
            DrawingToolbarToolGroup::Main => !policy.is_favorite(*tool),
        })
        .collect()
}

fn show_drawing_tool_button(
    ui: &mut egui::Ui,
    tool: DrawingTool,
    active: Option<DrawingTool>,
    access: DrawingToolAccess,
    favorite: bool,
) -> egui::Response {
    let selected = active == Some(tool);
    let enabled = access == DrawingToolAccess::Available;
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(DRAWING_TOOL_BUTTON_WIDTH, DRAWING_TOOL_BUTTON_HEIGHT),
        sense,
    );
    let painter = ui.painter_at(rect);
    let fill = if selected {
        egui::Color32::from_rgb(58, 94, 172)
    } else if response.hovered() && enabled {
        egui::Color32::from_rgb(54, 54, 54)
    } else if !enabled {
        egui::Color32::from_rgb(32, 32, 32)
    } else {
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 0)
    };
    painter.rect_filled(rect.shrink(1.0), 4.0, fill);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        tool.label(),
        egui::FontId::proportional(13.5),
        if enabled {
            egui::Color32::from_rgb(232, 232, 232)
        } else {
            egui::Color32::from_rgb(118, 118, 118)
        },
    );
    if favorite {
        painter.circle_filled(
            egui::pos2(rect.right() - 5.0, rect.top() + 5.0),
            2.0,
            egui::Color32::from_rgb(238, 185, 72),
        );
    }
    if enabled {
        response.on_hover_text(tool.tooltip())
    } else {
        response.on_hover_text(format!("{} disabled", tool.tooltip()))
    }
}

fn show_measure_tool_button(ui: &mut egui::Ui, active: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(DRAWING_TOOL_BUTTON_WIDTH, DRAWING_TOOL_BUTTON_HEIGHT),
        egui::Sense::click(),
    );
    let painter = ui.painter_at(rect);
    let fill = if active {
        egui::Color32::from_rgb(58, 94, 172)
    } else if response.hovered() {
        egui::Color32::from_rgb(54, 54, 54)
    } else {
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 0)
    };
    painter.rect_filled(rect.shrink(1.0), 4.0, fill);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "M",
        egui::FontId::proportional(13.5),
        egui::Color32::from_rgb(232, 232, 232),
    );
    response.on_hover_text("Measure").clicked()
}

fn show_drawing_tool_policy_menu(
    ui: &mut egui::Ui,
    policy: &mut DrawingToolPolicy,
    menu_open: &mut bool,
) -> bool {
    let button = egui::Button::new("F")
        .min_size(egui::vec2(
            DRAWING_TOOL_BUTTON_WIDTH,
            DRAWING_TOOL_GLOBAL_ACTION_HEIGHT,
        ))
        .corner_radius(4.0);
    let mut changed = false;
    let response = ui.add(button);
    if response.clicked() {
        *menu_open = !*menu_open;
    }
    if *menu_open && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
        *menu_open = false;
    }
    if *menu_open {
        let popup_position = egui::pos2(response.rect.right() + 8.0, response.rect.top());
        egui::Area::new(egui::Id::new("rustchart_drawing_tool_policy_menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(popup_position)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(278.0);
                    ui.label(
                        egui::RichText::new("Drawing tools")
                            .size(12.0)
                            .color(egui::Color32::from_rgb(162, 168, 176)),
                    );
                    ui.add_space(4.0);
                    for tool in DRAWING_TOOLS {
                        ui.horizontal(|ui| {
                            let mut favorite = policy.is_favorite(tool);
                            if ui.checkbox(&mut favorite, "").changed() {
                                policy.set_favorite(tool, favorite);
                                changed = true;
                            }
                            ui.label(tool.tooltip());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let mut access = policy.access(tool);
                                    egui::ComboBox::from_id_salt((
                                        "drawing_tool_access",
                                        drawing_tool_order(tool),
                                    ))
                                    .selected_text(drawing_tool_access_label(access))
                                    .width(92.0)
                                    .show_ui(ui, |ui| {
                                        for choice in [
                                            DrawingToolAccess::Available,
                                            DrawingToolAccess::Disabled,
                                            DrawingToolAccess::Hidden,
                                        ] {
                                            ui.selectable_value(
                                                &mut access,
                                                choice,
                                                drawing_tool_access_label(choice),
                                            );
                                        }
                                    });
                                    if access != policy.access(tool) {
                                        policy.set_access(tool, access);
                                        changed = true;
                                    }
                                },
                            );
                        });
                    }
                });
            });
    }
    response.on_hover_text("Drawing tool favorites and access");
    changed
}

fn drawing_tool_access_label(access: DrawingToolAccess) -> &'static str {
    match access {
        DrawingToolAccess::Available => "Available",
        DrawingToolAccess::Disabled => "Disabled",
        DrawingToolAccess::Hidden => "Hidden",
    }
}

fn show_drawing_magnet_control(
    ui: &mut egui::Ui,
    mode: &mut DrawingMagnetMode,
    modifier_inverts: bool,
    menu_open: &mut bool,
    hold_started_at: &mut Option<f64>,
    hold_opened: &mut bool,
) -> bool {
    let effective = effective_drawing_magnet_mode(*mode, modifier_inverts);
    let mut changed = false;
    let response = show_drawing_magnet_button(ui, effective);
    let now = ui.input(|input| input.time);
    if response.is_pointer_button_down_on() {
        let started = hold_started_at.get_or_insert(now);
        if !*hold_opened && now - *started >= DRAWING_MAGNET_HOLD_SECONDS {
            *menu_open = true;
            *hold_opened = true;
            ui.ctx().request_repaint();
        } else {
            ui.ctx().request_repaint_after(Duration::from_millis(40));
        }
    } else {
        *hold_started_at = None;
    }

    if response.clicked() {
        if *hold_opened {
            *hold_opened = false;
        } else {
            changed |= toggle_drawing_magnet_quick(mode);
            *menu_open = false;
        }
    } else if !ui.input(|input| input.pointer.primary_down()) {
        *hold_opened = false;
    }

    if *menu_open {
        changed |= show_drawing_magnet_popup(ui, &response, mode, menu_open);
    }

    response.on_hover_text(format!(
        "Magnet: {}{}. Click to toggle, hold for menu.",
        drawing_magnet_mode_label(effective),
        if modifier_inverts { " (temporary)" } else { "" }
    ));
    changed
}

fn show_drawing_magnet_button(ui: &mut egui::Ui, effective: DrawingMagnetMode) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(DRAWING_TOOL_BUTTON_WIDTH, DRAWING_TOOL_GLOBAL_ACTION_HEIGHT),
        egui::Sense::click(),
    );
    let painter = ui.painter_at(rect);
    let fill = if effective == DrawingMagnetMode::Off {
        egui::Color32::from_rgb(40, 40, 40)
    } else {
        egui::Color32::from_rgb(38, 98, 150)
    };
    painter.rect_filled(rect.shrink(1.0), 4.0, fill);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "M",
        egui::FontId::proportional(13.5),
        egui::Color32::from_rgb(224, 228, 232),
    );
    response
}

fn show_drawing_magnet_popup(
    ui: &mut egui::Ui,
    button_response: &egui::Response,
    mode: &mut DrawingMagnetMode,
    menu_open: &mut bool,
) -> bool {
    let mut changed = false;
    let position = button_response.rect.right_top() + egui::vec2(6.0, -4.0);
    let area = egui::Area::new(egui::Id::new("rustchart_drawing_magnet_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(position)
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(104.0);
                for choice in [
                    DrawingMagnetMode::Off,
                    DrawingMagnetMode::Weak,
                    DrawingMagnetMode::Strong,
                ] {
                    if ui
                        .selectable_label(*mode == choice, drawing_magnet_mode_label(choice))
                        .clicked()
                    {
                        *mode = choice;
                        changed = true;
                        *menu_open = false;
                    }
                }
            });
        });
    let popup_rect = area.response.rect;
    let clicked_outside = ui.input(|input| {
        input.pointer.primary_clicked()
            && input.pointer.interact_pos().is_some_and(|position| {
                !button_response.rect.contains(position) && !popup_rect.contains(position)
            })
    });
    if clicked_outside {
        *menu_open = false;
    }
    changed
}

fn toggle_drawing_magnet_quick(mode: &mut DrawingMagnetMode) -> bool {
    let next = if *mode == DrawingMagnetMode::Off {
        DrawingMagnetMode::Weak
    } else {
        DrawingMagnetMode::Off
    };
    if *mode == next {
        return false;
    }
    *mode = next;
    true
}

fn show_drawing_visibility_action_button(
    ui: &mut egui::Ui,
    enabled: bool,
    has_visible_drawings: bool,
) -> bool {
    show_drawing_toolbar_icon_button(
        ui,
        enabled,
        if has_visible_drawings {
            "Hide all drawings"
        } else {
            "Show all drawings"
        },
        |painter, rect, color| paint_toolbar_eye_icon(painter, rect, color, !has_visible_drawings),
    )
    .clicked()
        && enabled
}

fn show_drawing_delete_action_button(ui: &mut egui::Ui, enabled: bool) -> bool {
    show_drawing_toolbar_icon_button(ui, enabled, "Delete all drawings", paint_toolbar_x_icon)
        .clicked()
        && enabled
}

fn show_drawing_toolbar_icon_button(
    ui: &mut egui::Ui,
    enabled: bool,
    tooltip: &str,
    paint_icon: impl FnOnce(&egui::Painter, egui::Rect, egui::Color32),
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(DRAWING_TOOL_BUTTON_WIDTH, DRAWING_TOOL_GLOBAL_ACTION_HEIGHT),
        sense,
    );
    let painter = ui.painter_at(rect);
    let fill = if response.hovered() && enabled {
        egui::Color32::from_rgb(54, 54, 54)
    } else {
        egui::Color32::from_rgb(40, 40, 40)
    };
    let color = if enabled {
        egui::Color32::from_rgb(232, 232, 232)
    } else {
        egui::Color32::from_rgb(110, 110, 110)
    };
    painter.rect_filled(rect.shrink(1.0), 4.0, fill);
    paint_icon(&painter, rect, color);
    response.on_hover_text(tooltip)
}

fn paint_toolbar_eye_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    color: egui::Color32,
    slashed: bool,
) {
    let center = rect.center();
    let stroke = egui::Stroke::new(1.4, color);
    let left = center + egui::vec2(-9.0, 0.0);
    let right = center + egui::vec2(9.0, 0.0);
    let top = center + egui::vec2(0.0, -5.0);
    let bottom = center + egui::vec2(0.0, 5.0);
    painter.add(egui::Shape::line(
        vec![left, top, right, bottom, left],
        stroke,
    ));
    painter.circle_filled(center, 2.2, color);
    if slashed {
        painter.line_segment(
            [
                center + egui::vec2(-9.0, 7.0),
                center + egui::vec2(9.0, -7.0),
            ],
            stroke,
        );
    }
}

fn paint_toolbar_x_icon(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let center = rect.center();
    let stroke = egui::Stroke::new(1.8, color);
    painter.line_segment(
        [
            center + egui::vec2(-7.0, -7.0),
            center + egui::vec2(7.0, 7.0),
        ],
        stroke,
    );
    painter.line_segment(
        [
            center + egui::vec2(7.0, -7.0),
            center + egui::vec2(-7.0, 7.0),
        ],
        stroke,
    );
}

fn drawing_magnet_mode_label(mode: DrawingMagnetMode) -> &'static str {
    match mode {
        DrawingMagnetMode::Off => "Off",
        DrawingMagnetMode::Weak => "Weak",
        DrawingMagnetMode::Strong => "Strong",
    }
}

fn chart_crosshair_magnet(mode: DrawingMagnetMode) -> Option<ChartCrosshairMagnet> {
    let mode = match mode {
        DrawingMagnetMode::Off => return None,
        DrawingMagnetMode::Weak => ChartCrosshairMagnetMode::Weak,
        DrawingMagnetMode::Strong => ChartCrosshairMagnetMode::Strong,
    };
    Some(ChartCrosshairMagnet {
        mode,
        weak_price_distance_px: MAGNET_WEAK_PRICE_DISTANCE_PX,
    })
}

fn drawing_magnet_modifier(modifiers: egui::Modifiers) -> bool {
    modifiers.command
}

fn drawing_magnet_modifier_should_apply(
    active_drawing_tool: bool,
    drawing_creation_active: bool,
    drawing_drag_active: bool,
) -> bool {
    active_drawing_tool || drawing_creation_active || drawing_drag_active
}

fn effective_drawing_magnet_mode(
    configured: DrawingMagnetMode,
    modifier_held: bool,
) -> DrawingMagnetMode {
    match (configured, modifier_held) {
        (DrawingMagnetMode::Off, true) => DrawingMagnetMode::Weak,
        (DrawingMagnetMode::Off, false) => DrawingMagnetMode::Off,
        (_, true) => DrawingMagnetMode::Off,
        (mode, false) => mode,
    }
}

fn selected_drawing_toolbar_rect(
    chart: &ChartUiResponse,
    requested_position: Option<egui::Pos2>,
) -> egui::Rect {
    let size = selected_drawing_toolbar_size(chart.plot);
    let position = requested_position
        .unwrap_or_else(|| default_selected_drawing_toolbar_position(chart.plot, size));
    egui::Rect::from_min_size(
        clamp_selected_drawing_toolbar_position(position, chart.outer, size),
        size,
    )
}

fn drawing_controls_contain(
    chart: &ChartUiResponse,
    selected_toolbar_visible: bool,
    selected_toolbar_position: Option<egui::Pos2>,
    position: egui::Pos2,
) -> bool {
    selected_toolbar_visible
        && selected_drawing_toolbar_rect(chart, selected_toolbar_position).contains(position)
}

fn drawing_toolbar_position_from_rail(rail: egui::Rect) -> egui::Pos2 {
    let width = drawing_toolbar_width();
    egui::pos2(
        rail.center().x - width * 0.5,
        rail.top() + DRAWING_TOOLBAR_TOP_OFFSET,
    )
}

fn drawing_toolbar_rect_from_rail(rail: egui::Rect) -> egui::Rect {
    let position = drawing_toolbar_position_from_rail(rail);
    let size = egui::vec2(drawing_toolbar_width(), drawing_toolbar_content_height());
    egui::Rect::from_min_size(position, size)
}

fn drawing_toolbar_width() -> f32 {
    DRAWING_TOOL_BUTTON_WIDTH + DRAWING_TOOLBAR_MARGIN * 2.0
}

fn drawing_toolbar_content_height() -> f32 {
    DRAWING_TOOL_BUTTON_HEIGHT * DRAWING_TOOLS.len() as f32
        + DRAWING_TOOL_BUTTON_GAP * DRAWING_TOOLS.len().saturating_sub(1) as f32
        + DRAWING_TOOL_GLOBAL_ACTION_HEIGHT * 4.0
        + DRAWING_TOOL_BUTTON_GAP * 5.0
        + DRAWING_TOOLBAR_MARGIN * 2.0
}

#[cfg(test)]
fn selected_drawing_toolbar_rect_from_plot(plot: egui::Rect) -> egui::Rect {
    let size = selected_drawing_toolbar_size(plot);
    egui::Rect::from_min_size(default_selected_drawing_toolbar_position(plot, size), size)
}

fn selected_drawing_toolbar_size(plot: egui::Rect) -> egui::Vec2 {
    egui::vec2(
        SELECTED_DRAWING_TOOLBAR_WIDTH.min((plot.width() - 24.0).max(300.0)),
        SELECTED_DRAWING_TOOLBAR_HEIGHT,
    )
}

fn default_selected_drawing_toolbar_position(plot: egui::Rect, size: egui::Vec2) -> egui::Pos2 {
    egui::pos2(
        plot.center().x - size.x * 0.5,
        plot.top() + SELECTED_DRAWING_TOOLBAR_TOP_OFFSET,
    )
}

fn clamp_selected_drawing_toolbar_position(
    position: egui::Pos2,
    bounds: egui::Rect,
    size: egui::Vec2,
) -> egui::Pos2 {
    let margin = 6.0;
    let min_x = bounds.left() + margin;
    let min_y = bounds.top() + margin;
    let max_x = (bounds.right() - size.x - margin).max(min_x);
    let max_y = (bounds.bottom() - size.y - margin).max(min_y);
    egui::pos2(
        position.x.clamp(min_x, max_x),
        position.y.clamp(min_y, max_y),
    )
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct FibSettingsEditorChange {
    settings: bool,
    style: bool,
    coordinates: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PositionSettingsEditorChange {
    settings: bool,
    style: bool,
    coordinates: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FibExtendMode {
    None,
    Left,
    Right,
    Both,
}

fn show_fib_retracement_settings_editor(
    ui: &mut egui::Ui,
    settings: &mut FibRetracementSettings,
    style: &mut DrawingStyle,
    anchors: &mut Vec<DrawingAnchor>,
    kind: DrawingKind,
    bars: &(impl BarSeries + ?Sized),
    interval_seconds: Option<i64>,
) -> FibSettingsEditorChange {
    let mut changed = FibSettingsEditorChange::default();

    ui.label(
        egui::RichText::new("Style")
            .size(12.0)
            .color(egui::Color32::from_rgb(162, 168, 176)),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        changed.style |= show_drawing_color_menu(ui, kind, style);
        changed.style |= show_drawing_width_menu(ui, style);
        changed.style |= show_drawing_line_style_menu(ui, style);
    });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new("Options")
            .size(12.0)
            .color(egui::Color32::from_rgb(162, 168, 176)),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        changed.settings |= ui
            .checkbox(&mut settings.trend_line_visible, "Trend line")
            .changed();
        changed.settings |= ui
            .checkbox(&mut settings.labels_visible, "Labels")
            .changed();
        changed.settings |= ui
            .checkbox(&mut settings.prices_visible, "Prices")
            .changed();
        changed.settings |= ui
            .checkbox(&mut settings.reverse, "Reverse")
            .on_hover_text("Invert the Fib level direction")
            .changed();
    });
    ui.horizontal(|ui| {
        changed.settings |= ui
            .checkbox(&mut settings.background_visible, "Background")
            .changed();
        changed.settings |=
            show_chart_color_control(ui, "Fill color", &mut settings.background_color);
    });
    ui.horizontal(|ui| {
        ui.label("Values");
        let mut label_position = settings.label_position;
        egui::ComboBox::from_id_salt(ui.id().with("fib_label_position"))
            .selected_text(fib_label_position_label(label_position))
            .show_ui(ui, |ui| {
                for choice in [
                    FibRetracementLabelPosition::Above,
                    FibRetracementLabelPosition::OnLine,
                    FibRetracementLabelPosition::Below,
                ] {
                    ui.selectable_value(
                        &mut label_position,
                        choice,
                        fib_label_position_label(choice),
                    );
                }
            });
        if settings.label_position != label_position {
            settings.label_position = label_position;
            changed.settings = true;
        }
        let mut label_side = settings.label_side;
        egui::ComboBox::from_id_salt(ui.id().with("fib_label_side"))
            .selected_text(fib_label_side_label(label_side))
            .show_ui(ui, |ui| {
                for choice in [
                    FibRetracementLabelSide::Left,
                    FibRetracementLabelSide::Right,
                ] {
                    ui.selectable_value(&mut label_side, choice, fib_label_side_label(choice));
                }
            });
        if settings.label_side != label_side {
            settings.label_side = label_side;
            changed.settings = true;
        }
    });
    ui.horizontal(|ui| {
        ui.label("Extend");
        let mut extend = fib_extend_mode(settings.extend_left, settings.extend_right);
        egui::ComboBox::from_id_salt(ui.id().with("fib_extend"))
            .selected_text(fib_extend_mode_label(extend))
            .show_ui(ui, |ui| {
                for choice in [
                    FibExtendMode::None,
                    FibExtendMode::Left,
                    FibExtendMode::Right,
                    FibExtendMode::Both,
                ] {
                    ui.selectable_value(&mut extend, choice, fib_extend_mode_label(choice));
                }
            });
        if set_fib_extend_mode(settings, extend) {
            changed.settings = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("Reset")
                .on_hover_text("Reset Fib levels")
                .clicked()
            {
                *settings = FibRetracementSettings::default();
                changed.settings = true;
            }
        });
    });

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new("Coordinates")
            .size(12.0)
            .color(egui::Color32::from_rgb(162, 168, 176)),
    );
    ui.add_space(4.0);
    changed.coordinates |= show_fib_coordinates_editor(ui, anchors, bars, interval_seconds);

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new("Levels")
            .size(12.0)
            .color(egui::Color32::from_rgb(162, 168, 176)),
    );
    ui.add_space(4.0);
    let mut remove_level_index = None;
    egui::Grid::new("fib_retracement_levels_grid")
        .num_columns(4)
        .spacing(egui::vec2(8.0, 6.0))
        .show(ui, |ui| {
            for (index, level) in settings.levels.iter_mut().enumerate() {
                changed.settings |= ui
                    .checkbox(&mut level.visible, "")
                    .on_hover_text("Show level")
                    .changed();
                changed.settings |= ui
                    .add(
                        egui::DragValue::new(&mut level.value)
                            .range(-10.0..=10.0)
                            .speed(0.001)
                            .fixed_decimals(3),
                    )
                    .on_hover_text("Level ratio")
                    .changed();
                changed.settings |= ui
                    .push_id(("fib_level_color", index), |ui| {
                        show_fib_level_color_menu(ui, &mut level.color, ui.id().with(index))
                    })
                    .inner;
                if ui.small_button("-").on_hover_text("Remove level").clicked() {
                    remove_level_index = Some(index);
                }
                ui.end_row();
            }
        });
    if let Some(index) = remove_level_index {
        changed.settings |= remove_fib_level(settings, index);
    }
    ui.add_space(4.0);
    if ui.button("+ Add level").clicked() {
        add_fib_level(settings);
        changed.settings = true;
    }

    changed
}

fn show_fib_preset_controls(
    ui: &mut egui::Ui,
    presets: &FibPresetCollection,
    preset_name_input: &mut String,
    settings_valid: bool,
) -> Option<FibPresetUiAction> {
    let mut action = None;
    ui.separator();
    ui.horizontal(|ui| {
        ui.label("Template");
        let active_label = presets
            .active_preset()
            .map(|preset| preset.name.as_str())
            .unwrap_or("Default template");
        egui::ComboBox::from_id_salt(ui.id().with("fib_preset_picker"))
            .selected_text(active_label)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(
                        presets.active_preset_id.is_none(),
                        egui::RichText::new("Default template"),
                    )
                    .clicked()
                {
                    action = Some(FibPresetUiAction::ClearActive);
                }
                for preset in &presets.presets {
                    if ui
                        .selectable_label(
                            presets.active_preset_id.as_deref() == Some(preset.id.as_str()),
                            egui::RichText::new(&preset.name),
                        )
                        .clicked()
                    {
                        action = Some(FibPresetUiAction::Apply(preset.id.clone()));
                    }
                }
            });
    });
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(preset_name_input)
                .hint_text("Preset name")
                .desired_width(150.0),
        );
        if ui
            .add_enabled(settings_valid, egui::Button::new("Save"))
            .clicked()
        {
            action = Some(FibPresetUiAction::SaveAs);
        }
        let has_active = presets.active_preset().is_some();
        if ui
            .add_enabled(settings_valid && has_active, egui::Button::new("Update"))
            .clicked()
        {
            action = Some(FibPresetUiAction::UpdateActive);
        }
        if ui
            .add_enabled(has_active, egui::Button::new("Delete"))
            .clicked()
        {
            action = Some(FibPresetUiAction::DeleteActive);
        }
    });
    action
}

fn add_fib_level(settings: &mut FibRetracementSettings) {
    settings.levels.push(FibRetracementLevel::new(
        next_fib_level_value(&settings.levels),
        ChartColor::rgb(255, 152, 0),
    ));
}

fn remove_fib_level(settings: &mut FibRetracementSettings, index: usize) -> bool {
    if index >= settings.levels.len() {
        return false;
    }
    settings.levels.remove(index);
    true
}

fn fib_label_position_label(position: FibRetracementLabelPosition) -> &'static str {
    match position {
        FibRetracementLabelPosition::Above => "Above line",
        FibRetracementLabelPosition::OnLine => "On line",
        FibRetracementLabelPosition::Below => "Below line",
    }
}

fn fib_label_side_label(side: FibRetracementLabelSide) -> &'static str {
    match side {
        FibRetracementLabelSide::Left => "Left",
        FibRetracementLabelSide::Right => "Right",
    }
}

fn next_fib_level_value(levels: &[FibRetracementLevel]) -> f64 {
    const COMMON_EXTENSION_LEVELS: [f64; 9] =
        [1.272, 1.414, 1.618, 2.0, 2.272, 2.618, 3.0, 3.618, 4.236];
    for candidate in COMMON_EXTENSION_LEVELS {
        if !levels
            .iter()
            .any(|level| (level.value - candidate).abs() <= 0.0005)
        {
            return candidate;
        }
    }
    let max_level = levels
        .iter()
        .filter_map(|level| level.value.is_finite().then_some(level.value))
        .fold(1.0, f64::max);
    ((max_level + 0.5) * 1000.0).round() / 1000.0
}

fn show_fib_coordinates_editor(
    ui: &mut egui::Ui,
    anchors: &mut [DrawingAnchor],
    bars: &(impl BarSeries + ?Sized),
    interval_seconds: Option<i64>,
) -> bool {
    let mut changed = false;
    egui::Grid::new("fib_retracement_coordinates_grid")
        .num_columns(3)
        .spacing(egui::vec2(8.0, 6.0))
        .show(ui, |ui| {
            for index in 0..2 {
                ui.label(format!("Point {}", index + 1));
                let Some(anchor) = anchors.get(index).copied() else {
                    ui.add_enabled(false, egui::Label::new("Missing"));
                    ui.end_row();
                    continue;
                };
                let Some((mut logical, mut price)) =
                    fib_anchor_logical_price(anchor, bars, interval_seconds)
                else {
                    ui.add_enabled(false, egui::Label::new("Unavailable"));
                    ui.end_row();
                    continue;
                };
                let logical_changed = ui
                    .add(
                        egui::DragValue::new(&mut logical)
                            .speed(0.1)
                            .fixed_decimals(3),
                    )
                    .on_hover_text("Logical bar coordinate")
                    .changed();
                let price_speed = price_drag_speed(price);
                let price_decimals = price_drag_decimals(price);
                let price_changed = ui
                    .add(
                        egui::DragValue::new(&mut price)
                            .speed(price_speed)
                            .fixed_decimals(price_decimals),
                    )
                    .on_hover_text("Price")
                    .changed();
                if (logical_changed || price_changed) && logical.is_finite() && price.is_finite() {
                    anchors[index] = DrawingAnchor::LogicalPrice { logical, price };
                    changed = true;
                }
                ui.end_row();
            }
        });
    changed
}

fn show_position_settings_editor(
    ui: &mut egui::Ui,
    kind: DrawingKind,
    settings: &mut PositionDrawingSettings,
    style: &mut DrawingStyle,
    anchors: &mut Vec<DrawingAnchor>,
    bars: &(impl BarSeries + ?Sized),
    interval_seconds: Option<i64>,
) -> PositionSettingsEditorChange {
    let mut changed = PositionSettingsEditorChange::default();

    ui.label(
        egui::RichText::new("Style")
            .size(12.0)
            .color(egui::Color32::from_rgb(162, 168, 176)),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        changed.style |= show_drawing_color_menu(ui, kind, style);
        changed.style |= show_drawing_width_menu(ui, style);
        changed.style |= show_drawing_line_style_menu(ui, style);
    });

    ui.add_space(8.0);
    ui.horizontal(|ui| {
        changed.settings |= ui.checkbox(&mut settings.labels_visible, "Stats").changed();
        changed.settings |= ui
            .checkbox(&mut settings.prices_visible, "Price labels")
            .changed();
    });
    changed.settings |=
        show_chart_color_control(ui, "Target color", &mut settings.profit_fill_color);
    changed.settings |= show_chart_color_control(ui, "Stop color", &mut settings.risk_fill_color);

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new("Coordinates")
            .size(12.0)
            .color(egui::Color32::from_rgb(162, 168, 176)),
    );
    ui.add_space(4.0);
    changed.coordinates |=
        show_position_coordinates_editor(ui, kind, anchors, bars, interval_seconds);

    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui
            .button("Reset")
            .on_hover_text("Reset position colors and labels")
            .clicked()
        {
            *settings = PositionDrawingSettings::default();
            changed.settings = true;
        }
    });

    changed
}

fn show_position_coordinates_editor(
    ui: &mut egui::Ui,
    kind: DrawingKind,
    anchors: &mut Vec<DrawingAnchor>,
    bars: &(impl BarSeries + ?Sized),
    interval_seconds: Option<i64>,
) -> bool {
    let mut changed = false;
    egui::Grid::new("position_coordinates_grid")
        .num_columns(3)
        .spacing(egui::vec2(8.0, 6.0))
        .show(ui, |ui| {
            for (index, label) in ["Entry", "Target", "Stop"].iter().enumerate() {
                ui.label(*label);
                let Some(anchor) = anchors.get(index).copied() else {
                    ui.add_enabled(false, egui::Label::new("Missing"));
                    ui.end_row();
                    continue;
                };
                let Some((mut logical, mut price)) =
                    fib_anchor_logical_price(anchor, bars, interval_seconds)
                else {
                    ui.add_enabled(false, egui::Label::new("Unavailable"));
                    ui.end_row();
                    continue;
                };
                let logical_changed = ui
                    .add(
                        egui::DragValue::new(&mut logical)
                            .speed(0.1)
                            .fixed_decimals(3),
                    )
                    .on_hover_text("Logical bar coordinate")
                    .changed();
                let price_speed = price_drag_speed(price);
                let price_decimals = price_drag_decimals(price);
                let price_changed = ui
                    .add(
                        egui::DragValue::new(&mut price)
                            .speed(price_speed)
                            .fixed_decimals(price_decimals),
                    )
                    .on_hover_text("Price")
                    .changed();
                if (logical_changed || price_changed) && logical.is_finite() && price.is_finite() {
                    anchors[index] = DrawingAnchor::LogicalPrice { logical, price };
                    changed = true;
                }
                ui.end_row();
            }
        });
    if changed
        && let Some(normalized) =
            normalize_position_drawing_anchors(kind, anchors.clone(), bars, interval_seconds)
    {
        *anchors = normalized;
    }
    changed
}

fn fib_anchor_logical_price<B: BarSeries + ?Sized>(
    anchor: DrawingAnchor,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<(f64, f64)> {
    match anchor {
        DrawingAnchor::LogicalPrice { logical, price } => Some((logical, price)),
        DrawingAnchor::TimePrice { time, price } => Some((
            time_to_logical_for_bars(bars, time, interval_seconds)?,
            price,
        )),
        DrawingAnchor::StickedOhlc {
            time,
            field,
            price,
            source_interval_seconds,
        } => Some((
            time_to_logical_for_bars(bars, time, interval_seconds)?,
            sticked_ohlc_price_for_interval(
                bars,
                time,
                field,
                price,
                source_interval_seconds,
                interval_seconds,
            ),
        )),
        DrawingAnchor::TimeOnly { .. }
        | DrawingAnchor::PriceOnly { .. }
        | DrawingAnchor::Percent { .. } => None,
    }
}

fn position_anchor_with_price(anchor: DrawingAnchor, price: f64) -> Option<DrawingAnchor> {
    if !price.is_finite() {
        return None;
    }
    match anchor {
        DrawingAnchor::TimePrice { time, .. } => Some(DrawingAnchor::TimePrice { time, price }),
        DrawingAnchor::LogicalPrice { logical, .. } => {
            Some(DrawingAnchor::LogicalPrice { logical, price })
        }
        DrawingAnchor::StickedOhlc { time, .. } => Some(DrawingAnchor::TimePrice { time, price }),
        DrawingAnchor::TimeOnly { .. }
        | DrawingAnchor::PriceOnly { .. }
        | DrawingAnchor::Percent { .. } => None,
    }
}

fn position_anchor_with_logical<B: BarSeries + ?Sized>(
    anchor: DrawingAnchor,
    logical: f64,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<DrawingAnchor> {
    if !logical.is_finite() {
        return None;
    }
    match anchor {
        DrawingAnchor::TimePrice { price, .. } => Some(DrawingAnchor::TimePrice {
            time: logical_to_time_for_bars(bars, logical, interval_seconds)?,
            price,
        }),
        DrawingAnchor::LogicalPrice { price, .. } => {
            Some(DrawingAnchor::LogicalPrice { logical, price })
        }
        DrawingAnchor::StickedOhlc { price, .. } => Some(DrawingAnchor::TimePrice {
            time: logical_to_time_for_bars(bars, logical, interval_seconds)?,
            price,
        }),
        DrawingAnchor::TimeOnly { .. }
        | DrawingAnchor::PriceOnly { .. }
        | DrawingAnchor::Percent { .. } => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PositionAnchorRole {
    Target,
    Stop,
}

fn position_drawing_anchors_for_stage<B: BarSeries + ?Sized>(
    tool: DrawingTool,
    pane: PaneId,
    entry: DrawingAnchor,
    target: Option<DrawingAnchor>,
    stop: Option<DrawingAnchor>,
    chart: &ChartUiResponse,
    snapshot: &ChartSnapshot,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<Vec<DrawingAnchor>> {
    let transform = drawing_scene_transform_for_pane(pane, chart, snapshot)?;
    let (entry_logical, entry_price) = fib_anchor_logical_price(entry, bars, interval_seconds)?;
    let logical_span = position_default_logical_span(transform);
    let default_delta = position_default_price_delta(entry_price, transform);

    let target = match target {
        Some(anchor) => normalize_position_role_anchor(
            tool,
            entry_price,
            anchor,
            PositionAnchorRole::Target,
            bars,
            interval_seconds,
        )?,
        None => default_position_role_anchor(
            tool,
            PositionAnchorRole::Target,
            entry_logical + logical_span,
            entry_price,
            default_delta,
        )?,
    };
    let (target_logical, target_price) = fib_anchor_logical_price(target, bars, interval_seconds)?;
    let stop_delta = (target_price - entry_price).abs().max(default_delta);
    let stop = match stop {
        Some(anchor) => normalize_position_role_anchor(
            tool,
            entry_price,
            anchor,
            PositionAnchorRole::Stop,
            bars,
            interval_seconds,
        )?,
        None => default_position_role_anchor(
            tool,
            PositionAnchorRole::Stop,
            target_logical,
            entry_price,
            stop_delta,
        )?,
    };

    normalize_position_drawing_anchors(
        tool.kind(),
        vec![entry, target, stop],
        bars,
        interval_seconds,
    )
}

fn normalize_position_drawing_anchors<B: BarSeries + ?Sized>(
    kind: DrawingKind,
    mut anchors: Vec<DrawingAnchor>,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<Vec<DrawingAnchor>> {
    if !matches!(kind, DrawingKind::LongPosition | DrawingKind::ShortPosition) || anchors.len() != 3
    {
        return Some(anchors);
    }
    let tool = drawing_tool_for_kind(kind)?;
    let (_, entry_price) = fib_anchor_logical_price(anchors[0], bars, interval_seconds)?;
    anchors[1] = normalize_position_role_anchor(
        tool,
        entry_price,
        anchors[1],
        PositionAnchorRole::Target,
        bars,
        interval_seconds,
    )?;
    anchors[2] = normalize_position_role_anchor(
        tool,
        entry_price,
        anchors[2],
        PositionAnchorRole::Stop,
        bars,
        interval_seconds,
    )?;
    Some(anchors)
}

fn normalize_position_role_anchor<B: BarSeries + ?Sized>(
    tool: DrawingTool,
    entry_price: f64,
    anchor: DrawingAnchor,
    role: PositionAnchorRole,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<DrawingAnchor> {
    let (logical, price) = fib_anchor_logical_price(anchor, bars, interval_seconds)?;
    if position_role_price_is_valid(tool, role, entry_price, price) {
        return Some(anchor);
    }
    let delta = (price - entry_price)
        .abs()
        .max(position_min_price_delta(entry_price));
    default_position_role_anchor(tool, role, logical, entry_price, delta)
}

fn default_position_role_anchor(
    tool: DrawingTool,
    role: PositionAnchorRole,
    logical: f64,
    entry_price: f64,
    delta: f64,
) -> Option<DrawingAnchor> {
    if !logical.is_finite() || !entry_price.is_finite() || !delta.is_finite() {
        return None;
    }
    let price = match (tool, role) {
        (DrawingTool::LongPosition, PositionAnchorRole::Target)
        | (DrawingTool::ShortPosition, PositionAnchorRole::Stop) => entry_price + delta,
        (DrawingTool::LongPosition, PositionAnchorRole::Stop)
        | (DrawingTool::ShortPosition, PositionAnchorRole::Target) => entry_price - delta,
        _ => return None,
    };
    price
        .is_finite()
        .then_some(DrawingAnchor::LogicalPrice { logical, price })
}

fn position_role_price_is_valid(
    tool: DrawingTool,
    role: PositionAnchorRole,
    entry_price: f64,
    price: f64,
) -> bool {
    match (tool, role) {
        (DrawingTool::LongPosition, PositionAnchorRole::Target)
        | (DrawingTool::ShortPosition, PositionAnchorRole::Stop) => price > entry_price,
        (DrawingTool::LongPosition, PositionAnchorRole::Stop)
        | (DrawingTool::ShortPosition, PositionAnchorRole::Target) => price < entry_price,
        _ => false,
    }
}

fn position_default_logical_span(transform: SceneTransform) -> f64 {
    let visible_span = transform.time.span().abs();
    if !visible_span.is_finite() || visible_span <= f64::EPSILON {
        return 1.0;
    }
    let preferred = visible_span * 0.24;
    let viewport_cap = (visible_span * 0.45).max(0.25);
    preferred.clamp(0.25, 80.0).min(viewport_cap)
}

fn position_default_price_delta(entry_price: f64, transform: SceneTransform) -> f64 {
    let minimum = position_min_price_delta(entry_price);
    let visible_delta = transform.price.span().abs() * 0.12;
    let entry_cap = entry_price.abs().max(1.0) * 0.0045;
    visible_delta.min(entry_cap).max(minimum)
}

fn position_min_price_delta(entry_price: f64) -> f64 {
    entry_price.abs().max(1.0) * 0.0001
}

fn price_drag_speed(price: f64) -> f64 {
    (price.abs() * 0.001).clamp(0.0001, 10.0)
}

fn price_drag_decimals(price: f64) -> usize {
    if price.abs() >= 100.0 {
        2
    } else if price.abs() >= 1.0 {
        4
    } else {
        5
    }
}

fn fib_extend_mode(left: bool, right: bool) -> FibExtendMode {
    match (left, right) {
        (false, false) => FibExtendMode::None,
        (true, false) => FibExtendMode::Left,
        (false, true) => FibExtendMode::Right,
        (true, true) => FibExtendMode::Both,
    }
}

fn fib_extend_mode_label(mode: FibExtendMode) -> &'static str {
    match mode {
        FibExtendMode::None => "Don't extend",
        FibExtendMode::Left => "Extend left",
        FibExtendMode::Right => "Extend right",
        FibExtendMode::Both => "Extend both",
    }
}

fn set_fib_extend_mode(settings: &mut FibRetracementSettings, mode: FibExtendMode) -> bool {
    let (left, right) = match mode {
        FibExtendMode::None => (false, false),
        FibExtendMode::Left => (true, false),
        FibExtendMode::Right => (false, true),
        FibExtendMode::Both => (true, true),
    };
    let changed = settings.extend_left != left || settings.extend_right != right;
    settings.extend_left = left;
    settings.extend_right = right;
    changed
}

fn show_fib_level_color_menu(ui: &mut egui::Ui, color: &mut ChartColor, _id: egui::Id) -> bool {
    let mut changed = false;
    let button = egui::Button::new("")
        .min_size(egui::vec2(32.0, 24.0))
        .corner_radius(4.0);
    let (response, _) = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
        ui.set_min_width(430.0);
        for row in DRAWING_COLOR_PALETTE.chunks(10) {
            ui.horizontal(|ui| {
                for candidate in row {
                    if show_drawing_palette_swatch(ui, *candidate, *color == *candidate) {
                        *color = *candidate;
                        changed = true;
                    }
                }
            });
        }
    });
    let swatch = egui::Rect::from_center_size(response.rect.center(), egui::vec2(18.0, 18.0));
    ui.painter()
        .rect_filled(swatch, 3.0, chart_color_to_egui(*color));
    ui.painter().rect_stroke(
        swatch,
        3.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(220, 220, 220)),
        egui::StrokeKind::Inside,
    );
    response.on_hover_text("Level color");
    changed
}

fn is_line_style_drawing(kind: DrawingKind) -> bool {
    matches!(
        kind,
        DrawingKind::HorizontalLine
            | DrawingKind::HorizontalRay
            | DrawingKind::VerticalLine
            | DrawingKind::TrendLine
            | DrawingKind::Ray
            | DrawingKind::Brush
            | DrawingKind::FibRetracement
            | DrawingKind::LongPosition
            | DrawingKind::ShortPosition
            | DrawingKind::Rectangle
    )
}

fn is_style_toolbar_drawing(kind: DrawingKind) -> bool {
    is_line_style_drawing(kind)
        || matches!(
            kind,
            DrawingKind::Text | DrawingKind::AnchoredText | DrawingKind::PriceNote
        )
}

fn drawing_kind_label(kind: DrawingKind) -> &'static str {
    match kind {
        DrawingKind::HorizontalLine => "Horizontal",
        DrawingKind::HorizontalRay => "Horizontal ray",
        DrawingKind::VerticalLine => "Vertical",
        DrawingKind::TrendLine => "Trend",
        DrawingKind::Ray => "Ray",
        DrawingKind::Brush => "Brush",
        DrawingKind::FibRetracement => "Fib retracement",
        DrawingKind::LongPosition => "Long position",
        DrawingKind::ShortPosition => "Short position",
        DrawingKind::Rectangle => "Rectangle",
        DrawingKind::Text => "Text",
        DrawingKind::AnchoredText => "Anchored",
        DrawingKind::PriceNote => "Price note",
    }
}

fn drawing_settings_action_label(kind: DrawingKind) -> Option<&'static str> {
    match kind {
        DrawingKind::FibRetracement => Some("Fib settings"),
        DrawingKind::LongPosition | DrawingKind::ShortPosition => Some("Position settings"),
        _ => None,
    }
}

fn drawing_settings_toolbar_label(kind: DrawingKind) -> &'static str {
    match kind {
        DrawingKind::FibRetracement => "Fib",
        DrawingKind::LongPosition => "Long",
        DrawingKind::ShortPosition => "Short",
        _ => "Settings",
    }
}

fn drawing_tool_for_kind(kind: DrawingKind) -> Option<DrawingTool> {
    match kind {
        DrawingKind::HorizontalLine => Some(DrawingTool::HorizontalLine),
        DrawingKind::HorizontalRay => Some(DrawingTool::HorizontalRay),
        DrawingKind::VerticalLine => Some(DrawingTool::VerticalLine),
        DrawingKind::TrendLine => Some(DrawingTool::TrendLine),
        DrawingKind::Ray => Some(DrawingTool::Ray),
        DrawingKind::Brush => Some(DrawingTool::Brush),
        DrawingKind::FibRetracement => Some(DrawingTool::FibRetracement),
        DrawingKind::LongPosition => Some(DrawingTool::LongPosition),
        DrawingKind::ShortPosition => Some(DrawingTool::ShortPosition),
        DrawingKind::Rectangle => Some(DrawingTool::Rectangle),
        DrawingKind::Text => Some(DrawingTool::Text),
        DrawingKind::PriceNote => Some(DrawingTool::PriceNote),
        DrawingKind::AnchoredText => None,
    }
}

fn show_drawing_color_menu(ui: &mut egui::Ui, kind: DrawingKind, style: &mut DrawingStyle) -> bool {
    let mut changed = false;
    let button_size = egui::vec2(36.0, 24.0);
    let button = egui::Button::new("")
        .min_size(button_size)
        .corner_radius(4.0);
    let (response, _) = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
        ui.set_min_width(430.0);
        let current = drawing_style_color(kind, style);
        for row in DRAWING_COLOR_PALETTE.chunks(10) {
            ui.horizontal(|ui| {
                for color in row {
                    if show_drawing_palette_swatch(ui, *color, current == *color) {
                        set_drawing_style_color(kind, style, *color);
                        changed = true;
                    }
                }
            });
        }
        ui.separator();
        ui.label(
            egui::RichText::new("Opacity")
                .size(12.0)
                .color(egui::Color32::from_rgb(150, 154, 162)),
        );
        let mut opacity = f32::from(drawing_style_color(kind, style).a) / 255.0;
        if ui
            .add(egui::Slider::new(&mut opacity, 0.05..=1.0).show_value(true))
            .changed()
        {
            let mut color = drawing_style_color(kind, style);
            color.a = (opacity * 255.0).round().clamp(13.0, 255.0) as u8;
            set_drawing_style_color(kind, style, color);
            changed = true;
        }
    });
    paint_drawing_color_menu_button(
        ui.painter(),
        response.rect,
        drawing_style_color(kind, style),
    );
    response.on_hover_text("Color");
    changed
}

fn show_drawing_fill_color_menu(ui: &mut egui::Ui, style: &mut DrawingStyle) -> bool {
    let mut changed = false;
    let button = egui::Button::new("")
        .min_size(egui::vec2(36.0, 24.0))
        .corner_radius(4.0);
    let (response, _) = egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
        ui.set_min_width(430.0);
        if ui
            .selectable_label(style.fill_color.is_none(), "No fill")
            .clicked()
        {
            style.fill_color = None;
            changed = true;
            ui.close();
        }
        ui.separator();
        let current = style.fill_color;
        let alpha = current.map(|color| color.a).unwrap_or(64);
        for row in DRAWING_COLOR_PALETTE.chunks(10) {
            ui.horizontal(|ui| {
                for color in row {
                    let selected = current.is_some_and(|current| {
                        current.r == color.r && current.g == color.g && current.b == color.b
                    });
                    if show_drawing_palette_swatch(ui, *color, selected) {
                        let mut next = *color;
                        next.a = alpha;
                        style.fill_color = Some(next);
                        changed = true;
                    }
                }
            });
        }
        ui.separator();
        ui.label(
            egui::RichText::new("Fill opacity")
                .size(12.0)
                .color(egui::Color32::from_rgb(150, 154, 162)),
        );
        if let Some(mut fill) = style.fill_color {
            let mut opacity = f32::from(fill.a) / 255.0;
            if ui
                .add(egui::Slider::new(&mut opacity, 0.0..=1.0).show_value(true))
                .changed()
            {
                fill.a = (opacity * 255.0).round().clamp(0.0, 255.0) as u8;
                style.fill_color = Some(fill);
                changed = true;
            }
        } else {
            let mut opacity = 0.0;
            ui.add_enabled(
                false,
                egui::Slider::new(&mut opacity, 0.0..=1.0).show_value(true),
            );
        }
    });
    paint_drawing_fill_menu_button(ui.painter(), response.rect, style.fill_color);
    response.on_hover_text("Fill");
    changed
}

fn show_drawing_palette_swatch(ui: &mut egui::Ui, color: ChartColor, selected: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(34.0, 34.0), egui::Sense::click());
    let swatch = egui::Rect::from_center_size(rect.center(), egui::vec2(26.0, 26.0));
    ui.painter()
        .rect_filled(swatch, 3.0, chart_color_to_egui(color));
    if selected {
        ui.painter().rect_stroke(
            swatch.expand(4.0),
            5.0,
            egui::Stroke::new(2.0, egui::Color32::WHITE),
            egui::StrokeKind::Inside,
        );
    }
    response.clicked()
}

fn paint_drawing_color_menu_button(painter: &egui::Painter, rect: egui::Rect, color: ChartColor) {
    let swatch = egui::Rect::from_min_size(
        rect.left_center() + egui::vec2(8.0, -8.0),
        egui::vec2(16.0, 16.0),
    );
    painter.rect_filled(swatch, 3.0, chart_color_to_egui(color));
    painter.line_segment(
        [
            egui::pos2(rect.left() + 28.0, rect.center().y),
            egui::pos2(rect.right() - 8.0, rect.center().y),
        ],
        egui::Stroke::new(2.0, chart_color_to_egui(color)),
    );
}

fn paint_drawing_fill_menu_button(
    painter: &egui::Painter,
    rect: egui::Rect,
    fill_color: Option<ChartColor>,
) {
    let swatch = egui::Rect::from_center_size(rect.center(), egui::vec2(24.0, 16.0));
    if let Some(fill_color) = fill_color {
        painter.rect_filled(swatch, 3.0, chart_color_to_egui(fill_color));
    } else {
        painter.rect_filled(swatch, 3.0, egui::Color32::from_rgb(30, 30, 30));
        painter.line_segment(
            [swatch.left_top(), swatch.right_bottom()],
            egui::Stroke::new(1.2, egui::Color32::from_rgb(160, 160, 160)),
        );
    }
    painter.rect_stroke(
        swatch,
        3.0,
        egui::Stroke::new(1.2, egui::Color32::from_rgb(220, 220, 220)),
        egui::StrokeKind::Inside,
    );
}

fn drawing_style_color(kind: DrawingKind, style: &DrawingStyle) -> ChartColor {
    if matches!(kind, DrawingKind::Text | DrawingKind::AnchoredText) {
        style.text_color
    } else {
        style.stroke_color
    }
}

fn set_drawing_style_color(kind: DrawingKind, style: &mut DrawingStyle, color: ChartColor) {
    if matches!(kind, DrawingKind::Text | DrawingKind::AnchoredText) {
        style.text_color = color;
    } else {
        style.stroke_color = color;
        if matches!(kind, DrawingKind::PriceNote) {
            style.text_color = color;
        }
    }
}

fn selected_drawing_delete_shortcut_target(
    selected: Option<DrawingId>,
    text_editing_active: bool,
    egui_wants_keyboard_input: bool,
    delete_pressed: bool,
    backspace_pressed: bool,
) -> Option<DrawingId> {
    if text_editing_active || egui_wants_keyboard_input || !(delete_pressed || backspace_pressed) {
        return None;
    }
    selected
}

fn selected_drawing_cut_shortcut_target(
    selected: Option<DrawingId>,
    text_editing_active: bool,
    egui_wants_keyboard_input: bool,
    cut_pressed: bool,
) -> Option<DrawingId> {
    if text_editing_active || egui_wants_keyboard_input || !cut_pressed {
        return None;
    }
    selected
}

fn drawing_store_state_matches(left: &DrawingStore, right: &DrawingStore) -> bool {
    left.drawings() == right.drawings()
        && left.groups() == right.groups()
        && left.next_drawing_id() == right.next_drawing_id()
        && left.next_group_id() == right.next_group_id()
}

fn app_undo_snapshots_match(left: &AppUndoSnapshot, right: &AppUndoSnapshot) -> bool {
    drawing_store_state_matches(&left.drawings, &right.drawings)
        && left.selected_drawing_id == right.selected_drawing_id
        && left.study_instances == right.study_instances
        && left.selected_study_instance_id == right.selected_study_instance_id
        && left.study_settings_instance_id == right.study_settings_instance_id
        && left.selected_timeframe == right.selected_timeframe
}

fn push_app_snapshot_stack(stack: &mut Vec<AppUndoSnapshot>, snapshot: AppUndoSnapshot) -> bool {
    if stack
        .last()
        .is_some_and(|last| app_undo_snapshots_match(last, &snapshot))
    {
        return false;
    }
    stack.push(snapshot);
    if stack.len() > APP_UNDO_LIMIT {
        let overflow = stack.len() - APP_UNDO_LIMIT;
        stack.drain(0..overflow);
    }
    true
}

fn app_undo_shortcut(
    text_editing_active: bool,
    egui_wants_keyboard_input: bool,
    undo_pressed: bool,
    undo_available: bool,
) -> bool {
    !text_editing_active && !egui_wants_keyboard_input && undo_pressed && undo_available
}

fn app_redo_shortcut(
    text_editing_active: bool,
    egui_wants_keyboard_input: bool,
    redo_pressed: bool,
    redo_available: bool,
) -> bool {
    !text_editing_active && !egui_wants_keyboard_input && redo_pressed && redo_available
}

fn selected_drawing_blank_click_should_clear(
    selected_toolbar_visible: bool,
    hovered_drawing: Option<DrawingId>,
    primary_clicked: bool,
    chart_contains_pointer: bool,
) -> bool {
    selected_toolbar_visible
        && hovered_drawing.is_none()
        && primary_clicked
        && chart_contains_pointer
}

fn drawing_layer_blocks_chart_input(
    drawing_creation_active: bool,
    drawing_drag_active: bool,
    text_editing_active: bool,
    measure_handle_drag_active: bool,
    hovered_drawing_claims_pointer: bool,
) -> bool {
    drawing_creation_active
        || drawing_drag_active
        || text_editing_active
        || measure_handle_drag_active
        || hovered_drawing_claims_pointer
}

fn drawing_clipboard_shortcut(
    selected: Option<DrawingId>,
    clipboard_available: bool,
    text_editing_active: bool,
    keyboard_input_active: bool,
    copy_shortcut: bool,
    paste_shortcut: bool,
) -> Option<DrawingClipboardShortcut> {
    if text_editing_active || keyboard_input_active {
        return None;
    }
    if copy_shortcut {
        selected.map(DrawingClipboardShortcut::Copy)
    } else if paste_shortcut && clipboard_available {
        Some(DrawingClipboardShortcut::Paste)
    } else {
        None
    }
}

fn drawing_clipboard_event_flags(events: &[egui::Event]) -> (bool, bool) {
    let mut copy = false;
    let mut paste = false;
    for event in events {
        match event {
            egui::Event::Copy => copy = true,
            egui::Event::Paste(_) => paste = true,
            _ => {}
        }
    }
    (copy, paste)
}

fn drawing_cut_event_flag(events: &[egui::Event]) -> bool {
    events.iter().any(|event| matches!(event, egui::Event::Cut))
}

fn next_drawing_z_order(drawings: &DrawingStore) -> i32 {
    drawings
        .drawings()
        .iter()
        .map(|drawing| drawing.z_order)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

fn offset_drawing_anchors_for_paste<B: BarSeries + ?Sized>(
    drawing: &DrawingSpec,
    chart: &ChartUiResponse,
    snapshot: &ChartSnapshot,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<Vec<DrawingAnchor>> {
    let pane_rect = chart.pane_rect(drawing.pane)?;
    let transform = drawing_scene_transform_for_pane(drawing.pane, chart, snapshot)?;
    let start = pane_rect.center();
    let current = start + egui::vec2(18.0, 18.0);
    let delta = drawing_drag_delta(transform, start, current);
    drawing
        .anchors
        .iter()
        .copied()
        .map(|anchor| {
            moved_drawing_anchor_by_drag_delta(
                anchor,
                delta,
                pane_rect,
                transform,
                bars,
                interval_seconds,
            )
        })
        .collect()
}

fn apply_drawing_action_to_store(
    drawings: &mut DrawingStore,
    id: DrawingId,
    action: DrawingAction,
) -> Result<DrawingActionEffect, DrawingEditError> {
    let Some(drawing) = drawings.get(id) else {
        return Err(DrawingEditError::MissingDrawing(id));
    };
    let kind = drawing.kind;
    let currently_locked = drawing.locked;
    match action {
        DrawingAction::EditText => {
            if currently_locked {
                return Err(DrawingEditError::LockedDrawing(id));
            }
            if drawing_kind_is_text_editable(kind) {
                Ok(DrawingActionEffect::EditText)
            } else {
                Ok(DrawingActionEffect::Noop)
            }
        }
        DrawingAction::OpenSettings => {
            if drawing_settings_action_label(kind).is_some() {
                Ok(DrawingActionEffect::OpenSettings)
            } else {
                Ok(DrawingActionEffect::Noop)
            }
        }
        DrawingAction::ToggleLock => {
            let locked = !currently_locked;
            drawings.set_locked(id, locked);
            Ok(DrawingActionEffect::LockChanged(locked))
        }
        DrawingAction::Hide => {
            drawings.set_visible(id, false);
            Ok(DrawingActionEffect::Hidden)
        }
        DrawingAction::Delete => {
            drawings.remove(id)?;
            Ok(DrawingActionEffect::Deleted)
        }
    }
}

fn drawing_action_error_message(action: DrawingAction, error: DrawingEditError) -> String {
    let verb = match action {
        DrawingAction::EditText => "edited",
        DrawingAction::OpenSettings => "opened",
        DrawingAction::ToggleLock => "locked",
        DrawingAction::Hide => "hidden",
        DrawingAction::Delete => "deleted",
    };
    format!("Drawing could not be {verb}: {error:?}")
}

fn drawing_object_tree_entries(
    drawings: &DrawingStore,
    symbol_key: &str,
) -> Vec<DrawingObjectTreeEntry> {
    let mut entries = drawings
        .drawings()
        .iter()
        .filter(|drawing| drawing_matches_symbol_key(drawing, symbol_key))
        .map(|drawing| DrawingObjectTreeEntry {
            id: drawing.id,
            pane: drawing.pane,
            label: drawing_object_tree_label(drawing),
            visible: drawing.visible,
            locked: drawing.locked,
            selectable: drawing.selectable,
            group_id: drawing.group_id,
            z_order: drawing.z_order,
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| {
        (
            drawing_object_tree_pane_sort_key(entry.pane),
            entry.z_order,
            entry.id.0,
        )
    });
    entries
}

fn drawing_object_tree_label(drawing: &DrawingSpec) -> String {
    let kind = drawing_kind_label(drawing.kind);
    let Some(text) = drawing
        .text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    else {
        return kind.to_string();
    };
    format!("{kind} - {}", drawing_object_tree_text_preview(text, 28))
}

fn drawing_object_tree_text_preview(text: &str, max_chars: usize) -> String {
    let mut preview = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        preview.push_str("...");
    }
    preview
}

fn drawing_object_tree_pane_sort_key(pane: PaneId) -> (u8, u32) {
    match pane {
        PaneId::Main => (0, 0),
        PaneId::Volume => (1, 0),
        PaneId::Study(id) => (2, id),
    }
}

fn drawing_object_tree_pane_label(pane: PaneId) -> String {
    match pane {
        PaneId::Main => "Main pane".to_string(),
        PaneId::Volume => "Volume pane".to_string(),
        PaneId::Study(id) => format!("Study pane {id}"),
    }
}

fn drawing_object_tree_can_select(drawing: &DrawingSpec, symbol_key: &str) -> bool {
    drawing.visible && drawing.selectable && drawing_matches_symbol_key(drawing, symbol_key)
}

fn drawing_object_tree_can_group_with_selected(
    drawings: &DrawingStore,
    selected: Option<DrawingId>,
    target: DrawingId,
) -> bool {
    let Some(selected) = selected else {
        return false;
    };
    if selected == target {
        return false;
    }
    let Some(selected_drawing) = drawings.get(selected) else {
        return false;
    };
    let Some(target_drawing) = drawings.get(target) else {
        return false;
    };
    selected_drawing.group_id.is_none()
        && target_drawing.group_id.is_none()
        && selected_drawing.pane == target_drawing.pane
        && selected_drawing.symbol == target_drawing.symbol
}

fn show_drawing_object_tree_row(
    ui: &mut egui::Ui,
    entry: &DrawingObjectTreeEntry,
    selected: Option<DrawingId>,
    can_group: bool,
) -> Option<DrawingObjectTreeAction> {
    let mut action = None;
    let selected_row = selected == Some(entry.id);
    let fill = if selected_row {
        egui::Color32::from_rgb(42, 56, 86)
    } else {
        egui::Color32::from_rgb(30, 30, 30)
    };

    egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(
            1.0,
            if selected_row {
                egui::Color32::from_rgb(76, 111, 183)
            } else {
                egui::Color32::from_rgb(48, 48, 48)
            },
        ))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(7, 5))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    let select_enabled = entry.visible && entry.selectable;
                    let label_width = (ui.available_width() - 118.0).max(112.0);
                    let label_color = if select_enabled {
                        egui::Color32::from_rgb(232, 234, 238)
                    } else {
                        egui::Color32::from_rgb(125, 129, 138)
                    };
                    let label_response = ui.add_enabled(
                        select_enabled,
                        egui::Button::new(
                            egui::RichText::new(&entry.label)
                                .size(13.0)
                                .color(label_color),
                        )
                        .frame(false)
                        .min_size(egui::vec2(label_width, 24.0)),
                    );
                    if select_enabled && label_response.clicked() {
                        action = Some(DrawingObjectTreeAction::Select(entry.id));
                    }
                    if !select_enabled {
                        label_response.on_disabled_hover_text(if !entry.visible {
                            "Show drawing before selecting it"
                        } else {
                            "Drawing is not selectable"
                        });
                    }

                    ui.label(
                        egui::RichText::new(format!("z{}", entry.z_order))
                            .size(12.0)
                            .color(egui::Color32::from_rgb(150, 154, 162)),
                    );
                    if let Some(group_id) = entry.group_id {
                        ui.label(
                            egui::RichText::new(format!("G{}", group_id.0))
                                .size(12.0)
                                .color(egui::Color32::from_rgb(150, 170, 220)),
                        );
                    }
                });

                ui.add_space(3.0);
                ui.horizontal(|ui| {
                    if show_drawing_object_tree_action_button(
                        ui,
                        if entry.visible { "Hide" } else { "Show" },
                        true,
                        if entry.visible {
                            "Hide drawing"
                        } else {
                            "Show drawing"
                        },
                    ) {
                        action = Some(DrawingObjectTreeAction::ToggleVisible(entry.id));
                    }
                    if show_drawing_object_tree_action_button(
                        ui,
                        if entry.locked { "Unlock" } else { "Lock" },
                        true,
                        if entry.locked {
                            "Unlock drawing"
                        } else {
                            "Lock drawing"
                        },
                    ) {
                        action = Some(DrawingObjectTreeAction::ToggleLock(entry.id));
                    }
                    if show_drawing_object_tree_action_button(
                        ui,
                        "Del",
                        !entry.locked,
                        "Delete drawing",
                    ) {
                        action = Some(DrawingObjectTreeAction::Delete(entry.id));
                    }
                    ui.separator();
                    if show_drawing_object_tree_action_button(
                        ui,
                        "Top",
                        true,
                        "Bring drawing to front",
                    ) {
                        action = Some(DrawingObjectTreeAction::BringToFront(entry.id));
                    }
                    if show_drawing_object_tree_action_button(
                        ui,
                        "Up",
                        true,
                        "Bring drawing forward",
                    ) {
                        action = Some(DrawingObjectTreeAction::BringForward(entry.id));
                    }
                    if show_drawing_object_tree_action_button(
                        ui,
                        "Dn",
                        true,
                        "Send drawing backward",
                    ) {
                        action = Some(DrawingObjectTreeAction::SendBackward(entry.id));
                    }
                    if show_drawing_object_tree_action_button(
                        ui,
                        "Back",
                        true,
                        "Send drawing to back",
                    ) {
                        action = Some(DrawingObjectTreeAction::SendToBack(entry.id));
                    }
                    ui.separator();
                    if let Some(group_id) = entry.group_id {
                        if show_drawing_object_tree_action_button(
                            ui,
                            "Ungroup",
                            true,
                            "Ungroup drawings",
                        ) {
                            action = Some(DrawingObjectTreeAction::Ungroup(group_id));
                        }
                    } else if show_drawing_object_tree_action_button(
                        ui,
                        "Group",
                        can_group,
                        "Group with selected drawing",
                    ) {
                        action = Some(DrawingObjectTreeAction::GroupWithSelected(entry.id));
                    }
                });
            });
        });

    action
}

fn show_drawing_object_tree_action_button(
    ui: &mut egui::Ui,
    label: &str,
    enabled: bool,
    tooltip: &str,
) -> bool {
    let width = (label.len() as f32 * 7.0 + 18.0).clamp(32.0, 64.0);
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).size(12.0))
            .min_size(egui::vec2(width, 22.0))
            .corner_radius(3.0),
    );
    let clicked = enabled && response.clicked();
    response.on_hover_text(tooltip);
    clicked
}

fn show_drawing_line_style_menu(ui: &mut egui::Ui, style: &mut DrawingStyle) -> bool {
    let mut changed = false;
    let button = egui::Button::new(drawing_line_style_label(style.line_style))
        .min_size(egui::vec2(58.0, 24.0))
        .corner_radius(4.0);
    egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
        for line_style in [
            DrawingLineStyle::Solid,
            DrawingLineStyle::Dashed,
            DrawingLineStyle::Dotted,
        ] {
            if ui
                .selectable_label(
                    style.line_style == line_style,
                    drawing_line_style_label(line_style),
                )
                .clicked()
            {
                style.line_style = line_style;
                changed = true;
                ui.close();
            }
        }
    });
    changed
}

fn show_drawing_width_menu(ui: &mut egui::Ui, style: &mut DrawingStyle) -> bool {
    let mut changed = false;
    let button = egui::Button::new(format!("{:.0}px", style.stroke_width.round()))
        .min_size(egui::vec2(48.0, 24.0))
        .corner_radius(4.0);
    egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
        for width in [1.0, 2.0, 3.0, 4.0, 5.0] {
            if ui
                .selectable_label(
                    (style.stroke_width - width).abs() < f32::EPSILON,
                    format!("{width:.0}px"),
                )
                .clicked()
            {
                style.stroke_width = width;
                changed = true;
                ui.close();
            }
        }
    });
    changed
}

fn drawing_line_style_label(line_style: DrawingLineStyle) -> &'static str {
    match line_style {
        DrawingLineStyle::Solid => "Solid",
        DrawingLineStyle::Dashed => "Dash",
        DrawingLineStyle::Dotted => "Dot",
    }
}

fn timeframe_button_width(timeframe: &str) -> f32 {
    match timeframe.len() {
        1 => 30.0,
        2 => 41.0,
        _ => 51.0,
    }
}

fn show_context_menu_button(ui: &mut egui::Ui, label: &str) -> bool {
    show_context_menu_button_enabled(ui, label, true)
}

fn show_context_menu_button_enabled(ui: &mut egui::Ui, label: &str, enabled: bool) -> bool {
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(label).size(13.0).color(if enabled {
            egui::Color32::from_rgb(224, 226, 230)
        } else {
            egui::Color32::from_rgb(112, 116, 122)
        }))
        .frame(false)
        .min_size(egui::vec2(ui.available_width(), 26.0)),
    );
    if enabled {
        response.clicked()
    } else {
        response.on_disabled_hover_text("Unlock drawing first");
        false
    }
}

fn show_selected_drawing_action_button(
    ui: &mut egui::Ui,
    label: &str,
    tooltip: &str,
    enabled: bool,
) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new(label)
            .min_size(egui::vec2(selected_drawing_action_width(label), 24.0))
            .corner_radius(4.0),
    )
    .on_hover_text(tooltip)
    .clicked()
}

fn selected_drawing_action_width(label: &str) -> f32 {
    match label.len() {
        0..=3 => 36.0,
        4 => 44.0,
        _ => 54.0,
    }
}

fn drawing_lock_toolbar_label(locked: bool) -> &'static str {
    if locked { "Unlock" } else { "Lock" }
}

fn drawing_lock_action_label(locked: bool) -> &'static str {
    if locked {
        "Unlock drawing"
    } else {
        "Lock drawing"
    }
}

fn chart_context_target(chart: &ChartUiResponse, position: egui::Pos2) -> ChartContextTarget {
    chart_context_target_for_rects(
        chart.legend,
        chart.panes.as_slice(),
        chart.plot,
        chart.price_axis,
        chart.time_axis,
        position,
    )
}

fn chart_context_target_for_rects(
    legend: egui::Rect,
    panes: &[crate::chart_core::PaneRect<egui::Rect>],
    plot: egui::Rect,
    price_axis: egui::Rect,
    time_axis: egui::Rect,
    position: egui::Pos2,
) -> ChartContextTarget {
    if price_axis.contains(position) {
        return ChartContextTarget::PriceScale;
    }
    if time_axis.contains(position) {
        return ChartContextTarget::TimeScale;
    }
    if legend.contains(position) {
        return ChartContextTarget::Legend;
    }
    if plot.contains(position) {
        return ChartContextTarget::Plot;
    }
    panes
        .iter()
        .find(|pane| pane.rect.contains(position))
        .map(|pane| ChartContextTarget::Pane(pane.id))
        .unwrap_or(ChartContextTarget::Chart)
}

fn chart_context_target_for_drawing_hit(
    fallback: ChartContextTarget,
    drawing_hit: Option<DrawingHit>,
) -> ChartContextTarget {
    drawing_hit
        .map(|hit| ChartContextTarget::Drawing(hit.drawing_id))
        .unwrap_or(fallback)
}

fn chart_context_has_price_scale(target: ChartContextTarget) -> bool {
    matches!(
        target,
        ChartContextTarget::Plot | ChartContextTarget::PriceScale
    )
}

fn chart_context_has_time_scale(target: ChartContextTarget) -> bool {
    matches!(
        target,
        ChartContextTarget::Plot | ChartContextTarget::TimeScale
    )
}

fn scene_layout_from_chart_response(chart: &ChartUiResponse) -> SceneLayout {
    SceneLayout {
        outer: egui_rect_to_scene_rect(chart.outer),
        legend: egui_rect_to_scene_rect(chart.legend),
        panes: chart
            .panes
            .iter()
            .map(|pane| PaneRect {
                id: pane.id,
                rect: egui_rect_to_scene_rect(pane.rect),
            })
            .collect(),
        plot: egui_rect_to_scene_rect(chart.plot),
        volume: chart.pane_rect(PaneId::Volume).map(egui_rect_to_scene_rect),
        price_axis: egui_rect_to_scene_rect(chart.price_axis),
        time_axis: egui_rect_to_scene_rect(chart.time_axis),
    }
}

fn scene_snapshot_from_chart_snapshot(
    snapshot: &ChartSnapshot,
    layout: &SceneLayout,
) -> SceneSnapshot {
    SceneSnapshot {
        visible_bounds: snapshot.visible_bounds,
        price_range: snapshot.price_range,
        volume_range: snapshot.volume_range,
        study_ranges: snapshot.study_ranges.clone(),
        transform: SceneTransform::new(
            layout.plot,
            snapshot.transform.time,
            snapshot.transform.price,
        )
        .with_price_mode(snapshot.transform.price_mode),
    }
}

fn drawing_pane_at(chart: &ChartUiResponse, position: egui::Pos2) -> Option<(PaneId, egui::Rect)> {
    chart
        .panes
        .iter()
        .find(|pane| pane.rect.contains(position))
        .map(|pane| (pane.id, pane.rect))
}

fn measure_point_at_position<B: BarSeries + ?Sized>(
    chart: &ChartUiResponse,
    snapshot: &ChartSnapshot,
    bars: &B,
    position: egui::Pos2,
    interval_seconds: Option<i64>,
) -> Option<MeasurePoint> {
    let (pane, _) = drawing_pane_at(chart, position)?;
    let transform = drawing_scene_transform_for_pane(pane, chart, snapshot)?;
    let logical = nearest_logical_center_at_x(transform, position.x)?;
    let price = transform.y_to_price(position.y);
    if !logical.is_finite() || !price.is_finite() {
        return None;
    }
    Some(MeasurePoint {
        pane,
        logical,
        price,
        time: logical_to_time_for_bars(bars, logical, interval_seconds),
    })
}

fn nearest_logical_center_at_x(transform: SceneTransform, x: f32) -> Option<f64> {
    let logical = transform.x_to_logical(x);
    logical.is_finite().then(|| logical.round())
}

fn measure_point_screen_position(
    point: MeasurePoint,
    chart: &ChartUiResponse,
    snapshot: &ChartSnapshot,
) -> Option<egui::Pos2> {
    let transform = drawing_scene_transform_for_pane(point.pane, chart, snapshot)?;
    Some(egui::pos2(
        transform.logical_to_x(point.logical),
        transform.price_to_y(point.price),
    ))
}

fn measure_drag_target_at_position(
    state: MeasureState,
    chart: &ChartUiResponse,
    snapshot: &ChartSnapshot,
    position: egui::Pos2,
) -> Option<MeasureDragTarget> {
    let start = measure_point_screen_position(state.start, chart, snapshot)?;
    let current = measure_point_screen_position(state.current, chart, snapshot)?;
    let start_distance = start.distance(position);
    let current_distance = current.distance(position);
    let tolerance = 10.0;
    if start_distance <= tolerance && start_distance <= current_distance {
        Some(MeasureDragTarget::Start)
    } else if current_distance <= tolerance {
        Some(MeasureDragTarget::Current)
    } else {
        None
    }
}

fn measure_state_after_click(
    state: Option<MeasureState>,
    point: Option<MeasurePoint>,
    handle_target: Option<MeasureDragTarget>,
) -> Option<MeasureState> {
    match state {
        None => point.map(|point| MeasureState {
            start: point,
            current: point,
            phase: MeasurePhase::Placing,
        }),
        Some(mut state) => match state.phase {
            MeasurePhase::Placing => {
                if let Some(point) = point
                    && point.pane == state.start.pane
                {
                    state.current = point;
                    state.phase = MeasurePhase::Placed;
                }
                Some(state)
            }
            MeasurePhase::Placed => {
                if let Some(target) = handle_target {
                    state.phase = MeasurePhase::Adjusting(target);
                    Some(state)
                } else if point.is_some() {
                    None
                } else {
                    Some(state)
                }
            }
            MeasurePhase::Adjusting(_) => Some(state),
        },
    }
}

fn measure_state_after_hover(mut state: MeasureState, point: MeasurePoint) -> MeasureState {
    if state.phase == MeasurePhase::Placing && point.pane == state.start.pane {
        state.current = point;
    }
    state
}

fn measure_state_after_adjust_drag(
    mut state: MeasureState,
    target: MeasureDragTarget,
    point: MeasurePoint,
) -> MeasureState {
    if point.pane == state.start.pane {
        match target {
            MeasureDragTarget::Start => state.start = point,
            MeasureDragTarget::Current => state.current = point,
        }
    }
    state
}

fn measure_state_after_release(mut state: MeasureState) -> MeasureState {
    if matches!(state.phase, MeasurePhase::Adjusting(_)) {
        state.phase = MeasurePhase::Placed;
    }
    state
}

fn measure_state_blocks_chart_input(state: MeasureState) -> bool {
    matches!(state.phase, MeasurePhase::Adjusting(_))
}

fn measure_state_cleared_by_chart_click(state: MeasureState) -> bool {
    state.phase == MeasurePhase::Placed
}

fn measure_stats<B: BarSeries + ?Sized>(
    start: MeasurePoint,
    current: MeasurePoint,
    bars: &B,
) -> MeasureStats {
    let price_delta = current.price - start.price;
    let percent_delta =
        (start.price.abs() > f64::EPSILON).then_some(price_delta / start.price * 100.0);
    let bars_delta = (current.logical - start.logical).round().abs() as i64;
    let seconds_delta = start
        .time
        .zip(current.time)
        .map(|(start, current)| current.saturating_sub(start).abs());
    let volume = measure_volume(start.logical, current.logical, bars);
    MeasureStats {
        price_delta,
        percent_delta,
        bars_delta,
        seconds_delta,
        volume,
    }
}

fn measure_volume<B: BarSeries + ?Sized>(
    start_logical: f64,
    current_logical: f64,
    bars: &B,
) -> Option<f64> {
    if bars.is_empty() || !start_logical.is_finite() || !current_logical.is_finite() {
        return None;
    }
    let last_index = bars.len().saturating_sub(1) as i64;
    let start = (start_logical.round() as i64).clamp(0, last_index);
    let end = (current_logical.round() as i64).clamp(0, last_index);
    let (from, to) = if start <= end {
        (start, end)
    } else {
        (end, start)
    };
    let mut volume = 0.0;
    let mut count = 0;
    for index in from..=to {
        if let Some(bar) = bars.bar(index as usize)
            && bar.volume.is_finite()
        {
            volume += bar.volume.max(0.0);
            count += 1;
        }
    }
    (count > 0).then_some(volume)
}

fn format_measure_stats_lines(stats: MeasureStats) -> Vec<String> {
    let percent = stats
        .percent_delta
        .map(|value| format!("{value:.2}%"))
        .unwrap_or_else(|| "n/a".to_string());
    let duration = stats
        .seconds_delta
        .map(format_measure_duration)
        .unwrap_or_else(|| "n/a".to_string());
    let mut lines = vec![
        format!("{:.2} ({percent})", stats.price_delta),
        format!("{} bars, {duration}", stats.bars_delta),
    ];
    if let Some(volume) = stats.volume {
        lines.push(format!("Vol {}", format_measure_volume(volume)));
    }
    lines
}

#[cfg(test)]
fn format_measure_stats(stats: MeasureStats) -> String {
    format_measure_stats_lines(stats).join("  ")
}

fn format_measure_duration(seconds: i64) -> String {
    let seconds = seconds.max(0);
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{seconds}s")
    }
}

fn format_measure_volume(value: f64) -> String {
    let abs = value.abs();
    if abs >= 1_000_000_000.0 {
        format!("{:.2} B", value / 1_000_000_000.0)
    } else if abs >= 1_000_000.0 {
        format!("{:.2} M", value / 1_000_000.0)
    } else if abs >= 1_000.0 {
        format!("{:.2} K", value / 1_000.0)
    } else {
        format!("{value:.0}")
    }
}

fn paint_measure_arrow_line(
    painter: &egui::Painter,
    start: egui::Pos2,
    end: egui::Pos2,
    color: egui::Color32,
) {
    let stroke = egui::Stroke::new(1.2, color);
    painter.line_segment([start, end], stroke);
    let delta = end - start;
    let length = delta.length();
    if length <= 1.0 {
        return;
    }
    let direction = delta / length;
    let normal = egui::vec2(-direction.y, direction.x);
    let head_len = 8.0;
    let head_width = 5.0;
    let base = end - direction * head_len;
    painter.line_segment([end, base + normal * head_width], stroke);
    painter.line_segment([end, base - normal * head_width], stroke);
}

fn clamp_label_position(position: egui::Pos2, size: egui::Vec2, bounds: egui::Rect) -> egui::Pos2 {
    let margin = 6.0;
    let max_x = (bounds.right() - size.x - margin).max(bounds.left() + margin);
    let max_y = (bounds.bottom() - size.y - margin).max(bounds.top() + margin);
    egui::pos2(
        position.x.clamp(bounds.left() + margin, max_x),
        position.y.clamp(bounds.top() + margin, max_y),
    )
}

fn drawing_anchor_for_tool<B: BarSeries + ?Sized>(
    tool: DrawingTool,
    pane: PaneId,
    position: egui::Pos2,
    chart: &ChartUiResponse,
    snapshot: &ChartSnapshot,
    bars: &B,
    magnet_mode: DrawingMagnetMode,
    interval_seconds: Option<i64>,
) -> Option<DrawingAnchor> {
    let transform = drawing_scene_transform_for_pane(pane, chart, snapshot)?;
    let snap = magnet_snap_for_position(pane, transform, position, bars, magnet_mode);
    match tool {
        DrawingTool::HorizontalLine => {
            let price = snap
                .map(|snap| snap.price)
                .unwrap_or_else(|| transform.y_to_price(position.y));
            price
                .is_finite()
                .then_some(DrawingAnchor::PriceOnly { price })
        }
        DrawingTool::VerticalLine => snap
            .map(|snap| DrawingAnchor::TimeOnly { time: snap.time })
            .or_else(|| time_anchor_at(transform, position, bars, interval_seconds)),
        DrawingTool::HorizontalRay
        | DrawingTool::TrendLine
        | DrawingTool::Ray
        | DrawingTool::FibRetracement
        | DrawingTool::LongPosition
        | DrawingTool::ShortPosition
        | DrawingTool::Rectangle
        | DrawingTool::Text
        | DrawingTool::PriceNote => snap
            .map(|snap| sticked_ohlc_anchor_from_snap(snap, interval_seconds))
            .or_else(|| time_price_anchor_at(transform, position, bars, interval_seconds)),
        DrawingTool::Brush => brush_anchor_at_position(transform, position, bars, interval_seconds),
    }
}

fn brush_anchor_at_position<B: BarSeries + ?Sized>(
    transform: SceneTransform,
    position: egui::Pos2,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<DrawingAnchor> {
    let price = transform.y_to_price(position.y);
    if !price.is_finite() {
        return None;
    }
    if !bars.is_empty() {
        let logical = unclamped_logical_at_x(transform, position.x);
        return logical_to_time_for_bars(bars, logical, interval_seconds)
            .map(|time| DrawingAnchor::TimePrice { time, price });
    }
    logical_price_anchor_at_unclamped(transform, position)
}

fn brush_anchor_for_position<B: BarSeries + ?Sized>(
    pane: PaneId,
    position: egui::Pos2,
    chart: &ChartUiResponse,
    snapshot: &ChartSnapshot,
    bars: &B,
    _magnet_mode: DrawingMagnetMode,
    interval_seconds: Option<i64>,
) -> Option<DrawingAnchor> {
    let transform = drawing_scene_transform_for_pane(pane, chart, snapshot)?;
    brush_anchor_at_position(transform, position, bars, interval_seconds)
}

fn brush_sampled_anchors<B: BarSeries + ?Sized>(
    current: &[DrawingAnchor],
    candidate: DrawingAnchor,
    pane: PaneId,
    chart: &ChartUiResponse,
    snapshot: &ChartSnapshot,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<Vec<DrawingAnchor>> {
    let pane_rect = chart.pane_rect(pane)?;
    let transform = drawing_scene_transform_for_pane(pane, chart, snapshot)?;
    let mut anchors = current.to_vec();
    if anchors.is_empty() {
        return Some(vec![candidate, candidate]);
    }
    if anchors.len() == 1 {
        anchors.push(candidate);
        return Some(anchors);
    }
    if anchors.len() == 2 && anchors[0] == anchors[1] {
        anchors[1] = candidate;
        return Some(anchors);
    }

    let basis_index = anchors.len().saturating_sub(2);
    let basis = drawing_anchor_screen_point(
        anchors[basis_index],
        pane_rect,
        transform,
        bars,
        interval_seconds,
    )?;
    let candidate_point =
        drawing_anchor_screen_point(candidate, pane_rect, transform, bars, interval_seconds)?;
    if basis.distance(candidate_point) >= BRUSH_SAMPLE_MIN_DISTANCE_PX {
        if let Some(last) = anchors.last_mut() {
            *last = candidate;
        }
        anchors.push(candidate);
    } else if let Some(last) = anchors.last_mut() {
        *last = candidate;
    }
    Some(anchors)
}

fn brush_compacted_anchors<B: BarSeries + ?Sized>(
    anchors: &[DrawingAnchor],
    pane: PaneId,
    chart: &ChartUiResponse,
    snapshot: &ChartSnapshot,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<Vec<DrawingAnchor>> {
    let pane_rect = chart.pane_rect(pane)?;
    let transform = drawing_scene_transform_for_pane(pane, chart, snapshot)?;
    let mut compacted = Vec::new();
    let mut last_point = None;
    for anchor in anchors.iter().copied() {
        let point =
            drawing_anchor_screen_point(anchor, pane_rect, transform, bars, interval_seconds)?;
        if last_point
            .is_none_or(|last: egui::Pos2| last.distance(point) >= BRUSH_SAMPLE_MIN_DISTANCE_PX)
        {
            compacted.push(anchor);
            last_point = Some(point);
        }
    }
    (compacted.len() >= 2).then_some(compacted)
}

fn brush_has_visible_span<B: BarSeries + ?Sized>(
    anchors: &[DrawingAnchor],
    pane: PaneId,
    chart: &ChartUiResponse,
    snapshot: &ChartSnapshot,
    bars: &B,
    interval_seconds: Option<i64>,
) -> bool {
    let Some(pane_rect) = chart.pane_rect(pane) else {
        return false;
    };
    let Some(transform) = drawing_scene_transform_for_pane(pane, chart, snapshot) else {
        return false;
    };
    let mut points = anchors.iter().copied().filter_map(|anchor| {
        drawing_anchor_screen_point(anchor, pane_rect, transform, bars, interval_seconds)
    });
    let Some(first) = points.next() else {
        return false;
    };
    points.any(|point| first.distance(point) >= BRUSH_SAMPLE_MIN_DISTANCE_PX)
}

fn logical_price_anchor_at(
    transform: SceneTransform,
    position: egui::Pos2,
) -> Option<DrawingAnchor> {
    let logical = transform.x_to_logical(position.x);
    let price = transform.y_to_price(position.y);
    (logical.is_finite() && price.is_finite())
        .then_some(DrawingAnchor::LogicalPrice { logical, price })
}

fn logical_price_anchor_at_unclamped(
    transform: SceneTransform,
    position: egui::Pos2,
) -> Option<DrawingAnchor> {
    let logical = unclamped_logical_at_x(transform, position.x);
    let coordinate = unclamped_price_coordinate_at_y(transform, position.y);
    let price = transform.price_mode.value(coordinate)?;
    (logical.is_finite() && price.is_finite())
        .then_some(DrawingAnchor::LogicalPrice { logical, price })
}

fn moved_drawing_anchors<B: BarSeries + ?Sized>(
    start_anchors: &[DrawingAnchor],
    pane: PaneId,
    start_position: egui::Pos2,
    current_position: egui::Pos2,
    chart: &ChartUiResponse,
    snapshot: &ChartSnapshot,
    bars: &B,
    magnet_mode: DrawingMagnetMode,
    interval_seconds: Option<i64>,
) -> Option<Vec<DrawingAnchor>> {
    let transform = drawing_scene_transform_for_pane(pane, chart, snapshot)?;
    let pane_rect = chart.pane_rect(pane)?;
    let current_position = magnet_drag_position(
        pane,
        transform,
        current_position,
        bars,
        magnet_mode,
        interval_seconds,
    )
    .unwrap_or(current_position);
    let delta = drawing_drag_delta(transform, start_position, current_position);
    start_anchors
        .iter()
        .copied()
        .map(|anchor| {
            moved_drawing_anchor_by_drag_delta(
                anchor,
                delta,
                pane_rect,
                transform,
                bars,
                interval_seconds,
            )
        })
        .collect()
}

fn magnet_drag_position<B: BarSeries + ?Sized>(
    pane: PaneId,
    transform: SceneTransform,
    position: egui::Pos2,
    bars: &B,
    magnet_mode: DrawingMagnetMode,
    interval_seconds: Option<i64>,
) -> Option<egui::Pos2> {
    let snap = magnet_snap_for_position(pane, transform, position, bars, magnet_mode)?;
    let logical = time_to_logical_for_bars(bars, snap.time, interval_seconds)?;
    Some(egui::pos2(
        transform.logical_to_x(logical),
        transform.price_to_y(snap.price),
    ))
}

#[derive(Debug, Clone, Copy)]
struct DrawingDragDelta {
    logical: f64,
    coordinate: f64,
    screen: egui::Vec2,
}

impl DrawingDragDelta {
    fn is_zero(self) -> bool {
        self.logical.abs() < 1e-9
            && self.coordinate.abs() < 1e-9
            && self.screen.x.abs() < 1e-6
            && self.screen.y.abs() < 1e-6
    }
}

fn drawing_drag_delta(
    transform: SceneTransform,
    start_position: egui::Pos2,
    current_position: egui::Pos2,
) -> DrawingDragDelta {
    DrawingDragDelta {
        logical: unclamped_logical_at_x(transform, current_position.x)
            - unclamped_logical_at_x(transform, start_position.x),
        coordinate: unclamped_price_coordinate_at_y(transform, current_position.y)
            - unclamped_price_coordinate_at_y(transform, start_position.y),
        screen: current_position - start_position,
    }
}

fn moved_drawing_anchor_by_drag_delta<B: BarSeries + ?Sized>(
    anchor: DrawingAnchor,
    delta: DrawingDragDelta,
    pane_rect: egui::Rect,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<DrawingAnchor> {
    if delta.is_zero() {
        return Some(anchor);
    }

    match anchor {
        DrawingAnchor::TimePrice { time, price } => Some(DrawingAnchor::TimePrice {
            time: move_time_by_logical_delta(time, delta.logical, bars, interval_seconds)?,
            price: move_price_by_coordinate_delta(price, delta.coordinate, transform.price_mode)?,
        }),
        DrawingAnchor::TimeOnly { time } => Some(DrawingAnchor::TimeOnly {
            time: move_time_by_logical_delta(time, delta.logical, bars, interval_seconds)?,
        }),
        DrawingAnchor::PriceOnly { price } => Some(DrawingAnchor::PriceOnly {
            price: move_price_by_coordinate_delta(price, delta.coordinate, transform.price_mode)?,
        }),
        DrawingAnchor::LogicalPrice { logical, price } => Some(DrawingAnchor::LogicalPrice {
            logical: logical + delta.logical,
            price: move_price_by_coordinate_delta(price, delta.coordinate, transform.price_mode)?,
        }),
        DrawingAnchor::StickedOhlc {
            time,
            field,
            price,
            source_interval_seconds,
        } => {
            let moved_time =
                move_time_by_logical_delta(time, delta.logical, bars, interval_seconds)?;
            let resolved_price = sticked_ohlc_price_for_interval(
                bars,
                time,
                field,
                price,
                source_interval_seconds,
                interval_seconds,
            );
            Some(DrawingAnchor::TimePrice {
                time: moved_time,
                price: move_price_by_coordinate_delta(
                    resolved_price,
                    delta.coordinate,
                    transform.price_mode,
                )?,
            })
        }
        DrawingAnchor::Percent { x, y } => {
            let next_x = x + delta.screen.x / pane_rect.width().max(1.0);
            let next_y = y + delta.screen.y / pane_rect.height().max(1.0);
            Some(DrawingAnchor::Percent {
                x: next_x.clamp(0.0, 1.0),
                y: next_y.clamp(0.0, 1.0),
            })
        }
    }
}

fn move_time_by_logical_delta<B: BarSeries + ?Sized>(
    time: i64,
    logical_delta: f64,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<i64> {
    let logical = time_to_logical_for_bars(bars, time, interval_seconds)? + logical_delta;
    logical_to_time_for_bars(bars, logical, interval_seconds)
}

fn move_price_by_coordinate_delta(
    price: f64,
    coordinate_delta: f64,
    value_mode: PriceScaleValueMode,
) -> Option<f64> {
    let coordinate = value_mode.coordinate(price)? + coordinate_delta;
    let price = value_mode.value(coordinate)?;
    price.is_finite().then_some(price)
}

fn unclamped_logical_at_x(transform: SceneTransform, x: f32) -> f64 {
    let fraction = f64::from((x - transform.plot.left()) / transform.plot.width().max(1.0));
    transform.time.from - 0.5 + fraction * transform.time.span()
}

fn unclamped_price_coordinate_at_y(transform: SceneTransform, y: f32) -> f64 {
    let coordinate_range = transform.price_mode.coordinate_range(transform.price);
    let normalized = 1.0 - f64::from((y - transform.plot.top()) / transform.plot.height().max(1.0));
    coordinate_range.min + normalized * coordinate_range.span()
}

fn resized_drawing_anchors<B: BarSeries + ?Sized>(
    start_anchors: &[DrawingAnchor],
    kind: DrawingKind,
    anchor_index: usize,
    pane: PaneId,
    current_position: egui::Pos2,
    chart: &ChartUiResponse,
    snapshot: &ChartSnapshot,
    bars: &B,
    magnet_mode: DrawingMagnetMode,
    interval_seconds: Option<i64>,
) -> Option<Vec<DrawingAnchor>> {
    if kind == DrawingKind::Rectangle {
        return resized_rectangle_anchors(
            start_anchors,
            anchor_index,
            pane,
            current_position,
            chart,
            snapshot,
            bars,
            magnet_mode,
            interval_seconds,
        );
    }
    if matches!(kind, DrawingKind::LongPosition | DrawingKind::ShortPosition) {
        return resized_position_drawing_anchors(
            start_anchors,
            kind,
            anchor_index,
            pane,
            current_position,
            chart,
            snapshot,
            bars,
            magnet_mode,
            interval_seconds,
        );
    }
    let transform = drawing_scene_transform_for_pane(pane, chart, snapshot)?;
    let pane_rect = chart.pane_rect(pane)?;
    let mut anchors = start_anchors.to_vec();
    let original = *anchors.get(anchor_index)?;
    anchors[anchor_index] = moved_anchor_from_screen_point(
        original,
        pane,
        current_position,
        pane_rect,
        transform,
        bars,
        magnet_mode,
        interval_seconds,
    )?;
    Some(anchors)
}

const POSITION_WIDTH_HANDLE_INDEX: usize = 3;

fn resized_position_drawing_anchors<B: BarSeries + ?Sized>(
    start_anchors: &[DrawingAnchor],
    kind: DrawingKind,
    anchor_index: usize,
    pane: PaneId,
    current_position: egui::Pos2,
    chart: &ChartUiResponse,
    snapshot: &ChartSnapshot,
    bars: &B,
    magnet_mode: DrawingMagnetMode,
    interval_seconds: Option<i64>,
) -> Option<Vec<DrawingAnchor>> {
    if start_anchors.len() != 3 {
        return None;
    }
    let transform = drawing_scene_transform_for_pane(pane, chart, snapshot)?;
    let current_position = magnet_drag_position(
        pane,
        transform,
        current_position,
        bars,
        magnet_mode,
        interval_seconds,
    )
    .unwrap_or(current_position);
    resized_position_drawing_anchors_for_transform(
        start_anchors,
        kind,
        anchor_index,
        current_position,
        transform,
        bars,
        interval_seconds,
    )
}

fn resized_position_drawing_anchors_for_transform<B: BarSeries + ?Sized>(
    start_anchors: &[DrawingAnchor],
    kind: DrawingKind,
    anchor_index: usize,
    current_position: egui::Pos2,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<Vec<DrawingAnchor>> {
    if start_anchors.len() != 3 {
        return None;
    }
    let mut anchors = start_anchors.to_vec();
    let price = transform.price_mode.value(unclamped_price_coordinate_at_y(
        transform,
        current_position.y,
    ))?;
    if anchor_index == POSITION_WIDTH_HANDLE_INDEX {
        let (entry_logical, _) = fib_anchor_logical_price(anchors[0], bars, interval_seconds)?;
        let logical =
            unclamped_logical_at_x(transform, current_position.x).max(entry_logical + 0.25);
        anchors[1] = position_anchor_with_logical(anchors[1], logical, bars, interval_seconds)?;
        anchors[2] = position_anchor_with_logical(anchors[2], logical, bars, interval_seconds)?;
    } else {
        let anchor = *anchors.get(anchor_index)?;
        anchors[anchor_index] = position_anchor_with_price(anchor, price)?;
    }
    normalize_position_drawing_anchors(kind, anchors, bars, interval_seconds)
}

fn resized_rectangle_anchors<B: BarSeries + ?Sized>(
    start_anchors: &[DrawingAnchor],
    corner_index: usize,
    pane: PaneId,
    current_position: egui::Pos2,
    chart: &ChartUiResponse,
    snapshot: &ChartSnapshot,
    bars: &B,
    magnet_mode: DrawingMagnetMode,
    interval_seconds: Option<i64>,
) -> Option<Vec<DrawingAnchor>> {
    let transform = drawing_scene_transform_for_pane(pane, chart, snapshot)?;
    let pane_rect = chart.pane_rect(pane)?;
    let corners =
        rectangle_screen_corners(start_anchors, pane_rect, transform, bars, interval_seconds)?;
    let opposite_corner = *corners.get(rectangle_opposite_corner_index(corner_index)?)?;
    let dragged = drawing_model_anchor_at_screen_position(
        pane,
        current_position,
        transform,
        bars,
        magnet_mode,
        interval_seconds,
    )?;
    let opposite = logical_price_anchor_at_unclamped(transform, opposite_corner)?;

    Some(match corner_index {
        0 | 3 => vec![dragged, opposite],
        1 | 2 => vec![opposite, dragged],
        _ => return None,
    })
}

fn rectangle_opposite_corner_index(corner_index: usize) -> Option<usize> {
    match corner_index {
        0 => Some(2),
        1 => Some(3),
        2 => Some(0),
        3 => Some(1),
        _ => None,
    }
}

fn rectangle_screen_corners<B: BarSeries + ?Sized>(
    anchors: &[DrawingAnchor],
    pane_rect: egui::Rect,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<[egui::Pos2; 4]> {
    let first = drawing_anchor_screen_point(
        *anchors.first()?,
        pane_rect,
        transform,
        bars,
        interval_seconds,
    )?;
    let second = drawing_anchor_screen_point(
        *anchors.get(1)?,
        pane_rect,
        transform,
        bars,
        interval_seconds,
    )?;
    let left = first.x.min(second.x);
    let right = first.x.max(second.x);
    let top = first.y.min(second.y);
    let bottom = first.y.max(second.y);
    Some([
        egui::pos2(left, top),
        egui::pos2(right, top),
        egui::pos2(right, bottom),
        egui::pos2(left, bottom),
    ])
}

fn drawing_model_anchor_at_screen_position<B: BarSeries + ?Sized>(
    pane: PaneId,
    position: egui::Pos2,
    transform: SceneTransform,
    bars: &B,
    magnet_mode: DrawingMagnetMode,
    interval_seconds: Option<i64>,
) -> Option<DrawingAnchor> {
    magnet_snap_for_position(pane, transform, position, bars, magnet_mode)
        .map(|snap| sticked_ohlc_anchor_from_snap(snap, interval_seconds))
        .or_else(|| logical_price_anchor_at_unclamped(transform, position))
}

fn drawing_anchor_screen_point<B: BarSeries + ?Sized>(
    anchor: DrawingAnchor,
    pane_rect: egui::Rect,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<egui::Pos2> {
    match anchor {
        DrawingAnchor::TimePrice { time, price } => Some(egui::pos2(
            transform.logical_to_x(time_to_logical_for_bars(bars, time, interval_seconds)?),
            transform.price_to_y(price),
        )),
        DrawingAnchor::TimeOnly { time } => Some(egui::pos2(
            transform.logical_to_x(time_to_logical_for_bars(bars, time, interval_seconds)?),
            pane_rect.center().y,
        )),
        DrawingAnchor::PriceOnly { price } => Some(egui::pos2(
            pane_rect.center().x,
            transform.price_to_y(price),
        )),
        DrawingAnchor::LogicalPrice { logical, price } => Some(egui::pos2(
            transform.logical_to_x(logical),
            transform.price_to_y(price),
        )),
        DrawingAnchor::Percent { x, y } => Some(egui::pos2(
            pane_rect.left() + pane_rect.width() * x,
            pane_rect.top() + pane_rect.height() * y,
        )),
        DrawingAnchor::StickedOhlc {
            time,
            field,
            price,
            source_interval_seconds,
        } => Some(egui::pos2(
            transform.logical_to_x(time_to_logical_for_bars(bars, time, interval_seconds)?),
            transform.price_to_y(sticked_ohlc_price_for_interval(
                bars,
                time,
                field,
                price,
                source_interval_seconds,
                interval_seconds,
            )),
        )),
    }
}

fn drawing_anchor_overlay_position<B: BarSeries + ?Sized>(
    drawing: &chart_core::DrawingSpec,
    chart: &ChartUiResponse,
    snapshot: &ChartSnapshot,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<egui::Pos2> {
    let pane_rect = chart.pane_rect(drawing.pane)?;
    let transform = drawing_scene_transform_for_pane(drawing.pane, chart, snapshot)?;
    drawing_anchor_screen_point(
        drawing.anchors.first().copied()?,
        pane_rect,
        transform,
        bars,
        interval_seconds,
    )
}

fn moved_anchor_from_screen_point<B: BarSeries + ?Sized>(
    original: DrawingAnchor,
    pane: PaneId,
    point: egui::Pos2,
    pane_rect: egui::Rect,
    transform: SceneTransform,
    bars: &B,
    magnet_mode: DrawingMagnetMode,
    interval_seconds: Option<i64>,
) -> Option<DrawingAnchor> {
    let snap = magnet_snap_for_position(pane, transform, point, bars, magnet_mode);
    match original {
        DrawingAnchor::TimePrice { .. } | DrawingAnchor::StickedOhlc { .. } => snap
            .map(|snap| sticked_ohlc_anchor_from_snap(snap, interval_seconds))
            .or_else(|| time_price_anchor_at(transform, point, bars, interval_seconds)),
        DrawingAnchor::TimeOnly { .. } => snap
            .map(|snap| DrawingAnchor::TimeOnly { time: snap.time })
            .or_else(|| time_anchor_at(transform, point, bars, interval_seconds)),
        DrawingAnchor::PriceOnly { .. } => {
            let price = snap
                .map(|snap| snap.price)
                .unwrap_or_else(|| transform.y_to_price(point.y));
            price
                .is_finite()
                .then_some(DrawingAnchor::PriceOnly { price })
        }
        DrawingAnchor::LogicalPrice { .. } => snap
            .map(|snap| sticked_ohlc_anchor_from_snap(snap, interval_seconds))
            .or_else(|| time_price_anchor_at(transform, point, bars, interval_seconds)),
        DrawingAnchor::Percent { .. } => {
            let x = ((point.x - pane_rect.left()) / pane_rect.width()).clamp(0.0, 1.0);
            let y = ((point.y - pane_rect.top()) / pane_rect.height()).clamp(0.0, 1.0);
            (x.is_finite() && y.is_finite()).then_some(DrawingAnchor::Percent { x, y })
        }
    }
}

fn sticked_ohlc_anchor_from_snap(
    snap: MagnetSnap,
    source_interval_seconds: Option<i64>,
) -> DrawingAnchor {
    DrawingAnchor::StickedOhlc {
        time: snap.time,
        field: snap.field,
        price: snap.price,
        source_interval_seconds,
    }
}

fn time_price_anchor_at<B: BarSeries + ?Sized>(
    transform: SceneTransform,
    position: egui::Pos2,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<DrawingAnchor> {
    let price = transform.y_to_price(position.y);
    if !price.is_finite() {
        return None;
    }
    time_for_position(transform, position.x, bars, interval_seconds)
        .map(|time| DrawingAnchor::TimePrice { time, price })
        .or_else(|| logical_price_anchor_at(transform, position))
}

fn time_anchor_at<B: BarSeries + ?Sized>(
    transform: SceneTransform,
    position: egui::Pos2,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<DrawingAnchor> {
    time_for_position(transform, position.x, bars, interval_seconds)
        .map(|time| DrawingAnchor::TimeOnly { time })
        .or_else(|| logical_price_anchor_at(transform, position))
}

fn time_for_position<B: BarSeries + ?Sized>(
    transform: SceneTransform,
    x: f32,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<i64> {
    if bars.is_empty() || x < transform.plot.left() || x > transform.plot.right() {
        return None;
    }
    let logical = nearest_logical_center_at_x(transform, x)?;
    logical_to_time_for_bars(bars, logical, interval_seconds)
}

fn magnet_snap_for_position<B: BarSeries + ?Sized>(
    pane: PaneId,
    transform: SceneTransform,
    position: egui::Pos2,
    bars: &B,
    mode: DrawingMagnetMode,
) -> Option<MagnetSnap> {
    if mode == DrawingMagnetMode::Off || pane != PaneId::Main || bars.is_empty() {
        return None;
    }
    let logical = transform.x_to_logical(position.x);
    if !logical.is_finite() || logical < -0.5 || logical > bars.len() as f64 - 0.5 {
        return None;
    }
    let index = logical
        .round()
        .clamp(0.0, bars.len().saturating_sub(1) as f64) as usize;
    let bar = bars.bar(index)?;
    let (field, snapped_price) = [
        (OhlcField::Open, bar.open),
        (OhlcField::High, bar.high),
        (OhlcField::Low, bar.low),
        (OhlcField::Close, bar.close),
    ]
    .into_iter()
    .filter(|(_, price)| price.is_finite())
    .min_by(|left, right| {
        let left_distance = (transform.price_to_y(left.1) - position.y).abs();
        let right_distance = (transform.price_to_y(right.1) - position.y).abs();
        left_distance.total_cmp(&right_distance)
    })?;
    let distance = (transform.price_to_y(snapped_price) - position.y).abs();
    if mode == DrawingMagnetMode::Weak && distance > MAGNET_WEAK_PRICE_DISTANCE_PX {
        return None;
    }
    Some(MagnetSnap {
        time: bar.time,
        logical: index as f64,
        price: snapped_price,
        field,
    })
}

fn ohlc_field_value(bar: OhlcBar, field: OhlcField) -> f64 {
    match field {
        OhlcField::Open => bar.open,
        OhlcField::High => bar.high,
        OhlcField::Low => bar.low,
        OhlcField::Close => bar.close,
    }
}

fn drawing_scene_transform_for_pane(
    pane: PaneId,
    chart: &ChartUiResponse,
    snapshot: &ChartSnapshot,
) -> Option<SceneTransform> {
    let rect = chart.pane_rect(pane).map(egui_rect_to_scene_rect)?;
    match pane {
        PaneId::Main => Some(
            SceneTransform::new(rect, snapshot.transform.time, snapshot.price_range)
                .with_price_mode(snapshot.transform.price_mode),
        ),
        PaneId::Volume => Some(SceneTransform::new(
            rect,
            snapshot.transform.time,
            snapshot.volume_range?,
        )),
        PaneId::Study(_) => {
            let range = snapshot
                .study_ranges
                .iter()
                .find(|range| range.id == pane)
                .map(|range| range.rect)
                .unwrap_or(PriceRange {
                    min: 0.0,
                    max: 100.0,
                });
            Some(SceneTransform::new(rect, snapshot.transform.time, range))
        }
    }
}

#[cfg(test)]
fn nearest_bar_time_at_x<B: BarSeries + ?Sized>(
    transform: SceneTransform,
    x: f32,
    bars: &B,
) -> Option<i64> {
    if bars.is_empty() {
        return None;
    }
    let index = transform
        .x_to_logical(x)
        .round()
        .clamp(0.0, bars.len().saturating_sub(1) as f64) as usize;
    bars.bar(index).map(|bar| bar.time)
}

fn exact_bar_for_time<B: BarSeries + ?Sized>(bars: &B, time: i64) -> Option<(f64, OhlcBar)> {
    let normalized = normalize_timestamp_seconds(time);
    for index in 0..bars.len() {
        let Some(bar) = bars.bar(index) else {
            continue;
        };
        if normalize_timestamp_seconds(bar.time) == normalized {
            return Some((index as f64, bar));
        }
    }
    None
}

fn time_to_logical_for_bars<B: BarSeries + ?Sized>(
    bars: &B,
    time: i64,
    interval_seconds: Option<i64>,
) -> Option<f64> {
    if bars.is_empty() {
        return None;
    }
    if let Some((logical, _)) = exact_bar_for_time(bars, time) {
        return Some(logical);
    }

    let target = normalize_timestamp_seconds(time);
    let mut previous: Option<(usize, i64)> = None;
    for index in 0..bars.len() {
        let Some(bar) = bars.bar(index) else {
            continue;
        };
        let current = normalize_timestamp_seconds(bar.time);
        if current > target {
            if let Some((previous_index, previous_time)) = previous {
                let span = (current - previous_time) as f64;
                if span > 0.0 {
                    let ratio = (target - previous_time) as f64 / span;
                    return Some(previous_index as f64 + ratio.clamp(0.0, 1.0));
                }
            }
            return extrapolated_time_to_logical(bars, target, interval_seconds);
        }
        previous = Some((index, current));
    }

    extrapolated_time_to_logical(bars, target, interval_seconds)
}

fn logical_to_time_for_bars<B: BarSeries + ?Sized>(
    bars: &B,
    logical: f64,
    interval_seconds: Option<i64>,
) -> Option<i64> {
    if bars.is_empty() || !logical.is_finite() {
        return None;
    }

    let last_index = bars.len().saturating_sub(1);
    if logical >= 0.0 && logical <= last_index as f64 {
        let lower_index = logical.floor() as usize;
        let upper_index = logical.ceil() as usize;
        let lower_time = normalize_timestamp_seconds(bars.bar(lower_index.min(last_index))?.time);
        let upper_time = normalize_timestamp_seconds(bars.bar(upper_index.min(last_index))?.time);
        let fraction = logical.fract();
        return Some(
            (lower_time as f64 + (upper_time - lower_time) as f64 * fraction).round() as i64,
        );
    }

    let first = normalize_timestamp_seconds(bars.bar(0)?.time);
    let last = normalize_timestamp_seconds(bars.bar(last_index)?.time);
    let step = interval_seconds
        .filter(|seconds| *seconds > 0)
        .or_else(|| inferred_bar_seconds(bars))
        .unwrap_or(1) as f64;
    if logical < 0.0 {
        Some((first as f64 + logical * step).round() as i64)
    } else {
        Some((last as f64 + (logical - last_index as f64) * step).round() as i64)
    }
}

fn extrapolated_time_to_logical<B: BarSeries + ?Sized>(
    bars: &B,
    target: i64,
    interval_seconds: Option<i64>,
) -> Option<f64> {
    let first = normalize_timestamp_seconds(bars.bar(0)?.time);
    let last_index = bars.len().saturating_sub(1);
    let last = normalize_timestamp_seconds(bars.bar(last_index)?.time);
    let step = interval_seconds
        .filter(|seconds| *seconds > 0)
        .or_else(|| inferred_bar_seconds(bars))
        .unwrap_or(1) as f64;
    if target < first {
        Some((target - first) as f64 / step)
    } else {
        Some(last_index as f64 + (target - last) as f64 / step)
    }
}

fn inferred_bar_seconds<B: BarSeries + ?Sized>(bars: &B) -> Option<i64> {
    let first = normalize_timestamp_seconds(bars.bar(0)?.time);
    let second = normalize_timestamp_seconds(bars.bar(1)?.time);
    let delta = second - first;
    (delta > 0).then_some(delta)
}

fn sticked_ohlc_price_for_interval<B: BarSeries + ?Sized>(
    bars: &B,
    time: i64,
    field: OhlcField,
    fallback_price: f64,
    source_interval_seconds: Option<i64>,
    current_interval_seconds: Option<i64>,
) -> f64 {
    let same_interval = source_interval_seconds.is_none()
        || current_interval_seconds.is_some_and(|current| Some(current) == source_interval_seconds);
    if same_interval {
        if let Some((_, bar)) = exact_bar_for_time(bars, time) {
            return ohlc_field_value(bar, field);
        }
    }
    fallback_price
}

fn egui_rect_to_scene_rect(rect: egui::Rect) -> SceneRect {
    SceneRect::from_min_max(
        egui_pos_to_scene_point(rect.min),
        egui_pos_to_scene_point(rect.max),
    )
}

fn egui_pos_to_scene_point(point: egui::Pos2) -> ScenePoint {
    ScenePoint::new(point.x, point.y)
}

fn show_chart_preset_controls(
    ui: &mut egui::Ui,
    presets: &ChartPresetCollection,
    preset_name_input: &mut String,
) -> Option<ChartPresetUiAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.label("Template");
        let active_label = presets
            .active_preset()
            .map(|preset| preset.name.as_str())
            .unwrap_or("Default template");
        egui::ComboBox::from_id_salt(ui.id().with("chart_preset_picker"))
            .selected_text(active_label)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(
                        presets.active_preset_id.is_none(),
                        egui::RichText::new("Default template"),
                    )
                    .clicked()
                {
                    action = Some(ChartPresetUiAction::ClearActive);
                }
                for preset in &presets.presets {
                    if ui
                        .selectable_label(
                            presets.active_preset_id.as_deref() == Some(preset.id.as_str()),
                            egui::RichText::new(&preset.name),
                        )
                        .clicked()
                    {
                        action = Some(ChartPresetUiAction::Apply(preset.id.clone()));
                    }
                }
            });
    });
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(preset_name_input)
                .hint_text("Preset name")
                .desired_width(150.0),
        );
        if ui.button("Save").clicked() {
            action = Some(ChartPresetUiAction::SaveAs);
        }
        let has_active = presets.active_preset().is_some();
        if ui
            .add_enabled(has_active, egui::Button::new("Update"))
            .clicked()
        {
            action = Some(ChartPresetUiAction::UpdateActive);
        }
        if ui
            .add_enabled(has_active, egui::Button::new("Delete"))
            .clicked()
        {
            action = Some(ChartPresetUiAction::DeleteActive);
        }
    });
    action
}

fn show_chart_symbol_settings(ui: &mut egui::Ui, theme: &mut ChartTheme) -> bool {
    let mut changed = false;
    ui.label(
        egui::RichText::new("Candles")
            .size(12.0)
            .color(egui::Color32::from_rgb(162, 168, 176)),
    );
    ui.add_space(4.0);
    changed |= show_chart_color_control(ui, "Up body", &mut theme.candles.up_body);
    changed |= show_chart_color_control(ui, "Down body", &mut theme.candles.down_body);
    changed |= show_chart_color_control(ui, "Up wick", &mut theme.candles.up_wick);
    changed |= show_chart_color_control(ui, "Down wick", &mut theme.candles.down_wick);
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new("Volume")
            .size(12.0)
            .color(egui::Color32::from_rgb(162, 168, 176)),
    );
    ui.add_space(4.0);
    changed |= show_chart_color_control(ui, "Up volume", &mut theme.candles.volume_up);
    changed |= show_chart_color_control(ui, "Down volume", &mut theme.candles.volume_down);
    changed
}

fn show_chart_appearance_settings(
    ui: &mut egui::Ui,
    theme: &mut ChartTheme,
    volume_visible: &mut bool,
) -> bool {
    let mut changed = false;
    ui.label(
        egui::RichText::new("Panes")
            .size(12.0)
            .color(egui::Color32::from_rgb(162, 168, 176)),
    );
    ui.add_space(4.0);
    ui.checkbox(volume_visible, "Volume");
    changed |= show_chart_color_control(ui, "Background", &mut theme.appearance.pane_background);
    changed |=
        show_chart_color_control(ui, "Axis background", &mut theme.appearance.axis_background);
    changed |= show_chart_color_control(ui, "Separator", &mut theme.appearance.separator);
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new("Grid")
            .size(12.0)
            .color(egui::Color32::from_rgb(162, 168, 176)),
    );
    ui.add_space(4.0);
    changed |= ui
        .checkbox(&mut theme.appearance.grid.visible, "Grid")
        .changed();
    changed |= show_chart_color_control(ui, "Price grid", &mut theme.appearance.grid.price_color);
    changed |= show_chart_color_control(ui, "Time grid", &mut theme.appearance.grid.time_color);
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new("Crosshair")
            .size(12.0)
            .color(egui::Color32::from_rgb(162, 168, 176)),
    );
    ui.add_space(4.0);
    changed |= ui
        .checkbox(&mut theme.crosshair.visible, "Crosshair")
        .changed();
    changed |= show_chart_color_control(ui, "Line", &mut theme.crosshair.line);
    changed |= show_chart_color_control(ui, "Label fill", &mut theme.crosshair.label_fill);
    changed |= show_chart_color_control(ui, "Label text", &mut theme.crosshair.label_text);
    changed
}

fn show_chart_color_control(ui: &mut egui::Ui, label: &str, color: &mut ChartColor) -> bool {
    let mut color32 = chart_color_to_egui(*color);
    let changed = ui
        .horizontal(|ui| {
            ui.label(label);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.color_edit_button_srgba(&mut color32).changed()
            })
            .inner
        })
        .inner;
    if changed {
        *color = chart_color_from_egui(color32);
    }
    changed
}

fn chart_color_to_egui(color: ChartColor) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

fn chart_color_from_egui(color: egui::Color32) -> ChartColor {
    let [r, g, b, a] = color.to_array();
    ChartColor::rgba(r, g, b, a)
}

fn show_chart_scale_settings(ui: &mut egui::Ui, chart: &mut TvChartState) {
    ui.label(
        egui::RichText::new("Price scale")
            .size(12.0)
            .color(egui::Color32::from_rgb(162, 168, 176)),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Mode");
        let current =
            chart_price_mode_choice(chart.price_scales.scale(PriceScaleId::Right).value_mode());
        egui::ComboBox::from_id_salt("chart_price_scale_mode")
            .selected_text(chart_price_mode_label(current))
            .show_ui(ui, |ui| {
                for choice in chart_price_mode_choices() {
                    if ui
                        .selectable_label(choice == current, chart_price_mode_label(choice))
                        .clicked()
                    {
                        chart
                            .price_scales
                            .scale_mut(PriceScaleId::Right)
                            .set_value_mode(chart_price_mode_value(choice));
                        ui.close();
                    }
                }
            });
    });
    ui.add_space(8.0);
    if ui.button("Reset price scale").clicked() {
        chart.price_scales.reset(PriceScaleId::Right);
    }
}

fn chart_price_mode_choices() -> [ChartPriceModeChoice; 5] {
    [
        ChartPriceModeChoice::Regular,
        ChartPriceModeChoice::Percent,
        ChartPriceModeChoice::Indexed,
        ChartPriceModeChoice::Logarithmic,
        ChartPriceModeChoice::Inverted,
    ]
}

fn chart_price_mode_choice(mode: PriceScaleValueMode) -> ChartPriceModeChoice {
    match mode {
        PriceScaleValueMode::Linear => ChartPriceModeChoice::Regular,
        PriceScaleValueMode::Percent { .. } => ChartPriceModeChoice::Percent,
        PriceScaleValueMode::Indexed { .. } => ChartPriceModeChoice::Indexed,
        PriceScaleValueMode::Log => ChartPriceModeChoice::Logarithmic,
        PriceScaleValueMode::Inverse => ChartPriceModeChoice::Inverted,
    }
}

fn chart_price_mode_value(choice: ChartPriceModeChoice) -> PriceScaleValueMode {
    match choice {
        ChartPriceModeChoice::Regular => PriceScaleValueMode::Linear,
        ChartPriceModeChoice::Percent => PriceScaleValueMode::Percent { base: 1.0 },
        ChartPriceModeChoice::Indexed => PriceScaleValueMode::Indexed { base: 1.0 },
        ChartPriceModeChoice::Logarithmic => PriceScaleValueMode::Log,
        ChartPriceModeChoice::Inverted => PriceScaleValueMode::Inverse,
    }
}

fn chart_price_mode_label(choice: ChartPriceModeChoice) -> &'static str {
    match choice {
        ChartPriceModeChoice::Regular => "Regular",
        ChartPriceModeChoice::Percent => "Percent",
        ChartPriceModeChoice::Indexed => "Indexed to 100",
        ChartPriceModeChoice::Logarithmic => "Logarithmic",
        ChartPriceModeChoice::Inverted => "Invert scale",
    }
}

fn toolbar_scale(available_width: f32) -> f32 {
    (available_width / 720.0).clamp(0.68, 1.0)
}

fn live_chart_host_controls_width(mode: WidgetHostMode, scale: f32) -> f32 {
    let labels: &[&str] = match mode {
        WidgetHostMode::InShellWorkspaceSurface => &["OUT", "MIN"],
        WidgetHostMode::DetachedNativeViewport => &["FS", "IN", "MIN"],
    };
    let button_width: f32 = labels
        .iter()
        .map(|label| toolbar_toggle_width(label, scale * 0.82))
        .sum();
    let gap_width = LIVE_CHART_HOST_CONTROL_GAP * scale * labels.len().saturating_sub(1) as f32;
    button_width + gap_width
}

fn live_chart_header_control_rects(
    inner_rect: egui::Rect,
    host_width: f32,
    scale: f32,
) -> (egui::Rect, egui::Rect) {
    let host_left = (inner_rect.right() - host_width).max(inner_rect.left());
    let host_rect = egui::Rect::from_min_max(
        egui::pos2(host_left, inner_rect.top()),
        inner_rect.right_bottom(),
    );
    let content_right =
        (host_rect.left() - LIVE_CHART_HEADER_HOST_GAP * scale).max(inner_rect.left());
    let content_rect = egui::Rect::from_min_max(
        inner_rect.left_top(),
        egui::pos2(content_right, inner_rect.bottom()),
    );
    (content_rect, host_rect)
}

fn chart_local_overlay_order() -> egui::Order {
    egui::Order::Middle
}

fn header_symbol(symbol: &str) -> String {
    if symbol.eq_ignore_ascii_case("BTCUSDT") {
        "BTCUSDT.P".to_string()
    } else {
        symbol.to_string()
    }
}

fn drawing_symbol_key(source: MarketSource, symbol: &str) -> String {
    format!("{}:{}", source.label(), symbol)
}

fn drawing_matches_symbol_key(drawing: &DrawingSpec, symbol_key: &str) -> bool {
    drawing.symbol.as_deref() == Some(symbol_key)
}

fn search_palette_chart_click_should_cancel(
    search_open: bool,
    palette_rect: Option<egui::Rect>,
    chart_outer: egui::Rect,
    primary_clicked: bool,
    position: Option<egui::Pos2>,
) -> bool {
    if !search_open || !primary_clicked {
        return false;
    }
    let Some(position) = position else {
        return false;
    };
    chart_outer.contains(position) && !palette_rect.is_some_and(|rect| rect.contains(position))
}

fn floating_panels_chart_click_should_cancel(
    any_open: bool,
    panel_rects: &[Option<egui::Rect>],
    chart_outer: egui::Rect,
    primary_clicked: bool,
    position: Option<egui::Pos2>,
) -> bool {
    if !any_open || !primary_clicked {
        return false;
    }
    let Some(position) = position else {
        return false;
    };
    chart_outer.contains(position)
        && !panel_rects
            .iter()
            .flatten()
            .any(|rect| rect.contains(position))
}

fn floating_panels_chart_click_should_block_chart_input(
    any_open: bool,
    panel_rects: &[Option<egui::Rect>],
    chart_outer: egui::Rect,
    primary_clicked: bool,
    position: Option<egui::Pos2>,
    popup_open: bool,
) -> bool {
    if !any_open || !primary_clicked {
        return false;
    }
    let Some(position) = position else {
        return false;
    };
    chart_outer.contains(position)
        && (popup_open
            || panel_rects
                .iter()
                .flatten()
                .any(|rect| rect.contains(position)))
}

fn typed_search_prefix(events: &[egui::Event]) -> Option<String> {
    let mut typed = String::new();
    for event in events {
        if let egui::Event::Text(text) = event {
            typed.extend(text.chars().filter(|ch| type_to_search_char(*ch)));
        }
    }
    (!typed.is_empty()).then_some(typed)
}

fn type_to_search_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '.' | '/' | '-' | '_' | ':')
}

fn typed_timeframe_prefix(events: &[egui::Event]) -> Option<String> {
    let mut typed = String::new();
    for event in events {
        if let egui::Event::Text(text) = event {
            typed.extend(text.chars().filter(|ch| ch.is_ascii_alphanumeric()));
        }
    }
    typed
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_digit())
        .then_some(typed)
}

fn parse_timeframe_input(input: &str) -> Option<TimeframeInputCandidate> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

    let digit_count = input.chars().take_while(|ch| ch.is_ascii_digit()).count();
    if digit_count == 0 {
        return None;
    }
    let (amount, suffix) = input.split_at(digit_count);
    let amount = amount.parse::<u32>().ok().filter(|amount| *amount > 0)?;
    let suffix = suffix.trim().to_ascii_lowercase();
    let unit = match suffix.as_str() {
        "" => TimeframeInputUnit::Minutes,
        "h" | "hr" | "hour" | "hours" => TimeframeInputUnit::Hours,
        "d" | "day" | "days" => TimeframeInputUnit::Days,
        "w" | "wk" | "week" | "weeks" => TimeframeInputUnit::Weeks,
        "m" | "mo" | "mon" | "month" | "months" => TimeframeInputUnit::Months,
        _ => {
            return Some(TimeframeInputCandidate {
                interval: None,
                unit: TimeframeInputUnit::Minutes,
                amount,
            });
        }
    };
    Some(TimeframeInputCandidate {
        interval: timeframe_for_unit_amount(unit, amount),
        unit,
        amount,
    })
}

fn timeframe_for_unit_amount(unit: TimeframeInputUnit, amount: u32) -> Option<BarInterval> {
    let resolution = match unit {
        TimeframeInputUnit::Minutes => Resolution::Minutes(amount),
        TimeframeInputUnit::Hours => Resolution::Hours(amount),
        TimeframeInputUnit::Days => Resolution::Days(amount),
        TimeframeInputUnit::Weeks => Resolution::Weeks(amount),
        TimeframeInputUnit::Months => Resolution::Months(amount),
    };
    TIMEFRAMES
        .iter()
        .copied()
        .find(|timeframe| Resolution::from(*timeframe) == resolution)
}

fn timeframe_input_preview(candidate: Option<TimeframeInputCandidate>, raw_input: &str) -> String {
    let Some(candidate) = candidate else {
        return "Enter interval".to_string();
    };
    if candidate.interval.is_none() {
        return format!("Unsupported interval: {}", raw_input.trim());
    }
    plural_timeframe_label(candidate.amount, candidate.unit)
}

fn plural_timeframe_label(amount: u32, unit: TimeframeInputUnit) -> String {
    let singular = match unit {
        TimeframeInputUnit::Minutes => "minute",
        TimeframeInputUnit::Hours => "hour",
        TimeframeInputUnit::Days => "day",
        TimeframeInputUnit::Weeks => "week",
        TimeframeInputUnit::Months => "month",
    };
    if amount == 1 {
        format!("1 {singular}")
    } else {
        format!("{amount} {singular}s")
    }
}

fn search_candidates(
    query: &str,
    source_filter: SearchSourceFilter,
    type_filter: SearchTypeFilter,
) -> Vec<SearchCandidate> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    for source in SEARCH_SOURCES {
        if !source_matches_filter(source, source_filter) {
            continue;
        }
        let symbol = normalized_symbol(source, query);
        if symbol.is_empty() {
            continue;
        }
        for symbol_type in source_symbol_types(source) {
            if !type_matches_filter(*symbol_type, type_filter) {
                continue;
            }
            candidates.push(SearchCandidate {
                source,
                symbol_type: *symbol_type,
                symbol: symbol.clone(),
            });
        }
    }

    candidates.truncate(8);
    candidates
}

fn best_search_candidate(
    query: &str,
    current_source: MarketSource,
    source_filter: SearchSourceFilter,
    type_filter: SearchTypeFilter,
) -> Option<SearchCandidate> {
    let candidates = search_candidates(query, source_filter, type_filter);
    candidates
        .iter()
        .find(|candidate| candidate.source == current_source)
        .cloned()
        .or_else(|| candidates.into_iter().next())
}

fn source_matches_filter(source: MarketSource, filter: SearchSourceFilter) -> bool {
    match filter {
        SearchSourceFilter::All => true,
        SearchSourceFilter::Source(selected) => source == selected,
    }
}

fn type_matches_filter(symbol_type: SymbolType, filter: SearchTypeFilter) -> bool {
    filter
        .symbol_type()
        .is_none_or(|selected| selected == symbol_type)
}

fn source_symbol_types(source: MarketSource) -> &'static [SymbolType] {
    match source {
        MarketSource::YFinance => &YFINANCE_SEARCH_TYPES,
        MarketSource::Massive => &MASSIVE_SEARCH_TYPES,
        MarketSource::Binance | MarketSource::CoinGecko => &CRYPTO_SEARCH_TYPES,
    }
}

fn symbol_type_label(symbol_type: SymbolType) -> &'static str {
    match symbol_type {
        SymbolType::Stock => "Stock",
        SymbolType::Futures => "Futures",
        SymbolType::Forex => "Forex",
        SymbolType::Crypto => "Crypto",
        SymbolType::Index => "Index",
        SymbolType::Fund => "Fund",
        SymbolType::Unknown => "Unknown",
    }
}

fn study_legend_entries(instances: &[StudyInstance]) -> Vec<StudyLegendEntry> {
    instances
        .iter()
        .map(|instance| StudyLegendEntry {
            id: instance.id,
            pane: instance.pane(),
            label: instance.display_name(),
            visible: instance.visible,
        })
        .collect()
}

fn next_study_legend_row_index(pane_counts: &mut Vec<(PaneId, usize)>, pane: PaneId) -> usize {
    if let Some((_, count)) = pane_counts.iter_mut().find(|(id, _)| *id == pane) {
        let index = *count;
        *count = count.saturating_add(1);
        index
    } else {
        pane_counts.push((pane, 1));
        0
    }
}

fn study_legend_row_position(pane_rect: egui::Rect, row_index: usize) -> egui::Pos2 {
    egui::pos2(
        pane_rect.left() + 8.0,
        pane_rect.top() + 4.0 + row_index as f32 * (STUDY_LEGEND_ROW_HEIGHT + STUDY_LEGEND_ROW_GAP),
    )
}

fn show_study_legend_entry(
    ctx: &egui::Context,
    clip_rect: egui::Rect,
    entry: &StudyLegendEntry,
    position: egui::Pos2,
) -> Option<StudyLegendAction> {
    let mut action = None;
    egui::Area::new(egui::Id::new(("study_legend_entry", entry.id.0)))
        .order(chart_local_overlay_order())
        .fixed_pos(position)
        .show(ctx, |ui| {
            let row_width = study_legend_row_width(&entry.label);
            let (row_rect, row_response) = ui.allocate_exact_size(
                egui::vec2(row_width, STUDY_LEGEND_ROW_HEIGHT),
                egui::Sense::hover(),
            );
            let control_rects = study_legend_control_rects(row_rect);
            let settings_response = ui.interact(
                control_rects[0],
                ui.id().with(("settings", entry.id.0)),
                egui::Sense::click(),
            );
            let toggle_response = ui.interact(
                control_rects[1],
                ui.id().with(("toggle", entry.id.0)),
                egui::Sense::click(),
            );
            let delete_response = ui.interact(
                control_rects[2],
                ui.id().with(("delete", entry.id.0)),
                egui::Sense::click(),
            );
            let more_response = ui.interact(
                control_rects[3],
                ui.id().with(("more", entry.id.0)),
                egui::Sense::click(),
            );

            let settings_hovered = settings_response.hovered();
            let toggle_hovered = toggle_response.hovered();
            let delete_hovered = delete_response.hovered();
            let more_hovered = more_response.hovered();
            let show_controls = row_response.hovered()
                || settings_hovered
                || toggle_hovered
                || delete_hovered
                || more_hovered;

            let painter = ui.painter().with_clip_rect(clip_rect);
            if show_controls {
                painter.rect_filled(
                    row_rect,
                    4.0,
                    egui::Color32::from_rgba_unmultiplied(28, 28, 30, 230),
                );
                painter.rect_stroke(
                    row_rect,
                    4.0,
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(72, 72, 76)),
                    egui::StrokeKind::Inside,
                );
            }

            if entry.pane == PaneId::Main || show_controls || !entry.visible {
                painter.text(
                    egui::pos2(row_rect.left() + 5.0, row_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    &entry.label,
                    egui::FontId::monospace(10.0),
                    if !entry.visible {
                        egui::Color32::from_rgb(120, 126, 134)
                    } else if show_controls {
                        egui::Color32::from_rgb(232, 236, 240)
                    } else {
                        egui::Color32::from_rgb(182, 190, 198)
                    },
                );
            }

            if show_controls {
                paint_study_legend_button(
                    &painter,
                    control_rects[0],
                    StudyLegendIcon::Settings,
                    settings_hovered,
                );
                paint_study_legend_button(
                    &painter,
                    control_rects[1],
                    StudyLegendIcon::Visibility,
                    toggle_hovered,
                );
                paint_study_legend_button(
                    &painter,
                    control_rects[2],
                    StudyLegendIcon::Delete,
                    delete_hovered,
                );
                paint_study_legend_button(
                    &painter,
                    control_rects[3],
                    StudyLegendIcon::More,
                    more_hovered,
                );

                let settings_clicked = settings_response.clicked();
                let toggle_clicked = toggle_response.clicked();
                let delete_clicked = delete_response.clicked();
                let more_clicked = more_response.clicked();

                settings_response.on_hover_text("Settings");
                toggle_response.on_hover_text("Hide/show");
                delete_response.on_hover_text("Delete");
                more_response.on_hover_text("More");

                if settings_clicked {
                    action = Some(StudyLegendAction::Settings(entry.id));
                } else if toggle_clicked {
                    action = Some(StudyLegendAction::ToggleVisibility(entry.id));
                } else if delete_clicked {
                    action = Some(StudyLegendAction::Delete(entry.id));
                } else if more_clicked {
                    action = Some(StudyLegendAction::More(entry.id));
                }
            }
        });

    action
}

fn study_legend_row_width(label: &str) -> f32 {
    let label_width = label.chars().count() as f32 * 7.0;
    let controls_width = STUDY_LEGEND_CONTROL_SIZE * 4.0 + STUDY_LEGEND_CONTROL_GAP * 5.0;
    (label_width + controls_width + 14.0).clamp(150.0, 380.0)
}

fn study_legend_control_rects(row_rect: egui::Rect) -> [egui::Rect; 4] {
    let y = row_rect.center().y - STUDY_LEGEND_CONTROL_SIZE / 2.0;
    let mut right = row_rect.right() - STUDY_LEGEND_CONTROL_GAP;
    let mut next = || {
        let rect = egui::Rect::from_min_size(
            egui::pos2(right - STUDY_LEGEND_CONTROL_SIZE, y),
            egui::vec2(STUDY_LEGEND_CONTROL_SIZE, STUDY_LEGEND_CONTROL_SIZE),
        );
        right = rect.left() - STUDY_LEGEND_CONTROL_GAP;
        rect
    };
    let more = next();
    let delete = next();
    let toggle = next();
    let settings = next();
    [settings, toggle, delete, more]
}

fn paint_study_legend_button(
    painter: &egui::Painter,
    rect: egui::Rect,
    icon: StudyLegendIcon,
    hovered: bool,
) {
    painter.rect_filled(
        rect,
        3.0,
        if hovered {
            egui::Color32::from_rgb(72, 72, 76)
        } else {
            egui::Color32::from_rgb(42, 42, 45)
        },
    );
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(105, 105, 110)),
        egui::StrokeKind::Inside,
    );
    paint_study_legend_icon(painter, rect, icon);
}

fn paint_study_legend_icon(painter: &egui::Painter, rect: egui::Rect, icon: StudyLegendIcon) {
    let color = egui::Color32::from_rgb(230, 232, 236);
    let stroke = egui::Stroke::new(1.25, color);
    let center = rect.center();
    match icon {
        StudyLegendIcon::Settings => {
            painter.circle_stroke(center, 3.1, stroke);
            painter.circle_filled(center, 1.1, color);
            for (dx, dy) in [
                (0.0, -6.0),
                (0.0, 6.0),
                (-6.0, 0.0),
                (6.0, 0.0),
                (-4.2, -4.2),
                (4.2, 4.2),
                (-4.2, 4.2),
                (4.2, -4.2),
            ] {
                let inner = egui::pos2(center.x + dx * 0.62, center.y + dy * 0.62);
                let outer = egui::pos2(center.x + dx, center.y + dy);
                painter.line_segment([inner, outer], stroke);
            }
        }
        StudyLegendIcon::Visibility => {
            let left = egui::pos2(rect.left() + 3.0, center.y);
            let top = egui::pos2(center.x, rect.top() + 4.0);
            let right = egui::pos2(rect.right() - 3.0, center.y);
            let bottom = egui::pos2(center.x, rect.bottom() - 4.0);
            painter.line_segment([left, top], stroke);
            painter.line_segment([top, right], stroke);
            painter.line_segment([right, bottom], stroke);
            painter.line_segment([bottom, left], stroke);
            painter.circle_filled(center, 1.8, color);
        }
        StudyLegendIcon::Delete => {
            let body = egui::Rect::from_min_max(
                egui::pos2(center.x - 4.2, center.y - 2.2),
                egui::pos2(center.x + 4.2, center.y + 5.4),
            );
            painter.rect_stroke(body, 1.0, stroke, egui::StrokeKind::Inside);
            painter.line_segment(
                [
                    egui::pos2(center.x - 5.2, center.y - 4.4),
                    egui::pos2(center.x + 5.2, center.y - 4.4),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x - 2.0, center.y - 6.0),
                    egui::pos2(center.x + 2.0, center.y - 6.0),
                ],
                stroke,
            );
        }
        StudyLegendIcon::More => {
            for dx in [-4.0, 0.0, 4.0] {
                painter.circle_filled(egui::pos2(center.x + dx, center.y), 1.35, color);
            }
        }
    }
}

fn apply_study_legend_action(
    instances: &mut Vec<StudyInstance>,
    action: StudyLegendAction,
) -> bool {
    match action {
        StudyLegendAction::ToggleVisibility(id) => {
            if let Some(instance) = instances.iter_mut().find(|instance| instance.id == id) {
                instance.visible = !instance.visible;
                true
            } else {
                false
            }
        }
        StudyLegendAction::Delete(id) => remove_study_instance(instances, id),
        StudyLegendAction::Settings(_) | StudyLegendAction::More(_) => false,
    }
}

fn remove_study_instance(instances: &mut Vec<StudyInstance>, id: StudyId) -> bool {
    let old_len = instances.len();
    instances.retain(|instance| instance.id != id);
    instances.len() != old_len
}

fn apply_study_settings_update(instances: &mut [StudyInstance], updated: StudyInstance) -> bool {
    let Some(instance) = instances
        .iter_mut()
        .find(|instance| instance.id == updated.id)
    else {
        return false;
    };
    if *instance == updated {
        return false;
    }
    *instance = updated;
    true
}

fn show_study_settings_editor(
    ui: &mut egui::Ui,
    instance: &mut StudyInstance,
    tab: &mut StudySettingsTab,
) -> (bool, bool) {
    let mut changed = false;
    let mut remove = false;

    ui.monospace(instance.display_name());
    ui.add_space(4.0);
    changed |= ui.checkbox(&mut instance.visible, "Visible").changed();
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui
            .selectable_label(matches!(tab, StudySettingsTab::Inputs), "Inputs")
            .clicked()
        {
            *tab = StudySettingsTab::Inputs;
        }
        if ui
            .selectable_label(matches!(tab, StudySettingsTab::Style), "Style")
            .clicked()
        {
            *tab = StudySettingsTab::Style;
        }
        if ui
            .selectable_label(matches!(tab, StudySettingsTab::Visibility), "Visibility")
            .clicked()
        {
            *tab = StudySettingsTab::Visibility;
        }
    });
    ui.separator();
    ui.add_space(6.0);
    match tab {
        StudySettingsTab::Inputs => {
            changed |= show_study_settings_fields(
                ui,
                &mut instance.config,
                &mut instance.inputs,
                instance.visible,
            );
        }
        StudySettingsTab::Style => {
            changed |= show_study_style_fields(ui, &mut instance.style, instance.visible);
        }
        StudySettingsTab::Visibility => {
            changed |= show_study_visibility_fields(ui, &mut instance.visibility, instance.visible);
        }
    }
    ui.add_space(8.0);
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("Delete").clicked() {
            remove = true;
        }
    });

    (changed, remove)
}

fn show_study_settings_fields(
    ui: &mut egui::Ui,
    config: &mut StudyConfig,
    inputs: &mut StudyInputConfig,
    enabled: bool,
) -> bool {
    let mut changed = false;
    match (config, inputs) {
        (
            StudyConfig::Sma { period },
            StudyInputConfig::Sma {
                source,
                offset,
                calculation,
            },
        )
        | (
            StudyConfig::Ema { period },
            StudyInputConfig::Ema {
                source,
                offset,
                calculation,
            },
        ) => {
            changed |= show_study_settings_period(ui, "Length", period, enabled);
            changed |= show_study_source(ui, "Source", source, enabled);
            changed |= show_study_offset(ui, "Offset", offset, enabled);
            changed |= show_study_calculation_fields(ui, calculation, enabled);
        }
        (
            StudyConfig::Rsi {
                period,
                upper_band,
                middle_band,
                lower_band,
            },
            StudyInputConfig::Rsi {
                source,
                calculate_divergence,
                smoothing,
                calculation,
            },
        ) => {
            show_study_settings_section(ui, "RSI SETTINGS");
            changed |= show_study_settings_period(ui, "Length", period, enabled);
            changed |= show_study_source(ui, "Source", source, enabled);
            ui.add_enabled(
                false,
                egui::Checkbox::new(calculate_divergence, "Calculate divergence"),
            );
            changed |= show_study_settings_usize(ui, "Upper band", upper_band, 0..=100, enabled);
            changed |= show_study_settings_usize(ui, "Middle band", middle_band, 0..=100, enabled);
            changed |= show_study_settings_usize(ui, "Lower band", lower_band, 0..=100, enabled);
            show_study_settings_section(ui, "SMOOTHING");
            changed |= show_study_smoothing_fields(ui, smoothing, enabled);
            changed |= show_study_calculation_fields(ui, calculation, enabled);
        }
        (
            StudyConfig::Macd {
                fast_period,
                slow_period,
                signal_period,
            },
            StudyInputConfig::Macd {
                source,
                calculation,
            },
        ) => {
            changed |= show_study_settings_period(ui, "Fast length", fast_period, enabled);
            changed |= show_study_settings_period(ui, "Slow length", slow_period, enabled);
            changed |= show_study_source(ui, "Source", source, enabled);
            changed |= show_study_settings_period(ui, "Signal smoothing", signal_period, enabled);
            changed |= show_study_calculation_fields(ui, calculation, enabled);
        }
        _ => {}
    }
    changed
}

fn show_study_settings_section(ui: &mut egui::Ui, label: &str) {
    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(label)
            .size(11.0)
            .color(egui::Color32::from_rgb(152, 158, 166)),
    );
    ui.add_space(2.0);
}

fn show_study_settings_period(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut usize,
    enabled: bool,
) -> bool {
    show_study_settings_usize(
        ui,
        label,
        value,
        MIN_STUDY_PERIOD..=MAX_STUDY_PERIOD,
        enabled,
    )
}

fn show_study_settings_usize(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut usize,
    range: std::ops::RangeInclusive<usize>,
    enabled: bool,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_enabled_ui(enabled, |ui| {
            ui.set_width(270.0);
            ui.label(label);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                changed |= ui
                    .add(
                        egui::DragValue::new(value)
                            .range(range)
                            .speed(1.0)
                            .fixed_decimals(0),
                    )
                    .changed();
            });
        });
    });
    changed
}

fn show_study_source(
    ui: &mut egui::Ui,
    label: &str,
    source: &mut StudyPriceSource,
    enabled: bool,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_enabled_ui(enabled, |ui| {
            ui.set_width(270.0);
            ui.label(label);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::ComboBox::from_id_salt(("study_source", label))
                    .selected_text(source.label())
                    .show_ui(ui, |ui| {
                        for candidate in StudyPriceSource::ALL {
                            changed |= ui
                                .selectable_value(source, candidate, candidate.label())
                                .changed();
                        }
                    });
            });
        });
    });
    changed
}

fn show_study_offset(ui: &mut egui::Ui, label: &str, offset: &mut i32, enabled: bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_enabled_ui(enabled, |ui| {
            ui.set_width(270.0);
            ui.label(label);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                changed |= ui
                    .add(
                        egui::DragValue::new(offset)
                            .range(MIN_STUDY_OFFSET..=MAX_STUDY_OFFSET)
                            .speed(1.0)
                            .fixed_decimals(0),
                    )
                    .changed();
            });
        });
    });
    changed
}

fn show_study_smoothing_fields(
    ui: &mut egui::Ui,
    smoothing: &mut StudyRsiSmoothingConfig,
    enabled: bool,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_enabled_ui(enabled, |ui| {
            ui.set_width(270.0);
            ui.label("Type");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::ComboBox::from_id_salt("rsi_smoothing_type")
                    .selected_text(smoothing.kind.label())
                    .show_ui(ui, |ui| {
                        for candidate in StudySmoothingKind::ALL {
                            changed |= ui
                                .selectable_value(&mut smoothing.kind, candidate, candidate.label())
                                .changed();
                        }
                    });
            });
        });
    });
    changed |= show_study_settings_period(
        ui,
        "Length",
        &mut smoothing.length,
        enabled && smoothing.kind.is_enabled(),
    );
    changed
}

fn show_study_calculation_fields(
    ui: &mut egui::Ui,
    calculation: &mut StudyCalculationConfig,
    enabled: bool,
) -> bool {
    let mut changed = false;
    show_study_settings_section(ui, "CALCULATION");
    ui.horizontal(|ui| {
        ui.add_enabled_ui(enabled, |ui| {
            ui.set_width(270.0);
            ui.label("Timeframe");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::ComboBox::from_id_salt("study_calculation_timeframe")
                    .selected_text(calculation.timeframe.label())
                    .show_ui(ui, |ui| {
                        changed |= ui
                            .selectable_value(
                                &mut calculation.timeframe,
                                StudyCalculationTimeframe::Chart,
                                StudyCalculationTimeframe::Chart.label(),
                            )
                            .changed();
                    });
            });
        });
    });
    changed |= ui
        .add_enabled(
            enabled,
            egui::Checkbox::new(
                &mut calculation.wait_for_timeframe_close,
                "Wait for timeframe closes",
            ),
        )
        .changed();
    changed
}

fn show_study_visibility_fields(
    ui: &mut egui::Ui,
    visibility: &mut StudyVisibilityConfig,
    enabled: bool,
) -> bool {
    let mut changed = false;
    changed |= ui
        .add_enabled(enabled, egui::Checkbox::new(&mut visibility.ticks, "Ticks"))
        .changed();
    changed |= show_study_visibility_range(ui, "Seconds", &mut visibility.seconds, enabled);
    changed |= show_study_visibility_range(ui, "Minutes", &mut visibility.minutes, enabled);
    changed |= show_study_visibility_range(ui, "Hours", &mut visibility.hours, enabled);
    changed |= show_study_visibility_range(ui, "Days", &mut visibility.days, enabled);
    changed |= show_study_visibility_range(ui, "Weeks", &mut visibility.weeks, enabled);
    changed |= show_study_visibility_range(ui, "Months", &mut visibility.months, enabled);
    changed |= ui
        .add_enabled(
            enabled,
            egui::Checkbox::new(&mut visibility.ranges, "Ranges"),
        )
        .changed();
    changed
}

fn show_study_visibility_range(
    ui: &mut egui::Ui,
    label: &str,
    range: &mut StudyVisibilityRange,
    enabled: bool,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        changed |= ui
            .add_enabled(enabled, egui::Checkbox::new(&mut range.enabled, label))
            .changed();
        ui.add_enabled_ui(enabled && range.enabled, |ui| {
            changed |= ui
                .add(
                    egui::DragValue::new(&mut range.min)
                        .range(1..=999)
                        .speed(1.0)
                        .fixed_decimals(0),
                )
                .changed();
            ui.label("-");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut range.max)
                        .range(1..=999)
                        .speed(1.0)
                        .fixed_decimals(0),
                )
                .changed();
        });
    });
    changed
}

fn show_study_style_fields(ui: &mut egui::Ui, style: &mut StudyStyleConfig, enabled: bool) -> bool {
    let mut changed = false;
    egui::Grid::new("study_style_fields")
        .num_columns(4)
        .spacing([10.0, 8.0])
        .show(ui, |ui| match style {
            StudyStyleConfig::Sma { line } => {
                changed |= show_study_plot_style_row(ui, "SMA", line, enabled);
            }
            StudyStyleConfig::Ema { line } => {
                changed |= show_study_plot_style_row(ui, "EMA", line, enabled);
            }
            StudyStyleConfig::Rsi {
                line,
                smoothing,
                upper_band,
                middle_band,
                lower_band,
            } => {
                changed |= show_study_plot_style_row(ui, "RSI", line, enabled);
                changed |= show_study_plot_style_row(ui, "RSI-based MA", smoothing, enabled);
                changed |= show_study_plot_style_row(ui, "Upper band", upper_band, enabled);
                changed |= show_study_plot_style_row(ui, "Middle band", middle_band, enabled);
                changed |= show_study_plot_style_row(ui, "Lower band", lower_band, enabled);
            }
            StudyStyleConfig::Macd {
                macd,
                signal,
                histogram,
            } => {
                changed |= show_study_plot_style_row(ui, "MACD", macd, enabled);
                changed |= show_study_plot_style_row(ui, "Signal", signal, enabled);
                changed |= show_study_histogram_style_row(ui, "Histogram", histogram, enabled);
            }
        });
    changed
}

fn show_study_plot_style_row(
    ui: &mut egui::Ui,
    label: &str,
    style: &mut StudyPlotStyle,
    enabled: bool,
) -> bool {
    let mut changed = false;
    changed |= ui
        .add_enabled(enabled, egui::Checkbox::new(&mut style.visible, label))
        .changed();
    let controls_enabled = enabled && style.visible;
    changed |= show_study_color_button(ui, &mut style.color, controls_enabled);
    changed |= show_study_style_width(ui, &mut style.width, controls_enabled);
    ui.label("");
    ui.end_row();
    changed
}

fn show_study_histogram_style_row(
    ui: &mut egui::Ui,
    label: &str,
    style: &mut StudyHistogramStyle,
    enabled: bool,
) -> bool {
    let mut changed = false;
    changed |= ui
        .add_enabled(enabled, egui::Checkbox::new(&mut style.visible, label))
        .changed();
    let controls_enabled = enabled && style.visible;
    changed |= show_study_color_button(ui, &mut style.positive_color, controls_enabled);
    changed |= show_study_color_button(ui, &mut style.negative_color, controls_enabled);
    ui.label("");
    ui.end_row();
    changed
}

fn show_study_color_button(ui: &mut egui::Ui, color: &mut StudyColor, enabled: bool) -> bool {
    let mut color32 = study_color_to_egui(*color);
    let response = ui
        .add_enabled_ui(enabled, |ui| ui.color_edit_button_srgba(&mut color32))
        .inner;
    if response.changed() {
        *color = egui_color_to_study(color32);
        return true;
    }
    false
}

fn show_study_style_width(ui: &mut egui::Ui, width: &mut f32, enabled: bool) -> bool {
    ui.add_enabled(
        enabled,
        egui::DragValue::new(width)
            .range(MIN_STUDY_STYLE_WIDTH..=MAX_STUDY_STYLE_WIDTH)
            .speed(0.25)
            .fixed_decimals(1),
    )
    .changed()
}

fn study_color_to_egui(color: StudyColor) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

fn egui_color_to_study(color: egui::Color32) -> StudyColor {
    StudyColor::rgba(color.r(), color.g(), color.b(), color.a())
}

fn show_add_indicator_choices(ui: &mut egui::Ui) -> Vec<StudyConfig> {
    let mut configs = Vec::new();
    if show_add_indicator_choice(ui, "SMA", "Simple moving average") {
        configs.push(default_sma_config());
    }
    if show_add_indicator_choice(ui, "EMA", "Exponential moving average") {
        configs.push(default_ema_config());
    }
    if show_add_indicator_choice(ui, "RSI", "Relative strength index") {
        configs.push(default_rsi_config());
    }
    if show_add_indicator_choice(ui, "MACD", "Moving average convergence/divergence") {
        configs.push(default_macd_config());
    }
    configs
}

fn show_add_indicator_choice(ui: &mut egui::Ui, name: &str, description: &str) -> bool {
    let label = format!("{name}    {description}");
    ui.add_sized(
        [ui.available_width(), 30.0],
        egui::Button::new(egui::RichText::new(label).size(14.0)),
    )
    .clicked()
}

fn show_study_instance_row(
    ui: &mut egui::Ui,
    instance: &mut StudyInstance,
    selected: bool,
) -> StudyInstanceRowAction {
    let mut action = StudyInstanceRowAction::default();
    let row = ui.horizontal(|ui| {
        action.changed |= ui.checkbox(&mut instance.visible, "").changed();
        let label = ui.selectable_label(selected, instance.display_name());
        if label.clicked() {
            action.settings = true;
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("x").on_hover_text("Remove").clicked() {
                action.remove = true;
            }
            if ui.button("Settings").clicked() {
                action.settings = true;
            }
            ui.label(
                egui::RichText::new(study_instance_pane_badge(instance))
                    .size(11.0)
                    .color(egui::Color32::from_rgb(156, 164, 174)),
            );
        });
    });
    if selected {
        ui.painter().rect_stroke(
            row.response.rect.expand(2.0),
            4.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(86, 118, 180)),
            egui::StrokeKind::Inside,
        );
    }
    action
}

fn study_instance_pane_badge(instance: &StudyInstance) -> &'static str {
    match instance.pane() {
        PaneId::Main => "Overlay",
        PaneId::Volume => "Volume",
        PaneId::Study(_) => "Pane",
    }
}

fn default_study_instances() -> Vec<StudyInstance> {
    vec![
        StudyInstance::new(DEFAULT_SMA_STUDY_ID, default_sma_config()),
        StudyInstance::new(DEFAULT_RSI_STUDY_ID, default_rsi_config()),
    ]
}

fn next_study_instance_id(instances: &[StudyInstance]) -> u32 {
    instances
        .iter()
        .map(|instance| instance.id.0)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

fn default_sma_config() -> StudyConfig {
    StudyConfig::Sma {
        period: DEFAULT_SMA_PERIOD,
    }
}

fn default_ema_config() -> StudyConfig {
    StudyConfig::Ema {
        period: DEFAULT_EMA_PERIOD,
    }
}

fn default_rsi_config() -> StudyConfig {
    StudyConfig::Rsi {
        period: DEFAULT_RSI_PERIOD,
        upper_band: 70,
        middle_band: 50,
        lower_band: 30,
    }
}

fn default_macd_config() -> StudyConfig {
    StudyConfig::Macd {
        fast_period: DEFAULT_MACD_FAST_PERIOD,
        slow_period: DEFAULT_MACD_SLOW_PERIOD,
        signal_period: DEFAULT_MACD_SIGNAL_PERIOD,
    }
}

struct AppStudyDataSource<'a> {
    runtime: &'a StudyRuntime,
    instances: &'a [StudyInstance],
    active_interval: BarInterval,
}

impl StudyDataSource for AppStudyDataSource<'_> {
    fn output_values(&self, series_id: SeriesId) -> Option<&[StudyValue]> {
        self.runtime.output_values(series_id)
    }

    fn output_style(&self, series_id: SeriesId) -> Option<StudyOutputStyle> {
        let output = StudyOutputId::from_series_id(series_id)?;
        self.instances
            .iter()
            .find(|instance| instance.id == output.study)
            .and_then(|instance| {
                let mut style = instance.style.output_style(output.output)?;
                if !self.instance_renders_at_active_interval(instance) {
                    style.visible = false;
                }
                Some(style)
            })
    }

    fn output_offset(&self, series_id: SeriesId) -> i32 {
        let Some(output) = StudyOutputId::from_series_id(series_id) else {
            return 0;
        };
        self.instances
            .iter()
            .find(|instance| instance.id == output.study)
            .map(|instance| instance.inputs.output_offset(output.output))
            .unwrap_or(0)
    }

    fn pane_guides(&self, pane: PaneId) -> Vec<f64> {
        let Some(instance) = self.instances.iter().find(|instance| {
            self.instance_renders_at_active_interval(instance) && instance.pane() == pane
        }) else {
            return Vec::new();
        };

        match &instance.config {
            StudyConfig::Rsi {
                upper_band,
                middle_band,
                lower_band,
                ..
            } => vec![*lower_band as f64, *middle_band as f64, *upper_band as f64],
            StudyConfig::Macd { .. } => vec![0.0],
            StudyConfig::Sma { .. } | StudyConfig::Ema { .. } => Vec::new(),
        }
    }

    fn pane_guide_styles(&self, pane: PaneId) -> Vec<StudyGuideStyle> {
        let Some(instance) = self.instances.iter().find(|instance| {
            self.instance_renders_at_active_interval(instance) && instance.pane() == pane
        }) else {
            return Vec::new();
        };

        match (&instance.config, &instance.style) {
            (
                StudyConfig::Rsi {
                    upper_band,
                    middle_band,
                    lower_band,
                    ..
                },
                StudyStyleConfig::Rsi {
                    upper_band: upper_style,
                    middle_band: middle_style,
                    lower_band: lower_style,
                    ..
                },
            ) => vec![
                lower_style.to_guide_style(*lower_band as f64),
                middle_style.to_guide_style(*middle_band as f64),
                upper_style.to_guide_style(*upper_band as f64),
            ],
            _ => Vec::new(),
        }
    }

    fn pane_label(&self, pane: PaneId) -> Option<String> {
        let names = self
            .instances
            .iter()
            .filter(|instance| {
                self.instance_renders_at_active_interval(instance) && instance.pane() == pane
            })
            .map(StudyInstance::display_name)
            .collect::<Vec<_>>();

        (!names.is_empty()).then(|| names.join("  "))
    }
}

impl AppStudyDataSource<'_> {
    fn instance_renders_at_active_interval(&self, instance: &StudyInstance) -> bool {
        instance.visible && instance.visibility.allows_interval(self.active_interval)
    }
}

fn build_study_runtime(instances: &[StudyInstance]) -> StudyRuntime {
    let mut runtime = StudyRuntime::new();
    for instance in instances.iter().filter(|instance| instance.visible) {
        match (&instance.config, &instance.inputs) {
            (StudyConfig::Sma { period }, StudyInputConfig::Sma { source, .. }) => {
                runtime.add_study_with_id(
                    instance.id,
                    instance.display_name(),
                    SmaStudy::with_source(*period, *source),
                );
            }
            (StudyConfig::Ema { period }, StudyInputConfig::Ema { source, .. }) => {
                runtime.add_study_with_id(
                    instance.id,
                    instance.display_name(),
                    EmaStudy::with_source(*period, *source),
                );
            }
            (
                StudyConfig::Rsi { period, .. },
                StudyInputConfig::Rsi {
                    source, smoothing, ..
                },
            ) => {
                runtime.add_study_with_id(
                    instance.id,
                    instance.display_name(),
                    RsiStudy::with_source_and_smoothing(
                        *period,
                        *source,
                        smoothing.kind,
                        smoothing.length,
                    ),
                );
            }
            (
                StudyConfig::Macd {
                    fast_period,
                    slow_period,
                    signal_period,
                },
                StudyInputConfig::Macd { source, .. },
            ) => {
                runtime.add_study_with_id(
                    instance.id,
                    instance.display_name(),
                    MacdStudy::with_source(*fast_period, *slow_period, *signal_period, *source),
                );
            }
            _ => {}
        }
    }
    runtime
}

fn apply_indicator_layout(chart: &mut TvChartState, instances: &[StudyInstance]) {
    let active_study_panes = instances
        .iter()
        .filter_map(|instance| match instance.pane() {
            PaneId::Study(id) => Some(PaneId::Study(id)),
            PaneId::Main | PaneId::Volume => None,
        })
        .collect::<Vec<_>>();
    let existing_study_panes = chart
        .panes
        .panes()
        .iter()
        .filter_map(|pane| match pane.id {
            PaneId::Study(_) => Some(pane.id),
            PaneId::Main | PaneId::Volume => None,
        })
        .collect::<Vec<_>>();

    for pane in existing_study_panes {
        if !active_study_panes.contains(&pane) {
            chart.panes.set_visible(pane, false);
        }
    }

    for instance in instances {
        let PaneId::Study(id) = instance.pane() else {
            continue;
        };
        let (weight, min_height) = match &instance.config {
            StudyConfig::Rsi { .. } => (0.24, 80.0),
            StudyConfig::Macd { .. } => (0.28, 90.0),
            StudyConfig::Sma { .. } | StudyConfig::Ema { .. } => unreachable!(),
        };
        chart
            .panes
            .add_or_replace(PaneSpec::new(PaneId::Study(id), weight, min_height, true));
    }
}

fn reset_chart_for_loaded_market_data(chart: &mut TvChartState, instances: &[StudyInstance]) {
    let right_value_mode = chart.price_scales.scale(PriceScaleId::Right).value_mode();
    *chart = TvChartState::default();
    chart
        .price_scales
        .scale_mut(PriceScaleId::Right)
        .set_value_mode(right_value_mode);
    apply_indicator_layout(chart, instances);
}

fn normalize_persisted_study_instances(instances: &mut Vec<StudyInstance>) {
    normalize_study_instance_ids(instances);
    normalize_study_instances(instances);
}

fn normalize_study_instance_ids(instances: &mut [StudyInstance]) {
    let mut used = Vec::with_capacity(instances.len());
    let mut next_id = instances
        .iter()
        .map(|instance| instance.id.0)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    for instance in instances {
        if instance.id.0 == 0 || used.contains(&instance.id) {
            while used.contains(&StudyId::new(next_id)) {
                next_id = next_id.saturating_add(1);
            }
            instance.id = StudyId::new(next_id);
            next_id = next_id.saturating_add(1);
        }
        used.push(instance.id);
    }
}

fn normalize_study_instances(instances: &mut [StudyInstance]) {
    for instance in instances {
        if !instance.config.matches_inputs(&instance.inputs) {
            instance.inputs = StudyInputConfig::for_config(&instance.config);
        }
        if !instance.config.matches_style(&instance.style) {
            instance.style = StudyStyleConfig::for_config(&instance.config);
        }
        instance.inputs.normalize();
        instance.style.normalize();
        instance.visibility.normalize();
        match &mut instance.config {
            StudyConfig::Sma { period } | StudyConfig::Ema { period } => {
                *period = clamp_study_period(*period);
            }
            StudyConfig::Rsi {
                period,
                upper_band,
                middle_band,
                lower_band,
            } => {
                *period = clamp_study_period(*period);
                *upper_band = (*upper_band).min(100);
                *middle_band = (*middle_band).min(100);
                *lower_band = (*lower_band).min(100);
                if *lower_band > *upper_band {
                    std::mem::swap(lower_band, upper_band);
                }
                *middle_band = (*middle_band).clamp(*lower_band, *upper_band);
            }
            StudyConfig::Macd {
                fast_period,
                slow_period,
                signal_period,
            } => {
                *fast_period = clamp_study_period(*fast_period);
                *slow_period = clamp_study_period(*slow_period).max(*fast_period);
                *signal_period = clamp_study_period(*signal_period);
            }
        }
    }
}

fn clamp_study_period(period: usize) -> usize {
    period.clamp(MIN_STUDY_PERIOD, MAX_STUDY_PERIOD)
}

fn viewport_host_class(viewport_class: egui::ViewportClass) -> WidgetHostClass {
    match viewport_class {
        egui::ViewportClass::EmbeddedWindow => WidgetHostClass::InShellWorkspaceSurface,
        egui::ViewportClass::Root
        | egui::ViewportClass::Deferred
        | egui::ViewportClass::Immediate => WidgetHostClass::NativeViewport,
    }
}

fn detached_widget_viewport_builder(
    widget: WidgetSnapshot,
    display_title: &str,
) -> egui::ViewportBuilder {
    let mut builder = egui::ViewportBuilder::default().with_title(display_title);
    if widget.host_class != Some(WidgetHostClass::NativeViewport) {
        builder = builder
            .with_inner_size(widget.host.default_size)
            .with_min_inner_size(widget.host.min_size)
            .with_position(widget.host.default_pos)
            .with_resizable(true)
            .with_minimize_button(true)
            .with_maximize_button(true)
            .with_close_button(true);
    }
    builder
}

const WATCHLIST_PLACEHOLDER_ROWS: [(&str, &str); 3] =
    [("NQH6", "--"), ("BTC-USD", "--"), ("SPY", "--")];
const ORDER_TICKET_PLACEHOLDER_ROWS: [(&str, &str); 3] =
    [("Route", "disabled"), ("Qty", "--"), ("Status", "reserved")];
const NEWS_PLACEHOLDER_ROWS: [(&str, &str); 3] =
    [("Feed", "offline"), ("Items", "--"), ("Status", "reserved")];

fn placeholder_widget_rows(kind: WidgetKind) -> &'static [(&'static str, &'static str)] {
    match kind {
        WidgetKind::LiveChart
        | WidgetKind::SyncedTimeframes
        | WidgetKind::OrderBook
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
        | WidgetKind::MarketPulse => &[],
        WidgetKind::Watchlist => &WATCHLIST_PLACEHOLDER_ROWS,
        WidgetKind::OrderTicket => &ORDER_TICKET_PLACEHOLDER_ROWS,
        WidgetKind::News => &NEWS_PLACEHOLDER_ROWS,
    }
}

fn chart_widget_density(size: egui::Vec2) -> ChartWidgetDensity {
    if size.x < 280.0 || size.y < 190.0 {
        ChartWidgetDensity::Tiny
    } else if size.x < 480.0 || size.y < 300.0 {
        ChartWidgetDensity::Mini
    } else if size.x < 720.0 || size.y < 420.0 {
        ChartWidgetDensity::Compact
    } else {
        ChartWidgetDensity::Full
    }
}

fn visible_widget_display_titles(widgets: &[WidgetSnapshot]) -> BTreeMap<WidgetId, String> {
    let mut ids_by_kind: BTreeMap<WidgetKind, Vec<WidgetId>> = BTreeMap::new();
    for widget in widgets {
        ids_by_kind
            .entry(widget.kind())
            .or_default()
            .push(widget.id());
    }

    let mut titles = BTreeMap::new();
    for (kind, mut ids) in ids_by_kind {
        ids.sort();
        if ids.len() == 1 {
            titles.insert(ids[0], kind.label().to_string());
        } else {
            for (index, id) in ids.into_iter().enumerate() {
                titles.insert(id, format!("{} {}", kind.label(), index + 1));
            }
        }
    }
    titles
}

impl eframe::App for ChartOnlyApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let frame_started_at = Instant::now();
        let has_input_event = ui
            .ctx()
            .input(|input| input.events.iter().any(input_event_is_latency_relevant));
        self.latency_probe
            .begin_frame(frame_started_at, has_input_event);
        self.handle_chart_snapshot_events(ui.ctx());
        self.poll_market_events(ui.ctx());
        if let Some(action) = self.workspace_shell.handle_keyboard(ui.ctx()) {
            self.apply_shell_command_action(action);
        }

        egui::Panel::top("workspace_shell_top")
            .exact_size(workspace_shell::SHELL_TOP_BAR_HEIGHT)
            .show_inside(ui, |ui| {
                let top_bar_response = self
                    .workspace_shell
                    .show_top_bar(ui, self.debug_latency_overlay_visible);
                if top_bar_response.toggle_debug_latency_overlay {
                    self.debug_latency_overlay_visible = !self.debug_latency_overlay_visible;
                }
                if let Some(action) = top_bar_response.action {
                    self.apply_shell_command_action(action);
                }
            });
        if let Some(action) = self.workspace_shell.show_command_menu(ui.ctx()) {
            self.apply_shell_command_action(action);
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::default().fill(egui::Color32::from_rgb(13, 13, 13)))
            .show_inside(ui, |ui| {
                self.workspace_shell.show_workspace_background(ui);
            });

        let widgets = self.workspace_shell.widget_snapshots();
        let widget_display_titles = visible_widget_display_titles(&widgets);
        let active_widget_id = self.workspace_shell.active_widget_id();
        for widget in widgets {
            let show_overlay_areas = active_widget_id == Some(widget.id());
            let display_title = widget_display_titles
                .get(&widget.id())
                .cloned()
                .unwrap_or_else(|| widget.title());
            let response = self.show_workspace_widget_host(
                ui.ctx(),
                widget,
                &display_title,
                show_overlay_areas,
            );
            let mut workspace_layout_changed = response.layout_rect.is_some_and(|rect| {
                self.workspace_shell
                    .set_widget_layout_rect(widget.id(), rect)
            });
            let status = if response.open {
                WidgetRenderStatus::open(widget.id(), response.host_class, response.focused)
            } else {
                WidgetRenderStatus::closed(widget.id(), response.host_class)
            };
            workspace_layout_changed |= self.workspace_shell.set_widget_render_status(status);
            if !response.open && widget.kind() == WidgetKind::LiveChart {
                self.remove_live_chart_instance(widget.id());
            }
            if workspace_layout_changed {
                self.save_workspace_layout();
            }
        }

        if !self.workspace_shell.has_workspace_widgets() {
            self.show_live_chart_widget_floating_surfaces(ui.ctx());
            self.show_latency_overlay(ui.ctx());
        }
        let frame_finished_at = Instant::now();
        self.latency_probe
            .finish_frame(frame_finished_at, self.bar_store.generation());
        if cfg!(debug_assertions)
            && let Some(snapshot) = self
                .latency_probe
                .take_log_snapshot(frame_finished_at, LATENCY_LOG_INTERVAL)
        {
            eprintln!("{}", format_latency_log_line(snapshot));
        }
    }
}

fn show_latency_metric_row(ui: &mut egui::Ui, label: &str, metric: LatencyMetricSummary) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(178, 184, 190)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format_latency_metric(metric))
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(232, 234, 238)),
            );
        });
    });
}

fn show_latency_value_row(ui: &mut egui::Ui, label: &str, value_ms: Option<f32>) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .monospace()
                .size(11.0)
                .color(egui::Color32::from_rgb(178, 184, 190)),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format_latency_value(value_ms))
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(232, 234, 238)),
            );
        });
    });
}

fn format_latency_metric(metric: LatencyMetricSummary) -> String {
    match (metric.last_ms, metric.p50_ms, metric.p95_ms, metric.p99_ms) {
        (Some(last), Some(p50), Some(p95), Some(p99)) => {
            format!(
                "{}  p50 {}  p95 {}  p99 {}",
                format_ms(last),
                format_ms(p50),
                format_ms(p95),
                format_ms(p99)
            )
        }
        _ => "waiting".to_string(),
    }
}

fn format_latency_value(value_ms: Option<f32>) -> String {
    value_ms.map_or_else(|| "none".to_string(), format_ms)
}

fn format_latency_log_line(snapshot: LatencySnapshot) -> String {
    format!(
        "[latency] frames={} active={} samples={} idle_wakes={} update={} repaint={} idle_wake={} input->frame={} data->frame={} input_age={} data_age={}",
        snapshot.frame_count,
        snapshot.active_sample_count,
        snapshot.sample_count,
        snapshot.idle_wake_count,
        format_latency_metric(snapshot.update),
        format_latency_metric(snapshot.repaint_gap),
        format_latency_metric(snapshot.idle_wake_gap),
        format_latency_metric(snapshot.input_to_frame),
        format_latency_metric(snapshot.data_to_frame),
        format_latency_value(snapshot.current_input_age_ms),
        format_latency_value(snapshot.current_data_age_ms)
    )
}

fn format_ms(value: f32) -> String {
    if value >= 1_000.0 {
        format!("{:.2}s", value / 1_000.0)
    } else {
        format!("{value:.1}ms")
    }
}

fn should_request_backfill(
    visible: LogicalRange,
    oldest_time: Option<i64>,
    loading: bool,
    no_more_history: bool,
    last_backfill_to: Option<i64>,
) -> bool {
    if loading || no_more_history || visible.from > BACKFILL_TRIGGER_BARS {
        return false;
    }
    oldest_time.is_some_and(|time| last_backfill_to != Some(time))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DerivedCacheCliOptions {
    manifest_dir: PathBuf,
    output_root: PathBuf,
    symbol: Option<String>,
    raw_path_contains: Option<String>,
    start: Option<String>,
    end_exclusive: Option<String>,
    max_events: Option<u64>,
    batch_size: Option<usize>,
    checkpoint_every_events: Option<u64>,
    progress_every_events: Option<u64>,
    heatmap_depth_limit: Option<usize>,
    estimate_only: bool,
    estimate_all: bool,
    estimate_sample_seconds: Option<u64>,
    sample_raw_path_contains: Option<String>,
    sample_output_root: Option<PathBuf>,
    allow_full_run: bool,
}

impl Default for DerivedCacheCliOptions {
    fn default() -> Self {
        Self {
            manifest_dir: PathBuf::from("E-mini/manifests"),
            output_root: PathBuf::from("E-mini/derived"),
            symbol: None,
            raw_path_contains: None,
            start: None,
            end_exclusive: None,
            max_events: None,
            batch_size: None,
            checkpoint_every_events: None,
            progress_every_events: None,
            heatmap_depth_limit: None,
            estimate_only: false,
            estimate_all: false,
            estimate_sample_seconds: None,
            sample_raw_path_contains: None,
            sample_output_root: None,
            allow_full_run: false,
        }
    }
}

impl DerivedCacheCliOptions {
    fn usage() -> &'static str {
        "Usage: rustchart --build-derived-cache --symbol NQ.FUT --raw-path-contains 20241129 --max-events 250000 [--manifest-dir E-mini/manifests] [--output-root E-mini/derived] [--batch-size 50000] [--checkpoint-every-events 50000] [--progress-every-events 50000] [--heatmap-depth 12]\n\
         Estimate only: rustchart --build-derived-cache --symbol NQ.FUT --raw-path-contains 20241129 --estimate-only [--estimate-sample-seconds 132]\n\
         Estimate all: rustchart --build-derived-cache --estimate-all --sample-raw-path-contains 20241129 --sample-output-root /private/tmp/rustchart-derived-compressed-1m --estimate-sample-seconds 132\n\
         Use --allow-full-run only when intentionally processing an entire DBN manifest without --max-events."
    }

    fn build_config(&self, entry: &MboCacheEntry) -> DerivedCacheBuildConfig {
        let mut config = DerivedCacheBuildConfig::for_entry(entry);
        if let Some(batch_size) = self.batch_size {
            config.batch_size = batch_size.max(1);
        }
        if let Some(max_events) = self.max_events {
            config.max_events = Some(max_events);
        }
        if let Some(checkpoint_every_events) = self.checkpoint_every_events {
            config.replay.checkpoint_every_events = checkpoint_every_events.max(1);
        }
        if let Some(heatmap_depth_limit) = self.heatmap_depth_limit {
            config.replay.heatmap_depth_limit = heatmap_depth_limit;
        }
        config
    }

    fn progress_every_events(&self, config: &DerivedCacheBuildConfig) -> u64 {
        self.progress_every_events
            .unwrap_or_else(|| {
                config
                    .replay
                    .checkpoint_every_events
                    .max(config.batch_size as u64)
            })
            .max(1)
    }
}

fn parse_derived_cache_cli_options(
    args: &[String],
) -> Result<Option<DerivedCacheCliOptions>, String> {
    if !args.iter().any(|arg| arg == "--build-derived-cache") {
        return Ok(None);
    }

    let mut options = DerivedCacheCliOptions::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--build-derived-cache" => {
                index += 1;
            }
            "--help" | "-h" => {
                return Err(DerivedCacheCliOptions::usage().to_string());
            }
            "--manifest-dir" => {
                options.manifest_dir = PathBuf::from(required_cli_value(args, &mut index)?);
            }
            "--output-root" => {
                options.output_root = PathBuf::from(required_cli_value(args, &mut index)?);
            }
            "--symbol" => {
                options.symbol = Some(required_cli_value(args, &mut index)?);
            }
            "--raw-path-contains" => {
                options.raw_path_contains = Some(required_cli_value(args, &mut index)?);
            }
            "--start" => {
                options.start = Some(required_cli_value(args, &mut index)?);
            }
            "--end" | "--end-exclusive" => {
                options.end_exclusive = Some(required_cli_value(args, &mut index)?);
            }
            "--max-events" => {
                options.max_events = Some(parse_cli_u64(&required_cli_value(args, &mut index)?)?);
            }
            "--batch-size" => {
                options.batch_size =
                    Some(parse_cli_u64(&required_cli_value(args, &mut index)?)? as usize);
            }
            "--checkpoint-every-events" => {
                options.checkpoint_every_events =
                    Some(parse_cli_u64(&required_cli_value(args, &mut index)?)?);
            }
            "--progress-every-events" => {
                options.progress_every_events =
                    Some(parse_cli_u64(&required_cli_value(args, &mut index)?)?);
            }
            "--heatmap-depth" => {
                options.heatmap_depth_limit =
                    Some(parse_cli_u64(&required_cli_value(args, &mut index)?)? as usize);
            }
            "--estimate-only" => {
                options.estimate_only = true;
                index += 1;
            }
            "--estimate-all" => {
                options.estimate_all = true;
                index += 1;
            }
            "--estimate-sample-seconds" => {
                options.estimate_sample_seconds =
                    Some(parse_cli_u64(&required_cli_value(args, &mut index)?)?);
            }
            "--sample-raw-path-contains" => {
                options.sample_raw_path_contains = Some(required_cli_value(args, &mut index)?);
            }
            "--sample-output-root" => {
                options.sample_output_root =
                    Some(PathBuf::from(required_cli_value(args, &mut index)?));
            }
            "--allow-full-run" => {
                options.allow_full_run = true;
                index += 1;
            }
            other => {
                return Err(format!(
                    "unknown derived-cache argument `{other}`\n{}",
                    DerivedCacheCliOptions::usage()
                ));
            }
        }
    }

    if options.max_events.is_none()
        && !options.allow_full_run
        && !options.estimate_only
        && !options.estimate_all
    {
        return Err(format!(
            "--build-derived-cache requires --max-events for dry runs, --estimate-only/--estimate-all for projection, or --allow-full-run for an intentional full manifest build.\n{}",
            DerivedCacheCliOptions::usage()
        ));
    }

    Ok(Some(options))
}

fn required_cli_value(args: &[String], index: &mut usize) -> Result<String, String> {
    let flag = args
        .get(*index)
        .cloned()
        .unwrap_or_else(|| "<missing flag>".to_string());
    *index += 1;
    let Some(value) = args.get(*index) else {
        return Err(format!("{flag} requires a value"));
    };
    *index += 1;
    Ok(value.clone())
}

fn parse_cli_u64(value: &str) -> Result<u64, String> {
    value
        .replace('_', "")
        .parse::<u64>()
        .map_err(|error| format!("could not parse `{value}` as a positive integer: {error}"))
}

fn run_derived_cache_cli(options: DerivedCacheCliOptions) -> Result<(), String> {
    let registry = MboCacheRegistry::from_manifest_dir(&options.manifest_dir)
        .map_err(|error| format!("could not load MBO manifests: {error:?}"))?;
    if options.estimate_all {
        return run_derived_cache_estimate_all_cli(&registry, &options);
    }
    let entry = select_derived_cache_cli_entry(&registry, &options)?;
    if options.estimate_only {
        return run_derived_cache_estimate_cli(&entry, &options);
    }
    if !entry.raw_path.exists() {
        return Err(format!(
            "selected raw DBN path does not exist: {}",
            entry.raw_path.display()
        ));
    }

    let config = options.build_config(&entry);
    if config.max_events.is_none() && options.allow_full_run {
        let estimate = load_derived_cache_cli_estimate(&entry, &options)?;
        println!("derived cache full-run guardrail");
        print_derived_cache_cli_estimate(&estimate);
        if !estimate.has_time_projection() {
            return Err(
                "full-run guardrail requires --estimate-sample-seconds from a bounded dry-run elapsed value before running without --max-events"
                    .to_string(),
            );
        }
        println!(
            "full-run guardrail: proceeding only because --allow-full-run was supplied and an existing bounded estimate was found"
        );
    }
    let source = HistoricalDbnMboSource::from_cache_entry(&entry)
        .map_err(|error| format!("could not open DBN source: {error}"))?;
    let progress_every_events = options.progress_every_events(&config);
    let progress_path = derived_cache_manifest_path_for_entry(&options.output_root, &entry)
        .with_file_name("derived-cache-progress.v1.json");
    println!("derived cache build started");
    println!("source: {}", entry.source_key());
    println!("raw: {}", entry.raw_path.display());
    println!("output: {}", options.output_root.display());
    println!("progress: {}", progress_path.display());
    println!(
        "progress cadence: every {} events plus every partition write",
        progress_every_events
    );
    println!(
        "resume: completed partitions are written atomically; rerun the same command to replay completed progress and continue"
    );
    let started = Instant::now();
    let mut last_progress_line_events = 0_u64;
    let report = build_derived_cache_resumable_with_progress(
        &entry,
        source,
        &options.output_root,
        config,
        |event| {
            if should_print_derived_cache_cli_progress(
                &event,
                progress_every_events,
                &mut last_progress_line_events,
            ) {
                print_derived_cache_cli_progress(&event, started.elapsed());
            }
        },
    )
    .map_err(|error| format!("derived cache build failed: {error:?}"))?;
    println!("derived cache finalizing partition artifacts");
    let finalization_started = Instant::now();
    let manifest =
        finalize_derived_cache_partitions_with_progress(&entry, &options.output_root, |event| {
            print_derived_cache_cli_finalization_progress(&event, finalization_started.elapsed())
        })
        .map_err(|error| format!("derived cache finalization failed: {error:?}"))?;
    let finalization_elapsed = finalization_started.elapsed();
    let storage_report = derived_cache_artifact_storage_report(&manifest, &options.output_root)
        .map_err(|error| format!("derived cache storage report failed: {error:?}"))?;
    let (partition_count, partition_bytes) =
        derived_cache_progress_partition_storage(&report.progress, &options.output_root)?;
    let elapsed = started.elapsed();

    println!(
        "derived cache finalization complete: {:.2}s",
        finalization_elapsed.as_secs_f64()
    );
    println!("derived cache build complete");
    println!("source: {}", entry.source_key());
    println!("raw: {}", entry.raw_path.display());
    println!(
        "events: processed_new={} replayed={} manifest_total={} source_exhausted={}",
        report.processed_new_events,
        report.replayed_events,
        manifest.summary.events_read,
        report.source_exhausted
    );
    println!(
        "partitions: completed={} wrote_this_run={}",
        report.progress.completed_partitions.len(),
        report.wrote_partitions
    );
    println!(
        "summary: book={} prints={} instruments={} checkpoints={} heatmap={} bubbles={} bbo={} footprint={}",
        manifest.summary.applied_book_events,
        manifest.summary.trade_prints,
        manifest.summary.instrument_count,
        manifest.summary.checkpoint_count,
        manifest.summary.heatmap_cells,
        manifest.summary.volume_bubbles,
        manifest.summary.bbo_transitions,
        manifest.summary.footprint_cells
    );
    println!(
        "storage: final_artifacts={} final_shards={} final_bytes={} progress_partitions={} progress_partition_bytes={} retained_total_bytes={}",
        storage_report.artifact_count(),
        storage_report.shard_count(),
        storage_report.total_bytes(),
        partition_count,
        partition_bytes,
        storage_report.total_bytes().saturating_add(partition_bytes)
    );
    println!(
        "manifest: {}",
        derived_cache_manifest_path_for_entry(&options.output_root, &entry).display()
    );
    print_derived_cache_cli_estimate(&DerivedCacheCliEstimate::from_sample(
        &entry,
        manifest.summary.events_read,
        storage_report.total_bytes(),
        partition_bytes,
        Some(elapsed.as_secs_f64()),
    )?);
    println!("elapsed: {:.2}s", elapsed.as_secs_f64());
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
struct DerivedCacheCliEstimate {
    source_key: String,
    sample_events: u64,
    target_events: Option<u64>,
    sample_final_bytes: u64,
    sample_partition_bytes: u64,
    sample_total_bytes: u64,
    projected_final_bytes: Option<u64>,
    projected_partition_bytes: Option<u64>,
    projected_total_bytes: Option<u64>,
    sample_seconds: Option<f64>,
    projected_seconds: Option<f64>,
}

impl DerivedCacheCliEstimate {
    fn from_sample(
        entry: &MboCacheEntry,
        sample_events: u64,
        sample_final_bytes: u64,
        sample_partition_bytes: u64,
        sample_seconds: Option<f64>,
    ) -> Result<Self, String> {
        if sample_events == 0 {
            return Err("cannot estimate full run from a zero-event sample".to_string());
        }
        let sample_total_bytes = sample_final_bytes.saturating_add(sample_partition_bytes);
        let scale = entry
            .estimated_record_count
            .map(|target_events| target_events as f64 / sample_events as f64);
        Ok(Self {
            source_key: entry.source_key(),
            sample_events,
            target_events: entry.estimated_record_count,
            sample_final_bytes,
            sample_partition_bytes,
            sample_total_bytes,
            projected_final_bytes: scale.map(|scale| project_scaled_u64(sample_final_bytes, scale)),
            projected_partition_bytes: scale
                .map(|scale| project_scaled_u64(sample_partition_bytes, scale)),
            projected_total_bytes: scale.map(|scale| project_scaled_u64(sample_total_bytes, scale)),
            sample_seconds,
            projected_seconds: sample_seconds
                .zip(scale)
                .map(|(seconds, scale)| seconds * scale),
        })
    }

    fn has_time_projection(&self) -> bool {
        self.projected_seconds.is_some()
    }
}

fn project_scaled_u64(value: u64, scale: f64) -> u64 {
    if !scale.is_finite() || scale <= 0.0 {
        return 0;
    }
    (value as f64 * scale).round().clamp(0.0, u64::MAX as f64) as u64
}

fn run_derived_cache_estimate_cli(
    entry: &MboCacheEntry,
    options: &DerivedCacheCliOptions,
) -> Result<(), String> {
    let estimate = load_derived_cache_cli_estimate(entry, options)?;
    println!("derived cache estimate only");
    print_derived_cache_cli_estimate(&estimate);
    Ok(())
}

fn run_derived_cache_estimate_all_cli(
    registry: &MboCacheRegistry,
    options: &DerivedCacheCliOptions,
) -> Result<(), String> {
    let sample_entry = select_derived_cache_cli_sample_entry(registry, options)?;
    let sample_root = options
        .sample_output_root
        .as_deref()
        .unwrap_or(&options.output_root);
    let sample_estimate = load_derived_cache_cli_estimate_from_root(
        &sample_entry,
        sample_root,
        options.estimate_sample_seconds,
    )?;
    let entries = filtered_derived_cache_cli_entries(registry, options);
    if entries.is_empty() {
        return Err(format!(
            "no MBO manifests matched the estimate-all filters. Available entries:\n{}",
            format_mbo_entries(registry.entries())
        ));
    }

    let estimates = entries
        .iter()
        .map(|entry| {
            DerivedCacheCliEstimate::from_sample(
                entry,
                sample_estimate.sample_events,
                sample_estimate.sample_final_bytes,
                sample_estimate.sample_partition_bytes,
                sample_estimate.sample_seconds,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;

    println!("derived cache estimate all");
    println!("estimate sample source: {}", sample_estimate.source_key);
    println!("estimate sample root: {}", sample_root.display());
    println!(
        "estimate sample rate: events={} final_per_event={:.6} partition_per_event={:.6} seconds_per_event={}",
        sample_estimate.sample_events,
        sample_estimate.sample_final_bytes as f64 / sample_estimate.sample_events as f64,
        sample_estimate.sample_partition_bytes as f64 / sample_estimate.sample_events as f64,
        sample_estimate
            .sample_seconds
            .map(|seconds| format!("{seconds:.9}"))
            .unwrap_or_else(|| "unavailable".to_string())
    );
    println!("estimate entries: {}", estimates.len());
    for estimate in &estimates {
        print_derived_cache_cli_estimate_row(estimate);
    }
    print_derived_cache_cli_estimate_total(&estimates);
    Ok(())
}

fn load_derived_cache_cli_estimate(
    entry: &MboCacheEntry,
    options: &DerivedCacheCliOptions,
) -> Result<DerivedCacheCliEstimate, String> {
    load_derived_cache_cli_estimate_from_root(
        entry,
        &options.output_root,
        options.estimate_sample_seconds,
    )
}

fn load_derived_cache_cli_estimate_from_root(
    entry: &MboCacheEntry,
    output_root: &Path,
    estimate_sample_seconds: Option<u64>,
) -> Result<DerivedCacheCliEstimate, String> {
    let manifest_path = derived_cache_manifest_path_for_entry(output_root, entry);
    if !manifest_path.exists() {
        return Err(format!(
            "no derived-cache manifest found at {}; run a bounded --max-events build before estimating or starting a full run",
            manifest_path.display()
        ));
    }
    let manifest = DerivedCacheManifest::read_from_path(&manifest_path)
        .map_err(|error| format!("could not read derived-cache manifest: {error:?}"))?;
    let storage_report = derived_cache_artifact_storage_report(&manifest, output_root)
        .map_err(|error| format!("derived cache storage report failed: {error:?}"))?;
    let progress_path = manifest_path.with_file_name("derived-cache-progress.v1.json");
    let partition_bytes = if progress_path.exists() {
        let progress = DerivedCacheProgressManifest::read_from_path(&progress_path)
            .map_err(|error| format!("could not read progress manifest: {error:?}"))?;
        derived_cache_progress_partition_storage(&progress, output_root)?.1
    } else {
        0
    };
    DerivedCacheCliEstimate::from_sample(
        entry,
        manifest.summary.events_read,
        storage_report.total_bytes(),
        partition_bytes,
        estimate_sample_seconds.map(|seconds| seconds as f64),
    )
}

fn print_derived_cache_cli_estimate(estimate: &DerivedCacheCliEstimate) {
    println!("estimate source: {}", estimate.source_key);
    println!(
        "estimate sample: events={} final_bytes={} progress_partition_bytes={} retained_total_bytes={}",
        estimate.sample_events,
        estimate.sample_final_bytes,
        estimate.sample_partition_bytes,
        estimate.sample_total_bytes
    );
    println!(
        "estimate sample human: final={} progress_partitions={} retained_total={}",
        format_bytes(estimate.sample_final_bytes),
        format_bytes(estimate.sample_partition_bytes),
        format_bytes(estimate.sample_total_bytes)
    );
    match (
        estimate.target_events,
        estimate.projected_final_bytes,
        estimate.projected_partition_bytes,
        estimate.projected_total_bytes,
    ) {
        (
            Some(target_events),
            Some(projected_final_bytes),
            Some(projected_partition_bytes),
            Some(projected_total_bytes),
        ) => {
            println!("estimate target_events: {target_events}");
            println!(
                "estimate projected storage: final_bytes={} progress_partition_bytes={} retained_total_bytes={}",
                projected_final_bytes, projected_partition_bytes, projected_total_bytes
            );
            println!(
                "estimate projected storage human: final={} progress_partitions={} retained_total={}",
                format_bytes(projected_final_bytes),
                format_bytes(projected_partition_bytes),
                format_bytes(projected_total_bytes)
            );
        }
        _ => println!(
            "estimate projected storage: unavailable; manifest has no target record estimate"
        ),
    }
    if let (Some(sample_seconds), Some(projected_seconds)) =
        (estimate.sample_seconds, estimate.projected_seconds)
    {
        println!(
            "estimate time: sample={} projected_full={}",
            format_duration_seconds(sample_seconds),
            format_duration_seconds(projected_seconds)
        );
    } else {
        println!(
            "estimate time: unavailable; pass --estimate-sample-seconds from a bounded dry-run elapsed value"
        );
    }
}

fn print_derived_cache_cli_estimate_row(estimate: &DerivedCacheCliEstimate) {
    let records = estimate
        .target_events
        .map(|records| records.to_string())
        .unwrap_or_else(|| "-".to_string());
    let storage = estimate
        .projected_total_bytes
        .map(format_bytes)
        .unwrap_or_else(|| "unavailable".to_string());
    let time = estimate
        .projected_seconds
        .map(format_duration_seconds)
        .unwrap_or_else(|| "unavailable".to_string());
    println!(
        "estimate entry: source={} records={} retained_total={} projected_time={}",
        estimate.source_key, records, storage, time
    );
}

fn print_derived_cache_cli_estimate_total(estimates: &[DerivedCacheCliEstimate]) {
    let entries = estimates.len();
    let target_events = estimates
        .iter()
        .filter_map(|estimate| estimate.target_events)
        .sum::<u64>();
    let projected_final_bytes = estimates
        .iter()
        .filter_map(|estimate| estimate.projected_final_bytes)
        .sum::<u64>();
    let projected_partition_bytes = estimates
        .iter()
        .filter_map(|estimate| estimate.projected_partition_bytes)
        .sum::<u64>();
    let projected_total_bytes = estimates
        .iter()
        .filter_map(|estimate| estimate.projected_total_bytes)
        .sum::<u64>();
    let projected_seconds = estimates
        .iter()
        .filter_map(|estimate| estimate.projected_seconds)
        .sum::<f64>();

    println!(
        "estimate total: entries={} records={} final_bytes={} progress_partition_bytes={} retained_total_bytes={} projected_time={}",
        entries,
        target_events,
        projected_final_bytes,
        projected_partition_bytes,
        projected_total_bytes,
        format_duration_seconds(projected_seconds)
    );
    println!(
        "estimate total human: final={} progress_partitions={} retained_total={}",
        format_bytes(projected_final_bytes),
        format_bytes(projected_partition_bytes),
        format_bytes(projected_total_bytes)
    );
}

fn print_derived_cache_cli_finalization_progress(
    event: &DerivedCacheFinalizationEvent,
    elapsed: Duration,
) {
    let phase = match event.kind {
        DerivedCacheFinalizationEventKind::Started => "start",
        DerivedCacheFinalizationEventKind::ArtifactStarted => "artifact-start",
        DerivedCacheFinalizationEventKind::ArtifactFinished => "artifact-finished",
        DerivedCacheFinalizationEventKind::Finished => "finished",
    };
    let artifact = event
        .artifact_kind
        .map(|kind| format!(" artifact={kind:?}"))
        .unwrap_or_default();
    let records = event
        .records
        .map(|records| format!(" records={records}"))
        .unwrap_or_default();
    let storage = match (event.artifact_bytes, event.shard_count, event.shard_bytes) {
        (Some(artifact_bytes), Some(shard_count), Some(shard_bytes)) => format!(
            " artifact_bytes={} shard_count={} shard_bytes={}",
            artifact_bytes, shard_count, shard_bytes
        ),
        _ => String::new(),
    };
    println!(
        "derived cache finalization: phase={} completed={}/{} partitions={} elapsed={:.2}s{}{}{}",
        phase,
        event.completed_artifacts,
        event.total_artifacts,
        event.completed_partitions,
        elapsed.as_secs_f64(),
        artifact,
        records,
        storage
    );
}

fn derived_cache_progress_partition_storage(
    progress: &DerivedCacheProgressManifest,
    output_root: &Path,
) -> Result<(usize, u64), String> {
    let mut bytes = 0_u64;
    let mut count = 0_usize;
    for partition in &progress.completed_partitions {
        let path = output_root.join(&partition.artifact_relative_path);
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("could not inspect partition {}: {error}", path.display()))?;
        bytes = bytes.saturating_add(metadata.len());
        count += 1;
    }
    Ok((count, bytes))
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * KIB;
    const GIB: f64 = 1024.0 * MIB;
    const TIB: f64 = 1024.0 * GIB;
    let bytes = bytes as f64;
    if bytes >= TIB {
        format!("{:.2} TiB", bytes / TIB)
    } else if bytes >= GIB {
        format!("{:.2} GiB", bytes / GIB)
    } else if bytes >= MIB {
        format!("{:.2} MiB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.2} KiB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

fn format_duration_seconds(seconds: f64) -> String {
    if seconds >= 86_400.0 {
        format!("{:.2}d", seconds / 86_400.0)
    } else if seconds >= 3_600.0 {
        format!("{:.2}h", seconds / 3_600.0)
    } else if seconds >= 60.0 {
        format!("{:.2}m", seconds / 60.0)
    } else {
        format!("{seconds:.2}s")
    }
}

fn should_print_derived_cache_cli_progress(
    event: &DerivedCacheProgressEvent,
    progress_every_events: u64,
    last_progress_line_events: &mut u64,
) -> bool {
    let should_print = match event.kind {
        DerivedCacheProgressEventKind::Started
        | DerivedCacheProgressEventKind::PartitionWritten
        | DerivedCacheProgressEventKind::SourceExhausted
        | DerivedCacheProgressEventKind::Finished => true,
        DerivedCacheProgressEventKind::ReplayedBatch
        | DerivedCacheProgressEventKind::ProcessedBatch => {
            event
                .processed_events
                .saturating_sub(*last_progress_line_events)
                >= progress_every_events.max(1)
        }
    };

    if should_print {
        *last_progress_line_events = event.processed_events;
    }

    should_print
}

fn print_derived_cache_cli_progress(event: &DerivedCacheProgressEvent, elapsed: Duration) {
    let phase = match event.kind {
        DerivedCacheProgressEventKind::Started => "start",
        DerivedCacheProgressEventKind::ReplayedBatch => "replay",
        DerivedCacheProgressEventKind::ProcessedBatch => "process",
        DerivedCacheProgressEventKind::PartitionWritten => "partition",
        DerivedCacheProgressEventKind::SourceExhausted => "source-exhausted",
        DerivedCacheProgressEventKind::Finished => "finished",
    };
    let target = event
        .target_events
        .map(|target| {
            let percent = if target > 0 {
                (event.processed_events as f64 / target as f64) * 100.0
            } else {
                0.0
            };
            format!("/{target} ({percent:.2}%)")
        })
        .unwrap_or_default();
    let partition = event
        .latest_partition_index
        .map(|index| {
            format!(
                " latest_partition={} partition_events={} partition_bytes={}",
                index,
                event.latest_partition_events.unwrap_or_default(),
                event.latest_partition_bytes.unwrap_or_default()
            )
        })
        .unwrap_or_default();

    println!(
        "derived cache progress: phase={} events={}{} new={} replayed={} partitions={} wrote_this_run={} source_exhausted={} elapsed={:.2}s{}",
        phase,
        event.processed_events,
        target,
        event.processed_new_events,
        event.replayed_events,
        event.completed_partitions,
        event.wrote_partitions,
        event.source_exhausted,
        elapsed.as_secs_f64(),
        partition
    );
}

fn select_derived_cache_cli_entry(
    registry: &MboCacheRegistry,
    options: &DerivedCacheCliOptions,
) -> Result<MboCacheEntry, String> {
    let matches = filtered_derived_cache_cli_entries(registry, options);

    match matches.as_slice() {
        [entry] => Ok(entry.clone()),
        [] => Err(format!(
            "no MBO manifest matched the requested filters. Available entries:\n{}",
            format_mbo_entries(registry.entries())
        )),
        _ => Err(format!(
            "multiple MBO manifests matched; add --raw-path-contains, --start, or --end to select one:\n{}",
            format_mbo_entries(&matches)
        )),
    }
}

fn filtered_derived_cache_cli_entries(
    registry: &MboCacheRegistry,
    options: &DerivedCacheCliOptions,
) -> Vec<MboCacheEntry> {
    registry
        .entries()
        .iter()
        .filter(|entry| {
            options
                .symbol
                .as_deref()
                .is_none_or(|symbol| entry.symbol == symbol)
        })
        .filter(|entry| {
            options
                .raw_path_contains
                .as_deref()
                .is_none_or(|needle| entry.raw_path.to_string_lossy().contains(needle))
        })
        .filter(|entry| {
            options
                .start
                .as_deref()
                .is_none_or(|start| entry.start.as_deref() == Some(start))
        })
        .filter(|entry| {
            options
                .end_exclusive
                .as_deref()
                .is_none_or(|end| entry.end_exclusive.as_deref() == Some(end))
        })
        .cloned()
        .collect()
}

fn select_derived_cache_cli_sample_entry(
    registry: &MboCacheRegistry,
    options: &DerivedCacheCliOptions,
) -> Result<MboCacheEntry, String> {
    let sample_root = options
        .sample_output_root
        .as_deref()
        .unwrap_or(&options.output_root);
    let matches = registry
        .entries()
        .iter()
        .filter(|entry| {
            options
                .symbol
                .as_deref()
                .is_none_or(|symbol| entry.symbol == symbol)
        })
        .filter(|entry| {
            options
                .sample_raw_path_contains
                .as_deref()
                .is_none_or(|needle| entry.raw_path.to_string_lossy().contains(needle))
        })
        .filter(|entry| derived_cache_manifest_path_for_entry(sample_root, entry).exists())
        .cloned()
        .collect::<Vec<_>>();

    match matches.as_slice() {
        [entry] => Ok(entry.clone()),
        [] => Err(format!(
            "no sample derived-cache manifest matched; provide --sample-raw-path-contains and --sample-output-root. Available entries:\n{}",
            format_mbo_entries(registry.entries())
        )),
        _ => Err(format!(
            "multiple sample derived-cache manifests matched; add --sample-raw-path-contains:\n{}",
            format_mbo_entries(&matches)
        )),
    }
}

fn format_mbo_entries(entries: &[MboCacheEntry]) -> String {
    entries
        .iter()
        .map(|entry| {
            format!(
                "- symbol={} start={} end={} records={} raw={}",
                entry.symbol,
                entry.start.as_deref().unwrap_or("-"),
                entry.end_exclusive.as_deref().unwrap_or("-"),
                entry
                    .estimated_record_count
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                entry.raw_path.display()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod app_launch_options_tests {
    use super::{AppLaunchOptions, WidgetKind, parse_app_launch_options};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn app_launch_options_parse_open_widget_and_reset_workspace() {
        let options = parse_app_launch_options(&args(&[
            "--reset-workspace",
            "--open-widget",
            "bookmap-order-flow",
        ]))
        .expect("parse launch options");

        assert_eq!(
            options,
            AppLaunchOptions {
                csv_path: None,
                open_widget: Some(WidgetKind::BookmapOrderFlow),
                reset_workspace: true,
            }
        );
    }

    #[test]
    fn app_launch_options_preserve_positional_csv_path() {
        let options =
            parse_app_launch_options(&args(&["fixtures/sample.csv"])).expect("parse csv path");

        assert_eq!(options.csv_path.as_deref(), Some("fixtures/sample.csv"));
        assert_eq!(options.open_widget, None);
        assert!(!options.reset_workspace);
    }

    #[test]
    fn app_launch_options_reject_unknown_widget_key() {
        let error = parse_app_launch_options(&args(&["--open-widget", "not-a-widget"]))
            .expect_err("unknown widget key should fail");

        assert!(error.contains("unknown widget stable key"));
    }
}

#[cfg(test)]
mod derived_cache_cli_tests {
    use super::{
        DerivedCacheCliEstimate, DerivedCacheCliOptions, DerivedCacheProgressEvent,
        DerivedCacheProgressEventKind, MboCacheEntry, MboCacheRegistry,
        parse_derived_cache_cli_options, select_derived_cache_cli_entry,
        should_print_derived_cache_cli_progress,
    };
    use std::path::{Path, PathBuf};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn entry(start: &str, raw_path: &str) -> MboCacheEntry {
        MboCacheEntry {
            dataset: "GLBX.MDP3".to_string(),
            schema: "mbo".to_string(),
            symbol: "NQ.FUT".to_string(),
            stype_in: "parent".to_string(),
            stype_out: "instrument_id".to_string(),
            start: Some(start.to_string()),
            end_exclusive: Some("2024-12-03T00:00:00Z".to_string()),
            raw_path: PathBuf::from(raw_path),
            estimated_record_count: Some(100),
            estimated_cost_usd: None,
            estimated_billable_size_bytes: None,
            bytes_on_disk: None,
        }
    }

    #[test]
    fn derived_cache_cli_parser_requires_explicit_full_run_or_event_limit() {
        let error = parse_derived_cache_cli_options(&args(&[
            "--build-derived-cache",
            "--symbol",
            "NQ.FUT",
        ]))
        .expect_err("missing max events should fail");

        assert!(error.contains("--max-events"));
    }

    #[test]
    fn derived_cache_cli_parser_accepts_bounded_build_options() {
        let options = parse_derived_cache_cli_options(&args(&[
            "--build-derived-cache",
            "--symbol",
            "NQ.FUT",
            "--raw-path-contains",
            "20241129",
            "--max-events",
            "250_000",
            "--batch-size",
            "50_000",
            "--checkpoint-every-events",
            "50_000",
            "--progress-every-events",
            "25_000",
            "--heatmap-depth",
            "12",
        ]))
        .expect("parse")
        .expect("command");

        assert_eq!(options.symbol.as_deref(), Some("NQ.FUT"));
        assert_eq!(options.raw_path_contains.as_deref(), Some("20241129"));
        assert_eq!(options.max_events, Some(250_000));
        assert_eq!(options.batch_size, Some(50_000));
        assert_eq!(options.checkpoint_every_events, Some(50_000));
        assert_eq!(options.progress_every_events, Some(25_000));
        assert_eq!(options.heatmap_depth_limit, Some(12));
    }

    #[test]
    fn derived_cache_cli_parser_accepts_estimate_only_without_event_limit() {
        let options = parse_derived_cache_cli_options(&args(&[
            "--build-derived-cache",
            "--symbol",
            "NQ.FUT",
            "--raw-path-contains",
            "20241129",
            "--estimate-only",
            "--estimate-sample-seconds",
            "132",
        ]))
        .expect("parse")
        .expect("command");

        assert!(options.estimate_only);
        assert_eq!(options.max_events, None);
        assert_eq!(options.estimate_sample_seconds, Some(132));
    }

    #[test]
    fn derived_cache_cli_parser_accepts_estimate_all_with_sample_selector() {
        let options = parse_derived_cache_cli_options(&args(&[
            "--build-derived-cache",
            "--estimate-all",
            "--symbol",
            "NQ.FUT",
            "--sample-raw-path-contains",
            "20241129",
            "--sample-output-root",
            "/private/tmp/sample-derived",
            "--estimate-sample-seconds",
            "132",
        ]))
        .expect("parse")
        .expect("command");

        assert!(options.estimate_all);
        assert_eq!(options.max_events, None);
        assert_eq!(options.symbol.as_deref(), Some("NQ.FUT"));
        assert_eq!(
            options.sample_raw_path_contains.as_deref(),
            Some("20241129")
        );
        assert_eq!(
            options.sample_output_root.as_deref(),
            Some(Path::new("/private/tmp/sample-derived"))
        );
        assert_eq!(options.estimate_sample_seconds, Some(132));
    }

    #[test]
    fn derived_cache_cli_estimate_projects_storage_and_time_from_sample() {
        let estimate = DerivedCacheCliEstimate::from_sample(
            &entry("2024-11-29T00:00:00Z", "first.dbn.zst"),
            10,
            20,
            30,
            Some(4.0),
        )
        .expect("estimate");

        assert_eq!(estimate.target_events, Some(100));
        assert_eq!(estimate.projected_final_bytes, Some(200));
        assert_eq!(estimate.projected_partition_bytes, Some(300));
        assert_eq!(estimate.projected_total_bytes, Some(500));
        assert_eq!(estimate.projected_seconds, Some(40.0));
        assert!(estimate.has_time_projection());
    }

    #[test]
    fn derived_cache_cli_progress_printing_throttles_batch_events() {
        let mut last = 0;
        let event = DerivedCacheProgressEvent {
            kind: DerivedCacheProgressEventKind::ProcessedBatch,
            processed_events: 24_999,
            target_events: Some(100_000),
            replayed_events: 0,
            processed_new_events: 24_999,
            completed_partitions: 0,
            wrote_partitions: 0,
            latest_partition_index: None,
            latest_partition_events: None,
            latest_partition_bytes: None,
            source_exhausted: false,
        };

        assert!(!should_print_derived_cache_cli_progress(
            &event, 25_000, &mut last
        ));

        let event = DerivedCacheProgressEvent {
            processed_events: 25_000,
            processed_new_events: 25_000,
            ..event
        };
        assert!(should_print_derived_cache_cli_progress(
            &event, 25_000, &mut last
        ));
        assert_eq!(last, 25_000);

        let event = DerivedCacheProgressEvent {
            kind: DerivedCacheProgressEventKind::PartitionWritten,
            latest_partition_index: Some(0),
            latest_partition_events: Some(25_000),
            latest_partition_bytes: Some(1024),
            completed_partitions: 1,
            wrote_partitions: 1,
            ..event
        };
        assert!(should_print_derived_cache_cli_progress(
            &event, 25_000, &mut last
        ));
    }

    #[test]
    fn derived_cache_cli_selection_rejects_ambiguous_symbol_matches() {
        let registry = MboCacheRegistry::new(vec![
            entry("2024-11-29T00:00:00Z", "first.dbn.zst"),
            entry("2024-12-03T00:00:00Z", "second.dbn.zst"),
        ]);
        let options = DerivedCacheCliOptions {
            symbol: Some("NQ.FUT".to_string()),
            max_events: Some(10),
            ..DerivedCacheCliOptions::default()
        };

        let error = select_derived_cache_cli_entry(&registry, &options)
            .expect_err("ambiguous entries should fail");

        assert!(error.contains("multiple MBO manifests matched"));
        assert!(error.contains("--raw-path-contains"));
    }
}

#[cfg(test)]
mod app_tests {
    use super::{
        APP_UNDO_LIMIT, AppStudyDataSource, AppUndoSnapshot, ChartContextTarget,
        ChartPresetCollection, ChartPresetSnapshot, ChartPriceModeChoice, ChartWidgetDensity,
        DEFAULT_EMA_PERIOD, DEFAULT_INTERVAL, DEFAULT_RSI_PERIOD, DrawingAction,
        DrawingActionEffect, DrawingClipboardShortcut, DrawingDragDelta, DrawingMagnetMode,
        DrawingTool, DrawingToolAccess, DrawingToolPolicy, DrawingToolTemplates,
        DrawingToolbarToolGroup, FibExtendMode, FibPresetCollection, LayoutDocument, LogicalRange,
        MAX_STUDY_OFFSET, MAX_STUDY_PERIOD, MAX_STUDY_STYLE_WIDTH, MeasureDragTarget, MeasurePhase,
        MeasurePoint, MeasureState, MeasureStats, POSITION_WIDTH_HANDLE_INDEX, PaneId,
        PositionAnchorRole, SearchSourceFilter, SearchTypeFilter, StudyConfig, StudyInputConfig,
        StudyInstance, StudyLegendAction, StudyPlotStyle, StudyStyleConfig, TimeframeInputUnit,
        add_fib_level, app_redo_shortcut, app_undo_shortcut, apply_chart_tail_from_service,
        apply_drawing_action_to_store, apply_indicator_layout, apply_study_legend_action,
        apply_study_settings_update, best_search_candidate, brush_anchor_at_position,
        build_study_runtime, chart_context_has_price_scale, chart_context_has_time_scale,
        chart_context_target_for_drawing_hit, chart_context_target_for_rects,
        chart_price_mode_choice, chart_price_mode_label, chart_price_mode_value,
        chart_widget_density, clamp_selected_drawing_toolbar_position, clamp_study_period,
        default_rsi_config, default_study_instances, drawing_anchor_screen_point,
        drawing_clipboard_event_flags, drawing_clipboard_shortcut, drawing_cut_event_flag,
        drawing_document_scope, drawing_layer_blocks_chart_input, drawing_magnet_modifier,
        drawing_magnet_modifier_should_apply, drawing_object_tree_can_group_with_selected,
        drawing_object_tree_can_select, drawing_object_tree_entries, drawing_object_tree_label,
        drawing_object_tree_pane_label, drawing_object_tree_text_preview,
        drawing_tool_can_activate, drawing_tool_stays_active_after_creation,
        drawing_toolbar_position_from_rail, drawing_toolbar_rect_from_rail, drawing_toolbar_tools,
        effective_drawing_magnet_mode, fib_anchor_logical_price, fib_extend_mode,
        floating_panels_chart_click_should_block_chart_input,
        floating_panels_chart_click_should_cancel, format_measure_duration, format_measure_stats,
        live_chart_header_control_rects, live_chart_host_controls_width, live_chart_session_entry,
        load_chart_presets_state, load_drawing_document_state, load_drawing_tool_templates_state,
        load_fib_presets_state, load_layout_state, load_live_chart_sessions_state,
        load_user_settings_state, load_workspace_layout_state, logical_price_anchor_at,
        magnet_snap_for_position, measure_state_after_adjust_drag, measure_state_after_click,
        measure_state_after_hover, measure_state_after_release, measure_state_blocks_chart_input,
        measure_state_cleared_by_chart_click, measure_stats, moved_anchor_from_screen_point,
        moved_drawing_anchor_by_drag_delta, nearest_bar_time_at_x, nearest_logical_center_at_x,
        next_drawing_z_order, next_fib_level_value, next_study_legend_row_index,
        normalize_drawing_tool_policy, normalize_persisted_study_instances,
        normalize_position_drawing_anchors, normalize_study_instances, parse_timeframe_input,
        plural_timeframe_label, position_default_logical_span, position_default_price_delta,
        position_role_price_is_valid, prepend_chart_history_from_service, push_app_snapshot_stack,
        remove_fib_level, remove_study_instance, replace_chart_bars_from_service,
        reset_chart_for_loaded_market_data, reset_workspace_layout_state,
        resized_position_drawing_anchors_for_transform, save_chart_presets_state,
        save_drawing_document_state, save_drawing_tool_templates_state, save_fib_presets_state,
        save_layout_state, save_live_chart_sessions_state, save_user_settings_state,
        save_workspace_layout_state, search_candidates, search_palette_chart_click_should_cancel,
        selected_drawing_blank_click_should_clear, selected_drawing_cut_shortcut_target,
        selected_drawing_delete_shortcut_target, selected_drawing_toolbar_rect_from_plot,
        set_drawing_style_color, set_fib_extend_mode, should_request_backfill,
        sticked_ohlc_anchor_from_snap, study_legend_entries, study_legend_row_position,
        time_for_position, time_to_logical_for_bars, timeframe_input_preview,
        toggle_drawing_magnet_quick, type_to_search_char, typed_search_prefix,
        typed_timeframe_prefix, visible_widget_display_titles,
    };
    use crate::{
        chart_core::{
            BarStore, ChartColor, ChartTheme, DrawingAnchor, DrawingEditError, DrawingId,
            DrawingKind, DrawingSpec, DrawingStore, FibRetracementSettings, OhlcField,
            PositionDrawingSettings, PriceRange, TailUpdate,
        },
        market_data::{
            MarketDataKey, MarketDataService, MarketSource, Resolution, SymbolType, TIMEFRAMES,
        },
        persistence::{
            self, ChartPresetDocument, DrawingDocument, DrawingToolTemplatesDocument,
            FibPresetDocument, LiveChartSessionDocument, LiveChartSessionDocumentEntry,
            StoragePaths, UserSettingsDocument, WorkspaceLayoutDocument, read_json_document,
        },
        studies::{
            StudyColor, StudyDataSource, StudyId, StudyOutputId, StudyPriceSource,
            StudySmoothingKind, StudyValue,
        },
        tv_chart::{
            DrawingHit, DrawingHitTarget, OhlcBar, PriceScaleId, PriceScaleValueMode, ScenePoint,
            SceneRect, SceneTransform, SeriesKind, TvChartState,
        },
        widget_api::{
            WidgetDescriptor, WidgetHostMode, WidgetId, WidgetKind, WidgetLifecycle, WidgetSnapshot,
        },
        workspace_shell::{
            WorkspaceLayout, WorkspaceLayoutRect, WorkspaceLayoutWidget, WorkspacePresetKind,
            WorkspaceShell,
        },
    };
    use eframe::egui;
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    static APP_TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(1);

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let counter = APP_TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "rustchart-app-{name}-{}-{counter}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create test dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn chart_snapshot_file_name_sanitizes_symbol_and_timeframe() {
        assert_eq!(
            super::chart_snapshot_file_name("NQH6 /../", "5m", 42),
            "rustchart-nqh6-5m-42.png"
        );
        assert_eq!(
            super::chart_snapshot_file_name("***", "", -5),
            "rustchart-chart-chart-0.png"
        );
    }

    #[test]
    fn live_chart_instance_drawing_cache_key_keeps_symbol_scope_shared() {
        assert_eq!(
            super::live_chart_drawing_cache_key(MarketSource::YFinance, "NQH6"),
            "YFinance:YFinance:NQH6:default-chart"
        );
        assert_ne!(
            super::live_chart_drawing_cache_key(MarketSource::YFinance, "NQH6"),
            super::live_chart_drawing_cache_key(MarketSource::YFinance, "ESM6")
        );
    }

    #[test]
    fn live_chart_instance_market_events_carry_widget_identity() {
        let widget_id = super::WidgetId::new(7);
        let event = super::MarketEvent::Loaded {
            widget_id,
            request_id: 3,
            source: MarketSource::YFinance,
            symbol: "NQH6".to_string(),
            bars: Vec::new(),
        };

        match event {
            super::MarketEvent::Loaded {
                widget_id: actual,
                request_id,
                ..
            } => {
                assert_eq!(actual, widget_id);
                assert_eq!(request_id, 3);
            }
            _ => panic!("expected loaded event"),
        }
    }

    #[test]
    fn live_chart_instance_snapshot_request_carries_widget_identity() {
        let widget_id = super::WidgetId::new(12);
        let request = super::ChartSnapshotCaptureRequest {
            widget_id,
            viewport_id: egui::ViewportId::ROOT,
            action: super::ChartSnapshotAction::DownloadImage,
            capture_rect: egui::Rect::from_min_max(egui::pos2(1.0, 2.0), egui::pos2(3.0, 4.0)),
            pixels_per_point: 2.0,
            source_label: "NQH6".to_string(),
            timeframe_label: "5m".to_string(),
            requested_at: 42,
        };

        assert_eq!(request.widget_id, widget_id);
        assert_eq!(request.action, super::ChartSnapshotAction::DownloadImage);
    }

    #[test]
    fn chart_snapshot_image_region_clamps_to_viewport() {
        let image = egui::ColorImage::new(
            [4, 4],
            (0..16)
                .map(|value| egui::Color32::from_rgb(value, value, value))
                .collect(),
        );
        let region = egui::Rect::from_min_max(egui::pos2(-1.0, -1.0), egui::pos2(2.0, 2.0));
        let cropped = super::chart_snapshot_image_region(&image, region, 1.0)
            .expect("region intersects image");

        assert_eq!(cropped.size, [2, 2]);
        assert_eq!(cropped.pixels[0], egui::Color32::from_rgb(0, 0, 0));
        assert_eq!(cropped.pixels[3], egui::Color32::from_rgb(5, 5, 5));
    }

    #[test]
    fn chart_snapshot_rgba_bytes_are_unmultiplied() {
        let image = egui::ColorImage::new(
            [1, 1],
            vec![egui::Color32::from_rgba_unmultiplied(24, 48, 96, 128)],
        );

        assert_eq!(
            super::chart_snapshot_rgba_bytes(&image),
            vec![24, 48, 96, 128]
        );
    }

    fn open_widget_snapshot(id: u64, kind: WidgetKind) -> WidgetSnapshot {
        WidgetSnapshot {
            descriptor: WidgetDescriptor::new(WidgetId::new(id), kind),
            lifecycle: WidgetLifecycle::Open,
            host: kind.default_host_config(egui::pos2(64.0, 72.0)),
            host_class: None,
            z_order: id,
        }
    }

    #[test]
    fn single_visible_widget_display_title_hides_internal_id() {
        let widget = open_widget_snapshot(38, WidgetKind::LiveChart);
        let titles = visible_widget_display_titles(&[widget]);

        assert_eq!(
            titles.get(&WidgetId::new(38)).map(String::as_str),
            Some("Live Chart")
        );
        assert_eq!(widget.title(), "Live Chart 38");
    }

    #[test]
    fn duplicate_visible_widget_display_titles_use_per_kind_order() {
        let widgets = [
            open_widget_snapshot(38, WidgetKind::LiveChart),
            open_widget_snapshot(40, WidgetKind::Watchlist),
            open_widget_snapshot(41, WidgetKind::LiveChart),
            open_widget_snapshot(43, WidgetKind::Watchlist),
        ];
        let titles = visible_widget_display_titles(&widgets);

        assert_eq!(
            titles.get(&WidgetId::new(38)).map(String::as_str),
            Some("Live Chart 1")
        );
        assert_eq!(
            titles.get(&WidgetId::new(41)).map(String::as_str),
            Some("Live Chart 2")
        );
        assert_eq!(
            titles.get(&WidgetId::new(40)).map(String::as_str),
            Some("Watchlist 1")
        );
        assert_eq!(
            titles.get(&WidgetId::new(43)).map(String::as_str),
            Some("Watchlist 2")
        );
    }

    #[test]
    fn detached_viewport_builder_sets_size_only_before_native_viewport_exists() {
        let descriptor = crate::widget_api::WidgetDescriptor::new(
            crate::widget_api::WidgetId::new(7),
            crate::widget_api::WidgetKind::LiveChart,
        );
        let mut widget = crate::widget_api::WidgetSnapshot {
            descriptor,
            lifecycle: crate::widget_api::WidgetLifecycle::Open,
            host: crate::widget_api::WidgetKind::LiveChart
                .default_host_config(egui::pos2(64.0, 72.0)),
            host_class: None,
            z_order: 1,
        };
        widget.host.mode = crate::widget_api::WidgetHostMode::DetachedNativeViewport;

        let initial_builder = super::detached_widget_viewport_builder(widget, "Live Chart");
        assert_eq!(initial_builder.title, Some("Live Chart".to_string()));
        assert_eq!(initial_builder.inner_size, Some(widget.host.default_size));
        assert_eq!(initial_builder.position, Some(widget.host.default_pos));
        assert_eq!(initial_builder.maximize_button, Some(true));

        widget.host_class = Some(crate::widget_api::WidgetHostClass::NativeViewport);
        let steady_builder = super::detached_widget_viewport_builder(widget, "Live Chart");
        assert_eq!(steady_builder.inner_size, None);
        assert_eq!(steady_builder.position, None);
        assert_eq!(steady_builder.min_inner_size, None);
        assert_eq!(steady_builder.maximize_button, None);
        assert_eq!(steady_builder.resizable, None);
    }

    fn object_tree_text_spec(
        id: u64,
        pane: PaneId,
        symbol: &str,
        text: &str,
        z_order: i32,
    ) -> DrawingSpec {
        let mut spec = DrawingSpec::new(
            DrawingId::new(id),
            DrawingKind::Text,
            pane,
            vec![DrawingAnchor::TimePrice {
                time: 1_700_000_000 + id as i64,
                price: 100.0 + id as f64,
            }],
        )
        .with_symbol(symbol)
        .with_text(text);
        spec.z_order = z_order;
        spec
    }

    #[test]
    fn drawing_tools_map_to_kinds_and_creation_click_counts() {
        assert_eq!(
            DrawingTool::HorizontalLine.kind(),
            DrawingKind::HorizontalLine
        );
        assert_eq!(
            DrawingTool::HorizontalRay.kind(),
            DrawingKind::HorizontalRay
        );
        assert_eq!(DrawingTool::VerticalLine.kind(), DrawingKind::VerticalLine);
        assert_eq!(DrawingTool::TrendLine.kind(), DrawingKind::TrendLine);
        assert_eq!(DrawingTool::Ray.kind(), DrawingKind::Ray);
        assert_eq!(DrawingTool::Brush.kind(), DrawingKind::Brush);
        assert_eq!(
            DrawingTool::FibRetracement.kind(),
            DrawingKind::FibRetracement
        );
        assert_eq!(DrawingTool::LongPosition.kind(), DrawingKind::LongPosition);
        assert_eq!(
            DrawingTool::ShortPosition.kind(),
            DrawingKind::ShortPosition
        );
        assert_eq!(DrawingTool::Rectangle.kind(), DrawingKind::Rectangle);
        assert_eq!(DrawingTool::Text.kind(), DrawingKind::Text);
        assert_eq!(DrawingTool::PriceNote.kind(), DrawingKind::PriceNote);

        assert!(!DrawingTool::HorizontalLine.needs_two_clicks());
        assert!(!DrawingTool::HorizontalRay.needs_two_clicks());
        assert!(!DrawingTool::VerticalLine.needs_two_clicks());
        assert!(DrawingTool::TrendLine.needs_two_clicks());
        assert!(DrawingTool::Ray.needs_two_clicks());
        assert!(!DrawingTool::Brush.needs_two_clicks());
        assert!(DrawingTool::FibRetracement.needs_two_clicks());
        assert!(!DrawingTool::LongPosition.needs_two_clicks());
        assert!(!DrawingTool::ShortPosition.needs_two_clicks());
        assert!(DrawingTool::Rectangle.needs_two_clicks());
        assert!(!DrawingTool::Text.needs_two_clicks());
        assert!(!DrawingTool::PriceNote.needs_two_clicks());
    }

    #[test]
    fn position_tool_anchor_normalization_preserves_direction_rules() {
        let bars = Vec::<OhlcBar>::new();
        assert!(position_role_price_is_valid(
            DrawingTool::LongPosition,
            PositionAnchorRole::Target,
            100.0,
            110.0
        ));
        assert!(!position_role_price_is_valid(
            DrawingTool::ShortPosition,
            PositionAnchorRole::Target,
            100.0,
            110.0
        ));

        let normalized = normalize_position_drawing_anchors(
            DrawingKind::ShortPosition,
            vec![
                DrawingAnchor::LogicalPrice {
                    logical: 1.0,
                    price: 100.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 4.0,
                    price: 112.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 4.0,
                    price: 94.0,
                },
            ],
            &bars,
            None,
        )
        .expect("normalized short position anchors");

        assert_eq!(
            normalized,
            vec![
                DrawingAnchor::LogicalPrice {
                    logical: 1.0,
                    price: 100.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 4.0,
                    price: 88.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 4.0,
                    price: 106.0,
                },
            ]
        );
    }

    #[test]
    fn drawing_position_defaults_are_capped_for_zoomed_out_views() {
        let zoomed_out = SceneTransform::new(
            SceneRect::from_min_max(ScenePoint::new(0.0, 0.0), ScenePoint::new(800.0, 500.0)),
            LogicalRange {
                from: 0.0,
                to: 2_000.0,
            },
            PriceRange {
                min: 10_000.0,
                max: 30_000.0,
            },
        );
        let zoomed_in = SceneTransform::new(
            zoomed_out.plot,
            LogicalRange {
                from: 100.0,
                to: 102.0,
            },
            PriceRange {
                min: 18_000.0,
                max: 18_020.0,
            },
        );

        assert_eq!(position_default_logical_span(zoomed_out), 80.0);
        assert!(position_default_logical_span(zoomed_in) <= zoomed_in.time.span() * 0.45);
        assert_eq!(position_default_price_delta(18_000.0, zoomed_out), 81.0);
        assert_eq!(position_default_price_delta(18_000.0, zoomed_in), 2.4);
    }

    #[test]
    fn drawing_position_handles_resize_levels_and_width_independently() {
        fn assert_logical_price(anchor: DrawingAnchor, expected_logical: f64, expected_price: f64) {
            let DrawingAnchor::LogicalPrice { logical, price } = anchor else {
                panic!("expected logical price anchor");
            };
            assert!((logical - expected_logical).abs() < 1e-6);
            assert!((price - expected_price).abs() < 1e-6);
        }

        let bars = Vec::<OhlcBar>::new();
        let transform = SceneTransform::new(
            SceneRect::from_min_max(ScenePoint::new(0.0, 0.0), ScenePoint::new(200.0, 100.0)),
            LogicalRange {
                from: 0.0,
                to: 10.0,
            },
            PriceRange {
                min: 90.0,
                max: 110.0,
            },
        );
        let anchors = vec![
            DrawingAnchor::LogicalPrice {
                logical: 1.0,
                price: 100.0,
            },
            DrawingAnchor::LogicalPrice {
                logical: 6.0,
                price: 108.0,
            },
            DrawingAnchor::LogicalPrice {
                logical: 6.0,
                price: 96.0,
            },
        ];

        let target_resized = resized_position_drawing_anchors_for_transform(
            &anchors,
            DrawingKind::LongPosition,
            1,
            egui::pos2(transform.logical_to_x(2.0), transform.price_to_y(106.0)),
            transform,
            &bars,
            None,
        )
        .expect("target resize");
        assert_logical_price(target_resized[1], 6.0, 106.0);

        let stop_resized = resized_position_drawing_anchors_for_transform(
            &anchors,
            DrawingKind::LongPosition,
            2,
            egui::pos2(transform.logical_to_x(2.0), transform.price_to_y(94.0)),
            transform,
            &bars,
            None,
        )
        .expect("stop resize");
        assert_logical_price(stop_resized[2], 6.0, 94.0);

        let width_resized = resized_position_drawing_anchors_for_transform(
            &anchors,
            DrawingKind::LongPosition,
            POSITION_WIDTH_HANDLE_INDEX,
            egui::pos2(transform.logical_to_x(8.0), transform.price_to_y(92.0)),
            transform,
            &bars,
            None,
        )
        .expect("width resize");
        assert_logical_price(width_resized[1], 8.0, 108.0);
        assert_logical_price(width_resized[2], 8.0, 96.0);
    }

    #[test]
    fn brush_tool_stays_active_after_creating_a_stroke() {
        assert!(drawing_tool_stays_active_after_creation(DrawingTool::Brush));
        assert!(!drawing_tool_stays_active_after_creation(
            DrawingTool::TrendLine
        ));
        assert!(!drawing_tool_stays_active_after_creation(
            DrawingTool::Rectangle
        ));
        assert!(!drawing_tool_stays_active_after_creation(
            DrawingTool::HorizontalLine
        ));
    }

    #[test]
    fn brush_anchor_samples_interpolated_model_time_and_price() {
        let plot =
            SceneRect::from_min_max(ScenePoint::new(0.0, 0.0), ScenePoint::new(200.0, 100.0));
        let transform = SceneTransform::new(
            plot,
            LogicalRange { from: 0.0, to: 2.0 },
            PriceRange {
                min: 100.0,
                max: 102.0,
            },
        );
        let bars = vec![
            OhlcBar {
                time: 1_700_000_000,
                open: 100.0,
                high: 101.0,
                low: 99.0,
                close: 100.5,
                volume: 10.0,
            },
            OhlcBar {
                time: 1_700_000_060,
                open: 100.5,
                high: 102.0,
                low: 100.0,
                close: 101.5,
                volume: 12.0,
            },
        ];
        let position = egui::pos2(transform.logical_to_x(0.5), transform.price_to_y(101.0));

        let anchor =
            brush_anchor_at_position(transform, position, &bars, Some(60)).expect("brush anchor");

        assert_eq!(
            anchor,
            DrawingAnchor::TimePrice {
                time: 1_700_000_030,
                price: 101.0,
            }
        );
    }

    #[test]
    fn drawing_tool_policy_splits_favorites_and_main_toolbar_tools() {
        let mut policy = DrawingToolPolicy::default();

        assert_eq!(
            drawing_toolbar_tools(&policy, DrawingToolbarToolGroup::Favorites),
            vec![
                DrawingTool::HorizontalLine,
                DrawingTool::TrendLine,
                DrawingTool::Rectangle,
            ]
        );
        assert!(
            !drawing_toolbar_tools(&policy, DrawingToolbarToolGroup::Main)
                .contains(&DrawingTool::TrendLine)
        );
        assert!(
            drawing_toolbar_tools(&policy, DrawingToolbarToolGroup::Main)
                .contains(&DrawingTool::Brush)
        );

        policy.set_favorite(DrawingTool::Ray, true);
        assert_eq!(
            drawing_toolbar_tools(&policy, DrawingToolbarToolGroup::Favorites),
            vec![
                DrawingTool::HorizontalLine,
                DrawingTool::TrendLine,
                DrawingTool::Ray,
                DrawingTool::Rectangle,
            ]
        );

        policy.set_favorite(DrawingTool::TrendLine, false);
        assert!(
            !drawing_toolbar_tools(&policy, DrawingToolbarToolGroup::Favorites)
                .contains(&DrawingTool::TrendLine)
        );
        assert!(
            drawing_toolbar_tools(&policy, DrawingToolbarToolGroup::Main)
                .contains(&DrawingTool::TrendLine)
        );
    }

    #[test]
    fn persist_user_settings_load_defaults_when_file_is_missing() {
        let test_dir = TestDir::new("settings-missing");
        let paths = StoragePaths::from_root(test_dir.path());
        let mut default_policy = DrawingToolPolicy::default();
        default_policy.set_favorite(DrawingTool::Ray, true);

        let state = load_user_settings_state(&paths, default_policy.clone(), false);

        assert_eq!(state.drawing_tool_policy, default_policy);
        assert!(!state.volume_visible);
        assert_eq!(state.status, None);
    }

    #[test]
    fn persist_user_settings_loads_policy_and_volume_visibility() {
        let test_dir = TestDir::new("settings-load");
        let paths = StoragePaths::from_root(test_dir.path());
        let mut persisted_policy = DrawingToolPolicy::default();
        persisted_policy.set_favorite(DrawingTool::Ray, true);
        persisted_policy.set_access(DrawingTool::Text, DrawingToolAccess::Disabled);
        let document = UserSettingsDocument::from_drawing_tool_policy(&persisted_policy, false);
        save_user_settings_state(&paths, &persisted_policy, document.volume_visible)
            .expect("write settings");

        let state = load_user_settings_state(&paths, DrawingToolPolicy::default(), true);

        assert_eq!(state.drawing_tool_policy, persisted_policy);
        assert!(!state.volume_visible);
        assert_eq!(state.status, None);
    }

    #[test]
    fn persist_user_settings_load_reports_invalid_json_without_changing_defaults() {
        let test_dir = TestDir::new("settings-invalid-json");
        let paths = StoragePaths::from_root(test_dir.path());
        let path = paths.user_settings_path();
        fs::create_dir_all(path.parent().expect("settings parent")).expect("create settings dir");
        fs::write(&path, b"{not-json").expect("write invalid settings");
        let mut default_policy = DrawingToolPolicy::default();
        default_policy.set_access(DrawingTool::PriceNote, DrawingToolAccess::Hidden);

        let state = load_user_settings_state(&paths, default_policy.clone(), true);

        assert_eq!(state.drawing_tool_policy, default_policy);
        assert!(state.volume_visible);
        let status = state.status.expect("settings status");
        assert!(status.is_error);
        assert!(status.text.contains("User settings could not be loaded"));
    }

    #[test]
    fn persist_user_settings_save_writes_current_policy_document() {
        let test_dir = TestDir::new("settings-save");
        let paths = StoragePaths::from_root(test_dir.path());
        let mut policy = DrawingToolPolicy::default();
        policy.set_favorite(DrawingTool::TrendLine, false);
        policy.set_favorite(DrawingTool::HorizontalRay, true);
        policy.set_access(DrawingTool::Rectangle, DrawingToolAccess::Hidden);

        save_user_settings_state(&paths, &policy, false).expect("save settings");
        let saved = read_json_document::<UserSettingsDocument>(paths.user_settings_path())
            .expect("settings");

        assert_eq!(saved.drawing_tool_policy.to_policy(), policy);
        assert!(!saved.volume_visible);
    }

    #[test]
    fn persist_user_settings_normalizes_loaded_policy_order_and_duplicates() {
        let policy = DrawingToolPolicy {
            favorites: vec![
                DrawingTool::Rectangle,
                DrawingTool::TrendLine,
                DrawingTool::Rectangle,
                DrawingTool::HorizontalLine,
            ],
            access_overrides: vec![
                (DrawingTool::Ray, DrawingToolAccess::Disabled),
                (DrawingTool::Ray, DrawingToolAccess::Hidden),
                (DrawingTool::Text, DrawingToolAccess::Available),
            ],
        };

        assert_eq!(
            normalize_drawing_tool_policy(policy),
            DrawingToolPolicy {
                favorites: vec![
                    DrawingTool::HorizontalLine,
                    DrawingTool::TrendLine,
                    DrawingTool::Rectangle,
                ],
                access_overrides: vec![(DrawingTool::Ray, DrawingToolAccess::Disabled)],
            }
        );
    }

    #[test]
    fn persist_layout_load_defaults_when_file_is_missing() {
        let test_dir = TestDir::new("layout-missing");
        let paths = StoragePaths::from_root(test_dir.path());
        let mut defaults = default_study_instances();
        defaults.push(StudyInstance::new(
            StudyId::new(8),
            StudyConfig::Macd {
                fast_period: 12,
                slow_period: 26,
                signal_period: 9,
            },
        ));

        let state = load_layout_state(&paths, defaults.clone());

        assert_eq!(state.study_instances, defaults);
        assert_eq!(state.status, None);
    }

    #[test]
    fn persist_layout_saves_and_loads_study_instances() {
        let test_dir = TestDir::new("layout-save-load");
        let paths = StoragePaths::from_root(test_dir.path());
        let mut instances = default_study_instances();
        instances.push(StudyInstance::new(
            StudyId::new(9),
            StudyConfig::Ema { period: 50 },
        ));
        instances[0].visible = false;
        if let StudyInputConfig::Sma { source, offset, .. } = &mut instances[0].inputs {
            *source = StudyPriceSource::High;
            *offset = 3;
        }
        if let StudyStyleConfig::Sma { line } = &mut instances[0].style {
            line.color = StudyColor::rgb(12, 34, 56);
            line.width = 3.0;
        }
        instances[1].visibility.minutes.max = 15;

        save_layout_state(&paths, &instances).expect("save layout");
        let saved =
            read_json_document::<LayoutDocument>(paths.default_layout_path()).expect("layout");
        assert_eq!(
            saved.schema_version,
            persistence::LAYOUT_DOCUMENT_SCHEMA_VERSION
        );
        assert_eq!(saved.layout_id, persistence::DEFAULT_LAYOUT_ID);
        assert_eq!(saved.study_instances.len(), 3);

        let state = load_layout_state(&paths, Vec::new());

        assert_eq!(state.status, None);
        assert_eq!(state.study_instances, instances);
    }

    #[test]
    fn persist_layout_invalid_json_reports_error_and_defaults() {
        let test_dir = TestDir::new("layout-invalid-json");
        let paths = StoragePaths::from_root(test_dir.path());
        let path = paths.default_layout_path();
        fs::create_dir_all(path.parent().expect("layout parent")).expect("create layout dir");
        fs::write(&path, b"{not-json").expect("write invalid layout");
        let defaults = default_study_instances();

        let state = load_layout_state(&paths, defaults.clone());

        assert_eq!(state.study_instances, defaults);
        let status = state.status.expect("layout status");
        assert!(status.is_error);
        assert!(status.text.contains("Layout document could not be loaded"));
    }

    #[test]
    fn persist_workspace_layout_loads_empty_when_file_is_missing() {
        let test_dir = TestDir::new("workspace-layout-missing");
        let paths = StoragePaths::from_root(test_dir.path());

        let state = load_workspace_layout_state(&paths);

        assert_eq!(state.restore_input, None);
        assert_eq!(state.status, None);
    }

    #[test]
    fn persist_workspace_layout_saves_and_loads_restore_input() {
        let test_dir = TestDir::new("workspace-layout-save-load");
        let paths = StoragePaths::from_root(test_dir.path());
        let layout = WorkspaceLayout {
            next_widget_id: 4,
            next_z_order: 9,
            active_widget_id: Some(WidgetId::new(2)),
            widgets: vec![
                WorkspaceLayoutWidget {
                    id: WidgetId::new(1),
                    kind: WidgetKind::LiveChart,
                    lifecycle: WidgetLifecycle::Minimized,
                    host_mode: WidgetHostMode::InShellWorkspaceSurface,
                    rect: WorkspaceLayoutRect::new(64.0, 78.0, 760.0, 440.0),
                    z_order: 1,
                },
                WorkspaceLayoutWidget {
                    id: WidgetId::new(2),
                    kind: WidgetKind::SyncedTimeframes,
                    lifecycle: WidgetLifecycle::Open,
                    host_mode: WidgetHostMode::DetachedNativeViewport,
                    rect: WorkspaceLayoutRect::new(96.0, 112.0, 980.0, 500.0),
                    z_order: 8,
                },
            ],
        };

        save_workspace_layout_state(&paths, &layout).expect("save workspace layout");
        let saved = read_json_document::<WorkspaceLayoutDocument>(paths.workspace_layout_path())
            .expect("workspace layout");
        assert_eq!(
            saved.schema_version,
            persistence::WORKSPACE_LAYOUT_DOCUMENT_SCHEMA_VERSION
        );
        assert_eq!(saved.layout_id, persistence::DEFAULT_WORKSPACE_LAYOUT_ID);

        let state = load_workspace_layout_state(&paths);

        assert_eq!(state.status, None);
        assert_eq!(
            WorkspaceLayout::from(state.restore_input.expect("restore input")),
            layout
        );
    }

    #[test]
    fn persist_workspace_layout_invalid_json_reports_error_and_defaults() {
        let test_dir = TestDir::new("workspace-layout-invalid-json");
        let paths = StoragePaths::from_root(test_dir.path());
        let path = paths.workspace_layout_path();
        fs::create_dir_all(path.parent().expect("workspace layout parent"))
            .expect("create workspace layout dir");
        fs::write(&path, b"{not-json").expect("write invalid workspace layout");

        let state = load_workspace_layout_state(&paths);

        assert_eq!(state.restore_input, None);
        let status = state.status.expect("workspace layout status");
        assert!(status.is_error);
        assert!(status.text.contains("Workspace layout could not be loaded"));
    }

    #[test]
    fn persist_workspace_layout_reset_writes_empty_layout() {
        let test_dir = TestDir::new("workspace-layout-reset");
        let paths = StoragePaths::from_root(test_dir.path());
        let layout = WorkspaceLayout {
            next_widget_id: 2,
            next_z_order: 2,
            active_widget_id: Some(WidgetId::new(1)),
            widgets: vec![WorkspaceLayoutWidget {
                id: WidgetId::new(1),
                kind: WidgetKind::LiveChart,
                lifecycle: WidgetLifecycle::Open,
                host_mode: WidgetHostMode::InShellWorkspaceSurface,
                rect: WorkspaceLayoutRect::new(64.0, 78.0, 760.0, 440.0),
                z_order: 1,
            }],
        };
        save_workspace_layout_state(&paths, &layout).expect("save workspace layout");

        reset_workspace_layout_state(&paths).expect("reset workspace layout");
        let state = load_workspace_layout_state(&paths);
        let restored = state.restore_input.expect("reset layout");

        assert_eq!(state.status, None);
        assert_eq!(restored.widgets, Vec::new());
        assert_eq!(restored.next_widget_id, 1);
        assert_eq!(restored.next_z_order, 1);
        assert_eq!(restored.active_widget_id, None);
    }

    #[test]
    fn persist_live_chart_sessions_save_and_load_widget_local_state() {
        let test_dir = TestDir::new("live-chart-sessions-save-load");
        let paths = StoragePaths::from_root(test_dir.path());
        let sessions = vec![
            live_chart_session_entry(
                WidgetId::new(1),
                MarketSource::YFinance,
                "nqh6",
                TIMEFRAMES[2],
            ),
            live_chart_session_entry(
                WidgetId::new(2),
                MarketSource::Binance,
                "btcusdt",
                TIMEFRAMES[4],
            ),
        ];

        save_live_chart_sessions_state(&paths, sessions.clone()).expect("save live sessions");
        let saved =
            read_json_document::<LiveChartSessionDocument>(paths.live_chart_sessions_path())
                .expect("live sessions");
        assert_eq!(
            saved.schema_version,
            persistence::LIVE_CHART_SESSIONS_SCHEMA_VERSION
        );

        let state = load_live_chart_sessions_state(&paths);

        assert_eq!(state.status, None);
        assert_eq!(state.restores.len(), 2);
        assert_eq!(
            state.restores.get(&WidgetId::new(1)).map(|restore| (
                restore.source,
                restore.symbol.as_str(),
                restore.selected_timeframe.label()
            )),
            Some((MarketSource::YFinance, "NQH6", "5m"))
        );
        assert_eq!(
            state.restores.get(&WidgetId::new(2)).map(|restore| (
                restore.source,
                restore.symbol.as_str(),
                restore.selected_timeframe.label()
            )),
            Some((MarketSource::Binance, "BTCUSDT", "1h"))
        );
        assert_eq!(saved.sessions, sessions);
    }

    #[test]
    fn persist_live_chart_sessions_reports_unknown_restore_keys() {
        let test_dir = TestDir::new("live-chart-sessions-unknown-keys");
        let paths = StoragePaths::from_root(test_dir.path());
        save_live_chart_sessions_state(
            &paths,
            vec![LiveChartSessionDocumentEntry {
                widget_id: 1,
                source: "missing-source".to_string(),
                symbol: "NQH6".to_string(),
                timeframe: "5m".to_string(),
            }],
        )
        .expect("save live sessions");

        let state = load_live_chart_sessions_state(&paths);

        assert!(state.restores.is_empty());
        let status = state.status.expect("live session status");
        assert!(status.is_error);
        assert!(
            status
                .text
                .contains("Live Chart session could not be restored")
        );
    }

    #[test]
    fn workspace_v1_smoke_persists_relaunch_layout_states() {
        let test_dir = TestDir::new("workspace-v1-smoke");
        let paths = StoragePaths::from_root(test_dir.path());

        reset_workspace_layout_state(&paths).expect("reset workspace layout");
        let reset_state = load_workspace_layout_state(&paths);
        let mut restored_shell = WorkspaceShell::default();
        restored_shell
            .restore_workspace_layout(reset_state.restore_input.expect("reset restore input"));
        assert!(!restored_shell.has_workspace_widgets());

        let mut shell = WorkspaceShell::default();
        shell.apply_workspace_preset(WorkspacePresetKind::OneLiveChart);
        save_workspace_layout_state(&paths, &shell.workspace_layout_snapshot())
            .expect("save one chart layout");
        let one_chart = WorkspaceLayout::from(
            load_workspace_layout_state(&paths)
                .restore_input
                .expect("one chart restore input"),
        );
        assert_eq!(one_chart.widgets.len(), 1);
        assert_eq!(one_chart.widgets[0].kind, WidgetKind::LiveChart);
        assert_eq!(one_chart.widgets[0].lifecycle, WidgetLifecycle::Open);

        shell.apply_workspace_preset(WorkspacePresetKind::TwoLiveCharts);
        save_workspace_layout_state(&paths, &shell.workspace_layout_snapshot())
            .expect("save two chart layout");
        let two_charts = WorkspaceLayout::from(
            load_workspace_layout_state(&paths)
                .restore_input
                .expect("two chart restore input"),
        );
        assert_eq!(
            two_charts
                .widgets
                .iter()
                .filter(|widget| widget.kind == WidgetKind::LiveChart)
                .count(),
            2
        );
        assert_eq!(two_charts.active_widget_id, Some(WidgetId::new(2)));
        save_live_chart_sessions_state(
            &paths,
            vec![
                live_chart_session_entry(
                    WidgetId::new(1),
                    MarketSource::YFinance,
                    "NQH6",
                    TIMEFRAMES[0],
                ),
                live_chart_session_entry(
                    WidgetId::new(2),
                    MarketSource::Binance,
                    "BTCUSDT",
                    TIMEFRAMES[4],
                ),
            ],
        )
        .expect("save two chart sessions");
        let two_chart_sessions = load_live_chart_sessions_state(&paths);
        assert_eq!(
            two_chart_sessions
                .restores
                .get(&WidgetId::new(1))
                .map(|restore| (restore.symbol.as_str(), restore.selected_timeframe.label())),
            Some(("NQH6", "1m"))
        );
        assert_eq!(
            two_chart_sessions
                .restores
                .get(&WidgetId::new(2))
                .map(|restore| (restore.symbol.as_str(), restore.selected_timeframe.label())),
            Some(("BTCUSDT", "1h"))
        );

        shell.apply_workspace_preset(WorkspacePresetKind::LiveChartWithSyncedTimeframes);
        save_workspace_layout_state(&paths, &shell.workspace_layout_snapshot())
            .expect("save chart and timeframes layout");
        let chart_and_timeframes = WorkspaceLayout::from(
            load_workspace_layout_state(&paths)
                .restore_input
                .expect("chart and timeframes restore input"),
        );
        assert!(chart_and_timeframes.widgets.iter().any(|widget| {
            widget.kind == WidgetKind::LiveChart && widget.lifecycle == WidgetLifecycle::Open
        }));
        assert!(chart_and_timeframes.widgets.iter().any(|widget| {
            widget.kind == WidgetKind::SyncedTimeframes && widget.lifecycle == WidgetLifecycle::Open
        }));

        shell
            .minimize_widget(WidgetId::new(1))
            .expect("minimize live chart");
        save_workspace_layout_state(&paths, &shell.workspace_layout_snapshot())
            .expect("save minimized layout");
        let minimized = WorkspaceLayout::from(
            load_workspace_layout_state(&paths)
                .restore_input
                .expect("minimized restore input"),
        );
        assert!(minimized.widgets.iter().any(|widget| {
            widget.id == WidgetId::new(1) && widget.lifecycle == WidgetLifecycle::Minimized
        }));

        shell
            .return_widget_to_shell(WidgetId::new(1))
            .expect("restore shell host");
        shell
            .tear_out_widget(WidgetId::new(1))
            .expect("tear out live chart");
        save_workspace_layout_state(&paths, &shell.workspace_layout_snapshot())
            .expect("save detached layout");
        let detached = WorkspaceLayout::from(
            load_workspace_layout_state(&paths)
                .restore_input
                .expect("detached restore input"),
        );
        assert!(detached.widgets.iter().any(|widget| {
            widget.id == WidgetId::new(1)
                && widget.host_mode == WidgetHostMode::DetachedNativeViewport
                && widget.lifecycle == WidgetLifecycle::Open
        }));
    }

    #[test]
    fn persist_layout_normalizes_duplicate_ids_and_mismatched_shapes() {
        let mut instances = vec![
            StudyInstance::new(StudyId::new(4), StudyConfig::Ema { period: 0 }),
            StudyInstance {
                id: StudyId::new(4),
                visible: true,
                config: StudyConfig::Rsi {
                    period: 0,
                    upper_band: 20,
                    middle_band: 99,
                    lower_band: 80,
                },
                inputs: StudyInputConfig::Ema {
                    source: StudyPriceSource::Open,
                    offset: 999,
                    calculation: Default::default(),
                },
                style: StudyStyleConfig::Ema {
                    line: StudyPlotStyle::new(true, StudyColor::rgb(1, 2, 3), 99.0),
                },
                visibility: Default::default(),
            },
        ];

        normalize_persisted_study_instances(&mut instances);

        assert_eq!(instances[0].id, StudyId::new(4));
        assert_eq!(instances[1].id, StudyId::new(5));
        assert!(matches!(
            instances[1].inputs,
            StudyInputConfig::Rsi {
                source: StudyPriceSource::Close,
                ..
            }
        ));
        assert!(matches!(instances[1].style, StudyStyleConfig::Rsi { .. }));
        assert!(matches!(
            instances[1].config,
            StudyConfig::Rsi {
                period: 1,
                lower_band: 20,
                middle_band: 80,
                upper_band: 80,
            }
        ));
    }

    #[test]
    fn persist_drawing_tool_templates_save_and_load_styles() {
        let test_dir = TestDir::new("drawing-templates-save-load");
        let paths = StoragePaths::from_root(test_dir.path());
        let mut style = crate::chart_core::DrawingStyle::default();
        style.stroke_color = crate::chart_core::ChartColor::rgb(41, 98, 255);
        style.stroke_width = 4.0;
        style.fill_color = Some(crate::chart_core::ChartColor::rgba(41, 98, 255, 80));
        let mut fib_settings = crate::chart_core::FibRetracementSettings {
            reverse: true,
            background_visible: true,
            label_side: crate::chart_core::FibRetracementLabelSide::Right,
            ..Default::default()
        };
        fib_settings
            .levels
            .push(crate::chart_core::FibRetracementLevel::new(
                1.272,
                crate::chart_core::ChartColor::rgb(255, 152, 0),
            ));
        let fib_payload = crate::chart_core::DrawingPayload::FibRetracement(fib_settings.clone());
        let position_payload =
            crate::chart_core::DrawingPayload::Position(PositionDrawingSettings {
                labels_visible: true,
                prices_visible: false,
                profit_fill_color: ChartColor::rgba(46, 204, 113, 72),
                risk_fill_color: ChartColor::rgba(235, 87, 87, 72),
            });
        let mut templates = DrawingToolTemplates::default();
        templates.set_style(DrawingTool::Rectangle, style.clone());
        templates.set_payload(DrawingTool::FibRetracement, Some(fib_payload.clone()));
        templates.set_payload(DrawingTool::LongPosition, Some(position_payload.clone()));

        save_drawing_tool_templates_state(&paths, &templates).expect("save templates");
        let saved =
            read_json_document::<DrawingToolTemplatesDocument>(paths.drawing_tool_templates_path())
                .expect("templates document");
        assert_eq!(
            saved.schema_version,
            persistence::DRAWING_TOOL_TEMPLATES_SCHEMA_VERSION
        );
        assert_eq!(
            saved
                .tools
                .get(&DrawingTool::Rectangle)
                .map(|template| &template.style),
            Some(&style)
        );
        assert_eq!(
            saved
                .tools
                .get(&DrawingTool::FibRetracement)
                .and_then(|template| template.payload.as_ref()),
            Some(&fib_payload)
        );
        assert_eq!(
            saved
                .tools
                .get(&DrawingTool::LongPosition)
                .and_then(|template| template.payload.as_ref()),
            Some(&position_payload)
        );

        let loaded = load_drawing_tool_templates_state(&paths);

        assert_eq!(loaded.status, None);
        assert_eq!(loaded.templates.style(DrawingTool::Rectangle), style);
        assert_eq!(
            loaded.templates.payload(DrawingTool::FibRetracement),
            Some(fib_payload)
        );
        assert_eq!(
            loaded.templates.payload(DrawingTool::LongPosition),
            Some(position_payload)
        );
        assert_eq!(
            loaded.templates.style(DrawingTool::HorizontalLine),
            crate::chart_core::DrawingStyle::default()
        );
    }

    #[test]
    fn fib_preset_collection_supports_multiple_saved_settings() {
        let mut presets = FibPresetCollection::default();
        let mut first = FibRetracementSettings {
            reverse: true,
            ..Default::default()
        };
        first
            .levels
            .push(crate::chart_core::FibRetracementLevel::new(
                1.272,
                ChartColor::rgb(255, 152, 0),
            ));
        let mut second = FibRetracementSettings {
            background_visible: true,
            ..Default::default()
        };
        second
            .levels
            .push(crate::chart_core::FibRetracementLevel::new(
                1.618,
                ChartColor::rgb(255, 152, 0),
            ));

        let first_id = presets
            .save_new_preset("Pullback", first.clone(), 1_700_000_000)
            .expect("first preset");
        let second_id = presets
            .save_new_preset("Extensions", second.clone(), 1_700_000_001)
            .expect("second preset");

        assert_ne!(first_id, second_id);
        assert_eq!(presets.presets.len(), 2);
        assert_eq!(presets.active_settings(), Some(second.clone()));
        assert_eq!(
            presets
                .preset(&second_id)
                .map(|preset| preset.name.as_str()),
            Some("Extensions")
        );

        assert!(presets.set_active_preset(&first_id));
        assert_eq!(presets.active_settings(), Some(first.clone()));

        let updated = FibRetracementSettings {
            labels_visible: false,
            ..first
        };
        assert!(presets.update_preset(&first_id, updated.clone(), 1_700_000_100));
        assert_eq!(presets.active_settings(), Some(updated.clone()));

        assert!(presets.delete_preset(&first_id));
        assert_eq!(presets.active_settings(), None);
        assert_eq!(
            presets
                .preset(&second_id)
                .map(|preset| preset.settings.clone()),
            Some(second)
        );
    }

    #[test]
    fn persist_fib_presets_save_and_load_named_settings() {
        let test_dir = TestDir::new("fib-presets-save-load");
        let paths = StoragePaths::from_root(test_dir.path());
        let mut presets = FibPresetCollection::default();
        let mut settings = FibRetracementSettings {
            reverse: true,
            background_visible: true,
            ..Default::default()
        };
        settings
            .levels
            .push(crate::chart_core::FibRetracementLevel::new(
                1.618,
                ChartColor::rgb(255, 152, 0),
            ));
        let preset_id = presets
            .save_new_preset("Extension set", settings.clone(), 1_700_000_000)
            .expect("preset");

        save_fib_presets_state(&paths, &presets).expect("save fib presets");
        let saved =
            read_json_document::<FibPresetDocument>(paths.fib_presets_path()).expect("saved");
        assert_eq!(
            saved.schema_version,
            persistence::FIB_PRESETS_SCHEMA_VERSION
        );
        assert_eq!(saved.active_preset_id, Some(preset_id.clone()));
        assert_eq!(saved.presets.len(), 1);

        let loaded = load_fib_presets_state(&paths);

        assert_eq!(loaded.status, None);
        assert_eq!(loaded.presets.active_preset_id, Some(preset_id));
        assert_eq!(loaded.presets.active_settings(), Some(settings));
    }

    #[test]
    fn chart_preset_collection_supports_multiple_saved_themes() {
        let mut presets = ChartPresetCollection::default();
        let mut first_theme = ChartTheme::default();
        first_theme.candles.up_body = ChartColor::rgb(12, 34, 56);
        let mut second_theme = ChartTheme::default();
        second_theme.appearance.grid.visible = false;

        let first_id = presets.save_new_preset(
            "Candle colors",
            ChartPresetSnapshot::new(first_theme.clone(), true),
            1_700_000_000,
        );
        let second_id = presets.save_new_preset(
            "No grid",
            ChartPresetSnapshot::new(second_theme.clone(), false),
            1_700_000_001,
        );

        assert_ne!(first_id, second_id);
        assert_eq!(presets.presets.len(), 2);
        assert_eq!(
            presets.active_snapshot(),
            Some(ChartPresetSnapshot::new(second_theme.clone(), false))
        );

        assert!(presets.set_active_preset(&first_id));
        assert_eq!(
            presets.active_snapshot(),
            Some(ChartPresetSnapshot::new(first_theme.clone(), true))
        );

        let updated = ChartPresetSnapshot::new(second_theme.clone(), true);
        assert!(presets.update_preset(&first_id, updated.clone(), 1_700_000_100));
        assert_eq!(presets.active_snapshot(), Some(updated));

        assert!(presets.delete_preset(&first_id));
        assert_eq!(presets.active_snapshot(), None);
        assert_eq!(
            presets
                .preset(&second_id)
                .map(|preset| preset.snapshot.clone()),
            Some(ChartPresetSnapshot::new(second_theme, false))
        );
    }

    #[test]
    fn persist_chart_presets_save_and_load_named_settings() {
        let test_dir = TestDir::new("chart-presets-save-load");
        let paths = StoragePaths::from_root(test_dir.path());
        let mut presets = ChartPresetCollection::default();
        let mut theme = ChartTheme::default();
        theme.candles.down_body = ChartColor::rgb(180, 20, 40);
        theme.appearance.grid.visible = false;
        let snapshot = ChartPresetSnapshot::new(theme.clone(), false);
        let preset_id = presets.save_new_preset("Muted chart", snapshot.clone(), 1_700_000_000);

        save_chart_presets_state(&paths, &presets).expect("save chart presets");
        let saved =
            read_json_document::<ChartPresetDocument>(paths.chart_presets_path()).expect("saved");
        assert_eq!(
            saved.schema_version,
            persistence::CHART_PRESETS_SCHEMA_VERSION
        );
        assert_eq!(saved.active_preset_id, Some(preset_id.clone()));
        assert_eq!(saved.presets.len(), 1);

        let loaded = load_chart_presets_state(&paths);

        assert_eq!(loaded.status, None);
        assert_eq!(loaded.presets.active_preset_id, Some(preset_id));
        assert_eq!(loaded.presets.active_snapshot(), Some(snapshot));
    }

    #[test]
    fn persist_drawing_tool_templates_invalid_json_reports_error_and_defaults() {
        let test_dir = TestDir::new("drawing-templates-invalid-json");
        let paths = StoragePaths::from_root(test_dir.path());
        let path = paths.drawing_tool_templates_path();
        fs::create_dir_all(path.parent().expect("templates parent")).expect("create templates dir");
        fs::write(&path, b"{not-json").expect("write invalid templates");

        let loaded = load_drawing_tool_templates_state(&paths);

        assert_eq!(
            loaded.templates.style(DrawingTool::Rectangle),
            crate::chart_core::DrawingStyle::default()
        );
        let status = loaded.status.expect("templates status");
        assert!(status.is_error);
        assert!(
            status
                .text
                .contains("Drawing templates could not be loaded")
        );
    }

    #[test]
    fn persist_drawing_document_saves_and_loads_current_symbol_drawings() {
        let test_dir = TestDir::new("drawing-save-load");
        let paths = StoragePaths::from_root(test_dir.path());
        let scope = drawing_document_scope(MarketSource::YFinance, "AAPL");
        let mut drawings = DrawingStore::new();
        let first = drawings
            .insert(
                DrawingSpec::new(
                    DrawingId::new(7),
                    DrawingKind::Rectangle,
                    PaneId::Main,
                    vec![
                        DrawingAnchor::LogicalPrice {
                            logical: 1.0,
                            price: 100.0,
                        },
                        DrawingAnchor::LogicalPrice {
                            logical: 2.0,
                            price: 101.0,
                        },
                    ],
                )
                .with_symbol(scope.symbol_key.clone()),
            )
            .expect("first drawing");
        let second = drawings
            .insert(
                DrawingSpec::new(
                    DrawingId::new(8),
                    DrawingKind::HorizontalLine,
                    PaneId::Main,
                    vec![DrawingAnchor::PriceOnly { price: 102.0 }],
                )
                .with_symbol(scope.symbol_key.clone()),
            )
            .expect("second drawing");
        drawings
            .create_group(vec![first, second])
            .expect("same-symbol group");
        drawings
            .insert(
                DrawingSpec::new(
                    DrawingId::new(20),
                    DrawingKind::VerticalLine,
                    PaneId::Main,
                    vec![DrawingAnchor::TimeOnly {
                        time: 1_700_000_000,
                    }],
                )
                .with_symbol("YFinance:MSFT"),
            )
            .expect("other symbol drawing");

        save_drawing_document_state(&paths, scope.clone(), &drawings).expect("save drawings");
        let saved = read_json_document::<DrawingDocument>(
            paths
                .drawing_document_path(&scope)
                .expect("drawing document path"),
        )
        .expect("read saved drawings");
        assert_eq!(saved.scope, scope);
        assert_eq!(saved.drawings.len(), 2);
        assert_eq!(saved.groups.len(), 1);
        assert!(
            saved.drawings.iter().all(|drawing| {
                drawing.symbol.as_deref() == Some(saved.scope.symbol_key.as_str())
            })
        );

        let loaded = load_drawing_document_state(&paths, &saved.scope);
        assert_eq!(loaded.status, None);
        assert_eq!(loaded.drawings.drawings().len(), 2);
        assert_eq!(loaded.drawings.groups().len(), 1);
        assert_eq!(
            loaded.drawings.get(first).expect("loaded first").group_id,
            Some(saved.groups[0].id)
        );
    }

    #[test]
    fn persist_drawing_document_load_missing_file_as_empty_store() {
        let test_dir = TestDir::new("drawing-missing");
        let paths = StoragePaths::from_root(test_dir.path());
        let scope = drawing_document_scope(MarketSource::Binance, "BTCUSDT");

        let loaded = load_drawing_document_state(&paths, &scope);

        assert_eq!(loaded.status, None);
        assert!(loaded.drawings.drawings().is_empty());
        assert!(loaded.drawings.groups().is_empty());
    }

    #[test]
    fn persist_drawing_document_invalid_json_reports_error_and_empty_store() {
        let test_dir = TestDir::new("drawing-invalid-json");
        let paths = StoragePaths::from_root(test_dir.path());
        let scope = drawing_document_scope(MarketSource::YFinance, "AAPL");
        let path = paths
            .drawing_document_path(&scope)
            .expect("drawing document path");
        fs::create_dir_all(path.parent().expect("drawing parent")).expect("create drawing dir");
        fs::write(&path, b"{not-json").expect("write invalid drawing doc");

        let loaded = load_drawing_document_state(&paths, &scope);

        assert!(loaded.drawings.drawings().is_empty());
        let status = loaded.status.expect("drawing status");
        assert!(status.is_error);
        assert!(status.text.contains("Drawing document could not be loaded"));
    }

    #[test]
    fn drawing_tool_policy_gates_creation_without_hiding_existing_rows() {
        let symbol = "YFinance:AAPL";
        let mut policy = DrawingToolPolicy::default();
        policy.set_access(DrawingTool::Rectangle, DrawingToolAccess::Hidden);
        policy.set_access(DrawingTool::Ray, DrawingToolAccess::Disabled);

        assert!(!drawing_tool_can_activate(&policy, DrawingTool::Rectangle));
        assert!(!drawing_tool_can_activate(&policy, DrawingTool::Ray));
        assert!(drawing_tool_can_activate(&policy, DrawingTool::TrendLine));
        assert!(
            !drawing_toolbar_tools(&policy, DrawingToolbarToolGroup::Favorites)
                .contains(&DrawingTool::Rectangle)
        );
        assert!(
            drawing_toolbar_tools(&policy, DrawingToolbarToolGroup::Main)
                .contains(&DrawingTool::Ray)
        );

        let mut drawings = DrawingStore::new();
        let id = drawings
            .create_with_symbol(
                DrawingKind::Rectangle,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 1.0,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 4.0,
                        price: 104.0,
                    },
                ],
                Some(symbol.to_string()),
            )
            .expect("existing rectangle drawing");

        assert!(drawings.get(id).is_some());
        assert_eq!(drawing_object_tree_entries(&drawings, symbol).len(), 1);
    }

    #[test]
    fn object_tree_entries_include_hidden_and_sort_by_pane_z_order() {
        let symbol = "YFinance:AAPL";
        let mut drawings = DrawingStore::new();
        drawings
            .insert(object_tree_text_spec(20, PaneId::Main, symbol, "Front", 8))
            .expect("front drawing");
        let mut hidden = object_tree_text_spec(
            10,
            PaneId::Main,
            symbol,
            "Hidden drawing label that should be shortened",
            2,
        );
        hidden.visible = false;
        drawings.insert(hidden).expect("hidden drawing");
        drawings
            .insert(object_tree_text_spec(
                30,
                PaneId::Study(2),
                symbol,
                "Study",
                1,
            ))
            .expect("study drawing");
        drawings
            .insert(object_tree_text_spec(
                40,
                PaneId::Main,
                "YFinance:MSFT",
                "Other",
                0,
            ))
            .expect("other symbol drawing");

        let entries = drawing_object_tree_entries(&drawings, symbol);

        assert_eq!(
            entries.iter().map(|entry| entry.id).collect::<Vec<_>>(),
            vec![DrawingId::new(10), DrawingId::new(20), DrawingId::new(30)]
        );
        assert!(!entries[0].visible);
        assert_eq!(entries[0].label, "Text - Hidden drawing label that sh...");
        assert_eq!(entries[2].pane, PaneId::Study(2));
        assert_eq!(
            drawing_object_tree_pane_label(PaneId::Study(2)),
            "Study pane 2"
        );
    }

    #[test]
    fn object_tree_selection_requires_visible_selectable_current_symbol() {
        let symbol = "YFinance:AAPL";
        let drawing = object_tree_text_spec(1, PaneId::Main, symbol, "Selectable", 0);
        assert!(drawing_object_tree_can_select(&drawing, symbol));

        let mut hidden = drawing.clone();
        hidden.visible = false;
        assert!(!drawing_object_tree_can_select(&hidden, symbol));

        let mut unselectable = drawing.clone();
        unselectable.selectable = false;
        assert!(!drawing_object_tree_can_select(&unselectable, symbol));
        assert!(!drawing_object_tree_can_select(&drawing, "YFinance:MSFT"));
        assert_eq!(
            drawing_object_tree_text_preview("abcdefghijklmnopqrstuvwxyz", 8),
            "abcdefgh..."
        );
        assert_eq!(drawing_object_tree_label(&drawing), "Text - Selectable");
    }

    #[test]
    fn object_tree_group_eligibility_matches_store_boundaries() {
        let symbol = "YFinance:AAPL";
        let mut drawings = DrawingStore::new();
        drawings
            .insert(object_tree_text_spec(1, PaneId::Main, symbol, "One", 0))
            .expect("first drawing");
        drawings
            .insert(object_tree_text_spec(2, PaneId::Main, symbol, "Two", 1))
            .expect("second drawing");
        drawings
            .insert(object_tree_text_spec(
                3,
                PaneId::Study(1),
                symbol,
                "Study",
                2,
            ))
            .expect("study drawing");
        drawings
            .insert(object_tree_text_spec(
                4,
                PaneId::Main,
                "YFinance:MSFT",
                "Other",
                3,
            ))
            .expect("other symbol drawing");

        assert!(drawing_object_tree_can_group_with_selected(
            &drawings,
            Some(DrawingId::new(1)),
            DrawingId::new(2)
        ));
        assert!(!drawing_object_tree_can_group_with_selected(
            &drawings,
            Some(DrawingId::new(1)),
            DrawingId::new(1)
        ));
        assert!(!drawing_object_tree_can_group_with_selected(
            &drawings,
            None,
            DrawingId::new(2)
        ));
        assert!(!drawing_object_tree_can_group_with_selected(
            &drawings,
            Some(DrawingId::new(1)),
            DrawingId::new(3)
        ));
        assert!(!drawing_object_tree_can_group_with_selected(
            &drawings,
            Some(DrawingId::new(1)),
            DrawingId::new(4)
        ));

        drawings
            .create_group(vec![DrawingId::new(1), DrawingId::new(2)])
            .expect("group same pane and symbol drawings");
        assert!(!drawing_object_tree_can_group_with_selected(
            &drawings,
            Some(DrawingId::new(1)),
            DrawingId::new(2)
        ));
    }

    #[test]
    fn drawing_time_anchor_snaps_to_nearest_loaded_bar_without_mutating_data() {
        let bars = vec![
            OhlcBar {
                time: 100,
                open: 10.0,
                high: 11.0,
                low: 9.0,
                close: 10.5,
                volume: 1.0,
            },
            OhlcBar {
                time: 160,
                open: 11.0,
                high: 12.0,
                low: 10.0,
                close: 11.5,
                volume: 1.0,
            },
        ];
        let transform = SceneTransform::new(
            SceneRect::from_min_max(ScenePoint::new(0.0, 0.0), ScenePoint::new(200.0, 100.0)),
            LogicalRange { from: 0.0, to: 1.0 },
            PriceRange {
                min: 9.0,
                max: 12.0,
            },
        );

        assert_eq!(nearest_bar_time_at_x(transform, 0.0, &bars), Some(100));
        assert_eq!(nearest_bar_time_at_x(transform, 190.0, &bars), Some(160));
        assert_eq!(nearest_bar_time_at_x(transform, -200.0, &bars), Some(100));
        assert_eq!(nearest_bar_time_at_x(transform, 400.0, &bars), Some(160));
    }

    #[test]
    fn tool_time_coordinate_snaps_to_nearest_bar_center() {
        let bars = vec![
            OhlcBar {
                time: 100,
                open: 10.0,
                high: 11.0,
                low: 9.0,
                close: 10.5,
                volume: 1.0,
            },
            OhlcBar {
                time: 160,
                open: 11.0,
                high: 12.0,
                low: 10.0,
                close: 11.5,
                volume: 1.0,
            },
            OhlcBar {
                time: 220,
                open: 12.0,
                high: 13.0,
                low: 11.0,
                close: 12.5,
                volume: 1.0,
            },
        ];
        let transform = SceneTransform::new(
            SceneRect::from_min_max(ScenePoint::new(0.0, 0.0), ScenePoint::new(300.0, 100.0)),
            LogicalRange { from: 0.0, to: 2.0 },
            PriceRange {
                min: 9.0,
                max: 13.0,
            },
        );

        let left_of_center = transform.logical_to_x(0.35);
        let right_of_center = transform.logical_to_x(1.55);

        assert_eq!(
            nearest_logical_center_at_x(transform, left_of_center),
            Some(0.0)
        );
        assert_eq!(
            nearest_logical_center_at_x(transform, right_of_center),
            Some(2.0)
        );
        assert_eq!(
            time_for_position(transform, left_of_center, &bars, Some(60)),
            Some(100)
        );
        assert_eq!(
            time_for_position(transform, right_of_center, &bars, Some(60)),
            Some(220)
        );
    }

    #[test]
    fn logical_drawing_anchor_allows_empty_space_placement() {
        let transform = SceneTransform::new(
            SceneRect::from_min_max(ScenePoint::new(0.0, 0.0), ScenePoint::new(200.0, 100.0)),
            LogicalRange {
                from: 10.0,
                to: 30.0,
            },
            PriceRange {
                min: 90.0,
                max: 110.0,
            },
        );

        let anchor = logical_price_anchor_at(transform, egui::pos2(190.0, 25.0))
            .expect("logical anchor should be valid");

        match anchor {
            DrawingAnchor::LogicalPrice { logical, price } => {
                assert!(logical > 25.0);
                assert!(price > 100.0);
            }
            other => panic!("expected logical-price anchor, got {other:?}"),
        }
    }

    #[test]
    fn drawing_drag_anchor_conversion_moves_time_price_points() {
        let bars = vec![
            OhlcBar {
                time: 100,
                open: 10.0,
                high: 11.0,
                low: 9.0,
                close: 10.5,
                volume: 1.0,
            },
            OhlcBar {
                time: 160,
                open: 11.0,
                high: 12.0,
                low: 10.0,
                close: 11.5,
                volume: 1.0,
            },
        ];
        let pane = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(300.0, 100.0));
        let transform = SceneTransform::new(
            SceneRect::from_min_max(ScenePoint::new(0.0, 0.0), ScenePoint::new(300.0, 100.0)),
            LogicalRange { from: 0.0, to: 2.0 },
            PriceRange {
                min: 9.0,
                max: 12.0,
            },
        );
        let anchor = DrawingAnchor::TimePrice {
            time: 100,
            price: 10.0,
        };
        let target = egui::pos2(transform.logical_to_x(1.5), transform.price_to_y(10.6));
        let moved = moved_anchor_from_screen_point(
            anchor,
            PaneId::Main,
            target,
            pane,
            transform,
            &bars,
            DrawingMagnetMode::Off,
            Some(60),
        )
        .expect("moved anchor should map back to model space");

        match moved {
            DrawingAnchor::TimePrice { time, price } => {
                assert_eq!(time, 220);
                assert!(price > 10.0);
            }
            other => panic!("expected time-price anchor, got {other:?}"),
        }
    }

    #[test]
    fn time_price_drag_within_loaded_bars_preserves_time_anchor() {
        let bars = vec![
            OhlcBar {
                time: 100,
                open: 10.0,
                high: 11.0,
                low: 9.0,
                close: 10.5,
                volume: 1.0,
            },
            OhlcBar {
                time: 160,
                open: 11.0,
                high: 12.0,
                low: 10.0,
                close: 11.5,
                volume: 1.0,
            },
        ];
        let pane = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(300.0, 100.0));
        let transform = SceneTransform::new(
            SceneRect::from_min_max(ScenePoint::new(0.0, 0.0), ScenePoint::new(300.0, 100.0)),
            LogicalRange { from: 0.0, to: 2.0 },
            PriceRange {
                min: 9.0,
                max: 12.0,
            },
        );
        let anchor = DrawingAnchor::TimePrice {
            time: 100,
            price: 10.0,
        };
        let target = egui::pos2(transform.logical_to_x(1.0), transform.price_to_y(11.0));
        let moved = moved_anchor_from_screen_point(
            anchor,
            PaneId::Main,
            target,
            pane,
            transform,
            &bars,
            DrawingMagnetMode::Off,
            Some(60),
        )
        .expect("moved anchor should map back to model space");

        match moved {
            DrawingAnchor::TimePrice { time, price } => {
                assert_eq!(time, 160);
                assert!((price - 11.0).abs() < 1e-6);
            }
            other => panic!("expected time-price anchor, got {other:?}"),
        }
    }

    #[test]
    fn whole_drawing_move_preserves_offscreen_model_anchors_on_click() {
        let bars = vec![OhlcBar {
            time: 100,
            open: 10.0,
            high: 12.0,
            low: 9.0,
            close: 11.0,
            volume: 1.0,
        }];
        let pane = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(300.0, 100.0));
        let transform = SceneTransform::new(
            SceneRect::from_min_max(ScenePoint::new(0.0, 0.0), ScenePoint::new(300.0, 100.0)),
            LogicalRange {
                from: 20.0,
                to: 30.0,
            },
            PriceRange {
                min: 110.0,
                max: 135.0,
            },
        );
        let anchor = DrawingAnchor::LogicalPrice {
            logical: 0.0,
            price: 100.0,
        };
        let moved = moved_drawing_anchor_by_drag_delta(
            anchor,
            DrawingDragDelta {
                logical: 0.0,
                coordinate: 0.0,
                screen: egui::Vec2::ZERO,
            },
            pane,
            transform,
            &bars,
            Some(60),
        )
        .expect("anchor should remain valid");

        assert_eq!(moved, anchor);
    }

    #[test]
    fn whole_drawing_move_translates_offscreen_model_anchors_without_edge_clamping() {
        let bars = vec![
            OhlcBar {
                time: 100,
                open: 10.0,
                high: 12.0,
                low: 9.0,
                close: 11.0,
                volume: 1.0,
            },
            OhlcBar {
                time: 160,
                open: 11.0,
                high: 13.0,
                low: 10.0,
                close: 12.0,
                volume: 1.0,
            },
        ];
        let pane = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(300.0, 100.0));
        let transform = SceneTransform::new(
            SceneRect::from_min_max(ScenePoint::new(0.0, 0.0), ScenePoint::new(300.0, 100.0)),
            LogicalRange {
                from: 20.0,
                to: 30.0,
            },
            PriceRange {
                min: 110.0,
                max: 135.0,
            },
        );
        let moved = moved_drawing_anchor_by_drag_delta(
            DrawingAnchor::LogicalPrice {
                logical: 0.0,
                price: 100.0,
            },
            DrawingDragDelta {
                logical: 2.0,
                coordinate: 5.0,
                screen: egui::vec2(30.0, -20.0),
            },
            pane,
            transform,
            &bars,
            Some(60),
        )
        .expect("anchor should move in model space");

        match moved {
            DrawingAnchor::LogicalPrice { logical, price } => {
                assert!((logical - 2.0).abs() < 1e-9);
                assert!((price - 105.0).abs() < 1e-9);
            }
            other => panic!("expected logical-price anchor, got {other:?}"),
        }
    }

    #[test]
    fn sticked_ohlc_screen_point_tracks_selected_field_price() {
        let bars = vec![OhlcBar {
            time: 100,
            open: 10.0,
            high: 12.0,
            low: 9.0,
            close: 11.0,
            volume: 1.0,
        }];
        let pane = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(200.0, 100.0));
        let transform = SceneTransform::new(
            SceneRect::from_min_max(ScenePoint::new(0.0, 0.0), ScenePoint::new(200.0, 100.0)),
            LogicalRange { from: 0.0, to: 1.0 },
            PriceRange {
                min: 9.0,
                max: 12.0,
            },
        );

        let point = drawing_anchor_screen_point(
            DrawingAnchor::StickedOhlc {
                time: 100,
                field: OhlcField::High,
                price: 12.0,
                source_interval_seconds: Some(60),
            },
            pane,
            transform,
            &bars,
            Some(60),
        )
        .expect("sticked OHLC anchor should resolve");

        assert_eq!(point.x, transform.logical_to_x(0.0));
        assert_eq!(point.y, transform.price_to_y(12.0));
    }

    #[test]
    fn sticked_ohlc_cross_interval_keeps_original_snap_price() {
        let lower_timeframe_bars = vec![OhlcBar {
            time: 100,
            open: 10.0,
            high: 20.0,
            low: 9.0,
            close: 11.0,
            volume: 1.0,
        }];
        let pane = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(200.0, 100.0));
        let transform = SceneTransform::new(
            SceneRect::from_min_max(ScenePoint::new(0.0, 0.0), ScenePoint::new(200.0, 100.0)),
            LogicalRange { from: 0.0, to: 1.0 },
            PriceRange {
                min: 9.0,
                max: 20.0,
            },
        );

        let point = drawing_anchor_screen_point(
            DrawingAnchor::StickedOhlc {
                time: 100,
                field: OhlcField::High,
                price: 12.0,
                source_interval_seconds: Some(3_600),
            },
            pane,
            transform,
            &lower_timeframe_bars,
            Some(900),
        )
        .expect("sticked OHLC anchor should resolve across intervals");

        assert_eq!(point.x, transform.logical_to_x(0.0));
        assert_eq!(point.y, transform.price_to_y(12.0));
    }

    #[test]
    fn time_anchor_projection_uses_lower_timeframe_timestamp_position() {
        let lower_timeframe_bars = vec![
            OhlcBar {
                time: 0,
                open: 10.0,
                high: 11.0,
                low: 9.0,
                close: 10.5,
                volume: 1.0,
            },
            OhlcBar {
                time: 900,
                open: 10.0,
                high: 11.0,
                low: 9.0,
                close: 10.5,
                volume: 1.0,
            },
        ];

        assert_eq!(
            time_to_logical_for_bars(&lower_timeframe_bars, 3_600, Some(900)),
            Some(4.0)
        );
    }

    #[test]
    fn drawing_toolbar_guard_covers_left_rail_buttons() {
        let rail = egui::Rect::from_min_max(egui::pos2(0.0, 80.0), egui::pos2(56.0, 520.0));
        let chart_plot = egui::Rect::from_min_max(egui::pos2(64.0, 80.0), egui::pos2(840.0, 520.0));
        let position = drawing_toolbar_position_from_rail(rail);
        let rect = drawing_toolbar_rect_from_rail(rail);

        assert!(rect.contains(position + egui::vec2(1.0, 1.0)));
        assert!(rect.contains(
            position
                + egui::vec2(
                    super::DRAWING_TOOL_BUTTON_WIDTH,
                    super::DRAWING_TOOL_BUTTON_HEIGHT * super::DRAWING_TOOLS.len() as f32
                        + super::DRAWING_TOOL_BUTTON_GAP
                            * super::DRAWING_TOOLS.len().saturating_sub(1) as f32,
                )
        ));
        assert!(rail.contains(rect.center()));
        assert!(!rect.intersects(chart_plot));
    }

    #[test]
    fn selected_drawing_toolbar_defaults_to_compact_plot_top_rect() {
        let plot = egui::Rect::from_min_max(egui::pos2(80.0, 60.0), egui::pos2(980.0, 620.0));
        let rect = selected_drawing_toolbar_rect_from_plot(plot);

        assert_eq!(rect.width(), super::SELECTED_DRAWING_TOOLBAR_WIDTH);
        assert_eq!(rect.height(), super::SELECTED_DRAWING_TOOLBAR_HEIGHT);
        assert_eq!(rect.center().x, plot.center().x);
        assert_eq!(
            rect.top(),
            plot.top() + super::SELECTED_DRAWING_TOOLBAR_TOP_OFFSET
        );
    }

    #[test]
    fn selected_drawing_toolbar_position_clamps_to_chart_bounds() {
        let bounds = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(810.0, 620.0));
        let size = egui::vec2(480.0, 38.0);

        assert_eq!(
            clamp_selected_drawing_toolbar_position(egui::pos2(-100.0, -20.0), bounds, size),
            egui::pos2(16.0, 26.0)
        );
        assert_eq!(
            clamp_selected_drawing_toolbar_position(egui::pos2(900.0, 700.0), bounds, size),
            egui::pos2(324.0, 576.0)
        );
    }

    #[test]
    fn drawing_actions_lock_hide_and_delete_selected_store_entries() {
        let mut drawings = DrawingStore::new();
        let id = drawings
            .create(
                DrawingKind::Text,
                PaneId::Main,
                vec![DrawingAnchor::LogicalPrice {
                    logical: 10.0,
                    price: 100.0,
                }],
            )
            .expect("text drawing");

        assert_eq!(
            apply_drawing_action_to_store(&mut drawings, id, DrawingAction::ToggleLock)
                .expect("lock action"),
            DrawingActionEffect::LockChanged(true)
        );
        assert!(drawings.get(id).is_some_and(|drawing| drawing.locked));
        assert!(matches!(
            apply_drawing_action_to_store(&mut drawings, id, DrawingAction::Delete),
            Err(DrawingEditError::LockedDrawing(locked_id)) if locked_id == id
        ));
        assert!(matches!(
            apply_drawing_action_to_store(&mut drawings, id, DrawingAction::EditText),
            Err(DrawingEditError::LockedDrawing(locked_id)) if locked_id == id
        ));
        assert!(drawings.get(id).is_some());

        assert_eq!(
            apply_drawing_action_to_store(&mut drawings, id, DrawingAction::ToggleLock)
                .expect("unlock action"),
            DrawingActionEffect::LockChanged(false)
        );
        assert_eq!(
            apply_drawing_action_to_store(&mut drawings, id, DrawingAction::EditText)
                .expect("edit text action"),
            DrawingActionEffect::EditText
        );
        assert_eq!(
            apply_drawing_action_to_store(&mut drawings, id, DrawingAction::Hide)
                .expect("hide action"),
            DrawingActionEffect::Hidden
        );
        assert!(drawings.get(id).is_some_and(|drawing| !drawing.visible));
        assert_eq!(drawings.visible().count(), 0);
        assert_eq!(
            apply_drawing_action_to_store(&mut drawings, id, DrawingAction::Delete)
                .expect("delete action"),
            DrawingActionEffect::Deleted
        );
        assert!(drawings.get(id).is_none());
    }

    #[test]
    fn drawing_action_opens_supported_settings_surfaces() {
        let mut drawings = DrawingStore::new();
        let fib = drawings
            .create(
                DrawingKind::FibRetracement,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 1.0,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 2.0,
                        price: 110.0,
                    },
                ],
            )
            .expect("fib drawing");
        let trend = drawings
            .create(
                DrawingKind::TrendLine,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 1.0,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 2.0,
                        price: 110.0,
                    },
                ],
            )
            .expect("trend drawing");
        let position = drawings
            .create(
                DrawingKind::LongPosition,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 1.0,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 6.0,
                        price: 110.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 6.0,
                        price: 95.0,
                    },
                ],
            )
            .expect("position drawing");

        assert_eq!(
            apply_drawing_action_to_store(&mut drawings, fib, DrawingAction::OpenSettings)
                .expect("open fib settings"),
            DrawingActionEffect::OpenSettings
        );
        assert_eq!(
            apply_drawing_action_to_store(&mut drawings, position, DrawingAction::OpenSettings)
                .expect("open position settings"),
            DrawingActionEffect::OpenSettings
        );
        assert_eq!(
            apply_drawing_action_to_store(&mut drawings, trend, DrawingAction::OpenSettings)
                .expect("ignore drawing without settings"),
            DrawingActionEffect::Noop
        );
    }

    #[test]
    fn fib_extend_mode_updates_boolean_flags() {
        let mut settings = crate::chart_core::FibRetracementSettings::default();
        assert_eq!(
            fib_extend_mode(settings.extend_left, settings.extend_right),
            FibExtendMode::None
        );
        assert!(set_fib_extend_mode(&mut settings, FibExtendMode::Both));
        assert!(settings.extend_left);
        assert!(settings.extend_right);
        assert_eq!(
            fib_extend_mode(settings.extend_left, settings.extend_right),
            FibExtendMode::Both
        );
        assert!(set_fib_extend_mode(&mut settings, FibExtendMode::Right));
        assert!(!settings.extend_left);
        assert!(settings.extend_right);
        assert!(!set_fib_extend_mode(&mut settings, FibExtendMode::Right));
    }

    #[test]
    fn fib_level_helpers_add_and_remove_custom_rows() {
        let mut settings = crate::chart_core::FibRetracementSettings::default();
        let original_len = settings.levels.len();

        add_fib_level(&mut settings);
        assert_eq!(settings.levels.len(), original_len + 1);
        assert_eq!(settings.levels.last().expect("added level").value, 1.272);

        assert!(remove_fib_level(&mut settings, original_len));
        assert_eq!(settings.levels.len(), original_len);
        let missing_index = settings.levels.len();
        assert!(!remove_fib_level(&mut settings, missing_index));
    }

    #[test]
    fn next_fib_level_value_skips_existing_extensions() {
        let mut settings = crate::chart_core::FibRetracementSettings::default();
        settings
            .levels
            .push(crate::chart_core::FibRetracementLevel::new(
                1.272,
                ChartColor::rgb(255, 152, 0),
            ));

        assert_eq!(next_fib_level_value(&settings.levels), 1.414);
    }

    #[test]
    fn fib_anchor_logical_price_resolves_model_and_sticked_points() {
        let bars = vec![
            OhlcBar {
                time: 100,
                open: 10.0,
                high: 12.0,
                low: 9.0,
                close: 11.0,
                volume: 1.0,
            },
            OhlcBar {
                time: 160,
                open: 11.0,
                high: 14.0,
                low: 10.0,
                close: 13.0,
                volume: 1.0,
            },
        ];

        assert_eq!(
            fib_anchor_logical_price(
                DrawingAnchor::LogicalPrice {
                    logical: 2.5,
                    price: 100.0,
                },
                &bars,
                Some(60),
            ),
            Some((2.5, 100.0))
        );
        assert_eq!(
            fib_anchor_logical_price(
                DrawingAnchor::TimePrice {
                    time: 130,
                    price: 101.0,
                },
                &bars,
                Some(60),
            ),
            Some((0.5, 101.0))
        );
        assert_eq!(
            fib_anchor_logical_price(
                DrawingAnchor::StickedOhlc {
                    time: 160,
                    field: OhlcField::High,
                    price: 99.0,
                    source_interval_seconds: Some(60),
                },
                &bars,
                Some(60),
            ),
            Some((1.0, 14.0))
        );
        assert_eq!(
            fib_anchor_logical_price(DrawingAnchor::PriceOnly { price: 100.0 }, &bars, Some(60)),
            None
        );
    }

    #[test]
    fn selected_drawing_delete_shortcut_uses_selected_drawing_when_keyboard_is_free() {
        let id = crate::chart_core::DrawingId::new(7);

        assert_eq!(
            selected_drawing_delete_shortcut_target(Some(id), false, false, true, false),
            Some(id)
        );
        assert_eq!(
            selected_drawing_delete_shortcut_target(Some(id), false, false, false, true),
            Some(id)
        );
        assert_eq!(
            selected_drawing_delete_shortcut_target(Some(id), false, false, false, false),
            None
        );
        assert_eq!(
            selected_drawing_delete_shortcut_target(Some(id), true, false, true, false),
            None
        );
        assert_eq!(
            selected_drawing_delete_shortcut_target(Some(id), false, true, true, false),
            None
        );
        assert_eq!(
            selected_drawing_delete_shortcut_target(None, false, false, true, false),
            None
        );
    }

    #[test]
    fn selected_drawing_cut_shortcut_uses_selected_drawing_when_keyboard_is_free() {
        let id = crate::chart_core::DrawingId::new(9);

        assert_eq!(
            selected_drawing_cut_shortcut_target(Some(id), false, false, true),
            Some(id)
        );
        assert_eq!(
            selected_drawing_cut_shortcut_target(Some(id), false, false, false),
            None
        );
        assert_eq!(
            selected_drawing_cut_shortcut_target(Some(id), true, false, true),
            None
        );
        assert_eq!(
            selected_drawing_cut_shortcut_target(Some(id), false, true, true),
            None
        );
        assert_eq!(
            selected_drawing_cut_shortcut_target(None, false, false, true),
            None
        );
    }

    #[test]
    fn app_undo_shortcut_requires_free_keyboard_and_history() {
        assert!(app_undo_shortcut(false, false, true, true));
        assert!(!app_undo_shortcut(true, false, true, true));
        assert!(!app_undo_shortcut(false, true, true, true));
        assert!(!app_undo_shortcut(false, false, false, true));
        assert!(!app_undo_shortcut(false, false, true, false));
    }

    #[test]
    fn app_redo_shortcut_requires_free_keyboard_and_history() {
        assert!(app_redo_shortcut(false, false, true, true));
        assert!(!app_redo_shortcut(true, false, true, true));
        assert!(!app_redo_shortcut(false, true, true, true));
        assert!(!app_redo_shortcut(false, false, false, true));
        assert!(!app_redo_shortcut(false, false, true, false));
    }

    #[test]
    fn app_snapshot_stack_skips_duplicates_and_trims_oldest_entries() {
        fn snapshot(marker: u64) -> AppUndoSnapshot {
            AppUndoSnapshot {
                drawings: DrawingStore::new(),
                selected_drawing_id: Some(DrawingId::new(marker)),
                study_instances: Vec::new(),
                selected_study_instance_id: None,
                study_settings_instance_id: None,
                selected_timeframe: DEFAULT_INTERVAL,
            }
        }

        let mut stack = Vec::new();
        assert!(push_app_snapshot_stack(&mut stack, snapshot(1)));
        assert!(!push_app_snapshot_stack(&mut stack, snapshot(1)));
        for marker in 2..=(APP_UNDO_LIMIT as u64 + 2) {
            assert!(push_app_snapshot_stack(&mut stack, snapshot(marker)));
        }

        assert_eq!(stack.len(), APP_UNDO_LIMIT);
        assert_eq!(
            stack
                .first()
                .and_then(|snapshot| snapshot.selected_drawing_id),
            Some(DrawingId::new(3))
        );
        assert_eq!(
            stack
                .last()
                .and_then(|snapshot| snapshot.selected_drawing_id),
            Some(DrawingId::new(APP_UNDO_LIMIT as u64 + 2))
        );
    }

    #[test]
    fn drawing_clipboard_shortcuts_require_free_keyboard() {
        let id = crate::chart_core::DrawingId::new(17);

        assert_eq!(
            drawing_clipboard_shortcut(Some(id), false, false, false, true, false),
            Some(DrawingClipboardShortcut::Copy(id))
        );
        assert_eq!(
            drawing_clipboard_shortcut(None, true, false, false, false, true),
            Some(DrawingClipboardShortcut::Paste)
        );
        assert_eq!(
            drawing_clipboard_shortcut(Some(id), true, false, false, false, false),
            None
        );
        assert_eq!(
            drawing_clipboard_shortcut(Some(id), true, true, false, true, false),
            None
        );
        assert_eq!(
            drawing_clipboard_shortcut(Some(id), true, false, true, true, false),
            None
        );
    }

    #[test]
    fn drawing_clipboard_events_detect_platform_shortcuts() {
        let (copy, paste) =
            drawing_clipboard_event_flags(&[egui::Event::Copy, egui::Event::Paste(String::new())]);
        assert!(copy);
        assert!(paste);
        assert!(drawing_cut_event_flag(&[egui::Event::Cut]));
    }

    #[test]
    fn copied_drawing_z_order_moves_to_front() {
        let mut drawings = DrawingStore::new();
        drawings
            .insert(
                crate::chart_core::DrawingSpec::new(
                    crate::chart_core::DrawingId::new(1),
                    DrawingKind::HorizontalLine,
                    PaneId::Main,
                    vec![DrawingAnchor::PriceOnly { price: 10.0 }],
                )
                .with_z_order(3),
            )
            .expect("drawing");

        assert_eq!(next_drawing_z_order(&drawings), 4);
    }

    #[test]
    fn selected_drawing_blank_chart_click_clears_selected_toolbar() {
        let id = crate::chart_core::DrawingId::new(8);

        assert!(selected_drawing_blank_click_should_clear(
            true, None, true, true
        ));
        assert!(!selected_drawing_blank_click_should_clear(
            false, None, true, true
        ));
        assert!(!selected_drawing_blank_click_should_clear(
            true,
            Some(id),
            true,
            true
        ));
        assert!(!selected_drawing_blank_click_should_clear(
            true, None, false, true
        ));
        assert!(!selected_drawing_blank_click_should_clear(
            true, None, true, false
        ));
    }

    #[test]
    fn passive_drawing_states_do_not_block_chart_navigation() {
        assert!(!drawing_layer_blocks_chart_input(
            false, false, false, false, false
        ));
        assert!(drawing_layer_blocks_chart_input(
            true, false, false, false, false
        ));
        assert!(drawing_layer_blocks_chart_input(
            false, true, false, false, false
        ));
        assert!(drawing_layer_blocks_chart_input(
            false, false, true, false, false
        ));
        assert!(drawing_layer_blocks_chart_input(
            false, false, false, true, false
        ));
        assert!(drawing_layer_blocks_chart_input(
            false, false, false, false, true
        ));
    }

    #[test]
    fn measure_click_move_click_places_then_chart_click_clears() {
        fn point(logical: f64, price: f64) -> MeasurePoint {
            MeasurePoint {
                pane: PaneId::Main,
                logical,
                price,
                time: Some(logical as i64),
            }
        }

        let start = point(2.0, 100.0);
        let hover = point(5.0, 104.0);
        let finish = point(8.0, 109.0);
        let state = measure_state_after_click(None, Some(start), None).expect("placing");

        assert_eq!(
            state,
            MeasureState {
                start,
                current: start,
                phase: MeasurePhase::Placing,
            }
        );

        let state = measure_state_after_hover(state, hover);
        assert_eq!(state.current, hover);
        assert_eq!(state.phase, MeasurePhase::Placing);

        let state = measure_state_after_click(Some(state), Some(finish), None).expect("placed");
        assert_eq!(state.current, finish);
        assert_eq!(state.phase, MeasurePhase::Placed);
        assert!(!measure_state_blocks_chart_input(state));
        assert!(measure_state_cleared_by_chart_click(state));
        assert_eq!(
            measure_state_after_click(Some(state), Some(point(10.0, 111.0)), None),
            None
        );
    }

    #[test]
    fn measure_handle_click_adjusts_without_clearing() {
        fn point(logical: f64, price: f64) -> MeasurePoint {
            MeasurePoint {
                pane: PaneId::Main,
                logical,
                price,
                time: None,
            }
        }

        let start = point(2.0, 100.0);
        let current = point(8.0, 109.0);
        let adjusted_start = point(3.0, 101.0);
        let placed = MeasureState {
            start,
            current,
            phase: MeasurePhase::Placed,
        };
        let adjusting =
            measure_state_after_click(Some(placed), Some(start), Some(MeasureDragTarget::Start))
                .expect("adjusting");

        assert_eq!(
            adjusting.phase,
            MeasurePhase::Adjusting(MeasureDragTarget::Start)
        );
        assert!(measure_state_blocks_chart_input(adjusting));
        assert!(!measure_state_cleared_by_chart_click(adjusting));
        let adjusted =
            measure_state_after_adjust_drag(adjusting, MeasureDragTarget::Start, adjusted_start);
        assert_eq!(adjusted.start, adjusted_start);
        assert_eq!(adjusted.current, current);
        assert_eq!(
            measure_state_after_release(adjusted).phase,
            MeasurePhase::Placed
        );
    }

    #[test]
    fn measure_stats_report_price_percent_bars_and_duration() {
        let bars = (0..20)
            .map(|index| OhlcBar {
                time: index,
                open: 100.0,
                high: 101.0,
                low: 99.0,
                close: 100.0,
                volume: 1_000.0,
            })
            .collect::<Vec<_>>();
        let stats = measure_stats(
            MeasurePoint {
                pane: PaneId::Main,
                logical: 10.2,
                price: 100.0,
                time: Some(1_000),
            },
            MeasurePoint {
                pane: PaneId::Main,
                logical: 15.8,
                price: 112.5,
                time: Some(19_120),
            },
            &bars,
        );

        assert_eq!(stats.price_delta, 12.5);
        assert_eq!(stats.percent_delta, Some(12.5));
        assert_eq!(stats.bars_delta, 6);
        assert_eq!(stats.seconds_delta, Some(18_120));
        assert_eq!(stats.volume, Some(7_000.0));
        assert_eq!(format_measure_duration(18_120), "5h 2m");
        assert_eq!(
            format_measure_stats(stats),
            "12.50 (12.50%)  6 bars, 5h 2m  Vol 7.00 K"
        );
    }

    #[test]
    fn measure_stats_avoid_percent_when_start_price_is_zero() {
        let stats = measure_stats(
            MeasurePoint {
                pane: PaneId::Main,
                logical: 6.0,
                price: 0.0,
                time: None,
            },
            MeasurePoint {
                pane: PaneId::Main,
                logical: 4.0,
                price: -2.0,
                time: None,
            },
            &[] as &[OhlcBar],
        );

        assert_eq!(
            stats,
            MeasureStats {
                price_delta: -2.0,
                percent_delta: None,
                bars_delta: 2,
                seconds_delta: None,
                volume: None,
            }
        );
        assert_eq!(format_measure_stats(stats), "-2.00 (n/a)  2 bars, n/a");
    }

    #[test]
    fn drawing_style_helpers_target_text_and_line_styles() {
        let mut line_style = crate::chart_core::DrawingStyle::default();
        set_drawing_style_color(
            DrawingKind::TrendLine,
            &mut line_style,
            crate::chart_core::ChartColor::rgb(247, 49, 67),
        );
        line_style.stroke_width = 4.0;
        line_style.line_style = crate::chart_core::DrawingLineStyle::Dotted;

        assert_eq!(
            line_style.stroke_color,
            crate::chart_core::ChartColor::rgb(247, 49, 67)
        );
        assert_eq!(line_style.stroke_width, 4.0);
        assert_eq!(
            line_style.line_style,
            crate::chart_core::DrawingLineStyle::Dotted
        );

        let mut text_style = crate::chart_core::DrawingStyle::default();
        let original_stroke = text_style.stroke_color;
        set_drawing_style_color(
            DrawingKind::Text,
            &mut text_style,
            crate::chart_core::ChartColor::rgb(45, 98, 255),
        );

        assert_eq!(text_style.stroke_color, original_stroke);
        assert_eq!(
            text_style.text_color,
            crate::chart_core::ChartColor::rgb(45, 98, 255)
        );
    }

    #[test]
    fn magnet_mode_snaps_to_loaded_ohlc_values() {
        let bars = vec![OhlcBar {
            time: 100,
            open: 90.0,
            high: 110.0,
            low: 82.0,
            close: 100.0,
            volume: 1.0,
        }];
        let transform = SceneTransform::new(
            SceneRect::from_min_max(ScenePoint::new(0.0, 0.0), ScenePoint::new(100.0, 100.0)),
            LogicalRange { from: 0.0, to: 1.0 },
            PriceRange {
                min: 80.0,
                max: 120.0,
            },
        );
        let high_y = transform.price_to_y(110.0);

        assert!(
            magnet_snap_for_position(
                PaneId::Main,
                transform,
                egui::pos2(2.0, high_y + 8.0),
                &bars,
                DrawingMagnetMode::Off,
            )
            .is_none()
        );
        assert_eq!(
            magnet_snap_for_position(
                PaneId::Main,
                transform,
                egui::pos2(2.0, high_y + 8.0),
                &bars,
                DrawingMagnetMode::Weak,
            )
            .expect("weak magnet near OHLC")
            .price,
            110.0
        );
        assert_eq!(
            magnet_snap_for_position(
                PaneId::Main,
                transform,
                egui::pos2(2.0, high_y + 8.0),
                &bars,
                DrawingMagnetMode::Weak,
            )
            .expect("weak magnet near OHLC")
            .field,
            OhlcField::High
        );
        assert_eq!(
            sticked_ohlc_anchor_from_snap(
                magnet_snap_for_position(
                    PaneId::Main,
                    transform,
                    egui::pos2(2.0, high_y + 8.0),
                    &bars,
                    DrawingMagnetMode::Weak,
                )
                .expect("weak magnet near OHLC"),
                Some(60),
            ),
            DrawingAnchor::StickedOhlc {
                time: 100,
                field: OhlcField::High,
                price: 110.0,
                source_interval_seconds: Some(60),
            }
        );
        assert!(
            magnet_snap_for_position(
                PaneId::Main,
                transform,
                egui::pos2(2.0, high_y + 10_000.0),
                &bars,
                DrawingMagnetMode::Weak,
            )
            .is_none()
        );
        assert_eq!(
            magnet_snap_for_position(
                PaneId::Main,
                transform,
                egui::pos2(2.0, high_y + 44.0),
                &bars,
                DrawingMagnetMode::Strong,
            )
            .expect("strong magnet ignores distance")
            .time,
            100
        );
    }

    #[test]
    fn magnet_modifier_temporarily_inverts_configured_mode() {
        assert_eq!(
            effective_drawing_magnet_mode(DrawingMagnetMode::Off, false),
            DrawingMagnetMode::Off
        );
        assert_eq!(
            effective_drawing_magnet_mode(DrawingMagnetMode::Off, true),
            DrawingMagnetMode::Weak
        );
        assert_eq!(
            effective_drawing_magnet_mode(DrawingMagnetMode::Weak, true),
            DrawingMagnetMode::Off
        );
        assert_eq!(
            effective_drawing_magnet_mode(DrawingMagnetMode::Strong, true),
            DrawingMagnetMode::Off
        );
    }

    #[test]
    fn magnet_modifier_uses_command_shortcut() {
        let mut modifiers = egui::Modifiers {
            command: true,
            mac_cmd: true,
            ..Default::default()
        };
        assert!(drawing_magnet_modifier(modifiers));

        modifiers.command = false;
        modifiers.mac_cmd = false;
        modifiers.ctrl = true;
        assert!(!drawing_magnet_modifier(modifiers));

        modifiers.ctrl = false;
        modifiers.alt = true;
        assert!(!drawing_magnet_modifier(modifiers));
    }

    #[test]
    fn magnet_modifier_only_applies_to_drawing_placement_or_drag() {
        assert!(!drawing_magnet_modifier_should_apply(false, false, false));
        assert!(drawing_magnet_modifier_should_apply(true, false, false));
        assert!(drawing_magnet_modifier_should_apply(false, true, false));
        assert!(drawing_magnet_modifier_should_apply(false, false, true));
    }

    #[test]
    fn magnet_quick_action_toggles_off_and_weak_mode() {
        let mut mode = DrawingMagnetMode::Off;
        assert!(toggle_drawing_magnet_quick(&mut mode));
        assert_eq!(mode, DrawingMagnetMode::Weak);
        assert!(toggle_drawing_magnet_quick(&mut mode));
        assert_eq!(mode, DrawingMagnetMode::Off);

        mode = DrawingMagnetMode::Strong;
        assert!(toggle_drawing_magnet_quick(&mut mode));
        assert_eq!(mode, DrawingMagnetMode::Off);
    }

    #[test]
    fn drawing_context_target_takes_priority_over_plot_region() {
        let hit = DrawingHit {
            drawing_id: crate::chart_core::DrawingId::new(42),
            target: DrawingHitTarget::Body,
            distance: 0.0,
        };

        assert_eq!(
            chart_context_target_for_drawing_hit(ChartContextTarget::Plot, Some(hit)),
            ChartContextTarget::Drawing(hit.drawing_id)
        );
        assert_eq!(
            chart_context_target_for_drawing_hit(ChartContextTarget::PriceScale, None),
            ChartContextTarget::PriceScale
        );
    }

    #[test]
    fn backfill_triggers_near_oldest_loaded_bar() {
        assert!(should_request_backfill(
            LogicalRange {
                from: 12.0,
                to: 92.0
            },
            Some(1_700_000_000),
            false,
            false,
            None,
        ));
        assert!(!should_request_backfill(
            LogicalRange {
                from: 120.0,
                to: 200.0
            },
            Some(1_700_000_000),
            false,
            false,
            None,
        ));
    }

    #[test]
    fn backfill_does_not_repeat_same_oldest_time() {
        assert!(!should_request_backfill(
            LogicalRange {
                from: 0.0,
                to: 80.0
            },
            Some(1_700_000_000),
            false,
            false,
            Some(1_700_000_000),
        ));
    }

    #[test]
    fn chart_market_data_sync_helpers_keep_service_and_widget_store_aligned() {
        fn bar(time: i64, close: f64) -> OhlcBar {
            OhlcBar {
                time,
                open: close,
                high: close + 1.0,
                low: close - 1.0,
                close,
                volume: 100.0,
            }
        }

        let key = MarketDataKey::new(MarketSource::YFinance, "AAPL", Resolution::Minutes(5));
        let mut service = MarketDataService::new();
        let mut chart_store = BarStore::unbounded();

        let loaded = replace_chart_bars_from_service(
            &mut service,
            &mut chart_store,
            key.clone(),
            vec![bar(120, 12.0), bar(60, 11.0)],
        );

        assert_eq!(loaded.bar_count, 2);
        assert_eq!(
            service.store(&key).expect("service store").to_vec(),
            chart_store.to_vec()
        );

        let (tail_update, after_tail) = apply_chart_tail_from_service(
            &mut service,
            &mut chart_store,
            key.clone(),
            bar(180, 13.0),
        );

        assert_eq!(tail_update, TailUpdate::Appended);
        assert_eq!(after_tail.bar_count, 3);
        assert_eq!(
            service.store(&key).expect("service store").to_vec(),
            chart_store.to_vec()
        );

        let (added, after_history) = prepend_chart_history_from_service(
            &mut service,
            &mut chart_store,
            key.clone(),
            vec![bar(0, 10.0), bar(60, 99.0)],
        );

        assert_eq!(added, 1);
        assert_eq!(after_history.first_time, Some(0));
        assert_eq!(
            service.store(&key).expect("service store").to_vec(),
            chart_store.to_vec()
        );
    }

    #[test]
    fn search_candidates_filter_by_source_and_type() {
        let candidates = search_candidates(
            "btc/usdt",
            SearchSourceFilter::Source(MarketSource::Binance),
            SearchTypeFilter::Crypto,
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].source, MarketSource::Binance);
        assert_eq!(candidates[0].symbol_type, SymbolType::Crypto);
        assert_eq!(candidates[0].symbol, "BTCUSDT");

        assert!(
            search_candidates(
                "btc/usdt",
                SearchSourceFilter::Source(MarketSource::Binance),
                SearchTypeFilter::Stock,
            )
            .is_empty()
        );

        let massive_candidates = search_candidates(
            "brk.b",
            SearchSourceFilter::Source(MarketSource::Massive),
            SearchTypeFilter::Stock,
        );
        assert_eq!(massive_candidates.len(), 1);
        assert_eq!(massive_candidates[0].source, MarketSource::Massive);
        assert_eq!(massive_candidates[0].symbol_type, SymbolType::Stock);
        assert_eq!(massive_candidates[0].symbol, "BRK.B");
    }

    #[test]
    fn enter_search_prefers_current_source_when_filter_allows_it() {
        let candidate = best_search_candidate(
            "btc",
            MarketSource::CoinGecko,
            SearchSourceFilter::All,
            SearchTypeFilter::Crypto,
        )
        .expect("candidate");

        assert_eq!(candidate.source, MarketSource::CoinGecko);
        assert_eq!(candidate.symbol, "bitcoin");
    }

    #[test]
    fn type_to_search_accepts_symbol_prefix_characters() {
        assert!(type_to_search_char('s'));
        assert!(type_to_search_char('4'));
        assert!(type_to_search_char('/'));
        assert!(type_to_search_char('.'));
        assert!(!type_to_search_char(' '));
        assert!(!type_to_search_char('\n'));
    }

    #[test]
    fn typed_search_prefix_collects_text_events_only() {
        let events = vec![
            egui::Event::Text("s".to_string()),
            egui::Event::Copy,
            egui::Event::Text("py ".to_string()),
        ];

        assert_eq!(typed_search_prefix(&events), Some("spy".to_string()));
    }

    #[test]
    fn typed_timeframe_prefix_starts_with_digits_only() {
        let events = vec![
            egui::Event::Text("15".to_string()),
            egui::Event::Copy,
            egui::Event::Text("h ".to_string()),
        ];
        assert_eq!(typed_timeframe_prefix(&events), Some("15h".to_string()));

        let symbol_events = vec![egui::Event::Text("s1".to_string())];
        assert_eq!(typed_timeframe_prefix(&symbol_events), None);
    }

    #[test]
    fn timeframe_input_parses_bare_numbers_as_minutes_and_m_suffix_as_months() {
        let fifteen = parse_timeframe_input("15").expect("15 minute candidate");
        assert_eq!(fifteen.unit, TimeframeInputUnit::Minutes);
        assert_eq!(fifteen.amount, 15);
        assert_eq!(
            fifteen.interval.map(|interval| interval.label()),
            Some("15m")
        );
        assert_eq!(timeframe_input_preview(Some(fifteen), "15"), "15 minutes");

        let month = parse_timeframe_input("1m").expect("1 month candidate");
        assert_eq!(month.unit, TimeframeInputUnit::Months);
        assert_eq!(month.amount, 1);
        assert_eq!(month.interval.map(|interval| interval.label()), Some("M"));
        assert_eq!(timeframe_input_preview(Some(month), "1m"), "1 month");

        let hour = parse_timeframe_input("1h").expect("1 hour candidate");
        assert_eq!(hour.unit, TimeframeInputUnit::Hours);
        assert_eq!(hour.interval.map(|interval| interval.label()), Some("1h"));
        assert_eq!(
            plural_timeframe_label(4, TimeframeInputUnit::Hours),
            "4 hours"
        );
    }

    #[test]
    fn timeframe_input_reports_unsupported_intervals() {
        let unsupported = parse_timeframe_input("2h").expect("unsupported candidate");
        assert_eq!(unsupported.unit, TimeframeInputUnit::Hours);
        assert_eq!(unsupported.amount, 2);
        assert_eq!(unsupported.interval, None);
        assert_eq!(
            timeframe_input_preview(Some(unsupported), "2h"),
            "Unsupported interval: 2h"
        );
        assert_eq!(parse_timeframe_input("abc"), None);
    }

    #[test]
    fn search_palette_chart_click_dismisses_only_outside_palette() {
        let chart = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(500.0, 400.0));
        let palette = egui::Rect::from_min_max(egui::pos2(150.0, 100.0), egui::pos2(350.0, 220.0));

        assert!(search_palette_chart_click_should_cancel(
            true,
            Some(palette),
            chart,
            true,
            Some(egui::pos2(50.0, 50.0)),
        ));
        assert!(!search_palette_chart_click_should_cancel(
            true,
            Some(palette),
            chart,
            true,
            Some(egui::pos2(200.0, 150.0)),
        ));
        assert!(!search_palette_chart_click_should_cancel(
            true,
            Some(palette),
            chart,
            true,
            Some(egui::pos2(700.0, 50.0)),
        ));
        assert!(!search_palette_chart_click_should_cancel(
            false,
            Some(palette),
            chart,
            true,
            Some(egui::pos2(50.0, 50.0)),
        ));
    }

    #[test]
    fn floating_panel_chart_click_dismisses_only_outside_panel_rects() {
        let chart = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(500.0, 400.0));
        let panel = egui::Rect::from_min_max(egui::pos2(120.0, 80.0), egui::pos2(360.0, 240.0));

        assert!(floating_panels_chart_click_should_cancel(
            true,
            &[Some(panel)],
            chart,
            true,
            Some(egui::pos2(50.0, 50.0)),
        ));
        assert!(!floating_panels_chart_click_should_cancel(
            true,
            &[Some(panel)],
            chart,
            true,
            Some(egui::pos2(150.0, 100.0)),
        ));
        assert!(!floating_panels_chart_click_should_cancel(
            false,
            &[Some(panel)],
            chart,
            true,
            Some(egui::pos2(50.0, 50.0)),
        ));
        assert!(!floating_panels_chart_click_should_cancel(
            true,
            &[Some(panel)],
            chart,
            true,
            Some(egui::pos2(700.0, 50.0)),
        ));
    }

    #[test]
    fn floating_panel_chart_click_blocks_chart_input_for_panels_and_popups() {
        let chart = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(500.0, 400.0));
        let panel = egui::Rect::from_min_max(egui::pos2(120.0, 80.0), egui::pos2(360.0, 240.0));

        assert!(floating_panels_chart_click_should_block_chart_input(
            true,
            &[Some(panel)],
            chart,
            true,
            Some(egui::pos2(150.0, 100.0)),
            false,
        ));
        assert!(floating_panels_chart_click_should_block_chart_input(
            true,
            &[Some(panel)],
            chart,
            true,
            Some(egui::pos2(50.0, 50.0)),
            true,
        ));
        assert!(!floating_panels_chart_click_should_block_chart_input(
            true,
            &[Some(panel)],
            chart,
            true,
            Some(egui::pos2(50.0, 50.0)),
            false,
        ));
        assert!(!floating_panels_chart_click_should_block_chart_input(
            false,
            &[Some(panel)],
            chart,
            true,
            Some(egui::pos2(150.0, 100.0)),
            true,
        ));
    }

    #[test]
    fn chart_context_targets_are_region_specific() {
        let legend = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 30.0));
        let plot = egui::Rect::from_min_max(egui::pos2(0.0, 30.0), egui::pos2(320.0, 260.0));
        let panes = vec![crate::chart_core::PaneRect {
            id: PaneId::Main,
            rect: plot,
        }];
        let price_axis =
            egui::Rect::from_min_max(egui::pos2(320.0, 30.0), egui::pos2(400.0, 260.0));
        let time_axis = egui::Rect::from_min_max(egui::pos2(0.0, 260.0), egui::pos2(400.0, 300.0));

        assert_eq!(
            chart_context_target_for_rects(
                legend,
                panes.as_slice(),
                plot,
                price_axis,
                time_axis,
                egui::pos2(100.0, 100.0),
            ),
            ChartContextTarget::Plot
        );
        assert_eq!(
            chart_context_target_for_rects(
                legend,
                panes.as_slice(),
                plot,
                price_axis,
                time_axis,
                egui::pos2(350.0, 100.0),
            ),
            ChartContextTarget::PriceScale
        );
        assert_eq!(
            chart_context_target_for_rects(
                legend,
                panes.as_slice(),
                plot,
                price_axis,
                time_axis,
                egui::pos2(100.0, 280.0),
            ),
            ChartContextTarget::TimeScale
        );
        assert!(chart_context_has_price_scale(
            ChartContextTarget::PriceScale
        ));
        assert!(chart_context_has_time_scale(ChartContextTarget::TimeScale));
    }

    #[test]
    fn chart_price_mode_choices_map_to_engine_modes() {
        assert_eq!(
            chart_price_mode_choice(PriceScaleValueMode::Linear),
            ChartPriceModeChoice::Regular
        );
        assert_eq!(
            chart_price_mode_choice(PriceScaleValueMode::Percent { base: 123.0 }),
            ChartPriceModeChoice::Percent
        );
        assert_eq!(
            chart_price_mode_choice(PriceScaleValueMode::Indexed { base: 123.0 }),
            ChartPriceModeChoice::Indexed
        );
        assert_eq!(
            chart_price_mode_value(ChartPriceModeChoice::Logarithmic),
            PriceScaleValueMode::Log
        );
        assert_eq!(
            chart_price_mode_label(ChartPriceModeChoice::Inverted),
            "Invert scale"
        );
    }

    #[test]
    fn loaded_market_data_reset_preserves_price_scale_mode() {
        let mut chart = TvChartState::default();
        chart
            .price_scales
            .scale_mut(PriceScaleId::Right)
            .set_value_mode(PriceScaleValueMode::Log);
        chart.price_scales.scale_mut(PriceScaleId::Right).zoom_at_y(
            1.25,
            120.0,
            0.0,
            400.0,
            PriceRange {
                min: 10.0,
                max: 20.0,
            },
        );
        assert!(chart.price_scales.scale(PriceScaleId::Right).is_manual());

        reset_chart_for_loaded_market_data(&mut chart, &default_study_instances());

        let right_scale = chart.price_scales.scale(PriceScaleId::Right);
        assert_eq!(right_scale.value_mode(), PriceScaleValueMode::Log);
        assert!(right_scale.is_auto());
    }

    #[test]
    fn study_runtime_follows_visible_study_instances() {
        let enabled_instances = default_study_instances();
        let enabled = build_study_runtime(&enabled_instances);
        assert_eq!(enabled.study_count(), 2);

        let mut disabled_instances = default_study_instances();
        for instance in &mut disabled_instances {
            instance.visible = false;
        }
        let disabled = build_study_runtime(&disabled_instances);
        assert_eq!(disabled.study_count(), 0);
    }

    #[test]
    fn multiple_ema_instances_register_distinct_overlay_series() {
        let first = StudyId::new(8);
        let second = StudyId::new(9);
        let instances = vec![
            StudyInstance::new(
                first,
                StudyConfig::Ema {
                    period: DEFAULT_EMA_PERIOD,
                },
            ),
            StudyInstance::new(second, StudyConfig::Ema { period: 50 }),
        ];
        let runtime = build_study_runtime(&instances);
        let mut chart = TvChartState::default();
        runtime.register_output_series(&mut chart.series);

        let first_series = chart
            .series
            .get(StudyOutputId::new(first, 0).series_id())
            .expect("first EMA output series");
        let second_series = chart
            .series
            .get(StudyOutputId::new(second, 0).series_id())
            .expect("second EMA output series");
        assert_eq!(first_series.pane, PaneId::Main);
        assert_eq!(second_series.pane, PaneId::Main);
        assert_ne!(first_series.id, second_series.id);
    }

    #[test]
    fn study_legend_entries_are_per_placed_instance_and_pane() {
        let first_ema = StudyId::new(20);
        let second_ema = StudyId::new(21);
        let rsi = StudyId::new(22);
        let mut instances = vec![
            StudyInstance::new(
                first_ema,
                StudyConfig::Ema {
                    period: DEFAULT_EMA_PERIOD,
                },
            ),
            StudyInstance::new(second_ema, StudyConfig::Ema { period: 50 }),
            StudyInstance::new(rsi, default_rsi_config()),
        ];
        instances[1].visible = false;

        let entries = study_legend_entries(&instances);

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].id, first_ema);
        assert_eq!(entries[0].pane, PaneId::Main);
        assert_eq!(entries[0].label, "EMA 20 close");
        assert!(entries[0].visible);
        assert_eq!(entries[1].id, second_ema);
        assert_eq!(entries[1].pane, PaneId::Main);
        assert!(!entries[1].visible);
        assert_eq!(entries[2].id, rsi);
        assert_eq!(entries[2].pane, PaneId::Study(rsi.0));
    }

    #[test]
    fn study_legend_actions_target_exact_instance() {
        let first = StudyId::new(23);
        let second = StudyId::new(24);
        let mut instances = vec![
            StudyInstance::new(first, StudyConfig::Ema { period: 20 }),
            StudyInstance::new(second, StudyConfig::Ema { period: 50 }),
        ];

        assert!(apply_study_legend_action(
            &mut instances,
            StudyLegendAction::ToggleVisibility(second),
        ));
        assert!(instances[0].visible);
        assert!(!instances[1].visible);

        assert!(apply_study_legend_action(
            &mut instances,
            StudyLegendAction::Delete(first),
        ));
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].id, second);
    }

    #[test]
    fn study_settings_update_targets_exact_instance() {
        let first = StudyId::new(25);
        let second = StudyId::new(26);
        let mut instances = vec![
            StudyInstance::new(
                first,
                StudyConfig::Rsi {
                    period: 14,
                    upper_band: 70,
                    middle_band: 50,
                    lower_band: 30,
                },
            ),
            StudyInstance::new(
                second,
                StudyConfig::Rsi {
                    period: 14,
                    upper_band: 70,
                    middle_band: 50,
                    lower_band: 30,
                },
            ),
        ];
        let updated = StudyInstance::new(
            second,
            StudyConfig::Rsi {
                period: 21,
                upper_band: 80,
                middle_band: 55,
                lower_band: 25,
            },
        );

        assert!(apply_study_settings_update(&mut instances, updated));

        assert_eq!(
            instances[0].config,
            StudyConfig::Rsi {
                period: 14,
                upper_band: 70,
                middle_band: 50,
                lower_band: 30,
            }
        );
        assert_eq!(
            instances[1].config,
            StudyConfig::Rsi {
                period: 21,
                upper_band: 80,
                middle_band: 55,
                lower_band: 25,
            }
        );
    }

    #[test]
    fn removing_settings_instance_only_removes_that_id() {
        let first = StudyId::new(27);
        let second = StudyId::new(28);
        let mut instances = vec![
            StudyInstance::new(first, StudyConfig::Ema { period: 20 }),
            StudyInstance::new(second, StudyConfig::Ema { period: 50 }),
        ];

        assert!(remove_study_instance(&mut instances, first));
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].id, second);
        assert!(!remove_study_instance(&mut instances, first));
    }

    #[test]
    fn study_legend_rows_stack_inside_each_pane() {
        let mut counts = Vec::new();
        assert_eq!(next_study_legend_row_index(&mut counts, PaneId::Main), 0);
        assert_eq!(next_study_legend_row_index(&mut counts, PaneId::Main), 1);
        assert_eq!(
            next_study_legend_row_index(&mut counts, PaneId::Study(9)),
            0
        );

        let pane = egui::Rect::from_min_size(egui::pos2(100.0, 200.0), egui::vec2(500.0, 180.0));
        let first = study_legend_row_position(pane, 0);
        let second = study_legend_row_position(pane, 1);

        assert_eq!(first.x, 108.0);
        assert_eq!(first.y, 204.0);
        assert!(second.y > first.y);
    }

    #[test]
    fn hidden_study_instances_keep_their_pane_for_toggle_back() {
        let rsi_id = StudyId::new(7);
        let mut instances = vec![StudyInstance::new(rsi_id, default_rsi_config())];
        let mut chart = TvChartState::default();
        apply_indicator_layout(&mut chart, &instances);
        assert!(chart.panes.is_visible(PaneId::Study(rsi_id.0)));

        instances[0].visible = false;
        apply_indicator_layout(&mut chart, &instances);
        assert!(chart.panes.is_visible(PaneId::Study(rsi_id.0)));

        instances.clear();
        apply_indicator_layout(&mut chart, &instances);
        assert!(!chart.panes.is_visible(PaneId::Study(rsi_id.0)));
    }

    #[test]
    fn macd_instance_registers_study_pane_outputs() {
        let macd_id = StudyId::new(10);
        let instances = vec![StudyInstance::new(
            macd_id,
            StudyConfig::Macd {
                fast_period: 12,
                slow_period: 26,
                signal_period: 9,
            },
        )];
        let runtime = build_study_runtime(&instances);
        let mut chart = TvChartState::default();
        apply_indicator_layout(&mut chart, &instances);
        runtime.register_output_series(&mut chart.series);

        let macd_line = chart
            .series
            .get(StudyOutputId::new(macd_id, 0).series_id())
            .expect("MACD line series");
        let signal_line = chart
            .series
            .get(StudyOutputId::new(macd_id, 1).series_id())
            .expect("MACD signal series");
        let histogram = chart
            .series
            .get(StudyOutputId::new(macd_id, 2).series_id())
            .expect("MACD histogram series");

        assert_eq!(macd_line.pane, PaneId::Study(macd_id.0));
        assert_eq!(signal_line.pane, PaneId::Study(macd_id.0));
        assert_eq!(histogram.pane, PaneId::Study(macd_id.0));
        assert_eq!(histogram.kind, SeriesKind::Histogram);
        assert!(chart.panes.is_visible(PaneId::Study(macd_id.0)));
    }

    #[test]
    fn study_instance_inputs_are_normalized() {
        let mut instances = vec![
            StudyInstance::new(StudyId::new(10), StudyConfig::Ema { period: 20 }),
            StudyInstance::new(
                StudyId::new(11),
                StudyConfig::Macd {
                    fast_period: 50,
                    slow_period: 12,
                    signal_period: 0,
                },
            ),
            StudyInstance::new(
                StudyId::new(12),
                StudyConfig::Rsi {
                    period: 0,
                    upper_band: 20,
                    middle_band: 90,
                    lower_band: 80,
                },
            ),
        ];
        if let StudyInputConfig::Ema { offset, .. } = &mut instances[0].inputs {
            *offset = MAX_STUDY_OFFSET + 10;
        }
        if let StudyInputConfig::Rsi { smoothing, .. } = &mut instances[2].inputs {
            smoothing.length = 0;
        }
        instances[0].visibility.hours.min = 10;
        instances[0].visibility.hours.max = 2;

        normalize_study_instances(&mut instances);

        if let StudyInputConfig::Ema { offset, .. } = &instances[0].inputs {
            assert_eq!(*offset, MAX_STUDY_OFFSET);
        } else {
            panic!("expected EMA inputs");
        }
        assert_eq!(instances[0].visibility.hours.min, 10);
        assert_eq!(instances[0].visibility.hours.max, 10);
        assert_eq!(
            instances[1].config,
            StudyConfig::Macd {
                fast_period: 50,
                slow_period: 50,
                signal_period: 1,
            }
        );
        assert_eq!(
            instances[2].config,
            StudyConfig::Rsi {
                period: 1,
                upper_band: 80,
                middle_band: 80,
                lower_band: 20,
            }
        );
        if let StudyInputConfig::Rsi { smoothing, .. } = &instances[2].inputs {
            assert_eq!(smoothing.length, 1);
        } else {
            panic!("expected RSI inputs");
        }
    }

    #[test]
    fn rsi_instance_guides_are_instance_specific() {
        let rsi_id = StudyId::new(13);
        let instances = vec![StudyInstance::new(
            rsi_id,
            StudyConfig::Rsi {
                period: DEFAULT_RSI_PERIOD,
                upper_band: 80,
                middle_band: 45,
                lower_band: 20,
            },
        )];
        let runtime = build_study_runtime(&instances);
        let source = AppStudyDataSource {
            runtime: &runtime,
            instances: &instances,
            active_interval: DEFAULT_INTERVAL,
        };

        assert_eq!(
            source.pane_guides(PaneId::Study(rsi_id.0)),
            vec![20.0, 45.0, 80.0]
        );
    }

    #[test]
    fn study_output_styles_are_instance_specific() {
        let first = StudyId::new(14);
        let second = StudyId::new(15);
        let mut instances = vec![
            StudyInstance::new(first, StudyConfig::Ema { period: 20 }),
            StudyInstance::new(second, StudyConfig::Ema { period: 50 }),
        ];
        if let StudyStyleConfig::Ema { line } = &mut instances[1].style {
            line.color = StudyColor::rgb(12, 34, 56);
            line.width = 3.0;
        }
        let runtime = build_study_runtime(&instances);
        let source = AppStudyDataSource {
            runtime: &runtime,
            instances: &instances,
            active_interval: DEFAULT_INTERVAL,
        };

        let first_style = source
            .output_style(StudyOutputId::new(first, 0).series_id())
            .expect("first style");
        let second_style = source
            .output_style(StudyOutputId::new(second, 0).series_id())
            .expect("second style");

        assert_ne!(first_style.color, second_style.color);
        assert_eq!(second_style.color, StudyColor::rgb(12, 34, 56));
        assert_eq!(second_style.width, 3.0);
    }

    #[test]
    fn study_visibility_filters_rendering_without_removing_runtime() {
        let study_id = StudyId::new(21);
        let mut instance = StudyInstance::new(study_id, StudyConfig::Ema { period: 20 });
        instance.visibility.minutes.enabled = false;
        let instances = vec![instance];
        let runtime = build_study_runtime(&instances);
        let minute_source = AppStudyDataSource {
            runtime: &runtime,
            instances: &instances,
            active_interval: DEFAULT_INTERVAL,
        };
        let daily_source = AppStudyDataSource {
            runtime: &runtime,
            instances: &instances,
            active_interval: TIMEFRAMES[6],
        };

        assert_eq!(runtime.study_count(), 1);
        assert!(
            !minute_source
                .output_style(StudyOutputId::new(study_id, 0).series_id())
                .expect("minute style")
                .visible
        );
        assert!(minute_source.pane_label(PaneId::Main).is_none());
        assert!(
            daily_source
                .output_style(StudyOutputId::new(study_id, 0).series_id())
                .expect("daily style")
                .visible
        );
    }

    #[test]
    fn study_price_source_drives_runtime_values() {
        let study_id = StudyId::new(18);
        let mut instance = StudyInstance::new(study_id, StudyConfig::Sma { period: 2 });
        if let StudyInputConfig::Sma { source, .. } = &mut instance.inputs {
            *source = StudyPriceSource::High;
        }
        let bars = vec![
            OhlcBar {
                time: 1,
                open: 1.0,
                high: 10.0,
                low: 0.5,
                close: 2.0,
                volume: 1.0,
            },
            OhlcBar {
                time: 2,
                open: 2.0,
                high: 14.0,
                low: 1.5,
                close: 3.0,
                volume: 1.0,
            },
        ];
        let mut runtime = build_study_runtime(&[instance]);

        runtime.replay(&bars);
        let values = runtime
            .output_values(StudyOutputId::new(study_id, 0).series_id())
            .expect("SMA values");

        assert_eq!(values[0], StudyValue::Null);
        assert_eq!(values[1], StudyValue::Number(12.0));
    }

    #[test]
    fn study_output_offsets_are_instance_specific_inputs() {
        let study_id = StudyId::new(19);
        let mut instance = StudyInstance::new(study_id, StudyConfig::Ema { period: 20 });
        if let StudyInputConfig::Ema { offset, .. } = &mut instance.inputs {
            *offset = 7;
        }
        let instances = vec![instance];
        let runtime = build_study_runtime(&instances);
        let source = AppStudyDataSource {
            runtime: &runtime,
            instances: &instances,
            active_interval: DEFAULT_INTERVAL,
        };

        assert_eq!(
            source.output_offset(StudyOutputId::new(study_id, 0).series_id()),
            7
        );
    }

    #[test]
    fn rsi_smoothing_registers_secondary_output() {
        let rsi_id = StudyId::new(20);
        let mut instance = StudyInstance::new(rsi_id, default_rsi_config());
        if let StudyInputConfig::Rsi { smoothing, .. } = &mut instance.inputs {
            smoothing.kind = StudySmoothingKind::Ema;
            smoothing.length = 3;
        }
        let instances = vec![instance];
        let runtime = build_study_runtime(&instances);
        let mut chart = TvChartState::default();
        runtime.register_output_series(&mut chart.series);

        let smoothing = chart
            .series
            .get(StudyOutputId::new(rsi_id, 1).series_id())
            .expect("RSI smoothing output");

        assert_eq!(smoothing.name, "RSI-based MA");
        assert_eq!(smoothing.pane, PaneId::Study(rsi_id.0));
        assert_eq!(smoothing.kind, SeriesKind::Line);
    }

    #[test]
    fn rsi_guide_styles_follow_instance_style() {
        let rsi_id = StudyId::new(16);
        let mut instances = vec![StudyInstance::new(
            rsi_id,
            StudyConfig::Rsi {
                period: DEFAULT_RSI_PERIOD,
                upper_band: 80,
                middle_band: 45,
                lower_band: 20,
            },
        )];
        if let StudyStyleConfig::Rsi {
            upper_band,
            middle_band,
            lower_band,
            ..
        } = &mut instances[0].style
        {
            upper_band.color = StudyColor::rgb(200, 201, 202);
            middle_band.visible = false;
            lower_band.width = 2.5;
        }
        let runtime = build_study_runtime(&instances);
        let source = AppStudyDataSource {
            runtime: &runtime,
            instances: &instances,
            active_interval: DEFAULT_INTERVAL,
        };

        let guides = source.pane_guide_styles(PaneId::Study(rsi_id.0));

        assert_eq!(guides.len(), 3);
        assert_eq!(guides[0].level, 20.0);
        assert_eq!(guides[0].width, 2.5);
        assert_eq!(guides[1].level, 45.0);
        assert!(!guides[1].visible);
        assert_eq!(guides[2].level, 80.0);
        assert_eq!(guides[2].color, StudyColor::rgb(200, 201, 202));
    }

    #[test]
    fn study_style_widths_are_normalized() {
        let mut instances = vec![StudyInstance::new(
            StudyId::new(17),
            StudyConfig::Ema { period: 20 },
        )];
        if let StudyStyleConfig::Ema { line } = &mut instances[0].style {
            line.width = 99.0;
        }

        normalize_study_instances(&mut instances);

        if let StudyStyleConfig::Ema { line } = &instances[0].style {
            assert_eq!(line.width, MAX_STUDY_STYLE_WIDTH);
        } else {
            panic!("expected EMA style");
        }
    }

    #[test]
    fn study_period_settings_are_clamped() {
        assert_eq!(clamp_study_period(0), 1);
        assert_eq!(clamp_study_period(MAX_STUDY_PERIOD + 1), MAX_STUDY_PERIOD);
    }

    #[test]
    fn live_chart_header_reserves_non_overlapping_host_controls() {
        let inner_rect = egui::Rect::from_min_max(egui::pos2(10.0, 0.0), egui::pos2(420.0, 34.0));
        let host_width =
            live_chart_host_controls_width(WidgetHostMode::InShellWorkspaceSurface, 1.0);
        let (content_rect, host_rect) =
            live_chart_header_control_rects(inner_rect, host_width, 1.0);

        assert_eq!(host_rect.right(), inner_rect.right());
        assert!((host_rect.width() - host_width).abs() < 0.01);
        assert!(content_rect.right() <= host_rect.left());
    }

    #[test]
    fn detached_live_chart_header_reserves_extra_host_control_width() {
        let in_shell_width =
            live_chart_host_controls_width(WidgetHostMode::InShellWorkspaceSurface, 1.0);
        let detached_width =
            live_chart_host_controls_width(WidgetHostMode::DetachedNativeViewport, 1.0);

        assert!(detached_width > in_shell_width);
    }

    #[test]
    fn chart_widget_density_uses_contract_thresholds() {
        assert_eq!(
            chart_widget_density(egui::vec2(720.0, 420.0)),
            ChartWidgetDensity::Full
        );
        assert_eq!(
            chart_widget_density(egui::vec2(600.0, 420.0)),
            ChartWidgetDensity::Compact
        );
        assert_eq!(
            chart_widget_density(egui::vec2(480.0, 250.0)),
            ChartWidgetDensity::Mini
        );
        assert_eq!(
            chart_widget_density(egui::vec2(240.0, 180.0)),
            ChartWidgetDensity::Tiny
        );
    }
}

fn main() -> eframe::Result {
    let args = env::args().skip(1).collect::<Vec<_>>();
    match parse_derived_cache_cli_options(&args) {
        Ok(Some(options)) => {
            if let Err(error) = run_derived_cache_cli(options) {
                eprintln!("{error}");
                std::process::exit(2);
            }
            return Ok(());
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    }

    let launch_options = match parse_app_launch_options(&args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };

    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1920.0, 1080.0])
        .with_min_inner_size([1180.0, 760.0])
        .with_maximized(true)
        .with_title("rustchart");

    eframe::run_native(
        "rustchart",
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(|cc| Ok(Box::new(ChartOnlyApp::new(cc, launch_options)))),
    )
}

fn sample_bars() -> Vec<OhlcBar> {
    let mut rng = Lcg::new(0x8fd2_4f71_3c81_2bb5);
    let mut bars = Vec::with_capacity(720);
    let mut close = 18_240.0;
    let start = 1_714_620_000_i64;

    for index in 0..720 {
        let drift = (index as f64 / 34.0).sin() * 6.5 + (index as f64 / 89.0).cos() * 4.0;
        let impulse = (rng.next_f64() - 0.5) * 18.0;
        let open = close;
        close = (close + drift * 0.18 + impulse).max(100.0);
        let range_high = rng.next_f64() * 16.0 + 4.0;
        let range_low = rng.next_f64() * 14.0 + 4.0;
        let high = open.max(close) + range_high;
        let low = open.min(close) - range_low;
        let volume = 350.0 + rng.next_f64() * 1_500.0 + drift.abs() * 55.0;

        bars.push(OhlcBar {
            time: start + index as i64 * 60,
            open,
            high,
            low,
            close,
            volume,
        });
    }

    bars
}

struct Lcg {
    state: u64,
}

impl Lcg {
    const fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_f64(&mut self) -> f64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let value = self.state >> 11;
        value as f64 / ((1_u64 << 53) as f64)
    }
}
