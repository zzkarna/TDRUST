#![allow(dead_code)]
#![allow(unused_imports)]

pub mod egui_renderer;
pub mod engine;
pub mod input;
pub mod interaction;
pub mod model;
pub mod price_scale;
pub mod scene;
pub mod time_scale;
pub mod transform;
pub mod widget;

pub use crate::chart_core::{
    ChartColor, ChartTheme, DRAWING_SCHEMA_VERSION, DrawingAnchor, DrawingEditError,
    DrawingGroupId, DrawingGroupSpec, DrawingId, DrawingKind, DrawingLineStyle, DrawingOwner,
    DrawingPayload, DrawingSpec, DrawingStore, DrawingStyle, DrawingValidationError,
    FibRetracementLabelPosition, FibRetracementLabelSide, FibRetracementLevel,
    FibRetracementSettings, MAIN_PRICE_SERIES_ID, OhlcField, PaneId, PaneModel, PaneRect, PaneSpec,
    PositionDrawingSettings, SeriesId, SeriesKind, SeriesRegistry, SeriesSpec, SeriesValueSource,
    VOLUME_SERIES_ID,
};
pub use egui_renderer::EguiChartRenderer;
pub use engine::{ChartSnapshot, TvChartState};
pub use input::{GestureConfig, horizontal_pan_bars, should_pan_time, wheel_zoom_multiplier};
pub use model::{BarSeries, LogicalRange, OhlcBar};
pub use price_scale::{
    PriceRange, PriceScale, PriceScaleDomain, PriceScaleEntry, PriceScaleId, PriceScaleMode,
    PriceScalePlacement, PriceScaleRegistry, PriceScaleValueMode, ScaleMargins,
};
pub use scene::{
    ChartBaseSceneRequest, ChartOverlaySceneRequest, ChartSceneRequest, DrawingHit,
    DrawingHitTarget, DrawingHitTestRequest, DrawingSceneRequest, RenderCacheGenerationInputs,
    RenderCacheKey, RenderScene, RenderSceneCache, SceneColor, SceneFont, SceneFontFamily,
    SceneLayout, SceneLineStyle, ScenePoint, ScenePrimitive, SceneRect, SceneSize, SceneSnapshot,
    SceneStroke, SceneTextAnchor, SceneTransform, build_drawing_scene, hit_test_drawings,
};
pub use time_scale::{
    TimeAxisTick, TimeAxisUnit, TimeScale, TimeScaleOptions, format_time_axis_label,
    visible_time_axis_ticks,
};
pub use transform::ChartTransform;
pub use widget::{
    ChartCrosshairMagnet, ChartCrosshairMagnetMode, ChartDrawingLayer, ChartUiResponse,
    ChartWidgetSizing, show_candlestick_chart, show_candlestick_chart_with_sizing,
};
