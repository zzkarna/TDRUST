#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BookmapLayerKind {
    LiquidityHeatmap,
    BboPath,
    VolumeBubbles,
    StopsIcebergs,
    ReplayCursor,
    CobDomColumns,
    TradermapSplit,
    DomProPanel,
    MarketPulse,
}

impl BookmapLayerKind {
    pub const ALL: [Self; 9] = [
        Self::LiquidityHeatmap,
        Self::BboPath,
        Self::VolumeBubbles,
        Self::CobDomColumns,
        Self::ReplayCursor,
        Self::StopsIcebergs,
        Self::TradermapSplit,
        Self::DomProPanel,
        Self::MarketPulse,
    ];

    pub const RENDER_ORDER: [Self; 9] = [
        Self::LiquidityHeatmap,
        Self::BboPath,
        Self::VolumeBubbles,
        Self::StopsIcebergs,
        Self::ReplayCursor,
        Self::CobDomColumns,
        Self::TradermapSplit,
        Self::DomProPanel,
        Self::MarketPulse,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::LiquidityHeatmap => "Liquidity heatmap",
            Self::BboPath => "Best bid/ask path",
            Self::VolumeBubbles => "Volume bubbles",
            Self::StopsIcebergs => "Stops/Icebergs heuristic",
            Self::ReplayCursor => "Replay cursor",
            Self::CobDomColumns => "COB/DOM columns",
            Self::TradermapSplit => "Tradermap split mode",
            Self::DomProPanel => "DOM Pro side panel",
            Self::MarketPulse => "Market Pulse panels",
        }
    }

    pub const fn short_label(self) -> &'static str {
        match self {
            Self::LiquidityHeatmap => "HM",
            Self::BboPath => "BBO",
            Self::VolumeBubbles => "VOL",
            Self::StopsIcebergs => "S/I",
            Self::ReplayCursor => "RPL",
            Self::CobDomColumns => "COB",
            Self::TradermapSplit => "TRD",
            Self::DomProPanel => "DOM+",
            Self::MarketPulse => "PLS",
        }
    }

    pub const fn is_core(self) -> bool {
        matches!(
            self,
            Self::LiquidityHeatmap
                | Self::BboPath
                | Self::VolumeBubbles
                | Self::ReplayCursor
                | Self::CobDomColumns
        )
    }

    pub const fn is_add_on(self) -> bool {
        matches!(
            self,
            Self::StopsIcebergs | Self::TradermapSplit | Self::DomProPanel | Self::MarketPulse
        )
    }

    pub const fn intentionally_splits_chart(self) -> bool {
        matches!(self, Self::TradermapSplit)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmapLayerState {
    pub liquidity_heatmap: bool,
    pub bbo_path: bool,
    pub volume_bubbles: bool,
    pub stops_icebergs: bool,
    pub replay_cursor: bool,
    pub cob_dom_columns: bool,
    pub tradermap_split: bool,
    pub dom_pro_panel: bool,
    pub market_pulse: bool,
}

impl Default for BookmapLayerState {
    fn default() -> Self {
        Self {
            liquidity_heatmap: true,
            bbo_path: true,
            volume_bubbles: false,
            stops_icebergs: false,
            replay_cursor: true,
            cob_dom_columns: true,
            tradermap_split: false,
            dom_pro_panel: false,
            market_pulse: false,
        }
    }
}

impl BookmapLayerState {
    pub fn is_enabled(&self, kind: BookmapLayerKind) -> bool {
        match kind {
            BookmapLayerKind::LiquidityHeatmap => self.liquidity_heatmap,
            BookmapLayerKind::BboPath => self.bbo_path,
            BookmapLayerKind::VolumeBubbles => self.volume_bubbles,
            BookmapLayerKind::StopsIcebergs => self.stops_icebergs,
            BookmapLayerKind::ReplayCursor => self.replay_cursor,
            BookmapLayerKind::CobDomColumns => self.cob_dom_columns,
            BookmapLayerKind::TradermapSplit => self.tradermap_split,
            BookmapLayerKind::DomProPanel => self.dom_pro_panel,
            BookmapLayerKind::MarketPulse => self.market_pulse,
        }
    }

    pub fn set_enabled(&mut self, kind: BookmapLayerKind, enabled: bool) {
        match kind {
            BookmapLayerKind::LiquidityHeatmap => self.liquidity_heatmap = enabled,
            BookmapLayerKind::BboPath => self.bbo_path = enabled,
            BookmapLayerKind::VolumeBubbles => self.volume_bubbles = enabled,
            BookmapLayerKind::StopsIcebergs => self.stops_icebergs = enabled,
            BookmapLayerKind::ReplayCursor => self.replay_cursor = enabled,
            BookmapLayerKind::CobDomColumns => self.cob_dom_columns = enabled,
            BookmapLayerKind::TradermapSplit => self.tradermap_split = enabled,
            BookmapLayerKind::DomProPanel => self.dom_pro_panel = enabled,
            BookmapLayerKind::MarketPulse => self.market_pulse = enabled,
        }
    }

    pub fn toggle(&mut self, kind: BookmapLayerKind) {
        self.set_enabled(kind, !self.is_enabled(kind));
    }

    pub fn enabled_layers_in_render_order(&self) -> Vec<BookmapLayerKind> {
        BookmapLayerKind::RENDER_ORDER
            .into_iter()
            .filter(|kind| self.is_enabled(*kind))
            .collect()
    }

    pub fn enabled_count(&self) -> usize {
        self.enabled_layers_in_render_order().len()
    }

    pub fn active_add_on_count(&self) -> usize {
        BookmapLayerKind::ALL
            .into_iter()
            .filter(|kind| kind.is_add_on() && self.is_enabled(*kind))
            .count()
    }

    pub fn core_layer_count(&self) -> usize {
        BookmapLayerKind::ALL
            .into_iter()
            .filter(|kind| kind.is_core() && self.is_enabled(*kind))
            .count()
    }

    pub fn summary_label(&self) -> String {
        format!(
            "{} core / {} add-ons",
            self.core_layer_count(),
            self.active_add_on_count()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layer_state_enables_unobstructed_terminal_layers() {
        let state = BookmapLayerState::default();

        assert!(state.is_enabled(BookmapLayerKind::LiquidityHeatmap));
        assert!(state.is_enabled(BookmapLayerKind::BboPath));
        assert!(!state.is_enabled(BookmapLayerKind::VolumeBubbles));
        assert!(state.is_enabled(BookmapLayerKind::CobDomColumns));
        assert!(state.is_enabled(BookmapLayerKind::ReplayCursor));
        assert!(!state.is_enabled(BookmapLayerKind::StopsIcebergs));
        assert!(!state.is_enabled(BookmapLayerKind::MarketPulse));
        assert_eq!(state.core_layer_count(), 4);
        assert_eq!(state.active_add_on_count(), 0);
    }

    #[test]
    fn render_order_is_deterministic_and_matches_composite_contract() {
        let mut state = BookmapLayerState::default();
        state.set_enabled(BookmapLayerKind::VolumeBubbles, true);
        state.set_enabled(BookmapLayerKind::StopsIcebergs, true);
        state.set_enabled(BookmapLayerKind::DomProPanel, true);
        state.set_enabled(BookmapLayerKind::MarketPulse, true);

        assert_eq!(
            state.enabled_layers_in_render_order(),
            vec![
                BookmapLayerKind::LiquidityHeatmap,
                BookmapLayerKind::BboPath,
                BookmapLayerKind::VolumeBubbles,
                BookmapLayerKind::StopsIcebergs,
                BookmapLayerKind::ReplayCursor,
                BookmapLayerKind::CobDomColumns,
                BookmapLayerKind::DomProPanel,
                BookmapLayerKind::MarketPulse,
            ]
        );
    }

    #[test]
    fn toggling_add_on_layers_does_not_disable_primary_chart_layers() {
        let mut state = BookmapLayerState::default();

        state.toggle(BookmapLayerKind::TradermapSplit);
        state.toggle(BookmapLayerKind::MarketPulse);

        assert!(BookmapLayerKind::TradermapSplit.intentionally_splits_chart());
        assert!(state.is_enabled(BookmapLayerKind::TradermapSplit));
        assert!(state.is_enabled(BookmapLayerKind::MarketPulse));
        assert!(state.is_enabled(BookmapLayerKind::LiquidityHeatmap));
        assert!(state.is_enabled(BookmapLayerKind::BboPath));
        assert!(!state.is_enabled(BookmapLayerKind::VolumeBubbles));
        assert!(state.is_enabled(BookmapLayerKind::CobDomColumns));
    }
}
