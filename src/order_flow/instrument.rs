use std::path::PathBuf;

use super::{cache::MboCacheEntry, derived_cache::DerivedCacheSourceManifest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookmapInstrumentSelectionState {
    RegistryContract,
    TimelineReplay,
    DerivedCatalogFallback,
    Unavailable,
}

impl BookmapInstrumentSelectionState {
    pub fn label(self) -> &'static str {
        match self {
            Self::RegistryContract => "registry contract",
            Self::TimelineReplay => "timeline replay",
            Self::DerivedCatalogFallback => "derived catalog fallback",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmapInstrumentSelection {
    pub source_key: String,
    pub dataset: String,
    pub symbol: String,
    pub schema: String,
    pub stype_in: String,
    pub stype_out: String,
    pub range_label: String,
    pub raw_path: PathBuf,
    pub instrument_id: Option<u32>,
    pub available_instrument_ids: Vec<u32>,
    pub state: BookmapInstrumentSelectionState,
}

impl BookmapInstrumentSelection {
    pub fn from_cache_entry(entry: &MboCacheEntry, range_label: impl Into<String>) -> Self {
        Self {
            source_key: entry.source_key(),
            dataset: entry.dataset.clone(),
            symbol: entry.symbol.clone(),
            schema: entry.schema.clone(),
            stype_in: entry.stype_in.clone(),
            stype_out: entry.stype_out.clone(),
            range_label: range_label.into(),
            raw_path: entry.raw_path.clone(),
            instrument_id: None,
            available_instrument_ids: Vec::new(),
            state: BookmapInstrumentSelectionState::RegistryContract,
        }
    }

    pub fn from_derived_source_manifest(
        source: &DerivedCacheSourceManifest,
        range_label: impl Into<String>,
    ) -> Self {
        Self {
            source_key: source.source_key(),
            dataset: source.dataset.clone(),
            symbol: source.symbol.clone(),
            schema: source.schema.clone(),
            stype_in: source.stype_in.clone(),
            stype_out: source.stype_out.clone(),
            range_label: range_label.into(),
            raw_path: source.raw_path.clone(),
            instrument_id: None,
            available_instrument_ids: Vec::new(),
            state: BookmapInstrumentSelectionState::RegistryContract,
        }
    }

    pub fn with_timeline_instrument(mut self, instrument_id: Option<u32>) -> Self {
        self.instrument_id = instrument_id;
        self.available_instrument_ids = instrument_id.into_iter().collect();
        self.state = if instrument_id.is_some() {
            BookmapInstrumentSelectionState::TimelineReplay
        } else {
            BookmapInstrumentSelectionState::Unavailable
        };
        self
    }

    pub fn with_derived_catalog(mut self, mut instrument_ids: Vec<u32>) -> Self {
        instrument_ids.sort_unstable();
        instrument_ids.dedup();
        self.instrument_id = instrument_ids.first().copied();
        self.available_instrument_ids = instrument_ids;
        self.state = if self.instrument_id.is_some() {
            BookmapInstrumentSelectionState::DerivedCatalogFallback
        } else {
            BookmapInstrumentSelectionState::Unavailable
        };
        self
    }

    pub fn with_derived_catalog_preferred(
        mut self,
        mut instrument_ids: Vec<u32>,
        preferred_instrument_id: Option<u32>,
    ) -> Self {
        instrument_ids.sort_unstable();
        instrument_ids.dedup();
        self.instrument_id = preferred_instrument_id
            .filter(|instrument_id| instrument_ids.contains(instrument_id))
            .or_else(|| instrument_ids.first().copied());
        self.available_instrument_ids = instrument_ids;
        self.state = if self.instrument_id.is_some() {
            BookmapInstrumentSelectionState::DerivedCatalogFallback
        } else {
            BookmapInstrumentSelectionState::Unavailable
        };
        self
    }

    pub fn source_contract_label(&self) -> String {
        format!(
            "{} / {} / {} / {}->{}",
            self.dataset, self.symbol, self.schema, self.stype_in, self.stype_out
        )
    }

    pub fn instrument_label(&self) -> String {
        self.instrument_id
            .map(|instrument_id| format!("instrument_id {instrument_id}"))
            .unwrap_or_else(|| "instrument unavailable".to_string())
    }

    pub fn selection_label(&self) -> String {
        let available_count = self.available_instrument_ids.len();
        if available_count > 1 {
            format!("{} / {} available IDs", self.state.label(), available_count)
        } else {
            self.state.label().to_string()
        }
    }

    pub fn available_ids_label(&self) -> Option<String> {
        if self.available_instrument_ids.len() <= 1 {
            return None;
        }
        Some(
            self.available_instrument_ids
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        )
    }

    pub fn chrome_label(&self) -> String {
        format!(
            "{}  {}  {}",
            self.source_contract_label(),
            self.instrument_label(),
            self.selection_label()
        )
    }

    pub fn query_instrument_id(&self) -> Result<u32, String> {
        self.instrument_id.ok_or_else(|| {
            format!(
                "No selected instrument is available for {}",
                self.source_contract_label()
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(symbol: &str) -> MboCacheEntry {
        MboCacheEntry {
            dataset: "GLBX.MDP3".to_string(),
            schema: "mbo".to_string(),
            symbol: symbol.to_string(),
            stype_in: "parent".to_string(),
            stype_out: "instrument_id".to_string(),
            start: Some("2024-11-29T00:00:00Z".to_string()),
            end_exclusive: Some("2024-12-03T00:00:00Z".to_string()),
            raw_path: PathBuf::from("sample.dbn.zst"),
            estimated_record_count: None,
            estimated_cost_usd: None,
            estimated_billable_size_bytes: None,
            bytes_on_disk: None,
        }
    }

    #[test]
    fn registry_selection_preserves_contract_identity() {
        let selection = BookmapInstrumentSelection::from_cache_entry(&entry("NQ.FUT"), "range");

        assert_eq!(
            selection.source_key,
            "GLBX.MDP3:NQ.FUT:mbo:parent:instrument_id:2024-11-29T00:00:00Z:2024-12-03T00:00:00Z"
        );
        assert_eq!(
            selection.source_contract_label(),
            "GLBX.MDP3 / NQ.FUT / mbo / parent->instrument_id"
        );
        assert_eq!(
            selection.state,
            BookmapInstrumentSelectionState::RegistryContract
        );
        assert!(selection.instrument_id.is_none());
    }

    #[test]
    fn derived_catalog_fallback_is_deterministic_and_visible() {
        let selection = BookmapInstrumentSelection::from_cache_entry(&entry("NQ.FUT"), "range")
            .with_derived_catalog(vec![9, 3, 9, 7]);

        assert_eq!(selection.instrument_id, Some(3));
        assert_eq!(selection.available_instrument_ids, vec![3, 7, 9]);
        assert_eq!(
            selection.state,
            BookmapInstrumentSelectionState::DerivedCatalogFallback
        );
        assert!(selection.selection_label().contains("3 available IDs"));
        assert_eq!(selection.query_instrument_id(), Ok(3));
    }

    #[test]
    fn derived_catalog_can_prefer_visible_runtime_instrument() {
        let selection = BookmapInstrumentSelection::from_cache_entry(&entry("NQ.FUT"), "range")
            .with_derived_catalog_preferred(vec![9, 3, 9, 7], Some(7));

        assert_eq!(selection.instrument_id, Some(7));
        assert_eq!(selection.available_instrument_ids, vec![3, 7, 9]);
        assert_eq!(
            selection.state,
            BookmapInstrumentSelectionState::DerivedCatalogFallback
        );
        assert_eq!(selection.query_instrument_id(), Ok(7));
    }
}
