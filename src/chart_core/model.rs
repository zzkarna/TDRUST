#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogicalRange {
    pub from: f64,
    pub to: f64,
}

impl LogicalRange {
    pub fn new(from: f64, to: f64) -> Option<Self> {
        (from.is_finite() && to.is_finite() && to >= from).then_some(Self { from, to })
    }

    pub fn span(self) -> f64 {
        (self.to - self.from + 1.0).max(0.000_001)
    }

    pub fn center(self) -> f64 {
        (self.from + self.to) * 0.5
    }

    pub fn shifted(self, delta: f64) -> Self {
        Self {
            from: self.from + delta,
            to: self.to + delta,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OhlcBar {
    pub time: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

impl OhlcBar {
    pub fn is_valid(self) -> bool {
        self.open.is_finite()
            && self.high.is_finite()
            && self.low.is_finite()
            && self.close.is_finite()
            && self.volume.is_finite()
            && self.high >= self.low
            && self.high >= self.open.max(self.close)
            && self.low <= self.open.min(self.close)
            && self.volume >= 0.0
    }
}

pub trait BarSeries {
    fn len(&self) -> usize;
    fn bar(&self, index: usize) -> Option<OhlcBar>;

    fn generation(&self) -> u64 {
        0
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn last_bar(&self) -> Option<OhlcBar> {
        self.len().checked_sub(1).and_then(|index| self.bar(index))
    }
}

impl BarSeries for [OhlcBar] {
    fn len(&self) -> usize {
        <[OhlcBar]>::len(self)
    }

    fn bar(&self, index: usize) -> Option<OhlcBar> {
        self.get(index).copied()
    }
}

impl<const N: usize> BarSeries for [OhlcBar; N] {
    fn len(&self) -> usize {
        N
    }

    fn bar(&self, index: usize) -> Option<OhlcBar> {
        self.get(index).copied()
    }
}

impl BarSeries for Vec<OhlcBar> {
    fn len(&self) -> usize {
        Vec::len(self)
    }

    fn bar(&self, index: usize) -> Option<OhlcBar> {
        self.as_slice().get(index).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::{BarSeries, LogicalRange, OhlcBar};

    #[test]
    fn logical_range_rejects_empty_or_reversed_ranges() {
        assert!(LogicalRange::new(1.0, 2.0).is_some());
        assert!(LogicalRange::new(2.0, 2.0).is_some());
        assert!(LogicalRange::new(3.0, 2.0).is_none());
    }

    #[test]
    fn ohlc_bar_validation_checks_invariants() {
        assert!(
            OhlcBar {
                time: 1,
                open: 10.0,
                high: 12.0,
                low: 9.0,
                close: 11.0,
                volume: 100.0,
            }
            .is_valid()
        );
        assert!(
            !OhlcBar {
                time: 1,
                open: 10.0,
                high: 9.0,
                low: 12.0,
                close: 11.0,
                volume: 100.0,
            }
            .is_valid()
        );
    }

    #[test]
    fn bar_series_reads_slices_and_arrays_by_index() {
        let bars = [
            OhlcBar {
                time: 1,
                open: 10.0,
                high: 12.0,
                low: 9.0,
                close: 11.0,
                volume: 100.0,
            },
            OhlcBar {
                time: 2,
                open: 11.0,
                high: 13.0,
                low: 10.0,
                close: 12.0,
                volume: 200.0,
            },
        ];

        assert_eq!(BarSeries::len(&bars), 2);
        assert_eq!(bars.bar(1).map(|bar| bar.close), Some(12.0));
        assert_eq!(bars.last_bar().map(|bar| bar.time), Some(2));
    }
}
