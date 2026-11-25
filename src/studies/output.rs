use crate::chart_core::{
    PaneId, PriceScaleId, SeriesId, SeriesKind, SeriesSpec, SeriesValueSource,
};

pub const STUDY_SERIES_BASE: u32 = 10_000;
pub const STUDY_SERIES_STRIDE: u32 = 1_000;

pub trait StudyDataSource {
    fn output_values(&self, series_id: SeriesId) -> Option<&[StudyValue]>;

    fn output_style(&self, _series_id: SeriesId) -> Option<StudyOutputStyle> {
        None
    }

    fn output_offset(&self, _series_id: SeriesId) -> i32 {
        0
    }

    fn pane_guides(&self, _pane: PaneId) -> Vec<f64> {
        Vec::new()
    }

    fn pane_guide_styles(&self, _pane: PaneId) -> Vec<StudyGuideStyle> {
        Vec::new()
    }

    fn pane_label(&self, _pane: PaneId) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StudyColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl StudyColor {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StudyOutputStyle {
    pub visible: bool,
    pub color: StudyColor,
    pub negative_color: Option<StudyColor>,
    pub width: f32,
}

impl StudyOutputStyle {
    pub const fn line(visible: bool, color: StudyColor, width: f32) -> Self {
        Self {
            visible,
            color,
            negative_color: None,
            width,
        }
    }

    pub const fn histogram(
        visible: bool,
        positive_color: StudyColor,
        negative_color: StudyColor,
    ) -> Self {
        Self {
            visible,
            color: positive_color,
            negative_color: Some(negative_color),
            width: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StudyGuideStyle {
    pub level: f64,
    pub visible: bool,
    pub color: StudyColor,
    pub width: f32,
}

impl StudyGuideStyle {
    pub const fn new(level: f64, visible: bool, color: StudyColor, width: f32) -> Self {
        Self {
            level,
            visible,
            color,
            width,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct StudyId(pub u32);

impl StudyId {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StudyOutputId {
    pub study: StudyId,
    pub output: u32,
}

impl StudyOutputId {
    pub const fn new(study: StudyId, output: u32) -> Self {
        Self { study, output }
    }

    pub fn series_id(self) -> SeriesId {
        SeriesId::new(
            STUDY_SERIES_BASE
                .saturating_add(self.study.0.saturating_mul(STUDY_SERIES_STRIDE))
                .saturating_add(self.output),
        )
    }

    pub fn from_series_id(series_id: SeriesId) -> Option<Self> {
        let offset = series_id.0.checked_sub(STUDY_SERIES_BASE)?;
        let study = offset / STUDY_SERIES_STRIDE;
        let output = offset % STUDY_SERIES_STRIDE;
        Some(Self {
            study: StudyId::new(study),
            output,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StudyValue {
    Null,
    Number(f64),
}

impl StudyValue {
    pub fn number(value: f64) -> Self {
        if value.is_finite() {
            Self::Number(value)
        } else {
            Self::Null
        }
    }

    pub fn as_f64(self) -> Option<f64> {
        match self {
            Self::Null => None,
            Self::Number(value) => Some(value),
        }
    }

    pub fn is_null(self) -> bool {
        matches!(self, Self::Null)
    }

    pub fn sanitized(self) -> Self {
        match self {
            Self::Number(value) => Self::number(value),
            Self::Null => Self::Null,
        }
    }
}

impl From<f64> for StudyValue {
    fn from(value: f64) -> Self {
        Self::number(value)
    }
}

impl From<Option<f64>> for StudyValue {
    fn from(value: Option<f64>) -> Self {
        value.map(Self::number).unwrap_or(Self::Null)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StudyOutputDescriptor {
    pub id: StudyOutputId,
    pub name: String,
    pub kind: SeriesKind,
    pub pane: PaneId,
    pub price_scale: PriceScaleId,
    pub warmup_bars: usize,
    pub visible: bool,
    pub z_order: i32,
}

impl StudyOutputDescriptor {
    pub fn new(
        id: StudyOutputId,
        name: impl Into<String>,
        kind: SeriesKind,
        pane: PaneId,
        price_scale: PriceScaleId,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            pane,
            price_scale,
            warmup_bars: 0,
            visible: true,
            z_order: 100,
        }
    }

    pub fn line(
        id: StudyOutputId,
        name: impl Into<String>,
        pane: PaneId,
        price_scale: PriceScaleId,
    ) -> Self {
        Self::new(id, name, SeriesKind::Line, pane, price_scale)
    }

    pub fn with_warmup_bars(mut self, warmup_bars: usize) -> Self {
        self.warmup_bars = warmup_bars;
        self
    }

    pub fn with_visibility(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    pub fn with_z_order(mut self, z_order: i32) -> Self {
        self.z_order = z_order;
        self
    }

    pub fn series_id(&self) -> SeriesId {
        self.id.series_id()
    }

    pub fn to_series_spec(&self) -> SeriesSpec {
        SeriesSpec::new(
            self.series_id(),
            self.name.clone(),
            self.kind,
            SeriesValueSource::StudyOutput,
            self.pane,
            self.price_scale,
        )
        .with_visibility(self.visible)
        .with_z_order(self.z_order)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StudyOutputBuffer {
    descriptor: StudyOutputDescriptor,
    values: Vec<StudyValue>,
}

impl StudyOutputBuffer {
    pub fn new(descriptor: StudyOutputDescriptor) -> Self {
        Self {
            descriptor,
            values: Vec::new(),
        }
    }

    pub fn descriptor(&self) -> &StudyOutputDescriptor {
        &self.descriptor
    }

    pub fn series_id(&self) -> SeriesId {
        self.descriptor.series_id()
    }

    pub fn values(&self) -> &[StudyValue] {
        &self.values
    }

    pub fn value(&self, index: usize) -> Option<StudyValue> {
        self.values.get(index).copied()
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.values.clear();
    }

    pub(crate) fn push(&mut self, value: StudyValue) {
        self.values.push(value.sanitized());
    }
}
