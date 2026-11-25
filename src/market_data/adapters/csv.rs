use std::fs;

use crate::chart_core::OhlcBar;

pub fn load_csv(path: &str) -> Result<Vec<OhlcBar>, String> {
    let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut bars = Vec::new();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let fields: Vec<_> = line.split(',').map(str::trim).collect();
        if fields.len() < 6 {
            continue;
        }

        let parsed = (
            fields[0].parse::<i64>(),
            fields[1].parse::<f64>(),
            fields[2].parse::<f64>(),
            fields[3].parse::<f64>(),
            fields[4].parse::<f64>(),
            fields[5].parse::<f64>(),
        );

        let (Ok(time), Ok(open), Ok(high), Ok(low), Ok(close), Ok(volume)) = parsed else {
            continue;
        };

        let bar = OhlcBar {
            time,
            open,
            high,
            low,
            close,
            volume,
        };
        if bar.is_valid() {
            bars.push(bar);
        }
    }

    Ok(bars)
}
