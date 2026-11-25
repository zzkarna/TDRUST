use super::{BarStore, LogicalRange, OhlcBar, TailUpdate};

#[derive(Debug, Clone, PartialEq)]
pub enum BarStoreReplayEvent {
    ReplaceAll(Vec<OhlcBar>),
    PrependHistory(Vec<OhlcBar>),
    ApplyTail(OhlcBar),
    CaptureVisible(LogicalRange),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BarStoreReplayStep {
    ReplacedAll {
        len: usize,
        generation: u64,
    },
    PrependedHistory {
        added: usize,
        len: usize,
        generation: u64,
    },
    AppliedTail {
        update: TailUpdate,
        len: usize,
        generation: u64,
    },
    CapturedVisible(ReplayVisibleWindow),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplayVisibleWindow {
    pub range: LogicalRange,
    pub start: Option<usize>,
    pub end_exclusive: Option<usize>,
    pub times: Vec<i64>,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BarStoreReplayResult {
    pub steps: Vec<BarStoreReplayStep>,
    pub final_bars: Vec<OhlcBar>,
    pub final_generation: u64,
}

pub fn replay_bar_store(
    events: impl IntoIterator<Item = BarStoreReplayEvent>,
    max_len: Option<usize>,
) -> BarStoreReplayResult {
    let mut store = BarStore::new(max_len);
    let mut steps = Vec::new();

    for event in events {
        match event {
            BarStoreReplayEvent::ReplaceAll(bars) => {
                store.replace_all(bars);
                steps.push(BarStoreReplayStep::ReplacedAll {
                    len: store.len(),
                    generation: store.generation(),
                });
            }
            BarStoreReplayEvent::PrependHistory(bars) => {
                let added = store.prepend_history(bars);
                steps.push(BarStoreReplayStep::PrependedHistory {
                    added,
                    len: store.len(),
                    generation: store.generation(),
                });
            }
            BarStoreReplayEvent::ApplyTail(bar) => {
                let update = store.apply_tail(bar);
                steps.push(BarStoreReplayStep::AppliedTail {
                    update,
                    len: store.len(),
                    generation: store.generation(),
                });
            }
            BarStoreReplayEvent::CaptureVisible(range) => {
                steps.push(BarStoreReplayStep::CapturedVisible(capture_visible(
                    &store, range,
                )));
            }
        }
    }

    BarStoreReplayResult {
        steps,
        final_bars: store.to_vec(),
        final_generation: store.generation(),
    }
}

fn capture_visible(store: &BarStore, range: LogicalRange) -> ReplayVisibleWindow {
    let visible = store.visible_window(range);
    ReplayVisibleWindow {
        range,
        start: visible.as_ref().map(|window| window.start()),
        end_exclusive: visible.as_ref().map(|window| window.end_exclusive()),
        times: visible
            .map(|window| window.iter().map(|bar| bar.time).collect())
            .unwrap_or_default(),
        generation: store.generation(),
    }
}

#[cfg(test)]
mod tests {
    use super::{BarStoreReplayEvent, BarStoreReplayStep, ReplayVisibleWindow, replay_bar_store};
    use crate::chart_core::{LogicalRange, OhlcBar, TailUpdate};

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

    fn invalid_bar(time: i64) -> OhlcBar {
        OhlcBar {
            time,
            open: 10.0,
            high: 9.0,
            low: 11.0,
            close: 10.0,
            volume: 1.0,
        }
    }

    fn fixture_events() -> Vec<BarStoreReplayEvent> {
        vec![
            BarStoreReplayEvent::ReplaceAll(vec![
                bar(3, 3.0),
                bar(1, 1.0),
                bar(2, 2.0),
                bar(2, 20.0),
            ]),
            BarStoreReplayEvent::CaptureVisible(LogicalRange { from: 0.4, to: 1.2 }),
            BarStoreReplayEvent::ApplyTail(bar(3, 30.0)),
            BarStoreReplayEvent::ApplyTail(bar(2, 200.0)),
            BarStoreReplayEvent::ApplyTail(invalid_bar(4)),
            BarStoreReplayEvent::ApplyTail(bar(4, 4.0)),
            BarStoreReplayEvent::PrependHistory(vec![bar(0, 0.0), bar(1, 100.0)]),
            BarStoreReplayEvent::CaptureVisible(LogicalRange { from: 0.0, to: 2.0 }),
            BarStoreReplayEvent::ApplyTail(bar(5, 5.0)),
            BarStoreReplayEvent::CaptureVisible(LogicalRange {
                from: 0.0,
                to: 10.0,
            }),
        ]
    }

    #[test]
    fn replay_fixture_is_deterministic() {
        let first = replay_bar_store(fixture_events(), Some(5));
        let second = replay_bar_store(fixture_events(), Some(5));

        assert_eq!(first, second);
        assert_eq!(
            first
                .final_bars
                .iter()
                .map(|bar| (bar.time, bar.close))
                .collect::<Vec<_>>(),
            vec![(1, 1.0), (2, 20.0), (3, 30.0), (4, 4.0), (5, 5.0)]
        );
        assert_eq!(first.final_generation, 5);
    }

    #[test]
    fn replay_records_generations_and_visible_ranges() {
        let result = replay_bar_store(fixture_events(), Some(5));

        assert_eq!(
            result.steps,
            vec![
                BarStoreReplayStep::ReplacedAll {
                    len: 3,
                    generation: 1
                },
                BarStoreReplayStep::CapturedVisible(ReplayVisibleWindow {
                    range: LogicalRange { from: 0.4, to: 1.2 },
                    start: Some(0),
                    end_exclusive: Some(3),
                    times: vec![1, 2, 3],
                    generation: 1
                }),
                BarStoreReplayStep::AppliedTail {
                    update: TailUpdate::ReplacedTail,
                    len: 3,
                    generation: 2
                },
                BarStoreReplayStep::AppliedTail {
                    update: TailUpdate::IgnoredStale,
                    len: 3,
                    generation: 2
                },
                BarStoreReplayStep::AppliedTail {
                    update: TailUpdate::IgnoredInvalid,
                    len: 3,
                    generation: 2
                },
                BarStoreReplayStep::AppliedTail {
                    update: TailUpdate::Appended,
                    len: 4,
                    generation: 3
                },
                BarStoreReplayStep::PrependedHistory {
                    added: 1,
                    len: 5,
                    generation: 4
                },
                BarStoreReplayStep::CapturedVisible(ReplayVisibleWindow {
                    range: LogicalRange { from: 0.0, to: 2.0 },
                    start: Some(0),
                    end_exclusive: Some(3),
                    times: vec![0, 1, 2],
                    generation: 4
                }),
                BarStoreReplayStep::AppliedTail {
                    update: TailUpdate::Appended,
                    len: 5,
                    generation: 5
                },
                BarStoreReplayStep::CapturedVisible(ReplayVisibleWindow {
                    range: LogicalRange {
                        from: 0.0,
                        to: 10.0
                    },
                    start: Some(0),
                    end_exclusive: Some(5),
                    times: vec![1, 2, 3, 4, 5],
                    generation: 5
                }),
            ]
        );
    }
}
