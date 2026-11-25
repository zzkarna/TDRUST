use eframe::egui;

use crate::{
    chart_core::{BarStore, OhlcBar, normalize_timestamp_seconds},
    market_data::{BarInterval, TIMEFRAMES},
};

#[derive(Debug, Clone, Copy)]
struct TimeframePanel {
    interval: BarInterval,
    label: &'static str,
    seconds: i64,
}

pub fn show_synced_timeframes_widget(
    ui: &mut egui::Ui,
    bars: &BarStore,
    selected_timeframe: BarInterval,
    source_label: &str,
) -> Option<BarInterval> {
    let mut clicked = None;
    egui::Frame::default()
        .fill(egui::Color32::from_rgb(13, 13, 13))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            show_synced_timeframe_header(
                ui,
                selected_timeframe,
                source_label,
                bars.len(),
                &mut clicked,
            );
            ui.add_space(8.0);

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    show_synced_timeframe_grid(ui, bars, selected_timeframe, &mut clicked);
                });
        });
    clicked
}

fn show_synced_timeframe_header(
    ui: &mut egui::Ui,
    selected_timeframe: BarInterval,
    source_label: &str,
    bar_count: usize,
    clicked: &mut Option<BarInterval>,
) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new("Synced views")
                .monospace()
                .size(13.0)
                .color(egui::Color32::from_rgb(154, 160, 168)),
        );
        for panel in timeframe_panels() {
            if ui
                .selectable_label(selected_timeframe == panel.interval, panel.label)
                .clicked()
            {
                *clicked = Some(panel.interval);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{source_label}  {bar_count} bars"))
                    .monospace()
                    .size(11.0)
                    .color(egui::Color32::from_rgb(111, 119, 128)),
            );
        });
    });
}

fn show_synced_timeframe_grid(
    ui: &mut egui::Ui,
    bars: &BarStore,
    selected_timeframe: BarInterval,
    clicked: &mut Option<BarInterval>,
) {
    let available_width = ui.available_width().max(220.0);
    let columns = if available_width > 1_100.0 {
        4
    } else if available_width > 760.0 {
        3
    } else if available_width > 500.0 {
        2
    } else {
        1
    };
    let panel_gap = 8.0;
    let panel_width =
        ((available_width - panel_gap * (columns - 1) as f32) / columns as f32).max(180.0);
    let panel_height = 128.0;
    let base_seconds = infer_base_bar_seconds(bars);

    egui::Grid::new("synced_timeframes_widget_grid")
        .num_columns(columns)
        .spacing([panel_gap, panel_gap])
        .show(ui, |ui| {
            for (index, panel) in timeframe_panels().iter().enumerate() {
                let available = base_seconds.is_none_or(|base| panel.seconds >= base);
                let candles = if available {
                    aggregate_timeframe_bars(bars, panel.seconds)
                } else {
                    Vec::new()
                };
                let selected = selected_timeframe == panel.interval;
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(panel_width, panel_height),
                    egui::Sense::click(),
                );
                paint_synced_timeframe_panel(ui, rect, panel.label, &candles, selected, available);
                if response.clicked() {
                    *clicked = Some(panel.interval);
                }
                if !available {
                    response.on_hover_text("Load a lower timeframe to derive this view.");
                }

                if (index + 1) % columns == 0 {
                    ui.end_row();
                }
            }
        });
}

fn paint_synced_timeframe_panel(
    ui: &egui::Ui,
    rect: egui::Rect,
    label: &str,
    candles: &[OhlcBar],
    selected: bool,
    available: bool,
) {
    let painter = ui.painter_at(rect);
    let border = if selected {
        egui::Color32::from_rgb(125, 190, 255)
    } else {
        egui::Color32::from_rgb(37, 45, 54)
    };
    painter.rect_filled(rect, 4.0, egui::Color32::from_rgb(14, 18, 22));
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(if selected { 1.6 } else { 1.0 }, border),
        egui::StrokeKind::Middle,
    );
    painter.text(
        rect.left_top() + egui::vec2(8.0, 8.0),
        egui::Align2::LEFT_TOP,
        label,
        egui::FontId::monospace(12.0),
        if selected {
            egui::Color32::from_rgb(190, 220, 255)
        } else {
            egui::Color32::from_rgb(174, 184, 194)
        },
    );

    let Some(last) = candles.last() else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            if available { "waiting" } else { "unavailable" },
            egui::FontId::monospace(12.0),
            egui::Color32::from_rgb(118, 128, 138),
        );
        return;
    };

    painter.text(
        rect.right_top() + egui::vec2(-8.0, 8.0),
        egui::Align2::RIGHT_TOP,
        format_compact_price(last.close),
        egui::FontId::monospace(11.0),
        egui::Color32::from_rgb(214, 224, 234),
    );
    painter.text(
        rect.left_bottom() + egui::vec2(8.0, -8.0),
        egui::Align2::LEFT_BOTTOM,
        format!("{} candles", candles.len()),
        egui::FontId::monospace(10.0),
        egui::Color32::from_rgb(118, 128, 138),
    );

    let plot_rect = rect.shrink2(egui::vec2(8.0, 25.0));
    let visible = &candles[candles.len().saturating_sub(80)..];
    let Some((min_price, max_price)) = visible_price_range(visible) else {
        return;
    };
    let span = (max_price - min_price).max(0.000_01);
    let low = min_price - span * 0.06;
    let high = max_price + span * 0.06;
    let price_to_y = |price: f64| {
        let normalized = ((price - low) / (high - low)).clamp(0.0, 1.0) as f32;
        plot_rect.bottom() - normalized * plot_rect.height()
    };
    let x_step = plot_rect.width() / visible.len().max(1) as f32;
    let body_width = (x_step * 0.58).clamp(1.0, 7.0);

    for (index, candle) in visible.iter().enumerate() {
        let x = plot_rect.left() + (index as f32 + 0.5) * x_step;
        let color = if candle.close >= candle.open {
            egui::Color32::from_rgb(92, 184, 128)
        } else {
            egui::Color32::from_rgb(214, 82, 88)
        };
        let y_high = price_to_y(candle.high);
        let y_low = price_to_y(candle.low);
        let y_open = price_to_y(candle.open);
        let y_close = price_to_y(candle.close);
        painter.line_segment(
            [egui::pos2(x, y_high), egui::pos2(x, y_low)],
            egui::Stroke::new(1.0, color),
        );
        let body_top = y_open.min(y_close);
        let body_bottom = y_open.max(y_close);
        let body = egui::Rect::from_min_max(
            egui::pos2(x - body_width / 2.0, body_top),
            egui::pos2(x + body_width / 2.0, body_bottom.max(body_top + 1.0)),
        );
        painter.rect_filled(body, 0.0, color);
    }
}

fn timeframe_panels() -> [TimeframePanel; 12] {
    [
        TimeframePanel::from_interval(TIMEFRAMES[0], "1m", 60),
        TimeframePanel::from_interval(TIMEFRAMES[1], "3m", 3 * 60),
        TimeframePanel::from_interval(TIMEFRAMES[2], "5m", 5 * 60),
        TimeframePanel::from_interval(TIMEFRAMES[3], "15m", 15 * 60),
        TimeframePanel::from_interval(TIMEFRAMES[4], "1h", 60 * 60),
        TimeframePanel::from_interval(TIMEFRAMES[5], "4h", 4 * 60 * 60),
        TimeframePanel::from_interval(TIMEFRAMES[6], "1d", 24 * 60 * 60),
        TimeframePanel::from_interval(TIMEFRAMES[7], "1w", 7 * 24 * 60 * 60),
        TimeframePanel::from_interval(TIMEFRAMES[8], "1M", 30 * 24 * 60 * 60),
        TimeframePanel::from_interval(TIMEFRAMES[9], "3M", 90 * 24 * 60 * 60),
        TimeframePanel::from_interval(TIMEFRAMES[10], "6M", 180 * 24 * 60 * 60),
        TimeframePanel::from_interval(TIMEFRAMES[11], "12M", 365 * 24 * 60 * 60),
    ]
}

impl TimeframePanel {
    const fn from_interval(interval: BarInterval, label: &'static str, seconds: i64) -> Self {
        Self {
            interval,
            label,
            seconds,
        }
    }
}

fn aggregate_timeframe_bars(bars: &BarStore, seconds: i64) -> Vec<OhlcBar> {
    if seconds <= 0 {
        return Vec::new();
    }

    let mut aggregated: Vec<OhlcBar> = Vec::new();
    for bar in bars.iter().copied().filter(|bar| bar.is_valid()) {
        let normalized_time = normalize_timestamp_seconds(bar.time);
        let bucket_time = normalized_time.div_euclid(seconds) * seconds;
        if let Some(current) = aggregated.last_mut()
            && current.time == bucket_time
        {
            current.high = current.high.max(bar.high);
            current.low = current.low.min(bar.low);
            current.close = bar.close;
            current.volume += bar.volume;
            continue;
        }

        aggregated.push(OhlcBar {
            time: bucket_time,
            open: bar.open,
            high: bar.high,
            low: bar.low,
            close: bar.close,
            volume: bar.volume,
        });
    }
    aggregated
}

fn infer_base_bar_seconds(bars: &BarStore) -> Option<i64> {
    let mut previous = None;
    let mut min_delta = i64::MAX;
    for bar in bars.iter() {
        let current = normalize_timestamp_seconds(bar.time);
        if let Some(previous_time) = previous {
            let delta = current - previous_time;
            if delta > 0 {
                min_delta = min_delta.min(delta);
            }
        }
        previous = Some(current);
    }
    (min_delta != i64::MAX).then_some(min_delta.max(1))
}

fn visible_price_range(candles: &[OhlcBar]) -> Option<(f64, f64)> {
    let mut low = f64::INFINITY;
    let mut high = f64::NEG_INFINITY;
    for candle in candles {
        if candle.low.is_finite() && candle.high.is_finite() {
            low = low.min(candle.low);
            high = high.max(candle.high);
        }
    }
    (low.is_finite() && high.is_finite()).then_some((low, high))
}

fn format_compact_price(value: f64) -> String {
    if value.abs() >= 1_000.0 {
        format!("{value:.2}")
    } else if value.abs() >= 1.0 {
        format!("{value:.4}")
    } else {
        format!("{value:.5}")
    }
}

#[cfg(test)]
mod tests {
    use super::{aggregate_timeframe_bars, infer_base_bar_seconds, timeframe_panels};
    use crate::chart_core::{BarStore, OhlcBar};

    fn bar(time: i64, open: f64, high: f64, low: f64, close: f64) -> OhlcBar {
        OhlcBar {
            time,
            open,
            high,
            low,
            close,
            volume: 10.0,
        }
    }

    fn store_with(bars: Vec<OhlcBar>) -> BarStore {
        let mut store = BarStore::unbounded();
        store.replace_all(bars);
        store
    }

    #[test]
    fn synced_timeframe_widget_aggregates_loaded_bars() {
        let store = store_with(vec![
            bar(0, 10.0, 12.0, 9.0, 11.0),
            bar(60, 11.0, 13.0, 10.0, 12.0),
            bar(180, 12.0, 14.0, 11.0, 13.0),
        ]);

        let aggregated = aggregate_timeframe_bars(&store, 180);

        assert_eq!(aggregated.len(), 2);
        assert_eq!(aggregated[0].open, 10.0);
        assert_eq!(aggregated[0].high, 13.0);
        assert_eq!(aggregated[0].low, 9.0);
        assert_eq!(aggregated[0].close, 12.0);
        assert_eq!(aggregated[0].volume, 20.0);
    }

    #[test]
    fn synced_timeframe_widget_infers_loaded_base_interval() {
        let store = store_with(vec![
            bar(0, 10.0, 11.0, 9.0, 10.0),
            bar(300, 10.0, 11.0, 9.0, 10.0),
            bar(600, 10.0, 11.0, 9.0, 10.0),
        ]);

        assert_eq!(infer_base_bar_seconds(&store), Some(300));
    }

    #[test]
    fn synced_timeframe_widget_uses_full_terminal_timeframe_set() {
        let panels = timeframe_panels();

        assert_eq!(panels.len(), 12);
        assert_eq!(panels[0].label, "1m");
        assert_eq!(panels[8].label, "1M");
        assert_eq!(panels[11].label, "12M");
    }
}
