use crate::chart_core::{BarSeries, OhlcBar, SeriesId, SeriesRegistry, TailUpdate};

use super::output::{
    StudyDataSource, StudyId, StudyOutputBuffer, StudyOutputDescriptor, StudyValue,
};

pub trait StudyProgram {
    fn reset(&mut self);
    fn output_descriptors(&self, study: StudyId) -> Vec<StudyOutputDescriptor>;
    fn next(&mut self, index: usize, bar: OhlcBar) -> Vec<StudyValue>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StudyRuntimeUpdate {
    Unchanged,
    Reset,
    Replayed,
    TailAppended,
    TailRecomputed,
}

pub struct StudyRuntime {
    studies: Vec<StudyInstance>,
    next_id: u32,
    generation: u64,
}

impl Default for StudyRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl StudyRuntime {
    pub fn new() -> Self {
        Self {
            studies: Vec::new(),
            next_id: 1,
            generation: 0,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn is_empty(&self) -> bool {
        self.studies.is_empty()
    }

    pub fn study_count(&self) -> usize {
        self.studies.len()
    }

    pub fn add_study(
        &mut self,
        name: impl Into<String>,
        program: impl StudyProgram + 'static,
    ) -> StudyId {
        let id = StudyId::new(self.next_id);
        self.add_study_with_id(id, name, program);
        id
    }

    pub fn add_study_with_id(
        &mut self,
        id: StudyId,
        name: impl Into<String>,
        program: impl StudyProgram + 'static,
    ) -> StudyId {
        let instance = StudyInstance::new(id, name.into(), Box::new(program));
        if let Some(existing) = self.studies.iter_mut().find(|study| study.id == id) {
            *existing = instance;
        } else {
            self.studies.push(instance);
        }
        self.next_id = self.next_id.max(id.0.saturating_add(1));
        self.bump_generation();
        id
    }

    pub fn reset(&mut self) -> StudyRuntimeUpdate {
        if self.studies.is_empty() {
            return StudyRuntimeUpdate::Unchanged;
        }
        for study in &mut self.studies {
            study.reset();
        }
        self.bump_generation();
        StudyRuntimeUpdate::Reset
    }

    pub fn replay<B: BarSeries + ?Sized>(&mut self, bars: &B) -> StudyRuntimeUpdate {
        if self.studies.is_empty() {
            return StudyRuntimeUpdate::Unchanged;
        }
        for study in &mut self.studies {
            study.replay(bars);
        }
        self.bump_generation();
        StudyRuntimeUpdate::Replayed
    }

    pub fn apply_tail<B: BarSeries + ?Sized>(
        &mut self,
        bars: &B,
        update: TailUpdate,
    ) -> StudyRuntimeUpdate {
        if self.studies.is_empty() {
            return StudyRuntimeUpdate::Unchanged;
        }

        match update {
            TailUpdate::Appended => self.append_tail_or_replay(bars),
            TailUpdate::ReplacedTail => {
                for study in &mut self.studies {
                    study.replay(bars);
                }
                self.bump_generation();
                StudyRuntimeUpdate::TailRecomputed
            }
            TailUpdate::IgnoredStale | TailUpdate::IgnoredInvalid => StudyRuntimeUpdate::Unchanged,
        }
    }

    pub fn register_output_series(&self, registry: &mut SeriesRegistry) {
        for output in self.output_buffers() {
            registry.add_or_replace(output.descriptor().to_series_spec());
        }
    }

    pub fn output_buffers(&self) -> impl Iterator<Item = &StudyOutputBuffer> {
        self.studies.iter().flat_map(|study| study.outputs.iter())
    }

    pub fn output_values(&self, series_id: SeriesId) -> Option<&[StudyValue]> {
        self.output_buffers()
            .find(|output| output.series_id() == series_id)
            .map(|output| output.values())
    }

    fn append_tail_or_replay<B: BarSeries + ?Sized>(&mut self, bars: &B) -> StudyRuntimeUpdate {
        let Some(tail_index) = bars.len().checked_sub(1) else {
            return StudyRuntimeUpdate::Unchanged;
        };

        let mut recomputed = false;
        for study in &mut self.studies {
            if study.cursor == tail_index {
                study.push_index(tail_index, bars.bar(tail_index));
            } else {
                study.replay(bars);
                recomputed = true;
            }
        }

        self.bump_generation();
        if recomputed {
            StudyRuntimeUpdate::TailRecomputed
        } else {
            StudyRuntimeUpdate::TailAppended
        }
    }

    fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
}

impl StudyDataSource for StudyRuntime {
    fn output_values(&self, series_id: SeriesId) -> Option<&[StudyValue]> {
        StudyRuntime::output_values(self, series_id)
    }
}

struct StudyInstance {
    id: StudyId,
    name: String,
    program: Box<dyn StudyProgram>,
    outputs: Vec<StudyOutputBuffer>,
    cursor: usize,
}

impl StudyInstance {
    fn new(id: StudyId, name: String, program: Box<dyn StudyProgram>) -> Self {
        let outputs = program
            .output_descriptors(id)
            .into_iter()
            .map(StudyOutputBuffer::new)
            .collect();
        Self {
            id,
            name,
            program,
            outputs,
            cursor: 0,
        }
    }

    fn reset(&mut self) {
        self.program.reset();
        self.cursor = 0;
        for output in &mut self.outputs {
            output.clear();
        }
    }

    fn replay<B: BarSeries + ?Sized>(&mut self, bars: &B) {
        self.reset();
        for index in 0..bars.len() {
            self.push_index(index, bars.bar(index));
        }
    }

    fn push_index(&mut self, index: usize, bar: Option<OhlcBar>) {
        let values = match bar.filter(|bar| bar.is_valid()) {
            Some(bar) => self.program.next(index, bar),
            None => Vec::new(),
        };

        for (output_index, output) in self.outputs.iter_mut().enumerate() {
            let value = if index < output.descriptor().warmup_bars {
                StudyValue::Null
            } else {
                values
                    .get(output_index)
                    .copied()
                    .unwrap_or(StudyValue::Null)
                    .sanitized()
            };
            output.push(value);
        }
        self.cursor = index + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::{StudyProgram, StudyRuntime, StudyRuntimeUpdate};
    use crate::{
        chart_core::{
            BarStore, OhlcBar, PaneId, PriceScaleId, SeriesKind, SeriesRegistry, SeriesValueSource,
            TailUpdate,
        },
        studies::{StudyId, StudyOutputDescriptor, StudyOutputId, StudyValue},
    };

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

    struct RunningSumProgram {
        sum: f64,
        warmup_bars: usize,
    }

    impl RunningSumProgram {
        fn new(warmup_bars: usize) -> Self {
            Self {
                sum: 0.0,
                warmup_bars,
            }
        }
    }

    impl StudyProgram for RunningSumProgram {
        fn reset(&mut self) {
            self.sum = 0.0;
        }

        fn output_descriptors(&self, study: StudyId) -> Vec<StudyOutputDescriptor> {
            vec![
                StudyOutputDescriptor::line(
                    StudyOutputId::new(study, 0),
                    "Running Sum",
                    PaneId::Study(study.0),
                    PriceScaleId::Overlay,
                )
                .with_warmup_bars(self.warmup_bars),
            ]
        }

        fn next(&mut self, _index: usize, bar: OhlcBar) -> Vec<StudyValue> {
            self.sum += bar.close;
            vec![StudyValue::number(self.sum)]
        }
    }

    struct SparseProgram;

    impl StudyProgram for SparseProgram {
        fn reset(&mut self) {}

        fn output_descriptors(&self, study: StudyId) -> Vec<StudyOutputDescriptor> {
            vec![
                StudyOutputDescriptor::line(
                    StudyOutputId::new(study, 0),
                    "Finite",
                    PaneId::Study(study.0),
                    PriceScaleId::Overlay,
                )
                .with_warmup_bars(1),
                StudyOutputDescriptor::line(
                    StudyOutputId::new(study, 1),
                    "Missing",
                    PaneId::Study(study.0),
                    PriceScaleId::Overlay,
                ),
            ]
        }

        fn next(&mut self, index: usize, bar: OhlcBar) -> Vec<StudyValue> {
            let finite = if index == 1 { f64::NAN } else { bar.close };
            vec![StudyValue::number(finite)]
        }
    }

    #[test]
    fn replay_resets_state_and_writes_warmup_nulls() {
        let mut runtime = StudyRuntime::new();
        let study_id = runtime.add_study_with_id(StudyId::new(1), "Sum", RunningSumProgram::new(2));
        let mut bars = BarStore::unbounded();
        bars.replace_all([bar(1, 10.0), bar(2, 20.0), bar(3, 30.0)]);

        assert_eq!(runtime.replay(&bars), StudyRuntimeUpdate::Replayed);

        let values = runtime
            .output_values(StudyOutputId::new(study_id, 0).series_id())
            .expect("running sum output");
        assert_eq!(
            values,
            &[StudyValue::Null, StudyValue::Null, StudyValue::Number(60.0)]
        );
    }

    #[test]
    fn tail_append_updates_only_the_next_output_row() {
        let mut runtime = StudyRuntime::new();
        let study_id = runtime.add_study("Sum", RunningSumProgram::new(0));
        let mut bars = BarStore::unbounded();
        bars.replace_all([bar(1, 1.0), bar(2, 2.0)]);
        runtime.replay(&bars);

        let update = bars.apply_tail(bar(3, 3.0));
        assert_eq!(update, TailUpdate::Appended);
        assert_eq!(
            runtime.apply_tail(&bars, update),
            StudyRuntimeUpdate::TailAppended
        );

        let values = runtime
            .output_values(StudyOutputId::new(study_id, 0).series_id())
            .expect("running sum output");
        assert_eq!(
            values,
            &[
                StudyValue::Number(1.0),
                StudyValue::Number(3.0),
                StudyValue::Number(6.0)
            ]
        );
    }

    #[test]
    fn tail_replacement_replays_from_reset_for_determinism() {
        let mut runtime = StudyRuntime::new();
        let study_id = runtime.add_study("Sum", RunningSumProgram::new(0));
        let mut bars = BarStore::unbounded();
        bars.replace_all([bar(1, 1.0), bar(2, 2.0), bar(3, 3.0)]);
        runtime.replay(&bars);

        let update = bars.apply_tail(bar(3, 30.0));
        assert_eq!(update, TailUpdate::ReplacedTail);
        assert_eq!(
            runtime.apply_tail(&bars, update),
            StudyRuntimeUpdate::TailRecomputed
        );

        let values = runtime
            .output_values(StudyOutputId::new(study_id, 0).series_id())
            .expect("running sum output");
        assert_eq!(
            values,
            &[
                StudyValue::Number(1.0),
                StudyValue::Number(3.0),
                StudyValue::Number(33.0)
            ]
        );
    }

    #[test]
    fn invalid_or_missing_outputs_are_stored_as_nulls() {
        let mut runtime = StudyRuntime::new();
        let study_id = runtime.add_study("Sparse", SparseProgram);
        let mut bars = BarStore::unbounded();
        bars.replace_all([bar(1, 10.0), bar(2, 20.0), bar(3, 30.0)]);

        runtime.replay(&bars);

        let finite = runtime
            .output_values(StudyOutputId::new(study_id, 0).series_id())
            .expect("finite output");
        let missing = runtime
            .output_values(StudyOutputId::new(study_id, 1).series_id())
            .expect("missing output");

        assert_eq!(
            finite,
            &[StudyValue::Null, StudyValue::Null, StudyValue::Number(30.0)]
        );
        assert_eq!(
            missing,
            &[StudyValue::Null, StudyValue::Null, StudyValue::Null]
        );
    }

    #[test]
    fn output_series_register_with_the_chart_registry() {
        let mut runtime = StudyRuntime::new();
        let study_id = runtime.add_study_with_id(StudyId::new(7), "Sum", RunningSumProgram::new(0));
        let mut registry = SeriesRegistry::default();

        runtime.register_output_series(&mut registry);

        let spec = registry
            .get(StudyOutputId::new(study_id, 0).series_id())
            .expect("registered output series");
        assert_eq!(spec.name, "Running Sum");
        assert_eq!(spec.kind, SeriesKind::Line);
        assert_eq!(spec.value_source, SeriesValueSource::StudyOutput);
        assert_eq!(spec.pane, PaneId::Study(7));
        assert_eq!(spec.price_scale, PriceScaleId::Overlay);
    }

    #[test]
    fn ignored_tail_updates_do_not_bump_generation() {
        let mut runtime = StudyRuntime::new();
        runtime.add_study("Sum", RunningSumProgram::new(0));
        let generation = runtime.generation();

        assert_eq!(
            runtime.apply_tail(&Vec::<OhlcBar>::new(), TailUpdate::IgnoredStale),
            StudyRuntimeUpdate::Unchanged
        );
        assert_eq!(runtime.generation(), generation);
    }
}
