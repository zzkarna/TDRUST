use std::collections::VecDeque;

use super::model::{BarSeries, LogicalRange, OhlcBar};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailUpdate {
    Appended,
    ReplacedTail,
    IgnoredStale,
    IgnoredInvalid,
}

#[derive(Debug, Clone)]
pub struct BarStore {
    bars: VecDeque<OhlcBar>,
    max_len: Option<usize>,
    generation: u64,
}

impl Default for BarStore {
    fn default() -> Self {
        Self::unbounded()
    }
}

impl BarStore {
    pub fn unbounded() -> Self {
        Self {
            bars: VecDeque::new(),
            max_len: None,
            generation: 0,
        }
    }

    pub fn new(max_len: Option<usize>) -> Self {
        Self {
            bars: VecDeque::new(),
            max_len,
            generation: 0,
        }
    }

    pub fn replace_all(&mut self, bars: impl IntoIterator<Item = OhlcBar>) {
        let mut bars = sorted_unique_valid_bars(bars);
        if let Some(max_len) = self.max_len {
            let overflow = bars.len().saturating_sub(max_len);
            if overflow > 0 {
                bars = bars.into_iter().skip(overflow).collect();
            }
        }

        self.bars = VecDeque::from(bars);
        self.bump_generation();
    }

    pub fn prepend_history(&mut self, bars: impl IntoIterator<Item = OhlcBar>) -> usize {
        let mut bars = sorted_unique_valid_bars(bars);
        if let Some(first) = self.front() {
            bars.retain(|bar| bar.time < first.time);
        }
        if bars.is_empty() {
            return 0;
        }

        let added = bars.len();
        for bar in bars.into_iter().rev() {
            self.bars.push_front(bar);
        }
        if let Some(max_len) = self.max_len {
            while self.bars.len() > max_len {
                self.bars.pop_back();
            }
        }
        self.bump_generation();
        added
    }

    pub fn apply_tail(&mut self, bar: OhlcBar) -> TailUpdate {
        if !bar.is_valid() {
            return TailUpdate::IgnoredInvalid;
        }

        if let Some(last) = self.bars.back_mut() {
            if last.time == bar.time {
                *last = bar;
                self.bump_generation();
                return TailUpdate::ReplacedTail;
            }
            if last.time > bar.time {
                return TailUpdate::IgnoredStale;
            }
        }

        self.bars.push_back(bar);
        if let Some(max_len) = self.max_len {
            while self.bars.len() > max_len {
                self.bars.pop_front();
            }
        }
        self.bump_generation();
        TailUpdate::Appended
    }

    pub fn len(&self) -> usize {
        self.bars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bars.is_empty()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn front(&self) -> Option<&OhlcBar> {
        self.bars.front()
    }

    pub fn back(&self) -> Option<&OhlcBar> {
        self.bars.back()
    }

    pub fn get(&self, index: usize) -> Option<&OhlcBar> {
        self.bars.get(index)
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &OhlcBar> {
        self.bars.iter()
    }

    pub fn to_vec(&self) -> Vec<OhlcBar> {
        self.bars.iter().copied().collect()
    }

    pub fn visible_window(&self, range: LogicalRange) -> Option<VisibleBars<'_>> {
        let (start, end_inclusive) = visible_bounds(self.len(), range)?;
        Some(VisibleBars {
            store: self,
            start,
            end_exclusive: end_inclusive + 1,
        })
    }

    fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
}

impl BarSeries for BarStore {
    fn len(&self) -> usize {
        self.len()
    }

    fn bar(&self, index: usize) -> Option<OhlcBar> {
        self.get(index).copied()
    }

    fn generation(&self) -> u64 {
        BarStore::generation(self)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct VisibleBars<'a> {
    store: &'a BarStore,
    start: usize,
    end_exclusive: usize,
}

impl<'a> VisibleBars<'a> {
    pub fn start(&self) -> usize {
        self.start
    }

    pub fn end_exclusive(&self) -> usize {
        self.end_exclusive
    }

    pub fn len(&self) -> usize {
        self.end_exclusive.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = &'a OhlcBar> {
        (self.start..self.end_exclusive).filter_map(|index| self.store.get(index))
    }
}

fn visible_bounds(bar_count: usize, range: LogicalRange) -> Option<(usize, usize)> {
    if bar_count == 0 {
        return None;
    }
    let from = range.from.floor().max(0.0) as usize;
    let to = range
        .to
        .ceil()
        .max(0.0)
        .min(bar_count.saturating_sub(1) as f64) as usize;
    (from <= to).then_some((from, to))
}

fn sorted_unique_valid_bars(bars: impl IntoIterator<Item = OhlcBar>) -> Vec<OhlcBar> {
    let mut bars = bars
        .into_iter()
        .filter(|bar| bar.is_valid())
        .collect::<Vec<_>>();
    bars.sort_by_key(|bar| bar.time);

    let mut unique: Vec<OhlcBar> = Vec::with_capacity(bars.len());
    for bar in bars {
        if let Some(last) = unique.last_mut()
            && last.time == bar.time
        {
            *last = bar;
            continue;
        }
        unique.push(bar);
    }
    unique
}

#[cfg(test)]
mod tests {
    use super::{BarStore, TailUpdate};
    use crate::chart_core::{LogicalRange, OhlcBar};

    fn bar(time: i64, close: f64) -> OhlcBar {
        OhlcBar {
            time,
            open: close,
            high: close,
            low: close,
            close,
            volume: 1.0,
        }
    }

    #[test]
    fn replace_all_sorts_deduplicates_and_caps_history() {
        let mut store = BarStore::new(Some(3));

        store.replace_all([bar(3, 3.0), bar(1, 1.0), bar(2, 2.0), bar(2, 5.0)]);

        assert_eq!(store.len(), 3);
        assert_eq!(store.front().map(|bar| bar.time), Some(1));
        assert_eq!(store.get(1).map(|bar| bar.close), Some(5.0));
        assert_eq!(store.generation(), 1);
    }

    #[test]
    fn tail_updates_append_replace_and_ignore_stale_bars() {
        let mut store = BarStore::new(Some(3));
        store.replace_all([bar(1, 1.0), bar(2, 2.0)]);

        assert_eq!(store.apply_tail(bar(2, 20.0)), TailUpdate::ReplacedTail);
        assert_eq!(store.back().map(|bar| bar.close), Some(20.0));
        assert_eq!(store.apply_tail(bar(1, 10.0)), TailUpdate::IgnoredStale);
        assert_eq!(store.apply_tail(bar(3, 3.0)), TailUpdate::Appended);
        assert_eq!(store.apply_tail(bar(4, 4.0)), TailUpdate::Appended);

        assert_eq!(store.len(), 3);
        assert_eq!(store.front().map(|bar| bar.time), Some(2));
        assert_eq!(store.back().map(|bar| bar.time), Some(4));
    }

    #[test]
    fn prepend_history_adds_only_older_bars_and_deduplicates() {
        let mut store = BarStore::new(Some(5));
        store.replace_all([bar(3, 3.0), bar(4, 4.0)]);

        let added = store.prepend_history([bar(2, 2.0), bar(1, 1.0), bar(3, 30.0), bar(2, 20.0)]);

        assert_eq!(added, 2);
        assert_eq!(
            store.iter().map(|bar| bar.time).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert_eq!(store.get(1).map(|bar| bar.close), Some(20.0));
    }

    #[test]
    fn prepend_history_caps_from_newest_edge_when_full() {
        let mut store = BarStore::new(Some(3));
        store.replace_all([bar(3, 3.0), bar(4, 4.0), bar(5, 5.0)]);

        let added = store.prepend_history([bar(1, 1.0), bar(2, 2.0)]);

        assert_eq!(added, 2);
        assert_eq!(
            store.iter().map(|bar| bar.time).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn visible_window_is_bounded_by_logical_range() {
        let mut store = BarStore::unbounded();
        store.replace_all((0..10).map(|time| bar(time, time as f64)));

        let visible = store
            .visible_window(LogicalRange {
                from: 2.25,
                to: 5.1,
            })
            .expect("visible bars");
        let times = visible.iter().map(|bar| bar.time).collect::<Vec<_>>();

        assert_eq!(visible.start(), 2);
        assert_eq!(visible.end_exclusive(), 7);
        assert_eq!(times, vec![2, 3, 4, 5, 6]);
    }

    #[test]
    fn bar_series_generation_tracks_store_generation() {
        let mut store = BarStore::unbounded();

        assert_eq!(crate::chart_core::BarSeries::generation(&store), 0);
        store.replace_all([bar(1, 1.0)]);
        assert_eq!(
            crate::chart_core::BarSeries::generation(&store),
            store.generation()
        );
        store.apply_tail(bar(2, 2.0));
        assert_eq!(
            crate::chart_core::BarSeries::generation(&store),
            store.generation()
        );
    }
}
