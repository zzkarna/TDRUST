use eframe::egui;

use crate::studies::StudyDataSource;

use super::{
    BarSeries, DrawingId, DrawingStore, OhlcBar, PaneId, PaneRect,
    egui_renderer::{
        ChartLayout, EguiCandlestickRenderRequest, EguiChartRenderer, EguiDrawingRenderRequest,
        build_layout, paint_empty_message, paint_frame,
    },
    engine::{
        ChartSnapshot, TvChartState, prepare_chart_snapshot_with_studies,
        sync_chart_state_for_frame,
    },
    interaction::{handle_chart_input, show_price_scale_menu},
};

#[derive(Debug)]
pub struct ChartUiResponse {
    pub response: egui::Response,
    pub outer: egui::Rect,
    pub legend: egui::Rect,
    pub panes: Vec<PaneRect<egui::Rect>>,
    pub plot: egui::Rect,
    pub price_axis: egui::Rect,
    pub time_axis: egui::Rect,
}

impl ChartUiResponse {
    fn from_layout(response: egui::Response, layout: &ChartLayout) -> Self {
        Self {
            response,
            outer: layout.outer,
            legend: layout.legend,
            panes: layout.panes.clone(),
            plot: layout.plot,
            price_axis: layout.price_axis,
            time_axis: layout.time_axis,
        }
    }

    pub fn pane_rect(&self, pane: PaneId) -> Option<egui::Rect> {
        self.panes
            .iter()
            .find(|pane_rect| pane_rect.id == pane)
            .map(|pane_rect| pane_rect.rect)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChartWidgetSizing {
    min_size: egui::Vec2,
}

impl ChartWidgetSizing {
    pub fn standalone() -> Self {
        Self {
            min_size: egui::vec2(420.0, 420.0),
        }
    }

    pub fn embedded_shell_widget() -> Self {
        Self {
            min_size: egui::vec2(120.0, 96.0),
        }
    }

    fn desired_size(self, available: egui::Vec2) -> egui::Vec2 {
        egui::vec2(
            available.x.max(self.min_size.x),
            available.y.max(self.min_size.y),
        )
    }
}

impl Default for ChartWidgetSizing {
    fn default() -> Self {
        Self::standalone()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ChartDrawingLayer<'a> {
    pub drawings: &'a DrawingStore,
    pub selected: Option<DrawingId>,
    pub hovered: Option<DrawingId>,
    pub symbol_filter: Option<&'a str>,
    pub interval_seconds: Option<i64>,
    pub crosshair_magnet: Option<ChartCrosshairMagnet>,
    pub blocks_chart_input: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartCrosshairMagnetMode {
    Off,
    Weak,
    Strong,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChartCrosshairMagnet {
    pub mode: ChartCrosshairMagnetMode,
    pub weak_price_distance_px: f32,
}

pub fn show_candlestick_chart<B: BarSeries + ?Sized>(
    ui: &mut egui::Ui,
    state: &mut TvChartState,
    renderer: &mut EguiChartRenderer,
    _title: &str,
    bars: &B,
    studies: Option<&dyn StudyDataSource>,
    drawings: Option<ChartDrawingLayer<'_>>,
) -> ChartUiResponse {
    show_candlestick_chart_with_sizing(
        ui,
        state,
        renderer,
        _title,
        bars,
        studies,
        drawings,
        ChartWidgetSizing::standalone(),
    )
}

pub fn show_candlestick_chart_with_sizing<B: BarSeries + ?Sized>(
    ui: &mut egui::Ui,
    state: &mut TvChartState,
    renderer: &mut EguiChartRenderer,
    _title: &str,
    bars: &B,
    studies: Option<&dyn StudyDataSource>,
    drawings: Option<ChartDrawingLayer<'_>>,
    sizing: ChartWidgetSizing,
) -> ChartUiResponse {
    let desired = sizing.desired_size(ui.available_size());
    let (outer, response) = ui.allocate_exact_size(desired, egui::Sense::click_and_drag());
    let layout = build_layout(outer, &state.panes);
    let painter = ui.painter_at(outer);

    renderer.set_theme_generation(state.theme_generation());
    paint_frame(&painter, outer, state.theme());

    if bars.is_empty() {
        paint_empty_message(&painter, outer, state.theme());
        return ChartUiResponse::from_layout(response, &layout);
    }

    sync_chart_state_for_frame(state, bars, layout.plot);
    if !drawings.is_some_and(|layer| layer.blocks_chart_input) {
        handle_chart_input(ui, &response, state, &layout, bars);
    }
    let snapshot = prepare_chart_snapshot_with_studies(state, bars, layout.plot, studies);
    let hover = ui.input(|input| input.pointer.hover_pos());
    let crosshair_hover = drawings
        .and_then(|layer| {
            snapped_crosshair_hover(hover?, &layout, &snapshot, bars, layer.crosshair_magnet?)
        })
        .or(hover);
    renderer.paint_candlestick_scene(
        &painter,
        EguiCandlestickRenderRequest {
            layout: &layout,
            snapshot: snapshot.clone(),
            theme: state.theme(),
            series: &state.series,
            studies,
            bars,
            hover: crosshair_hover,
        },
    );
    if let Some(drawings) = drawings {
        renderer.paint_drawing_scene(
            &painter,
            EguiDrawingRenderRequest {
                layout: &layout,
                snapshot,
                drawings: drawings.drawings,
                bars,
                selected: drawings.selected,
                hovered: drawings.hovered,
                symbol_filter: drawings.symbol_filter,
                interval_seconds: drawings.interval_seconds,
            },
        );
    }
    show_price_scale_menu(ui, state, &layout);

    ChartUiResponse::from_layout(response, &layout)
}

fn snapped_crosshair_hover<B: BarSeries + ?Sized>(
    hover: egui::Pos2,
    layout: &ChartLayout,
    snapshot: &ChartSnapshot,
    bars: &B,
    magnet: ChartCrosshairMagnet,
) -> Option<egui::Pos2> {
    if magnet.mode == ChartCrosshairMagnetMode::Off
        || !layout.plot.contains(hover)
        || bars.is_empty()
    {
        return None;
    }
    let logical = snapshot.transform.x_to_logical(hover.x);
    if !logical.is_finite() || logical < -0.5 || logical > bars.len() as f64 - 0.5 {
        return None;
    }
    let index = logical
        .round()
        .clamp(0.0, bars.len().saturating_sub(1) as f64) as usize;
    let bar = bars.bar(index)?;
    let snapped_price = closest_ohlc_price(snapshot, hover.y, bar)?;
    let snapped_y = snapshot.transform.price_to_y(snapped_price);
    if magnet.mode == ChartCrosshairMagnetMode::Weak
        && (snapped_y - hover.y).abs() > magnet.weak_price_distance_px
    {
        return None;
    }
    Some(egui::pos2(
        snapshot.transform.logical_to_x(index as f64),
        snapped_y,
    ))
}

fn closest_ohlc_price(snapshot: &ChartSnapshot, y: f32, bar: OhlcBar) -> Option<f64> {
    [bar.open, bar.high, bar.low, bar.close]
        .into_iter()
        .filter(|price| price.is_finite())
        .min_by(|left, right| {
            let left_distance = (snapshot.transform.price_to_y(*left) - y).abs();
            let right_distance = (snapshot.transform.price_to_y(*right) - y).abs();
            left_distance.total_cmp(&right_distance)
        })
}

#[cfg(test)]
mod tests {
    use eframe::egui;

    use super::{
        ChartCrosshairMagnet, ChartCrosshairMagnetMode, ChartSnapshot, ChartWidgetSizing, OhlcBar,
        build_layout, snapped_crosshair_hover,
    };
    use crate::chart_core::{ChartTransform, LogicalRange, PaneModel, PriceRange};

    fn snapshot(plot: egui::Rect) -> ChartSnapshot {
        let price_range = PriceRange {
            min: 80.0,
            max: 120.0,
        };
        ChartSnapshot {
            visible_bounds: Some((0, 0)),
            auto_range: price_range,
            price_range,
            volume_range: None,
            study_ranges: Vec::new(),
            transform: ChartTransform::new(plot, LogicalRange { from: 0.0, to: 1.0 }, price_range),
        }
    }

    #[test]
    fn standalone_chart_sizing_preserves_existing_minimum() {
        assert_eq!(
            ChartWidgetSizing::standalone().desired_size(egui::vec2(200.0, 180.0)),
            egui::vec2(420.0, 420.0)
        );
    }

    #[test]
    fn embedded_chart_sizing_allows_shell_widget_resize() {
        assert_eq!(
            ChartWidgetSizing::embedded_shell_widget().desired_size(egui::vec2(180.0, 150.0)),
            egui::vec2(180.0, 150.0)
        );
        assert_eq!(
            ChartWidgetSizing::embedded_shell_widget().desired_size(egui::vec2(80.0, 70.0)),
            egui::vec2(120.0, 96.0)
        );
    }

    #[test]
    fn magnet_crosshair_snaps_to_nearest_ohlc_point() {
        let layout = build_layout(
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0)),
            &PaneModel::default(),
        );
        let snapshot = snapshot(layout.plot);
        let bars = vec![OhlcBar {
            time: 100,
            open: 90.0,
            high: 110.0,
            low: 82.0,
            close: 100.0,
            volume: 1.0,
        }];
        let high_y = snapshot.transform.price_to_y(110.0);
        let hover = egui::pos2(snapshot.transform.logical_to_x(0.2), high_y + 7.0);

        let snapped = snapped_crosshair_hover(
            hover,
            &layout,
            &snapshot,
            &bars,
            ChartCrosshairMagnet {
                mode: ChartCrosshairMagnetMode::Weak,
                weak_price_distance_px: 18.0,
            },
        )
        .expect("hover should snap near an OHLC value");

        assert_eq!(snapped.x, snapshot.transform.logical_to_x(0.0));
        assert_eq!(snapped.y, high_y);
    }

    #[test]
    fn weak_magnet_crosshair_leaves_distant_hover_unsnapped() {
        let layout = build_layout(
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0)),
            &PaneModel::default(),
        );
        let snapshot = snapshot(layout.plot);
        let bars = vec![OhlcBar {
            time: 100,
            open: 90.0,
            high: 110.0,
            low: 82.0,
            close: 100.0,
            volume: 1.0,
        }];
        let hover = egui::pos2(
            snapshot.transform.logical_to_x(0.2),
            snapshot.transform.price_to_y(115.0),
        );

        assert_eq!(
            snapped_crosshair_hover(
                hover,
                &layout,
                &snapshot,
                &bars,
                ChartCrosshairMagnet {
                    mode: ChartCrosshairMagnetMode::Weak,
                    weak_price_distance_px: 1.0,
                },
            ),
            None
        );
    }
}
