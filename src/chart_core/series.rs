use super::{pane::PaneId, price_scale::PriceScaleId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SeriesId(pub u32);

impl SeriesId {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }
}

pub const MAIN_PRICE_SERIES_ID: SeriesId = SeriesId::new(1);
pub const VOLUME_SERIES_ID: SeriesId = SeriesId::new(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SeriesKind {
    Candlestick,
    Bar,
    Line,
    Area,
    Baseline,
    Histogram,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SeriesValueSource {
    Ohlc,
    Open,
    High,
    Low,
    Close,
    Volume,
    StudyOutput,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeriesSpec {
    pub id: SeriesId,
    pub name: String,
    pub kind: SeriesKind,
    pub value_source: SeriesValueSource,
    pub pane: PaneId,
    pub price_scale: PriceScaleId,
    pub visible: bool,
    pub z_order: i32,
}

impl SeriesSpec {
    pub fn new(
        id: SeriesId,
        name: impl Into<String>,
        kind: SeriesKind,
        value_source: SeriesValueSource,
        pane: PaneId,
        price_scale: PriceScaleId,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            value_source,
            pane,
            price_scale,
            visible: true,
            z_order: 0,
        }
    }

    pub fn candlestick(id: SeriesId, name: impl Into<String>) -> Self {
        Self::new(
            id,
            name,
            SeriesKind::Candlestick,
            SeriesValueSource::Ohlc,
            PaneId::Main,
            PriceScaleId::Right,
        )
    }

    pub fn volume_histogram(id: SeriesId, name: impl Into<String>) -> Self {
        Self::new(
            id,
            name,
            SeriesKind::Histogram,
            SeriesValueSource::Volume,
            PaneId::Volume,
            PriceScaleId::Volume,
        )
    }

    pub fn with_pane(mut self, pane: PaneId) -> Self {
        self.pane = pane;
        self
    }

    pub fn with_price_scale(mut self, price_scale: PriceScaleId) -> Self {
        self.price_scale = price_scale;
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeriesRegistry {
    series: Vec<SeriesSpec>,
    generation: u64,
}

impl Default for SeriesRegistry {
    fn default() -> Self {
        Self::price_volume_default()
    }
}

impl SeriesRegistry {
    pub fn new(series: Vec<SeriesSpec>) -> Self {
        let mut registry = Self {
            series,
            generation: 0,
        };
        registry.sort_by_z_order();
        registry
    }

    pub fn price_volume_default() -> Self {
        Self::new(vec![main_price_series_spec(), volume_series_spec()])
    }

    pub fn series(&self) -> &[SeriesSpec] {
        &self.series
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn get(&self, id: SeriesId) -> Option<&SeriesSpec> {
        self.series.iter().find(|series| series.id == id)
    }

    pub fn is_visible(&self, id: SeriesId) -> bool {
        self.get(id).is_some_and(|series| series.visible)
    }

    pub fn visible(&self) -> impl Iterator<Item = &SeriesSpec> {
        self.series.iter().filter(|series| series.visible)
    }

    pub fn by_pane(&self, pane: PaneId) -> impl Iterator<Item = &SeriesSpec> {
        self.series.iter().filter(move |series| series.pane == pane)
    }

    pub fn visible_by_pane(&self, pane: PaneId) -> impl Iterator<Item = &SeriesSpec> {
        self.by_pane(pane).filter(|series| series.visible)
    }

    pub fn add_or_replace(&mut self, spec: SeriesSpec) {
        if let Some(series) = self.series.iter_mut().find(|series| series.id == spec.id) {
            *series = spec;
        } else {
            self.series.push(spec);
        }
        self.sort_by_z_order();
        self.bump_generation();
    }

    pub fn set_visible(&mut self, id: SeriesId, visible: bool) -> bool {
        let Some(series) = self.series.iter_mut().find(|series| series.id == id) else {
            return false;
        };
        if series.visible == visible {
            return false;
        }
        series.visible = visible;
        self.bump_generation();
        true
    }

    pub fn set_pane(&mut self, id: SeriesId, pane: PaneId) -> bool {
        let Some(series) = self.series.iter_mut().find(|series| series.id == id) else {
            return false;
        };
        if series.pane == pane {
            return false;
        }
        series.pane = pane;
        self.bump_generation();
        true
    }

    fn sort_by_z_order(&mut self) {
        self.series.sort_by_key(|series| series.z_order);
    }

    fn bump_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }
}

pub fn main_price_series_spec() -> SeriesSpec {
    SeriesSpec::candlestick(MAIN_PRICE_SERIES_ID, "Price").with_z_order(10)
}

pub fn volume_series_spec() -> SeriesSpec {
    SeriesSpec::volume_histogram(VOLUME_SERIES_ID, "Volume").with_z_order(20)
}

#[cfg(test)]
mod tests {
    use super::{
        MAIN_PRICE_SERIES_ID, SeriesId, SeriesKind, SeriesRegistry, SeriesSpec, SeriesValueSource,
        VOLUME_SERIES_ID, main_price_series_spec, volume_series_spec,
    };
    use crate::chart_core::{PaneId, PriceScaleId};

    #[test]
    fn default_registry_contains_price_and_volume_series() {
        let registry = SeriesRegistry::default();

        assert_eq!(registry.series().len(), 2);
        assert_eq!(
            registry.get(MAIN_PRICE_SERIES_ID),
            Some(&main_price_series_spec())
        );
        assert_eq!(registry.get(VOLUME_SERIES_ID), Some(&volume_series_spec()));
        assert_eq!(
            registry
                .visible_by_pane(PaneId::Main)
                .map(|series| series.id)
                .collect::<Vec<_>>(),
            vec![MAIN_PRICE_SERIES_ID]
        );
        assert_eq!(
            registry
                .visible_by_pane(PaneId::Volume)
                .map(|series| series.id)
                .collect::<Vec<_>>(),
            vec![VOLUME_SERIES_ID]
        );
    }

    #[test]
    fn every_render_kind_uses_the_common_series_spec() {
        let kinds = [
            SeriesKind::Candlestick,
            SeriesKind::Bar,
            SeriesKind::Line,
            SeriesKind::Area,
            SeriesKind::Baseline,
            SeriesKind::Histogram,
        ];

        for (index, kind) in kinds.into_iter().enumerate() {
            let source = if matches!(kind, SeriesKind::Candlestick | SeriesKind::Bar) {
                SeriesValueSource::Ohlc
            } else if kind == SeriesKind::Histogram {
                SeriesValueSource::Volume
            } else {
                SeriesValueSource::Close
            };
            let spec = SeriesSpec::new(
                SeriesId::new(index as u32 + 10),
                format!("{kind:?}"),
                kind,
                source,
                PaneId::Main,
                PriceScaleId::Right,
            );

            assert_eq!(spec.kind, kind);
            assert_eq!(spec.pane, PaneId::Main);
            assert_eq!(spec.price_scale, PriceScaleId::Right);
        }
    }

    #[test]
    fn registry_replaces_visibility_and_tracks_generation() {
        let mut registry = SeriesRegistry::default();
        let start_generation = registry.generation();

        assert!(registry.set_visible(VOLUME_SERIES_ID, false));
        assert!(!registry.is_visible(VOLUME_SERIES_ID));
        assert!(registry.generation() > start_generation);
        assert!(!registry.set_visible(VOLUME_SERIES_ID, false));

        registry.add_or_replace(
            SeriesSpec::new(
                VOLUME_SERIES_ID,
                "Vol",
                SeriesKind::Histogram,
                SeriesValueSource::Volume,
                PaneId::Volume,
                PriceScaleId::Volume,
            )
            .with_z_order(5),
        );

        assert_eq!(registry.get(VOLUME_SERIES_ID).unwrap().name, "Vol");
        assert!(registry.is_visible(VOLUME_SERIES_ID));
        assert_eq!(registry.series()[0].id, VOLUME_SERIES_ID);
    }

    #[test]
    fn registry_filters_by_pane_and_moves_series_between_panes() {
        let mut registry = SeriesRegistry::default();

        assert!(registry.set_pane(VOLUME_SERIES_ID, PaneId::Study(1)));

        assert!(registry.visible_by_pane(PaneId::Volume).next().is_none());
        assert_eq!(
            registry
                .visible_by_pane(PaneId::Study(1))
                .map(|series| series.id)
                .collect::<Vec<_>>(),
            vec![VOLUME_SERIES_ID]
        );
    }
}
