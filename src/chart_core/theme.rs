#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ChartColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl ChartColor {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChartTheme {
    pub appearance: ChartAppearanceTheme,
    pub candles: CandleTheme,
    pub crosshair: CrosshairTheme,
}

impl Default for ChartTheme {
    fn default() -> Self {
        Self {
            appearance: ChartAppearanceTheme::default(),
            candles: CandleTheme::default(),
            crosshair: CrosshairTheme::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChartAppearanceTheme {
    pub frame_background: ChartColor,
    pub frame_border: ChartColor,
    pub legend_background: ChartColor,
    pub pane_background: ChartColor,
    pub axis_background: ChartColor,
    pub volume_background: ChartColor,
    pub separator: ChartColor,
    pub grid: ChartGridTheme,
    pub axis_text: ChartColor,
    pub legend_text: ChartColor,
    pub empty_text: ChartColor,
    pub last_price_line: ChartColor,
    pub last_price_label_fill: ChartColor,
    pub last_price_label_text: ChartColor,
}

impl Default for ChartAppearanceTheme {
    fn default() -> Self {
        Self {
            frame_background: ChartColor::rgb(8, 10, 12),
            frame_border: ChartColor::rgb(34, 40, 46),
            legend_background: ChartColor::rgb(14, 17, 20),
            pane_background: ChartColor::rgb(8, 10, 12),
            axis_background: ChartColor::rgb(12, 14, 17),
            volume_background: ChartColor::rgb(10, 12, 15),
            separator: ChartColor::rgb(34, 40, 46),
            grid: ChartGridTheme::default(),
            axis_text: ChartColor::rgb(135, 150, 162),
            legend_text: ChartColor::rgba(210, 218, 226, 205),
            empty_text: ChartColor::rgb(145, 155, 165),
            last_price_line: ChartColor::rgba(235, 235, 72, 170),
            last_price_label_fill: ChartColor::rgb(235, 235, 72),
            last_price_label_text: ChartColor::rgb(9, 10, 11),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChartGridTheme {
    pub visible: bool,
    pub price_color: ChartColor,
    pub time_color: ChartColor,
}

impl Default for ChartGridTheme {
    fn default() -> Self {
        Self {
            visible: true,
            price_color: ChartColor::rgba(38, 44, 50, 150),
            time_color: ChartColor::rgba(38, 44, 50, 120),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CandleTheme {
    pub up_body: ChartColor,
    pub down_body: ChartColor,
    pub up_wick: ChartColor,
    pub down_wick: ChartColor,
    pub volume_up: ChartColor,
    pub volume_down: ChartColor,
}

impl Default for CandleTheme {
    fn default() -> Self {
        Self {
            up_body: ChartColor::rgb(54, 190, 132),
            down_body: ChartColor::rgb(232, 74, 92),
            up_wick: ChartColor::rgb(54, 190, 132),
            down_wick: ChartColor::rgb(232, 74, 92),
            volume_up: ChartColor::rgba(54, 190, 132, 110),
            volume_down: ChartColor::rgba(232, 74, 92, 110),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CrosshairTheme {
    pub visible: bool,
    pub line: ChartColor,
    pub label_fill: ChartColor,
    pub label_text: ChartColor,
}

impl Default for CrosshairTheme {
    fn default() -> Self {
        Self {
            visible: true,
            line: ChartColor::rgba(180, 190, 200, 120),
            label_fill: ChartColor::rgb(70, 78, 88),
            label_text: ChartColor::rgb(235, 240, 245),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ChartColor, ChartTheme};

    #[test]
    fn default_theme_preserves_existing_dark_chart_colors() {
        let theme = ChartTheme::default();

        assert_eq!(theme.appearance.pane_background, ChartColor::rgb(8, 10, 12));
        assert_eq!(
            theme.appearance.grid.price_color,
            ChartColor::rgba(38, 44, 50, 150)
        );
        assert_eq!(theme.candles.up_body, ChartColor::rgb(54, 190, 132));
        assert!(theme.crosshair.visible);
    }
}
