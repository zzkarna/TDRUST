use std::collections::VecDeque;

use crate::chart_core::{OhlcBar, PaneId, PriceScaleId, SeriesKind};

use super::{StudyId, StudyOutputDescriptor, StudyOutputId, StudyProgram, StudyValue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StudyPriceSource {
    Open,
    High,
    Low,
    Close,
    Hl2,
    Hlc3,
    Ohlc4,
}

impl StudyPriceSource {
    pub const ALL: [Self; 7] = [
        Self::Open,
        Self::High,
        Self::Low,
        Self::Close,
        Self::Hl2,
        Self::Hlc3,
        Self::Ohlc4,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::High => "High",
            Self::Low => "Low",
            Self::Close => "Close",
            Self::Hl2 => "HL2",
            Self::Hlc3 => "HLC3",
            Self::Ohlc4 => "OHLC4",
        }
    }

    pub fn value(self, bar: OhlcBar) -> f64 {
        match self {
            Self::Open => bar.open,
            Self::High => bar.high,
            Self::Low => bar.low,
            Self::Close => bar.close,
            Self::Hl2 => (bar.high + bar.low) * 0.5,
            Self::Hlc3 => (bar.high + bar.low + bar.close) / 3.0,
            Self::Ohlc4 => (bar.open + bar.high + bar.low + bar.close) * 0.25,
        }
    }
}

impl Default for StudyPriceSource {
    fn default() -> Self {
        Self::Close
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StudySmoothingKind {
    None,
    Sma,
    Ema,
}

impl StudySmoothingKind {
    pub const ALL: [Self; 3] = [Self::None, Self::Sma, Self::Ema];

    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Sma => "SMA",
            Self::Ema => "EMA",
        }
    }

    pub const fn is_enabled(self) -> bool {
        !matches!(self, Self::None)
    }
}

impl Default for StudySmoothingKind {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Clone)]
pub struct SmaStudy {
    period: usize,
    source: StudyPriceSource,
    window: VecDeque<f64>,
    sum: f64,
}

impl SmaStudy {
    pub fn new(period: usize) -> Self {
        Self::with_source(period, StudyPriceSource::Close)
    }

    pub fn with_source(period: usize, source: StudyPriceSource) -> Self {
        Self {
            period: normalize_period(period),
            source,
            window: VecDeque::new(),
            sum: 0.0,
        }
    }
}

impl StudyProgram for SmaStudy {
    fn reset(&mut self) {
        self.window.clear();
        self.sum = 0.0;
    }

    fn output_descriptors(&self, study: StudyId) -> Vec<StudyOutputDescriptor> {
        vec![
            price_overlay_line(study, 0, format!("SMA {}", self.period))
                .with_warmup_bars(self.period.saturating_sub(1)),
        ]
    }

    fn next(&mut self, _index: usize, bar: OhlcBar) -> Vec<StudyValue> {
        let value = self.source.value(bar);
        self.window.push_back(value);
        self.sum += value;
        if self.window.len() > self.period
            && let Some(oldest) = self.window.pop_front()
        {
            self.sum -= oldest;
        }

        vec![if self.window.len() == self.period {
            StudyValue::number(self.sum / self.period as f64)
        } else {
            StudyValue::Null
        }]
    }
}

#[derive(Debug, Clone)]
pub struct EmaStudy {
    source: StudyPriceSource,
    kernel: EmaKernel,
}

impl EmaStudy {
    pub fn new(period: usize) -> Self {
        Self::with_source(period, StudyPriceSource::Close)
    }

    pub fn with_source(period: usize, source: StudyPriceSource) -> Self {
        Self {
            source,
            kernel: EmaKernel::new(period),
        }
    }

    pub fn period(&self) -> usize {
        self.kernel.period()
    }
}

impl StudyProgram for EmaStudy {
    fn reset(&mut self) {
        self.kernel.reset();
    }

    fn output_descriptors(&self, study: StudyId) -> Vec<StudyOutputDescriptor> {
        vec![
            price_overlay_line(study, 0, format!("EMA {}", self.period()))
                .with_warmup_bars(self.period().saturating_sub(1)),
        ]
    }

    fn next(&mut self, _index: usize, bar: OhlcBar) -> Vec<StudyValue> {
        vec![StudyValue::from(self.kernel.next(self.source.value(bar)))]
    }
}

#[derive(Debug, Clone)]
pub struct VwapStudy {
    price_volume_sum: f64,
    volume_sum: f64,
}

impl VwapStudy {
    pub const fn new() -> Self {
        Self {
            price_volume_sum: 0.0,
            volume_sum: 0.0,
        }
    }
}

impl Default for VwapStudy {
    fn default() -> Self {
        Self::new()
    }
}

impl StudyProgram for VwapStudy {
    fn reset(&mut self) {
        self.price_volume_sum = 0.0;
        self.volume_sum = 0.0;
    }

    fn output_descriptors(&self, study: StudyId) -> Vec<StudyOutputDescriptor> {
        vec![price_overlay_line(study, 0, "VWAP")]
    }

    fn next(&mut self, _index: usize, bar: OhlcBar) -> Vec<StudyValue> {
        let typical_price = (bar.high + bar.low + bar.close) / 3.0;
        self.price_volume_sum += typical_price * bar.volume;
        self.volume_sum += bar.volume;

        vec![if self.volume_sum.abs() > f64::EPSILON {
            StudyValue::number(self.price_volume_sum / self.volume_sum)
        } else {
            StudyValue::Null
        }]
    }
}

#[derive(Debug, Clone)]
pub struct RsiStudy {
    period: usize,
    source: StudyPriceSource,
    smoothing: SmoothingKernel,
    previous_value: Option<f64>,
    changes: usize,
    gain_sum: f64,
    loss_sum: f64,
    average_gain: Option<f64>,
    average_loss: Option<f64>,
}

impl RsiStudy {
    pub fn new(period: usize) -> Self {
        Self::with_source_and_smoothing(
            period,
            StudyPriceSource::Close,
            StudySmoothingKind::None,
            1,
        )
    }

    pub fn with_source(period: usize, source: StudyPriceSource) -> Self {
        Self::with_source_and_smoothing(period, source, StudySmoothingKind::None, 1)
    }

    pub fn with_source_and_smoothing(
        period: usize,
        source: StudyPriceSource,
        smoothing_kind: StudySmoothingKind,
        smoothing_length: usize,
    ) -> Self {
        Self {
            period: normalize_period(period),
            source,
            smoothing: SmoothingKernel::new(smoothing_kind, smoothing_length),
            previous_value: None,
            changes: 0,
            gain_sum: 0.0,
            loss_sum: 0.0,
            average_gain: None,
            average_loss: None,
        }
    }
}

impl StudyProgram for RsiStudy {
    fn reset(&mut self) {
        self.previous_value = None;
        self.smoothing.reset();
        self.changes = 0;
        self.gain_sum = 0.0;
        self.loss_sum = 0.0;
        self.average_gain = None;
        self.average_loss = None;
    }

    fn output_descriptors(&self, study: StudyId) -> Vec<StudyOutputDescriptor> {
        let mut descriptors = vec![
            study_pane_line(study, 0, format!("RSI {}", self.period)).with_warmup_bars(self.period),
        ];
        if self.smoothing.kind().is_enabled() {
            descriptors.push(
                study_pane_line(study, 1, "RSI-based MA").with_warmup_bars(
                    self.period
                        .saturating_add(self.smoothing.period().saturating_sub(1)),
                ),
            );
        }
        descriptors
    }

    fn next(&mut self, _index: usize, bar: OhlcBar) -> Vec<StudyValue> {
        let value = self.source.value(bar);
        let Some(previous) = self.previous_value.replace(value) else {
            return self.rsi_outputs(None);
        };
        let change = value - previous;
        let gain = change.max(0.0);
        let loss = (-change).max(0.0);

        let value = match (self.average_gain, self.average_loss) {
            (Some(average_gain), Some(average_loss)) => {
                let period = self.period as f64;
                let average_gain = (average_gain * (period - 1.0) + gain) / period;
                let average_loss = (average_loss * (period - 1.0) + loss) / period;
                self.average_gain = Some(average_gain);
                self.average_loss = Some(average_loss);
                Some(rsi_from_averages(average_gain, average_loss))
            }
            _ => {
                self.changes += 1;
                self.gain_sum += gain;
                self.loss_sum += loss;
                if self.changes == self.period {
                    let average_gain = self.gain_sum / self.period as f64;
                    let average_loss = self.loss_sum / self.period as f64;
                    self.average_gain = Some(average_gain);
                    self.average_loss = Some(average_loss);
                    Some(rsi_from_averages(average_gain, average_loss))
                } else {
                    None
                }
            }
        };

        self.rsi_outputs(value)
    }
}

impl RsiStudy {
    fn rsi_outputs(&mut self, rsi: Option<f64>) -> Vec<StudyValue> {
        let mut outputs = vec![StudyValue::from(rsi)];
        if self.smoothing.kind().is_enabled() {
            let smoothed = rsi.and_then(|value| self.smoothing.next(value));
            outputs.push(StudyValue::from(smoothed));
        }
        outputs
    }
}

#[derive(Debug, Clone)]
pub struct AtrStudy {
    period: usize,
    previous_close: Option<f64>,
    true_ranges: usize,
    true_range_sum: f64,
    average_true_range: Option<f64>,
}

impl AtrStudy {
    pub fn new(period: usize) -> Self {
        Self {
            period: normalize_period(period),
            previous_close: None,
            true_ranges: 0,
            true_range_sum: 0.0,
            average_true_range: None,
        }
    }
}

impl StudyProgram for AtrStudy {
    fn reset(&mut self) {
        self.previous_close = None;
        self.true_ranges = 0;
        self.true_range_sum = 0.0;
        self.average_true_range = None;
    }

    fn output_descriptors(&self, study: StudyId) -> Vec<StudyOutputDescriptor> {
        vec![
            study_pane_line(study, 0, format!("ATR {}", self.period))
                .with_warmup_bars(self.period.saturating_sub(1)),
        ]
    }

    fn next(&mut self, _index: usize, bar: OhlcBar) -> Vec<StudyValue> {
        let true_range = true_range(bar, self.previous_close);
        self.previous_close = Some(bar.close);

        let value = match self.average_true_range {
            Some(average_true_range) => {
                let period = self.period as f64;
                let average_true_range =
                    (average_true_range * (period - 1.0) + true_range) / period;
                self.average_true_range = Some(average_true_range);
                Some(average_true_range)
            }
            None => {
                self.true_ranges += 1;
                self.true_range_sum += true_range;
                if self.true_ranges == self.period {
                    let average_true_range = self.true_range_sum / self.period as f64;
                    self.average_true_range = Some(average_true_range);
                    Some(average_true_range)
                } else {
                    None
                }
            }
        };

        vec![StudyValue::from(value)]
    }
}

#[derive(Debug, Clone)]
pub struct MacdStudy {
    source: StudyPriceSource,
    fast: EmaKernel,
    slow: EmaKernel,
    signal: EmaKernel,
}

impl MacdStudy {
    pub fn new(fast_period: usize, slow_period: usize, signal_period: usize) -> Self {
        Self::with_source(
            fast_period,
            slow_period,
            signal_period,
            StudyPriceSource::Close,
        )
    }

    pub fn with_source(
        fast_period: usize,
        slow_period: usize,
        signal_period: usize,
        source: StudyPriceSource,
    ) -> Self {
        let fast_period = normalize_period(fast_period);
        let slow_period = normalize_period(slow_period).max(fast_period);
        Self {
            source,
            fast: EmaKernel::new(fast_period),
            slow: EmaKernel::new(slow_period),
            signal: EmaKernel::new(signal_period),
        }
    }

    pub fn standard() -> Self {
        Self::new(12, 26, 9)
    }

    pub fn fast_period(&self) -> usize {
        self.fast.period()
    }

    pub fn slow_period(&self) -> usize {
        self.slow.period()
    }

    pub fn signal_period(&self) -> usize {
        self.signal.period()
    }
}

impl StudyProgram for MacdStudy {
    fn reset(&mut self) {
        self.fast.reset();
        self.slow.reset();
        self.signal.reset();
    }

    fn output_descriptors(&self, study: StudyId) -> Vec<StudyOutputDescriptor> {
        let macd_warmup = self.slow_period().saturating_sub(1);
        let signal_warmup = self
            .slow_period()
            .saturating_add(self.signal_period())
            .saturating_sub(2);
        vec![
            study_pane_line(study, 0, "MACD").with_warmup_bars(macd_warmup),
            study_pane_line(study, 1, "Signal").with_warmup_bars(signal_warmup),
            StudyOutputDescriptor::new(
                StudyOutputId::new(study, 2),
                "Histogram",
                SeriesKind::Histogram,
                PaneId::Study(study.0),
                PriceScaleId::Overlay,
            )
            .with_warmup_bars(signal_warmup),
        ]
    }

    fn next(&mut self, _index: usize, bar: OhlcBar) -> Vec<StudyValue> {
        let value = self.source.value(bar);
        let fast = self.fast.next(value);
        let slow = self.slow.next(value);
        let macd = fast.zip(slow).map(|(fast, slow)| fast - slow);
        let signal = macd.and_then(|value| self.signal.next(value));
        let histogram = macd.zip(signal).map(|(macd, signal)| macd - signal);

        vec![
            StudyValue::from(macd),
            StudyValue::from(signal),
            StudyValue::from(histogram),
        ]
    }
}

#[derive(Debug, Clone)]
struct EmaKernel {
    period: usize,
    alpha: f64,
    seed_count: usize,
    seed_sum: f64,
    value: Option<f64>,
}

impl EmaKernel {
    fn new(period: usize) -> Self {
        let period = normalize_period(period);
        Self {
            period,
            alpha: 2.0 / (period as f64 + 1.0),
            seed_count: 0,
            seed_sum: 0.0,
            value: None,
        }
    }

    fn period(&self) -> usize {
        self.period
    }

    fn reset(&mut self) {
        self.seed_count = 0;
        self.seed_sum = 0.0;
        self.value = None;
    }

    fn next(&mut self, value: f64) -> Option<f64> {
        if let Some(previous) = self.value {
            let value = previous + self.alpha * (value - previous);
            self.value = Some(value);
            return Some(value);
        }

        self.seed_count += 1;
        self.seed_sum += value;
        if self.seed_count == self.period {
            let value = self.seed_sum / self.period as f64;
            self.value = Some(value);
            Some(value)
        } else {
            None
        }
    }
}

fn normalize_period(period: usize) -> usize {
    period.max(1)
}

fn price_overlay_line(
    study: StudyId,
    output: u32,
    name: impl Into<String>,
) -> StudyOutputDescriptor {
    StudyOutputDescriptor::line(
        StudyOutputId::new(study, output),
        name,
        PaneId::Main,
        PriceScaleId::Right,
    )
}

fn study_pane_line(study: StudyId, output: u32, name: impl Into<String>) -> StudyOutputDescriptor {
    StudyOutputDescriptor::line(
        StudyOutputId::new(study, output),
        name,
        PaneId::Study(study.0),
        PriceScaleId::Overlay,
    )
}

#[derive(Debug, Clone)]
enum SmoothingKernel {
    None,
    Sma {
        period: usize,
        window: VecDeque<f64>,
        sum: f64,
    },
    Ema(EmaKernel),
}

impl SmoothingKernel {
    fn new(kind: StudySmoothingKind, period: usize) -> Self {
        let period = normalize_period(period);
        match kind {
            StudySmoothingKind::None => Self::None,
            StudySmoothingKind::Sma => Self::Sma {
                period,
                window: VecDeque::new(),
                sum: 0.0,
            },
            StudySmoothingKind::Ema => Self::Ema(EmaKernel::new(period)),
        }
    }

    const fn kind(&self) -> StudySmoothingKind {
        match self {
            Self::None => StudySmoothingKind::None,
            Self::Sma { .. } => StudySmoothingKind::Sma,
            Self::Ema(_) => StudySmoothingKind::Ema,
        }
    }

    fn period(&self) -> usize {
        match self {
            Self::None => 1,
            Self::Sma { period, .. } => *period,
            Self::Ema(kernel) => kernel.period(),
        }
    }

    fn reset(&mut self) {
        match self {
            Self::None => {}
            Self::Sma { window, sum, .. } => {
                window.clear();
                *sum = 0.0;
            }
            Self::Ema(kernel) => kernel.reset(),
        }
    }

    fn next(&mut self, value: f64) -> Option<f64> {
        match self {
            Self::None => Some(value),
            Self::Sma {
                period,
                window,
                sum,
            } => {
                window.push_back(value);
                *sum += value;
                if window.len() > *period
                    && let Some(oldest) = window.pop_front()
                {
                    *sum -= oldest;
                }
                (window.len() == *period).then_some(*sum / *period as f64)
            }
            Self::Ema(kernel) => kernel.next(value),
        }
    }
}

fn rsi_from_averages(average_gain: f64, average_loss: f64) -> f64 {
    if average_gain.abs() <= f64::EPSILON && average_loss.abs() <= f64::EPSILON {
        50.0
    } else if average_loss.abs() <= f64::EPSILON {
        100.0
    } else {
        let relative_strength = average_gain / average_loss;
        100.0 - 100.0 / (1.0 + relative_strength)
    }
}

fn true_range(bar: OhlcBar, previous_close: Option<f64>) -> f64 {
    let current_range = bar.high - bar.low;
    previous_close
        .map(|close| {
            current_range
                .max((bar.high - close).abs())
                .max((bar.low - close).abs())
        })
        .unwrap_or(current_range)
}

#[cfg(test)]
mod tests {
    use super::{
        AtrStudy, EmaStudy, MacdStudy, RsiStudy, SmaStudy, StudyPriceSource, StudySmoothingKind,
        VwapStudy,
    };
    use crate::{
        chart_core::{BarStore, OhlcBar, TailUpdate},
        studies::{StudyId, StudyOutputId, StudyProgram, StudyRuntime, StudyValue},
    };

    const STUDY_ID: StudyId = StudyId::new(1);

    fn close_bar(index: usize, close: f64) -> OhlcBar {
        OhlcBar {
            time: index as i64,
            open: close,
            high: close,
            low: close,
            close,
            volume: 1.0,
        }
    }

    fn bar(index: usize, high: f64, low: f64, close: f64, volume: f64) -> OhlcBar {
        OhlcBar {
            time: index as i64,
            open: close,
            high,
            low,
            close,
            volume,
        }
    }

    fn run_output<P: StudyProgram + 'static>(
        program: P,
        bars: Vec<OhlcBar>,
        output: u32,
    ) -> Vec<StudyValue> {
        let mut store = BarStore::unbounded();
        store.replace_all(bars);
        let mut runtime = StudyRuntime::new();
        runtime.add_study_with_id(STUDY_ID, "test", program);
        runtime.replay(&store);
        runtime
            .output_values(StudyOutputId::new(STUDY_ID, output).series_id())
            .expect("study output")
            .to_vec()
    }

    fn assert_values_approx(actual: &[StudyValue], expected: &[Option<f64>]) {
        assert_eq!(actual.len(), expected.len());
        for (index, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
            match (actual, expected) {
                (StudyValue::Null, None) => {}
                (StudyValue::Number(actual), Some(expected)) => assert!(
                    (actual - expected).abs() < 1e-6,
                    "index {index}: expected {expected}, got {actual}"
                ),
                _ => panic!("index {index}: expected {expected:?}, got {actual:?}"),
            }
        }
    }

    fn sample_bars() -> Vec<OhlcBar> {
        [
            (10.5, 9.5, 10.0, 100.0),
            (11.2, 9.9, 10.8, 120.0),
            (11.8, 10.4, 11.2, 130.0),
            (11.0, 9.7, 10.1, 90.0),
            (12.5, 10.8, 12.1, 150.0),
            (13.0, 11.4, 12.8, 170.0),
            (12.4, 10.9, 11.3, 140.0),
            (13.6, 11.8, 13.2, 180.0),
            (14.2, 12.6, 13.7, 200.0),
            (13.9, 12.1, 12.6, 160.0),
            (15.0, 13.2, 14.6, 210.0),
            (15.8, 14.0, 15.2, 230.0),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (high, low, close, volume))| bar(index, high, low, close, volume))
        .collect()
    }

    fn assert_incremental_matches_replay<P, F>(factory: F, output_count: u32)
    where
        P: StudyProgram + 'static,
        F: Fn() -> P,
    {
        let bars = sample_bars();
        let replayed = runtime_outputs(factory(), &bars, output_count);
        let mut store = BarStore::unbounded();
        store.replace_all(bars.iter().copied().take(4));
        let mut runtime = StudyRuntime::new();
        runtime.add_study_with_id(STUDY_ID, "incremental", factory());
        runtime.replay(&store);

        for bar in bars.iter().copied().skip(4) {
            let update = store.apply_tail(bar);
            assert_eq!(update, TailUpdate::Appended);
            runtime.apply_tail(&store, update);
        }

        let incremental = collect_outputs(&runtime, output_count);
        assert_eq!(incremental.len(), replayed.len());
        for (actual, expected) in incremental.iter().zip(replayed.iter()) {
            assert_values_match(actual, expected);
        }
    }

    fn runtime_outputs<P: StudyProgram + 'static>(
        program: P,
        bars: &[OhlcBar],
        output_count: u32,
    ) -> Vec<Vec<StudyValue>> {
        let mut store = BarStore::unbounded();
        store.replace_all(bars.iter().copied());
        let mut runtime = StudyRuntime::new();
        runtime.add_study_with_id(STUDY_ID, "replay", program);
        runtime.replay(&store);
        collect_outputs(&runtime, output_count)
    }

    fn collect_outputs(runtime: &StudyRuntime, output_count: u32) -> Vec<Vec<StudyValue>> {
        (0..output_count)
            .map(|output| {
                runtime
                    .output_values(StudyOutputId::new(STUDY_ID, output).series_id())
                    .expect("study output")
                    .to_vec()
            })
            .collect()
    }

    fn assert_values_match(actual: &[StudyValue], expected: &[StudyValue]) {
        assert_eq!(actual.len(), expected.len());
        for (index, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
            match (actual, expected) {
                (StudyValue::Null, StudyValue::Null) => {}
                (StudyValue::Number(actual), StudyValue::Number(expected)) => assert!(
                    (actual - expected).abs() < 1e-12,
                    "index {index}: expected {expected}, got {actual}"
                ),
                _ => panic!("index {index}: expected {expected:?}, got {actual:?}"),
            }
        }
    }

    #[test]
    fn sma_matches_golden_values() {
        let bars = [1.0, 2.0, 4.0, 8.0, 16.0]
            .into_iter()
            .enumerate()
            .map(|(index, close)| close_bar(index, close))
            .collect();

        assert_values_approx(
            &run_output(SmaStudy::new(3), bars, 0),
            &[
                None,
                None,
                Some(7.0 / 3.0),
                Some(14.0 / 3.0),
                Some(28.0 / 3.0),
            ],
        );
    }

    #[test]
    fn ema_matches_golden_values() {
        let bars = [1.0, 2.0, 4.0, 8.0, 16.0]
            .into_iter()
            .enumerate()
            .map(|(index, close)| close_bar(index, close))
            .collect();

        assert_values_approx(
            &run_output(EmaStudy::new(3), bars, 0),
            &[
                None,
                None,
                Some(7.0 / 3.0),
                Some(31.0 / 6.0),
                Some(127.0 / 12.0),
            ],
        );
    }

    #[test]
    fn vwap_matches_golden_values() {
        let bars = vec![
            bar(0, 2.0, 0.0, 1.0, 10.0),
            bar(1, 4.0, 2.0, 3.0, 20.0),
            bar(2, 8.0, 4.0, 6.0, 30.0),
        ];

        assert_values_approx(
            &run_output(VwapStudy::new(), bars, 0),
            &[Some(1.0), Some(7.0 / 3.0), Some(25.0 / 6.0)],
        );
    }

    #[test]
    fn rsi_matches_wilder_golden_values() {
        let bars = [1.0, 2.0, 3.0, 2.0, 2.5, 3.5]
            .into_iter()
            .enumerate()
            .map(|(index, close)| close_bar(index, close))
            .collect();

        assert_values_approx(
            &run_output(RsiStudy::new(3), bars, 0),
            &[
                None,
                None,
                None,
                Some(66.666_666_666_7),
                Some(73.333_333_333_3),
                Some(83.333_333_333_3),
            ],
        );
    }

    #[test]
    fn moving_average_source_can_use_ohlc_fields() {
        let bars = vec![bar(0, 20.0, 4.0, 10.0, 1.0), bar(1, 30.0, 6.0, 14.0, 2.0)];

        assert_values_approx(
            &run_output(SmaStudy::with_source(2, StudyPriceSource::High), bars, 0),
            &[None, Some(25.0)],
        );
    }

    #[test]
    fn rsi_smoothing_emits_secondary_output() {
        let bars = [1.0, 2.0, 3.0, 2.0, 2.5, 3.5]
            .into_iter()
            .enumerate()
            .map(|(index, close)| close_bar(index, close))
            .collect::<Vec<_>>();

        assert_values_approx(
            &run_output(
                RsiStudy::with_source_and_smoothing(
                    3,
                    StudyPriceSource::Close,
                    StudySmoothingKind::Sma,
                    2,
                ),
                bars,
                1,
            ),
            &[None, None, None, None, Some(70.0), Some(78.333_333_333_3)],
        );
    }

    #[test]
    fn atr_matches_wilder_golden_values() {
        let bars = vec![
            bar(0, 10.0, 9.0, 9.5, 1.0),
            bar(1, 11.0, 10.0, 10.5, 1.0),
            bar(2, 10.8, 9.8, 10.0, 1.0),
            bar(3, 12.0, 11.0, 11.5, 1.0),
            bar(4, 11.8, 10.8, 11.0, 1.0),
        ];

        assert_values_approx(
            &run_output(AtrStudy::new(3), bars, 0),
            &[
                None,
                None,
                Some(7.0 / 6.0),
                Some(13.0 / 9.0),
                Some(35.0 / 27.0),
            ],
        );
    }

    #[test]
    fn macd_matches_golden_values() {
        let bars = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0]
            .into_iter()
            .enumerate()
            .map(|(index, close)| close_bar(index, close))
            .collect::<Vec<_>>();

        assert_values_approx(
            &run_output(MacdStudy::new(2, 3, 2), bars.clone(), 0),
            &[
                None,
                None,
                Some(5.0 / 6.0),
                Some(11.0 / 9.0),
                Some(239.0 / 108.0),
                Some(2791.0 / 648.0),
            ],
        );
        assert_values_approx(
            &run_output(MacdStudy::new(2, 3, 2), bars.clone(), 1),
            &[
                None,
                None,
                None,
                Some(37.0 / 36.0),
                Some(589.0 / 324.0),
                Some(845.0 / 243.0),
            ],
        );
        assert_values_approx(
            &run_output(MacdStudy::new(2, 3, 2), bars, 2),
            &[
                None,
                None,
                None,
                Some(7.0 / 36.0),
                Some(128.0 / 324.0),
                Some(1613.0 / 1944.0),
            ],
        );
    }

    #[test]
    fn builtins_incremental_append_matches_full_replay() {
        assert_incremental_matches_replay(|| SmaStudy::new(4), 1);
        assert_incremental_matches_replay(|| EmaStudy::new(4), 1);
        assert_incremental_matches_replay(VwapStudy::new, 1);
        assert_incremental_matches_replay(|| RsiStudy::new(4), 1);
        assert_incremental_matches_replay(|| AtrStudy::new(4), 1);
        assert_incremental_matches_replay(|| MacdStudy::new(3, 5, 3), 3);
    }
}
