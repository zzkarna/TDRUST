use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub enum MboManifestError {
    Io(io::Error),
    Json(serde_json::Error),
    MissingField(&'static str),
    WrongSchema(String),
}

impl From<io::Error> for MboManifestError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for MboManifestError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
pub struct MboCacheManifest {
    pub kind: Option<String>,
    pub dataset: Option<String>,
    pub schema: Option<String>,
    pub symbol: Option<String>,
    pub stype_in: Option<String>,
    pub stype_out: Option<String>,
    pub start: Option<String>,
    pub end_exclusive: Option<String>,
    pub combined_range_start: Option<String>,
    pub combined_range_end_exclusive: Option<String>,
    pub raw_path: Option<PathBuf>,
    pub complements_existing_raw_path: Option<PathBuf>,
    pub estimated_record_count: Option<u64>,
    pub estimated_cost_usd: Option<f64>,
    pub combined_estimated_cost_usd: Option<f64>,
    pub estimated_billable_size_bytes: Option<u64>,
    pub bytes_on_disk: Option<u64>,
}

impl MboCacheManifest {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, MboManifestError> {
        let value = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&value)?)
    }

    pub fn into_entry(self) -> Result<MboCacheEntry, MboManifestError> {
        let schema = self
            .schema
            .as_deref()
            .ok_or(MboManifestError::MissingField("schema"))?;
        if schema != "mbo" {
            return Err(MboManifestError::WrongSchema(schema.to_string()));
        }

        Ok(MboCacheEntry {
            dataset: self
                .dataset
                .ok_or(MboManifestError::MissingField("dataset"))?,
            schema: schema.to_string(),
            symbol: self
                .symbol
                .ok_or(MboManifestError::MissingField("symbol"))?,
            stype_in: self.stype_in.unwrap_or_else(|| "parent".to_string()),
            stype_out: self
                .stype_out
                .unwrap_or_else(|| "instrument_id".to_string()),
            start: self.start.or(self.combined_range_start),
            end_exclusive: self.end_exclusive.or(self.combined_range_end_exclusive),
            raw_path: self
                .raw_path
                .ok_or(MboManifestError::MissingField("raw_path"))?,
            estimated_record_count: self.estimated_record_count,
            estimated_cost_usd: self.estimated_cost_usd.or(self.combined_estimated_cost_usd),
            estimated_billable_size_bytes: self.estimated_billable_size_bytes,
            bytes_on_disk: self.bytes_on_disk,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MboCacheEntry {
    pub dataset: String,
    pub schema: String,
    pub symbol: String,
    pub stype_in: String,
    pub stype_out: String,
    pub start: Option<String>,
    pub end_exclusive: Option<String>,
    pub raw_path: PathBuf,
    pub estimated_record_count: Option<u64>,
    pub estimated_cost_usd: Option<f64>,
    pub estimated_billable_size_bytes: Option<u64>,
    pub bytes_on_disk: Option<u64>,
}

impl MboCacheEntry {
    pub fn source_key(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            self.dataset,
            self.symbol,
            self.schema,
            self.stype_in,
            self.stype_out,
            self.start.as_deref().unwrap_or("-"),
            self.end_exclusive.as_deref().unwrap_or("-")
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MboCacheRegistry {
    entries: Vec<MboCacheEntry>,
}

impl MboCacheRegistry {
    pub fn new(entries: Vec<MboCacheEntry>) -> Self {
        Self { entries }
    }

    pub fn from_manifest_dir(path: impl AsRef<Path>) -> Result<Self, MboManifestError> {
        let mut entries = Vec::new();

        for item in fs::read_dir(path)? {
            let item = item?;
            let path = item.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }

            let manifest = MboCacheManifest::from_path(&path)?;
            if manifest.schema.as_deref() == Some("mbo") && manifest.raw_path.is_some() {
                entries.push(manifest.into_entry()?);
            }
        }

        entries.sort_by(|left, right| {
            left.dataset
                .cmp(&right.dataset)
                .then_with(|| left.symbol.cmp(&right.symbol))
                .then_with(|| left.start.cmp(&right.start))
                .then_with(|| left.raw_path.cmp(&right.raw_path))
        });
        Ok(Self::new(entries))
    }

    pub fn entries(&self) -> &[MboCacheEntry] {
        &self.entries
    }

    pub fn for_symbol(&self, symbol: &str) -> Vec<&MboCacheEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.symbol == symbol)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{MboCacheManifest, MboManifestError};

    #[test]
    fn manifest_accepts_current_nq_download_shape() {
        let manifest: MboCacheManifest = serde_json::from_str(
            r#"{
                "bytes_on_disk": 22941221592,
                "combined_estimated_cost_usd": 118.9826,
                "combined_range_end_exclusive": "2025-03-06T00:00:00Z",
                "combined_range_start": "2024-11-29T00:00:00Z",
                "dataset": "GLBX.MDP3",
                "end_exclusive": "2025-03-06T00:00:00Z",
                "estimated_billable_size_bytes": 69781817336,
                "estimated_cost_usd": 116.9809,
                "estimated_record_count": 1246103881,
                "kind": "databento_plan_download",
                "raw_path": "E-mini/data/raw/NQ-FUT/mbo/sample.dbn.zst",
                "schema": "mbo",
                "start": "2024-12-03T00:00:00Z",
                "stype_in": "parent",
                "stype_out": "instrument_id",
                "symbol": "NQ.FUT"
            }"#,
        )
        .expect("manifest");

        let entry = manifest.into_entry().expect("mbo entry");

        assert_eq!(
            entry.source_key(),
            "GLBX.MDP3:NQ.FUT:mbo:parent:instrument_id:2024-12-03T00:00:00Z:2025-03-06T00:00:00Z"
        );
        assert_eq!(entry.estimated_record_count, Some(1_246_103_881));
        assert_eq!(entry.bytes_on_disk, Some(22_941_221_592));
    }

    #[test]
    fn manifest_rejects_non_mbo_schema() {
        let manifest: MboCacheManifest = serde_json::from_str(
            r#"{
                "dataset": "GLBX.MDP3",
                "schema": "trades",
                "symbol": "NQ.FUT",
                "raw_path": "sample.dbn.zst"
            }"#,
        )
        .expect("manifest");

        assert!(matches!(
            manifest.into_entry(),
            Err(MboManifestError::WrongSchema(schema)) if schema == "trades"
        ));
    }
}
