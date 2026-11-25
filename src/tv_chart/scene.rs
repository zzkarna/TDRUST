use crate::studies::{StudyColor, StudyDataSource, StudyGuideStyle, StudyOutputStyle, StudyValue};

use super::{
    BarSeries, ChartColor, ChartTheme, DrawingAnchor, DrawingId, DrawingKind, DrawingLineStyle,
    DrawingSpec, DrawingStore, FibRetracementLabelPosition, FibRetracementLabelSide,
    FibRetracementSettings, LogicalRange, OhlcBar, OhlcField, PaneId, PaneRect,
    PositionDrawingSettings, PriceRange, PriceScale, PriceScaleValueMode, SeriesId, SeriesKind,
    SeriesRegistry, SeriesValueSource,
    time_scale::{format_time_axis_label, normalize_timestamp_seconds, visible_time_axis_ticks},
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScenePoint {
    pub x: f32,
    pub y: f32,
}

impl ScenePoint {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneRect {
    pub min: ScenePoint,
    pub max: ScenePoint,
}

impl SceneRect {
    pub const fn from_min_max(min: ScenePoint, max: ScenePoint) -> Self {
        Self { min, max }
    }

    pub fn from_center_size(center: ScenePoint, size: SceneSize) -> Self {
        let half_width = size.width * 0.5;
        let half_height = size.height * 0.5;
        Self {
            min: ScenePoint::new(center.x - half_width, center.y - half_height),
            max: ScenePoint::new(center.x + half_width, center.y + half_height),
        }
    }

    pub fn left(self) -> f32 {
        self.min.x
    }

    pub fn right(self) -> f32 {
        self.max.x
    }

    pub fn top(self) -> f32 {
        self.min.y
    }

    pub fn bottom(self) -> f32 {
        self.max.y
    }

    pub fn width(self) -> f32 {
        (self.right() - self.left()).max(0.0)
    }

    pub fn height(self) -> f32 {
        (self.bottom() - self.top()).max(0.0)
    }

    pub fn center(self) -> ScenePoint {
        ScenePoint::new(
            (self.left() + self.right()) * 0.5,
            (self.top() + self.bottom()) * 0.5,
        )
    }

    pub fn left_center(self) -> ScenePoint {
        ScenePoint::new(self.left(), self.center().y)
    }

    pub fn right_center(self) -> ScenePoint {
        ScenePoint::new(self.right(), self.center().y)
    }

    pub fn left_top(self) -> ScenePoint {
        self.min
    }

    pub fn right_top(self) -> ScenePoint {
        ScenePoint::new(self.right(), self.top())
    }

    pub fn left_bottom(self) -> ScenePoint {
        ScenePoint::new(self.left(), self.bottom())
    }

    pub fn right_bottom(self) -> ScenePoint {
        self.max
    }

    pub fn contains(self, point: ScenePoint) -> bool {
        point.x >= self.left()
            && point.x <= self.right()
            && point.y >= self.top()
            && point.y <= self.bottom()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneSize {
    pub width: f32,
    pub height: f32,
}

impl SceneSize {
    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl SceneColor {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

impl From<ChartColor> for SceneColor {
    fn from(color: ChartColor) -> Self {
        Self {
            r: color.r,
            g: color.g,
            b: color.b,
            a: color.a,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneStroke {
    pub width: f32,
    pub color: SceneColor,
    pub line_style: SceneLineStyle,
}

impl SceneStroke {
    pub const fn new(width: f32, color: SceneColor) -> Self {
        Self {
            width,
            color,
            line_style: SceneLineStyle::Solid,
        }
    }

    pub const fn with_line_style(mut self, line_style: SceneLineStyle) -> Self {
        self.line_style = line_style;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SceneLineStyle {
    Solid,
    Dashed,
    Dotted,
}

impl From<DrawingLineStyle> for SceneLineStyle {
    fn from(line_style: DrawingLineStyle) -> Self {
        match line_style {
            DrawingLineStyle::Solid => Self::Solid,
            DrawingLineStyle::Dashed => Self::Dashed,
            DrawingLineStyle::Dotted => Self::Dotted,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneFontFamily {
    Proportional,
    Monospace,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneFont {
    pub family: SceneFontFamily,
    pub size: f32,
}

impl SceneFont {
    pub const fn proportional(size: f32) -> Self {
        Self {
            family: SceneFontFamily::Proportional,
            size,
        }
    }

    pub const fn monospace(size: f32) -> Self {
        Self {
            family: SceneFontFamily::Monospace,
            size,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneTextAnchor {
    LeftCenter,
    RightCenter,
    Center,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ScenePrimitive {
    FilledRect {
        rect: SceneRect,
        radius: f32,
        color: SceneColor,
        clip: Option<SceneRect>,
    },
    StrokedRect {
        rect: SceneRect,
        radius: f32,
        stroke: SceneStroke,
        clip: Option<SceneRect>,
    },
    Line {
        from: ScenePoint,
        to: ScenePoint,
        stroke: SceneStroke,
        clip: Option<SceneRect>,
    },
    Text {
        position: ScenePoint,
        anchor: SceneTextAnchor,
        text: String,
        font: SceneFont,
        color: SceneColor,
        clip: Option<SceneRect>,
    },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RenderScene {
    primitives: Vec<ScenePrimitive>,
}

impl RenderScene {
    pub fn new() -> Self {
        Self {
            primitives: Vec::new(),
        }
    }

    pub fn push(&mut self, primitive: ScenePrimitive) {
        self.primitives.push(primitive);
    }

    pub fn extend(&mut self, other: RenderScene) {
        self.primitives.extend(other.primitives);
    }

    pub fn primitives(&self) -> &[ScenePrimitive] {
        &self.primitives
    }

    pub fn len(&self) -> usize {
        self.primitives.len()
    }

    pub fn is_empty(&self) -> bool {
        self.primitives.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderCacheKey {
    pub data_generation: u64,
    pub scale_generation: u64,
    pub layout_generation: u64,
    pub theme_generation: u64,
    pub series_generation: u64,
    pub study_generation: u64,
    pub drawing_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderCacheGenerationInputs {
    pub data_generation: u64,
    pub theme_generation: u64,
    pub series_generation: u64,
    pub study_generation: u64,
    pub drawing_generation: u64,
}

impl RenderCacheGenerationInputs {
    pub const fn new(
        data_generation: u64,
        theme_generation: u64,
        series_generation: u64,
        study_generation: u64,
    ) -> Self {
        Self {
            data_generation,
            theme_generation,
            series_generation,
            study_generation,
            drawing_generation: 0,
        }
    }

    pub const fn with_drawing_generation(mut self, drawing_generation: u64) -> Self {
        self.drawing_generation = drawing_generation;
        self
    }
}

impl RenderCacheKey {
    pub const fn new(
        data_generation: u64,
        scale_generation: u64,
        layout_generation: u64,
        theme_generation: u64,
        series_generation: u64,
        study_generation: u64,
    ) -> Self {
        Self {
            data_generation,
            scale_generation,
            layout_generation,
            theme_generation,
            series_generation,
            study_generation,
            drawing_generation: 0,
        }
    }

    pub fn from_scene(
        layout: &SceneLayout,
        snapshot: &SceneSnapshot,
        generations: RenderCacheGenerationInputs,
    ) -> Self {
        Self {
            data_generation: generations.data_generation,
            scale_generation: scale_generation(snapshot),
            layout_generation: layout_generation(layout),
            theme_generation: generations.theme_generation,
            series_generation: generations.series_generation,
            study_generation: generations.study_generation,
            drawing_generation: generations.drawing_generation,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RenderSceneCache {
    key: Option<RenderCacheKey>,
    scene: Option<RenderScene>,
}

impl RenderSceneCache {
    pub fn key(&self) -> Option<RenderCacheKey> {
        self.key
    }

    pub fn get_or_build(
        &mut self,
        key: RenderCacheKey,
        build: impl FnOnce() -> RenderScene,
    ) -> RenderScene {
        if self.key == Some(key)
            && let Some(scene) = &self.scene
        {
            return scene.clone();
        }

        let scene = build();
        self.key = Some(key);
        self.scene = Some(scene.clone());
        scene
    }

    pub fn clear(&mut self) {
        self.key = None;
        self.scene = None;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneLayout {
    pub outer: SceneRect,
    pub legend: SceneRect,
    pub panes: Vec<PaneRect<SceneRect>>,
    pub plot: SceneRect,
    pub volume: Option<SceneRect>,
    pub price_axis: SceneRect,
    pub time_axis: SceneRect,
}

impl SceneLayout {
    pub fn pane_rect(&self, pane: PaneId) -> Option<SceneRect> {
        self.panes
            .iter()
            .find(|pane_rect| pane_rect.id == pane)
            .map(|pane_rect| pane_rect.rect)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneTransform {
    pub plot: SceneRect,
    pub time: LogicalRange,
    pub price: PriceRange,
    pub price_mode: PriceScaleValueMode,
}

impl SceneTransform {
    pub const fn new(plot: SceneRect, time: LogicalRange, price: PriceRange) -> Self {
        Self {
            plot,
            time,
            price,
            price_mode: PriceScaleValueMode::Linear,
        }
    }

    pub fn with_price_mode(mut self, price_mode: PriceScaleValueMode) -> Self {
        self.price_mode = price_mode;
        self
    }

    pub fn logical_to_x(self, logical: f64) -> f32 {
        self.plot.left()
            + ((logical - self.time.from + 0.5) / self.time.span()) as f32 * self.plot.width()
    }

    pub fn x_to_logical(self, x: f32) -> f64 {
        let fraction = ((x - self.plot.left()) / self.plot.width().max(1.0)).clamp(0.0, 1.0);
        self.time.from - 0.5 + f64::from(fraction) * self.time.span()
    }

    pub fn price_to_y(self, price: f64) -> f32 {
        PriceScale::price_to_y_with_mode(
            self.price,
            self.price_mode,
            price,
            f64::from(self.plot.top()),
            f64::from(self.plot.height()),
        ) as f32
    }

    pub fn y_to_price(self, y: f32) -> f64 {
        PriceScale::y_to_price_with_mode(
            self.price,
            self.price_mode,
            f64::from(y),
            f64::from(self.plot.top()),
            f64::from(self.plot.height()),
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneSnapshot {
    pub visible_bounds: Option<(usize, usize)>,
    pub price_range: PriceRange,
    pub volume_range: Option<PriceRange>,
    pub study_ranges: Vec<PaneRect<PriceRange>>,
    pub transform: SceneTransform,
}

impl SceneSnapshot {
    pub fn study_range(&self, pane: PaneId) -> Option<PriceRange> {
        self.study_ranges
            .iter()
            .find(|pane_range| pane_range.id == pane)
            .map(|pane_range| pane_range.rect)
    }
}

pub struct ChartSceneRequest<'a, B: BarSeries + ?Sized> {
    pub layout: SceneLayout,
    pub snapshot: SceneSnapshot,
    pub theme: &'a ChartTheme,
    pub series: &'a SeriesRegistry,
    pub studies: Option<&'a dyn StudyDataSource>,
    pub bars: &'a B,
    pub hover: Option<ScenePoint>,
}

pub struct ChartBaseSceneRequest<'a, B: BarSeries + ?Sized> {
    pub layout: SceneLayout,
    pub snapshot: SceneSnapshot,
    pub theme: &'a ChartTheme,
    pub series: &'a SeriesRegistry,
    pub studies: Option<&'a dyn StudyDataSource>,
    pub bars: &'a B,
}

pub struct ChartOverlaySceneRequest<'a, B: BarSeries + ?Sized> {
    pub layout: SceneLayout,
    pub snapshot: SceneSnapshot,
    pub theme: &'a ChartTheme,
    pub series: &'a SeriesRegistry,
    pub studies: Option<&'a dyn StudyDataSource>,
    pub bars: &'a B,
    pub hover: Option<ScenePoint>,
}

pub fn build_candlestick_scene<B: BarSeries + ?Sized>(
    layout: SceneLayout,
    snapshot: SceneSnapshot,
    _title: &str,
    bars: &B,
    hover: Option<ScenePoint>,
) -> RenderScene {
    let series = SeriesRegistry::price_volume_default();
    let theme = ChartTheme::default();
    build_chart_scene(ChartSceneRequest {
        layout,
        snapshot,
        theme: &theme,
        series: &series,
        studies: None,
        bars,
        hover,
    })
}

pub fn build_chart_scene<B: BarSeries + ?Sized>(request: ChartSceneRequest<'_, B>) -> RenderScene {
    let mut scene = build_chart_base_scene(ChartBaseSceneRequest {
        layout: request.layout.clone(),
        snapshot: request.snapshot.clone(),
        theme: request.theme,
        series: request.series,
        studies: request.studies,
        bars: request.bars,
    });
    scene.extend(build_chart_overlay_scene(ChartOverlaySceneRequest {
        layout: request.layout,
        snapshot: request.snapshot,
        theme: request.theme,
        series: request.series,
        studies: request.studies,
        bars: request.bars,
        hover: request.hover,
    }));
    scene
}

pub fn build_candlestick_base_scene<B: BarSeries + ?Sized>(
    layout: SceneLayout,
    snapshot: SceneSnapshot,
    bars: &B,
) -> RenderScene {
    let series = SeriesRegistry::price_volume_default();
    let theme = ChartTheme::default();
    build_chart_base_scene(ChartBaseSceneRequest {
        layout,
        snapshot,
        theme: &theme,
        series: &series,
        studies: None,
        bars,
    })
}

pub fn build_chart_base_scene<B: BarSeries + ?Sized>(
    request: ChartBaseSceneRequest<'_, B>,
) -> RenderScene {
    let mut scene = RenderScene::new();
    push_background(&mut scene, &request.layout, request.theme);
    push_price_grid(
        &mut scene,
        &request.layout,
        request.snapshot.transform,
        request.theme,
    );
    push_time_grid(
        &mut scene,
        &request.layout,
        request.snapshot.transform,
        request.bars,
        request.theme,
    );
    push_study_pane_guides_and_labels(&mut scene, &request);
    push_registered_series(&mut scene, &request);
    if has_visible_main_price_series(request.series) {
        push_last_price(
            &mut scene,
            &request.layout,
            request.snapshot.transform,
            request.bars,
            request.theme,
        );
    }
    scene
}

pub fn build_candlestick_overlay_scene<B: BarSeries + ?Sized>(
    layout: SceneLayout,
    snapshot: SceneSnapshot,
    bars: &B,
    hover: Option<ScenePoint>,
) -> RenderScene {
    let series = SeriesRegistry::price_volume_default();
    let theme = ChartTheme::default();
    build_chart_overlay_scene(ChartOverlaySceneRequest {
        layout,
        snapshot,
        theme: &theme,
        series: &series,
        studies: None,
        bars,
        hover,
    })
}

pub fn build_chart_overlay_scene<B: BarSeries + ?Sized>(
    request: ChartOverlaySceneRequest<'_, B>,
) -> RenderScene {
    let mut scene = RenderScene::new();
    if has_visible_main_price_series(request.series) {
        push_crosshair_and_legend(
            &mut scene,
            &request.layout,
            request.snapshot.transform,
            request.bars,
            request.hover,
            request.theme,
        );
    }
    scene
}

pub struct DrawingSceneRequest<'a, B: BarSeries + ?Sized> {
    pub layout: SceneLayout,
    pub snapshot: SceneSnapshot,
    pub drawings: &'a DrawingStore,
    pub bars: &'a B,
    pub selected: Option<DrawingId>,
    pub hovered: Option<DrawingId>,
    pub symbol_filter: Option<&'a str>,
    pub interval_seconds: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawingHitTestRequest<'a, B: BarSeries + ?Sized> {
    pub layout: &'a SceneLayout,
    pub snapshot: &'a SceneSnapshot,
    pub drawings: &'a DrawingStore,
    pub bars: &'a B,
    pub point: ScenePoint,
    pub tolerance: f32,
    pub symbol_filter: Option<&'a str>,
    pub selected: Option<DrawingId>,
    pub hovered: Option<DrawingId>,
    pub interval_seconds: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawingHitTarget {
    Body,
    Edge,
    Anchor(usize),
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawingHit {
    pub drawing_id: DrawingId,
    pub target: DrawingHitTarget,
    pub distance: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DrawingGeometry {
    drawing_id: DrawingId,
    target: DrawingHitTarget,
    distance_bias: f32,
    shape: DrawingGeometryShape,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DrawingHitCandidate {
    hit: DrawingHit,
    z_order: i32,
    selected: bool,
    hovered: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum DrawingGeometryShape {
    Line {
        from: ScenePoint,
        to: ScenePoint,
        width: f32,
    },
    Rect {
        rect: SceneRect,
        edge_width: f32,
        filled: bool,
    },
    Text {
        rect: SceneRect,
    },
    Point {
        center: ScenePoint,
        radius: f32,
    },
}

pub fn build_drawing_scene<B: BarSeries + ?Sized>(
    request: DrawingSceneRequest<'_, B>,
) -> RenderScene {
    let mut scene = RenderScene::new();
    for drawing in request.drawings.visible() {
        if !drawing_matches_symbol_filter(drawing, request.symbol_filter) {
            continue;
        }
        if let Some(pane) = drawing_pane_rect(&request.layout, drawing.pane) {
            let transform = drawing_pane_transform(pane, &request.snapshot, drawing.pane);
            push_drawing(
                &mut scene,
                drawing,
                pane,
                transform,
                request.bars,
                request.interval_seconds,
                request.selected == Some(drawing.id),
                request.hovered == Some(drawing.id),
            );
        }
    }
    scene
}

pub fn hit_test_drawings<B: BarSeries + ?Sized>(
    request: DrawingHitTestRequest<'_, B>,
) -> Option<DrawingHit> {
    request
        .drawings
        .visible()
        .filter(|drawing| drawing_matches_symbol_filter(drawing, request.symbol_filter))
        .filter(|drawing| drawing.selectable)
        .filter_map(|drawing| {
            let pane = drawing_pane_rect(request.layout, drawing.pane)?;
            let transform = drawing_pane_transform(pane, request.snapshot, drawing.pane);
            Some((drawing, pane, transform))
        })
        .flat_map(|(drawing, pane, transform)| {
            drawing_geometries(
                drawing,
                pane,
                transform,
                request.bars,
                request.interval_seconds,
            )
            .into_iter()
            .filter_map(|geometry| {
                let distance = geometry.distance_to(request.point)?;
                (distance <= request.tolerance + geometry.hit_padding()).then_some(
                    DrawingHitCandidate {
                        hit: DrawingHit {
                            drawing_id: geometry.drawing_id,
                            target: geometry.target,
                            distance: distance + geometry.distance_bias,
                        },
                        z_order: drawing.z_order,
                        selected: request.selected == Some(drawing.id),
                        hovered: request.hovered == Some(drawing.id),
                    },
                )
            })
        })
        .min_by(compare_drawing_hit_candidates)
        .map(|candidate| candidate.hit)
}

fn compare_drawing_hit_candidates(
    left: &DrawingHitCandidate,
    right: &DrawingHitCandidate,
) -> std::cmp::Ordering {
    left.hit
        .distance
        .total_cmp(&right.hit.distance)
        .then_with(|| selected_anchor_hit(*right).cmp(&selected_anchor_hit(*left)))
        .then_with(|| selected_or_hovered_hit(*right).cmp(&selected_or_hovered_hit(*left)))
        .then_with(|| right.z_order.cmp(&left.z_order))
        .then_with(|| right.hit.drawing_id.cmp(&left.hit.drawing_id))
}

fn selected_anchor_hit(candidate: DrawingHitCandidate) -> bool {
    candidate.selected && matches!(candidate.hit.target, DrawingHitTarget::Anchor(_))
}

fn selected_or_hovered_hit(candidate: DrawingHitCandidate) -> bool {
    candidate.selected || candidate.hovered
}

fn drawing_matches_symbol_filter(drawing: &DrawingSpec, symbol_filter: Option<&str>) -> bool {
    match (symbol_filter, drawing.symbol.as_deref()) {
        (Some(expected), Some(actual)) => actual == expected,
        (Some(_), None) => false,
        (None, _) => true,
    }
}

fn push_drawing<B: BarSeries + ?Sized>(
    scene: &mut RenderScene,
    drawing: &DrawingSpec,
    pane: SceneRect,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
    selected: bool,
    hovered: bool,
) {
    let stroke = drawing_scene_stroke(drawing, selected, hovered);
    match drawing.kind {
        DrawingKind::HorizontalLine => {
            let Some(y) = drawing_price_y(drawing.anchors[0], transform, bars, interval_seconds)
            else {
                return;
            };
            let from = ScenePoint::new(pane.left(), y);
            let to = ScenePoint::new(pane.right(), y);
            scene.push(ScenePrimitive::Line {
                from,
                to,
                stroke,
                clip: Some(pane),
            });
            if selected {
                push_selection_handle(scene, pane, from);
                push_selection_handle(scene, pane, to);
            }
        }
        DrawingKind::HorizontalRay => {
            let Some(origin_model) =
                drawing_anchor_model_point(drawing.anchors[0], transform, bars, interval_seconds)
            else {
                return;
            };
            let Some((visible_from, visible_to)) =
                visible_horizontal_ray_segment(origin_model, transform)
            else {
                return;
            };
            scene.push(ScenePrimitive::Line {
                from: visible_from,
                to: visible_to,
                stroke,
                clip: Some(pane),
            });
            if selected {
                if let Some(origin) = model_point_to_scene(origin_model, transform) {
                    push_selection_handle(scene, pane, origin);
                }
            }
        }
        DrawingKind::VerticalLine => {
            let Some(x) = drawing_time_x(drawing.anchors[0], transform, bars, interval_seconds)
            else {
                return;
            };
            let from = ScenePoint::new(x, pane.top());
            let to = ScenePoint::new(x, pane.bottom());
            scene.push(ScenePrimitive::Line {
                from,
                to,
                stroke,
                clip: Some(pane),
            });
            if selected {
                push_selection_handle(scene, pane, from);
                push_selection_handle(scene, pane, to);
            }
        }
        DrawingKind::TrendLine => {
            let Some((from_model, to_model)) =
                drawing_two_model_points(drawing, transform, bars, interval_seconds)
            else {
                return;
            };
            let Some((visible_from, visible_to)) =
                visible_trend_segment(from_model, to_model, transform)
            else {
                return;
            };
            scene.push(ScenePrimitive::Line {
                from: visible_from,
                to: visible_to,
                stroke,
                clip: Some(pane),
            });
            if selected {
                if let Some(from) = model_point_to_scene(from_model, transform) {
                    push_selection_handle(scene, pane, from);
                }
                if let Some(to) = model_point_to_scene(to_model, transform) {
                    push_selection_handle(scene, pane, to);
                }
            }
        }
        DrawingKind::Ray => {
            let Some((from_model, through_model)) =
                drawing_two_model_points(drawing, transform, bars, interval_seconds)
            else {
                return;
            };
            let Some((visible_from, visible_to)) =
                visible_ray_segment(from_model, through_model, transform)
            else {
                return;
            };
            scene.push(ScenePrimitive::Line {
                from: visible_from,
                to: visible_to,
                stroke,
                clip: Some(pane),
            });
            if selected {
                if let Some(from) = model_point_to_scene(from_model, transform) {
                    push_selection_handle(scene, pane, from);
                }
                if let Some(through) = model_point_to_scene(through_model, transform) {
                    push_selection_handle(scene, pane, through);
                }
            }
        }
        DrawingKind::Brush => {
            let points = drawing_model_polyline_points(drawing, transform, bars, interval_seconds);
            for segment in points.windows(2) {
                scene.push(ScenePrimitive::Line {
                    from: segment[0],
                    to: segment[1],
                    stroke,
                    clip: Some(pane),
                });
            }
        }
        DrawingKind::FibRetracement => {
            let settings = drawing.fib_retracement_settings();
            let Some((from_model, to_model)) =
                drawing_two_model_points(drawing, transform, bars, interval_seconds)
            else {
                return;
            };
            let Some(from) = model_point_to_scene(from_model, transform) else {
                return;
            };
            let Some(to) = model_point_to_scene(to_model, transform) else {
                return;
            };
            let levels = fib_retracement_level_segments(
                from_model, to_model, pane, transform, &settings, stroke,
            );
            if settings.background_visible {
                for band in fib_retracement_background_bands(&levels, &settings) {
                    scene.push(ScenePrimitive::FilledRect {
                        rect: band,
                        radius: 0.0,
                        color: settings.background_color.into(),
                        clip: Some(pane),
                    });
                }
            }
            if settings.trend_line_visible {
                scene.push(ScenePrimitive::Line {
                    from,
                    to,
                    stroke,
                    clip: Some(pane),
                });
            }
            let mut label_y_positions = Vec::new();
            for level in levels {
                scene.push(ScenePrimitive::Line {
                    from: level.from,
                    to: level.to,
                    stroke: level.stroke,
                    clip: Some(pane),
                });
                if let Some(text) =
                    fib_retracement_level_label(&settings, level.value, level.price, transform)
                {
                    let (position, anchor) = fib_retracement_label_placement(
                        level.from,
                        level.to,
                        &settings,
                        drawing.style.text_size,
                    );
                    if !fib_label_has_vertical_room(
                        position.y,
                        &label_y_positions,
                        drawing.style.text_size,
                    ) {
                        continue;
                    }
                    label_y_positions.push(position.y);
                    scene.push(ScenePrimitive::Text {
                        position,
                        anchor,
                        text,
                        font: SceneFont::proportional(drawing.style.text_size),
                        color: drawing.style.text_color.into(),
                        clip: Some(pane),
                    });
                }
            }
            if selected {
                push_selection_handle(scene, pane, from);
                push_selection_handle(scene, pane, to);
            }
        }
        DrawingKind::LongPosition | DrawingKind::ShortPosition => {
            push_position_drawing(
                scene,
                drawing,
                pane,
                transform,
                bars,
                interval_seconds,
                stroke,
                selected,
                hovered,
            );
        }
        DrawingKind::Rectangle => {
            let Some((from, to)) =
                drawing_two_points(drawing, pane, transform, bars, interval_seconds)
            else {
                return;
            };
            let rect = normalized_rect(from, to);
            if let Some(fill_color) = drawing.style.fill_color {
                scene.push(ScenePrimitive::FilledRect {
                    rect,
                    radius: 0.0,
                    color: fill_color.into(),
                    clip: Some(pane),
                });
            }
            scene.push(ScenePrimitive::StrokedRect {
                rect,
                radius: 0.0,
                stroke,
                clip: Some(pane),
            });
            if selected {
                for corner in rectangle_corner_points(from, to) {
                    push_selection_handle(scene, pane, corner);
                }
            }
        }
        DrawingKind::Text | DrawingKind::AnchoredText | DrawingKind::PriceNote => {
            let Some(position) =
                drawing_text_position(drawing, pane, transform, bars, interval_seconds)
            else {
                return;
            };
            if drawing.kind == DrawingKind::PriceNote {
                let text = drawing_text(drawing, transform, position);
                let rect = text_rect(position, SceneTextAnchor::LeftCenter, &text, drawing);
                scene.push(ScenePrimitive::FilledRect {
                    rect,
                    radius: 2.0,
                    color: drawing
                        .style
                        .fill_color
                        .unwrap_or_else(|| ChartColor::rgba(20, 24, 28, 220))
                        .into(),
                    clip: Some(pane),
                });
                scene.push(ScenePrimitive::StrokedRect {
                    rect,
                    radius: 2.0,
                    stroke,
                    clip: Some(pane),
                });
                scene.push(ScenePrimitive::Text {
                    position: rect.center(),
                    anchor: SceneTextAnchor::Center,
                    text,
                    font: SceneFont::proportional(drawing.style.text_size),
                    color: drawing.style.text_color.into(),
                    clip: Some(pane),
                });
            } else {
                scene.push(ScenePrimitive::Text {
                    position,
                    anchor: SceneTextAnchor::LeftCenter,
                    text: drawing_text(drawing, transform, position),
                    font: SceneFont::proportional(drawing.style.text_size),
                    color: drawing.style.text_color.into(),
                    clip: Some(pane),
                });
            }
        }
    }
}

fn drawing_geometries<B: BarSeries + ?Sized>(
    drawing: &DrawingSpec,
    pane: SceneRect,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Vec<DrawingGeometry> {
    let width = drawing.style.stroke_width.max(0.5);
    match drawing.kind {
        DrawingKind::HorizontalLine => {
            drawing_price_y(drawing.anchors[0], transform, bars, interval_seconds)
                .map(|y| {
                    vec![DrawingGeometry::line(
                        drawing.id,
                        ScenePoint::new(pane.left(), y),
                        ScenePoint::new(pane.right(), y),
                        width,
                    )]
                })
                .unwrap_or_default()
        }
        DrawingKind::HorizontalRay => {
            let Some(origin_model) =
                drawing_anchor_model_point(drawing.anchors[0], transform, bars, interval_seconds)
            else {
                return Vec::new();
            };
            let mut geometries = if let Some((visible_from, visible_to)) =
                visible_horizontal_ray_segment(origin_model, transform)
            {
                vec![DrawingGeometry::line(
                    drawing.id,
                    visible_from,
                    visible_to,
                    width,
                )]
            } else {
                Vec::new()
            };
            if let Some(origin) = model_point_to_scene(origin_model, transform) {
                geometries.push(DrawingGeometry::anchor(drawing.id, 0, origin));
            }
            geometries
        }
        DrawingKind::VerticalLine => {
            drawing_time_x(drawing.anchors[0], transform, bars, interval_seconds)
                .map(|x| {
                    vec![DrawingGeometry::line(
                        drawing.id,
                        ScenePoint::new(x, pane.top()),
                        ScenePoint::new(x, pane.bottom()),
                        width,
                    )]
                })
                .unwrap_or_default()
        }
        DrawingKind::TrendLine | DrawingKind::Ray => {
            let Some((from_model, to_model)) =
                drawing_two_model_points(drawing, transform, bars, interval_seconds)
            else {
                return Vec::new();
            };
            let visible = if drawing.kind == DrawingKind::Ray {
                visible_ray_segment(from_model, to_model, transform)
            } else {
                visible_trend_segment(from_model, to_model, transform)
            };
            let mut geometries = if let Some((visible_from, visible_to)) = visible {
                vec![DrawingGeometry::line(
                    drawing.id,
                    visible_from,
                    visible_to,
                    width,
                )]
            } else {
                Vec::new()
            };
            if let Some(from) = model_point_to_scene(from_model, transform) {
                geometries.push(DrawingGeometry::anchor(drawing.id, 0, from));
            }
            if let Some(second) = model_point_to_scene(to_model, transform) {
                geometries.push(DrawingGeometry::anchor(drawing.id, 1, second));
            }
            geometries
        }
        DrawingKind::Brush => {
            drawing_model_polyline_points(drawing, transform, bars, interval_seconds)
                .windows(2)
                .map(|segment| DrawingGeometry::line(drawing.id, segment[0], segment[1], width))
                .collect()
        }
        DrawingKind::FibRetracement => {
            let settings = drawing.fib_retracement_settings();
            let Some((from_model, to_model)) =
                drawing_two_model_points(drawing, transform, bars, interval_seconds)
            else {
                return Vec::new();
            };
            let Some(from) = model_point_to_scene(from_model, transform) else {
                return Vec::new();
            };
            let Some(to) = model_point_to_scene(to_model, transform) else {
                return Vec::new();
            };
            let mut geometries = Vec::new();
            if settings.trend_line_visible {
                geometries.push(DrawingGeometry::line(drawing.id, from, to, width));
            }
            geometries.extend(
                fib_retracement_level_segments(
                    from_model,
                    to_model,
                    pane,
                    transform,
                    &settings,
                    SceneStroke::new(width, drawing.style.stroke_color.into())
                        .with_line_style(drawing.style.line_style.into()),
                )
                .into_iter()
                .map(|segment| DrawingGeometry::line(drawing.id, segment.from, segment.to, width)),
            );
            geometries.push(DrawingGeometry::anchor(drawing.id, 0, from));
            geometries.push(DrawingGeometry::anchor(drawing.id, 1, to));
            geometries
        }
        DrawingKind::LongPosition | DrawingKind::ShortPosition => {
            position_drawing_geometries(drawing, pane, transform, bars, interval_seconds, width)
        }
        DrawingKind::Rectangle => {
            let Some((from, to)) =
                drawing_two_points(drawing, pane, transform, bars, interval_seconds)
            else {
                return Vec::new();
            };
            let mut geometries = vec![DrawingGeometry::rect(
                drawing.id,
                normalized_rect(from, to),
                width,
                drawing.style.fill_color.is_some(),
            )];
            geometries.extend(
                rectangle_corner_points(from, to)
                    .into_iter()
                    .enumerate()
                    .map(|(index, corner)| DrawingGeometry::anchor(drawing.id, index, corner)),
            );
            geometries
        }
        DrawingKind::Text | DrawingKind::AnchoredText | DrawingKind::PriceNote => {
            let Some(position) =
                drawing_text_position(drawing, pane, transform, bars, interval_seconds)
            else {
                return Vec::new();
            };
            let anchor = if drawing.kind == DrawingKind::PriceNote {
                SceneTextAnchor::Center
            } else {
                SceneTextAnchor::LeftCenter
            };
            vec![
                DrawingGeometry::text(
                    drawing.id,
                    text_rect(
                        position,
                        anchor,
                        &drawing_text(drawing, transform, position),
                        drawing,
                    ),
                ),
                DrawingGeometry::anchor(drawing.id, 0, position),
            ]
        }
    }
}

fn push_selection_handle(scene: &mut RenderScene, pane: SceneRect, center: ScenePoint) {
    let size = 7.0;
    let rect = SceneRect::from_min_max(
        ScenePoint::new(center.x - size * 0.5, center.y - size * 0.5),
        ScenePoint::new(center.x + size * 0.5, center.y + size * 0.5),
    );
    scene.push(ScenePrimitive::FilledRect {
        rect,
        radius: 1.5,
        color: SceneColor::rgba(34, 38, 44, 235),
        clip: Some(pane),
    });
    scene.push(ScenePrimitive::StrokedRect {
        rect,
        radius: 1.5,
        stroke: SceneStroke::new(1.2, SceneColor::rgb(242, 242, 242)),
        clip: Some(pane),
    });
}

impl DrawingGeometry {
    fn line(drawing_id: DrawingId, from: ScenePoint, to: ScenePoint, width: f32) -> Self {
        Self {
            drawing_id,
            target: DrawingHitTarget::Edge,
            distance_bias: 0.0,
            shape: DrawingGeometryShape::Line { from, to, width },
        }
    }

    fn rect(drawing_id: DrawingId, rect: SceneRect, edge_width: f32, filled: bool) -> Self {
        Self {
            drawing_id,
            target: if filled {
                DrawingHitTarget::Body
            } else {
                DrawingHitTarget::Edge
            },
            distance_bias: 0.0,
            shape: DrawingGeometryShape::Rect {
                rect,
                edge_width,
                filled,
            },
        }
    }

    fn text(drawing_id: DrawingId, rect: SceneRect) -> Self {
        Self {
            drawing_id,
            target: DrawingHitTarget::Text,
            distance_bias: -0.25,
            shape: DrawingGeometryShape::Text { rect },
        }
    }

    fn anchor(drawing_id: DrawingId, index: usize, center: ScenePoint) -> Self {
        Self {
            drawing_id,
            target: DrawingHitTarget::Anchor(index),
            distance_bias: -1.0,
            shape: DrawingGeometryShape::Point {
                center,
                radius: 5.5,
            },
        }
    }

    fn hit_padding(self) -> f32 {
        match self.shape {
            DrawingGeometryShape::Line { width, .. } => width * 0.5,
            DrawingGeometryShape::Rect { edge_width, .. } => edge_width * 0.5,
            DrawingGeometryShape::Text { .. } => 0.0,
            DrawingGeometryShape::Point { .. } => 0.0,
        }
    }

    fn distance_to(self, point: ScenePoint) -> Option<f32> {
        match self.shape {
            DrawingGeometryShape::Line { from, to, .. } => {
                Some(point_line_distance(point, from, to))
            }
            DrawingGeometryShape::Rect { rect, filled, .. } => {
                if filled && rect.contains(point) {
                    Some(0.0)
                } else {
                    Some(rect_edge_distance(point, rect))
                }
            }
            DrawingGeometryShape::Text { rect } => rect
                .contains(point)
                .then_some(0.0)
                .or_else(|| Some(rect_edge_distance(point, rect))),
            DrawingGeometryShape::Point { center, radius } => {
                let distance = ((point.x - center.x).powi(2) + (point.y - center.y).powi(2)).sqrt();
                Some((distance - radius).max(0.0))
            }
        }
    }
}

fn drawing_pane_rect(layout: &SceneLayout, pane: PaneId) -> Option<SceneRect> {
    layout.pane_rect(pane).or(match pane {
        PaneId::Main => Some(layout.plot),
        PaneId::Volume => layout.volume,
        PaneId::Study(_) => None,
    })
}

fn drawing_pane_transform(
    pane: SceneRect,
    snapshot: &SceneSnapshot,
    pane_id: PaneId,
) -> SceneTransform {
    match pane_id {
        PaneId::Main => snapshot.transform,
        PaneId::Volume => SceneTransform::new(
            pane,
            snapshot.transform.time,
            snapshot.volume_range.unwrap_or(snapshot.price_range),
        )
        .with_price_mode(PriceScaleValueMode::Linear),
        PaneId::Study(_) => SceneTransform::new(
            pane,
            snapshot.transform.time,
            snapshot
                .study_range(pane_id)
                .unwrap_or(PriceRange { min: 0.0, max: 1.0 }),
        )
        .with_price_mode(PriceScaleValueMode::Linear),
    }
}

fn drawing_scene_stroke(drawing: &DrawingSpec, selected: bool, hovered: bool) -> SceneStroke {
    let width = drawing.style.stroke_width
        + if selected {
            1.0
        } else if hovered {
            0.5
        } else {
            0.0
        };
    SceneStroke::new(width.max(0.5), drawing.style.stroke_color.into())
        .with_line_style(drawing.style.line_style.into())
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct FibRetracementLevelSegment {
    value: f64,
    price: f64,
    from: ScenePoint,
    to: ScenePoint,
    stroke: SceneStroke,
}

fn fib_retracement_level_segments(
    from: DrawingModelPoint,
    to: DrawingModelPoint,
    pane: SceneRect,
    transform: SceneTransform,
    settings: &FibRetracementSettings,
    base_stroke: SceneStroke,
) -> Vec<FibRetracementLevelSegment> {
    let Some((left, right)) = fib_retracement_level_x_range(from, to, pane, transform, settings)
    else {
        return Vec::new();
    };
    settings
        .levels
        .iter()
        .copied()
        .filter(|level| level.visible && level.is_valid())
        .filter_map(|level| {
            let price = fib_retracement_level_price(from.price, to.price, level.value, settings);
            if !price_is_visible_in_transform(price, transform) {
                return None;
            }
            let y = transform.price_to_y(price);
            (y.is_finite() && y >= pane.top() && y <= pane.bottom()).then(|| {
                FibRetracementLevelSegment {
                    value: level.value,
                    price,
                    from: ScenePoint::new(left, y),
                    to: ScenePoint::new(right, y),
                    stroke: SceneStroke::new(base_stroke.width, level.color.into())
                        .with_line_style(base_stroke.line_style),
                }
            })
        })
        .collect()
}

fn price_is_visible_in_transform(price: f64, transform: SceneTransform) -> bool {
    let coordinate_range = transform.price_mode.coordinate_range(transform.price);
    let Some(coordinate) = transform.price_mode.coordinate(price) else {
        return false;
    };
    let epsilon = coordinate_range.span().abs() * 1e-9;
    coordinate >= coordinate_range.min - epsilon && coordinate <= coordinate_range.max + epsilon
}

fn fib_retracement_background_bands(
    levels: &[FibRetracementLevelSegment],
    settings: &FibRetracementSettings,
) -> Vec<SceneRect> {
    if levels.len() < 2 || settings.background_color.a == 0 {
        return Vec::new();
    }
    let mut levels = levels.to_vec();
    levels.sort_by(|a, b| a.from.y.total_cmp(&b.from.y));
    levels
        .windows(2)
        .filter_map(|pair| {
            let top = pair[0];
            let bottom = pair[1];
            let min_y = top.from.y.min(bottom.from.y);
            let max_y = top.from.y.max(bottom.from.y);
            if (max_y - min_y).abs() < f32::EPSILON {
                return None;
            }
            let left = top.from.x.min(top.to.x).min(bottom.from.x.min(bottom.to.x));
            let right = top.from.x.max(top.to.x).max(bottom.from.x.max(bottom.to.x));
            (right > left).then(|| {
                SceneRect::from_min_max(ScenePoint::new(left, min_y), ScenePoint::new(right, max_y))
            })
        })
        .collect()
}

fn fib_retracement_level_x_range(
    from: DrawingModelPoint,
    to: DrawingModelPoint,
    pane: SceneRect,
    transform: SceneTransform,
    settings: &FibRetracementSettings,
) -> Option<(f32, f32)> {
    let from = model_point_to_scene(from, transform)?;
    let to = model_point_to_scene(to, transform)?;
    let left = if settings.extend_left {
        pane.left()
    } else {
        from.x.min(to.x)
    };
    let right = if settings.extend_right {
        pane.right()
    } else {
        from.x.max(to.x)
    };
    (left.is_finite() && right.is_finite() && right >= left).then_some((left, right))
}

fn fib_retracement_level_price(
    from_price: f64,
    to_price: f64,
    value: f64,
    settings: &FibRetracementSettings,
) -> f64 {
    from_price + (to_price - from_price) * fib_retracement_effective_level_value(settings, value)
}

fn fib_retracement_effective_level_value(settings: &FibRetracementSettings, value: f64) -> f64 {
    if settings.reverse { value } else { 1.0 - value }
}

fn fib_retracement_label_placement(
    level_start: ScenePoint,
    level_end: ScenePoint,
    settings: &FibRetracementSettings,
    text_size: f32,
) -> (ScenePoint, SceneTextAnchor) {
    let y_offset = match settings.label_position {
        FibRetracementLabelPosition::Above => -(text_size * 0.65 + 2.0),
        FibRetracementLabelPosition::OnLine => 0.0,
        FibRetracementLabelPosition::Below => text_size * 0.65 + 2.0,
    };
    match settings.label_side {
        FibRetracementLabelSide::Left => (
            ScenePoint::new(level_start.x + 6.0, level_start.y + y_offset),
            SceneTextAnchor::LeftCenter,
        ),
        FibRetracementLabelSide::Right => (
            ScenePoint::new(level_end.x - 6.0, level_end.y + y_offset),
            SceneTextAnchor::RightCenter,
        ),
    }
}

fn fib_retracement_level_label(
    settings: &FibRetracementSettings,
    value: f64,
    price: f64,
    transform: SceneTransform,
) -> Option<String> {
    let mut parts = Vec::new();
    if settings.labels_visible {
        parts.push(format_fib_level_value(value));
    }
    if settings.prices_visible {
        parts.push(format_price_for_mode(transform.price_mode, price));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn fib_label_has_vertical_room(y: f32, existing: &[f32], text_size: f32) -> bool {
    let min_spacing = (text_size * 1.35).max(12.0);
    existing
        .iter()
        .all(|existing_y| (y - *existing_y).abs() >= min_spacing)
}

fn format_fib_level_value(value: f64) -> String {
    let mut formatted = format!("{value:.3}");
    while formatted.contains('.') && formatted.ends_with('0') {
        formatted.pop();
    }
    if formatted.ends_with('.') {
        formatted.pop();
    }
    formatted
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PositionDirection {
    Long,
    Short,
}

const POSITION_WIDTH_HANDLE_INDEX: usize = 3;
const POSITION_LABEL_PADDING_X: f32 = 10.0;
const POSITION_LABEL_PADDING_Y: f32 = 6.0;
const POSITION_LABEL_MIN_FONT_SIZE: f32 = 6.5;
const POSITION_LABEL_FULL_SIZE_WIDTH: f32 = 260.0;
const POSITION_LABEL_WIDTH_EXTENSION: f32 = 1.85;
const POSITION_LABEL_PANE_MARGIN: f32 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct PositionDrawingLayout {
    direction: PositionDirection,
    settings: PositionDrawingSettings,
    entry: DrawingModelPoint,
    target: DrawingModelPoint,
    stop: DrawingModelPoint,
    entry_point: ScenePoint,
    target_point: ScenePoint,
    stop_point: ScenePoint,
    left: f32,
    right: f32,
}

impl PositionDrawingLayout {
    fn entry_handle(self) -> ScenePoint {
        ScenePoint::new(self.left, self.entry_point.y)
    }

    fn target_handle(self) -> ScenePoint {
        ScenePoint::new(self.left, self.target_point.y)
    }

    fn stop_handle(self) -> ScenePoint {
        ScenePoint::new(self.left, self.stop_point.y)
    }

    fn width_handle(self) -> ScenePoint {
        ScenePoint::new(self.right, self.entry_point.y)
    }

    fn left_logical(self) -> f64 {
        self.entry
            .logical
            .min(self.target.logical)
            .min(self.stop.logical)
    }

    fn right_logical(self) -> f64 {
        self.entry
            .logical
            .max(self.target.logical)
            .max(self.stop.logical)
    }

    fn profit_rect(self) -> SceneRect {
        SceneRect::from_min_max(
            ScenePoint::new(self.left, self.target_point.y.min(self.entry_point.y)),
            ScenePoint::new(self.right, self.target_point.y.max(self.entry_point.y)),
        )
    }

    fn risk_rect(self) -> SceneRect {
        SceneRect::from_min_max(
            ScenePoint::new(self.left, self.stop_point.y.min(self.entry_point.y)),
            ScenePoint::new(self.right, self.stop_point.y.max(self.entry_point.y)),
        )
    }
}

fn push_position_drawing<B: BarSeries + ?Sized>(
    scene: &mut RenderScene,
    drawing: &DrawingSpec,
    pane: SceneRect,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
    stroke: SceneStroke,
    selected: bool,
    hovered: bool,
) {
    let Some(layout) = position_drawing_layout(drawing, pane, transform, bars, interval_seconds)
    else {
        return;
    };
    let profit_rect = layout.profit_rect();
    let risk_rect = layout.risk_rect();
    scene.push(ScenePrimitive::FilledRect {
        rect: profit_rect,
        radius: 0.0,
        color: layout.settings.profit_fill_color.into(),
        clip: Some(pane),
    });
    scene.push(ScenePrimitive::FilledRect {
        rect: risk_rect,
        radius: 0.0,
        color: layout.settings.risk_fill_color.into(),
        clip: Some(pane),
    });
    if let Some((rect, color)) = position_open_pnl_rect(&layout, transform, bars) {
        scene.push(ScenePrimitive::FilledRect {
            rect,
            radius: 0.0,
            color,
            clip: Some(pane),
        });
    }

    let entry_from = ScenePoint::new(layout.left, layout.entry_point.y);
    let entry_to = ScenePoint::new(layout.right, layout.entry_point.y);
    let target_from = ScenePoint::new(layout.left, layout.target_point.y);
    let target_to = ScenePoint::new(layout.right, layout.target_point.y);
    let stop_from = ScenePoint::new(layout.left, layout.stop_point.y);
    let stop_to = ScenePoint::new(layout.right, layout.stop_point.y);
    let target_stroke = position_region_stroke(
        stroke,
        layout.settings.profit_fill_color,
        drawing.style.line_style,
    );
    let stop_stroke = position_region_stroke(
        stroke,
        layout.settings.risk_fill_color,
        drawing.style.line_style,
    );

    scene.push(ScenePrimitive::Line {
        from: target_from,
        to: target_to,
        stroke: target_stroke,
        clip: Some(pane),
    });
    scene.push(ScenePrimitive::Line {
        from: entry_from,
        to: entry_to,
        stroke,
        clip: Some(pane),
    });
    scene.push(ScenePrimitive::Line {
        from: stop_from,
        to: stop_to,
        stroke: stop_stroke,
        clip: Some(pane),
    });

    if selected || hovered {
        push_position_labels(scene, drawing, pane, transform, &layout);
    }

    if selected {
        push_selection_handle(scene, pane, layout.target_handle());
        push_selection_handle(scene, pane, layout.entry_handle());
        push_selection_handle(scene, pane, layout.stop_handle());
        push_selection_handle(scene, pane, layout.width_handle());
    }
}

fn position_drawing_geometries<B: BarSeries + ?Sized>(
    drawing: &DrawingSpec,
    pane: SceneRect,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
    width: f32,
) -> Vec<DrawingGeometry> {
    let Some(layout) = position_drawing_layout(drawing, pane, transform, bars, interval_seconds)
    else {
        return Vec::new();
    };
    let entry_from = ScenePoint::new(layout.left, layout.entry_point.y);
    let entry_to = ScenePoint::new(layout.right, layout.entry_point.y);
    let target_from = ScenePoint::new(layout.left, layout.target_point.y);
    let target_to = ScenePoint::new(layout.right, layout.target_point.y);
    let stop_from = ScenePoint::new(layout.left, layout.stop_point.y);
    let stop_to = ScenePoint::new(layout.right, layout.stop_point.y);
    vec![
        DrawingGeometry::rect(
            drawing.id,
            SceneRect::from_min_max(
                ScenePoint::new(layout.left, layout.target_point.y.min(layout.entry_point.y)),
                ScenePoint::new(
                    layout.right,
                    layout.target_point.y.max(layout.entry_point.y),
                ),
            ),
            width,
            true,
        ),
        DrawingGeometry::rect(
            drawing.id,
            SceneRect::from_min_max(
                ScenePoint::new(layout.left, layout.stop_point.y.min(layout.entry_point.y)),
                ScenePoint::new(layout.right, layout.stop_point.y.max(layout.entry_point.y)),
            ),
            width,
            true,
        ),
        DrawingGeometry::line(drawing.id, target_from, target_to, width),
        DrawingGeometry::line(drawing.id, entry_from, entry_to, width),
        DrawingGeometry::line(drawing.id, stop_from, stop_to, width),
        DrawingGeometry::anchor(drawing.id, 1, layout.target_handle()),
        DrawingGeometry::anchor(drawing.id, 0, layout.entry_handle()),
        DrawingGeometry::anchor(drawing.id, 2, layout.stop_handle()),
        DrawingGeometry::anchor(
            drawing.id,
            POSITION_WIDTH_HANDLE_INDEX,
            layout.width_handle(),
        ),
    ]
}

fn position_drawing_layout<B: BarSeries + ?Sized>(
    drawing: &DrawingSpec,
    _pane: SceneRect,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<PositionDrawingLayout> {
    let direction = position_direction_for_kind(drawing.kind)?;
    let settings = drawing.position_settings();
    let (entry, target, stop) =
        drawing_three_model_points(drawing, transform, bars, interval_seconds)?;
    if !position_direction_is_valid(direction, entry.price, target.price, stop.price) {
        return None;
    }
    let entry_point = model_point_to_scene(entry, transform)?;
    let target_point = model_point_to_scene(target, transform)?;
    let stop_point = model_point_to_scene(stop, transform)?;
    let left = entry_point.x.min(target_point.x).min(stop_point.x);
    let right = entry_point.x.max(target_point.x).max(stop_point.x);
    (right > left).then_some(PositionDrawingLayout {
        direction,
        settings,
        entry,
        target,
        stop,
        entry_point,
        target_point,
        stop_point,
        left,
        right,
    })
}

fn position_direction_for_kind(kind: DrawingKind) -> Option<PositionDirection> {
    match kind {
        DrawingKind::LongPosition => Some(PositionDirection::Long),
        DrawingKind::ShortPosition => Some(PositionDirection::Short),
        _ => None,
    }
}

fn position_direction_is_valid(
    direction: PositionDirection,
    entry: f64,
    target: f64,
    stop: f64,
) -> bool {
    match direction {
        PositionDirection::Long => target > entry && stop < entry,
        PositionDirection::Short => target < entry && stop > entry,
    }
}

fn position_region_stroke(
    base: SceneStroke,
    fill_color: ChartColor,
    line_style: DrawingLineStyle,
) -> SceneStroke {
    let mut color = fill_color;
    color.a = 230;
    SceneStroke::new(base.width, color.into()).with_line_style(line_style.into())
}

fn position_open_pnl_rect<B: BarSeries + ?Sized>(
    layout: &PositionDrawingLayout,
    transform: SceneTransform,
    bars: &B,
) -> Option<(SceneRect, SceneColor)> {
    let outcome = position_open_pnl_outcome(layout, bars)?;
    if (outcome.price - layout.entry.price).abs() <= f64::EPSILON {
        return None;
    }
    let level_y = transform.price_to_y(outcome.price);
    let left = transform
        .logical_to_x(outcome.start_logical)
        .clamp(layout.left, layout.right);
    let right = transform
        .logical_to_x(outcome.logical)
        .clamp(layout.left, layout.right);
    if right <= left {
        return None;
    }
    let rect = SceneRect::from_min_max(
        ScenePoint::new(left, level_y.min(layout.entry_point.y)),
        ScenePoint::new(right, level_y.max(layout.entry_point.y)),
    );
    Some((rect, position_open_pnl_color(outcome.color)))
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PositionOpenPnlOutcome {
    start_logical: f64,
    logical: f64,
    price: f64,
    color: ChartColor,
}

fn position_open_pnl_outcome<B: BarSeries + ?Sized>(
    layout: &PositionDrawingLayout,
    bars: &B,
) -> Option<PositionOpenPnlOutcome> {
    if bars.is_empty() {
        return None;
    }
    let left = layout.left_logical();
    let right = layout.right_logical();
    if !left.is_finite() || !right.is_finite() || right <= left {
        return None;
    }
    let last_index = bars.len().saturating_sub(1);
    let start = left.floor().max(0.0) as usize;
    let end = right.ceil().clamp(0.0, last_index as f64) as usize;
    let mut entry_start_logical = None;
    let mut last_inside = None;
    if start <= end && start <= last_index {
        for index in start..=end.min(last_index) {
            let Some(bar) = bars.bar(index) else {
                continue;
            };
            let logical = index as f64;
            if logical < left || logical > right {
                continue;
            }
            if entry_start_logical.is_none() {
                if !position_bar_touches_price(bar, layout.entry.price) {
                    continue;
                }
                entry_start_logical = Some(logical);
            }
            let start_logical = entry_start_logical?;
            if let Some(outcome) = position_closed_pnl_outcome(layout, start_logical, logical, bar)
            {
                return Some(outcome);
            }
            last_inside = Some(PositionOpenPnlOutcome {
                start_logical,
                logical,
                price: position_open_pnl_price(layout, bar.close),
                color: position_open_pnl_color_for_price(layout, bar.close),
            });
        }
    }
    if let Some(outcome) = last_inside {
        return Some(outcome);
    }
    None
}

fn position_closed_pnl_outcome(
    layout: &PositionDrawingLayout,
    start_logical: f64,
    logical: f64,
    bar: OhlcBar,
) -> Option<PositionOpenPnlOutcome> {
    match layout.direction {
        PositionDirection::Long if bar.high >= layout.target.price => {
            Some(PositionOpenPnlOutcome {
                start_logical,
                logical,
                price: layout.target.price,
                color: layout.settings.profit_fill_color,
            })
        }
        PositionDirection::Long if bar.low <= layout.stop.price => Some(PositionOpenPnlOutcome {
            start_logical,
            logical,
            price: layout.stop.price,
            color: layout.settings.risk_fill_color,
        }),
        PositionDirection::Short if bar.low <= layout.target.price => {
            Some(PositionOpenPnlOutcome {
                start_logical,
                logical,
                price: layout.target.price,
                color: layout.settings.profit_fill_color,
            })
        }
        PositionDirection::Short if bar.high >= layout.stop.price => Some(PositionOpenPnlOutcome {
            start_logical,
            logical,
            price: layout.stop.price,
            color: layout.settings.risk_fill_color,
        }),
        _ => None,
    }
}

fn position_bar_touches_price(bar: OhlcBar, price: f64) -> bool {
    price.is_finite() && bar.low <= price && bar.high >= price
}

fn position_open_pnl_price(layout: &PositionDrawingLayout, price: f64) -> f64 {
    match layout.direction {
        PositionDirection::Long => price.clamp(layout.stop.price, layout.target.price),
        PositionDirection::Short => price.clamp(layout.target.price, layout.stop.price),
    }
}

fn position_open_pnl_color_for_price(layout: &PositionDrawingLayout, price: f64) -> ChartColor {
    match layout.direction {
        PositionDirection::Long if price >= layout.entry.price => layout.settings.profit_fill_color,
        PositionDirection::Long => layout.settings.risk_fill_color,
        PositionDirection::Short if price <= layout.entry.price => {
            layout.settings.profit_fill_color
        }
        PositionDirection::Short => layout.settings.risk_fill_color,
    }
}

fn position_open_pnl_color(mut color: ChartColor) -> SceneColor {
    color.a = color.a.saturating_add(56).max(104);
    color.into()
}

fn push_position_labels(
    scene: &mut RenderScene,
    drawing: &DrawingSpec,
    pane: SceneRect,
    transform: SceneTransform,
    layout: &PositionDrawingLayout,
) {
    let Some(entry_label) = position_entry_label(&layout.settings, layout, transform) else {
        return;
    };
    let target_label = position_level_label(
        &layout.settings,
        "Target",
        layout.entry.price,
        layout.target.price,
        transform,
    );
    let stop_label = position_level_label(
        &layout.settings,
        "Stop",
        layout.entry.price,
        layout.stop.price,
        transform,
    );
    let label_texts = [
        Some(entry_label.as_str()),
        target_label.as_deref(),
        stop_label.as_deref(),
    ];
    let Some(font) = position_label_font_for_layout(layout, drawing.style.text_size, &label_texts)
    else {
        return;
    };
    let text_color = drawing.style.text_color.into();
    let center_x = (layout.left + layout.right) * 0.5;
    let mut occupied_labels = Vec::new();
    let stats_size = position_label_size(&entry_label, font);
    if let Some(stats_position) = position_stats_label_position(layout, stats_size) {
        if let Some(rect) = position_label_rect(pane, stats_position, stats_size) {
            push_position_label_box(
                scene,
                pane,
                rect,
                entry_label,
                font,
                text_color,
                position_opaque_color(layout.settings.profit_fill_color),
            );
            occupied_labels.push(rect);
        }
    }
    if let Some(target_label) = target_label {
        let label_size = position_label_size(&target_label, font);
        let rect = position_label_rect(
            pane,
            ScenePoint::new(
                center_x,
                position_level_label_y(layout.entry_point.y, layout.target_point.y),
            ),
            label_size,
        );
        if let Some(rect) = rect
            && !position_label_overlaps_existing(rect, &occupied_labels)
        {
            push_position_label_box(
                scene,
                pane,
                rect,
                target_label,
                font,
                text_color,
                position_opaque_color(layout.settings.profit_fill_color),
            );
            occupied_labels.push(rect);
        }
    }
    if let Some(stop_label) = stop_label {
        let label_size = position_label_size(&stop_label, font);
        let rect = position_label_rect(
            pane,
            ScenePoint::new(
                center_x,
                position_level_label_y(layout.entry_point.y, layout.stop_point.y),
            ),
            label_size,
        );
        if let Some(rect) = rect
            && !position_label_overlaps_existing(rect, &occupied_labels)
        {
            push_position_label_box(
                scene,
                pane,
                rect,
                stop_label,
                font,
                text_color,
                position_opaque_color(layout.settings.risk_fill_color),
            );
            occupied_labels.push(rect);
        }
    }
}

fn position_stats_label_position(
    layout: &PositionDrawingLayout,
    label_size: SceneSize,
) -> Option<ScenePoint> {
    let profit_rect = layout.profit_rect();
    let risk_rect = layout.risk_rect();
    let rect = if position_label_fits_vertically(risk_rect, label_size) {
        risk_rect
    } else if position_label_fits_vertically(profit_rect, label_size) {
        profit_rect
    } else {
        return None;
    };
    Some(ScenePoint::new(
        (layout.left + layout.right) * 0.5,
        position_label_center_y_in_rect(rect, label_size.height),
    ))
}

fn position_label_center_y_in_rect(rect: SceneRect, label_height: f32) -> f32 {
    let half = label_height * 0.5;
    let inset = (label_height * 0.14).clamp(2.0, POSITION_LABEL_PADDING_Y);
    let min_y = rect.min.y.min(rect.max.y) + half + inset;
    let max_y = rect.min.y.max(rect.max.y) - half - inset;
    if min_y <= max_y {
        (rect.center().y).clamp(min_y, max_y)
    } else {
        rect.center().y
    }
}

fn position_level_label_y(entry_y: f32, level_y: f32) -> f32 {
    let offset = 16.0;
    if level_y < entry_y {
        level_y - offset
    } else {
        level_y + offset
    }
}

fn push_position_label_box(
    scene: &mut RenderScene,
    pane: SceneRect,
    rect: SceneRect,
    text: String,
    font: SceneFont,
    text_color: SceneColor,
    fill_color: SceneColor,
) {
    scene.push(ScenePrimitive::FilledRect {
        rect,
        radius: 5.0,
        color: fill_color,
        clip: Some(pane),
    });
    scene.push(ScenePrimitive::StrokedRect {
        rect,
        radius: 5.0,
        stroke: SceneStroke::new(1.0, SceneColor::rgba(255, 255, 255, 210)),
        clip: Some(pane),
    });
    scene.push(ScenePrimitive::Text {
        position: rect.center(),
        anchor: SceneTextAnchor::Center,
        text,
        font,
        color: text_color,
        clip: Some(pane),
    });
}

fn position_label_size(text: &str, font: SceneFont) -> SceneSize {
    let line_count = text.lines().count().max(1) as f32;
    let max_chars = text
        .lines()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0) as f32;
    SceneSize::new(
        max_chars * font.size * 0.58 + position_label_padding_x(font) * 2.0,
        line_count * font.size * 1.2 + position_label_padding_y(font) * 2.0,
    )
}

fn position_label_font_for_layout(
    layout: &PositionDrawingLayout,
    base_size: f32,
    label_texts: &[Option<&str>],
) -> Option<SceneFont> {
    let drawing_width = (layout.right - layout.left).max(0.0);
    if !drawing_width.is_finite() || drawing_width <= 0.0 || !base_size.is_finite() {
        return None;
    }
    let base_font = SceneFont::proportional(base_size);
    let max_base_width = label_texts
        .iter()
        .flatten()
        .map(|text| position_label_size(text, base_font).width)
        .fold(0.0_f32, f32::max);
    if max_base_width <= 0.0 {
        return None;
    }
    let width_scale = (drawing_width * POSITION_LABEL_WIDTH_EXTENSION / max_base_width).min(1.0);
    let projection_scale = (drawing_width / POSITION_LABEL_FULL_SIZE_WIDTH).clamp(0.55, 1.0);
    let font_size = base_size * width_scale.min(projection_scale);
    (font_size >= POSITION_LABEL_MIN_FONT_SIZE).then(|| SceneFont::proportional(font_size))
}

fn position_label_padding_x(font: SceneFont) -> f32 {
    (font.size * 0.75).clamp(4.0, POSITION_LABEL_PADDING_X)
}

fn position_label_padding_y(font: SceneFont) -> f32 {
    (font.size * 0.45).clamp(3.0, POSITION_LABEL_PADDING_Y)
}

fn position_label_fits_vertically(rect: SceneRect, size: SceneSize) -> bool {
    rect.height() >= size.height + 4.0
}

fn position_label_rect(pane: SceneRect, center: ScenePoint, size: SceneSize) -> Option<SceneRect> {
    let half_width = size.width * 0.5;
    if center.x - half_width < pane.left() + POSITION_LABEL_PANE_MARGIN
        || center.x + half_width > pane.right() - POSITION_LABEL_PANE_MARGIN
    {
        return None;
    }
    let mut center = center;
    let half_height = size.height * 0.5;
    let min_y = pane.top() + half_height + POSITION_LABEL_PANE_MARGIN;
    let max_y = pane.bottom() - half_height - POSITION_LABEL_PANE_MARGIN;
    if min_y > max_y {
        return None;
    }
    center.y = center.y.clamp(min_y, max_y);
    Some(SceneRect::from_center_size(center, size))
}

fn position_label_overlaps_existing(rect: SceneRect, existing: &[SceneRect]) -> bool {
    existing
        .iter()
        .any(|other| position_label_rects_overlap(rect, *other, 4.0))
}

fn position_label_rects_overlap(a: SceneRect, b: SceneRect, margin: f32) -> bool {
    a.left() < b.right() + margin
        && a.right() + margin > b.left()
        && a.top() < b.bottom() + margin
        && a.bottom() + margin > b.top()
}

fn position_opaque_color(mut color: ChartColor) -> SceneColor {
    color.a = 230;
    color.into()
}

fn position_entry_label(
    settings: &PositionDrawingSettings,
    layout: &PositionDrawingLayout,
    transform: SceneTransform,
) -> Option<String> {
    if !settings.labels_visible && !settings.prices_visible {
        return None;
    }
    let mut parts = Vec::new();
    if settings.labels_visible {
        parts.push("Open P&L: --".to_string());
    }
    if settings.prices_visible {
        parts.push(format!(
            "Entry: {}",
            format_price_for_mode(transform.price_mode, layout.entry.price)
        ));
    }
    if settings.labels_visible
        && let Some(ratio) =
            position_risk_reward_ratio(layout.entry.price, layout.target.price, layout.stop.price)
    {
        parts.push(format!("\nRisk/reward Ratio: {ratio:.2}"));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn position_level_label(
    settings: &PositionDrawingSettings,
    label: &str,
    entry_price: f64,
    level_price: f64,
    transform: SceneTransform,
) -> Option<String> {
    if !settings.labels_visible && !settings.prices_visible {
        return None;
    }
    let mut parts = Vec::new();
    if settings.labels_visible {
        parts.push(label.to_string());
    }
    if settings.prices_visible {
        parts.push(format_price_for_mode(transform.price_mode, level_price));
    }
    if settings.labels_visible {
        let delta = (level_price - entry_price).abs();
        parts.push(format_position_price_delta(transform.price_mode, delta));
        if entry_price != 0.0 {
            parts.push(format!("({:.2}%)", delta / entry_price.abs() * 100.0));
        }
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn position_risk_reward_ratio(entry: f64, target: f64, stop: f64) -> Option<f64> {
    let reward = (target - entry).abs();
    let risk = (entry - stop).abs();
    (risk > f64::EPSILON && reward.is_finite() && risk.is_finite()).then_some(reward / risk)
}

fn format_position_price_delta(mode: PriceScaleValueMode, value: f64) -> String {
    match mode {
        PriceScaleValueMode::Linear | PriceScaleValueMode::Log | PriceScaleValueMode::Inverse => {
            if value.abs() >= 1.0 {
                format!("{value:.2}")
            } else if value.abs() >= 0.01 {
                format!("{value:.4}")
            } else {
                format!("{value:.5}")
            }
        }
        PriceScaleValueMode::Percent { .. } | PriceScaleValueMode::Indexed { .. } => {
            format_price_for_mode(mode, value)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DrawingModelPoint {
    logical: f64,
    price: f64,
    coordinate: f64,
}

impl DrawingModelPoint {
    fn new(logical: f64, price: f64, coordinate: f64) -> Option<Self> {
        (logical.is_finite() && price.is_finite() && coordinate.is_finite()).then_some(Self {
            logical,
            price,
            coordinate,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct DrawingModelClip {
    logical_min: f64,
    logical_max: f64,
    coordinate_min: f64,
    coordinate_max: f64,
}

impl DrawingModelClip {
    fn from_transform(transform: SceneTransform) -> Self {
        let coordinate_range = transform.price_mode.coordinate_range(transform.price);
        Self {
            logical_min: transform.time.from - 0.5,
            logical_max: transform.time.to + 0.5,
            coordinate_min: coordinate_range.min,
            coordinate_max: coordinate_range.max,
        }
    }

    fn contains(self, point: DrawingModelPoint) -> bool {
        point.logical >= self.logical_min
            && point.logical <= self.logical_max
            && point.coordinate >= self.coordinate_min
            && point.coordinate <= self.coordinate_max
    }
}

fn drawing_two_model_points<B: BarSeries + ?Sized>(
    drawing: &DrawingSpec,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<(DrawingModelPoint, DrawingModelPoint)> {
    let first = drawing_anchor_model_point(
        drawing.anchors.first().copied()?,
        transform,
        bars,
        interval_seconds,
    )?;
    let second = drawing_anchor_model_point(
        drawing.anchors.get(1).copied()?,
        transform,
        bars,
        interval_seconds,
    )?;
    Some((first, second))
}

fn drawing_three_model_points<B: BarSeries + ?Sized>(
    drawing: &DrawingSpec,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<(DrawingModelPoint, DrawingModelPoint, DrawingModelPoint)> {
    let first = drawing_anchor_model_point(
        drawing.anchors.first().copied()?,
        transform,
        bars,
        interval_seconds,
    )?;
    let second = drawing_anchor_model_point(
        drawing.anchors.get(1).copied()?,
        transform,
        bars,
        interval_seconds,
    )?;
    let third = drawing_anchor_model_point(
        drawing.anchors.get(2).copied()?,
        transform,
        bars,
        interval_seconds,
    )?;
    Some((first, second, third))
}

fn drawing_model_polyline_points<B: BarSeries + ?Sized>(
    drawing: &DrawingSpec,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Vec<ScenePoint> {
    drawing
        .anchors
        .iter()
        .copied()
        .filter_map(|anchor| drawing_anchor_model_point(anchor, transform, bars, interval_seconds))
        .filter_map(|point| model_point_to_scene(point, transform))
        .collect()
}

fn drawing_anchor_model_point<B: BarSeries + ?Sized>(
    anchor: DrawingAnchor,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<DrawingModelPoint> {
    let (logical, price) = match anchor {
        DrawingAnchor::TimePrice { time, price } => {
            (time_to_logical(time, bars, interval_seconds)?, price)
        }
        DrawingAnchor::LogicalPrice { logical, price } => (logical, price),
        DrawingAnchor::StickedOhlc {
            time,
            field,
            price,
            source_interval_seconds,
        } => (
            time_to_logical(time, bars, interval_seconds)?,
            sticked_ohlc_price(
                time,
                field,
                price,
                source_interval_seconds,
                interval_seconds,
                bars,
            ),
        ),
        DrawingAnchor::TimeOnly { .. }
        | DrawingAnchor::PriceOnly { .. }
        | DrawingAnchor::Percent { .. } => return None,
    };
    DrawingModelPoint::new(logical, price, transform.price_mode.coordinate(price)?)
}

fn model_point_to_scene(point: DrawingModelPoint, transform: SceneTransform) -> Option<ScenePoint> {
    let x = transform.logical_to_x(point.logical);
    let y = coordinate_to_y(point.coordinate, transform)?;
    (x.is_finite() && y.is_finite()).then_some(ScenePoint::new(x, y))
}

fn coordinate_to_y(coordinate: f64, transform: SceneTransform) -> Option<f32> {
    let coordinate_range = transform.price_mode.coordinate_range(transform.price);
    let normalized = (coordinate - coordinate_range.min) / coordinate_range.span();
    let y =
        f64::from(transform.plot.top()) + (1.0 - normalized) * f64::from(transform.plot.height());
    y.is_finite().then_some(y as f32)
}

fn drawing_two_points<B: BarSeries + ?Sized>(
    drawing: &DrawingSpec,
    pane: SceneRect,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<(ScenePoint, ScenePoint)> {
    let first = drawing_anchor_point(
        drawing.anchors.first().copied()?,
        pane,
        transform,
        bars,
        interval_seconds,
    )?;
    let second = drawing_anchor_point(
        drawing.anchors.get(1).copied()?,
        pane,
        transform,
        bars,
        interval_seconds,
    )?;
    Some((first, second))
}

fn drawing_anchor_point<B: BarSeries + ?Sized>(
    anchor: DrawingAnchor,
    pane: SceneRect,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<ScenePoint> {
    match anchor {
        DrawingAnchor::TimePrice { time, price } => Some(ScenePoint::new(
            time_to_x(time, transform, bars, interval_seconds)?,
            transform.price_to_y(price),
        )),
        DrawingAnchor::LogicalPrice { logical, price } => Some(ScenePoint::new(
            transform.logical_to_x(logical),
            transform.price_to_y(price),
        )),
        DrawingAnchor::StickedOhlc {
            time,
            field,
            price,
            source_interval_seconds,
        } => Some(ScenePoint::new(
            time_to_x(time, transform, bars, interval_seconds)?,
            transform.price_to_y(sticked_ohlc_price(
                time,
                field,
                price,
                source_interval_seconds,
                interval_seconds,
                bars,
            )),
        )),
        DrawingAnchor::PriceOnly { price } => Some(ScenePoint::new(
            pane.center().x,
            transform.price_to_y(price),
        )),
        DrawingAnchor::TimeOnly { time } => Some(ScenePoint::new(
            time_to_x(time, transform, bars, interval_seconds)?,
            pane.center().y,
        )),
        DrawingAnchor::Percent { x, y } => Some(ScenePoint::new(
            pane.left() + pane.width() * x,
            pane.top() + pane.height() * y,
        )),
    }
}

fn drawing_price_y<B: BarSeries + ?Sized>(
    anchor: DrawingAnchor,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<f32> {
    match anchor {
        DrawingAnchor::TimePrice { price, .. }
        | DrawingAnchor::PriceOnly { price }
        | DrawingAnchor::LogicalPrice { price, .. } => Some(transform.price_to_y(price)),
        DrawingAnchor::StickedOhlc {
            time,
            field,
            price,
            source_interval_seconds,
        } => Some(transform.price_to_y(sticked_ohlc_price(
            time,
            field,
            price,
            source_interval_seconds,
            interval_seconds,
            bars,
        ))),
        DrawingAnchor::TimeOnly { .. } | DrawingAnchor::Percent { .. } => None,
    }
}

fn drawing_time_x<B: BarSeries + ?Sized>(
    anchor: DrawingAnchor,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<f32> {
    match anchor {
        DrawingAnchor::TimePrice { time, .. }
        | DrawingAnchor::TimeOnly { time }
        | DrawingAnchor::StickedOhlc { time, .. } => {
            time_to_x(time, transform, bars, interval_seconds)
        }
        DrawingAnchor::LogicalPrice { logical, .. } => Some(transform.logical_to_x(logical)),
        DrawingAnchor::PriceOnly { .. } | DrawingAnchor::Percent { .. } => None,
    }
}

fn drawing_text_position<B: BarSeries + ?Sized>(
    drawing: &DrawingSpec,
    pane: SceneRect,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<ScenePoint> {
    let point = drawing_anchor_point(drawing.anchors[0], pane, transform, bars, interval_seconds)?;
    Some(if drawing.kind == DrawingKind::PriceNote {
        ScenePoint::new((point.x + 8.0).min(pane.right() - 4.0), point.y)
    } else {
        point
    })
}

fn drawing_text(drawing: &DrawingSpec, transform: SceneTransform, position: ScenePoint) -> String {
    drawing.text.clone().unwrap_or_else(|| match drawing.kind {
        DrawingKind::PriceNote => {
            format_price_for_mode(transform.price_mode, transform.y_to_price(position.y))
        }
        DrawingKind::Text | DrawingKind::AnchoredText => "Text".to_string(),
        _ => String::new(),
    })
}

fn time_to_x<B: BarSeries + ?Sized>(
    time: i64,
    transform: SceneTransform,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<f32> {
    time_to_logical(time, bars, interval_seconds).map(|logical| transform.logical_to_x(logical))
}

fn nearest_bar_for_time<B: BarSeries + ?Sized>(time: i64, bars: &B) -> Option<(f64, OhlcBar)> {
    let normalized = normalize_timestamp_seconds(time);
    let mut best: Option<(usize, OhlcBar, i64)> = None;
    for index in 0..bars.len() {
        let Some(bar) = bars.bar(index) else {
            continue;
        };
        let distance = (normalize_timestamp_seconds(bar.time) - normalized).abs();
        if best.is_none_or(|(_, _, best_distance)| distance < best_distance) {
            best = Some((index, bar, distance));
        }
    }
    best.map(|(index, bar, _)| (index as f64, bar))
}

fn exact_bar_for_time<B: BarSeries + ?Sized>(time: i64, bars: &B) -> Option<(f64, OhlcBar)> {
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

fn time_to_logical<B: BarSeries + ?Sized>(
    time: i64,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<f64> {
    if bars.is_empty() {
        return None;
    }
    if let Some((logical, _)) = exact_bar_for_time(time, bars) {
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
            return extrapolated_time_to_logical(target, bars, interval_seconds);
        }
        previous = Some((index, current));
    }

    extrapolated_time_to_logical(target, bars, interval_seconds)
}

fn extrapolated_time_to_logical<B: BarSeries + ?Sized>(
    target: i64,
    bars: &B,
    interval_seconds: Option<i64>,
) -> Option<f64> {
    let first = bars
        .bar(0)
        .map(|bar| normalize_timestamp_seconds(bar.time))?;
    let last_index = bars.len().saturating_sub(1);
    let last = bars
        .bar(last_index)
        .map(|bar| normalize_timestamp_seconds(bar.time))?;
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

fn sticked_ohlc_price<B: BarSeries + ?Sized>(
    time: i64,
    field: OhlcField,
    fallback_price: f64,
    source_interval_seconds: Option<i64>,
    current_interval_seconds: Option<i64>,
    bars: &B,
) -> f64 {
    let same_interval = source_interval_seconds.is_none()
        || current_interval_seconds.is_some_and(|current| Some(current) == source_interval_seconds);
    if same_interval {
        if let Some((_, bar)) = exact_bar_for_time(time, bars) {
            return ohlc_field_value(bar, field);
        }
    }
    fallback_price
}

fn ohlc_field_value(bar: OhlcBar, field: OhlcField) -> f64 {
    match field {
        OhlcField::Open => bar.open,
        OhlcField::High => bar.high,
        OhlcField::Low => bar.low,
        OhlcField::Close => bar.close,
    }
}

fn visible_trend_segment(
    from: DrawingModelPoint,
    to: DrawingModelPoint,
    transform: SceneTransform,
) -> Option<(ScenePoint, ScenePoint)> {
    let clip = DrawingModelClip::from_transform(transform);
    let (from, to) = clipped_model_line_segment(from, to, clip, 0.0, 1.0)?;
    Some((
        model_point_to_scene(from, transform)?,
        model_point_to_scene(to, transform)?,
    ))
}

fn visible_ray_segment(
    from: DrawingModelPoint,
    through: DrawingModelPoint,
    transform: SceneTransform,
) -> Option<(ScenePoint, ScenePoint)> {
    let clip = DrawingModelClip::from_transform(transform);
    let (from, to) = clipped_model_line_segment(from, through, clip, 0.0, f64::INFINITY)?;
    Some((
        model_point_to_scene(from, transform)?,
        model_point_to_scene(to, transform)?,
    ))
}

fn visible_horizontal_ray_segment(
    origin: DrawingModelPoint,
    transform: SceneTransform,
) -> Option<(ScenePoint, ScenePoint)> {
    let clip = DrawingModelClip::from_transform(transform);
    if origin.coordinate < clip.coordinate_min
        || origin.coordinate > clip.coordinate_max
        || origin.logical > clip.logical_max
    {
        return None;
    }

    let from = DrawingModelPoint {
        logical: origin.logical.max(clip.logical_min),
        price: origin.price,
        coordinate: origin.coordinate,
    };
    let to = DrawingModelPoint {
        logical: clip.logical_max,
        price: origin.price,
        coordinate: origin.coordinate,
    };
    Some((
        model_point_to_scene(from, transform)?,
        model_point_to_scene(to, transform)?,
    ))
}

fn clipped_model_line_segment(
    from: DrawingModelPoint,
    through: DrawingModelPoint,
    clip: DrawingModelClip,
    mut min_t: f64,
    mut max_t: f64,
) -> Option<(DrawingModelPoint, DrawingModelPoint)> {
    let d_logical = through.logical - from.logical;
    let d_price = through.price - from.price;
    let d_coordinate = through.coordinate - from.coordinate;
    if d_logical.abs() < f64::EPSILON && d_coordinate.abs() < f64::EPSILON {
        return clip.contains(from).then_some((from, through));
    }

    if !clip_model_axis(
        from.logical,
        d_logical,
        clip.logical_min,
        clip.logical_max,
        &mut min_t,
        &mut max_t,
    ) || !clip_model_axis(
        from.coordinate,
        d_coordinate,
        clip.coordinate_min,
        clip.coordinate_max,
        &mut min_t,
        &mut max_t,
    ) || max_t < min_t
        || !max_t.is_finite()
    {
        return None;
    }

    Some((
        point_on_model_line(from, d_logical, d_price, d_coordinate, min_t)?,
        point_on_model_line(from, d_logical, d_price, d_coordinate, max_t)?,
    ))
}

fn clip_model_axis(
    origin: f64,
    delta: f64,
    min: f64,
    max: f64,
    min_t: &mut f64,
    max_t: &mut f64,
) -> bool {
    if delta.abs() < f64::EPSILON {
        return origin >= min && origin <= max;
    }

    let first = (min - origin) / delta;
    let second = (max - origin) / delta;
    *min_t = min_t.max(first.min(second));
    *max_t = max_t.min(first.max(second));
    *min_t <= *max_t
}

fn point_on_model_line(
    from: DrawingModelPoint,
    d_logical: f64,
    d_price: f64,
    d_coordinate: f64,
    t: f64,
) -> Option<DrawingModelPoint> {
    DrawingModelPoint::new(
        from.logical + d_logical * t,
        from.price + d_price * t,
        from.coordinate + d_coordinate * t,
    )
}

fn normalized_rect(first: ScenePoint, second: ScenePoint) -> SceneRect {
    SceneRect::from_min_max(
        ScenePoint::new(first.x.min(second.x), first.y.min(second.y)),
        ScenePoint::new(first.x.max(second.x), first.y.max(second.y)),
    )
}

fn rectangle_corner_points(first: ScenePoint, second: ScenePoint) -> [ScenePoint; 4] {
    let rect = normalized_rect(first, second);
    [
        ScenePoint::new(rect.left(), rect.top()),
        ScenePoint::new(rect.right(), rect.top()),
        ScenePoint::new(rect.right(), rect.bottom()),
        ScenePoint::new(rect.left(), rect.bottom()),
    ]
}

fn text_rect(
    position: ScenePoint,
    anchor: SceneTextAnchor,
    text: &str,
    drawing: &DrawingSpec,
) -> SceneRect {
    let width = (text.chars().count() as f32 * drawing.style.text_size * 0.62 + 14.0).max(28.0);
    let height = drawing.style.text_size + 10.0;
    match anchor {
        SceneTextAnchor::Center => {
            SceneRect::from_center_size(position, SceneSize::new(width, height))
        }
        SceneTextAnchor::LeftCenter => SceneRect::from_min_max(
            ScenePoint::new(position.x - 4.0, position.y - height * 0.5),
            ScenePoint::new(position.x - 4.0 + width, position.y + height * 0.5),
        ),
        SceneTextAnchor::RightCenter => SceneRect::from_min_max(
            ScenePoint::new(position.x - width + 4.0, position.y - height * 0.5),
            ScenePoint::new(position.x + 4.0, position.y + height * 0.5),
        ),
    }
}

fn point_line_distance(point: ScenePoint, from: ScenePoint, to: ScenePoint) -> f32 {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let length_sq = dx * dx + dy * dy;
    if length_sq <= f32::EPSILON {
        return ((point.x - from.x).powi(2) + (point.y - from.y).powi(2)).sqrt();
    }
    let t = (((point.x - from.x) * dx + (point.y - from.y) * dy) / length_sq).clamp(0.0, 1.0);
    let projection = ScenePoint::new(from.x + t * dx, from.y + t * dy);
    ((point.x - projection.x).powi(2) + (point.y - projection.y).powi(2)).sqrt()
}

fn rect_edge_distance(point: ScenePoint, rect: SceneRect) -> f32 {
    if rect.contains(point) {
        return (point.x - rect.left())
            .abs()
            .min((point.x - rect.right()).abs())
            .min((point.y - rect.top()).abs())
            .min((point.y - rect.bottom()).abs());
    }
    let dx = if point.x < rect.left() {
        rect.left() - point.x
    } else if point.x > rect.right() {
        point.x - rect.right()
    } else {
        0.0
    };
    let dy = if point.y < rect.top() {
        rect.top() - point.y
    } else if point.y > rect.bottom() {
        point.y - rect.bottom()
    } else {
        0.0
    };
    (dx * dx + dy * dy).sqrt()
}

fn push_registered_series<B: BarSeries + ?Sized>(
    scene: &mut RenderScene,
    request: &ChartBaseSceneRequest<'_, B>,
) {
    for series in request.series.visible() {
        match (series.kind, series.value_source, series.pane) {
            (SeriesKind::Candlestick, SeriesValueSource::Ohlc, PaneId::Main) => {
                push_candles(
                    scene,
                    &request.layout,
                    request.snapshot.transform,
                    request.snapshot.visible_bounds,
                    request.bars,
                    request.theme,
                );
            }
            (SeriesKind::Histogram, SeriesValueSource::Volume, PaneId::Volume) => {
                if let (Some(volume), Some(volume_range)) =
                    (request.layout.volume, request.snapshot.volume_range)
                {
                    push_volume(
                        scene,
                        volume,
                        volume_range,
                        request.snapshot.transform,
                        request.snapshot.visible_bounds,
                        request.bars,
                        request.theme,
                    );
                }
            }
            (SeriesKind::Line, SeriesValueSource::StudyOutput, PaneId::Main) => {
                if let Some(studies) = request.studies
                    && let Some(values) = studies.output_values(series.id)
                    && let Some(style) = study_output_style(studies, series.id, series.kind)
                {
                    push_study_line(
                        scene,
                        request.layout.plot,
                        request.snapshot.transform,
                        request.snapshot.visible_bounds,
                        values,
                        studies.output_offset(series.id),
                        style.color.into(),
                        style.width,
                    );
                }
            }
            (SeriesKind::Line, SeriesValueSource::StudyOutput, pane @ PaneId::Study(_)) => {
                if let (Some(studies), Some(rect), Some(range)) = (
                    request.studies,
                    request.layout.pane_rect(pane),
                    request.snapshot.study_range(pane),
                ) && let Some(values) = studies.output_values(series.id)
                    && let Some(style) = study_output_style(studies, series.id, series.kind)
                {
                    let transform = study_pane_transform(rect, request.snapshot.transform, range);
                    push_study_line(
                        scene,
                        rect,
                        transform,
                        request.snapshot.visible_bounds,
                        values,
                        studies.output_offset(series.id),
                        style.color.into(),
                        style.width,
                    );
                }
            }
            (SeriesKind::Histogram, SeriesValueSource::StudyOutput, pane @ PaneId::Study(_)) => {
                if let (Some(studies), Some(rect), Some(range)) = (
                    request.studies,
                    request.layout.pane_rect(pane),
                    request.snapshot.study_range(pane),
                ) && let Some(values) = studies.output_values(series.id)
                    && let Some(style) = study_output_style(studies, series.id, series.kind)
                {
                    let transform = study_pane_transform(rect, request.snapshot.transform, range);
                    push_study_histogram(
                        scene,
                        rect,
                        transform,
                        request.snapshot.visible_bounds,
                        values,
                        studies.output_offset(series.id),
                        style.color.into(),
                        style.negative_color.unwrap_or(style.color).into(),
                    );
                }
            }
            _ => {}
        }
    }
}

fn push_study_pane_guides_and_labels<B: BarSeries + ?Sized>(
    scene: &mut RenderScene,
    request: &ChartBaseSceneRequest<'_, B>,
) {
    for pane in &request.layout.panes {
        if !matches!(pane.id, PaneId::Study(_)) {
            continue;
        }
        let Some(range) = request.snapshot.study_range(pane.id) else {
            continue;
        };
        let transform = study_pane_transform(pane.rect, request.snapshot.transform, range);
        let guide_styles = request
            .studies
            .map(|studies| studies.pane_guide_styles(pane.id))
            .filter(|styles| !styles.is_empty())
            .unwrap_or_else(|| {
                study_pane_guide_levels(request.series, pane.id)
                    .into_iter()
                    .map(default_guide_style)
                    .collect()
            });
        for guide in guide_styles {
            if !guide.visible {
                continue;
            }
            let level = guide.level;
            if level < range.min || level > range.max {
                continue;
            }
            let y = transform.price_to_y(level);
            scene.push(ScenePrimitive::Line {
                from: ScenePoint::new(pane.rect.left(), y),
                to: ScenePoint::new(pane.rect.right(), y),
                stroke: SceneStroke::new(guide.width.max(0.5), guide.color.into()),
                clip: Some(pane.rect),
            });
        }
        let label = request
            .studies
            .and_then(|studies| studies.pane_label(pane.id))
            .or_else(|| study_pane_label(request.series, pane.id));
        if let Some(label) = label {
            scene.push(ScenePrimitive::Text {
                position: ScenePoint::new(pane.rect.left() + 8.0, pane.rect.top() + 13.0),
                anchor: SceneTextAnchor::LeftCenter,
                text: label,
                font: SceneFont::monospace(10.0),
                color: request.theme.appearance.legend_text.into(),
                clip: Some(pane.rect),
            });
        }
    }
}

fn study_pane_guide_levels(series: &SeriesRegistry, pane: PaneId) -> Vec<f64> {
    let mut levels = Vec::new();
    let has_rsi = series.visible_by_pane(pane).any(|series| {
        series.value_source == SeriesValueSource::StudyOutput && series.name.starts_with("RSI")
    });
    if has_rsi {
        levels.extend([30.0, 50.0, 70.0]);
    }

    let has_zero_line = series.visible_by_pane(pane).any(|series| {
        series.value_source == SeriesValueSource::StudyOutput
            && (series.kind == SeriesKind::Histogram
                || series.name == "MACD"
                || series.name == "Signal")
    });
    if has_zero_line && !levels.contains(&0.0) {
        levels.push(0.0);
    }

    levels
}

fn study_pane_label(series: &SeriesRegistry, pane: PaneId) -> Option<String> {
    let names = series
        .visible_by_pane(pane)
        .filter(|series| series.value_source == SeriesValueSource::StudyOutput)
        .map(|series| series.name.as_str())
        .take(3)
        .collect::<Vec<_>>();

    (!names.is_empty()).then(|| names.join("  "))
}

fn study_output_style(
    studies: &dyn StudyDataSource,
    series_id: SeriesId,
    kind: SeriesKind,
) -> Option<StudyOutputStyle> {
    let style = studies
        .output_style(series_id)
        .unwrap_or_else(|| match kind {
            SeriesKind::Histogram => StudyOutputStyle::histogram(
                true,
                StudyColor::rgba(54, 190, 132, 110),
                StudyColor::rgba(232, 74, 92, 110),
            ),
            _ => StudyOutputStyle::line(true, study_color(series_id.0).into(), 1.5),
        });
    style.visible.then_some(style)
}

fn default_guide_style(level: f64) -> StudyGuideStyle {
    StudyGuideStyle::new(level, true, default_guide_color(level).into(), 1.0)
}

fn default_guide_color(level: f64) -> SceneColor {
    if level.abs() <= f64::EPSILON || (level - 50.0).abs() <= f64::EPSILON {
        SceneColor::rgba(132, 145, 158, 90)
    } else {
        SceneColor::rgba(132, 145, 158, 125)
    }
}

impl From<StudyColor> for SceneColor {
    fn from(color: StudyColor) -> Self {
        Self::rgba(color.r, color.g, color.b, color.a)
    }
}

impl From<SceneColor> for StudyColor {
    fn from(color: SceneColor) -> Self {
        Self::rgba(color.r, color.g, color.b, color.a)
    }
}

fn has_visible_main_price_series(series: &SeriesRegistry) -> bool {
    series.visible().any(|series| {
        matches!(series.kind, SeriesKind::Candlestick)
            && series.value_source == SeriesValueSource::Ohlc
            && series.pane == PaneId::Main
    })
}

fn scale_generation(snapshot: &SceneSnapshot) -> u64 {
    let mut hash = fnv_seed();
    hash_option_bounds(&mut hash, snapshot.visible_bounds);
    hash_f64(&mut hash, snapshot.price_range.min);
    hash_f64(&mut hash, snapshot.price_range.max);
    hash_option_range(&mut hash, snapshot.volume_range);
    hash_rect(&mut hash, snapshot.transform.plot);
    hash_f64(&mut hash, snapshot.transform.time.from);
    hash_f64(&mut hash, snapshot.transform.time.to);
    hash_f64(&mut hash, snapshot.transform.price.min);
    hash_f64(&mut hash, snapshot.transform.price.max);
    hash_price_mode(&mut hash, snapshot.transform.price_mode);
    hash_u64(&mut hash, snapshot.study_ranges.len() as u64);
    for pane_range in &snapshot.study_ranges {
        hash_pane_id(&mut hash, pane_range.id);
        hash_f64(&mut hash, pane_range.rect.min);
        hash_f64(&mut hash, pane_range.rect.max);
    }
    hash
}

fn layout_generation(layout: &SceneLayout) -> u64 {
    let mut hash = fnv_seed();
    hash_rect(&mut hash, layout.outer);
    hash_rect(&mut hash, layout.legend);
    hash_u64(&mut hash, layout.panes.len() as u64);
    for pane in &layout.panes {
        hash_pane_id(&mut hash, pane.id);
        hash_rect(&mut hash, pane.rect);
    }
    hash_rect(&mut hash, layout.plot);
    hash_option_rect(&mut hash, layout.volume);
    hash_rect(&mut hash, layout.price_axis);
    hash_rect(&mut hash, layout.time_axis);
    hash
}

fn hash_pane_id(hash: &mut u64, pane: PaneId) {
    match pane {
        PaneId::Main => hash_u64(hash, 0),
        PaneId::Volume => hash_u64(hash, 1),
        PaneId::Study(id) => {
            hash_u64(hash, 2);
            hash_u64(hash, u64::from(id));
        }
    }
}

fn fnv_seed() -> u64 {
    0xcbf2_9ce4_8422_2325
}

fn hash_u64(hash: &mut u64, value: u64) {
    *hash ^= value;
    *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
}

fn hash_f32(hash: &mut u64, value: f32) {
    hash_u64(
        hash,
        if value == 0.0 {
            0
        } else {
            u64::from(value.to_bits())
        },
    );
}

fn hash_f64(hash: &mut u64, value: f64) {
    hash_u64(hash, if value == 0.0 { 0 } else { value.to_bits() });
}

fn hash_rect(hash: &mut u64, rect: SceneRect) {
    hash_f32(hash, rect.min.x);
    hash_f32(hash, rect.min.y);
    hash_f32(hash, rect.max.x);
    hash_f32(hash, rect.max.y);
}

fn hash_option_rect(hash: &mut u64, rect: Option<SceneRect>) {
    match rect {
        Some(rect) => {
            hash_u64(hash, 1);
            hash_rect(hash, rect);
        }
        None => hash_u64(hash, 0),
    }
}

fn hash_option_bounds(hash: &mut u64, bounds: Option<(usize, usize)>) {
    match bounds {
        Some((from, to)) => {
            hash_u64(hash, 1);
            hash_u64(hash, from as u64);
            hash_u64(hash, to as u64);
        }
        None => hash_u64(hash, 0),
    }
}

fn hash_option_range(hash: &mut u64, range: Option<PriceRange>) {
    match range {
        Some(range) => {
            hash_u64(hash, 1);
            hash_f64(hash, range.min);
            hash_f64(hash, range.max);
        }
        None => hash_u64(hash, 0),
    }
}

fn hash_price_mode(hash: &mut u64, mode: PriceScaleValueMode) {
    match mode {
        PriceScaleValueMode::Linear => hash_u64(hash, 0),
        PriceScaleValueMode::Log => hash_u64(hash, 1),
        PriceScaleValueMode::Percent { base } => {
            hash_u64(hash, 2);
            hash_f64(hash, base);
        }
        PriceScaleValueMode::Indexed { base } => {
            hash_u64(hash, 3);
            hash_f64(hash, base);
        }
        PriceScaleValueMode::Inverse => hash_u64(hash, 4),
    }
}

fn push_background(scene: &mut RenderScene, layout: &SceneLayout, theme: &ChartTheme) {
    scene.push(ScenePrimitive::FilledRect {
        rect: layout.legend,
        radius: 0.0,
        color: theme.appearance.legend_background.into(),
        clip: None,
    });
    scene.push(ScenePrimitive::FilledRect {
        rect: layout.plot,
        radius: 0.0,
        color: theme.appearance.pane_background.into(),
        clip: None,
    });
    scene.push(ScenePrimitive::FilledRect {
        rect: layout.price_axis,
        radius: 0.0,
        color: theme.appearance.axis_background.into(),
        clip: None,
    });
    scene.push(ScenePrimitive::FilledRect {
        rect: layout.time_axis,
        radius: 0.0,
        color: theme.appearance.axis_background.into(),
        clip: None,
    });
    if let Some(volume) = layout.volume {
        scene.push(ScenePrimitive::FilledRect {
            rect: volume,
            radius: 0.0,
            color: theme.appearance.volume_background.into(),
            clip: None,
        });
        scene.push(ScenePrimitive::Line {
            from: volume.left_top(),
            to: volume.right_top(),
            stroke: SceneStroke::new(1.0, theme.appearance.separator.into()),
            clip: None,
        });
    }
    for pane in &layout.panes {
        if !matches!(pane.id, PaneId::Study(_)) {
            continue;
        }
        scene.push(ScenePrimitive::FilledRect {
            rect: pane.rect,
            radius: 0.0,
            color: theme.appearance.volume_background.into(),
            clip: None,
        });
        scene.push(ScenePrimitive::Line {
            from: pane.rect.left_top(),
            to: pane.rect.right_top(),
            stroke: SceneStroke::new(1.0, theme.appearance.separator.into()),
            clip: None,
        });
    }
    scene.push(ScenePrimitive::StrokedRect {
        rect: layout.plot,
        radius: 0.0,
        stroke: SceneStroke::new(1.0, theme.appearance.separator.into()),
        clip: None,
    });
    scene.push(ScenePrimitive::Line {
        from: layout.legend.left_bottom(),
        to: layout.legend.right_bottom(),
        stroke: SceneStroke::new(1.0, theme.appearance.separator.into()),
        clip: None,
    });
}

fn push_price_grid(
    scene: &mut RenderScene,
    layout: &SceneLayout,
    transform: SceneTransform,
    theme: &ChartTheme,
) {
    for step in 0..=6 {
        let fraction = step as f32 / 6.0;
        let y = layout.plot.bottom() - layout.plot.height() * fraction;
        let price = transform.y_to_price(y);
        if theme.appearance.grid.visible {
            scene.push(ScenePrimitive::Line {
                from: ScenePoint::new(layout.plot.left(), y),
                to: ScenePoint::new(layout.price_axis.right(), y),
                stroke: SceneStroke::new(1.0, theme.appearance.grid.price_color.into()),
                clip: None,
            });
        }
        scene.push(ScenePrimitive::Text {
            position: ScenePoint::new(layout.price_axis.right() - 6.0, y),
            anchor: SceneTextAnchor::RightCenter,
            text: format_price_for_mode(transform.price_mode, price),
            font: SceneFont::monospace(10.0),
            color: theme.appearance.axis_text.into(),
            clip: None,
        });
    }
}

fn push_time_grid<B: BarSeries + ?Sized>(
    scene: &mut RenderScene,
    layout: &SceneLayout,
    transform: SceneTransform,
    bars: &B,
    theme: &ChartTheme,
) {
    for tick in visible_time_axis_ticks(bars, transform.time, layout.plot.width()) {
        let x = transform.logical_to_x(tick.logical);
        if theme.appearance.grid.visible {
            scene.push(ScenePrimitive::Line {
                from: ScenePoint::new(x, layout.plot.top()),
                to: ScenePoint::new(x, layout.time_axis.top() + 5.0),
                stroke: SceneStroke::new(1.0, theme.appearance.grid.time_color.into()),
                clip: None,
            });
        }
        scene.push(ScenePrimitive::Text {
            position: ScenePoint::new(x, layout.time_axis.center().y + 2.0),
            anchor: SceneTextAnchor::Center,
            text: format_time_axis_label(tick.timestamp, tick.unit),
            font: SceneFont::monospace(10.0),
            color: theme.appearance.axis_text.into(),
            clip: None,
        });
    }
}

fn push_candles<B: BarSeries + ?Sized>(
    scene: &mut RenderScene,
    layout: &SceneLayout,
    transform: SceneTransform,
    visible_bounds: Option<(usize, usize)>,
    bars: &B,
    theme: &ChartTheme,
) {
    let bar_spacing = layout.plot.width() / transform.time.span() as f32;
    let body_width = candle_body_width(bar_spacing);

    let Some((from, to)) = visible_bounds else {
        return;
    };
    for index in from..=to {
        let Some(bar) = bars.bar(index) else {
            continue;
        };
        let x = transform.logical_to_x(index as f64);
        if x < layout.plot.left() - body_width || x > layout.plot.right() + body_width {
            continue;
        }
        let up = bar.close >= bar.open;
        let body_color = candle_body_color(theme, up);
        let wick_color = candle_wick_color(theme, up);
        let y_high = transform.price_to_y(bar.high);
        let y_low = transform.price_to_y(bar.low);
        let y_open = transform.price_to_y(bar.open);
        let y_close = transform.price_to_y(bar.close);

        scene.push(ScenePrimitive::Line {
            from: ScenePoint::new(x, y_high),
            to: ScenePoint::new(x, y_low),
            stroke: SceneStroke::new(1.0, wick_color),
            clip: Some(layout.plot),
        });
        let top = y_open.min(y_close);
        let bottom = y_open.max(y_close).max(top + 1.0);
        scene.push(ScenePrimitive::FilledRect {
            rect: SceneRect::from_min_max(
                ScenePoint::new(x - body_width * 0.5, top),
                ScenePoint::new(x + body_width * 0.5, bottom),
            ),
            radius: 0.0,
            color: body_color,
            clip: Some(layout.plot),
        });
    }
}

fn push_volume<B: BarSeries + ?Sized>(
    scene: &mut RenderScene,
    rect: SceneRect,
    range: PriceRange,
    transform: SceneTransform,
    visible_bounds: Option<(usize, usize)>,
    bars: &B,
    theme: &ChartTheme,
) {
    let Some((from, to)) = visible_bounds else {
        return;
    };
    let bar_spacing = rect.width() / transform.time.span() as f32;
    let body_width = candle_body_width(bar_spacing).max(1.0);
    for index in from..=to {
        let Some(bar) = bars.bar(index) else {
            continue;
        };
        let x = transform.logical_to_x(index as f64);
        if x < rect.left() - body_width || x > rect.right() + body_width {
            continue;
        }
        let normalized = ((bar.volume - range.min) / range.span()).clamp(0.0, 1.0) as f32;
        let height = rect.height() * normalized;
        scene.push(ScenePrimitive::FilledRect {
            rect: SceneRect::from_min_max(
                ScenePoint::new(x - body_width * 0.5, rect.bottom() - height),
                ScenePoint::new(x + body_width * 0.5, rect.bottom()),
            ),
            radius: 0.0,
            color: if bar.close >= bar.open {
                theme.candles.volume_up.into()
            } else {
                theme.candles.volume_down.into()
            },
            clip: Some(rect),
        });
    }
}

fn push_study_line(
    scene: &mut RenderScene,
    rect: SceneRect,
    transform: SceneTransform,
    visible_bounds: Option<(usize, usize)>,
    values: &[StudyValue],
    logical_offset: i32,
    color: SceneColor,
    width: f32,
) {
    let Some((from, to)) = visible_bounds else {
        return;
    };
    let mut previous = None;
    for index in from..=to {
        let Some(StudyValue::Number(value)) = values.get(index).copied() else {
            previous = None;
            continue;
        };
        if !value.is_finite() {
            previous = None;
            continue;
        }

        let point = ScenePoint::new(
            transform.logical_to_x(index as f64 + f64::from(logical_offset)),
            transform.price_to_y(value),
        );
        if let Some(from) = previous {
            scene.push(ScenePrimitive::Line {
                from,
                to: point,
                stroke: SceneStroke::new(width.max(0.5), color),
                clip: Some(rect),
            });
        }
        previous = Some(point);
    }
}

fn push_study_histogram(
    scene: &mut RenderScene,
    rect: SceneRect,
    transform: SceneTransform,
    visible_bounds: Option<(usize, usize)>,
    values: &[StudyValue],
    logical_offset: i32,
    positive_color: SceneColor,
    negative_color: SceneColor,
) {
    let Some((from, to)) = visible_bounds else {
        return;
    };
    let bar_spacing = rect.width() / transform.time.span() as f32;
    let body_width = candle_body_width(bar_spacing).max(1.0);
    let zero_y = transform.price_to_y(0.0).clamp(rect.top(), rect.bottom());

    for index in from..=to {
        let Some(StudyValue::Number(value)) = values.get(index).copied() else {
            continue;
        };
        if !value.is_finite() {
            continue;
        }
        let x = transform.logical_to_x(index as f64 + f64::from(logical_offset));
        if x < rect.left() - body_width || x > rect.right() + body_width {
            continue;
        }
        let value_y = transform.price_to_y(value).clamp(rect.top(), rect.bottom());
        let top = value_y.min(zero_y);
        let mut bottom = value_y.max(zero_y);
        if bottom - top < 1.0 {
            bottom = (top + 1.0).min(rect.bottom());
        }
        scene.push(ScenePrimitive::FilledRect {
            rect: SceneRect::from_min_max(
                ScenePoint::new(x - body_width * 0.5, top),
                ScenePoint::new(x + body_width * 0.5, bottom),
            ),
            radius: 0.0,
            color: if value >= 0.0 {
                positive_color
            } else {
                negative_color
            },
            clip: Some(rect),
        });
    }
}

fn study_pane_transform(
    rect: SceneRect,
    base: SceneTransform,
    range: PriceRange,
) -> SceneTransform {
    SceneTransform::new(rect, base.time, range)
}

fn push_last_price<B: BarSeries + ?Sized>(
    scene: &mut RenderScene,
    layout: &SceneLayout,
    transform: SceneTransform,
    bars: &B,
    theme: &ChartTheme,
) {
    let Some(last) = bars.last_bar() else {
        return;
    };
    let y = transform
        .price_to_y(last.close)
        .clamp(layout.plot.top(), layout.plot.bottom());
    scene.push(ScenePrimitive::Line {
        from: ScenePoint::new(layout.plot.left(), y),
        to: ScenePoint::new(layout.price_axis.right(), y),
        stroke: SceneStroke::new(1.0, theme.appearance.last_price_line.into()),
        clip: None,
    });
    let label = SceneRect::from_center_size(
        ScenePoint::new(layout.price_axis.center().x, y),
        SceneSize::new(layout.price_axis.width() - 10.0, 20.0),
    );
    scene.push(ScenePrimitive::FilledRect {
        rect: label,
        radius: 1.0,
        color: theme.appearance.last_price_label_fill.into(),
        clip: None,
    });
    scene.push(ScenePrimitive::Text {
        position: label.center(),
        anchor: SceneTextAnchor::Center,
        text: format_price_for_mode(transform.price_mode, last.close),
        font: SceneFont::monospace(10.0),
        color: theme.appearance.last_price_label_text.into(),
        clip: None,
    });
}

fn push_crosshair_and_legend<B: BarSeries + ?Sized>(
    scene: &mut RenderScene,
    layout: &SceneLayout,
    transform: SceneTransform,
    bars: &B,
    hover: Option<ScenePoint>,
    theme: &ChartTheme,
) {
    let hover_logical = hover
        .filter(|pos| {
            layout.plot.contains(*pos) || layout.volume.is_some_and(|rect| rect.contains(*pos))
        })
        .and_then(|pos| nearest_logical_center_at_x(transform, pos.x));
    let index = hover_logical
        .map(|index| index.clamp(0.0, bars.len().saturating_sub(1) as f64) as usize)
        .unwrap_or_else(|| bars.len().saturating_sub(1));
    let Some(bar) = bars.bar(index) else {
        return;
    };
    let x = transform.logical_to_x(index as f64);
    let crosshair_x = hover
        .filter(|pos| layout.plot.contains(*pos))
        .and_then(|pos| nearest_logical_center_at_x(transform, pos.x))
        .map(|logical| transform.logical_to_x(logical))
        .unwrap_or(x);
    let price = hover
        .map(|pos| transform.y_to_price(pos.y))
        .unwrap_or(bar.close);
    let y = transform
        .price_to_y(price)
        .clamp(layout.plot.top(), layout.plot.bottom());

    if theme.crosshair.visible && hover.is_some_and(|pos| layout.plot.contains(pos)) {
        scene.push(ScenePrimitive::Line {
            from: ScenePoint::new(crosshair_x, layout.plot.top()),
            to: ScenePoint::new(crosshair_x, layout.time_axis.bottom()),
            stroke: SceneStroke::new(1.0, theme.crosshair.line.into()),
            clip: None,
        });
        scene.push(ScenePrimitive::Line {
            from: ScenePoint::new(layout.plot.left(), y),
            to: ScenePoint::new(layout.price_axis.right(), y),
            stroke: SceneStroke::new(1.0, theme.crosshair.line.into()),
            clip: None,
        });
        let price_label = SceneRect::from_center_size(
            ScenePoint::new(layout.price_axis.center().x, y),
            SceneSize::new(layout.price_axis.width() - 10.0, 18.0),
        );
        scene.push(ScenePrimitive::FilledRect {
            rect: price_label,
            radius: 1.0,
            color: theme.crosshair.label_fill.into(),
            clip: None,
        });
        scene.push(ScenePrimitive::Text {
            position: price_label.center(),
            anchor: SceneTextAnchor::Center,
            text: format_price_for_mode(transform.price_mode, price),
            font: SceneFont::monospace(9.0),
            color: theme.crosshair.label_text.into(),
            clip: None,
        });
    }

    let change = bar.close - bar.open;
    let legend = format!(
        "O {}  H {}  L {}  C {}  Vol {}  {}",
        format_price(bar.open),
        format_price(bar.high),
        format_price(bar.low),
        format_price(bar.close),
        format_volume(bar.volume),
        format_time_label(bar.time)
    );
    scene.push(ScenePrimitive::Text {
        position: ScenePoint::new(
            layout.legend.left_center().x + 8.0,
            layout.legend.left_center().y,
        ),
        anchor: SceneTextAnchor::LeftCenter,
        text: legend,
        font: SceneFont::monospace(11.0),
        color: candle_body_color(theme, change >= 0.0),
        clip: None,
    });
}

fn nearest_logical_center_at_x(transform: SceneTransform, x: f32) -> Option<f64> {
    let logical = transform.x_to_logical(x);
    logical.is_finite().then(|| logical.round())
}

fn candle_body_color(theme: &ChartTheme, up: bool) -> SceneColor {
    if up {
        theme.candles.up_body.into()
    } else {
        theme.candles.down_body.into()
    }
}

fn candle_wick_color(theme: &ChartTheme, up: bool) -> SceneColor {
    if up {
        theme.candles.up_wick.into()
    } else {
        theme.candles.down_wick.into()
    }
}

fn study_color(series_id: u32) -> SceneColor {
    const COLORS: [SceneColor; 6] = [
        SceneColor::rgb(68, 152, 255),
        SceneColor::rgb(244, 180, 64),
        SceneColor::rgb(167, 105, 255),
        SceneColor::rgb(80, 202, 192),
        SceneColor::rgb(255, 112, 150),
        SceneColor::rgb(184, 220, 84),
    ];
    COLORS[series_id as usize % COLORS.len()]
}

fn candle_body_width(bar_spacing: f32) -> f32 {
    if bar_spacing >= 3.0 {
        (bar_spacing * 0.72).clamp(2.0, 24.0)
    } else {
        1.0
    }
}

fn format_price(value: f64) -> String {
    if value.abs() >= 100.0 {
        format!("{value:.2}")
    } else if value.abs() >= 1.0 {
        format!("{value:.4}")
    } else {
        format!("{value:.5}")
    }
}

fn format_price_for_mode(mode: PriceScaleValueMode, value: f64) -> String {
    match mode {
        PriceScaleValueMode::Percent { .. } => mode
            .coordinate(value)
            .map(format_percent)
            .unwrap_or_else(|| format_price(value)),
        PriceScaleValueMode::Indexed { .. } => mode
            .coordinate(value)
            .map(|indexed| format!("{indexed:.2}"))
            .unwrap_or_else(|| format_price(value)),
        PriceScaleValueMode::Linear | PriceScaleValueMode::Log | PriceScaleValueMode::Inverse => {
            format_price(value)
        }
    }
}

fn format_percent(value: f64) -> String {
    if value > 0.0 {
        format!("+{value:.2}%")
    } else {
        format!("{value:.2}%")
    }
}

fn format_volume(value: f64) -> String {
    let abs = value.abs();
    if abs >= 1_000_000.0 {
        format!("{:.1}m", abs / 1_000_000.0)
    } else if abs >= 1_000.0 {
        format!("{:.1}k", abs / 1_000.0)
    } else {
        format!("{value:.0}")
    }
}

fn format_time_label(timestamp: i64) -> String {
    let seconds = normalize_timestamp_seconds(timestamp).max(0);
    let hour = (seconds / 3600) % 24;
    let minute = (seconds / 60) % 60;
    format!("{hour:02}:{minute:02}")
}

#[cfg(test)]
mod tests {
    use super::{
        ChartSceneRequest, ChartTheme, DrawingHitTarget, DrawingHitTestRequest,
        DrawingSceneRequest, POSITION_LABEL_MIN_FONT_SIZE, POSITION_WIDTH_HANDLE_INDEX,
        RenderCacheGenerationInputs, RenderCacheKey, RenderScene, RenderSceneCache, SceneColor,
        SceneLayout, SceneLineStyle, ScenePoint, ScenePrimitive, SceneRect, SceneSnapshot,
        SceneTextAnchor, SceneTransform, build_candlestick_scene, build_chart_scene,
        build_drawing_scene, candle_body_width, fib_label_has_vertical_room,
        fib_retracement_label_placement, format_price, format_price_for_mode, format_time_label,
        hit_test_drawings,
    };
    use crate::{
        chart_core::{
            ChartColor, DrawingAnchor, DrawingId, DrawingKind, DrawingLineStyle, DrawingPayload,
            DrawingSpec, DrawingStore, FibRetracementLabelPosition, FibRetracementLabelSide,
            FibRetracementLevel, FibRetracementSettings,
        },
        studies::{StudyDataSource, StudyValue},
        tv_chart::{
            LogicalRange, OhlcBar, PaneId, PaneRect, PriceRange, PriceScaleId, PriceScaleValueMode,
            SeriesId, SeriesKind, SeriesRegistry, SeriesSpec, SeriesValueSource, VOLUME_SERIES_ID,
        },
    };

    fn bar(time: i64, open: f64, close: f64, volume: f64) -> OhlcBar {
        OhlcBar {
            time,
            open,
            high: open.max(close) + 1.0,
            low: open.min(close) - 1.0,
            close,
            volume,
        }
    }

    fn layout() -> SceneLayout {
        let plot =
            SceneRect::from_min_max(ScenePoint::new(8.0, 34.0), ScenePoint::new(310.0, 200.0));
        let volume =
            SceneRect::from_min_max(ScenePoint::new(8.0, 200.0), ScenePoint::new(310.0, 274.0));
        let study =
            SceneRect::from_min_max(ScenePoint::new(8.0, 274.0), ScenePoint::new(310.0, 330.0));
        SceneLayout {
            outer: SceneRect::from_min_max(
                ScenePoint::new(0.0, 0.0),
                ScenePoint::new(400.0, 360.0),
            ),
            legend: SceneRect::from_min_max(
                ScenePoint::new(8.0, 8.0),
                ScenePoint::new(392.0, 34.0),
            ),
            panes: vec![
                PaneRect {
                    id: PaneId::Main,
                    rect: plot,
                },
                PaneRect {
                    id: PaneId::Volume,
                    rect: volume,
                },
                PaneRect {
                    id: PaneId::Study(2),
                    rect: study,
                },
            ],
            plot,
            volume: Some(volume),
            price_axis: SceneRect::from_min_max(
                ScenePoint::new(310.0, 34.0),
                ScenePoint::new(392.0, 330.0),
            ),
            time_axis: SceneRect::from_min_max(
                ScenePoint::new(8.0, 330.0),
                ScenePoint::new(392.0, 352.0),
            ),
        }
    }

    fn snapshot(layout: &SceneLayout) -> SceneSnapshot {
        SceneSnapshot {
            visible_bounds: Some((0, 2)),
            price_range: PriceRange {
                min: 98.0,
                max: 105.0,
            },
            volume_range: Some(PriceRange {
                min: 0.0,
                max: 30.0,
            }),
            study_ranges: vec![PaneRect {
                id: PaneId::Study(2),
                rect: PriceRange {
                    min: 0.0,
                    max: 100.0,
                },
            }],
            transform: SceneTransform::new(
                layout.plot,
                LogicalRange { from: 0.0, to: 2.0 },
                PriceRange {
                    min: 98.0,
                    max: 105.0,
                },
            ),
        }
    }

    fn volume_bar_count(scene: &RenderScene, volume: SceneRect) -> usize {
        scene
            .primitives()
            .iter()
            .filter(|primitive| {
                matches!(
                    primitive,
                    ScenePrimitive::FilledRect {
                        clip: Some(clip),
                        ..
                    } if *clip == volume
                )
            })
            .count()
    }

    fn study_line_segment_count(scene: &RenderScene, plot: SceneRect) -> usize {
        scene
            .primitives()
            .iter()
            .filter(|primitive| {
                matches!(
                    primitive,
                    ScenePrimitive::Line {
                        stroke,
                        clip: Some(clip),
                        ..
                    } if *clip == plot && (stroke.width - 1.5).abs() < f32::EPSILON
                )
            })
            .count()
    }

    fn study_guide_line_count(scene: &RenderScene, plot: SceneRect) -> usize {
        scene
            .primitives()
            .iter()
            .filter(|primitive| {
                matches!(
                    primitive,
                    ScenePrimitive::Line {
                        stroke,
                        clip: Some(clip),
                        ..
                    } if *clip == plot && (stroke.width - 1.0).abs() < f32::EPSILON
                )
            })
            .count()
    }

    fn study_label_count(scene: &RenderScene, plot: SceneRect, text_prefix: &str) -> usize {
        scene
            .primitives()
            .iter()
            .filter(|primitive| {
                matches!(
                    primitive,
                    ScenePrimitive::Text {
                        text,
                        clip: Some(clip),
                        ..
                    } if *clip == plot && text.starts_with(text_prefix)
                )
            })
            .count()
    }

    struct TestStudies {
        series_id: SeriesId,
        values: Vec<StudyValue>,
    }

    impl StudyDataSource for TestStudies {
        fn output_values(&self, series_id: SeriesId) -> Option<&[StudyValue]> {
            (series_id == self.series_id).then_some(self.values.as_slice())
        }
    }

    struct OffsetStudies {
        series_id: SeriesId,
        values: Vec<StudyValue>,
        offset: i32,
    }

    impl StudyDataSource for OffsetStudies {
        fn output_values(&self, series_id: SeriesId) -> Option<&[StudyValue]> {
            (series_id == self.series_id).then_some(self.values.as_slice())
        }

        fn output_offset(&self, series_id: SeriesId) -> i32 {
            (series_id == self.series_id)
                .then_some(self.offset)
                .unwrap_or(0)
        }
    }

    fn generations(
        data_generation: u64,
        theme_generation: u64,
        series_generation: u64,
        study_generation: u64,
    ) -> RenderCacheGenerationInputs {
        RenderCacheGenerationInputs::new(
            data_generation,
            theme_generation,
            series_generation,
            study_generation,
        )
    }

    #[test]
    fn scene_records_primitives_in_paint_order() {
        let layout = layout();
        let bars = vec![
            bar(1_700_000_000, 100.0, 101.0, 10.0),
            bar(1_700_000_060, 101.0, 99.0, 20.0),
            bar(1_700_000_120, 99.0, 102.0, 30.0),
        ];

        let scene = build_candlestick_scene(
            layout.clone(),
            snapshot(&layout),
            "BTC",
            &bars,
            Some(ScenePoint::new(160.0, 100.0)),
        );

        assert!(!scene.is_empty());
        assert!(matches!(
            scene.primitives().first(),
            Some(ScenePrimitive::FilledRect {
                color: SceneColor {
                    r: 14,
                    g: 17,
                    b: 20,
                    a: 255
                },
                ..
            })
        ));
        assert!(scene.primitives().iter().any(|primitive| {
            matches!(primitive, ScenePrimitive::Text { text, .. } if text.starts_with("O "))
        }));
        assert!(scene.primitives().iter().any(|primitive| matches!(
            primitive,
            ScenePrimitive::FilledRect { clip: Some(_), .. }
        )));
    }

    #[test]
    fn chart_scene_uses_series_registry_for_volume_primitives() {
        let layout = layout();
        let bars = vec![
            bar(1_700_000_000, 100.0, 101.0, 10.0),
            bar(1_700_000_060, 101.0, 99.0, 20.0),
            bar(1_700_000_120, 99.0, 102.0, 30.0),
        ];
        let mut series = SeriesRegistry::default();

        let with_volume = build_chart_scene(ChartSceneRequest {
            layout: layout.clone(),
            snapshot: snapshot(&layout),
            theme: &ChartTheme::default(),
            series: &series,
            studies: None,
            bars: &bars,
            hover: None,
        });

        assert!(volume_bar_count(&with_volume, layout.volume.unwrap()) > 0);

        series.set_visible(VOLUME_SERIES_ID, false);
        let without_volume = build_chart_scene(ChartSceneRequest {
            layout: layout.clone(),
            snapshot: snapshot(&layout),
            theme: &ChartTheme::default(),
            series: &series,
            studies: None,
            bars: &bars,
            hover: None,
        });

        assert_eq!(volume_bar_count(&without_volume, layout.volume.unwrap()), 0);
    }

    #[test]
    fn chart_scene_uses_theme_for_background_grid_and_candles() {
        let layout = layout();
        let bars = vec![
            bar(1_700_000_000, 100.0, 101.0, 10.0),
            bar(1_700_000_060, 101.0, 99.0, 20.0),
            bar(1_700_000_120, 99.0, 102.0, 30.0),
        ];
        let mut theme = ChartTheme::default();
        theme.appearance.pane_background = ChartColor::rgb(1, 2, 3);
        theme.appearance.grid.visible = false;
        theme.candles.up_body = ChartColor::rgb(4, 5, 6);
        theme.candles.up_wick = ChartColor::rgb(7, 8, 9);

        let scene = build_chart_scene(ChartSceneRequest {
            layout: layout.clone(),
            snapshot: snapshot(&layout),
            theme: &theme,
            series: &SeriesRegistry::default(),
            studies: None,
            bars: &bars,
            hover: None,
        });

        assert!(scene.primitives().iter().any(|primitive| {
            matches!(
                primitive,
                ScenePrimitive::FilledRect { rect, color, .. }
                    if *rect == layout.plot && *color == SceneColor::rgb(1, 2, 3)
            )
        }));
        assert!(scene.primitives().iter().any(|primitive| {
            matches!(
                primitive,
                ScenePrimitive::FilledRect {
                    color,
                    clip: Some(clip),
                    ..
                } if *clip == layout.plot && *color == SceneColor::rgb(4, 5, 6)
            )
        }));
        assert!(scene.primitives().iter().any(|primitive| {
            matches!(
                primitive,
                ScenePrimitive::Line {
                    stroke,
                    clip: Some(clip),
                    ..
                } if *clip == layout.plot && stroke.color == SceneColor::rgb(7, 8, 9)
            )
        }));
        assert!(!scene.primitives().iter().any(|primitive| {
            matches!(
                primitive,
                ScenePrimitive::Line {
                    stroke,
                    clip: None,
                    ..
                } if stroke.color == SceneColor::rgba(38, 44, 50, 150)
                    || stroke.color == SceneColor::rgba(38, 44, 50, 120)
            )
        }));
    }

    #[test]
    fn chart_scene_renders_main_pane_study_line_outputs() {
        let layout = layout();
        let bars = vec![
            bar(1_700_000_000, 100.0, 101.0, 10.0),
            bar(1_700_000_060, 101.0, 99.0, 20.0),
            bar(1_700_000_120, 99.0, 102.0, 30.0),
        ];
        let series_id = SeriesId::new(10_000);
        let studies = TestStudies {
            series_id,
            values: vec![
                StudyValue::Null,
                StudyValue::Number(100.0),
                StudyValue::Number(102.0),
            ],
        };
        let mut series = SeriesRegistry::default();
        series.add_or_replace(SeriesSpec::new(
            series_id,
            "SMA 20",
            SeriesKind::Line,
            SeriesValueSource::StudyOutput,
            PaneId::Main,
            PriceScaleId::Right,
        ));

        let scene = build_chart_scene(ChartSceneRequest {
            layout: layout.clone(),
            snapshot: snapshot(&layout),
            theme: &ChartTheme::default(),
            series: &series,
            studies: Some(&studies),
            bars: &bars,
            hover: None,
        });

        assert_eq!(study_line_segment_count(&scene, layout.plot), 1);
    }

    #[test]
    fn chart_scene_applies_study_output_offsets() {
        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let bars = vec![
            bar(1_700_000_000, 100.0, 101.0, 10.0),
            bar(1_700_000_060, 101.0, 99.0, 20.0),
            bar(1_700_000_120, 99.0, 102.0, 30.0),
        ];
        let series_id = SeriesId::new(10_000);
        let studies = OffsetStudies {
            series_id,
            values: vec![StudyValue::Number(100.0), StudyValue::Number(102.0)],
            offset: 1,
        };
        let mut series = SeriesRegistry::default();
        series.add_or_replace(SeriesSpec::new(
            series_id,
            "SMA 20",
            SeriesKind::Line,
            SeriesValueSource::StudyOutput,
            PaneId::Main,
            PriceScaleId::Right,
        ));

        let scene = build_chart_scene(ChartSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot.clone(),
            theme: &ChartTheme::default(),
            series: &series,
            studies: Some(&studies),
            bars: &bars,
            hover: None,
        });
        let expected_from = scene_snapshot.transform.logical_to_x(1.0);
        let expected_to = scene_snapshot.transform.logical_to_x(2.0);

        assert!(scene.primitives().iter().any(|primitive| {
            matches!(
                primitive,
                ScenePrimitive::Line {
                    from,
                    to,
                    stroke,
                    clip: Some(clip),
                } if *clip == layout.plot
                    && (stroke.width - 1.5).abs() < f32::EPSILON
                    && (from.x - expected_from).abs() < f32::EPSILON
                    && (to.x - expected_to).abs() < f32::EPSILON
            )
        }));
    }

    #[test]
    fn chart_scene_renders_study_pane_line_outputs() {
        let layout = layout();
        let study = layout.pane_rect(PaneId::Study(2)).expect("study pane");
        let bars = vec![
            bar(1_700_000_000, 100.0, 101.0, 10.0),
            bar(1_700_000_060, 101.0, 99.0, 20.0),
            bar(1_700_000_120, 99.0, 102.0, 30.0),
        ];
        let series_id = SeriesId::new(10_000);
        let studies = TestStudies {
            series_id,
            values: vec![
                StudyValue::Null,
                StudyValue::Number(45.0),
                StudyValue::Number(65.0),
            ],
        };
        let mut series = SeriesRegistry::default();
        series.add_or_replace(SeriesSpec::new(
            series_id,
            "RSI 14",
            SeriesKind::Line,
            SeriesValueSource::StudyOutput,
            PaneId::Study(2),
            PriceScaleId::Overlay,
        ));

        let scene = build_chart_scene(ChartSceneRequest {
            layout: layout.clone(),
            snapshot: snapshot(&layout),
            theme: &ChartTheme::default(),
            series: &series,
            studies: Some(&studies),
            bars: &bars,
            hover: None,
        });

        assert_eq!(study_line_segment_count(&scene, study), 1);
        assert_eq!(study_line_segment_count(&scene, layout.plot), 0);
        assert_eq!(study_guide_line_count(&scene, study), 3);
        assert_eq!(study_label_count(&scene, study, "RSI 14"), 1);
    }

    #[test]
    fn chart_scene_renders_study_pane_histogram_outputs() {
        let layout = layout();
        let study = layout.pane_rect(PaneId::Study(2)).expect("study pane");
        let bars = vec![
            bar(1_700_000_000, 100.0, 101.0, 10.0),
            bar(1_700_000_060, 101.0, 99.0, 20.0),
            bar(1_700_000_120, 99.0, 102.0, 30.0),
        ];
        let series_id = SeriesId::new(10_000);
        let studies = TestStudies {
            series_id,
            values: vec![
                StudyValue::Number(-1.0),
                StudyValue::Number(0.5),
                StudyValue::Number(1.5),
            ],
        };
        let mut series = SeriesRegistry::default();
        series.add_or_replace(SeriesSpec::new(
            series_id,
            "MACD Histogram",
            SeriesKind::Histogram,
            SeriesValueSource::StudyOutput,
            PaneId::Study(2),
            PriceScaleId::Overlay,
        ));

        let scene = build_chart_scene(ChartSceneRequest {
            layout: layout.clone(),
            snapshot: snapshot(&layout),
            theme: &ChartTheme::default(),
            series: &series,
            studies: Some(&studies),
            bars: &bars,
            hover: None,
        });

        assert_eq!(volume_bar_count(&scene, study), 3);
        assert_eq!(study_guide_line_count(&scene, study), 1);
        assert_eq!(study_label_count(&scene, study, "MACD"), 1);
    }

    #[test]
    fn crosshair_vertical_line_snaps_to_nearest_logical_center_past_loaded_bars() {
        let layout = layout();
        let mut scene_snapshot = snapshot(&layout);
        scene_snapshot.transform = SceneTransform::new(
            layout.plot,
            LogicalRange { from: 0.0, to: 5.0 },
            scene_snapshot.price_range,
        );
        let bars = vec![
            bar(1_700_000_000, 100.0, 101.0, 10.0),
            bar(1_700_000_060, 101.0, 99.0, 20.0),
            bar(1_700_000_120, 99.0, 102.0, 30.0),
        ];
        let series = SeriesRegistry::default();
        let hover = ScenePoint::new(layout.plot.right() - 8.0, layout.plot.center().y);
        let expected_vertical_x = scene_snapshot
            .transform
            .logical_to_x(scene_snapshot.transform.x_to_logical(hover.x).round());
        let expected_horizontal_y = hover.y;

        let scene = build_chart_scene(ChartSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot,
            theme: &ChartTheme::default(),
            series: &series,
            studies: None,
            bars: &bars,
            hover: Some(hover),
        });

        assert!(scene.primitives().iter().any(|primitive| {
            matches!(
                primitive,
                ScenePrimitive::Line {
                    from,
                    to,
                    clip: None,
                    ..
                } if (from.x - expected_vertical_x).abs() < f32::EPSILON
                    && (to.x - expected_vertical_x).abs() < f32::EPSILON
                    && (from.y - layout.plot.top()).abs() < f32::EPSILON
                    && (to.y - layout.time_axis.bottom()).abs() < f32::EPSILON
            )
        }));
        assert!(scene.primitives().iter().any(|primitive| {
            matches!(
                primitive,
                ScenePrimitive::Line {
                    from,
                    to,
                    clip: None,
                    ..
                } if (from.y - expected_horizontal_y).abs() < f32::EPSILON
                    && (to.y - expected_horizontal_y).abs() < f32::EPSILON
                    && (from.x - layout.plot.left()).abs() < f32::EPSILON
                    && (to.x - layout.price_axis.right()).abs() < f32::EPSILON
            )
        }));
    }

    #[test]
    fn drawing_scene_renders_model_space_lines_rectangles_and_text() {
        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let bars = vec![
            bar(1_700_000_000, 100.0, 101.0, 10.0),
            bar(1_700_000_060, 101.0, 99.0, 20.0),
            bar(1_700_000_120, 99.0, 102.0, 30.0),
        ];
        let mut store = DrawingStore::new();
        let horizontal = store
            .create(
                DrawingKind::HorizontalLine,
                PaneId::Main,
                vec![DrawingAnchor::PriceOnly { price: 100.0 }],
            )
            .expect("horizontal line");
        let mut horizontal_style = store
            .get(horizontal)
            .expect("horizontal line exists")
            .style
            .clone();
        horizontal_style.line_style = DrawingLineStyle::Dashed;
        store
            .replace_style(horizontal, horizontal_style)
            .expect("line style should update");
        let vertical = store
            .insert(DrawingSpec::new(
                DrawingId::new(10),
                DrawingKind::VerticalLine,
                PaneId::Main,
                vec![DrawingAnchor::TimeOnly {
                    time: bars[1].time + 20,
                }],
            ))
            .expect("vertical line");
        store
            .insert(DrawingSpec::new(
                DrawingId::new(11),
                DrawingKind::Rectangle,
                PaneId::Main,
                vec![
                    DrawingAnchor::TimePrice {
                        time: bars[0].time,
                        price: 99.0,
                    },
                    DrawingAnchor::TimePrice {
                        time: bars[2].time,
                        price: 103.0,
                    },
                ],
            ))
            .expect("rectangle");
        store
            .insert(
                DrawingSpec::new(
                    DrawingId::new(12),
                    DrawingKind::AnchoredText,
                    PaneId::Main,
                    vec![DrawingAnchor::Percent { x: 0.5, y: 0.25 }],
                )
                .with_text("Plan"),
            )
            .expect("text");

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot.clone(),
            drawings: &store,
            bars: &bars,
            selected: Some(horizontal),
            hovered: Some(vertical),
            symbol_filter: None,
            interval_seconds: Some(60),
        });
        let expected_horizontal_y = scene_snapshot.transform.price_to_y(100.0);
        let expected_vertical_x = scene_snapshot.transform.logical_to_x(1.0 + 20.0 / 60.0);

        assert!(scene.primitives().iter().any(|primitive| {
            matches!(
                primitive,
                ScenePrimitive::Line {
                    from,
                    to,
                    stroke,
                    clip: Some(clip),
                } if *clip == layout.plot
                    && (from.y - expected_horizontal_y).abs() < f32::EPSILON
                    && (to.y - expected_horizontal_y).abs() < f32::EPSILON
                    && (stroke.width - 2.5).abs() < f32::EPSILON
                    && stroke.line_style == SceneLineStyle::Dashed
            )
        }));
        assert!(scene.primitives().iter().any(|primitive| {
            matches!(
                primitive,
                ScenePrimitive::Line {
                    from,
                    to,
                    stroke,
                    clip: Some(clip),
                } if *clip == layout.plot
                    && (from.x - expected_vertical_x).abs() < f32::EPSILON
                    && (to.x - expected_vertical_x).abs() < f32::EPSILON
                    && (stroke.width - 2.0).abs() < f32::EPSILON
            )
        }));
        assert!(
            scene
                .primitives()
                .iter()
                .any(|primitive| matches!(primitive, ScenePrimitive::StrokedRect { .. }))
        );
        assert!(scene.primitives().iter().any(|primitive| {
            matches!(primitive, ScenePrimitive::Text { text, .. } if text == "Plan")
        }));
        assert_eq!(
            store
                .get(vertical)
                .and_then(|drawing| drawing.anchors.first()),
            Some(&DrawingAnchor::TimeOnly {
                time: bars[1].time + 20,
            })
        );
    }

    #[test]
    fn drawing_scene_renders_and_hit_tests_brush_polyline_segments() {
        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let bars = vec![
            bar(1_700_000_000, 100.0, 101.0, 10.0),
            bar(1_700_000_060, 101.0, 99.0, 20.0),
            bar(1_700_000_120, 99.0, 102.0, 30.0),
        ];
        let mut store = DrawingStore::new();
        let brush = store
            .insert(DrawingSpec::new(
                DrawingId::new(40),
                DrawingKind::Brush,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 0.0,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 1.0,
                        price: 102.0,
                    },
                    DrawingAnchor::TimePrice {
                        time: bars[2].time,
                        price: 101.0,
                    },
                ],
            ))
            .expect("brush drawing");

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot.clone(),
            drawings: &store,
            bars: &bars,
            selected: Some(brush),
            hovered: None,
            symbol_filter: None,
            interval_seconds: Some(60),
        });

        let brush_lines = scene
            .primitives()
            .iter()
            .filter(|primitive| {
                matches!(
                    primitive,
                    ScenePrimitive::Line {
                        stroke,
                        clip: Some(clip),
                        ..
                    } if *clip == layout.plot
                        && (stroke.width - 2.5).abs() < f32::EPSILON
                )
            })
            .count();
        assert_eq!(brush_lines, 2);

        let hit_point = ScenePoint::new(
            scene_snapshot.transform.logical_to_x(0.5),
            scene_snapshot.transform.price_to_y(101.0),
        );
        let hit = hit_test_drawings(DrawingHitTestRequest {
            layout: &layout,
            snapshot: &scene_snapshot,
            drawings: &store,
            bars: &bars,
            point: hit_point,
            tolerance: 5.0,
            symbol_filter: None,
            selected: Some(brush),
            hovered: None,
            interval_seconds: Some(60),
        })
        .expect("brush segment hit");

        assert_eq!(hit.drawing_id, brush);
        assert_eq!(hit.target, DrawingHitTarget::Edge);
    }

    #[test]
    fn drawing_scene_renders_and_hit_tests_fib_retracement_levels() {
        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let bars = vec![
            bar(1_700_000_000, 100.0, 101.0, 10.0),
            bar(1_700_000_060, 101.0, 99.0, 20.0),
            bar(1_700_000_120, 99.0, 102.0, 30.0),
        ];
        let mut store = DrawingStore::new();
        let fib = store
            .insert(DrawingSpec::new(
                DrawingId::new(41),
                DrawingKind::FibRetracement,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 0.0,
                        price: 99.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 2.0,
                        price: 104.0,
                    },
                ],
            ))
            .expect("fib retracement");

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot.clone(),
            drawings: &store,
            bars: &bars,
            selected: Some(fib),
            hovered: None,
            symbol_filter: None,
            interval_seconds: Some(60),
        });

        let fib_lines = scene
            .primitives()
            .iter()
            .filter(|primitive| {
                matches!(
                    primitive,
                    ScenePrimitive::Line {
                        stroke,
                        clip: Some(clip),
                        ..
                    } if *clip == layout.plot
                        && (stroke.width - 2.5).abs() < f32::EPSILON
                )
            })
            .count();
        assert_eq!(fib_lines, 8);
        assert!(scene.primitives().iter().any(|primitive| {
            matches!(primitive, ScenePrimitive::Text { text, .. } if text.starts_with("0.5 "))
        }));

        let hit_point = ScenePoint::new(
            scene_snapshot.transform.logical_to_x(0.25),
            scene_snapshot.transform.price_to_y(101.5),
        );
        let hit = hit_test_drawings(DrawingHitTestRequest {
            layout: &layout,
            snapshot: &scene_snapshot,
            drawings: &store,
            bars: &bars,
            point: hit_point,
            tolerance: 5.0,
            symbol_filter: None,
            selected: Some(fib),
            hovered: None,
            interval_seconds: Some(60),
        })
        .expect("fib level hit");

        assert_eq!(hit.drawing_id, fib);
        assert_eq!(hit.target, DrawingHitTarget::Edge);
    }

    #[test]
    fn drawing_scene_renders_and_hit_tests_long_short_position_plans() {
        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let bars = Vec::<OhlcBar>::new();
        let mut store = DrawingStore::new();
        let long = store
            .insert(DrawingSpec::new(
                DrawingId::new(42),
                DrawingKind::LongPosition,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 0.2,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 1.0,
                        price: 104.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 1.0,
                        price: 98.0,
                    },
                ],
            ))
            .expect("long position");
        store
            .insert(DrawingSpec::new(
                DrawingId::new(43),
                DrawingKind::ShortPosition,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 1.2,
                        price: 103.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 1.9,
                        price: 99.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 1.9,
                        price: 105.0,
                    },
                ],
            ))
            .expect("short position");

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot.clone(),
            drawings: &store,
            bars: &bars,
            selected: Some(long),
            hovered: None,
            symbol_filter: None,
            interval_seconds: None,
        });

        let fills = scene
            .primitives()
            .iter()
            .filter(|primitive| {
                matches!(
                    primitive,
                    ScenePrimitive::FilledRect {
                        radius,
                        clip: Some(clip),
                        ..
                    } if *radius == 0.0 && *clip == layout.plot
                )
            })
            .count();
        assert_eq!(fills, 4);
        assert!(scene.primitives().iter().any(|primitive| {
            matches!(primitive, ScenePrimitive::Text { text, .. } if text.contains("Open P&L") && text.contains("Risk/reward Ratio: 2.00"))
        }));
        assert!(scene.primitives().iter().any(|primitive| {
            matches!(primitive, ScenePrimitive::Text { text, .. } if text.contains("Target") && text.contains("4.00%"))
        }));

        let hit = hit_test_drawings(DrawingHitTestRequest {
            layout: &layout,
            snapshot: &scene_snapshot,
            drawings: &store,
            bars: &bars,
            point: ScenePoint::new(
                scene_snapshot.transform.logical_to_x(0.2),
                scene_snapshot.transform.price_to_y(104.0),
            ),
            tolerance: 5.0,
            symbol_filter: None,
            selected: Some(long),
            hovered: None,
            interval_seconds: None,
        })
        .expect("target handle hit");
        assert_eq!(hit.drawing_id, long);
        assert_eq!(hit.target, DrawingHitTarget::Anchor(1));

        let hit = hit_test_drawings(DrawingHitTestRequest {
            layout: &layout,
            snapshot: &scene_snapshot,
            drawings: &store,
            bars: &bars,
            point: ScenePoint::new(
                scene_snapshot.transform.logical_to_x(1.0),
                scene_snapshot.transform.price_to_y(100.0),
            ),
            tolerance: 5.0,
            symbol_filter: None,
            selected: Some(long),
            hovered: None,
            interval_seconds: None,
        })
        .expect("width handle hit");
        assert_eq!(hit.drawing_id, long);
        assert_eq!(
            hit.target,
            DrawingHitTarget::Anchor(POSITION_WIDTH_HANDLE_INDEX)
        );
    }

    #[test]
    fn drawing_scene_hides_position_labels_when_projection_is_tiny() {
        let layout = layout();
        let mut scene_snapshot = snapshot(&layout);
        scene_snapshot.price_range = PriceRange {
            min: 0.0,
            max: 1000.0,
        };
        scene_snapshot.transform = SceneTransform::new(
            layout.plot,
            LogicalRange {
                from: 0.0,
                to: 200.0,
            },
            scene_snapshot.price_range,
        );
        let bars = Vec::<OhlcBar>::new();
        let mut store = DrawingStore::new();
        let long = store
            .insert(DrawingSpec::new(
                DrawingId::new(42),
                DrawingKind::LongPosition,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 100.0,
                        price: 500.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 104.0,
                        price: 502.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 104.0,
                        price: 498.0,
                    },
                ],
            ))
            .expect("long position");

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot,
            drawings: &store,
            bars: &bars,
            selected: Some(long),
            hovered: None,
            symbol_filter: None,
            interval_seconds: None,
        });

        assert!(!scene.primitives().iter().any(|primitive| {
            matches!(primitive, ScenePrimitive::Text { text, .. }
                if text.contains("Open P&L")
                    || text.contains("Target")
                    || text.contains("Stop"))
        }));
        assert!(scene.primitives().iter().any(|primitive| {
            matches!(primitive, ScenePrimitive::FilledRect { radius, .. } if *radius == 0.0)
        }));
    }

    #[test]
    fn drawing_scene_shows_position_labels_only_when_hovered_or_selected() {
        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let bars = Vec::<OhlcBar>::new();
        let mut store = DrawingStore::new();
        let long = store
            .insert(DrawingSpec::new(
                DrawingId::new(42),
                DrawingKind::LongPosition,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 0.2,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 1.0,
                        price: 104.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 1.0,
                        price: 98.0,
                    },
                ],
            ))
            .expect("long position");

        let idle_scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot.clone(),
            drawings: &store,
            bars: &bars,
            selected: None,
            hovered: None,
            symbol_filter: None,
            interval_seconds: None,
        });
        assert!(!idle_scene.primitives().iter().any(|primitive| {
            matches!(primitive, ScenePrimitive::Text { text, .. } if text.contains("Open P&L"))
        }));

        let hovered_scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot,
            drawings: &store,
            bars: &bars,
            selected: None,
            hovered: Some(long),
            symbol_filter: None,
            interval_seconds: None,
        });
        assert!(hovered_scene.primitives().iter().any(|primitive| {
            matches!(primitive, ScenePrimitive::Text { text, .. } if text.contains("Open P&L"))
        }));
    }

    #[test]
    fn drawing_scene_renders_position_open_pnl_region_from_latest_close() {
        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let bars = vec![
            bar(1_700_000_000, 100.2, 101.0, 10.0),
            bar(1_700_000_060, 101.0, 102.0, 10.0),
        ];
        let mut store = DrawingStore::new();
        store
            .insert(DrawingSpec::new(
                DrawingId::new(42),
                DrawingKind::LongPosition,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 0.0,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 2.0,
                        price: 104.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 2.0,
                        price: 98.0,
                    },
                ],
            ))
            .expect("long position");

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot.clone(),
            drawings: &store,
            bars: &bars,
            selected: None,
            hovered: None,
            symbol_filter: None,
            interval_seconds: None,
        });
        let position_fills: Vec<_> = scene
            .primitives()
            .iter()
            .filter_map(|primitive| match primitive {
                ScenePrimitive::FilledRect {
                    rect,
                    radius,
                    color,
                    clip: Some(clip),
                } if *radius == 0.0 && *clip == layout.plot => Some((*rect, *color)),
                _ => None,
            })
            .collect();
        assert_eq!(position_fills.len(), 3);
        let current_y = scene_snapshot.transform.price_to_y(102.0);
        let entry_y = scene_snapshot.transform.price_to_y(100.0);
        assert!(position_fills.iter().any(|(rect, color)| {
            color.a > 100
                && (rect.left() - scene_snapshot.transform.logical_to_x(0.0)).abs() < 0.01
                && (rect.right() - scene_snapshot.transform.logical_to_x(1.0)).abs() < 0.01
                && (rect.top() - current_y.min(entry_y)).abs() < 0.01
                && (rect.bottom() - current_y.max(entry_y)).abs() < 0.01
        }));
    }

    #[test]
    fn drawing_scene_position_open_pnl_locks_at_first_target_hit() {
        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let bars = vec![
            bar(1_700_000_000, 100.0, 101.0, 10.0),
            bar(1_700_000_060, 101.0, 104.0, 10.0),
            bar(1_700_000_120, 104.0, 102.0, 10.0),
        ];
        let mut store = DrawingStore::new();
        store
            .insert(DrawingSpec::new(
                DrawingId::new(42),
                DrawingKind::LongPosition,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 0.0,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 2.0,
                        price: 103.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 2.0,
                        price: 98.0,
                    },
                ],
            ))
            .expect("long position");

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot.clone(),
            drawings: &store,
            bars: &bars,
            selected: None,
            hovered: None,
            symbol_filter: None,
            interval_seconds: None,
        });
        let target_y = scene_snapshot.transform.price_to_y(103.0);
        let entry_y = scene_snapshot.transform.price_to_y(100.0);
        let hit_x = scene_snapshot.transform.logical_to_x(1.0);
        assert!(scene.primitives().iter().any(|primitive| {
            matches!(
                primitive,
                ScenePrimitive::FilledRect {
                    rect,
                    radius,
                    color,
                    clip: Some(clip),
                } if *radius == 0.0
                    && *clip == layout.plot
                    && color.a > 100
                    && (rect.right() - hit_x).abs() < 0.01
                    && (rect.top() - target_y.min(entry_y)).abs() < 0.01
                    && (rect.bottom() - target_y.max(entry_y)).abs() < 0.01
            )
        }));
    }

    #[test]
    fn drawing_scene_position_open_pnl_starts_at_first_entry_touch() {
        let layout = layout();
        let mut scene_snapshot = snapshot(&layout);
        scene_snapshot.visible_bounds = Some((0, 4));
        scene_snapshot.transform = SceneTransform::new(
            layout.plot,
            LogicalRange { from: 0.0, to: 4.0 },
            scene_snapshot.price_range,
        );
        let bars = vec![
            OhlcBar {
                time: 1_700_000_000,
                open: 104.5,
                high: 105.0,
                low: 104.0,
                close: 104.4,
                volume: 10.0,
            },
            OhlcBar {
                time: 1_700_000_060,
                open: 102.0,
                high: 103.0,
                low: 101.0,
                close: 102.0,
                volume: 10.0,
            },
            OhlcBar {
                time: 1_700_000_120,
                open: 101.0,
                high: 102.0,
                low: 99.5,
                close: 101.5,
                volume: 10.0,
            },
            OhlcBar {
                time: 1_700_000_180,
                open: 101.5,
                high: 102.6,
                low: 101.0,
                close: 102.0,
                volume: 10.0,
            },
        ];
        let mut store = DrawingStore::new();
        store
            .insert(DrawingSpec::new(
                DrawingId::new(42),
                DrawingKind::LongPosition,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 0.0,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 4.0,
                        price: 104.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 4.0,
                        price: 98.0,
                    },
                ],
            ))
            .expect("long position");

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot.clone(),
            drawings: &store,
            bars: &bars,
            selected: None,
            hovered: None,
            symbol_filter: None,
            interval_seconds: None,
        });
        let current_y = scene_snapshot.transform.price_to_y(102.0);
        let entry_y = scene_snapshot.transform.price_to_y(100.0);
        let entry_touch_x = scene_snapshot.transform.logical_to_x(2.0);
        let current_x = scene_snapshot.transform.logical_to_x(3.0);
        assert!(scene.primitives().iter().any(|primitive| {
            matches!(
                primitive,
                ScenePrimitive::FilledRect {
                    rect,
                    radius,
                    color,
                    clip: Some(clip),
                } if *radius == 0.0
                    && *clip == layout.plot
                    && color.a > 100
                    && (rect.left() - entry_touch_x).abs() < 0.01
                    && (rect.right() - current_x).abs() < 0.01
                    && (rect.top() - current_y.min(entry_y)).abs() < 0.01
                    && (rect.bottom() - current_y.max(entry_y)).abs() < 0.01
            )
        }));
    }

    #[test]
    fn drawing_scene_scales_position_labels_with_projected_width() {
        let layout = layout();
        let bars = Vec::<OhlcBar>::new();
        let mut store = DrawingStore::new();
        let long = store
            .insert(DrawingSpec::new(
                DrawingId::new(42),
                DrawingKind::LongPosition,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 0.2,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 1.8,
                        price: 104.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 1.8,
                        price: 98.0,
                    },
                ],
            ))
            .expect("long position");
        let mut wide_snapshot = snapshot(&layout);
        wide_snapshot.transform = SceneTransform::new(
            layout.plot,
            LogicalRange { from: 0.0, to: 2.0 },
            wide_snapshot.price_range,
        );
        let mut narrow_snapshot = wide_snapshot.clone();
        narrow_snapshot.transform = SceneTransform::new(
            layout.plot,
            LogicalRange { from: 0.0, to: 4.0 },
            narrow_snapshot.price_range,
        );

        let wide_scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: wide_snapshot,
            drawings: &store,
            bars: &bars,
            selected: Some(long),
            hovered: None,
            symbol_filter: None,
            interval_seconds: None,
        });
        let narrow_scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: narrow_snapshot,
            drawings: &store,
            bars: &bars,
            selected: Some(long),
            hovered: None,
            symbol_filter: None,
            interval_seconds: None,
        });
        let position_label_font_size = |scene: &RenderScene| {
            scene
                .primitives()
                .iter()
                .filter_map(|primitive| match primitive {
                    ScenePrimitive::Text { text, font, .. }
                        if text.contains("Open P&L")
                            || text.contains("Target")
                            || text.contains("Stop") =>
                    {
                        Some(font.size)
                    }
                    _ => None,
                })
                .fold(0.0_f32, f32::max)
        };

        let wide_font = position_label_font_size(&wide_scene);
        let narrow_font = position_label_font_size(&narrow_scene);
        assert!(wide_font > narrow_font);
        assert!(narrow_font >= POSITION_LABEL_MIN_FONT_SIZE);
    }

    #[test]
    fn drawing_scene_hides_position_labels_instead_of_clamping_to_pane_edge() {
        let layout = layout();
        let mut scene_snapshot = snapshot(&layout);
        scene_snapshot.transform = SceneTransform::new(
            layout.plot,
            LogicalRange { from: 0.0, to: 2.0 },
            scene_snapshot.price_range,
        );
        let bars = Vec::<OhlcBar>::new();
        let mut store = DrawingStore::new();
        let long = store
            .insert(DrawingSpec::new(
                DrawingId::new(42),
                DrawingKind::LongPosition,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 1.55,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 2.35,
                        price: 104.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 2.35,
                        price: 98.0,
                    },
                ],
            ))
            .expect("long position");

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot,
            drawings: &store,
            bars: &bars,
            selected: Some(long),
            hovered: None,
            symbol_filter: None,
            interval_seconds: None,
        });

        assert!(!scene.primitives().iter().any(|primitive| {
            matches!(primitive, ScenePrimitive::Text { text, .. }
                if text.contains("Open P&L")
                    || text.contains("Target")
                    || text.contains("Stop"))
        }));
        assert!(scene.primitives().iter().any(|primitive| {
            matches!(primitive, ScenePrimitive::FilledRect { radius, .. } if *radius == 0.0)
        }));
    }

    #[test]
    fn drawing_scene_skips_hidden_fib_levels() {
        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let bars = vec![bar(1_700_000_000, 100.0, 101.0, 10.0)];
        let mut spec = DrawingSpec::new(
            DrawingId::new(42),
            DrawingKind::FibRetracement,
            PaneId::Main,
            vec![
                DrawingAnchor::LogicalPrice {
                    logical: 0.0,
                    price: 99.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 2.0,
                    price: 104.0,
                },
            ],
        );
        let Some(DrawingPayload::FibRetracement(settings)) = spec.payload.as_mut() else {
            panic!("fib settings");
        };
        settings.trend_line_visible = false;
        settings.levels[3].visible = false;
        let mut store = DrawingStore::new();
        let fib = store.insert(spec).expect("fib retracement");

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot.clone(),
            drawings: &store,
            bars: &bars,
            selected: Some(fib),
            hovered: None,
            symbol_filter: None,
            interval_seconds: Some(60),
        });

        assert!(!scene.primitives().iter().any(|primitive| {
            matches!(primitive, ScenePrimitive::Text { text, .. } if text.starts_with("0.5 "))
        }));
    }

    #[test]
    fn drawing_scene_declutters_fib_labels_when_levels_are_tight() {
        assert!(fib_label_has_vertical_room(10.0, &[40.0], 12.0));
        assert!(!fib_label_has_vertical_room(10.0, &[22.0], 12.0));

        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let bars = Vec::<OhlcBar>::new();
        let mut store = DrawingStore::new();
        store
            .insert(DrawingSpec::new(
                DrawingId::new(44),
                DrawingKind::FibRetracement,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 0.0,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 2.0,
                        price: 100.4,
                    },
                ],
            ))
            .expect("fib retracement");

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout,
            snapshot: scene_snapshot,
            drawings: &store,
            bars: &bars,
            selected: None,
            hovered: None,
            symbol_filter: None,
            interval_seconds: None,
        });
        let fib_label_count = scene
            .primitives()
            .iter()
            .filter(|primitive| {
                matches!(
                    primitive,
                    ScenePrimitive::Text { text, .. }
                        if ["0", "0.236", "0.382", "0.5", "0.618", "0.786", "1"]
                            .iter()
                            .any(|prefix| text.starts_with(prefix))
                )
            })
            .count();
        assert!(fib_label_count < 7);
    }

    #[test]
    fn drawing_scene_renders_fib_background_and_default_level_direction() {
        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let bars = vec![bar(1_700_000_000, 100.0, 101.0, 10.0)];
        let mut spec = DrawingSpec::new(
            DrawingId::new(43),
            DrawingKind::FibRetracement,
            PaneId::Main,
            vec![
                DrawingAnchor::LogicalPrice {
                    logical: 0.0,
                    price: 99.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 2.0,
                    price: 104.0,
                },
            ],
        );
        let Some(DrawingPayload::FibRetracement(settings)) = spec.payload.as_mut() else {
            panic!("fib settings");
        };
        settings.trend_line_visible = false;
        settings.background_visible = true;
        settings.background_color = ChartColor::rgba(41, 98, 255, 42);
        settings.levels = vec![
            FibRetracementLevel::new(0.0, ChartColor::rgb(142, 148, 156)),
            FibRetracementLevel::new(0.5, ChartColor::rgb(76, 175, 80)),
            FibRetracementLevel::new(1.0, ChartColor::rgb(142, 148, 156)),
        ];
        let mut store = DrawingStore::new();
        store.insert(spec).expect("fib retracement");

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot.clone(),
            drawings: &store,
            bars: &bars,
            selected: None,
            hovered: None,
            symbol_filter: None,
            interval_seconds: Some(60),
        });

        let background_fills = scene
            .primitives()
            .iter()
            .filter(|primitive| {
                matches!(
                    primitive,
                    ScenePrimitive::FilledRect {
                        color,
                        clip: Some(clip),
                        ..
                    } if *clip == layout.plot && *color == SceneColor::rgba(41, 98, 255, 42)
                )
            })
            .count();
        assert_eq!(background_fills, 2);

        let zero_level_y = scene_snapshot.transform.price_to_y(104.0);
        assert!(scene.primitives().iter().any(|primitive| {
            matches!(
                primitive,
                ScenePrimitive::Text { text, position, .. }
                    if text.starts_with("0 ") && (position.y - zero_level_y).abs() < 0.01
            )
        }));
    }

    #[test]
    fn drawing_scene_skips_fib_levels_outside_visible_pane() {
        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let bars = vec![bar(1_700_000_000, 100.0, 101.0, 10.0)];
        let mut spec = DrawingSpec::new(
            DrawingId::new(45),
            DrawingKind::FibRetracement,
            PaneId::Main,
            vec![
                DrawingAnchor::LogicalPrice {
                    logical: 0.0,
                    price: 99.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 2.0,
                    price: 104.0,
                },
            ],
        );
        let Some(DrawingPayload::FibRetracement(settings)) = spec.payload.as_mut() else {
            panic!("fib settings");
        };
        settings.trend_line_visible = false;
        settings.prices_visible = false;
        settings.levels = vec![
            FibRetracementLevel::new(0.0, ChartColor::rgb(142, 148, 156)),
            FibRetracementLevel::new(1.0, ChartColor::rgb(142, 148, 156)),
            FibRetracementLevel::new(1.618, ChartColor::rgb(255, 152, 0)),
        ];
        let mut store = DrawingStore::new();
        let fib = store.insert(spec).expect("fib retracement");

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot,
            drawings: &store,
            bars: &bars,
            selected: Some(fib),
            hovered: None,
            symbol_filter: None,
            interval_seconds: Some(60),
        });

        assert!(!scene.primitives().iter().any(|primitive| {
            matches!(
                primitive,
                ScenePrimitive::Line {
                    stroke,
                    clip: Some(clip),
                    ..
                } if *clip == layout.plot
                    && stroke.color == SceneColor::rgb(255, 152, 0)
            )
        }));
        assert!(!scene.primitives().iter().any(|primitive| {
            matches!(primitive, ScenePrimitive::Text { text, .. } if text.starts_with("1.618"))
        }));
    }

    #[test]
    fn drawing_scene_reverses_fib_ladder_when_reverse_is_enabled() {
        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let bars = vec![bar(1_700_000_000, 100.0, 101.0, 10.0)];
        let mut spec = DrawingSpec::new(
            DrawingId::new(44),
            DrawingKind::FibRetracement,
            PaneId::Main,
            vec![
                DrawingAnchor::LogicalPrice {
                    logical: 0.0,
                    price: 99.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 2.0,
                    price: 104.0,
                },
            ],
        );
        let Some(DrawingPayload::FibRetracement(settings)) = spec.payload.as_mut() else {
            panic!("fib settings");
        };
        settings.trend_line_visible = false;
        settings.reverse = true;
        settings.levels = vec![
            FibRetracementLevel::new(0.0, ChartColor::rgb(142, 148, 156)),
            FibRetracementLevel::new(1.0, ChartColor::rgb(142, 148, 156)),
        ];
        let mut store = DrawingStore::new();
        store.insert(spec).expect("fib retracement");

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout,
            snapshot: scene_snapshot.clone(),
            drawings: &store,
            bars: &bars,
            selected: None,
            hovered: None,
            symbol_filter: None,
            interval_seconds: Some(60),
        });

        let zero_level_y = scene_snapshot.transform.price_to_y(99.0);
        assert!(scene.primitives().iter().any(|primitive| {
            matches!(
                primitive,
                ScenePrimitive::Text { text, position, .. }
                    if text.starts_with("0 ") && (position.y - zero_level_y).abs() < 0.01
            )
        }));
    }

    #[test]
    fn drawing_scene_offsets_fib_labels_above_and_below_lines() {
        let base = ScenePoint::new(20.0, 80.0);
        let end = ScenePoint::new(120.0, 80.0);
        let mut settings = FibRetracementSettings::default();
        assert_eq!(
            fib_retracement_label_placement(base, end, &settings, 12.0),
            (ScenePoint::new(26.0, 80.0), SceneTextAnchor::LeftCenter)
        );

        settings.label_position = FibRetracementLabelPosition::Above;
        assert!(
            fib_retracement_label_placement(base, end, &settings, 12.0)
                .0
                .y
                < base.y
        );

        settings.label_position = FibRetracementLabelPosition::Below;
        assert!(
            fib_retracement_label_placement(base, end, &settings, 12.0)
                .0
                .y
                > base.y
        );

        settings.label_position = FibRetracementLabelPosition::OnLine;
        settings.label_side = FibRetracementLabelSide::Right;
        assert_eq!(
            fib_retracement_label_placement(base, end, &settings, 12.0),
            (ScenePoint::new(114.0, 80.0), SceneTextAnchor::RightCenter)
        );
    }

    #[test]
    fn drawing_scene_uses_study_pane_ranges_and_skips_hidden_drawings() {
        let layout = layout();
        let study = layout.pane_rect(PaneId::Study(2)).expect("study pane");
        let bars = vec![bar(1_700_000_000, 100.0, 101.0, 10.0)];
        let mut store = DrawingStore::new();
        let visible = store
            .create(
                DrawingKind::HorizontalLine,
                PaneId::Study(2),
                vec![DrawingAnchor::PriceOnly { price: 50.0 }],
            )
            .expect("study line");
        let hidden = store
            .create(
                DrawingKind::HorizontalLine,
                PaneId::Study(2),
                vec![DrawingAnchor::PriceOnly { price: 70.0 }],
            )
            .expect("hidden line");
        store.set_visible(hidden, false);

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: snapshot(&layout),
            drawings: &store,
            bars: &bars,
            selected: None,
            hovered: None,
            symbol_filter: None,
            interval_seconds: Some(60),
        });

        assert!(scene.primitives().iter().any(|primitive| {
            matches!(
                primitive,
                ScenePrimitive::Line {
                    from,
                    to,
                    clip: Some(clip),
                    ..
                } if *clip == study
                    && (from.y - study.center().y).abs() < 1e-4
                    && (to.y - study.center().y).abs() < 1e-4
            )
        }));
        assert_eq!(scene.len(), 1);
        assert_eq!(
            store
                .visible()
                .map(|drawing| drawing.id)
                .collect::<Vec<_>>(),
            vec![visible]
        );
    }

    #[test]
    fn drawing_scene_filters_drawings_by_symbol_key() {
        let layout = layout();
        let bars = vec![bar(1_700_000_000, 100.0, 101.0, 10.0)];
        let mut store = DrawingStore::new();
        store
            .insert(
                DrawingSpec::new(
                    DrawingId::new(1),
                    DrawingKind::HorizontalLine,
                    PaneId::Main,
                    vec![DrawingAnchor::PriceOnly { price: 100.0 }],
                )
                .with_symbol("YFinance:NVDA"),
            )
            .expect("matching drawing");
        store
            .insert(
                DrawingSpec::new(
                    DrawingId::new(2),
                    DrawingKind::HorizontalLine,
                    PaneId::Main,
                    vec![DrawingAnchor::PriceOnly { price: 101.0 }],
                )
                .with_symbol("YFinance:AAPL"),
            )
            .expect("other symbol drawing");

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: snapshot(&layout),
            drawings: &store,
            bars: &bars,
            selected: None,
            hovered: None,
            symbol_filter: Some("YFinance:NVDA"),
            interval_seconds: Some(60),
        });

        assert_eq!(scene.len(), 1);
    }

    #[test]
    fn drawing_scene_clips_offscreen_trendline_and_ray_segments() {
        let layout = layout();
        let scene_snapshot = SceneSnapshot {
            visible_bounds: Some((20, 30)),
            price_range: PriceRange {
                min: 98.0,
                max: 105.0,
            },
            volume_range: None,
            study_ranges: Vec::new(),
            transform: SceneTransform::new(
                layout.plot,
                LogicalRange {
                    from: 20.0,
                    to: 30.0,
                },
                PriceRange {
                    min: 98.0,
                    max: 105.0,
                },
            ),
        };
        let bars = vec![bar(1_700_000_000, 100.0, 101.0, 10.0)];
        let mut store = DrawingStore::new();
        store
            .insert(DrawingSpec::new(
                DrawingId::new(70),
                DrawingKind::TrendLine,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 0.0,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 100.0,
                        price: 102.0,
                    },
                ],
            ))
            .expect("trend line");
        store
            .insert(DrawingSpec::new(
                DrawingId::new(71),
                DrawingKind::Ray,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 0.0,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 100.0,
                        price: 102.0,
                    },
                ],
            ))
            .expect("ray");

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot,
            drawings: &store,
            bars: &bars,
            selected: None,
            hovered: None,
            symbol_filter: None,
            interval_seconds: Some(60),
        });
        let line_segments = scene
            .primitives()
            .iter()
            .filter_map(|primitive| match primitive {
                ScenePrimitive::Line {
                    from,
                    to,
                    clip: Some(clip),
                    ..
                } if *clip == layout.plot => Some((*from, *to)),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(line_segments.len(), 2);
        for (from, to) in line_segments {
            assert!(layout.plot.contains(from), "line start was {from:?}");
            assert!(layout.plot.contains(to), "line end was {to:?}");
        }
    }

    #[test]
    fn drawing_scene_keeps_offscreen_ray_on_model_price_during_zoom() {
        let layout = layout();
        let bars = vec![bar(1_700_000_000, 100.0, 101.0, 10.0)];
        let mut store = DrawingStore::new();
        store
            .insert(DrawingSpec::new(
                DrawingId::new(72),
                DrawingKind::Ray,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 0.0,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 100.0,
                        price: 200.0,
                    },
                ],
            ))
            .expect("ray");

        for (visible, probe_logical, expected_price) in [
            (
                LogicalRange {
                    from: 20.0,
                    to: 30.0,
                },
                25.0,
                125.0,
            ),
            (
                LogicalRange {
                    from: 24.0,
                    to: 34.0,
                },
                28.0,
                128.0,
            ),
        ] {
            let scene_snapshot = SceneSnapshot {
                visible_bounds: None,
                price_range: PriceRange {
                    min: 110.0,
                    max: 135.0,
                },
                volume_range: None,
                study_ranges: Vec::new(),
                transform: SceneTransform::new(
                    layout.plot,
                    visible,
                    PriceRange {
                        min: 110.0,
                        max: 135.0,
                    },
                ),
            };

            let scene = build_drawing_scene(DrawingSceneRequest {
                layout: layout.clone(),
                snapshot: scene_snapshot.clone(),
                drawings: &store,
                bars: &bars,
                selected: None,
                hovered: None,
                symbol_filter: None,
                interval_seconds: Some(60),
            });
            let (from, to) = scene
                .primitives()
                .iter()
                .find_map(|primitive| match primitive {
                    ScenePrimitive::Line {
                        from,
                        to,
                        clip: Some(clip),
                        ..
                    } if *clip == layout.plot => Some((*from, *to)),
                    _ => None,
                })
                .expect("visible ray segment");
            let probe_x = scene_snapshot.transform.logical_to_x(probe_logical);
            let t = (probe_x - from.x) / (to.x - from.x);
            let y_on_ray = from.y + (to.y - from.y) * t;
            let expected_y = scene_snapshot.transform.price_to_y(expected_price);

            assert!(
                (y_on_ray - expected_y).abs() < 0.01,
                "ray y was {y_on_ray}, expected {expected_y} for visible {visible:?}",
            );
        }
    }

    #[test]
    fn drawing_scene_renders_horizontal_ray_from_origin_to_right_edge() {
        let layout = layout();
        let bars = vec![bar(1_700_000_000, 100.0, 101.0, 10.0)];
        let scene_snapshot = SceneSnapshot {
            visible_bounds: None,
            price_range: PriceRange {
                min: 90.0,
                max: 110.0,
            },
            volume_range: None,
            study_ranges: Vec::new(),
            transform: SceneTransform::new(
                layout.plot,
                LogicalRange {
                    from: 10.0,
                    to: 20.0,
                },
                PriceRange {
                    min: 90.0,
                    max: 110.0,
                },
            ),
        };
        let mut store = DrawingStore::new();
        let ray = store
            .insert(DrawingSpec::new(
                DrawingId::new(75),
                DrawingKind::HorizontalRay,
                PaneId::Main,
                vec![DrawingAnchor::LogicalPrice {
                    logical: 5.0,
                    price: 100.0,
                }],
            ))
            .expect("horizontal ray");

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot.clone(),
            drawings: &store,
            bars: &bars,
            selected: None,
            hovered: None,
            symbol_filter: None,
            interval_seconds: Some(60),
        });
        let expected_y = scene_snapshot.transform.price_to_y(100.0);
        let (from, to) = scene
            .primitives()
            .iter()
            .find_map(|primitive| match primitive {
                ScenePrimitive::Line {
                    from,
                    to,
                    clip: Some(clip),
                    ..
                } if *clip == layout.plot => Some((*from, *to)),
                _ => None,
            })
            .expect("visible horizontal ray");

        assert!((from.x - layout.plot.left()).abs() < f32::EPSILON);
        assert!((to.x - layout.plot.right()).abs() < f32::EPSILON);
        assert!((from.y - expected_y).abs() < f32::EPSILON);
        assert!((to.y - expected_y).abs() < f32::EPSILON);

        let hit = hit_test_drawings(DrawingHitTestRequest {
            layout: &layout,
            snapshot: &scene_snapshot,
            drawings: &store,
            bars: &bars,
            point: ScenePoint::new(layout.plot.center().x, expected_y),
            tolerance: 2.0,
            symbol_filter: None,
            selected: None,
            hovered: None,
            interval_seconds: Some(60),
        })
        .expect("horizontal ray hit");

        assert_eq!(hit.drawing_id, ray);
        assert_eq!(hit.target, DrawingHitTarget::Edge);
    }

    #[test]
    fn drawing_scene_keeps_offscreen_trendline_on_model_price_during_zoom() {
        let layout = layout();
        let bars = vec![bar(1_700_000_000, 100.0, 101.0, 10.0)];
        let mut store = DrawingStore::new();
        store
            .insert(DrawingSpec::new(
                DrawingId::new(74),
                DrawingKind::TrendLine,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 0.0,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 100.0,
                        price: 200.0,
                    },
                ],
            ))
            .expect("trend line");
        let scene_snapshot = SceneSnapshot {
            visible_bounds: None,
            price_range: PriceRange {
                min: 110.0,
                max: 135.0,
            },
            volume_range: None,
            study_ranges: Vec::new(),
            transform: SceneTransform::new(
                layout.plot,
                LogicalRange {
                    from: 24.0,
                    to: 34.0,
                },
                PriceRange {
                    min: 110.0,
                    max: 135.0,
                },
            ),
        };
        let scene = build_drawing_scene(DrawingSceneRequest {
            layout: layout.clone(),
            snapshot: scene_snapshot.clone(),
            drawings: &store,
            bars: &bars,
            selected: None,
            hovered: None,
            symbol_filter: None,
            interval_seconds: Some(60),
        });
        let (from, to) = scene
            .primitives()
            .iter()
            .find_map(|primitive| match primitive {
                ScenePrimitive::Line {
                    from,
                    to,
                    clip: Some(clip),
                    ..
                } if *clip == layout.plot => Some((*from, *to)),
                _ => None,
            })
            .expect("visible trendline segment");
        let probe_x = scene_snapshot.transform.logical_to_x(28.0);
        let t = (probe_x - from.x) / (to.x - from.x);
        let y_on_line = from.y + (to.y - from.y) * t;
        let expected_y = scene_snapshot.transform.price_to_y(128.0);

        assert!(
            (y_on_line - expected_y).abs() < 0.01,
            "trendline y was {y_on_line}, expected {expected_y}",
        );
    }

    #[test]
    fn drawing_hit_testing_uses_model_space_ray_when_anchor_prices_are_offscreen() {
        let layout = layout();
        let scene_snapshot = SceneSnapshot {
            visible_bounds: None,
            price_range: PriceRange {
                min: 110.0,
                max: 135.0,
            },
            volume_range: None,
            study_ranges: Vec::new(),
            transform: SceneTransform::new(
                layout.plot,
                LogicalRange {
                    from: 20.0,
                    to: 30.0,
                },
                PriceRange {
                    min: 110.0,
                    max: 135.0,
                },
            ),
        };
        let bars = vec![bar(1_700_000_000, 100.0, 101.0, 10.0)];
        let mut store = DrawingStore::new();
        let ray = store
            .insert(DrawingSpec::new(
                DrawingId::new(73),
                DrawingKind::Ray,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 0.0,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 100.0,
                        price: 200.0,
                    },
                ],
            ))
            .expect("ray");
        let true_model_point = ScenePoint::new(
            scene_snapshot.transform.logical_to_x(25.0),
            scene_snapshot.transform.price_to_y(125.0),
        );
        let distorted_old_point = ScenePoint::new(
            scene_snapshot.transform.logical_to_x(25.0),
            scene_snapshot.transform.price_to_y(116.0),
        );

        let hit = hit_test_drawings(DrawingHitTestRequest {
            layout: &layout,
            snapshot: &scene_snapshot,
            drawings: &store,
            bars: &bars,
            point: true_model_point,
            tolerance: 2.0,
            symbol_filter: None,
            selected: None,
            hovered: None,
            interval_seconds: Some(60),
        })
        .expect("ray hit");

        assert_eq!(hit.drawing_id, ray);
        assert_eq!(hit.target, DrawingHitTarget::Edge);
        assert_eq!(
            hit_test_drawings(DrawingHitTestRequest {
                layout: &layout,
                snapshot: &scene_snapshot,
                drawings: &store,
                bars: &bars,
                point: distorted_old_point,
                tolerance: 2.0,
                symbol_filter: None,
                selected: None,
                hovered: None,
                interval_seconds: Some(60),
            }),
            None
        );
    }

    #[test]
    fn drawing_hit_testing_returns_nearest_visible_selectable_geometry() {
        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let bars = vec![
            bar(1_700_000_000, 100.0, 101.0, 10.0),
            bar(1_700_000_060, 101.0, 99.0, 20.0),
        ];
        let mut store = DrawingStore::new();
        let line = store
            .create(
                DrawingKind::HorizontalLine,
                PaneId::Main,
                vec![DrawingAnchor::PriceOnly { price: 101.0 }],
            )
            .expect("line");
        store
            .insert(
                DrawingSpec::new(
                    DrawingId::new(20),
                    DrawingKind::AnchoredText,
                    PaneId::Main,
                    vec![DrawingAnchor::Percent { x: 0.1, y: 0.1 }],
                )
                .with_text("Hidden")
                .with_selectable(false),
            )
            .expect("unselectable text");
        let y = scene_snapshot.transform.price_to_y(101.0);

        let hit = hit_test_drawings(DrawingHitTestRequest {
            layout: &layout,
            snapshot: &scene_snapshot,
            drawings: &store,
            bars: &bars,
            point: ScenePoint::new(layout.plot.center().x, y + 1.0),
            tolerance: 4.0,
            symbol_filter: None,
            selected: None,
            hovered: None,
            interval_seconds: Some(60),
        })
        .expect("line hit");

        assert_eq!(hit.drawing_id, line);
        assert_eq!(hit.target, DrawingHitTarget::Edge);

        store.set_visible(line, false);
        assert_eq!(
            hit_test_drawings(DrawingHitTestRequest {
                layout: &layout,
                snapshot: &scene_snapshot,
                drawings: &store,
                bars: &bars,
                point: ScenePoint::new(layout.plot.center().x, y + 1.0),
                tolerance: 4.0,
                symbol_filter: None,
                selected: None,
                hovered: None,
                interval_seconds: Some(60),
            }),
            None
        );
    }

    #[test]
    fn drawing_hit_testing_prioritizes_anchor_handles() {
        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let bars = vec![
            bar(1_700_000_000, 100.0, 101.0, 10.0),
            bar(1_700_000_060, 101.0, 99.0, 20.0),
        ];
        let mut store = DrawingStore::new();
        let line = store
            .insert(DrawingSpec::new(
                DrawingId::new(40),
                DrawingKind::TrendLine,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 0.0,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 1.0,
                        price: 102.0,
                    },
                ],
            ))
            .expect("trend line");
        let point = ScenePoint::new(
            scene_snapshot.transform.logical_to_x(1.0),
            scene_snapshot.transform.price_to_y(102.0),
        );

        let hit = hit_test_drawings(DrawingHitTestRequest {
            layout: &layout,
            snapshot: &scene_snapshot,
            drawings: &store,
            bars: &bars,
            point,
            tolerance: 4.0,
            symbol_filter: None,
            selected: Some(line),
            hovered: None,
            interval_seconds: Some(60),
        })
        .expect("endpoint hit");

        assert_eq!(hit.drawing_id, line);
        assert_eq!(hit.target, DrawingHitTarget::Anchor(1));
    }

    #[test]
    fn drawing_hit_testing_breaks_ties_by_selection_z_order_and_id() {
        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let bars = vec![
            bar(1_700_000_000, 100.0, 101.0, 10.0),
            bar(1_700_000_060, 101.0, 99.0, 20.0),
        ];
        let mut store = DrawingStore::new();
        let lower_id = store
            .insert(
                DrawingSpec::new(
                    DrawingId::new(50),
                    DrawingKind::HorizontalLine,
                    PaneId::Main,
                    vec![DrawingAnchor::PriceOnly { price: 101.0 }],
                )
                .with_z_order(2),
            )
            .expect("lower id");
        let higher_id = store
            .insert(
                DrawingSpec::new(
                    DrawingId::new(51),
                    DrawingKind::HorizontalLine,
                    PaneId::Main,
                    vec![DrawingAnchor::PriceOnly { price: 101.0 }],
                )
                .with_z_order(2),
            )
            .expect("higher id");
        let y = scene_snapshot.transform.price_to_y(101.0);
        let id_tie_hit = hit_test_drawings(DrawingHitTestRequest {
            layout: &layout,
            snapshot: &scene_snapshot,
            drawings: &store,
            bars: &bars,
            point: ScenePoint::new(layout.plot.center().x, y),
            tolerance: 4.0,
            symbol_filter: None,
            selected: None,
            hovered: None,
            interval_seconds: Some(60),
        })
        .expect("id tie hit");
        assert_eq!(id_tie_hit.drawing_id, higher_id);

        store.send_to_back(higher_id).expect("send back");
        let z_order_hit = hit_test_drawings(DrawingHitTestRequest {
            layout: &layout,
            snapshot: &scene_snapshot,
            drawings: &store,
            bars: &bars,
            point: ScenePoint::new(layout.plot.center().x, y),
            tolerance: 4.0,
            symbol_filter: None,
            selected: None,
            hovered: None,
            interval_seconds: Some(60),
        })
        .expect("z tie hit");
        assert_eq!(z_order_hit.drawing_id, lower_id);

        let selected_low_z = store
            .insert(
                DrawingSpec::new(
                    DrawingId::new(52),
                    DrawingKind::TrendLine,
                    PaneId::Main,
                    vec![
                        DrawingAnchor::LogicalPrice {
                            logical: 0.0,
                            price: 100.0,
                        },
                        DrawingAnchor::LogicalPrice {
                            logical: 1.0,
                            price: 102.0,
                        },
                    ],
                )
                .with_z_order(0),
            )
            .expect("selected");
        let unselected_high_z = store
            .insert(
                DrawingSpec::new(
                    DrawingId::new(53),
                    DrawingKind::TrendLine,
                    PaneId::Main,
                    vec![
                        DrawingAnchor::LogicalPrice {
                            logical: 0.0,
                            price: 100.0,
                        },
                        DrawingAnchor::LogicalPrice {
                            logical: 1.0,
                            price: 102.0,
                        },
                    ],
                )
                .with_z_order(10),
            )
            .expect("unselected");
        let endpoint = ScenePoint::new(
            scene_snapshot.transform.logical_to_x(1.0),
            scene_snapshot.transform.price_to_y(102.0),
        );
        let selected_hit = hit_test_drawings(DrawingHitTestRequest {
            layout: &layout,
            snapshot: &scene_snapshot,
            drawings: &store,
            bars: &bars,
            point: endpoint,
            tolerance: 4.0,
            symbol_filter: None,
            selected: Some(selected_low_z),
            hovered: Some(unselected_high_z),
            interval_seconds: Some(60),
        })
        .expect("selected anchor tie hit");
        assert_eq!(selected_hit.drawing_id, selected_low_z);
        assert_eq!(selected_hit.target, DrawingHitTarget::Anchor(1));
    }

    #[test]
    fn selected_rectangle_renders_four_corner_handles() {
        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let bars = vec![
            bar(1_700_000_000, 100.0, 101.0, 10.0),
            bar(1_700_000_060, 101.0, 99.0, 20.0),
        ];
        let mut store = DrawingStore::new();
        let rectangle = store
            .insert(DrawingSpec::new(
                DrawingId::new(41),
                DrawingKind::Rectangle,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 0.0,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 1.0,
                        price: 102.0,
                    },
                ],
            ))
            .expect("rectangle");

        let scene = build_drawing_scene(DrawingSceneRequest {
            layout,
            snapshot: scene_snapshot,
            drawings: &store,
            bars: &bars,
            selected: Some(rectangle),
            hovered: None,
            symbol_filter: None,
            interval_seconds: Some(60),
        });
        let handle_count = scene
            .primitives()
            .iter()
            .filter(|primitive| {
                matches!(
                    primitive,
                    ScenePrimitive::FilledRect { rect, radius, .. }
                        if (*radius - 1.5).abs() < 1e-6
                            && (rect.width() - 7.0).abs() < 1e-6
                            && (rect.height() - 7.0).abs() < 1e-6
                )
            })
            .count();

        assert_eq!(handle_count, 4);
    }

    #[test]
    fn rectangle_hit_testing_exposes_four_corner_handles() {
        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let bars = vec![
            bar(1_700_000_000, 100.0, 101.0, 10.0),
            bar(1_700_000_060, 101.0, 99.0, 20.0),
        ];
        let mut store = DrawingStore::new();
        let rectangle = store
            .insert(DrawingSpec::new(
                DrawingId::new(42),
                DrawingKind::Rectangle,
                PaneId::Main,
                vec![
                    DrawingAnchor::LogicalPrice {
                        logical: 0.0,
                        price: 100.0,
                    },
                    DrawingAnchor::LogicalPrice {
                        logical: 1.0,
                        price: 102.0,
                    },
                ],
            ))
            .expect("rectangle");
        let top_right = ScenePoint::new(
            scene_snapshot.transform.logical_to_x(1.0),
            scene_snapshot.transform.price_to_y(102.0),
        );
        let bottom_left = ScenePoint::new(
            scene_snapshot.transform.logical_to_x(0.0),
            scene_snapshot.transform.price_to_y(100.0),
        );

        let top_right_hit = hit_test_drawings(DrawingHitTestRequest {
            layout: &layout,
            snapshot: &scene_snapshot,
            drawings: &store,
            bars: &bars,
            point: top_right,
            tolerance: 4.0,
            symbol_filter: None,
            selected: Some(rectangle),
            hovered: None,
            interval_seconds: Some(60),
        })
        .expect("top-right handle hit");
        let bottom_left_hit = hit_test_drawings(DrawingHitTestRequest {
            layout: &layout,
            snapshot: &scene_snapshot,
            drawings: &store,
            bars: &bars,
            point: bottom_left,
            tolerance: 4.0,
            symbol_filter: None,
            selected: Some(rectangle),
            hovered: None,
            interval_seconds: Some(60),
        })
        .expect("bottom-left handle hit");

        assert_eq!(top_right_hit.drawing_id, rectangle);
        assert_eq!(top_right_hit.target, DrawingHitTarget::Anchor(1));
        assert_eq!(bottom_left_hit.drawing_id, rectangle);
        assert_eq!(bottom_left_hit.target, DrawingHitTarget::Anchor(3));
    }

    #[test]
    fn empty_scene_has_no_primitives() {
        assert!(RenderScene::new().is_empty());
    }

    #[test]
    fn cache_key_changes_by_generation_inputs() {
        let layout = layout();
        let scene_snapshot = snapshot(&layout);
        let key = RenderCacheKey::from_scene(&layout, &scene_snapshot, generations(1, 2, 3, 4));

        assert_ne!(
            key,
            RenderCacheKey::from_scene(&layout, &scene_snapshot, generations(9, 2, 3, 4))
        );
        assert_ne!(
            key,
            RenderCacheKey::from_scene(&layout, &scene_snapshot, generations(1, 9, 3, 4))
        );
        assert_ne!(
            key,
            RenderCacheKey::from_scene(&layout, &scene_snapshot, generations(1, 2, 9, 4))
        );
        assert_ne!(
            key,
            RenderCacheKey::from_scene(&layout, &scene_snapshot, generations(1, 2, 3, 9))
        );
        assert_ne!(
            key,
            RenderCacheKey::from_scene(
                &layout,
                &scene_snapshot,
                generations(1, 2, 3, 4).with_drawing_generation(9),
            )
        );

        let mut moved_layout = layout.clone();
        moved_layout.plot =
            SceneRect::from_min_max(ScenePoint::new(9.0, 34.0), ScenePoint::new(310.0, 200.0));
        assert_ne!(
            key.layout_generation,
            RenderCacheKey::from_scene(
                &moved_layout,
                &snapshot(&moved_layout),
                generations(1, 2, 3, 4),
            )
            .layout_generation
        );

        let mut zoomed_snapshot = scene_snapshot.clone();
        zoomed_snapshot.transform.time = LogicalRange { from: 1.0, to: 3.0 };
        assert_ne!(
            key.scale_generation,
            RenderCacheKey::from_scene(&layout, &zoomed_snapshot, generations(1, 2, 3, 4))
                .scale_generation
        );

        let mut volume_snapshot = scene_snapshot.clone();
        volume_snapshot.volume_range = Some(PriceRange {
            min: 0.0,
            max: 60.0,
        });
        assert_ne!(
            key.scale_generation,
            RenderCacheKey::from_scene(&layout, &volume_snapshot, generations(1, 2, 3, 4))
                .scale_generation
        );

        let mut mode_snapshot = scene_snapshot;
        mode_snapshot.transform = mode_snapshot
            .transform
            .with_price_mode(PriceScaleValueMode::Log);
        assert_ne!(
            key.scale_generation,
            RenderCacheKey::from_scene(&layout, &mode_snapshot, generations(1, 2, 3, 4))
                .scale_generation
        );
    }

    #[test]
    fn render_scene_cache_reuses_matching_key_and_rebuilds_on_change() {
        let mut cache = RenderSceneCache::default();
        let key = RenderCacheKey::new(1, 2, 3, 4, 5, 6);
        let changed = RenderCacheKey::new(1, 2, 3, 4, 5, 7);
        let mut builds = 0;

        let first = cache.get_or_build(key, || {
            builds += 1;
            let mut scene = RenderScene::new();
            scene.push(ScenePrimitive::FilledRect {
                rect: SceneRect::from_min_max(ScenePoint::new(0.0, 0.0), ScenePoint::new(1.0, 1.0)),
                radius: 0.0,
                color: SceneColor::rgb(1, 2, 3),
                clip: None,
            });
            scene
        });
        let second = cache.get_or_build(key, || {
            builds += 1;
            RenderScene::new()
        });
        let third = cache.get_or_build(changed, || {
            builds += 1;
            RenderScene::new()
        });

        assert_eq!(builds, 2);
        assert_eq!(first, second);
        assert!(third.is_empty());
        assert_eq!(cache.key(), Some(changed));
    }

    #[test]
    fn candle_width_has_dense_lod_floor() {
        assert_eq!(candle_body_width(1.4), 1.0);
        assert!(candle_body_width(10.0) > 6.0);
    }

    #[test]
    fn formatting_handles_fx_and_large_prices() {
        assert_eq!(format_price(0.651234), "0.65123");
        assert_eq!(format_price(101.23456), "101.23");
        assert_eq!(
            format_price_for_mode(PriceScaleValueMode::Percent { base: 100.0 }, 101.25),
            "+1.25%"
        );
        assert_eq!(
            format_price_for_mode(PriceScaleValueMode::Indexed { base: 100.0 }, 101.25),
            "101.25"
        );
    }

    #[test]
    fn time_label_handles_seconds_or_nanoseconds() {
        assert_eq!(format_time_label(3_600), "01:00");
        assert_eq!(format_time_label(3_600_000_000_000), "01:00");
    }
}
