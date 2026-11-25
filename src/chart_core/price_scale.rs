use super::model::OhlcBar;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PriceRange {
    pub min: f64,
    pub max: f64,
}

impl PriceRange {
    pub fn new(min: f64, max: f64) -> Option<Self> {
        (min.is_finite() && max.is_finite() && max > min).then_some(Self { min, max })
    }

    pub fn span(self) -> f64 {
        (self.max - self.min).max(0.000_001)
    }

    pub fn padded(self, margins: ScaleMargins) -> Self {
        let span = self.span();
        Self {
            min: self.min - span * margins.bottom.max(0.0),
            max: self.max + span * margins.top.max(0.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaleMargins {
    pub top: f64,
    pub bottom: f64,
}

impl Default for ScaleMargins {
    fn default() -> Self {
        Self {
            top: 0.08,
            bottom: 0.22,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PriceScaleMode {
    Auto,
    Manual(PriceRange),
}

impl Default for PriceScaleMode {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PriceScaleValueMode {
    Linear,
    Log,
    Percent { base: f64 },
    Indexed { base: f64 },
    Inverse,
}

impl Default for PriceScaleValueMode {
    fn default() -> Self {
        Self::Linear
    }
}

impl PriceScaleValueMode {
    const LOG_MIN_VALUE: f64 = 1e-12;

    pub fn coordinate(self, value: f64) -> Option<f64> {
        if !value.is_finite() {
            return None;
        }
        match self {
            Self::Linear => Some(value),
            Self::Log => (value > 0.0).then(|| value.ln()),
            Self::Percent { base } => finite_nonzero(base).map(|base| (value / base - 1.0) * 100.0),
            Self::Indexed { base } => finite_nonzero(base).map(|base| value / base * 100.0),
            Self::Inverse => Some(-value),
        }
    }

    pub fn value(self, coordinate: f64) -> Option<f64> {
        if !coordinate.is_finite() {
            return None;
        }
        match self {
            Self::Linear => Some(coordinate),
            Self::Log => Some(coordinate.exp()),
            Self::Percent { base } => {
                finite_nonzero(base).map(|base| base * (1.0 + coordinate / 100.0))
            }
            Self::Indexed { base } => finite_nonzero(base).map(|base| base * coordinate / 100.0),
            Self::Inverse => Some(-coordinate),
        }
    }

    pub fn coordinate_range(self, range: PriceRange) -> PriceRange {
        match self {
            Self::Linear => range,
            Self::Log => {
                let min = range.min.max(Self::LOG_MIN_VALUE);
                let max = range.max.max(min * 1.000_001);
                PriceRange {
                    min: min.ln(),
                    max: max.ln(),
                }
            }
            Self::Percent { .. } | Self::Indexed { .. } | Self::Inverse => {
                let Some(first) = self.coordinate(range.min) else {
                    return range;
                };
                let Some(second) = self.coordinate(range.max) else {
                    return range;
                };
                sorted_range(first, second).unwrap_or(range)
            }
        }
    }

    pub fn value_range(self, coordinate_range: PriceRange) -> Option<PriceRange> {
        let first = self.value(coordinate_range.min)?;
        let second = self.value(coordinate_range.max)?;
        sorted_range(first, second)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PriceScale {
    mode: PriceScaleMode,
    value_mode: PriceScaleValueMode,
    margins: ScaleMargins,
}

impl Default for PriceScale {
    fn default() -> Self {
        Self {
            mode: PriceScaleMode::Auto,
            value_mode: PriceScaleValueMode::Linear,
            margins: ScaleMargins::default(),
        }
    }
}

impl PriceScale {
    pub fn new(margins: ScaleMargins) -> Self {
        Self {
            mode: PriceScaleMode::Auto,
            value_mode: PriceScaleValueMode::Linear,
            margins,
        }
    }

    pub fn with_value_mode(mut self, value_mode: PriceScaleValueMode) -> Self {
        self.value_mode = value_mode;
        self
    }

    pub fn mode(self) -> PriceScaleMode {
        self.mode
    }

    pub fn value_mode(self) -> PriceScaleValueMode {
        self.value_mode
    }

    pub fn set_value_mode(&mut self, value_mode: PriceScaleValueMode) {
        if self.value_mode != value_mode {
            self.reset();
        }
        self.value_mode = value_mode;
    }

    pub fn is_auto(self) -> bool {
        matches!(self.mode, PriceScaleMode::Auto)
    }

    pub fn is_manual(self) -> bool {
        matches!(self.mode, PriceScaleMode::Manual(_))
    }

    pub fn reset(&mut self) {
        self.mode = PriceScaleMode::Auto;
    }

    pub fn range(self, auto_range: PriceRange) -> PriceRange {
        match self.mode {
            PriceScaleMode::Auto => self.padded_auto_range(auto_range),
            PriceScaleMode::Manual(range) => range,
        }
    }

    fn padded_auto_range(self, auto_range: PriceRange) -> PriceRange {
        let coordinate_range = self.value_mode.coordinate_range(auto_range);
        let padded = coordinate_range.padded(self.margins);
        self.value_mode
            .value_range(padded)
            .unwrap_or_else(|| auto_range.padded(self.margins))
    }

    pub fn price_to_y(range: PriceRange, price: f64, plot_top: f64, plot_height: f64) -> f64 {
        Self::price_to_y_with_mode(
            range,
            PriceScaleValueMode::Linear,
            price,
            plot_top,
            plot_height,
        )
    }

    pub fn y_to_price(range: PriceRange, y: f64, plot_top: f64, plot_height: f64) -> f64 {
        Self::y_to_price_with_mode(range, PriceScaleValueMode::Linear, y, plot_top, plot_height)
    }

    pub fn price_to_y_with_mode(
        range: PriceRange,
        value_mode: PriceScaleValueMode,
        price: f64,
        plot_top: f64,
        plot_height: f64,
    ) -> f64 {
        let coordinate_range = value_mode.coordinate_range(range);
        let coordinate = value_mode.coordinate(price).unwrap_or(coordinate_range.min);
        let normalized =
            ((coordinate - coordinate_range.min) / coordinate_range.span()).clamp(0.0, 1.0);
        plot_top + (1.0 - normalized) * plot_height
    }

    pub fn y_to_price_with_mode(
        range: PriceRange,
        value_mode: PriceScaleValueMode,
        y: f64,
        plot_top: f64,
        plot_height: f64,
    ) -> f64 {
        let coordinate_range = value_mode.coordinate_range(range);
        let normalized = (1.0 - ((y - plot_top) / plot_height.max(1.0))).clamp(0.0, 1.0);
        let coordinate = coordinate_range.min + normalized * coordinate_range.span();
        value_mode.value(coordinate).unwrap_or(coordinate)
    }

    pub fn zoom_at_y(
        &mut self,
        multiplier: f64,
        anchor_y: f64,
        plot_top: f64,
        plot_height: f64,
        current: PriceRange,
    ) {
        let multiplier = multiplier.clamp(0.02, 50.0);
        let coordinate_range = self.value_mode.coordinate_range(current);
        let anchor_price =
            Self::y_to_price_with_mode(current, self.value_mode, anchor_y, plot_top, plot_height);
        let anchor_coordinate = self
            .value_mode
            .coordinate(anchor_price)
            .unwrap_or(coordinate_range.min);
        let anchor_ratio =
            ((anchor_coordinate - coordinate_range.min) / coordinate_range.span()).clamp(0.0, 1.0);
        let new_span = coordinate_range.span() / multiplier;
        let min = anchor_coordinate - new_span * anchor_ratio;
        let coordinate_range = PriceRange {
            min,
            max: min + new_span,
        };
        if let Some(range) = self.value_mode.value_range(coordinate_range) {
            self.mode = PriceScaleMode::Manual(range);
        }
    }

    pub fn pan_by_pixels(&mut self, delta_y: f64, plot_height: f64, current: PriceRange) {
        let coordinate_range = self.value_mode.coordinate_range(current);
        let delta = delta_y / plot_height.max(1.0) * coordinate_range.span();
        let shifted = PriceRange {
            min: coordinate_range.min + delta,
            max: coordinate_range.max + delta,
        };
        if let Some(range) = self.value_mode.value_range(shifted) {
            self.mode = PriceScaleMode::Manual(range);
        }
    }
}

fn finite_nonzero(value: f64) -> Option<f64> {
    (value.is_finite() && value.abs() > f64::EPSILON).then_some(value)
}

fn sorted_range(first: f64, second: f64) -> Option<PriceRange> {
    PriceRange::new(first.min(second), first.max(second))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PriceScaleId {
    Right,
    Left,
    Overlay,
    Volume,
}

impl PriceScaleId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Left => "left",
            Self::Overlay => "overlay",
            Self::Volume => "volume",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceScalePlacement {
    Right,
    Left,
    Overlay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceScaleDomain {
    Price,
    Volume,
    Study,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PriceScaleEntry {
    id: PriceScaleId,
    placement: PriceScalePlacement,
    domain: PriceScaleDomain,
    scale: PriceScale,
}

impl PriceScaleEntry {
    pub fn new(
        id: PriceScaleId,
        placement: PriceScalePlacement,
        domain: PriceScaleDomain,
        scale: PriceScale,
    ) -> Self {
        Self {
            id,
            placement,
            domain,
            scale,
        }
    }

    pub fn id(self) -> PriceScaleId {
        self.id
    }

    pub fn placement(self) -> PriceScalePlacement {
        self.placement
    }

    pub fn domain(self) -> PriceScaleDomain {
        self.domain
    }

    pub fn scale(self) -> PriceScale {
        self.scale
    }

    pub fn scale_mut(&mut self) -> &mut PriceScale {
        &mut self.scale
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PriceScaleRegistry {
    entries: [PriceScaleEntry; 4],
}

impl Default for PriceScaleRegistry {
    fn default() -> Self {
        Self {
            entries: [
                PriceScaleEntry::new(
                    PriceScaleId::Right,
                    PriceScalePlacement::Right,
                    PriceScaleDomain::Price,
                    PriceScale::default(),
                ),
                PriceScaleEntry::new(
                    PriceScaleId::Left,
                    PriceScalePlacement::Left,
                    PriceScaleDomain::Price,
                    PriceScale::default(),
                ),
                PriceScaleEntry::new(
                    PriceScaleId::Overlay,
                    PriceScalePlacement::Overlay,
                    PriceScaleDomain::Study,
                    PriceScale::default(),
                ),
                PriceScaleEntry::new(
                    PriceScaleId::Volume,
                    PriceScalePlacement::Overlay,
                    PriceScaleDomain::Volume,
                    PriceScale::new(ScaleMargins {
                        top: 0.05,
                        bottom: 0.0,
                    }),
                ),
            ],
        }
    }
}

impl PriceScaleRegistry {
    pub fn entries(&self) -> &[PriceScaleEntry] {
        &self.entries
    }

    pub fn by_placement(
        &self,
        placement: PriceScalePlacement,
    ) -> impl Iterator<Item = &PriceScaleEntry> {
        self.entries
            .iter()
            .filter(move |entry| entry.placement == placement)
    }

    pub fn entry(&self, id: PriceScaleId) -> &PriceScaleEntry {
        &self.entries[scale_index(id)]
    }

    pub fn entry_mut(&mut self, id: PriceScaleId) -> &mut PriceScaleEntry {
        &mut self.entries[scale_index(id)]
    }

    pub fn scale(&self, id: PriceScaleId) -> PriceScale {
        self.entry(id).scale()
    }

    pub fn scale_mut(&mut self, id: PriceScaleId) -> &mut PriceScale {
        self.entry_mut(id).scale_mut()
    }

    pub fn range(&self, id: PriceScaleId, auto_range: PriceRange) -> PriceRange {
        self.scale(id).range(auto_range)
    }

    pub fn reset(&mut self, id: PriceScaleId) {
        self.scale_mut(id).reset();
    }

    pub fn reset_all(&mut self) {
        for entry in &mut self.entries {
            entry.scale.reset();
        }
    }
}

fn scale_index(id: PriceScaleId) -> usize {
    match id {
        PriceScaleId::Right => 0,
        PriceScaleId::Left => 1,
        PriceScaleId::Overlay => 2,
        PriceScaleId::Volume => 3,
    }
}

pub fn ohlc_auto_range(bars: impl IntoIterator<Item = OhlcBar>) -> Option<PriceRange> {
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for bar in bars {
        if !bar.is_valid() {
            continue;
        }
        min = min.min(bar.low);
        max = max.max(bar.high);
    }
    PriceRange::new(min, max).or_else(|| {
        min.is_finite()
            .then_some(PriceRange {
                min: min - 0.5,
                max: min + 0.5,
            })
            .filter(|range| range.max > range.min)
    })
}

pub fn volume_auto_range(bars: impl IntoIterator<Item = OhlcBar>) -> Option<PriceRange> {
    let mut max = f64::NEG_INFINITY;
    for bar in bars {
        if !bar.is_valid() {
            continue;
        }
        max = max.max(bar.volume);
    }
    max.is_finite().then(|| PriceRange {
        min: 0.0,
        max: max.max(1.0),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        PriceRange, PriceScale, PriceScaleDomain, PriceScaleId, PriceScaleMode,
        PriceScalePlacement, PriceScaleRegistry, PriceScaleValueMode, ScaleMargins,
        volume_auto_range,
    };
    use crate::tv_chart::model::OhlcBar;
    use crate::tv_chart::price_scale::ohlc_auto_range;

    #[test]
    fn auto_range_applies_top_and_bottom_margins() {
        let scale = PriceScale::new(ScaleMargins {
            top: 0.1,
            bottom: 0.2,
        });

        let range = scale.range(PriceRange {
            min: 100.0,
            max: 200.0,
        });

        assert_eq!(
            range,
            PriceRange {
                min: 80.0,
                max: 210.0
            }
        );
    }

    #[test]
    fn price_coordinate_transform_round_trips() {
        let range = PriceRange {
            min: 100.0,
            max: 200.0,
        };
        let y = PriceScale::price_to_y(range, 125.0, 10.0, 400.0);

        assert!((PriceScale::y_to_price(range, y, 10.0, 400.0) - 125.0).abs() < 1e-9);
    }

    #[test]
    fn value_modes_transform_price_coordinates() {
        let range = PriceRange {
            min: 100.0,
            max: 200.0,
        };

        assert_eq!(PriceScaleValueMode::Linear.coordinate_range(range), range);
        assert_eq!(
            PriceScaleValueMode::Inverse.coordinate_range(range),
            PriceRange {
                min: -200.0,
                max: -100.0
            }
        );
        assert!(
            (PriceScaleValueMode::Percent { base: 100.0 }
                .coordinate(110.0)
                .unwrap()
                - 10.0)
                .abs()
                < 1e-12
        );
        assert!(
            (PriceScaleValueMode::Indexed { base: 100.0 }
                .coordinate(110.0)
                .unwrap()
                - 110.0)
                .abs()
                < 1e-12
        );

        let log_range = PriceScaleValueMode::Log.coordinate_range(range);
        assert!((log_range.min - 100.0_f64.ln()).abs() < 1e-12);
        assert!((log_range.max - 200.0_f64.ln()).abs() < 1e-12);
    }

    #[test]
    fn value_modes_round_trip_through_y_coordinates() {
        let range = PriceRange {
            min: 80.0,
            max: 120.0,
        };

        for mode in [
            PriceScaleValueMode::Linear,
            PriceScaleValueMode::Log,
            PriceScaleValueMode::Percent { base: 100.0 },
            PriceScaleValueMode::Indexed { base: 100.0 },
            PriceScaleValueMode::Inverse,
        ] {
            let y = PriceScale::price_to_y_with_mode(range, mode, 110.0, 0.0, 400.0);
            let price = PriceScale::y_to_price_with_mode(range, mode, y, 0.0, 400.0);

            assert!(
                (price - 110.0).abs() < 1e-9,
                "mode {mode:?} round-tripped to {price}"
            );
        }
    }

    #[test]
    fn zoom_keeps_anchor_price_stable() {
        let mut scale = PriceScale::default();
        let current = PriceRange {
            min: 100.0,
            max: 200.0,
        };
        let anchor_y = 300.0;
        let before = PriceScale::y_to_price(current, anchor_y, 0.0, 400.0);

        scale.zoom_at_y(2.0, anchor_y, 0.0, 400.0, current);
        let PriceScaleMode::Manual(after_range) = scale.mode() else {
            panic!("expected manual price scale");
        };
        let after = PriceScale::y_to_price(after_range, anchor_y, 0.0, 400.0);

        assert!((after - before).abs() < 1e-9);
        assert!(after_range.span() < current.span());
    }

    #[test]
    fn manual_state_tracks_price_axis_adjustments_and_reset() {
        let mut scale = PriceScale::default();
        assert!(scale.is_auto());
        assert!(!scale.is_manual());

        scale.pan_by_pixels(
            40.0,
            400.0,
            PriceRange {
                min: 100.0,
                max: 200.0,
            },
        );

        assert!(scale.is_manual());
        assert!(!scale.is_auto());

        scale.reset();

        assert!(scale.is_auto());
        assert!(!scale.is_manual());
    }

    #[test]
    fn changing_value_mode_resets_manual_range() {
        let mut scale = PriceScale::default();
        scale.zoom_at_y(
            2.0,
            200.0,
            0.0,
            400.0,
            PriceRange {
                min: 100.0,
                max: 200.0,
            },
        );
        assert!(scale.is_manual());

        scale.set_value_mode(PriceScaleValueMode::Log);

        assert_eq!(scale.value_mode(), PriceScaleValueMode::Log);
        assert!(scale.is_auto());
    }

    #[test]
    fn pan_by_pixels_preserves_span_and_moves_visible_range() {
        let mut scale = PriceScale::default();
        let current = PriceRange {
            min: 100.0,
            max: 200.0,
        };

        scale.pan_by_pixels(40.0, 400.0, current);

        assert_eq!(
            scale.mode(),
            PriceScaleMode::Manual(PriceRange {
                min: 110.0,
                max: 210.0
            })
        );
    }

    #[test]
    fn ohlc_auto_range_ignores_invalid_bars() {
        let bars = [
            OhlcBar {
                time: 1,
                open: 10.0,
                high: 11.0,
                low: 9.0,
                close: 10.5,
                volume: 1.0,
            },
            OhlcBar {
                time: 2,
                open: 10.0,
                high: 9.0,
                low: 11.0,
                close: 10.5,
                volume: 1.0,
            },
        ];

        assert_eq!(
            ohlc_auto_range(bars),
            Some(PriceRange {
                min: 9.0,
                max: 11.0
            })
        );
    }

    #[test]
    fn registry_exposes_right_left_overlay_and_volume_scales() {
        let registry = PriceScaleRegistry::default();

        assert_eq!(
            registry
                .entries()
                .iter()
                .map(|entry| entry.id())
                .collect::<Vec<_>>(),
            vec![
                PriceScaleId::Right,
                PriceScaleId::Left,
                PriceScaleId::Overlay,
                PriceScaleId::Volume
            ]
        );
        assert_eq!(
            registry.entry(PriceScaleId::Right).placement(),
            PriceScalePlacement::Right
        );
        assert_eq!(
            registry.entry(PriceScaleId::Left).placement(),
            PriceScalePlacement::Left
        );
        assert_eq!(
            registry.entry(PriceScaleId::Overlay).placement(),
            PriceScalePlacement::Overlay
        );
        assert_eq!(
            registry.entry(PriceScaleId::Volume).domain(),
            PriceScaleDomain::Volume
        );
    }

    #[test]
    fn registry_filters_by_scale_placement() {
        let registry = PriceScaleRegistry::default();

        assert_eq!(
            registry
                .by_placement(PriceScalePlacement::Overlay)
                .map(|entry| entry.id())
                .collect::<Vec<_>>(),
            vec![PriceScaleId::Overlay, PriceScaleId::Volume]
        );
    }

    #[test]
    fn volume_range_uses_volume_without_expanding_price_range() {
        let bars = [
            OhlcBar {
                time: 1,
                open: 10.0,
                high: 11.0,
                low: 9.0,
                close: 10.0,
                volume: 50_000.0,
            },
            OhlcBar {
                time: 2,
                open: 10.0,
                high: 12.0,
                low: 8.0,
                close: 11.0,
                volume: 120_000.0,
            },
        ];

        assert_eq!(
            ohlc_auto_range(bars),
            Some(PriceRange {
                min: 8.0,
                max: 12.0
            })
        );
        assert_eq!(
            volume_auto_range(bars),
            Some(PriceRange {
                min: 0.0,
                max: 120_000.0
            })
        );
    }
}
