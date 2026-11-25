use eframe::egui::{self, Color32};

use crate::studies::StudyDataSource;

use super::{
    BarSeries, ChartColor, ChartSnapshot, ChartTheme, DrawingId, DrawingStore, PaneId, PaneModel,
    PaneRect, RenderCacheGenerationInputs, RenderCacheKey, RenderScene, RenderSceneCache,
    SceneColor, SceneFont, SceneFontFamily, SceneLayout, SceneLineStyle, ScenePoint,
    ScenePrimitive, SceneRect, SceneSnapshot, SceneStroke, SceneTextAnchor, SceneTransform,
    SeriesRegistry,
    scene::{
        ChartBaseSceneRequest, ChartOverlaySceneRequest, DrawingSceneRequest,
        build_chart_base_scene, build_chart_overlay_scene, build_drawing_scene,
    },
};

#[derive(Debug, Clone, Default)]
pub struct EguiChartRenderer {
    cache: RenderSceneCache,
    theme_generation: u64,
    study_generation: u64,
}

impl EguiChartRenderer {
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    pub fn set_theme_generation(&mut self, generation: u64) {
        self.theme_generation = generation;
    }

    pub fn set_study_generation(&mut self, generation: u64) {
        self.study_generation = generation;
    }

    pub fn theme_generation(&self) -> u64 {
        self.theme_generation
    }

    pub fn study_generation(&self) -> u64 {
        self.study_generation
    }

    pub(crate) fn paint_candlestick_scene<B: BarSeries + ?Sized>(
        &mut self,
        painter: &egui::Painter,
        request: EguiCandlestickRenderRequest<'_, B>,
    ) {
        let scene_layout = to_scene_layout(request.layout);
        let scene_snapshot = to_scene_snapshot(request.snapshot, &scene_layout);
        let key = RenderCacheKey::from_scene(
            &scene_layout,
            &scene_snapshot,
            RenderCacheGenerationInputs::new(
                request.bars.generation(),
                self.theme_generation,
                request.series.generation(),
                self.study_generation,
            ),
        );
        let base_scene = self.cache.get_or_build(key, || {
            build_chart_base_scene(ChartBaseSceneRequest {
                layout: scene_layout.clone(),
                snapshot: scene_snapshot.clone(),
                theme: request.theme,
                series: request.series,
                studies: request.studies,
                bars: request.bars,
            })
        });
        let overlay_scene = build_chart_overlay_scene(ChartOverlaySceneRequest {
            layout: scene_layout,
            snapshot: scene_snapshot,
            theme: request.theme,
            series: request.series,
            studies: request.studies,
            bars: request.bars,
            hover: request.hover.map(to_scene_point),
        });
        paint_scene(painter, &base_scene);
        paint_scene(painter, &overlay_scene);
    }

    pub(crate) fn paint_drawing_scene<B: BarSeries + ?Sized>(
        &self,
        painter: &egui::Painter,
        request: EguiDrawingRenderRequest<'_, B>,
    ) {
        let scene_layout = to_scene_layout(request.layout);
        let scene_snapshot = to_scene_snapshot(request.snapshot, &scene_layout);
        let scene = build_drawing_scene(DrawingSceneRequest {
            layout: scene_layout,
            snapshot: scene_snapshot,
            drawings: request.drawings,
            bars: request.bars,
            selected: request.selected,
            hovered: request.hovered,
            symbol_filter: request.symbol_filter,
            interval_seconds: request.interval_seconds,
        });
        paint_scene(painter, &scene);
    }
}

pub(crate) struct EguiCandlestickRenderRequest<'a, B: BarSeries + ?Sized> {
    pub layout: &'a ChartLayout,
    pub snapshot: ChartSnapshot,
    pub theme: &'a ChartTheme,
    pub series: &'a SeriesRegistry,
    pub studies: Option<&'a dyn StudyDataSource>,
    pub bars: &'a B,
    pub hover: Option<egui::Pos2>,
}

pub(crate) struct EguiDrawingRenderRequest<'a, B: BarSeries + ?Sized> {
    pub layout: &'a ChartLayout,
    pub snapshot: ChartSnapshot,
    pub drawings: &'a DrawingStore,
    pub bars: &'a B,
    pub selected: Option<DrawingId>,
    pub hovered: Option<DrawingId>,
    pub symbol_filter: Option<&'a str>,
    pub interval_seconds: Option<i64>,
}

#[derive(Debug, Clone)]
pub(crate) struct ChartLayout {
    pub outer: egui::Rect,
    pub legend: egui::Rect,
    pub panes: Vec<PaneRect<egui::Rect>>,
    pub plot: egui::Rect,
    pub volume: Option<egui::Rect>,
    pub price_axis: egui::Rect,
    pub time_axis: egui::Rect,
}

pub(crate) fn build_layout(outer: egui::Rect, panes: &PaneModel) -> ChartLayout {
    let pad = 8.0;
    let axis_width = 82.0;
    let time_axis_height = 26.0;
    let legend_height = 26.0;
    let inner = outer.shrink2(egui::vec2(pad, pad));
    let legend = egui::Rect::from_min_max(
        inner.left_top(),
        egui::pos2(inner.right(), inner.top() + legend_height),
    );
    let time_axis = egui::Rect::from_min_max(
        egui::pos2(inner.left(), inner.bottom() - time_axis_height),
        inner.right_bottom(),
    );
    let chart_bottom = time_axis.top();
    let pane_area = egui::Rect::from_min_max(
        egui::pos2(inner.left(), legend.bottom()),
        egui::pos2(inner.right() - axis_width, chart_bottom),
    );
    let pane_rects = build_pane_rects(pane_area, panes);
    let plot = pane_rects
        .iter()
        .find(|pane| pane.id == PaneId::Main)
        .map(|pane| pane.rect)
        .unwrap_or(pane_area);
    let volume = pane_rects
        .iter()
        .find(|pane| pane.id == PaneId::Volume)
        .map(|pane| pane.rect);
    let price_axis = egui::Rect::from_min_max(
        egui::pos2(pane_area.right(), pane_area.top()),
        egui::pos2(inner.right(), chart_bottom),
    );

    ChartLayout {
        outer,
        legend,
        panes: pane_rects,
        plot,
        volume,
        price_axis,
        time_axis,
    }
}

fn build_pane_rects(area: egui::Rect, panes: &PaneModel) -> Vec<PaneRect<egui::Rect>> {
    let panes = panes.normalized_visible_weights();
    if panes.is_empty() {
        return vec![PaneRect {
            id: PaneId::Main,
            rect: area,
        }];
    }

    let area_height = area.height();
    let total_min_height = panes
        .iter()
        .map(|(_, _, min_height)| min_height.max(0.0))
        .sum::<f32>();
    let available_weight_height = (area_height - total_min_height).max(0.0);
    let min_scale = if total_min_height > area_height && total_min_height > f32::EPSILON {
        area_height / total_min_height
    } else {
        1.0
    };
    let mut y = area.top();
    let mut remaining_height = area_height;
    let mut rects = Vec::with_capacity(panes.len());
    let last_index = panes.len().saturating_sub(1);
    for (index, (id, weight, min_height)) in panes.into_iter().enumerate() {
        let mut height = if index == last_index {
            remaining_height
        } else {
            (min_height.max(0.0) * min_scale + available_weight_height * weight)
                .min(remaining_height)
        };
        if index == last_index {
            height = height.max(0.0);
        }
        let rect = egui::Rect::from_min_max(
            egui::pos2(area.left(), y),
            egui::pos2(area.right(), y + height),
        );
        rects.push(PaneRect { id, rect });
        y += height;
        remaining_height = (area.bottom() - y).max(0.0);
    }

    if let Some(last) = rects.last_mut() {
        last.rect.max.y = area.bottom();
    }

    rects
}

#[cfg(test)]
pub(crate) fn pane_rects_for_test(
    area: egui::Rect,
    panes: &PaneModel,
) -> Vec<PaneRect<egui::Rect>> {
    build_pane_rects(area, panes)
}

#[cfg(test)]
mod layout_tests {
    use super::{PaneId, PaneModel, PaneRect, build_layout, pane_rects_for_test};
    use eframe::egui;

    #[test]
    fn pane_layout_allocates_main_and_volume_panes() {
        let area = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1_000.0, 500.0));
        let rects = pane_rects_for_test(area, &PaneModel::default());

        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].id, PaneId::Main);
        assert_eq!(rects[1].id, PaneId::Volume);
        assert_eq!(rects[0].rect.top(), area.top());
        assert_eq!(rects[1].rect.bottom(), area.bottom());
        assert!(rects[0].rect.height() > rects[1].rect.height());
    }

    #[test]
    fn pane_layout_hides_volume_when_disabled() {
        let mut panes = PaneModel::default();
        panes.set_visible(PaneId::Volume, false);
        let layout = build_layout(
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1_000.0, 600.0)),
            &panes,
        );

        assert_eq!(layout.panes.len(), 1);
        assert_eq!(layout.panes[0].id, PaneId::Main);
        assert!(layout.volume.is_none());
        assert_eq!(layout.plot.bottom(), layout.time_axis.top());
    }

    #[test]
    fn pane_layout_exposes_compatibility_aliases() {
        let layout = build_layout(
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1_000.0, 600.0)),
            &PaneModel::default(),
        );
        let main = layout
            .panes
            .iter()
            .find(|pane| pane.id == PaneId::Main)
            .map(|pane| pane.rect);
        let volume = layout
            .panes
            .iter()
            .find(|pane| pane.id == PaneId::Volume)
            .map(|pane| pane.rect);

        assert_eq!(Some(layout.plot), main);
        assert_eq!(layout.volume, volume);
        assert!(layout.price_axis.top() <= layout.plot.top());
    }

    #[test]
    fn pane_layout_respects_min_heights_when_space_allows() {
        let area = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1_000.0, 300.0));
        let rects = pane_rects_for_test(area, &PaneModel::default());

        assert!(rects[0].rect.height() >= 180.0);
        assert!(rects[1].rect.height() >= 54.0);
        assert_eq!(rects[0].rect.top(), area.top());
        assert_eq!(rects[1].rect.bottom(), area.bottom());
    }

    #[test]
    fn pane_rect_type_is_copyable_for_scene_bridges() {
        let pane = PaneRect {
            id: PaneId::Main,
            rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1.0, 1.0)),
        };

        assert_eq!(pane, pane);
    }
}

pub(crate) fn paint_frame(painter: &egui::Painter, outer: egui::Rect, theme: &ChartTheme) {
    painter.rect_filled(
        outer,
        0.0,
        to_egui_chart_color(theme.appearance.frame_background),
    );
    painter.rect_stroke(
        outer,
        0.0,
        egui::Stroke::new(1.0, to_egui_chart_color(theme.appearance.frame_border)),
        egui::StrokeKind::Inside,
    );
}

pub(crate) fn paint_empty_message(painter: &egui::Painter, outer: egui::Rect, theme: &ChartTheme) {
    painter.text(
        outer.center(),
        egui::Align2::CENTER_CENTER,
        "No candle data",
        egui::FontId::proportional(14.0),
        to_egui_chart_color(theme.appearance.empty_text),
    );
}

fn paint_scene(painter: &egui::Painter, scene: &RenderScene) {
    for primitive in scene.primitives() {
        match primitive {
            ScenePrimitive::FilledRect {
                rect,
                radius,
                color,
                clip,
            } => paint_filled_rect(painter, *clip, *rect, *radius, *color),
            ScenePrimitive::StrokedRect {
                rect,
                radius,
                stroke,
                clip,
            } => paint_stroked_rect(painter, *clip, *rect, *radius, *stroke),
            ScenePrimitive::Line {
                from,
                to,
                stroke,
                clip,
            } => paint_line(painter, *clip, *from, *to, *stroke),
            ScenePrimitive::Text {
                position,
                anchor,
                text,
                font,
                color,
                clip,
            } => paint_text(painter, *clip, *position, *anchor, text, *font, *color),
        }
    }
}

fn paint_filled_rect(
    painter: &egui::Painter,
    clip: Option<SceneRect>,
    rect: SceneRect,
    radius: f32,
    color: SceneColor,
) {
    if let Some(clip) = clip {
        painter.with_clip_rect(to_egui_rect(clip)).rect_filled(
            to_egui_rect(rect),
            radius,
            to_egui_color(color),
        );
    } else {
        painter.rect_filled(to_egui_rect(rect), radius, to_egui_color(color));
    }
}

fn paint_stroked_rect(
    painter: &egui::Painter,
    clip: Option<SceneRect>,
    rect: SceneRect,
    radius: f32,
    stroke: SceneStroke,
) {
    if stroke.line_style == SceneLineStyle::Solid {
        let stroke = to_egui_stroke(stroke);
        if let Some(clip) = clip {
            painter.with_clip_rect(to_egui_rect(clip)).rect_stroke(
                to_egui_rect(rect),
                radius,
                stroke,
                egui::StrokeKind::Inside,
            );
        } else {
            painter.rect_stroke(to_egui_rect(rect), radius, stroke, egui::StrokeKind::Inside);
        }
        return;
    }

    let rect = to_egui_rect(rect);
    let paint = |painter: &egui::Painter| {
        paint_styled_line_segment(painter, rect.left_top(), rect.right_top(), stroke);
        paint_styled_line_segment(painter, rect.right_top(), rect.right_bottom(), stroke);
        paint_styled_line_segment(painter, rect.right_bottom(), rect.left_bottom(), stroke);
        paint_styled_line_segment(painter, rect.left_bottom(), rect.left_top(), stroke);
    };
    if let Some(clip) = clip {
        let clipped = painter.with_clip_rect(to_egui_rect(clip));
        paint(&clipped);
    } else {
        paint(painter);
    }
}

fn paint_line(
    painter: &egui::Painter,
    clip: Option<SceneRect>,
    from: ScenePoint,
    to: ScenePoint,
    stroke: SceneStroke,
) {
    let from = to_egui_pos(from);
    let to = to_egui_pos(to);
    if let Some(clip) = clip {
        let clipped = painter.with_clip_rect(to_egui_rect(clip));
        paint_styled_line_segment(&clipped, from, to, stroke);
    } else {
        paint_styled_line_segment(painter, from, to, stroke);
    }
}

fn paint_styled_line_segment(
    painter: &egui::Painter,
    from: egui::Pos2,
    to: egui::Pos2,
    stroke: SceneStroke,
) {
    let egui_stroke = to_egui_stroke(stroke);
    if stroke.line_style == SceneLineStyle::Solid {
        painter.line_segment([from, to], egui_stroke);
        return;
    }

    let delta = to - from;
    let length = delta.length();
    if length <= f32::EPSILON {
        painter.circle_filled(from, (stroke.width * 0.5).max(1.0), egui_stroke.color);
        return;
    }

    let direction = delta / length;
    match stroke.line_style {
        SceneLineStyle::Solid => unreachable!("solid line style returned early"),
        SceneLineStyle::Dashed => {
            let dash = (stroke.width * 4.5).max(8.0);
            let gap = (stroke.width * 3.0).max(5.0);
            let mut start = 0.0;
            while start < length {
                let end = (start + dash).min(length);
                painter.line_segment(
                    [from + direction * start, from + direction * end],
                    egui_stroke,
                );
                start += dash + gap;
            }
        }
        SceneLineStyle::Dotted => {
            let radius = (stroke.width * 0.5).max(1.0);
            let gap = (stroke.width * 3.2).max(5.0);
            let mut offset = 0.0;
            while offset <= length {
                painter.circle_filled(from + direction * offset, radius, egui_stroke.color);
                offset += gap;
            }
        }
    }
}

fn paint_text(
    painter: &egui::Painter,
    clip: Option<SceneRect>,
    position: ScenePoint,
    anchor: SceneTextAnchor,
    text: &str,
    font: SceneFont,
    color: SceneColor,
) {
    if let Some(clip) = clip {
        painter.with_clip_rect(to_egui_rect(clip)).text(
            to_egui_pos(position),
            to_egui_anchor(anchor),
            text,
            to_egui_font(font),
            to_egui_color(color),
        );
    } else {
        painter.text(
            to_egui_pos(position),
            to_egui_anchor(anchor),
            text,
            to_egui_font(font),
            to_egui_color(color),
        );
    }
}

fn to_scene_layout(layout: &ChartLayout) -> SceneLayout {
    SceneLayout {
        outer: to_scene_rect(layout.outer),
        legend: to_scene_rect(layout.legend),
        panes: layout
            .panes
            .iter()
            .map(|pane| PaneRect {
                id: pane.id,
                rect: to_scene_rect(pane.rect),
            })
            .collect(),
        plot: to_scene_rect(layout.plot),
        volume: layout.volume.map(to_scene_rect),
        price_axis: to_scene_rect(layout.price_axis),
        time_axis: to_scene_rect(layout.time_axis),
    }
}

fn to_scene_snapshot(snapshot: ChartSnapshot, layout: &SceneLayout) -> SceneSnapshot {
    SceneSnapshot {
        visible_bounds: snapshot.visible_bounds,
        price_range: snapshot.price_range,
        volume_range: snapshot.volume_range,
        study_ranges: snapshot.study_ranges,
        transform: SceneTransform::new(
            layout.plot,
            snapshot.transform.time,
            snapshot.transform.price,
        )
        .with_price_mode(snapshot.transform.price_mode),
    }
}

fn to_scene_rect(rect: egui::Rect) -> SceneRect {
    SceneRect::from_min_max(to_scene_point(rect.min), to_scene_point(rect.max))
}

fn to_scene_point(point: egui::Pos2) -> ScenePoint {
    ScenePoint::new(point.x, point.y)
}

fn to_egui_rect(rect: SceneRect) -> egui::Rect {
    egui::Rect::from_min_max(to_egui_pos(rect.min), to_egui_pos(rect.max))
}

fn to_egui_pos(point: ScenePoint) -> egui::Pos2 {
    egui::pos2(point.x, point.y)
}

fn to_egui_color(color: SceneColor) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

pub(crate) fn to_egui_chart_color(color: ChartColor) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

fn to_egui_stroke(stroke: SceneStroke) -> egui::Stroke {
    egui::Stroke::new(stroke.width, to_egui_color(stroke.color))
}

fn to_egui_anchor(anchor: SceneTextAnchor) -> egui::Align2 {
    match anchor {
        SceneTextAnchor::LeftCenter => egui::Align2::LEFT_CENTER,
        SceneTextAnchor::RightCenter => egui::Align2::RIGHT_CENTER,
        SceneTextAnchor::Center => egui::Align2::CENTER_CENTER,
    }
}

fn to_egui_font(font: SceneFont) -> egui::FontId {
    match font.family {
        SceneFontFamily::Proportional => egui::FontId::proportional(font.size),
        SceneFontFamily::Monospace => egui::FontId::monospace(font.size),
    }
}
