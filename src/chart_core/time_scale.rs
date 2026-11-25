use super::model::{BarSeries, LogicalRange, OhlcBar};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeAxisUnit {
    Second,
    Minute,
    Hour,
    Day,
    Month,
    Year,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeAxisTick {
    pub logical: f64,
    pub index: usize,
    /// Normalized Unix timestamp in seconds.
    pub timestamp: i64,
    pub unit: TimeAxisUnit,
}

const MIN_TICK_SPACING_PX: f64 = 70.0;
const TIME_STEPS: &[(i64, TimeAxisUnit)] = &[
    (1, TimeAxisUnit::Second),
    (5, TimeAxisUnit::Second),
    (15, TimeAxisUnit::Second),
    (30, TimeAxisUnit::Second),
    (60, TimeAxisUnit::Minute),
    (5 * 60, TimeAxisUnit::Minute),
    (15 * 60, TimeAxisUnit::Minute),
    (30 * 60, TimeAxisUnit::Minute),
    (60 * 60, TimeAxisUnit::Hour),
    (2 * 60 * 60, TimeAxisUnit::Hour),
    (4 * 60 * 60, TimeAxisUnit::Hour),
    (12 * 60 * 60, TimeAxisUnit::Hour),
    (24 * 60 * 60, TimeAxisUnit::Day),
    (7 * 24 * 60 * 60, TimeAxisUnit::Day),
    (30 * 24 * 60 * 60, TimeAxisUnit::Month),
    (90 * 24 * 60 * 60, TimeAxisUnit::Month),
    (365 * 24 * 60 * 60, TimeAxisUnit::Year),
];

pub fn visible_time_axis_ticks<B: BarSeries + ?Sized>(
    bars: &B,
    visible: LogicalRange,
    plot_width: f32,
) -> Vec<TimeAxisTick> {
    let Some((from, to)) = visible_bar_bounds(bars.len(), visible) else {
        return Vec::new();
    };

    let times = (from..=to)
        .filter_map(|index| {
            bars.bar(index)
                .map(|bar| (index, normalize_timestamp_seconds(bar.time)))
        })
        .collect::<Vec<_>>();

    if times.is_empty() {
        return Vec::new();
    }

    let first_timestamp = times[0].1;
    let last_timestamp = times.last().map_or(first_timestamp, |(_, time)| *time);
    let duration = (last_timestamp - first_timestamp).abs().max(1);
    let target_ticks = target_tick_count(plot_width);
    let (step_seconds, unit) = choose_time_step(duration, target_ticks);
    let min_index_spacing = min_index_spacing(visible, plot_width);
    let monotonic = times.windows(2).all(|pair| pair[0].1 <= pair[1].1);

    if !monotonic {
        return scan_time_axis_ticks(&times, step_seconds, unit, min_index_spacing);
    }

    let mut ticks = Vec::new();
    let mut last_logical = None;
    push_tick(
        &mut ticks,
        times[0].0,
        times[0].1,
        unit,
        min_index_spacing,
        &mut last_logical,
    );

    if matches!(unit, TimeAxisUnit::Month | TimeAxisUnit::Year) {
        push_calendar_time_axis_ticks(
            &mut ticks,
            &times,
            first_timestamp,
            last_timestamp,
            step_seconds,
            unit,
            min_index_spacing,
            &mut last_logical,
        );
        return ticks;
    }

    let mut boundary = align_up(first_timestamp, step_seconds);
    while boundary <= last_timestamp && ticks.len() < 64 {
        let pos = lower_bound_time(&times, boundary);
        if pos >= times.len() {
            break;
        }

        let (index, timestamp) = times[pos];
        if align_down(timestamp, step_seconds) == boundary {
            push_tick(
                &mut ticks,
                index,
                timestamp,
                unit,
                min_index_spacing,
                &mut last_logical,
            );
        }

        let Some(next_boundary) = boundary.checked_add(step_seconds) else {
            break;
        };
        if next_boundary <= boundary {
            break;
        }
        boundary = next_boundary;
    }

    ticks
}

pub fn normalize_timestamp_seconds(timestamp: i64) -> i64 {
    let magnitude = timestamp.unsigned_abs();

    if magnitude >= 1_000_000_000_000_000_000 {
        timestamp / 1_000_000_000
    } else if magnitude >= 1_000_000_000_000_000 {
        timestamp / 1_000_000
    } else if magnitude >= 100_000_000_000 && !is_compact_nanosecond_duration(timestamp) {
        timestamp / 1_000
    } else if magnitude > 10_000_000_000 {
        timestamp / 1_000_000_000
    } else {
        timestamp
    }
}

pub fn format_time_axis_label(timestamp: i64, unit: TimeAxisUnit) -> String {
    let seconds = normalize_timestamp_seconds(timestamp);
    let (year, month, day, hour, minute, second) = utc_parts(seconds);

    match unit {
        TimeAxisUnit::Second => format!("{hour:02}:{minute:02}:{second:02}"),
        TimeAxisUnit::Minute | TimeAxisUnit::Hour => {
            if hour == 0 && minute == 0 {
                format!("{month:02}-{day:02}")
            } else {
                format!("{hour:02}:{minute:02}")
            }
        }
        TimeAxisUnit::Day => {
            if month == 1 && day == 1 {
                year.to_string()
            } else {
                format!("{month:02}-{day:02}")
            }
        }
        TimeAxisUnit::Month => {
            if month == 1 {
                year.to_string()
            } else {
                month_name(month).to_string()
            }
        }
        TimeAxisUnit::Year => year.to_string(),
    }
}

fn visible_bar_bounds(bar_count: usize, range: LogicalRange) -> Option<(usize, usize)> {
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

fn target_tick_count(plot_width: f32) -> usize {
    (f64::from(plot_width.max(1.0)) / 90.0)
        .floor()
        .clamp(2.0, 12.0) as usize
}

fn choose_time_step(duration_seconds: i64, target_ticks: usize) -> (i64, TimeAxisUnit) {
    TIME_STEPS
        .iter()
        .copied()
        .find(|(step, _)| duration_seconds as f64 / (*step).max(1) as f64 <= target_ticks as f64)
        .unwrap_or_else(|| *TIME_STEPS.last().expect("time steps are configured"))
}

fn min_index_spacing(visible: LogicalRange, plot_width: f32) -> f64 {
    let pixels_per_bar = f64::from(plot_width.max(1.0)) / visible.span();
    (MIN_TICK_SPACING_PX / pixels_per_bar).max(1.0)
}

fn scan_time_axis_ticks(
    times: &[(usize, i64)],
    step_seconds: i64,
    unit: TimeAxisUnit,
    min_index_spacing: f64,
) -> Vec<TimeAxisTick> {
    let mut ticks = Vec::new();
    let mut last_bucket = None;
    let mut last_logical = None;

    for &(index, timestamp) in times {
        let bucket = timestamp.div_euclid(step_seconds);
        if last_bucket == Some(bucket) {
            continue;
        }
        push_tick(
            &mut ticks,
            index,
            timestamp,
            unit,
            min_index_spacing,
            &mut last_logical,
        );
        last_bucket = Some(bucket);
        if ticks.len() >= 64 {
            break;
        }
    }

    ticks
}

fn push_calendar_time_axis_ticks(
    ticks: &mut Vec<TimeAxisTick>,
    times: &[(usize, i64)],
    first_timestamp: i64,
    last_timestamp: i64,
    step_seconds: i64,
    unit: TimeAxisUnit,
    min_index_spacing: f64,
    last_logical: &mut Option<f64>,
) {
    let mut boundary = first_calendar_boundary_at_or_after(first_timestamp, step_seconds, unit);
    while boundary <= last_timestamp && ticks.len() < 64 {
        let pos = lower_bound_time(times, boundary);
        if pos >= times.len() {
            break;
        }

        let (index, timestamp) = times[pos];
        push_tick(
            ticks,
            index,
            timestamp,
            unit,
            min_index_spacing,
            last_logical,
        );

        let Some(next_boundary) = next_calendar_boundary(boundary, step_seconds, unit) else {
            break;
        };
        if next_boundary <= boundary {
            break;
        }
        boundary = next_boundary;
    }
}

fn push_tick(
    ticks: &mut Vec<TimeAxisTick>,
    index: usize,
    timestamp: i64,
    unit: TimeAxisUnit,
    min_index_spacing: f64,
    last_logical: &mut Option<f64>,
) {
    let logical = index as f64;
    if last_logical.is_some_and(|last| logical - last < min_index_spacing) {
        return;
    }

    ticks.push(TimeAxisTick {
        logical,
        index,
        timestamp,
        unit,
    });
    *last_logical = Some(logical);
}

fn lower_bound_time(times: &[(usize, i64)], target: i64) -> usize {
    times.partition_point(|(_, timestamp)| *timestamp < target)
}

fn align_down(value: i64, step: i64) -> i64 {
    value.div_euclid(step) * step
}

fn align_up(value: i64, step: i64) -> i64 {
    let down = align_down(value, step);
    if down < value { down + step } else { down }
}

fn is_compact_nanosecond_duration(timestamp: i64) -> bool {
    timestamp % 1_000_000_000 == 0 && (timestamp / 1_000_000_000).unsigned_abs() <= 86_400
}

fn first_calendar_boundary_at_or_after(
    timestamp: i64,
    step_seconds: i64,
    unit: TimeAxisUnit,
) -> i64 {
    let (year, month, _, _, _, _) = utc_parts(timestamp);
    let boundary = match unit {
        TimeAxisUnit::Month => {
            let step_months = calendar_step_months(step_seconds);
            let start_month = ((month - 1) / step_months) * step_months + 1;
            timestamp_from_utc(year, start_month, 1, 0, 0, 0)
        }
        TimeAxisUnit::Year => timestamp_from_utc(year, 1, 1, 0, 0, 0),
        _ => align_up(timestamp, step_seconds),
    };

    if boundary < timestamp {
        next_calendar_boundary(boundary, step_seconds, unit).unwrap_or(boundary)
    } else {
        boundary
    }
}

fn next_calendar_boundary(timestamp: i64, step_seconds: i64, unit: TimeAxisUnit) -> Option<i64> {
    let (year, month, _, _, _, _) = utc_parts(timestamp);
    match unit {
        TimeAxisUnit::Month => {
            let step_months = calendar_step_months(step_seconds);
            let (next_year, next_month) = add_months(year, month, step_months);
            Some(timestamp_from_utc(next_year, next_month, 1, 0, 0, 0))
        }
        TimeAxisUnit::Year => year
            .checked_add(1)
            .map(|next_year| timestamp_from_utc(next_year, 1, 1, 0, 0, 0)),
        _ => timestamp.checked_add(step_seconds),
    }
}

fn calendar_step_months(step_seconds: i64) -> u32 {
    if step_seconds >= 90 * 24 * 60 * 60 {
        3
    } else {
        1
    }
}

fn add_months(year: i64, month: u32, step_months: u32) -> (i64, u32) {
    let month_index = year * 12 + i64::from(month - 1) + i64::from(step_months);
    let next_year = month_index.div_euclid(12);
    let next_month = month_index.rem_euclid(12) as u32 + 1;

    (next_year, next_month)
}

fn timestamp_from_utc(year: i64, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> i64 {
    days_from_civil(year, month, day) * 86_400
        + i64::from(hour) * 3_600
        + i64::from(minute) * 60
        + i64::from(second)
}

fn utc_parts(timestamp: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = timestamp.div_euclid(86_400);
    let seconds_of_day = timestamp.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = (seconds_of_day / 3_600) as u32;
    let minute = ((seconds_of_day / 60) % 60) as u32;
    let second = (seconds_of_day % 60) as u32;

    (year, month, day, hour, minute, second)
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;

    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);

    (year, month as u32, day as u32)
}

fn month_name(month: u32) -> &'static str {
    match month {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        12 => "Dec",
        _ => "",
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeScaleOptions {
    pub min_bar_spacing: f64,
    pub max_bar_spacing: f64,
    pub right_offset_bars: f64,
    pub default_visible_bars: f64,
    pub overscroll_bars: f64,
}

impl Default for TimeScaleOptions {
    fn default() -> Self {
        Self {
            min_bar_spacing: 0.5,
            max_bar_spacing: 120.0,
            right_offset_bars: 4.0,
            default_visible_bars: 80.0,
            overscroll_bars: 300.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeScale {
    visible: LogicalRange,
    bar_spacing: f64,
    follow_realtime: bool,
    options: TimeScaleOptions,
}

impl Default for TimeScale {
    fn default() -> Self {
        Self::new(TimeScaleOptions::default())
    }
}

impl TimeScale {
    const MIN_VISIBLE_BARS: f64 = 1.2;

    pub fn new(options: TimeScaleOptions) -> Self {
        let visible = options.default_visible_bars.max(Self::MIN_VISIBLE_BARS);
        Self {
            visible: LogicalRange {
                from: -visible + options.right_offset_bars,
                to: options.right_offset_bars,
            },
            bar_spacing: 6.0,
            follow_realtime: true,
            options,
        }
    }

    pub fn visible_logical_range(self) -> LogicalRange {
        self.visible
    }

    pub fn visible_bars(self) -> f64 {
        self.visible.span()
    }

    pub fn bar_spacing(self) -> f64 {
        self.bar_spacing
    }

    pub fn follows_realtime(self) -> bool {
        self.follow_realtime
    }

    pub fn set_visible_logical_range(&mut self, range: LogicalRange, plot_width: f64) {
        self.visible = range;
        self.sync_bar_spacing(plot_width);
        self.follow_realtime = false;
    }

    pub fn sync_bar_spacing(&mut self, plot_width: f64) {
        if plot_width > 0.0 {
            self.bar_spacing = (plot_width / self.visible.span())
                .clamp(self.options.min_bar_spacing, self.options.max_bar_spacing);
        }
    }

    pub fn reset(&mut self, data_len: usize, plot_width: f64) {
        self.reset_with_visible_bars(data_len, plot_width, self.options.default_visible_bars);
    }

    pub fn reset_with_visible_bars(
        &mut self,
        data_len: usize,
        plot_width: f64,
        target_visible_bars: f64,
    ) {
        let span =
            target_visible_bars.clamp(Self::MIN_VISIBLE_BARS, self.max_visible_bars(data_len));
        let to = self.latest_logical(data_len) + self.options.right_offset_bars;
        self.visible = LogicalRange {
            from: to - span + 1.0,
            to,
        };
        self.follow_realtime = true;
        self.sync_bar_spacing(plot_width);
    }

    pub fn fit_content(&mut self, data_len: usize, plot_width: f64) {
        if data_len == 0 {
            self.reset(data_len, plot_width);
            return;
        }
        self.visible = LogicalRange {
            from: 0.0,
            to: data_len.saturating_sub(1) as f64 + self.options.right_offset_bars,
        };
        self.follow_realtime = false;
        self.sync_bar_spacing(plot_width);
    }

    pub fn scroll_to_realtime(&mut self, data_len: usize) {
        let span = self.visible.span();
        let to = self.latest_logical(data_len) + self.options.right_offset_bars;
        self.visible = LogicalRange {
            from: to - span + 1.0,
            to,
        };
        self.follow_realtime = true;
    }

    pub fn on_data_len_changed(&mut self, old_len: usize, new_len: usize) {
        if old_len == 0 || new_len < old_len {
            self.scroll_to_realtime(new_len);
            return;
        }
        let old_last_visible = self.visible.to >= old_len.saturating_sub(1) as f64;
        if (self.follow_realtime || old_last_visible) && new_len > old_len {
            let added = new_len.saturating_sub(old_len) as f64;
            self.visible = self.visible.shifted(added);
            self.follow_realtime = true;
        }
        self.clamp(new_len);
    }

    pub fn logical_to_x(self, logical: f64, plot_left: f64, plot_width: f64) -> f64 {
        plot_left + ((logical - self.visible.from + 0.5) / self.visible.span()) * plot_width
    }

    pub fn x_to_logical(self, x: f64, plot_left: f64, plot_width: f64) -> f64 {
        let ratio = ((x - plot_left) / plot_width.max(1.0)).clamp(0.0, 1.0);
        self.visible.from - 0.5 + ratio * self.visible.span()
    }

    pub fn pan_by_bars(&mut self, delta_bars: f64, data_len: usize) {
        self.visible = self.visible.shifted(delta_bars);
        self.follow_realtime = false;
        self.clamp(data_len);
    }

    pub fn pan_by_pixels(&mut self, delta_x: f64, data_len: usize) {
        self.pan_by_bars(-delta_x / self.bar_spacing.max(0.000_001), data_len);
    }

    pub fn zoom_at_x(
        &mut self,
        multiplier: f64,
        anchor_x: f64,
        plot_left: f64,
        plot_width: f64,
        data_len: usize,
    ) {
        if plot_width <= 0.0 {
            return;
        }
        let multiplier = multiplier.clamp(0.02, 50.0);
        let old_anchor = self.x_to_logical(anchor_x, plot_left, plot_width);
        let anchor_ratio = ((anchor_x - plot_left) / plot_width).clamp(0.0, 1.0);
        self.bar_spacing = (self.bar_spacing * multiplier)
            .clamp(self.options.min_bar_spacing, self.options.max_bar_spacing);
        let new_span = (plot_width / self.bar_spacing)
            .clamp(Self::MIN_VISIBLE_BARS, self.max_visible_bars(data_len));
        let from = old_anchor + 0.5 - anchor_ratio * new_span;
        self.visible = LogicalRange {
            from,
            to: from + new_span - 1.0,
        };
        self.follow_realtime = false;
        self.clamp(data_len);
        self.sync_bar_spacing(plot_width);
    }

    fn latest_logical(self, data_len: usize) -> f64 {
        data_len.saturating_sub(1) as f64
    }

    fn max_visible_bars(self, data_len: usize) -> f64 {
        (data_len.max(1) as f64 + self.options.right_offset_bars + self.options.overscroll_bars)
            .max(self.options.default_visible_bars)
    }

    fn clamp(&mut self, data_len: usize) {
        let span = self.visible.span();
        let max_to = self.latest_logical(data_len)
            + self.options.right_offset_bars
            + self.options.overscroll_bars;
        if self.visible.to > max_to {
            self.visible.to = max_to;
            self.visible.from = self.visible.to - span + 1.0;
        }

        let min_from = -self.options.overscroll_bars.max(span);
        if self.visible.from < min_from {
            self.visible.from = min_from;
            self.visible.to = self.visible.from + span - 1.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LogicalRange, OhlcBar, TimeAxisUnit, TimeScale, format_time_axis_label,
        normalize_timestamp_seconds, timestamp_from_utc, visible_time_axis_ticks,
    };

    fn bar_at(time: i64) -> OhlcBar {
        OhlcBar {
            time,
            open: 10.0,
            high: 12.0,
            low: 9.0,
            close: 11.0,
            volume: 100.0,
        }
    }

    fn daily_bars(start: i64, end: i64) -> Vec<OhlcBar> {
        let mut bars = Vec::new();
        let mut time = start;
        while time <= end {
            bars.push(bar_at(time));
            time += 86_400;
        }
        bars
    }

    #[test]
    fn reset_places_latest_bar_near_right_offset() {
        let mut scale = TimeScale::default();
        scale.reset_with_visible_bars(100, 800.0, 80.0);

        assert_eq!(scale.visible_logical_range().to, 103.0);
        assert_eq!(scale.visible_logical_range().from, 24.0);
        assert!((scale.bar_spacing() - 10.0).abs() < 1e-9);
        assert!(scale.follows_realtime());
    }

    #[test]
    fn coordinate_transform_round_trips_fractional_logical_indices() {
        let mut scale = TimeScale::default();
        scale.set_visible_logical_range(
            LogicalRange {
                from: -4.75,
                to: 5.25,
            },
            1_000.0,
        );

        let x = scale.logical_to_x(0.5, 100.0, 1_000.0);
        assert!((scale.x_to_logical(x, 100.0, 1_000.0) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn zoom_at_coordinate_keeps_anchor_logical_stable() {
        let mut scale = TimeScale::default();
        scale.set_visible_logical_range(
            LogicalRange {
                from: 0.0,
                to: 100.0,
            },
            1_000.0,
        );
        let anchor_x = 250.0;
        let before = scale.x_to_logical(anchor_x, 0.0, 1_000.0);

        scale.zoom_at_x(2.0, anchor_x, 0.0, 1_000.0, 200);

        let after = scale.x_to_logical(anchor_x, 0.0, 1_000.0);
        assert!((after - before).abs() < 1e-9);
        assert!(scale.visible_bars() < 100.0);
    }

    #[test]
    fn follow_realtime_shifts_when_new_bars_arrive() {
        let mut scale = TimeScale::default();
        scale.reset_with_visible_bars(10, 800.0, 20.0);
        let before = scale.visible_logical_range();

        scale.on_data_len_changed(10, 15);

        assert_eq!(
            scale.visible_logical_range(),
            LogicalRange {
                from: before.from + 5.0,
                to: before.to + 5.0
            }
        );
    }

    #[test]
    fn fit_content_includes_half_bar_edges() {
        let mut scale = TimeScale::default();
        scale.fit_content(5, 500.0);

        assert_eq!(
            scale.visible_logical_range(),
            LogicalRange { from: 0.0, to: 8.0 }
        );
    }

    #[test]
    fn visible_time_axis_ticks_returns_empty_for_no_bars() {
        assert!(
            visible_time_axis_ticks(
                &[],
                LogicalRange {
                    from: 0.0,
                    to: 10.0
                },
                800.0
            )
            .is_empty()
        );
    }

    #[test]
    fn visible_time_axis_ticks_handles_single_bar() {
        let bars = [bar_at(3_600)];
        let ticks = visible_time_axis_ticks(&bars, LogicalRange { from: 0.0, to: 0.0 }, 800.0);

        assert_eq!(ticks.len(), 1);
        assert_eq!(ticks[0].index, 0);
        assert_eq!(
            format_time_axis_label(ticks[0].timestamp, ticks[0].unit),
            "01:00:00"
        );
    }

    #[test]
    fn visible_time_axis_ticks_uses_timestamp_buckets_not_index_steps() {
        let bars = (0..=120)
            .map(|index| bar_at(index * 60))
            .collect::<Vec<_>>();
        let ticks = visible_time_axis_ticks(
            &bars,
            LogicalRange {
                from: 0.0,
                to: 120.0,
            },
            900.0,
        );

        assert!(ticks.len() >= 6);
        assert!(ticks.iter().all(|tick| tick.index % 15 == 0));
        assert!(
            ticks
                .iter()
                .any(|tick| format_time_axis_label(tick.timestamp, tick.unit) == "00:15")
        );
    }

    #[test]
    fn visible_time_axis_ticks_preserves_nanosecond_timestamps() {
        let bars = (0..=60)
            .map(|index| bar_at(index * 60 * 1_000_000_000))
            .collect::<Vec<_>>();
        let ticks = visible_time_axis_ticks(
            &bars,
            LogicalRange {
                from: 0.0,
                to: 60.0,
            },
            900.0,
        );

        assert_eq!(normalize_timestamp_seconds(3_600_000_000_000), 3_600);
        assert!(
            ticks
                .iter()
                .any(|tick| format_time_axis_label(tick.timestamp, tick.unit) == "01:00")
        );
    }

    #[test]
    fn timestamp_normalization_handles_epoch_milliseconds_and_compact_nanoseconds() {
        assert_eq!(normalize_timestamp_seconds(946_684_799_000), 946_684_799);
        assert_eq!(normalize_timestamp_seconds(3_600_000_000_000), 3_600);
        assert_eq!(
            normalize_timestamp_seconds(1_704_067_200_000_000_000),
            1_704_067_200
        );
        assert_eq!(
            normalize_timestamp_seconds(1_704_067_200_000_000),
            1_704_067_200
        );
        assert_eq!(
            normalize_timestamp_seconds(4_102_444_800_000),
            4_102_444_800
        );
        assert_eq!(normalize_timestamp_seconds(-3_600_000_000_000), -3_600);
    }

    #[test]
    fn visible_time_axis_ticks_suppresses_dense_overlapping_ticks() {
        let bars = (0..=120)
            .map(|index| bar_at(index * 60))
            .collect::<Vec<_>>();
        let ticks = visible_time_axis_ticks(
            &bars,
            LogicalRange {
                from: 0.0,
                to: 120.0,
            },
            120.0,
        );

        assert!(ticks.len() <= 3);
    }

    #[test]
    fn visible_time_axis_ticks_handles_session_gap() {
        let bars = [
            bar_at(9 * 3_600 + 30 * 60),
            bar_at(9 * 3_600 + 31 * 60),
            bar_at(9 * 3_600 + 32 * 60),
            bar_at(16 * 3_600),
        ];
        let ticks = visible_time_axis_ticks(&bars, LogicalRange { from: 0.0, to: 3.0 }, 900.0);
        let labels = ticks
            .iter()
            .map(|tick| format_time_axis_label(tick.timestamp, tick.unit))
            .collect::<Vec<_>>();

        assert_eq!(labels.first().map(String::as_str), Some("09:30"));
        assert!(labels.iter().any(|label| label == "16:00"));
        assert!(!labels.iter().any(|label| label == "10:00"));
    }

    #[test]
    fn visible_time_axis_ticks_aligns_month_ticks_to_first_of_month() {
        let bars = daily_bars(
            timestamp_from_utc(2024, 1, 20, 0, 0, 0),
            timestamp_from_utc(2024, 4, 10, 0, 0, 0),
        );
        let ticks = visible_time_axis_ticks(
            &bars,
            LogicalRange {
                from: 0.0,
                to: bars.len().saturating_sub(1) as f64,
            },
            600.0,
        );

        assert!(ticks.iter().any(|tick| {
            tick.unit == TimeAxisUnit::Month
                && tick.timestamp == timestamp_from_utc(2024, 2, 1, 0, 0, 0)
        }));
        assert!(ticks.iter().any(|tick| {
            tick.unit == TimeAxisUnit::Month
                && tick.timestamp == timestamp_from_utc(2024, 3, 1, 0, 0, 0)
        }));
    }

    #[test]
    fn visible_time_axis_ticks_aligns_quarter_ticks_to_calendar_months() {
        let bars = daily_bars(
            timestamp_from_utc(2024, 1, 20, 0, 0, 0),
            timestamp_from_utc(2024, 6, 10, 0, 0, 0),
        );
        let ticks = visible_time_axis_ticks(
            &bars,
            LogicalRange {
                from: 0.0,
                to: bars.len().saturating_sub(1) as f64,
            },
            160.0,
        );

        assert!(ticks.iter().any(|tick| {
            tick.unit == TimeAxisUnit::Month
                && tick.timestamp == timestamp_from_utc(2024, 4, 1, 0, 0, 0)
        }));
    }

    #[test]
    fn visible_time_axis_ticks_aligns_year_ticks_to_january_first() {
        let bars = daily_bars(
            timestamp_from_utc(2019, 12, 15, 0, 0, 0),
            timestamp_from_utc(2022, 1, 15, 0, 0, 0),
        );
        let ticks = visible_time_axis_ticks(
            &bars,
            LogicalRange {
                from: 0.0,
                to: bars.len().saturating_sub(1) as f64,
            },
            600.0,
        );

        assert!(ticks.iter().any(|tick| {
            tick.unit == TimeAxisUnit::Year
                && tick.timestamp == timestamp_from_utc(2021, 1, 1, 0, 0, 0)
        }));
    }

    #[test]
    fn time_axis_label_formats_intraday_and_calendar_units() {
        assert_eq!(
            format_time_axis_label(3_661, TimeAxisUnit::Second),
            "01:01:01"
        );
        assert_eq!(format_time_axis_label(3_600, TimeAxisUnit::Minute), "01:00");
        assert_eq!(
            format_time_axis_label(1_704_067_200, TimeAxisUnit::Day),
            "2024"
        );
        assert_eq!(
            format_time_axis_label(1_711_929_600, TimeAxisUnit::Month),
            "Apr"
        );
    }
}
