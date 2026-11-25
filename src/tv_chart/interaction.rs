use eframe::egui;

use super::{
    BarSeries, PriceScaleId, PriceScaleValueMode,
    egui_renderer::ChartLayout,
    engine::{TvChartState, current_price_range, should_pan_price_with_plot_drag},
    horizontal_pan_bars, input, should_pan_time, wheel_zoom_multiplier,
};

pub(crate) fn show_price_scale_menu(
    ui: &mut egui::Ui,
    state: &mut TvChartState,
    layout: &ChartLayout,
) {
    let button_rect = price_scale_button_rect(layout);
    let current_mode = state.price_scales.scale(PriceScaleId::Right).value_mode();

    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt("rust_tv_chart_price_scale_menu")
            .max_rect(button_rect),
        |ui| {
            ui.shrink_clip_rect(layout.outer);
            let button = egui::Button::new("")
                .min_size(price_scale_button_size())
                .corner_radius(3.0);
            let (response, _) =
                egui::containers::menu::MenuButton::from_button(button).ui(ui, |ui| {
                    ui.set_min_width(210.0);
                    show_price_scale_menu_item(
                        ui,
                        state.price_scales.scale(PriceScaleId::Right).is_auto(),
                        "Auto (fit data to screen)",
                        state,
                        |state| state.price_scales.reset(PriceScaleId::Right),
                    );
                    ui.separator();
                    show_price_scale_menu_item(
                        ui,
                        price_mode_selected(current_mode, PriceScaleValueMode::Linear),
                        "Regular",
                        state,
                        |state| {
                            state
                                .price_scales
                                .scale_mut(PriceScaleId::Right)
                                .set_value_mode(PriceScaleValueMode::Linear)
                        },
                    );
                    show_price_scale_menu_item(
                        ui,
                        price_mode_selected(
                            current_mode,
                            PriceScaleValueMode::Percent { base: 1.0 },
                        ),
                        "Percent",
                        state,
                        |state| {
                            state
                                .price_scales
                                .scale_mut(PriceScaleId::Right)
                                .set_value_mode(PriceScaleValueMode::Percent { base: 1.0 })
                        },
                    );
                    show_price_scale_menu_item(
                        ui,
                        price_mode_selected(
                            current_mode,
                            PriceScaleValueMode::Indexed { base: 1.0 },
                        ),
                        "Indexed to 100",
                        state,
                        |state| {
                            state
                                .price_scales
                                .scale_mut(PriceScaleId::Right)
                                .set_value_mode(PriceScaleValueMode::Indexed { base: 1.0 })
                        },
                    );
                    show_price_scale_menu_item(
                        ui,
                        price_mode_selected(current_mode, PriceScaleValueMode::Log),
                        "Logarithmic",
                        state,
                        |state| {
                            state
                                .price_scales
                                .scale_mut(PriceScaleId::Right)
                                .set_value_mode(PriceScaleValueMode::Log)
                        },
                    );
                    ui.separator();
                    show_price_scale_menu_item(
                        ui,
                        price_mode_selected(current_mode, PriceScaleValueMode::Inverse),
                        "Invert scale",
                        state,
                        |state| {
                            state
                                .price_scales
                                .scale_mut(PriceScaleId::Right)
                                .set_value_mode(PriceScaleValueMode::Inverse)
                        },
                    );
                });
            paint_settings_icon(ui.painter(), response.rect, response.hovered());
            response.on_hover_text("Price scale");
        },
    );
}

pub(crate) fn handle_chart_input<B: BarSeries + ?Sized>(
    ui: &egui::Ui,
    _response: &egui::Response,
    state: &mut TvChartState,
    layout: &ChartLayout,
    bars: &B,
) {
    let plot_response = ui.interact(
        layout.plot,
        ui.id().with("rust_tv_chart_plot"),
        egui::Sense::click_and_drag(),
    );
    let price_response = ui.interact(
        layout.price_axis,
        ui.id().with("rust_tv_chart_price_axis"),
        egui::Sense::click_and_drag(),
    );
    let time_response = ui.interact(
        layout.time_axis,
        ui.id().with("rust_tv_chart_time_axis"),
        egui::Sense::click_and_drag(),
    );

    let hover = ui
        .input(|input| input.pointer.hover_pos())
        .unwrap_or(layout.plot.center());
    // Raw rectangle containment also matches charts covered by another window.
    // Let egui's layer hit test route gestures to the visible chart only.
    let hovered = ui.is_enabled() && ui.rect_contains_pointer(layout.outer);

    if hovered {
        let current_range = current_price_range(state, bars);
        let gesture_config = state.gesture_config();
        let mut handled = false;
        ui.input(|input_state| {
            for event in &input_state.events {
                match event {
                    egui::Event::MouseWheel {
                        unit,
                        delta,
                        modifiers,
                        ..
                    } => {
                        let delta = input::wheel_delta_points(*unit, *delta, layout.plot.height());
                        if delta == egui::Vec2::ZERO {
                            continue;
                        }
                        handled = true;
                        if layout.price_axis.contains(hover) {
                            let multiplier = wheel_zoom_multiplier(delta.y, gesture_config);
                            state.price_scales.scale_mut(PriceScaleId::Right).zoom_at_y(
                                multiplier,
                                f64::from(hover.y),
                                f64::from(layout.plot.top()),
                                f64::from(layout.plot.height()),
                                current_range,
                            );
                        } else if should_pan_time(delta, *modifiers, gesture_config) {
                            let pan_points = if modifiers.shift && delta.x.abs() <= f32::EPSILON {
                                delta.y
                            } else {
                                delta.x
                            };
                            state.time_scale.pan_by_bars(
                                horizontal_pan_bars(pan_points, state.time_scale.bar_spacing()),
                                bars.len(),
                            );
                        } else if delta.y.abs() > f32::EPSILON {
                            state.time_scale.zoom_at_x(
                                wheel_zoom_multiplier(delta.y, gesture_config),
                                f64::from(hover.x),
                                f64::from(layout.plot.left()),
                                f64::from(layout.plot.width()),
                                bars.len(),
                            );
                        }
                    }
                    egui::Event::Zoom(zoom_delta) => {
                        handled = true;
                        state.time_scale.zoom_at_x(
                            input::pinch_zoom_multiplier(*zoom_delta, gesture_config),
                            f64::from(hover.x),
                            f64::from(layout.plot.left()),
                            f64::from(layout.plot.width()),
                            bars.len(),
                        );
                    }
                    _ => {}
                }
            }
        });
        if handled {
            state.follow_realtime = state.time_scale.follows_realtime();
            ui.ctx().request_repaint();
        }
    }

    let pointer_delta = ui.input(|input_state| input_state.pointer.delta());
    if plot_response.dragged() {
        let current_range = current_price_range(state, bars);
        state
            .time_scale
            .pan_by_pixels(f64::from(pointer_delta.x), bars.len());
        if should_pan_price_with_plot_drag(state) {
            state
                .price_scales
                .scale_mut(PriceScaleId::Right)
                .pan_by_pixels(
                    f64::from(pointer_delta.y),
                    f64::from(layout.plot.height()),
                    current_range,
                );
        }
    }
    if time_response.dragged() {
        state.time_scale.zoom_at_x(
            f64::from((-pointer_delta.x * 0.01).exp()).clamp(0.25, 4.0),
            f64::from(hover.x),
            f64::from(layout.plot.left()),
            f64::from(layout.plot.width()),
            bars.len(),
        );
    }
    if price_response.dragged() {
        let current_range = current_price_range(state, bars);
        let modifiers = ui.input(|input_state| input_state.modifiers);
        if modifiers.ctrl || modifiers.command {
            state
                .price_scales
                .scale_mut(PriceScaleId::Right)
                .pan_by_pixels(
                    f64::from(pointer_delta.y),
                    f64::from(layout.plot.height()),
                    current_range,
                );
        } else {
            state.price_scales.scale_mut(PriceScaleId::Right).zoom_at_y(
                f64::from((-pointer_delta.y * 0.01).exp()).clamp(0.25, 4.0),
                f64::from(hover.y),
                f64::from(layout.plot.top()),
                f64::from(layout.plot.height()),
                current_range,
            );
        }
    }
    if time_response.double_clicked() {
        state
            .time_scale
            .reset(bars.len(), f64::from(layout.plot.width()));
        state.follow_realtime = true;
    }
    if price_response.double_clicked() || plot_response.double_clicked() {
        state.price_scales.reset(PriceScaleId::Right);
    }
}

fn show_price_scale_menu_item(
    ui: &mut egui::Ui,
    selected: bool,
    label: &str,
    state: &mut TvChartState,
    action: impl FnOnce(&mut TvChartState),
) {
    if ui.selectable_label(selected, label).clicked() {
        action(state);
        ui.close();
    }
}

fn price_mode_selected(current: PriceScaleValueMode, candidate: PriceScaleValueMode) -> bool {
    matches!(
        (current, candidate),
        (PriceScaleValueMode::Linear, PriceScaleValueMode::Linear)
            | (PriceScaleValueMode::Log, PriceScaleValueMode::Log)
            | (
                PriceScaleValueMode::Percent { .. },
                PriceScaleValueMode::Percent { .. }
            )
            | (
                PriceScaleValueMode::Indexed { .. },
                PriceScaleValueMode::Indexed { .. }
            )
            | (PriceScaleValueMode::Inverse, PriceScaleValueMode::Inverse)
    )
}

fn price_scale_button_rect(layout: &ChartLayout) -> egui::Rect {
    let button_size = price_scale_button_size();
    let button_pos = egui::pos2(
        layout.price_axis.center().x - button_size.x * 0.5,
        layout.time_axis.center().y - button_size.y * 0.5,
    );
    egui::Rect::from_min_size(button_pos, button_size)
}

fn price_scale_button_size() -> egui::Vec2 {
    egui::vec2(30.0, 22.0)
}

fn paint_settings_icon(painter: &egui::Painter, rect: egui::Rect, hovered: bool) {
    let center = rect.center();
    let color = if hovered {
        egui::Color32::WHITE
    } else {
        egui::Color32::from_rgb(218, 218, 218)
    };
    let stroke = egui::Stroke::new(1.25, color);
    let outer = 7.0;
    let inner = 3.0;

    for index in 0..8 {
        let angle = index as f32 * std::f32::consts::TAU / 8.0;
        let direction = egui::vec2(angle.cos(), angle.sin());
        let from = center + direction * (outer - 1.7);
        let to = center + direction * (outer + 1.7);
        painter.line_segment([from, to], stroke);
    }

    painter.circle_stroke(center, outer - 2.2, stroke);
    painter.circle_stroke(center, inner, stroke);
}

#[cfg(test)]
mod tests {
    use super::price_scale_button_rect;
    use crate::{chart_core::PaneModel, tv_chart::egui_renderer::build_layout};
    use eframe::egui;

    #[test]
    fn overlapping_charts_route_gestures_to_top_layer_only() {
        for event in [
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                phase: egui::TouchPhase::Move,
                delta: egui::vec2(0.0, 40.0),
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                phase: egui::TouchPhase::Move,
                delta: egui::vec2(40.0, 0.0),
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::Zoom(1.2),
        ] {
            for top in [0, 1] {
                let ctx = egui::Context::default();
                let mut states = [
                    super::TvChartState::default(),
                    super::TvChartState::default(),
                ];
                let bars = vec![
                    crate::chart_core::OhlcBar {
                        time: 1,
                        open: 100.0,
                        high: 102.0,
                        low: 98.0,
                        close: 101.0,
                        volume: 1.0,
                    };
                    200
                ];
                for state in &mut states {
                    state.time_scale.reset(bars.len(), 500.0);
                }
                let before = states[0].time_scale;
                // Warm up egui's area geometry before delivering the gesture.
                for frame in 0..3 {
                    let mut raw = egui::RawInput::default();
                    raw.screen_rect = Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 800.0),
                    ));
                    raw.events
                        .push(egui::Event::PointerMoved(egui::pos2(200.0, 200.0)));
                    if frame == 2 {
                        raw.events.push(event.clone());
                    }
                    let _ = ctx.run(raw, |ctx| {
                        for (index, state) in states.iter_mut().enumerate() {
                            egui::Area::new(egui::Id::new(index))
                                .order(if index == top {
                                    egui::Order::Foreground
                                } else {
                                    egui::Order::Middle
                                })
                                .fixed_pos(egui::pos2(20.0, 20.0))
                                .show(ctx, |ui| {
                                    let (rect, response) = ui.allocate_exact_size(
                                        egui::vec2(600.0, 450.0),
                                        egui::Sense::click_and_drag(),
                                    );
                                    let layout = build_layout(rect, &state.panes);
                                    super::handle_chart_input(ui, &response, state, &layout, &bars);
                                });
                        }
                    });
                }
                assert_eq!(
                    states[1 - top].time_scale,
                    before,
                    "covered chart changed: {event:?}"
                );
                assert_ne!(
                    states[top].time_scale, before,
                    "top chart ignored: {event:?}"
                );
            }
        }
    }

    #[test]
    fn price_scale_button_sits_at_price_time_axis_corner() {
        let layout = build_layout(
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1_000.0, 600.0)),
            &PaneModel::default(),
        );
        let rect = price_scale_button_rect(&layout);

        assert_eq!(rect.center().x, layout.price_axis.center().x);
        assert_eq!(rect.center().y, layout.time_axis.center().y);
        assert!(layout.outer.contains(rect.center()));
    }
}
