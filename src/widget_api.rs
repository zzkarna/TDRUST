use eframe::egui;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WidgetId(u64);

impl WidgetId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for WidgetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WidgetKind {
    LiveChart,
    SyncedTimeframes,
    Watchlist,
    OrderTicket,
    News,
    OrderBook,
    LiquidityHeatmap,
    Footprint,
    BookmapOrderFlow,
    NanosecondZoom,
    RecordReplay,
    DeveloperApi,
    MarketDataConnections,
    StopsIcebergs,
    TradermapPro,
    DomPro,
    FilteredFlowMap,
    HeuristicOrderFlowEvents,
    MarketPulse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WidgetCategory {
    Chart,
    Market,
    Execution,
    Research,
}

impl WidgetCategory {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Chart => "Chart",
            Self::Market => "Market",
            Self::Execution => "Execution",
            Self::Research => "Research",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WidgetCatalogEntry {
    pub kind: WidgetKind,
    pub category: WidgetCategory,
}

impl WidgetCatalogEntry {
    pub const fn new(kind: WidgetKind, category: WidgetCategory) -> Self {
        Self { kind, category }
    }

    pub fn matches_query(self, input: &str) -> bool {
        let query = input.trim().to_ascii_lowercase();
        query.is_empty()
            || self.kind.mnemonic().to_ascii_lowercase().contains(&query)
            || self.kind.label().to_ascii_lowercase().contains(&query)
            || self.kind.stable_key().contains(&query)
            || self
                .kind
                .description()
                .to_ascii_lowercase()
                .contains(&query)
            || self.category.label().to_ascii_lowercase().contains(&query)
    }
}

impl WidgetKind {
    pub const fn stable_key(self) -> &'static str {
        match self {
            Self::LiveChart => "live-chart",
            Self::SyncedTimeframes => "synced-timeframes",
            Self::Watchlist => "watchlist",
            Self::OrderTicket => "order-ticket",
            Self::News => "news",
            Self::OrderBook => "order-book",
            Self::LiquidityHeatmap => "liquidity-heatmap",
            Self::Footprint => "footprint",
            Self::BookmapOrderFlow => "bookmap-order-flow",
            Self::NanosecondZoom => "nanosecond-zoom",
            Self::RecordReplay => "record-replay",
            Self::DeveloperApi => "developer-api",
            Self::MarketDataConnections => "market-data-connections",
            Self::StopsIcebergs => "stops-icebergs",
            Self::TradermapPro => "tradermap-pro",
            Self::DomPro => "dom-pro",
            Self::FilteredFlowMap => "filtered-flow-map",
            Self::HeuristicOrderFlowEvents => "heuristic-order-flow-events",
            Self::MarketPulse => "market-pulse",
        }
    }

    pub fn from_stable_key(value: &str) -> Option<Self> {
        match value {
            "live-chart" => Some(Self::LiveChart),
            "synced-timeframes" => Some(Self::SyncedTimeframes),
            "watchlist" => Some(Self::Watchlist),
            "order-ticket" => Some(Self::OrderTicket),
            "news" => Some(Self::News),
            "order-book" => Some(Self::OrderBook),
            "liquidity-heatmap" => Some(Self::LiquidityHeatmap),
            "footprint" => Some(Self::Footprint),
            "bookmap-order-flow" => Some(Self::BookmapOrderFlow),
            "nanosecond-zoom" => Some(Self::NanosecondZoom),
            "record-replay" => Some(Self::RecordReplay),
            "developer-api" => Some(Self::DeveloperApi),
            "market-data-connections" => Some(Self::MarketDataConnections),
            "stops-icebergs" => Some(Self::StopsIcebergs),
            "tradermap-pro" => Some(Self::TradermapPro),
            "dom-pro" => Some(Self::DomPro),
            "filtered-flow-map" => Some(Self::FilteredFlowMap),
            "heuristic-order-flow-events" => Some(Self::HeuristicOrderFlowEvents),
            "market-pulse" => Some(Self::MarketPulse),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::LiveChart => "Live Chart",
            Self::SyncedTimeframes => "Synced Timeframes",
            Self::Watchlist => "Watchlist",
            Self::OrderTicket => "Order Ticket",
            Self::News => "News",
            Self::OrderBook => "Order Book",
            Self::LiquidityHeatmap => "Liquidity Heatmap",
            Self::Footprint => "Footprint",
            Self::BookmapOrderFlow => "Order Flow Map",
            Self::NanosecondZoom => "Nanosecond Zoom",
            Self::RecordReplay => "Record & Replay",
            Self::DeveloperApi => "Developer API",
            Self::MarketDataConnections => "Market Data Connections",
            Self::StopsIcebergs => "Stops & Icebergs",
            Self::TradermapPro => "Tradermap Pro",
            Self::DomPro => "DOM Pro",
            Self::FilteredFlowMap => "Filtered Flow Map",
            Self::HeuristicOrderFlowEvents => "Heuristic Events",
            Self::MarketPulse => "Market Pulse",
        }
    }

    pub const fn mnemonic(self) -> &'static str {
        match self {
            Self::LiveChart => "CH",
            Self::SyncedTimeframes => "TF",
            Self::Watchlist => "WL",
            Self::OrderTicket => "OT",
            Self::News => "NW",
            Self::OrderBook => "OB",
            Self::LiquidityHeatmap => "HM",
            Self::Footprint => "FP",
            Self::BookmapOrderFlow => "OF",
            Self::NanosecondZoom => "NZ",
            Self::RecordReplay => "RR",
            Self::DeveloperApi => "API",
            Self::MarketDataConnections => "MD",
            Self::StopsIcebergs => "SI",
            Self::TradermapPro => "TM",
            Self::DomPro => "DP",
            Self::FilteredFlowMap => "FM",
            Self::HeuristicOrderFlowEvents => "HE",
            Self::MarketPulse => "MP",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::LiveChart => "Launch another RustChart widget",
            Self::SyncedTimeframes => "Open multi-timeframe chart overview",
            Self::Watchlist => "Open reserved watchlist surface",
            Self::OrderTicket => "Open reserved order ticket surface",
            Self::News => "Open reserved news surface",
            Self::OrderBook => "Open diagnostic historical MBO order book layer view",
            Self::LiquidityHeatmap => "Open diagnostic historical MBO liquidity heatmap layer",
            Self::Footprint => "Open diagnostic historical MBO footprint layer view",
            Self::BookmapOrderFlow => "Open integrated Bookmap-style historical MBO terminal",
            Self::NanosecondZoom => "Open diagnostic event-level MBO zoom layer view",
            Self::RecordReplay => "Open diagnostic historical MBO replay timeline view",
            Self::DeveloperApi => "Open diagnostic RustChart order-flow event contract surface",
            Self::MarketDataConnections => {
                "Open diagnostic market-data capability and Databento status surface"
            }
            Self::StopsIcebergs => "Open diagnostic MBO stops and icebergs heuristic layer",
            Self::TradermapPro => "Open diagnostic MBO filtered Tradermap-style layer",
            Self::DomPro => "Open diagnostic dense DOM Pro-style ladder panel",
            Self::FilteredFlowMap => "Open diagnostic filtered synthetic MBO flow layer",
            Self::HeuristicOrderFlowEvents => {
                "Open diagnostic MBO heuristic stops and icebergs-style events"
            }
            Self::MarketPulse => "Open diagnostic historical MBO market pulse panel",
        }
    }

    pub const fn is_placeholder(self) -> bool {
        match self {
            Self::LiveChart
            | Self::SyncedTimeframes
            | Self::OrderBook
            | Self::LiquidityHeatmap
            | Self::Footprint
            | Self::BookmapOrderFlow
            | Self::NanosecondZoom
            | Self::RecordReplay
            | Self::DeveloperApi
            | Self::MarketDataConnections
            | Self::StopsIcebergs
            | Self::TradermapPro
            | Self::DomPro
            | Self::FilteredFlowMap
            | Self::HeuristicOrderFlowEvents
            | Self::MarketPulse => false,
            Self::Watchlist | Self::OrderTicket | Self::News => true,
        }
    }

    pub fn default_host_config(self, default_pos: egui::Pos2) -> WidgetHostConfig {
        self.host_config(default_pos, WidgetHostMode::InShellWorkspaceSurface)
    }

    fn host_config(self, default_pos: egui::Pos2, mode: WidgetHostMode) -> WidgetHostConfig {
        match self {
            Self::LiveChart => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(760.0, 440.0),
                min_size: egui::vec2(120.0, 96.0),
                in_shell_min_size: egui::vec2(280.0, 190.0),
            },
            Self::SyncedTimeframes => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(1_160.0, 520.0),
                min_size: egui::vec2(360.0, 260.0),
                in_shell_min_size: egui::vec2(360.0, 260.0),
            },
            Self::Watchlist => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(360.0, 520.0),
                min_size: egui::vec2(260.0, 220.0),
                in_shell_min_size: egui::vec2(260.0, 220.0),
            },
            Self::OrderTicket => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(360.0, 500.0),
                min_size: egui::vec2(300.0, 260.0),
                in_shell_min_size: egui::vec2(300.0, 260.0),
            },
            Self::News => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(520.0, 420.0),
                min_size: egui::vec2(320.0, 220.0),
                in_shell_min_size: egui::vec2(320.0, 220.0),
            },
            Self::OrderBook => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(420.0, 560.0),
                min_size: egui::vec2(320.0, 280.0),
                in_shell_min_size: egui::vec2(320.0, 280.0),
            },
            Self::LiquidityHeatmap => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(760.0, 520.0),
                min_size: egui::vec2(420.0, 300.0),
                in_shell_min_size: egui::vec2(420.0, 300.0),
            },
            Self::Footprint => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(680.0, 520.0),
                min_size: egui::vec2(420.0, 300.0),
                in_shell_min_size: egui::vec2(420.0, 300.0),
            },
            Self::BookmapOrderFlow => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(1_840.0, 1_020.0),
                min_size: egui::vec2(960.0, 640.0),
                in_shell_min_size: egui::vec2(960.0, 640.0),
            },
            Self::NanosecondZoom => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(1_080.0, 520.0),
                min_size: egui::vec2(600.0, 340.0),
                in_shell_min_size: egui::vec2(600.0, 340.0),
            },
            Self::RecordReplay => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(820.0, 460.0),
                min_size: egui::vec2(500.0, 300.0),
                in_shell_min_size: egui::vec2(500.0, 300.0),
            },
            Self::DeveloperApi => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(720.0, 520.0),
                min_size: egui::vec2(440.0, 300.0),
                in_shell_min_size: egui::vec2(440.0, 300.0),
            },
            Self::MarketDataConnections => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(760.0, 500.0),
                min_size: egui::vec2(460.0, 300.0),
                in_shell_min_size: egui::vec2(460.0, 300.0),
            },
            Self::StopsIcebergs => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(920.0, 560.0),
                min_size: egui::vec2(520.0, 340.0),
                in_shell_min_size: egui::vec2(520.0, 340.0),
            },
            Self::TradermapPro => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(1_140.0, 620.0),
                min_size: egui::vec2(660.0, 360.0),
                in_shell_min_size: egui::vec2(660.0, 360.0),
            },
            Self::DomPro => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(760.0, 680.0),
                min_size: egui::vec2(460.0, 380.0),
                in_shell_min_size: egui::vec2(460.0, 380.0),
            },
            Self::FilteredFlowMap => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(760.0, 540.0),
                min_size: egui::vec2(440.0, 320.0),
                in_shell_min_size: egui::vec2(440.0, 320.0),
            },
            Self::HeuristicOrderFlowEvents => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(520.0, 500.0),
                min_size: egui::vec2(360.0, 280.0),
                in_shell_min_size: egui::vec2(360.0, 280.0),
            },
            Self::MarketPulse => WidgetHostConfig {
                mode,
                default_pos,
                default_size: egui::vec2(460.0, 420.0),
                min_size: egui::vec2(340.0, 260.0),
                in_shell_min_size: egui::vec2(340.0, 260.0),
            },
        }
    }
}

pub const WIDGET_CATALOG: [WidgetCatalogEntry; 19] = [
    WidgetCatalogEntry::new(WidgetKind::LiveChart, WidgetCategory::Chart),
    WidgetCatalogEntry::new(WidgetKind::SyncedTimeframes, WidgetCategory::Chart),
    WidgetCatalogEntry::new(WidgetKind::Watchlist, WidgetCategory::Market),
    WidgetCatalogEntry::new(WidgetKind::OrderTicket, WidgetCategory::Execution),
    WidgetCatalogEntry::new(WidgetKind::News, WidgetCategory::Research),
    WidgetCatalogEntry::new(WidgetKind::OrderBook, WidgetCategory::Market),
    WidgetCatalogEntry::new(WidgetKind::LiquidityHeatmap, WidgetCategory::Market),
    WidgetCatalogEntry::new(WidgetKind::Footprint, WidgetCategory::Market),
    WidgetCatalogEntry::new(WidgetKind::BookmapOrderFlow, WidgetCategory::Market),
    WidgetCatalogEntry::new(WidgetKind::NanosecondZoom, WidgetCategory::Market),
    WidgetCatalogEntry::new(WidgetKind::RecordReplay, WidgetCategory::Market),
    WidgetCatalogEntry::new(WidgetKind::DeveloperApi, WidgetCategory::Research),
    WidgetCatalogEntry::new(WidgetKind::MarketDataConnections, WidgetCategory::Market),
    WidgetCatalogEntry::new(WidgetKind::StopsIcebergs, WidgetCategory::Market),
    WidgetCatalogEntry::new(WidgetKind::TradermapPro, WidgetCategory::Market),
    WidgetCatalogEntry::new(WidgetKind::DomPro, WidgetCategory::Execution),
    WidgetCatalogEntry::new(WidgetKind::FilteredFlowMap, WidgetCategory::Market),
    WidgetCatalogEntry::new(WidgetKind::HeuristicOrderFlowEvents, WidgetCategory::Market),
    WidgetCatalogEntry::new(WidgetKind::MarketPulse, WidgetCategory::Market),
];

pub fn widget_catalog() -> &'static [WidgetCatalogEntry] {
    &WIDGET_CATALOG
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetHostMode {
    InShellWorkspaceSurface,
    DetachedNativeViewport,
}

impl WidgetHostMode {
    pub const fn stable_key(self) -> &'static str {
        match self {
            Self::InShellWorkspaceSurface => "in-shell",
            Self::DetachedNativeViewport => "detached-native-viewport",
        }
    }

    pub fn from_stable_key(value: &str) -> Option<Self> {
        match value {
            "in-shell" => Some(Self::InShellWorkspaceSurface),
            "detached-native-viewport" => Some(Self::DetachedNativeViewport),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::InShellWorkspaceSurface => "in-shell workspace surface",
            Self::DetachedNativeViewport => "detached native viewport",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetHostClass {
    NativeViewport,
    InShellWorkspaceSurface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetLifecycle {
    Open,
    Minimized,
    Closed,
}

impl WidgetLifecycle {
    pub const fn stable_key(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Minimized => "minimized",
            Self::Closed => "closed",
        }
    }

    pub fn from_stable_key(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "minimized" => Some(Self::Minimized),
            "closed" => Some(Self::Closed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WidgetDescriptor {
    pub id: WidgetId,
    pub kind: WidgetKind,
}

impl WidgetDescriptor {
    pub const fn new(id: WidgetId, kind: WidgetKind) -> Self {
        Self { id, kind }
    }

    pub fn title(self) -> String {
        format!("{} {}", self.kind.label(), self.id)
    }

    pub fn viewport_id(self) -> egui::ViewportId {
        egui::ViewportId::from_hash_of((
            "workspace_widget_viewport",
            self.kind.stable_key(),
            self.id.get(),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WidgetHostConfig {
    pub mode: WidgetHostMode,
    pub default_pos: egui::Pos2,
    pub default_size: egui::Vec2,
    pub min_size: egui::Vec2,
    pub in_shell_min_size: egui::Vec2,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WidgetSnapshot {
    pub descriptor: WidgetDescriptor,
    pub lifecycle: WidgetLifecycle,
    pub host: WidgetHostConfig,
    pub host_class: Option<WidgetHostClass>,
    pub z_order: u64,
}

impl WidgetSnapshot {
    pub const fn id(self) -> WidgetId {
        self.descriptor.id
    }

    pub const fn kind(self) -> WidgetKind {
        self.descriptor.kind
    }

    pub fn title(self) -> String {
        self.descriptor.title()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WidgetRenderStatus {
    pub id: WidgetId,
    pub lifecycle: WidgetLifecycle,
    pub host_class: WidgetHostClass,
    pub focused: bool,
}

impl WidgetRenderStatus {
    pub const fn open(id: WidgetId, host_class: WidgetHostClass, focused: bool) -> Self {
        Self {
            id,
            lifecycle: WidgetLifecycle::Open,
            host_class,
            focused,
        }
    }

    pub const fn closed(id: WidgetId, host_class: WidgetHostClass) -> Self {
        Self {
            id,
            lifecycle: WidgetLifecycle::Closed,
            host_class,
            focused: false,
        }
    }

    pub const fn is_open(self) -> bool {
        matches!(self.lifecycle, WidgetLifecycle::Open)
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui;
    use std::collections::BTreeSet;

    use super::{WidgetDescriptor, WidgetHostMode, WidgetId, WidgetKind, widget_catalog};

    #[test]
    fn live_chart_widget_metadata_is_stable() {
        let kind = WidgetKind::LiveChart;
        assert_eq!(kind.stable_key(), "live-chart");
        assert_eq!(kind.label(), "Live Chart");
        assert_eq!(kind.mnemonic(), "CH");
        assert!(kind.description().contains("RustChart"));
        assert!(!kind.is_placeholder());
    }

    #[test]
    fn synced_timeframes_widget_metadata_is_stable() {
        let kind = WidgetKind::SyncedTimeframes;
        assert_eq!(kind.stable_key(), "synced-timeframes");
        assert_eq!(kind.label(), "Synced Timeframes");
        assert_eq!(kind.mnemonic(), "TF");
        assert!(kind.description().contains("timeframe"));
        assert!(!kind.is_placeholder());
    }

    #[test]
    fn order_flow_widget_metadata_is_stable() {
        for (kind, key, label, mnemonic) in [
            (WidgetKind::OrderBook, "order-book", "Order Book", "OB"),
            (
                WidgetKind::LiquidityHeatmap,
                "liquidity-heatmap",
                "Liquidity Heatmap",
                "HM",
            ),
            (WidgetKind::Footprint, "footprint", "Footprint", "FP"),
            (
                WidgetKind::BookmapOrderFlow,
                "bookmap-order-flow",
                "Order Flow Map",
                "OF",
            ),
            (
                WidgetKind::NanosecondZoom,
                "nanosecond-zoom",
                "Nanosecond Zoom",
                "NZ",
            ),
            (
                WidgetKind::RecordReplay,
                "record-replay",
                "Record & Replay",
                "RR",
            ),
            (
                WidgetKind::DeveloperApi,
                "developer-api",
                "Developer API",
                "API",
            ),
            (
                WidgetKind::MarketDataConnections,
                "market-data-connections",
                "Market Data Connections",
                "MD",
            ),
            (
                WidgetKind::StopsIcebergs,
                "stops-icebergs",
                "Stops & Icebergs",
                "SI",
            ),
            (
                WidgetKind::TradermapPro,
                "tradermap-pro",
                "Tradermap Pro",
                "TM",
            ),
            (WidgetKind::DomPro, "dom-pro", "DOM Pro", "DP"),
            (
                WidgetKind::FilteredFlowMap,
                "filtered-flow-map",
                "Filtered Flow Map",
                "FM",
            ),
            (
                WidgetKind::HeuristicOrderFlowEvents,
                "heuristic-order-flow-events",
                "Heuristic Events",
                "HE",
            ),
            (
                WidgetKind::MarketPulse,
                "market-pulse",
                "Market Pulse",
                "MP",
            ),
        ] {
            assert_eq!(kind.stable_key(), key);
            assert_eq!(WidgetKind::from_stable_key(key), Some(kind));
            assert_eq!(kind.label(), label);
            assert_eq!(kind.mnemonic(), mnemonic);
            assert!(!kind.is_placeholder());
        }
    }

    #[test]
    fn order_flow_catalog_marks_layers_as_diagnostics() {
        assert!(
            WidgetKind::BookmapOrderFlow
                .description()
                .contains("terminal")
        );
        for kind in [
            WidgetKind::OrderBook,
            WidgetKind::LiquidityHeatmap,
            WidgetKind::Footprint,
            WidgetKind::NanosecondZoom,
            WidgetKind::RecordReplay,
            WidgetKind::StopsIcebergs,
            WidgetKind::TradermapPro,
            WidgetKind::DomPro,
            WidgetKind::FilteredFlowMap,
            WidgetKind::HeuristicOrderFlowEvents,
            WidgetKind::MarketPulse,
        ] {
            assert!(
                kind.description().contains("diagnostic"),
                "{} should be cataloged as diagnostic",
                kind.stable_key()
            );
        }
    }

    #[test]
    fn widget_descriptor_titles_are_human_readable() {
        assert_eq!(
            WidgetDescriptor::new(WidgetId::new(42), WidgetKind::LiveChart).title(),
            "Live Chart 42"
        );
        assert_eq!(
            WidgetDescriptor::new(WidgetId::new(9), WidgetKind::OrderTicket).title(),
            "Order Ticket 9"
        );
    }

    #[test]
    fn live_chart_default_host_prefers_in_shell_workspace_surface() {
        let host = WidgetKind::LiveChart.default_host_config(egui::pos2(10.0, 20.0));
        assert_eq!(host.mode, WidgetHostMode::InShellWorkspaceSurface);
        assert_eq!(host.default_size, egui::vec2(760.0, 440.0));
        assert_eq!(host.min_size, egui::vec2(120.0, 96.0));
        assert_eq!(host.in_shell_min_size, egui::vec2(280.0, 190.0));
    }

    #[test]
    fn synced_timeframes_default_host_prefers_wide_terminal_surface() {
        let host = WidgetKind::SyncedTimeframes.default_host_config(egui::pos2(10.0, 20.0));
        assert_eq!(host.mode, WidgetHostMode::InShellWorkspaceSurface);
        assert_eq!(host.default_size, egui::vec2(1_160.0, 520.0));
        assert_eq!(host.min_size, egui::vec2(360.0, 260.0));
    }

    #[test]
    fn bookmap_order_flow_default_host_prefers_max_workspace_surface() {
        let host = WidgetKind::BookmapOrderFlow.default_host_config(egui::pos2(10.0, 20.0));
        assert_eq!(host.mode, WidgetHostMode::InShellWorkspaceSurface);
        assert_eq!(host.default_size, egui::vec2(1_840.0, 1_020.0));
        assert_eq!(host.min_size, egui::vec2(960.0, 640.0));
        assert_eq!(host.in_shell_min_size, egui::vec2(960.0, 640.0));
    }

    #[test]
    fn detached_native_viewport_mode_remains_available_for_tear_outs() {
        let mut host = WidgetKind::LiveChart.default_host_config(egui::pos2(10.0, 20.0));
        host.mode = WidgetHostMode::DetachedNativeViewport;
        assert_eq!(host.mode, WidgetHostMode::DetachedNativeViewport);
        assert_eq!(host.default_size, egui::vec2(760.0, 440.0));
        assert_eq!(host.min_size, egui::vec2(120.0, 96.0));
    }

    #[test]
    fn widget_catalog_has_unique_keys_mnemonics_and_kinds() {
        let mut keys = BTreeSet::new();
        let mut mnemonics = BTreeSet::new();
        let mut kinds = BTreeSet::new();
        for entry in widget_catalog() {
            assert!(keys.insert(entry.kind.stable_key()));
            assert!(mnemonics.insert(entry.kind.mnemonic()));
            assert!(kinds.insert(entry.kind));
        }
        assert_eq!(widget_catalog().len(), 19);
    }

    #[test]
    fn widget_catalog_query_matches_kind_category_and_description() {
        let catalog = widget_catalog();
        assert!(catalog.iter().any(|entry| entry.matches_query("chart")));
        assert!(catalog.iter().any(|entry| entry.matches_query("TF")));
        assert!(
            catalog
                .iter()
                .any(|entry| entry.matches_query("synced timeframes"))
        );
        assert!(catalog.iter().any(|entry| entry.matches_query("market")));
        assert!(catalog.iter().any(|entry| entry.matches_query("order")));
        assert!(catalog.iter().any(|entry| entry.matches_query("heatmap")));
        assert!(catalog.iter().any(|entry| entry.matches_query("footprint")));
        assert!(catalog.iter().any(|entry| entry.matches_query("tradermap")));
        assert!(catalog.iter().any(|entry| entry.matches_query("databento")));
        assert!(catalog.iter().any(|entry| entry.matches_query("NW")));
        assert!(!catalog.iter().any(|entry| entry.matches_query("broker")));
    }

    #[test]
    fn placeholder_widgets_have_terminal_host_defaults() {
        for entry in widget_catalog()
            .iter()
            .filter(|entry| entry.kind.is_placeholder())
        {
            let host = entry.kind.default_host_config(egui::pos2(10.0, 20.0));
            assert_eq!(host.mode, WidgetHostMode::InShellWorkspaceSurface);
            assert!(host.default_size.x >= host.min_size.x);
            assert!(host.default_size.y >= host.min_size.y);
            assert!(host.default_size.x >= host.in_shell_min_size.x);
            assert!(host.default_size.y >= host.in_shell_min_size.y);
        }
    }
}
