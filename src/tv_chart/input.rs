use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GestureConfig {
    pub wheel_zoom_sensitivity: f64,
    pub min_zoom_multiplier: f64,
    pub max_zoom_multiplier: f64,
    pub horizontal_pan_bias: f32,
}

impl Default for GestureConfig {
    fn default() -> Self {
        Self {
            wheel_zoom_sensitivity: 0.0035,
            min_zoom_multiplier: 0.08,
            max_zoom_multiplier: 12.0,
            horizontal_pan_bias: 1.2,
        }
    }
}

pub fn wheel_zoom_multiplier(delta_y: f32, config: GestureConfig) -> f64 {
    let scaled = (f64::from(delta_y) / 120.0).clamp(-1.0, 1.0);
    (1.0 + scaled / 10.0).clamp(config.min_zoom_multiplier, config.max_zoom_multiplier)
}

pub fn pinch_zoom_multiplier(zoom_delta: f32, config: GestureConfig) -> f64 {
    (1.0 + (f64::from(zoom_delta) - 1.0) * 0.5)
        .clamp(config.min_zoom_multiplier, config.max_zoom_multiplier)
}

pub fn horizontal_pan_bars(delta_x: f32, bar_spacing_px: f64) -> f64 {
    -f64::from(delta_x) / bar_spacing_px.max(0.01)
}

pub fn should_pan_time(
    delta: egui::Vec2,
    modifiers: egui::Modifiers,
    config: GestureConfig,
) -> bool {
    modifiers.shift || delta.x.abs() > delta.y.abs() * config.horizontal_pan_bias
}

pub fn wheel_delta_points(
    unit: egui::MouseWheelUnit,
    delta: egui::Vec2,
    page_height: f32,
) -> egui::Vec2 {
    match unit {
        egui::MouseWheelUnit::Point => delta,
        egui::MouseWheelUnit::Line => delta * 50.0,
        egui::MouseWheelUnit::Page => delta * page_height.max(1.0),
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui;

    use super::{
        GestureConfig, horizontal_pan_bars, pinch_zoom_multiplier, should_pan_time,
        wheel_delta_points, wheel_zoom_multiplier,
    };

    #[test]
    fn wheel_zoom_positive_delta_increases_bar_spacing() {
        let config = GestureConfig::default();

        assert!(wheel_zoom_multiplier(120.0, config) > 1.0);
        assert!(wheel_zoom_multiplier(-120.0, config) < 1.0);
    }

    #[test]
    fn pinch_zoom_passes_through_as_bar_spacing_multiplier() {
        let config = GestureConfig::default();

        assert_eq!(pinch_zoom_multiplier(2.0, config), 1.5);
        assert_eq!(pinch_zoom_multiplier(0.5, config), 0.75);
    }

    #[test]
    fn horizontal_pan_converts_pixels_to_bars() {
        assert_eq!(horizontal_pan_bars(40.0, 10.0), -4.0);
        assert_eq!(horizontal_pan_bars(-40.0, 10.0), 4.0);
    }

    #[test]
    fn pan_detection_uses_horizontal_bias_or_shift() {
        let config = GestureConfig::default();

        assert!(should_pan_time(
            egui::vec2(30.0, 5.0),
            egui::Modifiers::default(),
            config
        ));
        assert!(!should_pan_time(
            egui::vec2(5.0, 30.0),
            egui::Modifiers::default(),
            config
        ));
        assert!(should_pan_time(
            egui::vec2(0.0, 30.0),
            egui::Modifiers {
                shift: true,
                ..Default::default()
            },
            config
        ));
    }

    #[test]
    fn wheel_units_convert_to_points_without_allocation() {
        assert_eq!(
            wheel_delta_points(egui::MouseWheelUnit::Point, egui::vec2(1.0, 2.0), 400.0),
            egui::vec2(1.0, 2.0)
        );
        assert_eq!(
            wheel_delta_points(egui::MouseWheelUnit::Line, egui::vec2(1.0, 2.0), 400.0),
            egui::vec2(50.0, 100.0)
        );
        assert_eq!(
            wheel_delta_points(egui::MouseWheelUnit::Page, egui::vec2(1.0, 2.0), 400.0),
            egui::vec2(400.0, 800.0)
        );
    }
}
