use eframe::egui;

use super::{
    model::LogicalRange,
    price_scale::{PriceRange, PriceScale, PriceScaleValueMode},
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChartTransform {
    pub plot: egui::Rect,
    pub time: LogicalRange,
    pub price: PriceRange,
    pub price_mode: PriceScaleValueMode,
}

impl ChartTransform {
    pub fn new(plot: egui::Rect, time: LogicalRange, price: PriceRange) -> Self {
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

#[cfg(test)]
mod tests {
    use eframe::egui;

    use super::ChartTransform;
    use crate::tv_chart::{LogicalRange, PriceRange, PriceScaleValueMode};

    #[test]
    fn transform_round_trips_time_and_price() {
        let transform = ChartTransform::new(
            egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(1_000.0, 500.0)),
            LogicalRange {
                from: -4.75,
                to: 95.25,
            },
            PriceRange {
                min: 100.0,
                max: 200.0,
            },
        );

        let x = transform.logical_to_x(12.5);
        assert!((transform.x_to_logical(x) - 12.5).abs() < 1e-6);

        let y = transform.price_to_y(125.0);
        assert!((transform.y_to_price(y) - 125.0).abs() < 1e-6);
    }

    #[test]
    fn transform_round_trips_log_prices() {
        let transform = ChartTransform::new(
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, 400.0)),
            LogicalRange {
                from: 0.0,
                to: 10.0,
            },
            PriceRange {
                min: 10.0,
                max: 1_000.0,
            },
        )
        .with_price_mode(PriceScaleValueMode::Log);

        let y = transform.price_to_y(100.0);

        assert!((transform.y_to_price(y) - 100.0).abs() < 1e-6);
    }

    #[test]
    fn log_transform_changes_candle_vertical_distribution() {
        let plot = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, 400.0));
        let time = LogicalRange {
            from: 0.0,
            to: 10.0,
        };
        let range = PriceRange {
            min: 10.0,
            max: 1_000.0,
        };
        let linear = ChartTransform::new(plot, time, range);
        let log = ChartTransform::new(plot, time, range).with_price_mode(PriceScaleValueMode::Log);

        let linear_y = linear.price_to_y(100.0);
        let log_y = log.price_to_y(100.0);

        assert!((log_y - plot.center().y).abs() < 1e-4);
        assert!(
            (linear_y - log_y).abs() > 120.0,
            "log mode must materially change candle y placement"
        );
    }

    #[test]
    fn transform_round_trips_percent_indexed_and_inverse_prices() {
        let plot = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, 400.0));
        let time = LogicalRange {
            from: 0.0,
            to: 10.0,
        };
        let range = PriceRange {
            min: 80.0,
            max: 120.0,
        };

        for mode in [
            PriceScaleValueMode::Percent { base: 100.0 },
            PriceScaleValueMode::Indexed { base: 100.0 },
            PriceScaleValueMode::Inverse,
        ] {
            let transform = ChartTransform::new(plot, time, range).with_price_mode(mode);
            let y = transform.price_to_y(110.0);

            assert!((transform.y_to_price(y) - 110.0).abs() < 1e-6);
        }
    }
}
