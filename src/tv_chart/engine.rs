use eframe::egui;

use crate::studies::{StudyDataSource, StudyValue};

use super::{
    BarSeries, ChartTheme, ChartTransform, GestureConfig, LogicalRange, PaneId, PaneModel,
    PaneRect, PriceRange, PriceScaleId, PriceScaleRegistry, PriceScaleValueMode, SeriesKind,
    SeriesRegistry, SeriesValueSource, TimeScale, VOLUME_SERIES_ID,
    price_scale::{ohlc_auto_range, volume_auto_range},
};

#[derive(Debug, Clone)]
pub struct TvChartState {
    pub time_scale: TimeScale,
    pub price_scales: PriceScaleRegistry,
    pub panes: PaneModel,
    pub series: SeriesRegistry,
    pub theme: ChartTheme,
    pub data_len: usize,
    pub follow_realtime: bool,
    gesture_config: GestureConfig,
    theme_generation: u64,
}

impl Default for TvChartState {
    fn default() -> Self {
        Self {
            time_scale: TimeScale::default(),
            price_scales: PriceScaleRegistry::default(),
            panes: PaneModel::default(),
            series: SeriesRegistry::default(),
            theme: ChartTheme::default(),
            data_len: 0,
            follow_realtime: true,
            gesture_config: GestureConfig::default(),
            theme_generation: 0,
        }
    }
}

impl TvChartState {
    pub(crate) fn gesture_config(&self) -> GestureConfig {
        self.gesture_config
    }

    pub fn set_volume_visible(&mut self, visible: bool) {
        self.panes.set_visible(PaneId::Volume, visible);
        self.series.set_visible(VOLUME_SERIES_ID, visible);
    }

    pub fn volume_visible(&self) -> bool {
        self.panes.is_visible(PaneId::Volume) && self.series.is_visible(VOLUME_SERIES_ID)
    }

    pub fn theme(&self) -> &ChartTheme {
        &self.theme
    }

    pub fn set_theme(&mut self, theme: ChartTheme) -> bool {
        if self.theme == theme {
            return false;
        }
        self.theme = theme;
        self.theme_generation = self.theme_generation.saturating_add(1);
        true
    }

    pub fn theme_generation(&self) -> u64 {
        self.theme_generation
    }
}

pub fn sync_chart_state_for_frame<B: BarSeries + ?Sized>(
    state: &mut TvChartState,
    bars: &B,
    plot: egui::Rect,
) {
    sync_state(state, bars.len(), plot.width());
}

pub fn prepare_chart_snapshot<B: BarSeries + ?Sized>(
    state: &TvChartState,
    bars: &B,
    plot: egui::Rect,
) -> ChartSnapshot {
    build_snapshot(state, bars, plot)
}

pub fn prepare_chart_snapshot_with_studies<B: BarSeries + ?Sized>(
    state: &TvChartState,
    bars: &B,
    plot: egui::Rect,
    studies: Option<&dyn StudyDataSource>,
) -> ChartSnapshot {
    build_snapshot_with_studies(state, bars, plot, studies)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartSnapshot {
    pub visible_bounds: Option<(usize, usize)>,
    pub auto_range: PriceRange,
    pub price_range: PriceRange,
    pub volume_range: Option<PriceRange>,
    pub study_ranges: Vec<PaneRect<PriceRange>>,
    pub transform: ChartTransform,
}

pub fn sync_state(state: &mut TvChartState, data_len: usize, plot_width: f32) {
    if state.data_len == 0 {
        state.time_scale.reset(data_len, f64::from(plot_width));
    } else {
        state
            .time_scale
            .on_data_len_changed(state.data_len, data_len);
        state.time_scale.sync_bar_spacing(f64::from(plot_width));
    }
    if state.follow_realtime && !state.time_scale.follows_realtime() {
        state.follow_realtime = false;
    }
    state.data_len = data_len;
}

pub fn build_snapshot<B: BarSeries + ?Sized>(
    state: &TvChartState,
    bars: &B,
    plot: egui::Rect,
) -> ChartSnapshot {
    build_snapshot_with_studies(state, bars, plot, None)
}

pub fn build_snapshot_with_studies<B: BarSeries + ?Sized>(
    state: &TvChartState,
    bars: &B,
    plot: egui::Rect,
    studies: Option<&dyn StudyDataSource>,
) -> ChartSnapshot {
    let visible_bounds = visible_bounds(bars.len(), state.time_scale.visible_logical_range());
    let bar_auto_range = visible_bounds
        .and_then(|(from, to)| ohlc_auto_range((from..=to).filter_map(|index| bars.bar(index))))
        .unwrap_or(PriceRange { min: 0.0, max: 1.0 });
    let mut auto_range = bar_auto_range;
    if let Some(study_range) = main_price_study_auto_range(state, studies, visible_bounds) {
        auto_range = merge_price_ranges(auto_range, study_range);
    }
    let price_scale = state
        .price_scales
        .scale(PriceScaleId::Right)
        .with_value_mode(resolve_visible_base_price_mode(
            state.price_scales.scale(PriceScaleId::Right).value_mode(),
            visible_bounds,
            bars,
        ));
    let price_range = price_scale.range(auto_range);
    let volume_range = if state.volume_visible() {
        visible_bounds
            .and_then(|(from, to)| {
                volume_auto_range((from..=to).filter_map(|index| bars.bar(index)))
            })
            .map(|range| state.price_scales.range(PriceScaleId::Volume, range))
    } else {
        None
    };
    let study_ranges = study_auto_ranges(state, studies, visible_bounds);
    let transform =
        ChartTransform::new(plot, state.time_scale.visible_logical_range(), price_range)
            .with_price_mode(price_scale.value_mode());

    ChartSnapshot {
        visible_bounds,
        auto_range,
        price_range,
        volume_range,
        study_ranges,
        transform,
    }
}

fn main_price_study_auto_range(
    state: &TvChartState,
    studies: Option<&dyn StudyDataSource>,
    visible_bounds: Option<(usize, usize)>,
) -> Option<PriceRange> {
    let studies = studies?;
    let (from, to) = visible_bounds?;
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;

    for series in state.series.visible() {
        if series.value_source != SeriesValueSource::StudyOutput
            || series.pane != PaneId::Main
            || series.price_scale != PriceScaleId::Right
        {
            continue;
        }
        if matches!(studies.output_style(series.id), Some(style) if !style.visible) {
            continue;
        }
        let Some(values) = studies.output_values(series.id) else {
            continue;
        };
        let Some(indices) =
            shifted_visible_indices(values.len(), from, to, studies.output_offset(series.id))
        else {
            continue;
        };
        for index in indices {
            if let Some(StudyValue::Number(value)) = values.get(index).copied()
                && value.is_finite()
            {
                min = min.min(value);
                max = max.max(value);
            }
        }
    }

    finite_value_range(min, max)
}

fn finite_value_range(min: f64, max: f64) -> Option<PriceRange> {
    PriceRange::new(min, max).or_else(|| {
        min.is_finite().then_some(PriceRange {
            min: min - 0.5,
            max: min + 0.5,
        })
    })
}

fn merge_price_ranges(first: PriceRange, second: PriceRange) -> PriceRange {
    PriceRange {
        min: first.min.min(second.min),
        max: first.max.max(second.max),
    }
}

fn study_auto_ranges(
    state: &TvChartState,
    studies: Option<&dyn StudyDataSource>,
    visible_bounds: Option<(usize, usize)>,
) -> Vec<PaneRect<PriceRange>> {
    let Some(studies) = studies else {
        return Vec::new();
    };
    let Some((from, to)) = visible_bounds else {
        return Vec::new();
    };
    let mut ranges: Vec<PaneRect<PriceRange>> = Vec::new();

    for series in state.series.visible() {
        if series.value_source != SeriesValueSource::StudyOutput
            || !matches!(series.pane, PaneId::Study(_))
        {
            continue;
        }
        if matches!(studies.output_style(series.id), Some(style) if !style.visible) {
            continue;
        }
        let Some(values) = studies.output_values(series.id) else {
            continue;
        };
        let Some(indices) =
            shifted_visible_indices(values.len(), from, to, studies.output_offset(series.id))
        else {
            continue;
        };
        let include_zero = matches!(series.kind, SeriesKind::Histogram);
        let value_range = study_values_range(values, indices, include_zero);
        let base_range = study_series_base_range(series.name.as_str());
        let Some(range) = merge_optional_price_ranges(base_range, value_range) else {
            continue;
        };
        if let Some(existing) = ranges
            .iter_mut()
            .find(|pane_range| pane_range.id == series.pane)
        {
            existing.rect = merge_price_ranges(existing.rect, range);
        } else {
            ranges.push(PaneRect {
                id: series.pane,
                rect: range,
            });
        }
    }

    ranges
}

fn study_series_base_range(name: &str) -> Option<PriceRange> {
    name.starts_with("RSI").then_some(PriceRange {
        min: 0.0,
        max: 100.0,
    })
}

fn merge_optional_price_ranges(
    first: Option<PriceRange>,
    second: Option<PriceRange>,
) -> Option<PriceRange> {
    match (first, second) {
        (Some(first), Some(second)) => Some(merge_price_ranges(first, second)),
        (Some(range), None) | (None, Some(range)) => Some(range),
        (None, None) => None,
    }
}

fn study_values_range(
    values: &[StudyValue],
    indices: std::ops::RangeInclusive<usize>,
    include_zero: bool,
) -> Option<PriceRange> {
    let mut min = if include_zero { 0.0 } else { f64::INFINITY };
    let mut max = if include_zero { 0.0 } else { f64::NEG_INFINITY };

    for index in indices {
        if let Some(StudyValue::Number(value)) = values.get(index).copied()
            && value.is_finite()
        {
            min = min.min(value);
            max = max.max(value);
        }
    }

    finite_value_range(min, max)
}

fn shifted_visible_indices(
    len: usize,
    visible_from: usize,
    visible_to: usize,
    offset: i32,
) -> Option<std::ops::RangeInclusive<usize>> {
    if len == 0 {
        return None;
    }
    let source_from = (visible_from as i64 - i64::from(offset)).max(0);
    let source_to = (visible_to as i64 - i64::from(offset)).min(len.saturating_sub(1) as i64);
    (source_from <= source_to).then_some(source_from as usize..=source_to as usize)
}

pub fn current_price_range<B: BarSeries + ?Sized>(state: &TvChartState, bars: &B) -> PriceRange {
    let visible = visible_bounds(bars.len(), state.time_scale.visible_logical_range());
    let auto = visible
        .and_then(|(from, to)| ohlc_auto_range((from..=to).filter_map(|index| bars.bar(index))))
        .unwrap_or(PriceRange { min: 0.0, max: 1.0 });
    state
        .price_scales
        .scale(PriceScaleId::Right)
        .with_value_mode(resolve_visible_base_price_mode(
            state.price_scales.scale(PriceScaleId::Right).value_mode(),
            visible,
            bars,
        ))
        .range(auto)
}

pub fn visible_base_close<B: BarSeries + ?Sized>(
    visible_bounds: Option<(usize, usize)>,
    bars: &B,
) -> Option<f64> {
    let (from, to) = visible_bounds?;
    (from..=to)
        .filter_map(|index| bars.bar(index))
        .map(|bar| bar.close)
        .find(|close| close.is_finite() && close.abs() > f64::EPSILON)
}

pub fn resolve_visible_base_price_mode<B: BarSeries + ?Sized>(
    mode: PriceScaleValueMode,
    visible_bounds: Option<(usize, usize)>,
    bars: &B,
) -> PriceScaleValueMode {
    match mode {
        PriceScaleValueMode::Percent { base } => PriceScaleValueMode::Percent {
            base: visible_base_close(visible_bounds, bars).unwrap_or(base),
        },
        PriceScaleValueMode::Indexed { base } => PriceScaleValueMode::Indexed {
            base: visible_base_close(visible_bounds, bars).unwrap_or(base),
        },
        other => other,
    }
}

pub fn should_pan_price_with_plot_drag(state: &TvChartState) -> bool {
    state.price_scales.scale(PriceScaleId::Right).is_manual()
}

pub fn visible_bounds(bar_count: usize, range: LogicalRange) -> Option<(usize, usize)> {
    if bar_count == 0 {
        return None;
    }
    let from = range.from.floor().max(0.0) as usize;
    let to = range
        .to
        .ceil()
        .max(0.0)
        .min(bar_count.saturating_sub(1) as f64) as usize;
    (from <= to).then_some((from, to))
}

#[cfg(test)]
mod tests {
    use super::{
        TvChartState, build_snapshot, build_snapshot_with_studies, resolve_visible_base_price_mode,
        should_pan_price_with_plot_drag, visible_base_close, visible_bounds,
    };
    use crate::{
        studies::{StudyColor, StudyDataSource, StudyOutputStyle, StudyValue},
        tv_chart::{
            LogicalRange, OhlcBar, PaneId, PriceRange, PriceScaleId, PriceScaleValueMode, SeriesId,
            SeriesKind, SeriesSpec, SeriesValueSource,
        },
    };
    use eframe::egui;

    struct TestStudies {
        series_id: SeriesId,
        values: Vec<StudyValue>,
    }

    impl StudyDataSource for TestStudies {
        fn output_values(&self, series_id: SeriesId) -> Option<&[StudyValue]> {
            (series_id == self.series_id).then_some(self.values.as_slice())
        }
    }

    struct StyledTestStudies {
        series_id: SeriesId,
        values: Vec<StudyValue>,
        style: StudyOutputStyle,
    }

    impl StudyDataSource for StyledTestStudies {
        fn output_values(&self, series_id: SeriesId) -> Option<&[StudyValue]> {
            (series_id == self.series_id).then_some(self.values.as_slice())
        }

        fn output_style(&self, series_id: SeriesId) -> Option<StudyOutputStyle> {
            (series_id == self.series_id).then_some(self.style)
        }
    }

    struct OffsetTestStudies {
        series_id: SeriesId,
        values: Vec<StudyValue>,
        offset: i32,
    }

    impl StudyDataSource for OffsetTestStudies {
        fn output_values(&self, series_id: SeriesId) -> Option<&[StudyValue]> {
            (series_id == self.series_id).then_some(self.values.as_slice())
        }

        fn output_offset(&self, series_id: SeriesId) -> i32 {
            (series_id == self.series_id)
                .then_some(self.offset)
                .unwrap_or(0)
        }
    }

    #[test]
    fn visible_bounds_allow_partial_edge_bars() {
        assert_eq!(
            visible_bounds(
                100,
                LogicalRange {
                    from: -4.7,
                    to: 10.2
                }
            ),
            Some((0, 11))
        );
    }

    #[test]
    fn plot_drag_pans_price_only_after_manual_scale_activation() {
        let mut state = TvChartState::default();

        assert!(!should_pan_price_with_plot_drag(&state));

        state.price_scales.scale_mut(PriceScaleId::Right).zoom_at_y(
            2.0,
            200.0,
            0.0,
            400.0,
            PriceRange {
                min: 100.0,
                max: 200.0,
            },
        );

        assert!(should_pan_price_with_plot_drag(&state));

        state.price_scales.reset(PriceScaleId::Right);

        assert!(!should_pan_price_with_plot_drag(&state));
    }

    #[test]
    fn snapshot_keeps_volume_range_off_main_price_scale() {
        let mut state = TvChartState::default();
        state
            .time_scale
            .set_visible_logical_range(LogicalRange { from: 0.0, to: 1.0 }, 400.0);
        let bars = vec![
            OhlcBar {
                time: 1,
                open: 10.0,
                high: 11.0,
                low: 9.0,
                close: 10.0,
                volume: 100_000.0,
            },
            OhlcBar {
                time: 2,
                open: 12.0,
                high: 13.0,
                low: 11.0,
                close: 12.0,
                volume: 250_000.0,
            },
        ];

        let snapshot = build_snapshot(
            &state,
            &bars,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 300.0)),
        );

        assert_eq!(
            snapshot.auto_range,
            PriceRange {
                min: 9.0,
                max: 13.0
            }
        );
        assert_eq!(
            snapshot.volume_range,
            Some(PriceRange {
                min: 0.0,
                max: 262_500.0
            })
        );
        assert!(snapshot.price_range.max < 20.0);
    }

    #[test]
    fn volume_visibility_tracks_pane_and_series_state() {
        let mut state = TvChartState::default();
        state
            .time_scale
            .set_visible_logical_range(LogicalRange { from: 0.0, to: 1.0 }, 400.0);
        let bars = vec![
            OhlcBar {
                time: 1,
                open: 10.0,
                high: 11.0,
                low: 9.0,
                close: 10.0,
                volume: 100_000.0,
            },
            OhlcBar {
                time: 2,
                open: 12.0,
                high: 13.0,
                low: 11.0,
                close: 12.0,
                volume: 250_000.0,
            },
        ];
        let plot = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 300.0));

        assert!(state.volume_visible());
        assert!(build_snapshot(&state, &bars, plot).volume_range.is_some());

        state.set_volume_visible(false);

        assert!(!state.volume_visible());
        assert!(build_snapshot(&state, &bars, plot).volume_range.is_none());
    }

    #[test]
    fn snapshot_autoscales_main_price_study_outputs() {
        let mut state = TvChartState::default();
        state
            .time_scale
            .set_visible_logical_range(LogicalRange { from: 0.0, to: 1.0 }, 400.0);
        let series_id = SeriesId::new(10_000);
        state.series.add_or_replace(SeriesSpec::new(
            series_id,
            "SMA 20",
            SeriesKind::Line,
            SeriesValueSource::StudyOutput,
            PaneId::Main,
            PriceScaleId::Right,
        ));
        let studies = TestStudies {
            series_id,
            values: vec![StudyValue::Number(80.0), StudyValue::Number(120.0)],
        };
        let bars = vec![
            OhlcBar {
                time: 1,
                open: 99.0,
                high: 101.0,
                low: 98.0,
                close: 100.0,
                volume: 1.0,
            },
            OhlcBar {
                time: 2,
                open: 100.0,
                high: 102.0,
                low: 99.0,
                close: 101.0,
                volume: 1.0,
            },
        ];
        let plot = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 300.0));

        let snapshot = build_snapshot_with_studies(&state, &bars, plot, Some(&studies));

        assert_eq!(
            snapshot.auto_range,
            PriceRange {
                min: 80.0,
                max: 120.0
            }
        );
    }

    #[test]
    fn snapshot_autoscale_applies_study_output_offsets() {
        let mut state = TvChartState::default();
        state
            .time_scale
            .set_visible_logical_range(LogicalRange { from: 1.0, to: 1.0 }, 400.0);
        let series_id = SeriesId::new(10_000);
        state.series.add_or_replace(SeriesSpec::new(
            series_id,
            "SMA 20",
            SeriesKind::Line,
            SeriesValueSource::StudyOutput,
            PaneId::Main,
            PriceScaleId::Right,
        ));
        let studies = OffsetTestStudies {
            series_id,
            values: vec![StudyValue::Number(150.0), StudyValue::Number(80.0)],
            offset: 1,
        };
        let bars = vec![
            OhlcBar {
                time: 1,
                open: 99.0,
                high: 101.0,
                low: 98.0,
                close: 100.0,
                volume: 1.0,
            },
            OhlcBar {
                time: 2,
                open: 100.0,
                high: 102.0,
                low: 99.0,
                close: 101.0,
                volume: 1.0,
            },
        ];
        let plot = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 300.0));

        let snapshot = build_snapshot_with_studies(&state, &bars, plot, Some(&studies));

        assert_eq!(
            snapshot.auto_range,
            PriceRange {
                min: 99.0,
                max: 150.5
            }
        );
    }

    #[test]
    fn snapshot_skips_hidden_study_output_styles_for_autoscale() {
        let mut state = TvChartState::default();
        state
            .time_scale
            .set_visible_logical_range(LogicalRange { from: 0.0, to: 1.0 }, 400.0);
        let series_id = SeriesId::new(10_000);
        state.series.add_or_replace(SeriesSpec::new(
            series_id,
            "SMA 20",
            SeriesKind::Line,
            SeriesValueSource::StudyOutput,
            PaneId::Main,
            PriceScaleId::Right,
        ));
        let studies = StyledTestStudies {
            series_id,
            values: vec![StudyValue::Number(20.0), StudyValue::Number(200.0)],
            style: StudyOutputStyle::line(false, StudyColor::rgb(41, 98, 255), 1.5),
        };
        let bars = vec![
            OhlcBar {
                time: 1,
                open: 99.0,
                high: 101.0,
                low: 98.0,
                close: 100.0,
                volume: 1.0,
            },
            OhlcBar {
                time: 2,
                open: 100.0,
                high: 102.0,
                low: 99.0,
                close: 101.0,
                volume: 1.0,
            },
        ];
        let plot = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 300.0));

        let snapshot = build_snapshot_with_studies(&state, &bars, plot, Some(&studies));

        assert_eq!(
            snapshot.auto_range,
            PriceRange {
                min: 98.0,
                max: 102.0
            }
        );
    }

    #[test]
    fn snapshot_autoscales_separate_study_pane_outputs() {
        let mut state = TvChartState::default();
        state
            .time_scale
            .set_visible_logical_range(LogicalRange { from: 0.0, to: 1.0 }, 400.0);
        let series_id = SeriesId::new(10_000);
        state.series.add_or_replace(SeriesSpec::new(
            series_id,
            "RSI 14",
            SeriesKind::Line,
            SeriesValueSource::StudyOutput,
            PaneId::Study(7),
            PriceScaleId::Overlay,
        ));
        let studies = TestStudies {
            series_id,
            values: vec![StudyValue::Number(45.0), StudyValue::Number(65.0)],
        };
        let bars = vec![
            OhlcBar {
                time: 1,
                open: 99.0,
                high: 101.0,
                low: 98.0,
                close: 100.0,
                volume: 1.0,
            },
            OhlcBar {
                time: 2,
                open: 100.0,
                high: 102.0,
                low: 99.0,
                close: 101.0,
                volume: 1.0,
            },
        ];
        let plot = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 300.0));

        let snapshot = build_snapshot_with_studies(&state, &bars, plot, Some(&studies));

        assert_eq!(snapshot.study_ranges.len(), 1);
        assert_eq!(snapshot.study_ranges[0].id, PaneId::Study(7));
        assert_eq!(
            snapshot.study_ranges[0].rect,
            PriceRange {
                min: 0.0,
                max: 100.0,
            }
        );
    }

    #[test]
    fn snapshot_log_mode_reprojects_price_coordinates() {
        let mut state = TvChartState::default();
        state
            .time_scale
            .set_visible_logical_range(LogicalRange { from: 0.0, to: 2.0 }, 400.0);
        let bars = vec![
            OhlcBar {
                time: 1,
                open: 10.0,
                high: 11.0,
                low: 9.0,
                close: 10.0,
                volume: 1.0,
            },
            OhlcBar {
                time: 2,
                open: 100.0,
                high: 110.0,
                low: 90.0,
                close: 100.0,
                volume: 1.0,
            },
            OhlcBar {
                time: 3,
                open: 1_000.0,
                high: 1_100.0,
                low: 900.0,
                close: 1_000.0,
                volume: 1.0,
            },
        ];
        let plot = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(400.0, 400.0));
        let linear = build_snapshot(&state, &bars, plot);
        state
            .price_scales
            .scale_mut(PriceScaleId::Right)
            .set_value_mode(PriceScaleValueMode::Log);
        let log = build_snapshot(&state, &bars, plot);

        assert_eq!(log.transform.price_mode, PriceScaleValueMode::Log);
        assert!(
            (linear.transform.price_to_y(100.0) - log.transform.price_to_y(100.0)).abs() > 40.0,
            "log backend must alter candle y placement, not only labels"
        );
    }

    #[test]
    fn visible_base_modes_use_first_visible_close() {
        let bars = vec![
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
                open: 20.0,
                high: 21.0,
                low: 19.0,
                close: 20.5,
                volume: 1.0,
            },
            OhlcBar {
                time: 3,
                open: 30.0,
                high: 31.0,
                low: 29.0,
                close: 30.5,
                volume: 1.0,
            },
        ];
        let bounds = Some((1, 2));

        assert_eq!(visible_base_close(bounds, &bars), Some(20.5));
        assert_eq!(
            resolve_visible_base_price_mode(
                PriceScaleValueMode::Percent { base: 1.0 },
                bounds,
                &bars,
            ),
            PriceScaleValueMode::Percent { base: 20.5 }
        );
        assert_eq!(
            resolve_visible_base_price_mode(
                PriceScaleValueMode::Indexed { base: 1.0 },
                bounds,
                &bars,
            ),
            PriceScaleValueMode::Indexed { base: 20.5 }
        );
        assert_eq!(
            resolve_visible_base_price_mode(PriceScaleValueMode::Log, bounds, &bars),
            PriceScaleValueMode::Log
        );
    }
}
