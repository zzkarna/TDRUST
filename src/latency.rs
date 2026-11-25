use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use eframe::egui;

const DEFAULT_SAMPLE_LIMIT: usize = 600;
const IDLE_REPAINT_GAP_MS: f32 = 250.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatencyMetricSummary {
    pub last_ms: Option<f32>,
    pub p50_ms: Option<f32>,
    pub p95_ms: Option<f32>,
    pub p99_ms: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatencySnapshot {
    pub frame_count: u64,
    pub sample_count: usize,
    pub active_sample_count: usize,
    pub idle_wake_count: usize,
    pub update: LatencyMetricSummary,
    pub repaint_gap: LatencyMetricSummary,
    pub idle_wake_gap: LatencyMetricSummary,
    pub input_to_frame: LatencyMetricSummary,
    pub data_to_frame: LatencyMetricSummary,
    pub current_input_age_ms: Option<f32>,
    pub current_data_age_ms: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct LatencySample {
    update_ms: f32,
    repaint_gap_ms: Option<f32>,
    input_to_frame_ms: Option<f32>,
    data_to_frame_ms: Option<f32>,
    active: bool,
}

#[derive(Debug, Clone)]
pub struct LatencyProbe {
    samples: VecDeque<LatencySample>,
    sample_limit: usize,
    frame_started_at: Option<Instant>,
    previous_frame_started_at: Option<Instant>,
    last_input_event_at: Option<Instant>,
    pending_input_event_at: Option<Instant>,
    last_data_ingest_at: Option<Instant>,
    pending_data_ingest_at: Option<Instant>,
    last_data_generation: u64,
    last_log_at: Option<Instant>,
    frame_count: u64,
}

impl Default for LatencyProbe {
    fn default() -> Self {
        Self::with_sample_limit(DEFAULT_SAMPLE_LIMIT)
    }
}

impl LatencyProbe {
    fn with_sample_limit(sample_limit: usize) -> Self {
        Self {
            samples: VecDeque::new(),
            sample_limit: sample_limit.max(1),
            frame_started_at: None,
            previous_frame_started_at: None,
            last_input_event_at: None,
            pending_input_event_at: None,
            last_data_ingest_at: None,
            pending_data_ingest_at: None,
            last_data_generation: 0,
            last_log_at: None,
            frame_count: 0,
        }
    }

    pub fn begin_frame(&mut self, now: Instant, has_input_event: bool) {
        self.frame_started_at = Some(now);
        if has_input_event {
            self.last_input_event_at = Some(now);
            self.pending_input_event_at = Some(now);
        }
    }

    pub fn record_data_ingest(&mut self, now: Instant) {
        self.last_data_ingest_at = Some(now);
        self.pending_data_ingest_at = Some(now);
    }

    pub fn finish_frame(&mut self, now: Instant, data_generation: u64) {
        let Some(frame_started_at) = self.frame_started_at.take() else {
            return;
        };
        let data_to_frame_ms = if data_generation != self.last_data_generation {
            self.last_data_generation = data_generation;
            self.pending_data_ingest_at
                .take()
                .map(|ingested_at| duration_ms(now.saturating_duration_since(ingested_at)))
        } else {
            None
        };
        let repaint_gap_ms = self
            .previous_frame_started_at
            .map(|previous| duration_ms(frame_started_at.saturating_duration_since(previous)));
        let input_to_frame_ms = self
            .pending_input_event_at
            .take()
            .map(|input_at| duration_ms(now.saturating_duration_since(input_at)));
        let active = repaint_gap_ms.is_none_or(|gap| gap <= IDLE_REPAINT_GAP_MS)
            || input_to_frame_ms.is_some()
            || data_to_frame_ms.is_some();
        let sample = LatencySample {
            update_ms: duration_ms(now.saturating_duration_since(frame_started_at)),
            repaint_gap_ms,
            input_to_frame_ms,
            data_to_frame_ms,
            active,
        };
        self.previous_frame_started_at = Some(frame_started_at);
        self.frame_count = self.frame_count.saturating_add(1);
        self.samples.push_back(sample);
        while self.samples.len() > self.sample_limit {
            self.samples.pop_front();
        }
    }

    pub fn snapshot(&self, now: Instant) -> LatencySnapshot {
        LatencySnapshot {
            frame_count: self.frame_count,
            sample_count: self.samples.len(),
            active_sample_count: self.samples.iter().filter(|sample| sample.active).count(),
            idle_wake_count: self
                .samples
                .iter()
                .filter(|sample| {
                    sample
                        .repaint_gap_ms
                        .is_some_and(|gap| gap > IDLE_REPAINT_GAP_MS)
                })
                .count(),
            update: summarize_metric(&self.samples, |sample| {
                sample.active.then_some(sample.update_ms)
            }),
            repaint_gap: summarize_metric(&self.samples, |sample| {
                sample
                    .repaint_gap_ms
                    .filter(|gap| *gap <= IDLE_REPAINT_GAP_MS)
            }),
            idle_wake_gap: summarize_metric(&self.samples, |sample| {
                sample
                    .repaint_gap_ms
                    .filter(|gap| *gap > IDLE_REPAINT_GAP_MS)
            }),
            input_to_frame: summarize_metric(&self.samples, |sample| sample.input_to_frame_ms),
            data_to_frame: summarize_metric(&self.samples, |sample| sample.data_to_frame_ms),
            current_input_age_ms: self
                .last_input_event_at
                .map(|input_at| duration_ms(now.saturating_duration_since(input_at))),
            current_data_age_ms: self
                .last_data_ingest_at
                .map(|data_at| duration_ms(now.saturating_duration_since(data_at))),
        }
    }

    pub fn take_log_snapshot(
        &mut self,
        now: Instant,
        interval: Duration,
    ) -> Option<LatencySnapshot> {
        if self.samples.is_empty() {
            return None;
        }
        if self
            .last_log_at
            .is_some_and(|last_log_at| now.saturating_duration_since(last_log_at) < interval)
        {
            return None;
        }
        self.last_log_at = Some(now);
        Some(self.snapshot(now))
    }
}

pub fn input_event_is_latency_relevant(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::Copy
            | egui::Event::Cut
            | egui::Event::Paste(_)
            | egui::Event::Text(_)
            | egui::Event::Key { .. }
            | egui::Event::PointerMoved(_)
            | egui::Event::MouseMoved(_)
            | egui::Event::PointerButton { .. }
            | egui::Event::PointerGone
            | egui::Event::Zoom(_)
            | egui::Event::Rotate(_)
            | egui::Event::Touch { .. }
            | egui::Event::MouseWheel { .. }
            | egui::Event::WindowFocused(_)
    )
}

fn summarize_metric(
    samples: &VecDeque<LatencySample>,
    metric: impl Fn(&LatencySample) -> Option<f32>,
) -> LatencyMetricSummary {
    let last_ms = samples.iter().rev().find_map(&metric);
    let mut values = samples.iter().filter_map(metric).collect::<Vec<_>>();
    values.sort_by(f32::total_cmp);
    LatencyMetricSummary {
        last_ms,
        p50_ms: percentile(&values, 50.0),
        p95_ms: percentile(&values, 95.0),
        p99_ms: percentile(&values, 99.0),
    }
}

fn percentile(values: &[f32], percentile: f32) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    let rank = ((percentile / 100.0) * values.len() as f32).ceil() as usize;
    let index = rank.saturating_sub(1).min(values.len() - 1);
    values.get(index).copied()
}

fn duration_ms(duration: Duration) -> f32 {
    duration.as_secs_f32() * 1_000.0
}

#[cfg(test)]
mod tests {
    use super::{LatencyProbe, input_event_is_latency_relevant};
    use eframe::egui;
    use std::time::{Duration, Instant};

    #[test]
    fn frame_samples_report_update_and_input_latency() {
        let mut probe = LatencyProbe::with_sample_limit(8);
        let start = Instant::now();
        probe.begin_frame(start, true);
        probe.finish_frame(start + Duration::from_millis(8), 0);

        let snapshot = probe.snapshot(start + Duration::from_millis(9));
        assert_eq!(snapshot.frame_count, 1);
        assert_eq!(snapshot.sample_count, 1);
        assert_eq!(snapshot.active_sample_count, 1);
        assert_eq!(snapshot.idle_wake_count, 0);
        assert_eq!(snapshot.update.last_ms, Some(8.0));
        assert_eq!(snapshot.input_to_frame.last_ms, Some(8.0));
        assert_eq!(snapshot.current_input_age_ms, Some(9.0));
    }

    #[test]
    fn data_latency_is_recorded_when_generation_changes() {
        let mut probe = LatencyProbe::with_sample_limit(8);
        let start = Instant::now();
        probe.begin_frame(start, false);
        probe.finish_frame(start + Duration::from_millis(1), 0);

        let ingest = start + Duration::from_millis(10);
        probe.record_data_ingest(ingest);
        probe.begin_frame(start + Duration::from_millis(12), false);
        probe.finish_frame(start + Duration::from_millis(20), 1);

        let snapshot = probe.snapshot(start + Duration::from_millis(21));
        assert_eq!(snapshot.data_to_frame.last_ms, Some(10.0));
        assert_eq!(snapshot.current_data_age_ms, Some(11.0));
    }

    #[test]
    fn sample_limit_keeps_recent_values_and_percentiles() {
        let mut probe = LatencyProbe::with_sample_limit(3);
        let start = Instant::now();
        for index in 0..5 {
            let frame_start = start + Duration::from_millis(index * 10);
            probe.begin_frame(frame_start, false);
            probe.finish_frame(frame_start + Duration::from_millis(index + 1), 0);
        }

        let snapshot = probe.snapshot(start + Duration::from_millis(100));
        assert_eq!(snapshot.sample_count, 3);
        assert_eq!(snapshot.update.last_ms, Some(5.0));
        assert_eq!(snapshot.update.p50_ms, Some(4.0));
        assert_eq!(snapshot.update.p95_ms, Some(5.0));
        assert_eq!(snapshot.update.p99_ms, Some(5.0));
    }

    #[test]
    fn idle_repaint_gaps_are_reported_separately() {
        let mut probe = LatencyProbe::with_sample_limit(8);
        let start = Instant::now();
        probe.begin_frame(start, false);
        probe.finish_frame(start + Duration::from_millis(4), 0);

        let idle_frame_start = start + Duration::from_secs(3);
        probe.begin_frame(idle_frame_start, false);
        probe.finish_frame(idle_frame_start + Duration::from_millis(5), 0);

        let active_frame_start = idle_frame_start + Duration::from_millis(16);
        probe.begin_frame(active_frame_start, false);
        probe.finish_frame(active_frame_start + Duration::from_millis(6), 0);

        let snapshot = probe.snapshot(active_frame_start + Duration::from_millis(7));
        assert_eq!(snapshot.sample_count, 3);
        assert_eq!(snapshot.active_sample_count, 2);
        assert_eq!(snapshot.idle_wake_count, 1);
        assert_eq!(snapshot.repaint_gap.p99_ms, Some(16.0));
        assert_eq!(snapshot.idle_wake_gap.last_ms, Some(3_000.0));
    }

    #[test]
    fn log_snapshot_is_throttled() {
        let mut probe = LatencyProbe::with_sample_limit(8);
        let start = Instant::now();
        assert_eq!(probe.take_log_snapshot(start, Duration::from_secs(2)), None);

        probe.begin_frame(start, false);
        probe.finish_frame(start + Duration::from_millis(4), 0);
        assert!(
            probe
                .take_log_snapshot(start + Duration::from_millis(5), Duration::from_secs(2))
                .is_some()
        );
        assert_eq!(
            probe.take_log_snapshot(start + Duration::from_secs(1), Duration::from_secs(2)),
            None
        );
        assert!(
            probe
                .take_log_snapshot(start + Duration::from_secs(3), Duration::from_secs(2))
                .is_some()
        );
    }

    #[test]
    fn latency_input_filter_ignores_non_interaction_events() {
        assert!(input_event_is_latency_relevant(&egui::Event::PointerMoved(
            egui::pos2(1.0, 1.0),
        )));
        assert!(input_event_is_latency_relevant(&egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }));
        assert!(!input_event_is_latency_relevant(&egui::Event::Screenshot {
            viewport_id: egui::ViewportId::ROOT,
            user_data: egui::UserData::default(),
            image: std::sync::Arc::new(egui::ColorImage::example()),
        }));
    }
}
