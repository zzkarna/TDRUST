use std::{
    cmp::Ordering,
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Serialize, de::DeserializeOwned};

use super::{
    cache::MboCacheEntry,
    dom::DomColumnSnapshot,
    flow::{AggressorSide, MboTradePrint},
    heatmap::{LiquidityCell, LiquidityCellKey},
    mbo::{FIXED_PRICE_SCALE, MboEventSource, MboPrice, MboSide},
    runtime::{OrderFlowReplayConfig, OrderFlowReplayRuntime, OrderFlowReplayState},
};

pub const DERIVED_CACHE_SCHEMA_VERSION: u32 = 1;
pub const DERIVED_CACHE_MANIFEST_KIND: &str = "rustchart_bookmap_derived_cache";
pub const DERIVED_CACHE_PROGRESS_KIND: &str = "rustchart_bookmap_derived_cache_progress";
pub const DERIVED_CACHE_PARTITION_KIND: &str = "rustchart_bookmap_derived_cache_partition";
pub const DERIVED_CACHE_SHARD_INDEX_KIND: &str = "rustchart_bookmap_derived_cache_shard_index";
const DERIVED_CACHE_ARTIFACT_SHARD_RECORD_LIMIT: usize = 250_000;
pub const SECOND_NS: u64 = 1_000_000_000;
pub const MINUTE_NS: u64 = 60 * SECOND_NS;
pub const HOUR_NS: u64 = 60 * MINUTE_NS;
pub const DAY_NS: u64 = 24 * HOUR_NS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DerivedInterval {
    pub label: &'static str,
    pub interval_ns: u64,
}

pub const BOOKMAP_CANDLE_INTERVALS: [DerivedInterval; 11] = [
    DerivedInterval {
        label: "1m",
        interval_ns: MINUTE_NS,
    },
    DerivedInterval {
        label: "3m",
        interval_ns: 3 * MINUTE_NS,
    },
    DerivedInterval {
        label: "5m",
        interval_ns: 5 * MINUTE_NS,
    },
    DerivedInterval {
        label: "15m",
        interval_ns: 15 * MINUTE_NS,
    },
    DerivedInterval {
        label: "1h",
        interval_ns: HOUR_NS,
    },
    DerivedInterval {
        label: "4h",
        interval_ns: 4 * HOUR_NS,
    },
    DerivedInterval {
        label: "1w",
        interval_ns: 7 * DAY_NS,
    },
    DerivedInterval {
        label: "1mo",
        interval_ns: 30 * DAY_NS,
    },
    DerivedInterval {
        label: "3mo",
        interval_ns: 90 * DAY_NS,
    },
    DerivedInterval {
        label: "6mo",
        interval_ns: 180 * DAY_NS,
    },
    DerivedInterval {
        label: "12mo",
        interval_ns: 365 * DAY_NS,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeatmapLodPlan {
    pub label: &'static str,
    pub bucket_ns: u64,
    pub max_price_levels: usize,
}

pub const BOOKMAP_HEATMAP_LODS: [HeatmapLodPlan; 5] = [
    HeatmapLodPlan {
        label: "event",
        bucket_ns: SECOND_NS / 10,
        max_price_levels: 80,
    },
    HeatmapLodPlan {
        label: "1s",
        bucket_ns: SECOND_NS,
        max_price_levels: 120,
    },
    HeatmapLodPlan {
        label: "10s",
        bucket_ns: 10 * SECOND_NS,
        max_price_levels: 180,
    },
    HeatmapLodPlan {
        label: "1m",
        bucket_ns: MINUTE_NS,
        max_price_levels: 240,
    },
    HeatmapLodPlan {
        label: "15m",
        bucket_ns: 15 * MINUTE_NS,
        max_price_levels: 360,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DerivedCachePlan {
    pub estimated_events: u64,
    pub checkpoint_every_events: u64,
    pub event_partitions: u64,
    pub candle_intervals: &'static [DerivedInterval],
    pub heatmap_lods: &'static [HeatmapLodPlan],
}

impl DerivedCachePlan {
    pub fn for_estimated_events(estimated_events: u64) -> Self {
        let checkpoint_every_events = if estimated_events >= 1_000_000_000 {
            1_000_000
        } else if estimated_events >= 100_000_000 {
            250_000
        } else {
            50_000
        };
        Self {
            estimated_events,
            checkpoint_every_events,
            event_partitions: estimated_events.saturating_add(checkpoint_every_events - 1)
                / checkpoint_every_events,
            candle_intervals: &BOOKMAP_CANDLE_INTERVALS,
            heatmap_lods: &BOOKMAP_HEATMAP_LODS,
        }
    }
}

#[derive(Debug)]
pub enum DerivedCacheManifestError {
    Io(io::Error),
    Json(serde_json::Error),
    WrongKind(String),
    UnsupportedSchemaVersion { found: u32, expected: u32 },
}

#[derive(Debug)]
pub enum DerivedCacheBuildError {
    Source(String),
    Manifest(DerivedCacheManifestError),
    MissingArtifact(DerivedCacheArtifactKind),
}

impl From<DerivedCacheManifestError> for DerivedCacheBuildError {
    fn from(value: DerivedCacheManifestError) -> Self {
        Self::Manifest(value)
    }
}

impl From<io::Error> for DerivedCacheManifestError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for DerivedCacheManifestError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DerivedCacheSourceManifest {
    pub dataset: String,
    pub schema: String,
    pub symbol: String,
    pub stype_in: String,
    pub stype_out: String,
    pub start: Option<String>,
    pub end_exclusive: Option<String>,
    pub raw_path: PathBuf,
    pub estimated_record_count: Option<u64>,
    pub estimated_billable_size_bytes: Option<u64>,
    pub bytes_on_disk: Option<u64>,
}

impl DerivedCacheSourceManifest {
    pub fn from_entry(entry: &MboCacheEntry) -> Self {
        Self {
            dataset: entry.dataset.clone(),
            schema: entry.schema.clone(),
            symbol: entry.symbol.clone(),
            stype_in: entry.stype_in.clone(),
            stype_out: entry.stype_out.clone(),
            start: entry.start.clone(),
            end_exclusive: entry.end_exclusive.clone(),
            raw_path: entry.raw_path.clone(),
            estimated_record_count: entry.estimated_record_count,
            estimated_billable_size_bytes: entry.estimated_billable_size_bytes,
            bytes_on_disk: entry.bytes_on_disk,
        }
    }

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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DerivedIntervalManifest {
    pub label: String,
    pub interval_ns: u64,
}

impl From<DerivedInterval> for DerivedIntervalManifest {
    fn from(value: DerivedInterval) -> Self {
        Self {
            label: value.label.to_string(),
            interval_ns: value.interval_ns,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HeatmapLodManifest {
    pub label: String,
    pub bucket_ns: u64,
    pub max_price_levels: usize,
}

impl From<HeatmapLodPlan> for HeatmapLodManifest {
    fn from(value: HeatmapLodPlan) -> Self {
        Self {
            label: value.label.to_string(),
            bucket_ns: value.bucket_ns,
            max_price_levels: value.max_price_levels,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DerivedCachePlanManifest {
    pub estimated_events: u64,
    pub checkpoint_every_events: u64,
    pub event_partitions: u64,
    pub candle_intervals: Vec<DerivedIntervalManifest>,
    pub heatmap_lods: Vec<HeatmapLodManifest>,
}

impl From<DerivedCachePlan> for DerivedCachePlanManifest {
    fn from(value: DerivedCachePlan) -> Self {
        Self {
            estimated_events: value.estimated_events,
            checkpoint_every_events: value.checkpoint_every_events,
            event_partitions: value.event_partitions,
            candle_intervals: value
                .candle_intervals
                .iter()
                .copied()
                .map(DerivedIntervalManifest::from)
                .collect(),
            heatmap_lods: value
                .heatmap_lods
                .iter()
                .copied()
                .map(HeatmapLodManifest::from)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DerivedCacheArtifactKind {
    ReplayCheckpoints,
    OhlcvBars,
    HeatmapTiles,
    FootprintCells,
    VolumeBubbles,
    BboTransitions,
    DomSnapshots,
    AddOnEvents,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DerivedCacheArtifactManifest {
    pub kind: DerivedCacheArtifactKind,
    pub label: String,
    pub relative_path: PathBuf,
}

impl DerivedCacheArtifactManifest {
    pub fn new(
        kind: DerivedCacheArtifactKind,
        label: impl Into<String>,
        relative_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            kind,
            label: label.into(),
            relative_path: relative_path.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DerivedCacheShardRecord {
    pub relative_path: PathBuf,
    pub record_count: usize,
    pub start_ts: Option<u64>,
    pub end_ts: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DerivedCacheShardIndex {
    pub kind: String,
    pub schema_version: u32,
    pub artifact_kind: DerivedCacheArtifactKind,
    pub interval_ns: Option<u64>,
    pub shards: Vec<DerivedCacheShardRecord>,
}

impl DerivedCacheShardIndex {
    fn new(
        artifact_kind: DerivedCacheArtifactKind,
        interval_ns: Option<u64>,
        shards: Vec<DerivedCacheShardRecord>,
    ) -> Self {
        Self {
            kind: DERIVED_CACHE_SHARD_INDEX_KIND.to_string(),
            schema_version: DERIVED_CACHE_SCHEMA_VERSION,
            artifact_kind,
            interval_ns,
            shards,
        }
    }

    fn validate(
        &self,
        artifact_kind: DerivedCacheArtifactKind,
    ) -> Result<(), DerivedCacheBuildError> {
        if self.kind != DERIVED_CACHE_SHARD_INDEX_KIND {
            return Err(DerivedCacheManifestError::WrongKind(self.kind.clone()).into());
        }
        if self.schema_version != DERIVED_CACHE_SCHEMA_VERSION {
            return Err(DerivedCacheManifestError::UnsupportedSchemaVersion {
                found: self.schema_version,
                expected: DERIVED_CACHE_SCHEMA_VERSION,
            }
            .into());
        }
        if self.artifact_kind != artifact_kind {
            return Err(DerivedCacheBuildError::MissingArtifact(artifact_kind));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedCacheArtifactStorageEntry {
    pub kind: DerivedCacheArtifactKind,
    pub relative_path: PathBuf,
    pub artifact_bytes: u64,
    pub shard_count: usize,
    pub shard_bytes: u64,
}

impl DerivedCacheArtifactStorageEntry {
    pub const fn total_bytes(&self) -> u64 {
        self.artifact_bytes.saturating_add(self.shard_bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedCacheArtifactStorageReport {
    pub entries: Vec<DerivedCacheArtifactStorageEntry>,
}

impl DerivedCacheArtifactStorageReport {
    pub fn artifact_count(&self) -> usize {
        self.entries.len()
    }

    pub fn shard_count(&self) -> usize {
        self.entries
            .iter()
            .map(|entry| entry.shard_count)
            .sum::<usize>()
    }

    pub fn total_bytes(&self) -> u64 {
        self.entries
            .iter()
            .map(DerivedCacheArtifactStorageEntry::total_bytes)
            .sum::<u64>()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DerivedCacheBuildSummary {
    pub events_read: u64,
    pub applied_book_events: u64,
    pub trade_prints: u64,
    pub first_event_ts: Option<u64>,
    pub last_event_ts: Option<u64>,
    pub instrument_count: usize,
    pub checkpoint_count: usize,
    pub ohlcv_bars: usize,
    pub footprint_cells: usize,
    pub heatmap_cells: usize,
    pub volume_bubbles: usize,
    pub bbo_transitions: usize,
    #[serde(default)]
    pub add_on_events: usize,
}

impl DerivedCacheBuildSummary {
    pub fn from_replay_state(
        state: &OrderFlowReplayState,
        first_event_ts: Option<u64>,
        applied_book_events: u64,
        trade_prints: u64,
        instrument_count: usize,
    ) -> Self {
        Self {
            events_read: state.event_count,
            applied_book_events,
            trade_prints,
            first_event_ts: first_event_ts.or(state.first_event_ts),
            last_event_ts: state.last_event_ts,
            instrument_count,
            checkpoint_count: state.checkpoints.checkpoints().len(),
            ohlcv_bars: state.ohlcv.bar_count(),
            footprint_cells: state.footprint_snapshot().cells.len(),
            heatmap_cells: state.liquidity_surface.cell_count(),
            volume_bubbles: state.volume_bubbles.bubbles().count(),
            bbo_transitions: state.bbo_path.transitions().len(),
            add_on_events: heuristic_add_on_events_from_records(
                &heatmap_tiles_artifact(state).cells,
                &volume_bubbles_artifact(state).bubbles,
                256,
            )
            .len(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DerivedCacheBuildConfig {
    pub replay: OrderFlowReplayConfig,
    pub batch_size: usize,
    pub max_events: Option<u64>,
}

impl DerivedCacheBuildConfig {
    pub fn for_entry(entry: &MboCacheEntry) -> Self {
        let plan =
            DerivedCachePlan::for_estimated_events(entry.estimated_record_count.unwrap_or(0));
        Self {
            replay: OrderFlowReplayConfig {
                checkpoint_every_events: plan.checkpoint_every_events,
                heatmap_interval_ns: SECOND_NS,
                heatmap_depth_limit: BOOKMAP_HEATMAP_LODS
                    .first()
                    .map(|lod| lod.max_price_levels)
                    .unwrap_or(80),
                ..OrderFlowReplayConfig::default()
            },
            batch_size: 50_000,
            max_events: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DerivedCacheManifest {
    pub kind: String,
    pub schema_version: u32,
    pub source: DerivedCacheSourceManifest,
    pub plan: DerivedCachePlanManifest,
    pub summary: DerivedCacheBuildSummary,
    pub artifacts: Vec<DerivedCacheArtifactManifest>,
}

impl DerivedCacheManifest {
    pub fn from_entry(entry: &MboCacheEntry, summary: DerivedCacheBuildSummary) -> Self {
        let estimated_events = entry.estimated_record_count.unwrap_or(summary.events_read);
        let plan = DerivedCachePlan::for_estimated_events(estimated_events);
        let source = DerivedCacheSourceManifest::from_entry(entry);
        let source_slug = source_slug(&source);

        Self {
            kind: DERIVED_CACHE_MANIFEST_KIND.to_string(),
            schema_version: DERIVED_CACHE_SCHEMA_VERSION,
            source,
            plan: DerivedCachePlanManifest::from(plan),
            summary,
            artifacts: planned_artifacts(&source_slug),
        }
    }

    pub fn validate_schema(&self) -> Result<(), DerivedCacheManifestError> {
        if self.kind != DERIVED_CACHE_MANIFEST_KIND {
            return Err(DerivedCacheManifestError::WrongKind(self.kind.clone()));
        }
        if self.schema_version != DERIVED_CACHE_SCHEMA_VERSION {
            return Err(DerivedCacheManifestError::UnsupportedSchemaVersion {
                found: self.schema_version,
                expected: DERIVED_CACHE_SCHEMA_VERSION,
            });
        }
        Ok(())
    }

    pub fn from_json(value: &str) -> Result<Self, DerivedCacheManifestError> {
        let manifest: Self = serde_json::from_str(value)?;
        manifest.validate_schema()?;
        Ok(manifest)
    }

    pub fn to_pretty_json(&self) -> Result<String, DerivedCacheManifestError> {
        self.validate_schema()?;
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        Ok(json)
    }

    pub fn read_from_path(path: impl AsRef<Path>) -> Result<Self, DerivedCacheManifestError> {
        let value = fs::read_to_string(path)?;
        Self::from_json(&value)
    }

    pub fn write_to_path_atomic(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<(), DerivedCacheManifestError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp_path = sibling_temp_path(path);
        fs::write(&temp_path, self.to_pretty_json()?)?;
        fs::rename(&temp_path, path)?;
        Ok(())
    }

    pub fn artifact(
        &self,
        kind: DerivedCacheArtifactKind,
    ) -> Option<&DerivedCacheArtifactManifest> {
        self.artifacts.iter().find(|artifact| artifact.kind == kind)
    }

    pub fn artifact_path(
        &self,
        root: impl AsRef<Path>,
        kind: DerivedCacheArtifactKind,
    ) -> Result<PathBuf, DerivedCacheBuildError> {
        self.artifact(kind)
            .map(|artifact| root.as_ref().join(&artifact.relative_path))
            .ok_or(DerivedCacheBuildError::MissingArtifact(kind))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReplayCheckpointRecord {
    pub event_index: u64,
    pub timestamp_ns: u64,
    pub instrument_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReplayCheckpointArtifact {
    pub checkpoint_every_events: u64,
    pub checkpoints: Vec<ReplayCheckpointRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OhlcvBarRecord {
    pub interval_ns: u64,
    pub interval_start_ns: u64,
    pub instrument_id: u32,
    pub open_fixed: i64,
    pub high_fixed: i64,
    pub low_fixed: i64,
    pub close_fixed: i64,
    pub volume: u64,
    pub buy_volume: u64,
    pub sell_volume: u64,
    pub unknown_volume: u64,
    pub trade_count: u64,
    pub fill_count: u64,
    pub delta: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OhlcvBarsArtifact {
    pub bars: Vec<OhlcvBarRecord>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HeatmapCellRecord {
    pub interval_start_ns: u64,
    pub instrument_id: u32,
    pub price_fixed: i64,
    pub side: String,
    pub displayed_size: u64,
    pub max_displayed_size: u64,
    pub samples: u64,
    pub intensity: f32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HeatmapTilesArtifact {
    pub cells: Vec<HeatmapCellRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FootprintCellRecord {
    pub interval_start_ns: u64,
    pub price_fixed: i64,
    pub buy_volume: u64,
    pub sell_volume: u64,
    pub unknown_volume: u64,
    pub total_volume: u64,
    pub delta: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FootprintCellsArtifact {
    pub interval_ns: u64,
    pub cells: Vec<FootprintCellRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VolumeBubbleRecord {
    pub interval_start_ns: u64,
    pub instrument_id: u32,
    pub price_fixed: i64,
    pub buy_volume: u64,
    pub sell_volume: u64,
    pub unknown_volume: u64,
    pub total_volume: u64,
    pub delta: i64,
    pub trade_count: u64,
    pub fill_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VolumeBubblesArtifact {
    pub interval_ns: u64,
    pub bubbles: Vec<VolumeBubbleRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PriceLevelRecord {
    pub price_fixed: i64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BboTransitionRecord {
    pub ts_event: u64,
    pub instrument_id: u32,
    pub bid: Option<PriceLevelRecord>,
    pub ask: Option<PriceLevelRecord>,
    pub spread: Option<f64>,
    pub mid: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BboTransitionsArtifact {
    pub transitions: Vec<BboTransitionRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DomLevelRecord {
    pub price_fixed: i64,
    pub bid_size: u64,
    pub ask_size: u64,
    pub total_size: u64,
    pub side_hint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DomSnapshotRecord {
    pub timestamp_ns: Option<u64>,
    pub instrument_id: u32,
    pub best_bid: Option<PriceLevelRecord>,
    pub best_ask: Option<PriceLevelRecord>,
    pub max_size: u64,
    pub levels: Vec<DomLevelRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DomSnapshotsArtifact {
    pub snapshots: Vec<DomSnapshotRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddOnEventsArtifact {
    pub events: Vec<AddOnEventRecord>,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DerivedCachePartitionArtifacts {
    pub replay_checkpoints: ReplayCheckpointArtifact,
    pub ohlcv_bars: OhlcvBarsArtifact,
    pub heatmap_tiles: HeatmapTilesArtifact,
    pub footprint_cells: FootprintCellsArtifact,
    pub volume_bubbles: VolumeBubblesArtifact,
    pub bbo_transitions: BboTransitionsArtifact,
    pub dom_snapshots: DomSnapshotsArtifact,
    pub addon_events: AddOnEventsArtifact,
}

impl DerivedCachePartitionArtifacts {
    fn is_empty(&self) -> bool {
        self.replay_checkpoints.checkpoints.is_empty()
            && self.ohlcv_bars.bars.is_empty()
            && self.heatmap_tiles.cells.is_empty()
            && self.footprint_cells.cells.is_empty()
            && self.volume_bubbles.bubbles.is_empty()
            && self.bbo_transitions.transitions.is_empty()
            && self.dom_snapshots.snapshots.is_empty()
            && self.addon_events.events.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddOnEventRecord {
    pub kind: String,
    pub timestamp_ns: u64,
    pub instrument_id: u32,
    pub price_fixed: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    pub confidence_bps: u16,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub display_label: String,
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeatmapViewportQuery {
    pub instrument_id: u32,
    pub start_ts: u64,
    pub end_ts: u64,
    pub min_price_fixed: Option<i64>,
    pub max_price_fixed: Option<i64>,
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OhlcvWindowQuery {
    pub interval_ns: u64,
    pub instrument_id: u32,
    pub start_ts: u64,
    pub end_ts: u64,
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactTimeWindowQuery {
    pub instrument_id: u32,
    pub start_ts: u64,
    pub end_ts: u64,
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactPriceTimeWindowQuery {
    pub instrument_id: u32,
    pub start_ts: u64,
    pub end_ts: u64,
    pub min_price_fixed: Option<i64>,
    pub max_price_fixed: Option<i64>,
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FootprintWindowQuery {
    pub start_ts: u64,
    pub end_ts: u64,
    pub min_price_fixed: Option<i64>,
    pub max_price_fixed: Option<i64>,
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DomSnapshotQuery {
    pub instrument_id: u32,
    pub at_or_before_ts: Option<u64>,
    pub min_price_fixed: Option<i64>,
    pub max_price_fixed: Option<i64>,
    pub snapshot_limit: usize,
    pub level_limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DerivedActivityWindow {
    pub instrument_id: u32,
    pub start_ts: u64,
    pub end_ts: u64,
    pub bubble_count: usize,
    pub total_volume: u64,
}

trait ShardableArtifact: Serialize + DeserializeOwned {
    type Record: Clone;

    fn records(&self) -> &[Self::Record];
    fn from_records(records: Vec<Self::Record>, index: &DerivedCacheShardIndex) -> Self;
    fn interval_ns(&self) -> Option<u64> {
        None
    }
    fn record_timestamp(record: &Self::Record) -> Option<u64>;
}

impl ShardableArtifact for OhlcvBarsArtifact {
    type Record = OhlcvBarRecord;

    fn records(&self) -> &[Self::Record] {
        &self.bars
    }

    fn from_records(records: Vec<Self::Record>, _index: &DerivedCacheShardIndex) -> Self {
        Self { bars: records }
    }

    fn record_timestamp(record: &Self::Record) -> Option<u64> {
        Some(record.interval_start_ns)
    }
}

impl ShardableArtifact for HeatmapTilesArtifact {
    type Record = HeatmapCellRecord;

    fn records(&self) -> &[Self::Record] {
        &self.cells
    }

    fn from_records(records: Vec<Self::Record>, _index: &DerivedCacheShardIndex) -> Self {
        Self { cells: records }
    }

    fn record_timestamp(record: &Self::Record) -> Option<u64> {
        Some(record.interval_start_ns)
    }
}

impl ShardableArtifact for FootprintCellsArtifact {
    type Record = FootprintCellRecord;

    fn records(&self) -> &[Self::Record] {
        &self.cells
    }

    fn from_records(records: Vec<Self::Record>, index: &DerivedCacheShardIndex) -> Self {
        Self {
            interval_ns: index.interval_ns.unwrap_or_default(),
            cells: records,
        }
    }

    fn interval_ns(&self) -> Option<u64> {
        Some(self.interval_ns)
    }

    fn record_timestamp(record: &Self::Record) -> Option<u64> {
        Some(record.interval_start_ns)
    }
}

impl ShardableArtifact for VolumeBubblesArtifact {
    type Record = VolumeBubbleRecord;

    fn records(&self) -> &[Self::Record] {
        &self.bubbles
    }

    fn from_records(records: Vec<Self::Record>, index: &DerivedCacheShardIndex) -> Self {
        Self {
            interval_ns: index.interval_ns.unwrap_or_default(),
            bubbles: records,
        }
    }

    fn interval_ns(&self) -> Option<u64> {
        Some(self.interval_ns)
    }

    fn record_timestamp(record: &Self::Record) -> Option<u64> {
        Some(record.interval_start_ns)
    }
}

impl ShardableArtifact for BboTransitionsArtifact {
    type Record = BboTransitionRecord;

    fn records(&self) -> &[Self::Record] {
        &self.transitions
    }

    fn from_records(records: Vec<Self::Record>, _index: &DerivedCacheShardIndex) -> Self {
        Self {
            transitions: records,
        }
    }

    fn record_timestamp(record: &Self::Record) -> Option<u64> {
        Some(record.ts_event)
    }
}

impl ShardableArtifact for DomSnapshotsArtifact {
    type Record = DomSnapshotRecord;

    fn records(&self) -> &[Self::Record] {
        &self.snapshots
    }

    fn from_records(records: Vec<Self::Record>, _index: &DerivedCacheShardIndex) -> Self {
        Self { snapshots: records }
    }

    fn record_timestamp(record: &Self::Record) -> Option<u64> {
        record.timestamp_ns
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DerivedCacheBuildStatus {
    Running,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DerivedCachePartitionRecord {
    pub partition_index: u64,
    pub start_event_index: u64,
    pub end_event_index: u64,
    pub events_read: u64,
    pub last_event_ts: Option<u64>,
    pub artifact_relative_path: PathBuf,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DerivedCacheProgressManifest {
    pub kind: String,
    pub schema_version: u32,
    pub source: DerivedCacheSourceManifest,
    pub plan: DerivedCachePlanManifest,
    pub status: DerivedCacheBuildStatus,
    pub partition_events: u64,
    pub processed_events: u64,
    pub first_event_ts: Option<u64>,
    pub last_event_ts: Option<u64>,
    pub completed_partitions: Vec<DerivedCachePartitionRecord>,
}

impl DerivedCacheProgressManifest {
    pub fn from_entry(entry: &MboCacheEntry, partition_events: u64) -> Self {
        let estimated_events = entry.estimated_record_count.unwrap_or(0);
        let plan = DerivedCachePlan::for_estimated_events(estimated_events);
        Self {
            kind: DERIVED_CACHE_PROGRESS_KIND.to_string(),
            schema_version: DERIVED_CACHE_SCHEMA_VERSION,
            source: DerivedCacheSourceManifest::from_entry(entry),
            plan: DerivedCachePlanManifest::from(plan),
            status: DerivedCacheBuildStatus::Running,
            partition_events: partition_events.max(1),
            processed_events: 0,
            first_event_ts: None,
            last_event_ts: None,
            completed_partitions: Vec::new(),
        }
    }

    pub fn validate_schema(&self) -> Result<(), DerivedCacheManifestError> {
        if self.kind != DERIVED_CACHE_PROGRESS_KIND {
            return Err(DerivedCacheManifestError::WrongKind(self.kind.clone()));
        }
        if self.schema_version != DERIVED_CACHE_SCHEMA_VERSION {
            return Err(DerivedCacheManifestError::UnsupportedSchemaVersion {
                found: self.schema_version,
                expected: DERIVED_CACHE_SCHEMA_VERSION,
            });
        }
        Ok(())
    }

    pub fn from_json(value: &str) -> Result<Self, DerivedCacheManifestError> {
        let manifest: Self = serde_json::from_str(value)?;
        manifest.validate_schema()?;
        Ok(manifest)
    }

    pub fn to_pretty_json(&self) -> Result<String, DerivedCacheManifestError> {
        self.validate_schema()?;
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        Ok(json)
    }

    pub fn read_from_path(path: impl AsRef<Path>) -> Result<Self, DerivedCacheManifestError> {
        let value = fs::read_to_string(path)?;
        Self::from_json(&value)
    }

    pub fn write_to_path_atomic(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<(), DerivedCacheManifestError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp_path = sibling_temp_path(path);
        fs::write(&temp_path, self.to_pretty_json()?)?;
        fs::rename(&temp_path, path)?;
        Ok(())
    }

    pub fn is_complete(&self) -> bool {
        self.status == DerivedCacheBuildStatus::Complete
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DerivedCachePartitionArtifact {
    pub kind: String,
    pub schema_version: u32,
    pub source_key: String,
    pub partition: DerivedCachePartitionRecord,
    pub summary: DerivedCacheBuildSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<DerivedCachePartitionArtifacts>,
}

impl DerivedCachePartitionArtifact {
    fn new(
        progress: &DerivedCacheProgressManifest,
        partition: DerivedCachePartitionRecord,
        summary: DerivedCacheBuildSummary,
    ) -> Self {
        Self {
            kind: DERIVED_CACHE_PARTITION_KIND.to_string(),
            schema_version: DERIVED_CACHE_SCHEMA_VERSION,
            source_key: progress.source.source_key(),
            partition,
            summary,
            artifacts: None,
        }
    }

    fn with_artifacts(mut self, artifacts: DerivedCachePartitionArtifacts) -> Self {
        self.artifacts = Some(artifacts);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedCacheResumableBuildReport {
    pub progress: DerivedCacheProgressManifest,
    pub replayed_events: u64,
    pub processed_new_events: u64,
    pub wrote_partitions: usize,
    pub source_exhausted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerivedCacheProgressEventKind {
    Started,
    ReplayedBatch,
    ProcessedBatch,
    PartitionWritten,
    SourceExhausted,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DerivedCacheProgressEvent {
    pub kind: DerivedCacheProgressEventKind,
    pub processed_events: u64,
    pub target_events: Option<u64>,
    pub replayed_events: u64,
    pub processed_new_events: u64,
    pub completed_partitions: usize,
    pub wrote_partitions: usize,
    pub latest_partition_index: Option<u64>,
    pub latest_partition_events: Option<u64>,
    pub latest_partition_bytes: Option<u64>,
    pub source_exhausted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WrittenProgressPartition {
    record: DerivedCachePartitionRecord,
    artifact_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerivedCacheFinalizationEventKind {
    Started,
    ArtifactStarted,
    ArtifactFinished,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DerivedCacheFinalizationEvent {
    pub kind: DerivedCacheFinalizationEventKind,
    pub artifact_kind: Option<DerivedCacheArtifactKind>,
    pub completed_artifacts: usize,
    pub total_artifacts: usize,
    pub records: Option<usize>,
    pub artifact_bytes: Option<u64>,
    pub shard_count: Option<usize>,
    pub shard_bytes: Option<u64>,
    pub completed_partitions: usize,
}

pub fn build_derived_cache_artifacts<S>(
    entry: &MboCacheEntry,
    source: S,
    output_root: impl AsRef<Path>,
    config: DerivedCacheBuildConfig,
) -> Result<DerivedCacheManifest, DerivedCacheBuildError>
where
    S: MboEventSource,
{
    let output_root = output_root.as_ref();
    let mut runtime = OrderFlowReplayRuntime::new(source, config.replay);
    let mut applied_book_events = 0_u64;
    let mut trade_prints = 0_u64;

    loop {
        let processed = runtime.state().event_count;
        let Some(next_batch_size) =
            next_batch_size(processed, config.max_events, config.batch_size)
        else {
            break;
        };
        let report = runtime
            .process_next_batch(next_batch_size)
            .map_err(DerivedCacheBuildError::Source)?;
        if report.read_events == 0 {
            break;
        }
        applied_book_events += report.applied_book_events as u64;
        trade_prints += report.trade_prints as u64;
    }

    let summary = DerivedCacheBuildSummary::from_replay_state(
        runtime.state(),
        runtime.state().first_event_ts,
        applied_book_events,
        trade_prints,
        runtime.state().engine.instrument_count(),
    );
    let manifest = DerivedCacheManifest::from_entry(entry, summary);
    write_artifacts(output_root, &manifest, runtime.state())?;
    manifest.write_to_path_atomic(derived_cache_manifest_path(output_root, &manifest.source))?;
    Ok(manifest)
}

pub fn build_derived_cache_resumable<S>(
    entry: &MboCacheEntry,
    source: S,
    output_root: impl AsRef<Path>,
    config: DerivedCacheBuildConfig,
) -> Result<DerivedCacheResumableBuildReport, DerivedCacheBuildError>
where
    S: MboEventSource,
{
    build_derived_cache_resumable_with_progress(entry, source, output_root, config, |_| {})
}

pub fn build_derived_cache_resumable_with_progress<S, F>(
    entry: &MboCacheEntry,
    source: S,
    output_root: impl AsRef<Path>,
    config: DerivedCacheBuildConfig,
    mut on_progress: F,
) -> Result<DerivedCacheResumableBuildReport, DerivedCacheBuildError>
where
    S: MboEventSource,
    F: FnMut(DerivedCacheProgressEvent),
{
    let output_root = output_root.as_ref();
    let source_manifest = DerivedCacheSourceManifest::from_entry(entry);
    let source_slug = source_slug(&source_manifest);
    let progress_path = progress_manifest_path(output_root, &source_slug);
    let mut progress = if progress_path.exists() {
        DerivedCacheProgressManifest::read_from_path(&progress_path)?
    } else {
        DerivedCacheProgressManifest::from_entry(entry, config.replay.checkpoint_every_events)
    };
    progress.validate_schema()?;
    let target_events = config.max_events.or(entry.estimated_record_count);

    on_progress(DerivedCacheProgressEvent {
        kind: DerivedCacheProgressEventKind::Started,
        processed_events: progress.processed_events,
        target_events,
        replayed_events: 0,
        processed_new_events: 0,
        completed_partitions: progress.completed_partitions.len(),
        wrote_partitions: 0,
        latest_partition_index: None,
        latest_partition_events: None,
        latest_partition_bytes: None,
        source_exhausted: progress.is_complete(),
    });

    if progress.is_complete() {
        on_progress(DerivedCacheProgressEvent {
            kind: DerivedCacheProgressEventKind::Finished,
            processed_events: progress.processed_events,
            target_events,
            replayed_events: 0,
            processed_new_events: 0,
            completed_partitions: progress.completed_partitions.len(),
            wrote_partitions: 0,
            latest_partition_index: None,
            latest_partition_events: None,
            latest_partition_bytes: None,
            source_exhausted: true,
        });
        return Ok(DerivedCacheResumableBuildReport {
            progress,
            replayed_events: 0,
            processed_new_events: 0,
            wrote_partitions: 0,
            source_exhausted: true,
        });
    }

    let mut runtime = OrderFlowReplayRuntime::new(source, config.replay);
    let mut applied_book_events = 0_u64;
    let mut trade_prints = 0_u64;
    let mut replayed_events = 0_u64;
    let mut processed_new_events = 0_u64;
    let mut wrote_partitions = 0_usize;
    let mut source_exhausted = false;

    while runtime.state().event_count < progress.processed_events {
        let remaining = progress.processed_events - runtime.state().event_count;
        let batch_size = (remaining as usize).min(config.batch_size.max(1));
        let report = runtime
            .process_next_batch(batch_size)
            .map_err(DerivedCacheBuildError::Source)?;
        if report.read_events == 0 {
            return Err(DerivedCacheBuildError::Source(format!(
                "source exhausted while replaying {} completed progress events",
                progress.processed_events
            )));
        }
        replayed_events += report.read_events as u64;
        applied_book_events += report.applied_book_events as u64;
        trade_prints += report.trade_prints as u64;
        on_progress(DerivedCacheProgressEvent {
            kind: DerivedCacheProgressEventKind::ReplayedBatch,
            processed_events: runtime.state().event_count,
            target_events,
            replayed_events,
            processed_new_events,
            completed_partitions: progress.completed_partitions.len(),
            wrote_partitions,
            latest_partition_index: None,
            latest_partition_events: None,
            latest_partition_bytes: None,
            source_exhausted,
        });
    }

    loop {
        let processed = runtime.state().event_count;
        let Some(next_batch_size) = next_resumable_batch_size(processed, &progress, &config) else {
            break;
        };
        let report = runtime
            .process_next_batch(next_batch_size)
            .map_err(DerivedCacheBuildError::Source)?;
        if report.read_events == 0 {
            if runtime.state().event_count > progress.processed_events
                || has_pending_partition_records(runtime.state())
            {
                if let Some(written) = write_progress_partition(
                    output_root,
                    &source_slug,
                    &mut progress,
                    runtime.state_mut(),
                    applied_book_events,
                    trade_prints,
                    &progress_path,
                    PartitionFlushMode::All,
                )? {
                    wrote_partitions += 1;
                    on_progress(DerivedCacheProgressEvent {
                        kind: DerivedCacheProgressEventKind::PartitionWritten,
                        processed_events: runtime.state().event_count,
                        target_events,
                        replayed_events,
                        processed_new_events,
                        completed_partitions: progress.completed_partitions.len(),
                        wrote_partitions,
                        latest_partition_index: Some(written.record.partition_index),
                        latest_partition_events: Some(written.record.events_read),
                        latest_partition_bytes: Some(written.artifact_bytes),
                        source_exhausted,
                    });
                }
            }
            progress.status = DerivedCacheBuildStatus::Complete;
            progress.first_event_ts = progress.first_event_ts.or(runtime.state().first_event_ts);
            progress.last_event_ts = runtime.state().last_event_ts.or(progress.last_event_ts);
            progress.write_to_path_atomic(&progress_path)?;
            on_progress(DerivedCacheProgressEvent {
                kind: DerivedCacheProgressEventKind::SourceExhausted,
                processed_events: runtime.state().event_count,
                target_events,
                replayed_events,
                processed_new_events,
                completed_partitions: progress.completed_partitions.len(),
                wrote_partitions,
                latest_partition_index: None,
                latest_partition_events: None,
                latest_partition_bytes: None,
                source_exhausted: true,
            });
            let mut ignore_finalization_progress = |_| {};
            finalize_derived_cache_from_progress(
                entry,
                output_root,
                &progress,
                &mut ignore_finalization_progress,
            )?;
            source_exhausted = true;
            break;
        }
        processed_new_events += report.read_events as u64;
        applied_book_events += report.applied_book_events as u64;
        trade_prints += report.trade_prints as u64;
        on_progress(DerivedCacheProgressEvent {
            kind: DerivedCacheProgressEventKind::ProcessedBatch,
            processed_events: runtime.state().event_count,
            target_events,
            replayed_events,
            processed_new_events,
            completed_partitions: progress.completed_partitions.len(),
            wrote_partitions,
            latest_partition_index: None,
            latest_partition_events: None,
            latest_partition_bytes: None,
            source_exhausted,
        });

        if should_flush_progress_partition(runtime.state().event_count, &progress, &config) {
            let flush_mode = if config
                .max_events
                .is_some_and(|max_events| runtime.state().event_count >= max_events)
            {
                PartitionFlushMode::All
            } else {
                PartitionFlushMode::Ready
            };
            if let Some(written) = write_progress_partition(
                output_root,
                &source_slug,
                &mut progress,
                runtime.state_mut(),
                applied_book_events,
                trade_prints,
                &progress_path,
                flush_mode,
            )? {
                wrote_partitions += 1;
                on_progress(DerivedCacheProgressEvent {
                    kind: DerivedCacheProgressEventKind::PartitionWritten,
                    processed_events: runtime.state().event_count,
                    target_events,
                    replayed_events,
                    processed_new_events,
                    completed_partitions: progress.completed_partitions.len(),
                    wrote_partitions,
                    latest_partition_index: Some(written.record.partition_index),
                    latest_partition_events: Some(written.record.events_read),
                    latest_partition_bytes: Some(written.artifact_bytes),
                    source_exhausted,
                });
            }
        }
    }

    on_progress(DerivedCacheProgressEvent {
        kind: DerivedCacheProgressEventKind::Finished,
        processed_events: progress.processed_events,
        target_events,
        replayed_events,
        processed_new_events,
        completed_partitions: progress.completed_partitions.len(),
        wrote_partitions,
        latest_partition_index: None,
        latest_partition_events: None,
        latest_partition_bytes: None,
        source_exhausted,
    });

    Ok(DerivedCacheResumableBuildReport {
        progress,
        replayed_events,
        processed_new_events,
        wrote_partitions,
        source_exhausted,
    })
}

fn next_batch_size(processed: u64, max_events: Option<u64>, batch_size: usize) -> Option<usize> {
    let batch_size = batch_size.max(1);
    if let Some(max_events) = max_events {
        if processed >= max_events {
            return None;
        }
        Some(((max_events - processed) as usize).min(batch_size).max(1))
    } else {
        Some(batch_size)
    }
}

fn next_resumable_batch_size(
    processed: u64,
    progress: &DerivedCacheProgressManifest,
    config: &DerivedCacheBuildConfig,
) -> Option<usize> {
    if config
        .max_events
        .is_some_and(|max_events| processed >= max_events)
    {
        return None;
    }
    let batch_size = config.batch_size.max(1) as u64;
    let next_partition_end = progress
        .processed_events
        .saturating_add(progress.partition_events.max(1));
    let mut target = next_partition_end.max(processed.saturating_add(1));
    if let Some(max_events) = config.max_events {
        target = target.min(max_events);
    }
    if processed >= target {
        return None;
    }
    Some((target - processed).min(batch_size) as usize)
}

fn should_flush_progress_partition(
    processed: u64,
    progress: &DerivedCacheProgressManifest,
    config: &DerivedCacheBuildConfig,
) -> bool {
    if processed <= progress.processed_events {
        return false;
    }
    let next_partition_end = progress
        .processed_events
        .saturating_add(progress.partition_events.max(1));
    processed >= next_partition_end
        || config
            .max_events
            .is_some_and(|max_events| processed >= max_events)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartitionFlushMode {
    Ready,
    All,
}

impl PartitionFlushMode {
    const fn include_open_bucket(self) -> bool {
        matches!(self, Self::All)
    }
}

fn write_progress_partition(
    output_root: &Path,
    source_slug: &str,
    progress: &mut DerivedCacheProgressManifest,
    state: &mut OrderFlowReplayState,
    applied_book_events: u64,
    trade_prints: u64,
    progress_path: &Path,
    flush_mode: PartitionFlushMode,
) -> Result<Option<WrittenProgressPartition>, DerivedCacheBuildError> {
    let previous_processed_events = progress.processed_events;
    let end_event_index = state.event_count;
    let artifacts = take_partition_artifacts(state, flush_mode);
    if end_event_index == previous_processed_events && artifacts.is_empty() {
        return Ok(None);
    }

    let partition_index = progress.completed_partitions.len() as u64;
    let start_event_index = if end_event_index > previous_processed_events {
        previous_processed_events.saturating_add(1)
    } else {
        previous_processed_events
    };
    let relative_path = partition_relative_path(source_slug, partition_index);
    let partition = DerivedCachePartitionRecord {
        partition_index,
        start_event_index,
        end_event_index,
        events_read: end_event_index.saturating_sub(previous_processed_events),
        last_event_ts: state.last_event_ts,
        artifact_relative_path: relative_path,
        complete: true,
    };
    let summary = partition_summary_from_artifacts(
        state,
        applied_book_events,
        trade_prints,
        state.engine.instrument_count(),
        &artifacts,
    );
    let artifact = DerivedCachePartitionArtifact::new(progress, partition.clone(), summary)
        .with_artifacts(artifacts);
    let partition_path = output_root.join(&partition.artifact_relative_path);
    write_json_zstd_atomic(&partition_path, &artifact)?;
    let artifact_bytes = fs::metadata(&partition_path)
        .map_err(DerivedCacheManifestError::Io)?
        .len();

    progress.processed_events = end_event_index;
    progress.first_event_ts = progress.first_event_ts.or(state.first_event_ts);
    progress.last_event_ts = state.last_event_ts.or(progress.last_event_ts);
    progress.completed_partitions.push(partition);
    progress.write_to_path_atomic(progress_path)?;
    Ok(Some(WrittenProgressPartition {
        record: progress
            .completed_partitions
            .last()
            .expect("partition was just pushed")
            .clone(),
        artifact_bytes,
    }))
}

fn has_pending_partition_records(state: &OrderFlowReplayState) -> bool {
    state.checkpoints.checkpoint_count() > 0
        || state.ohlcv.bar_count() > 0
        || state.liquidity_surface.cell_count() > 0
        || state.footprint.cell_count() > 0
        || state.volume_bubbles.bubble_count() > 0
        || state.bbo_path.transition_count() > 0
}

fn take_partition_artifacts(
    state: &mut OrderFlowReplayState,
    flush_mode: PartitionFlushMode,
) -> DerivedCachePartitionArtifacts {
    let cutoff_ts = state.last_event_ts.unwrap_or_default();
    let include_open_bucket = flush_mode.include_open_bucket();
    let replay_checkpoints = replay_checkpoint_artifact_from_checkpoints(
        state.checkpoints.checkpoint_every_events(),
        state.checkpoints.take_checkpoints(),
    );
    let ohlcv_bars = ohlcv_bars_artifact_from_entries(
        state.ohlcv.drain_ready_bars(cutoff_ts, include_open_bucket),
    );
    let heatmap_tiles = heatmap_tiles_artifact_from_entries(
        state
            .liquidity_surface
            .drain_ready_cells(cutoff_ts, include_open_bucket),
    );
    let footprint_cells = footprint_cells_artifact_from_snapshot(
        state
            .footprint
            .drain_ready_cells(cutoff_ts, include_open_bucket),
    );
    let volume_bubbles = volume_bubbles_artifact_from_entries(
        state
            .volume_bubbles
            .drain_ready_bubbles(cutoff_ts, include_open_bucket),
        state.volume_bubbles.interval_ns(),
    );
    let bbo_transitions =
        bbo_transitions_artifact_from_transitions(state.bbo_path.take_transitions());
    let dom_snapshots = dom_snapshots_artifact(state);
    let addon_events =
        add_on_events_artifact_from_records(&heatmap_tiles.cells, &volume_bubbles.bubbles);
    state.flow.clear_prints();

    DerivedCachePartitionArtifacts {
        replay_checkpoints,
        ohlcv_bars,
        heatmap_tiles,
        footprint_cells,
        volume_bubbles,
        bbo_transitions,
        dom_snapshots,
        addon_events,
    }
}

fn partition_summary_from_artifacts(
    state: &OrderFlowReplayState,
    applied_book_events: u64,
    trade_prints: u64,
    instrument_count: usize,
    artifacts: &DerivedCachePartitionArtifacts,
) -> DerivedCacheBuildSummary {
    DerivedCacheBuildSummary {
        events_read: state.event_count,
        applied_book_events,
        trade_prints,
        first_event_ts: state.first_event_ts,
        last_event_ts: state.last_event_ts,
        instrument_count,
        checkpoint_count: artifacts.replay_checkpoints.checkpoints.len(),
        ohlcv_bars: artifacts.ohlcv_bars.bars.len(),
        footprint_cells: artifacts.footprint_cells.cells.len(),
        heatmap_cells: artifacts.heatmap_tiles.cells.len(),
        volume_bubbles: artifacts.volume_bubbles.bubbles.len(),
        bbo_transitions: artifacts.bbo_transitions.transitions.len(),
        add_on_events: artifacts.addon_events.events.len(),
    }
}

pub fn finalize_derived_cache_partitions(
    entry: &MboCacheEntry,
    output_root: impl AsRef<Path>,
) -> Result<DerivedCacheManifest, DerivedCacheBuildError> {
    finalize_derived_cache_partitions_with_progress(entry, output_root, |_| {})
}

pub fn finalize_derived_cache_partitions_with_progress<F>(
    entry: &MboCacheEntry,
    output_root: impl AsRef<Path>,
    mut on_progress: F,
) -> Result<DerivedCacheManifest, DerivedCacheBuildError>
where
    F: FnMut(DerivedCacheFinalizationEvent),
{
    let output_root = output_root.as_ref();
    let source_manifest = DerivedCacheSourceManifest::from_entry(entry);
    let source_slug = source_slug(&source_manifest);
    let progress_path = progress_manifest_path(output_root, &source_slug);
    let progress = DerivedCacheProgressManifest::read_from_path(&progress_path)?;
    progress.validate_schema()?;
    finalize_derived_cache_from_progress(entry, output_root, &progress, &mut on_progress)
}

fn finalize_derived_cache_from_progress<F>(
    entry: &MboCacheEntry,
    output_root: &Path,
    progress: &DerivedCacheProgressManifest,
    on_progress: &mut F,
) -> Result<DerivedCacheManifest, DerivedCacheBuildError>
where
    F: FnMut(DerivedCacheFinalizationEvent),
{
    let source_slug = source_slug(&progress.source);
    let partition_summaries = read_partition_summaries(output_root, progress)?;
    let last_summary = partition_summaries.last();
    let total_artifacts = planned_artifacts(&source_slug).len();
    let mut completed_artifacts = 0_usize;

    let mut summary = DerivedCacheBuildSummary {
        events_read: progress.processed_events,
        applied_book_events: last_summary
            .map(|summary| summary.applied_book_events)
            .unwrap_or_default(),
        trade_prints: last_summary
            .map(|summary| summary.trade_prints)
            .unwrap_or_default(),
        first_event_ts: progress.first_event_ts,
        last_event_ts: progress.last_event_ts,
        instrument_count: last_summary
            .map(|summary| summary.instrument_count)
            .unwrap_or_default(),
        checkpoint_count: 0,
        ohlcv_bars: 0,
        footprint_cells: 0,
        heatmap_cells: 0,
        volume_bubbles: 0,
        bbo_transitions: 0,
        add_on_events: 0,
    };
    let mut manifest = DerivedCacheManifest::from_entry(entry, summary.clone());
    manifest.source = progress.source.clone();
    manifest.artifacts = planned_artifacts(&source_slug);

    emit_finalization_event(
        on_progress,
        DerivedCacheFinalizationEventKind::Started,
        None,
        completed_artifacts,
        total_artifacts,
        None,
        None,
        progress.completed_partitions.len(),
    );

    emit_finalization_event(
        on_progress,
        DerivedCacheFinalizationEventKind::ArtifactStarted,
        Some(DerivedCacheArtifactKind::ReplayCheckpoints),
        completed_artifacts,
        total_artifacts,
        None,
        None,
        progress.completed_partitions.len(),
    );
    summary.checkpoint_count =
        write_partitioned_replay_checkpoints(&manifest, output_root, progress)?;
    completed_artifacts += 1;
    emit_finalized_artifact(
        on_progress,
        &manifest,
        output_root,
        DerivedCacheArtifactKind::ReplayCheckpoints,
        completed_artifacts,
        total_artifacts,
        summary.checkpoint_count,
        progress.completed_partitions.len(),
    )?;

    emit_finalization_event(
        on_progress,
        DerivedCacheFinalizationEventKind::ArtifactStarted,
        Some(DerivedCacheArtifactKind::OhlcvBars),
        completed_artifacts,
        total_artifacts,
        None,
        None,
        progress.completed_partitions.len(),
    );
    summary.ohlcv_bars = write_partitioned_shardable_artifact(
        &manifest,
        output_root,
        progress,
        DerivedCacheArtifactKind::OhlcvBars,
        |artifacts| &artifacts.ohlcv_bars,
        OhlcvBarsArtifact { bars: Vec::new() },
    )?;
    completed_artifacts += 1;
    emit_finalized_artifact(
        on_progress,
        &manifest,
        output_root,
        DerivedCacheArtifactKind::OhlcvBars,
        completed_artifacts,
        total_artifacts,
        summary.ohlcv_bars,
        progress.completed_partitions.len(),
    )?;

    emit_finalization_event(
        on_progress,
        DerivedCacheFinalizationEventKind::ArtifactStarted,
        Some(DerivedCacheArtifactKind::HeatmapTiles),
        completed_artifacts,
        total_artifacts,
        None,
        None,
        progress.completed_partitions.len(),
    );
    summary.heatmap_cells = write_partitioned_shardable_artifact(
        &manifest,
        output_root,
        progress,
        DerivedCacheArtifactKind::HeatmapTiles,
        |artifacts| &artifacts.heatmap_tiles,
        HeatmapTilesArtifact { cells: Vec::new() },
    )?;
    completed_artifacts += 1;
    emit_finalized_artifact(
        on_progress,
        &manifest,
        output_root,
        DerivedCacheArtifactKind::HeatmapTiles,
        completed_artifacts,
        total_artifacts,
        summary.heatmap_cells,
        progress.completed_partitions.len(),
    )?;

    emit_finalization_event(
        on_progress,
        DerivedCacheFinalizationEventKind::ArtifactStarted,
        Some(DerivedCacheArtifactKind::FootprintCells),
        completed_artifacts,
        total_artifacts,
        None,
        None,
        progress.completed_partitions.len(),
    );
    summary.footprint_cells = write_partitioned_shardable_artifact(
        &manifest,
        output_root,
        progress,
        DerivedCacheArtifactKind::FootprintCells,
        |artifacts| &artifacts.footprint_cells,
        FootprintCellsArtifact {
            interval_ns: progress
                .plan
                .candle_intervals
                .first()
                .map(|interval| interval.interval_ns)
                .unwrap_or(MINUTE_NS),
            cells: Vec::new(),
        },
    )?;
    completed_artifacts += 1;
    emit_finalized_artifact(
        on_progress,
        &manifest,
        output_root,
        DerivedCacheArtifactKind::FootprintCells,
        completed_artifacts,
        total_artifacts,
        summary.footprint_cells,
        progress.completed_partitions.len(),
    )?;

    emit_finalization_event(
        on_progress,
        DerivedCacheFinalizationEventKind::ArtifactStarted,
        Some(DerivedCacheArtifactKind::VolumeBubbles),
        completed_artifacts,
        total_artifacts,
        None,
        None,
        progress.completed_partitions.len(),
    );
    summary.volume_bubbles = write_partitioned_shardable_artifact(
        &manifest,
        output_root,
        progress,
        DerivedCacheArtifactKind::VolumeBubbles,
        |artifacts| &artifacts.volume_bubbles,
        VolumeBubblesArtifact {
            interval_ns: progress
                .plan
                .candle_intervals
                .first()
                .map(|interval| interval.interval_ns)
                .unwrap_or(MINUTE_NS),
            bubbles: Vec::new(),
        },
    )?;
    completed_artifacts += 1;
    emit_finalized_artifact(
        on_progress,
        &manifest,
        output_root,
        DerivedCacheArtifactKind::VolumeBubbles,
        completed_artifacts,
        total_artifacts,
        summary.volume_bubbles,
        progress.completed_partitions.len(),
    )?;

    emit_finalization_event(
        on_progress,
        DerivedCacheFinalizationEventKind::ArtifactStarted,
        Some(DerivedCacheArtifactKind::BboTransitions),
        completed_artifacts,
        total_artifacts,
        None,
        None,
        progress.completed_partitions.len(),
    );
    summary.bbo_transitions = write_partitioned_shardable_artifact(
        &manifest,
        output_root,
        progress,
        DerivedCacheArtifactKind::BboTransitions,
        |artifacts| &artifacts.bbo_transitions,
        BboTransitionsArtifact {
            transitions: Vec::new(),
        },
    )?;
    completed_artifacts += 1;
    emit_finalized_artifact(
        on_progress,
        &manifest,
        output_root,
        DerivedCacheArtifactKind::BboTransitions,
        completed_artifacts,
        total_artifacts,
        summary.bbo_transitions,
        progress.completed_partitions.len(),
    )?;

    emit_finalization_event(
        on_progress,
        DerivedCacheFinalizationEventKind::ArtifactStarted,
        Some(DerivedCacheArtifactKind::DomSnapshots),
        completed_artifacts,
        total_artifacts,
        None,
        None,
        progress.completed_partitions.len(),
    );
    let dom_snapshot_count = write_partitioned_shardable_artifact(
        &manifest,
        output_root,
        progress,
        DerivedCacheArtifactKind::DomSnapshots,
        |artifacts| &artifacts.dom_snapshots,
        DomSnapshotsArtifact {
            snapshots: Vec::new(),
        },
    )?;
    completed_artifacts += 1;
    emit_finalized_artifact(
        on_progress,
        &manifest,
        output_root,
        DerivedCacheArtifactKind::DomSnapshots,
        completed_artifacts,
        total_artifacts,
        dom_snapshot_count,
        progress.completed_partitions.len(),
    )?;

    emit_finalization_event(
        on_progress,
        DerivedCacheFinalizationEventKind::ArtifactStarted,
        Some(DerivedCacheArtifactKind::AddOnEvents),
        completed_artifacts,
        total_artifacts,
        None,
        None,
        progress.completed_partitions.len(),
    );
    summary.add_on_events = write_partitioned_add_on_events(&manifest, output_root, progress)?;
    completed_artifacts += 1;
    emit_finalized_artifact(
        on_progress,
        &manifest,
        output_root,
        DerivedCacheArtifactKind::AddOnEvents,
        completed_artifacts,
        total_artifacts,
        summary.add_on_events,
        progress.completed_partitions.len(),
    )?;

    manifest.summary = summary;
    manifest.write_to_path_atomic(derived_cache_manifest_path(output_root, &manifest.source))?;
    emit_finalization_event(
        on_progress,
        DerivedCacheFinalizationEventKind::Finished,
        None,
        completed_artifacts,
        total_artifacts,
        None,
        None,
        progress.completed_partitions.len(),
    );
    Ok(manifest)
}

fn emit_finalized_artifact<F>(
    on_progress: &mut F,
    manifest: &DerivedCacheManifest,
    output_root: &Path,
    kind: DerivedCacheArtifactKind,
    completed_artifacts: usize,
    total_artifacts: usize,
    records: usize,
    completed_partitions: usize,
) -> Result<(), DerivedCacheBuildError>
where
    F: FnMut(DerivedCacheFinalizationEvent),
{
    let storage = manifest
        .artifact(kind)
        .map(|artifact| derived_cache_artifact_storage_entry(manifest, output_root, artifact))
        .transpose()?;
    emit_finalization_event(
        on_progress,
        DerivedCacheFinalizationEventKind::ArtifactFinished,
        Some(kind),
        completed_artifacts,
        total_artifacts,
        Some(records),
        storage.as_ref(),
        completed_partitions,
    );
    Ok(())
}

fn emit_finalization_event<F>(
    on_progress: &mut F,
    kind: DerivedCacheFinalizationEventKind,
    artifact_kind: Option<DerivedCacheArtifactKind>,
    completed_artifacts: usize,
    total_artifacts: usize,
    records: Option<usize>,
    storage: Option<&DerivedCacheArtifactStorageEntry>,
    completed_partitions: usize,
) where
    F: FnMut(DerivedCacheFinalizationEvent),
{
    on_progress(DerivedCacheFinalizationEvent {
        kind,
        artifact_kind,
        completed_artifacts,
        total_artifacts,
        records,
        artifact_bytes: storage.map(|entry| entry.artifact_bytes),
        shard_count: storage.map(|entry| entry.shard_count),
        shard_bytes: storage.map(|entry| entry.shard_bytes),
        completed_partitions,
    });
}

fn read_partition_summaries(
    output_root: &Path,
    progress: &DerivedCacheProgressManifest,
) -> Result<Vec<DerivedCacheBuildSummary>, DerivedCacheBuildError> {
    progress
        .completed_partitions
        .iter()
        .map(|partition| {
            let artifact = read_partition_artifact(output_root, partition)?;
            Ok(artifact.summary)
        })
        .collect()
}

fn read_partition_artifact(
    output_root: &Path,
    partition: &DerivedCachePartitionRecord,
) -> Result<DerivedCachePartitionArtifact, DerivedCacheBuildError> {
    let value = read_json_or_zstd_to_string(&output_root.join(&partition.artifact_relative_path))?;
    serde_json::from_str(&value)
        .map_err(DerivedCacheManifestError::Json)
        .map_err(DerivedCacheBuildError::Manifest)
}

fn partition_artifacts<'a>(
    artifact: &'a DerivedCachePartitionArtifact,
    kind: DerivedCacheArtifactKind,
) -> Result<&'a DerivedCachePartitionArtifacts, DerivedCacheBuildError> {
    artifact.artifacts.as_ref().ok_or_else(|| {
        DerivedCacheBuildError::Source(format!(
            "partition {} does not contain {kind:?} records; rebuild the partition with the current derived-cache schema",
            artifact.partition.partition_index
        ))
    })
}

fn write_partitioned_replay_checkpoints(
    manifest: &DerivedCacheManifest,
    output_root: &Path,
    progress: &DerivedCacheProgressManifest,
) -> Result<usize, DerivedCacheBuildError> {
    let mut checkpoints = Vec::new();
    for partition in &progress.completed_partitions {
        let artifact = read_partition_artifact(output_root, partition)?;
        checkpoints.extend(
            partition_artifacts(&artifact, DerivedCacheArtifactKind::ReplayCheckpoints)?
                .replay_checkpoints
                .checkpoints
                .iter()
                .cloned(),
        );
    }
    checkpoints.sort_by_key(|checkpoint| checkpoint.event_index);
    let checkpoint_count = checkpoints.len();
    write_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::ReplayCheckpoints,
        &ReplayCheckpointArtifact {
            checkpoint_every_events: progress.plan.checkpoint_every_events,
            checkpoints,
        },
    )?;
    Ok(checkpoint_count)
}

fn write_partitioned_add_on_events(
    manifest: &DerivedCacheManifest,
    output_root: &Path,
    progress: &DerivedCacheProgressManifest,
) -> Result<usize, DerivedCacheBuildError> {
    let mut events = Vec::new();
    for partition in &progress.completed_partitions {
        let artifact = read_partition_artifact(output_root, partition)?;
        events.extend(
            partition_artifacts(&artifact, DerivedCacheArtifactKind::AddOnEvents)?
                .addon_events
                .events
                .iter()
                .cloned(),
        );
    }
    events.sort_by(|left, right| {
        left.timestamp_ns
            .cmp(&right.timestamp_ns)
            .then_with(|| left.instrument_id.cmp(&right.instrument_id))
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| right.confidence_bps.cmp(&left.confidence_bps))
    });
    let event_count = events.len();
    write_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::AddOnEvents,
        &AddOnEventsArtifact {
            events,
            note: "RustChart heuristic add-on stream from public MBO-derived partitions; not proprietary Bookmap algorithm parity".to_string(),
        },
    )?;
    Ok(event_count)
}

fn write_partitioned_shardable_artifact<T, F>(
    manifest: &DerivedCacheManifest,
    output_root: &Path,
    progress: &DerivedCacheProgressManifest,
    kind: DerivedCacheArtifactKind,
    select: F,
    empty_artifact: T,
) -> Result<usize, DerivedCacheBuildError>
where
    T: ShardableArtifact + Clone,
    F: Fn(&DerivedCachePartitionArtifacts) -> &T,
{
    let artifact_manifest = manifest
        .artifact(kind)
        .ok_or(DerivedCacheBuildError::MissingArtifact(kind))?;
    let index_path = manifest.artifact_path(output_root, kind)?;
    let artifact_parent = artifact_manifest
        .relative_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let stem = artifact_manifest
        .relative_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("artifact");
    let shard_dir = artifact_parent.join(format!("{stem}.shards"));
    let mut shards = Vec::new();
    let mut record_count = 0_usize;
    let mut single_partition_artifact = None;

    for partition in &progress.completed_partitions {
        let artifact = read_partition_artifact(output_root, partition)?;
        let partition_artifacts = partition_artifacts(&artifact, kind)?;
        let selected = select(partition_artifacts);
        if selected.records().is_empty() {
            continue;
        }
        record_count += selected.records().len();
        single_partition_artifact = Some(selected.clone());
        let shard_relative_path = shard_dir.join(format!(
            "{stem}-partition-{:06}.v1.json.zst",
            partition.partition_index
        ));
        write_json_zstd_atomic(&output_root.join(&shard_relative_path), selected)?;
        let (start_ts, end_ts) = shard_time_range::<T>(selected.records());
        shards.push(DerivedCacheShardRecord {
            relative_path: shard_relative_path,
            record_count: selected.records().len(),
            start_ts,
            end_ts,
        });
    }

    match shards.len() {
        0 => write_artifact(manifest, output_root, kind, &empty_artifact)?,
        1 => write_artifact(
            manifest,
            output_root,
            kind,
            &single_partition_artifact.unwrap_or(empty_artifact),
        )?,
        _ => {
            let shard_index =
                DerivedCacheShardIndex::new(kind, empty_artifact.interval_ns(), shards);
            if index_path.exists() {
                let _ = fs::remove_file(&index_path);
            }
            write_json_atomic(&index_path, &shard_index)?;
        }
    }
    Ok(record_count)
}

fn write_artifacts(
    output_root: &Path,
    manifest: &DerivedCacheManifest,
    state: &OrderFlowReplayState,
) -> Result<(), DerivedCacheBuildError> {
    write_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::ReplayCheckpoints,
        &replay_checkpoint_artifact(state),
    )?;
    write_shardable_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::OhlcvBars,
        &ohlcv_bars_artifact(state),
        DERIVED_CACHE_ARTIFACT_SHARD_RECORD_LIMIT,
    )?;
    write_shardable_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::HeatmapTiles,
        &heatmap_tiles_artifact(state),
        DERIVED_CACHE_ARTIFACT_SHARD_RECORD_LIMIT,
    )?;
    write_shardable_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::FootprintCells,
        &footprint_cells_artifact(state),
        DERIVED_CACHE_ARTIFACT_SHARD_RECORD_LIMIT,
    )?;
    write_shardable_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::VolumeBubbles,
        &volume_bubbles_artifact(state),
        DERIVED_CACHE_ARTIFACT_SHARD_RECORD_LIMIT,
    )?;
    write_shardable_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::BboTransitions,
        &bbo_transitions_artifact(state),
        DERIVED_CACHE_ARTIFACT_SHARD_RECORD_LIMIT,
    )?;
    write_shardable_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::DomSnapshots,
        &dom_snapshots_artifact(state),
        DERIVED_CACHE_ARTIFACT_SHARD_RECORD_LIMIT,
    )?;
    write_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::AddOnEvents,
        &add_on_events_artifact(state),
    )?;
    Ok(())
}

fn write_artifact<T>(
    manifest: &DerivedCacheManifest,
    output_root: &Path,
    kind: DerivedCacheArtifactKind,
    artifact: &T,
) -> Result<(), DerivedCacheBuildError>
where
    T: Serialize,
{
    let path = manifest.artifact_path(output_root, kind)?;
    write_json_atomic(&path, artifact)?;
    Ok(())
}

fn write_shardable_artifact<T>(
    manifest: &DerivedCacheManifest,
    output_root: &Path,
    kind: DerivedCacheArtifactKind,
    artifact: &T,
    shard_record_limit: usize,
) -> Result<(), DerivedCacheBuildError>
where
    T: ShardableArtifact,
{
    if shard_record_limit == 0 || artifact.records().len() <= shard_record_limit {
        return write_artifact(manifest, output_root, kind, artifact);
    }

    let artifact_manifest = manifest
        .artifact(kind)
        .ok_or(DerivedCacheBuildError::MissingArtifact(kind))?;
    let index_path = manifest.artifact_path(output_root, kind)?;
    let artifact_parent = artifact_manifest
        .relative_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let stem = artifact_manifest
        .relative_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("artifact");
    let shard_dir = artifact_parent.join(format!("{stem}.shards"));
    let mut shards = Vec::new();

    for (index, chunk) in artifact.records().chunks(shard_record_limit).enumerate() {
        let records = chunk.to_vec();
        let shard_artifact = T::from_records(
            records,
            &DerivedCacheShardIndex::new(kind, artifact.interval_ns(), Vec::new()),
        );
        let shard_relative_path = shard_dir.join(format!("{stem}-{index:06}.v1.json.zst"));
        let shard_path = output_root.join(&shard_relative_path);
        write_json_zstd_atomic(&shard_path, &shard_artifact)?;
        let (start_ts, end_ts) = shard_time_range::<T>(chunk);
        shards.push(DerivedCacheShardRecord {
            relative_path: shard_relative_path,
            record_count: chunk.len(),
            start_ts,
            end_ts,
        });
    }

    let shard_index = DerivedCacheShardIndex::new(kind, artifact.interval_ns(), shards);
    if index_path.exists() {
        let _ = fs::remove_file(&index_path);
    }
    write_json_atomic(&index_path, &shard_index)?;
    Ok(())
}

fn shard_time_range<T>(records: &[T::Record]) -> (Option<u64>, Option<u64>)
where
    T: ShardableArtifact,
{
    let mut min_ts = None;
    let mut max_ts = None;
    for timestamp in records.iter().filter_map(T::record_timestamp) {
        min_ts = Some(min_ts.map_or(timestamp, |current: u64| current.min(timestamp)));
        max_ts = Some(max_ts.map_or(timestamp, |current: u64| current.max(timestamp)));
    }
    (min_ts, max_ts)
}

pub fn read_cache_artifact<T>(
    manifest: &DerivedCacheManifest,
    output_root: impl AsRef<Path>,
    kind: DerivedCacheArtifactKind,
) -> Result<T, DerivedCacheBuildError>
where
    T: DeserializeOwned,
{
    let path = manifest.artifact_path(output_root, kind)?;
    let value = read_json_or_zstd_to_string(&path)?;
    serde_json::from_str(&value)
        .map_err(DerivedCacheManifestError::Json)
        .map_err(DerivedCacheBuildError::Manifest)
}

fn read_shardable_cache_artifact<T>(
    manifest: &DerivedCacheManifest,
    output_root: impl AsRef<Path>,
    kind: DerivedCacheArtifactKind,
    query_time_window: Option<(u64, u64)>,
) -> Result<T, DerivedCacheBuildError>
where
    T: ShardableArtifact,
{
    let output_root = output_root.as_ref();
    let path = manifest.artifact_path(output_root, kind)?;
    let value = read_json_or_zstd_to_string(&path)?;
    let json: serde_json::Value = serde_json::from_str(&value)
        .map_err(DerivedCacheManifestError::Json)
        .map_err(DerivedCacheBuildError::Manifest)?;
    if json
        .get("kind")
        .and_then(|kind| kind.as_str())
        .is_some_and(|value| value == DERIVED_CACHE_SHARD_INDEX_KIND)
    {
        let index: DerivedCacheShardIndex = serde_json::from_value(json)
            .map_err(DerivedCacheManifestError::Json)
            .map_err(DerivedCacheBuildError::Manifest)?;
        index.validate(kind)?;
        let mut records = Vec::new();
        for shard in index
            .shards
            .iter()
            .filter(|shard| shard_overlaps_window(shard, query_time_window))
        {
            let shard_path = output_root.join(&shard.relative_path);
            let shard_value = read_json_or_zstd_to_string(&shard_path)?;
            let shard_artifact: T = serde_json::from_str(&shard_value)
                .map_err(DerivedCacheManifestError::Json)
                .map_err(DerivedCacheBuildError::Manifest)?;
            records.extend_from_slice(shard_artifact.records());
        }
        Ok(T::from_records(records, &index))
    } else {
        serde_json::from_value(json)
            .map_err(DerivedCacheManifestError::Json)
            .map_err(DerivedCacheBuildError::Manifest)
    }
}

fn shard_overlaps_window(
    shard: &DerivedCacheShardRecord,
    query_time_window: Option<(u64, u64)>,
) -> bool {
    let Some((query_start, query_end)) = query_time_window else {
        return true;
    };
    match (shard.start_ts, shard.end_ts) {
        (Some(start_ts), Some(end_ts)) => end_ts >= query_start && start_ts <= query_end,
        _ => true,
    }
}

pub fn derived_cache_artifact_storage_report(
    manifest: &DerivedCacheManifest,
    output_root: impl AsRef<Path>,
) -> Result<DerivedCacheArtifactStorageReport, DerivedCacheBuildError> {
    let output_root = output_root.as_ref();
    let mut entries = Vec::with_capacity(manifest.artifacts.len());
    for artifact in &manifest.artifacts {
        entries.push(derived_cache_artifact_storage_entry(
            manifest,
            output_root,
            artifact,
        )?);
    }
    Ok(DerivedCacheArtifactStorageReport { entries })
}

fn derived_cache_artifact_storage_entry(
    _manifest: &DerivedCacheManifest,
    output_root: &Path,
    artifact: &DerivedCacheArtifactManifest,
) -> Result<DerivedCacheArtifactStorageEntry, DerivedCacheBuildError> {
    let path = output_root.join(&artifact.relative_path);
    let artifact_bytes = fs::metadata(&path)
        .map_err(DerivedCacheManifestError::Io)?
        .len();
    let value = fs::read_to_string(&path).map_err(DerivedCacheManifestError::Io)?;
    let json: serde_json::Value = serde_json::from_str(&value)
        .map_err(DerivedCacheManifestError::Json)
        .map_err(DerivedCacheBuildError::Manifest)?;
    let (shard_count, shard_bytes) = if json
        .get("kind")
        .and_then(|kind| kind.as_str())
        .is_some_and(|value| value == DERIVED_CACHE_SHARD_INDEX_KIND)
    {
        let index: DerivedCacheShardIndex = serde_json::from_value(json)
            .map_err(DerivedCacheManifestError::Json)
            .map_err(DerivedCacheBuildError::Manifest)?;
        index.validate(artifact.kind)?;
        let shard_bytes = index
            .shards
            .iter()
            .map(|shard| {
                fs::metadata(output_root.join(&shard.relative_path))
                    .map(|metadata| metadata.len())
                    .map_err(DerivedCacheManifestError::Io)
                    .map_err(DerivedCacheBuildError::Manifest)
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .sum::<u64>();
        (index.shards.len(), shard_bytes)
    } else {
        (0, 0)
    };
    Ok(DerivedCacheArtifactStorageEntry {
        kind: artifact.kind,
        relative_path: artifact.relative_path.clone(),
        artifact_bytes,
        shard_count,
        shard_bytes,
    })
}

pub fn query_heatmap_cells(
    manifest: &DerivedCacheManifest,
    output_root: impl AsRef<Path>,
    query: HeatmapViewportQuery,
) -> Result<Vec<HeatmapCellRecord>, DerivedCacheBuildError> {
    let artifact: HeatmapTilesArtifact = read_shardable_cache_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::HeatmapTiles,
        Some((query.start_ts, query.end_ts)),
    )?;
    let mut cells = artifact
        .cells
        .into_iter()
        .filter(|cell| {
            cell.instrument_id == query.instrument_id
                && cell.interval_start_ns >= query.start_ts
                && cell.interval_start_ns <= query.end_ts
                && query
                    .min_price_fixed
                    .is_none_or(|min_price| cell.price_fixed >= min_price)
                && query
                    .max_price_fixed
                    .is_none_or(|max_price| cell.price_fixed <= max_price)
        })
        .collect::<Vec<_>>();
    cells.sort_by(|left, right| {
        left.interval_start_ns
            .cmp(&right.interval_start_ns)
            .then_with(|| left.price_fixed.cmp(&right.price_fixed))
            .then_with(|| left.side.cmp(&right.side))
    });
    if query.limit > 0 && cells.len() > query.limit {
        cells.drain(0..cells.len() - query.limit);
    }
    Ok(cells)
}

pub fn query_ohlcv_bars(
    manifest: &DerivedCacheManifest,
    output_root: impl AsRef<Path>,
    query: OhlcvWindowQuery,
) -> Result<Vec<OhlcvBarRecord>, DerivedCacheBuildError> {
    let artifact: OhlcvBarsArtifact = read_shardable_cache_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::OhlcvBars,
        Some((query.start_ts, query.end_ts)),
    )?;
    let mut bars = artifact
        .bars
        .into_iter()
        .filter(|bar| {
            bar.interval_ns == query.interval_ns
                && bar.instrument_id == query.instrument_id
                && bar.interval_start_ns >= query.start_ts
                && bar.interval_start_ns <= query.end_ts
        })
        .collect::<Vec<_>>();
    bars.sort_by(|left, right| {
        left.interval_start_ns
            .cmp(&right.interval_start_ns)
            .then_with(|| left.instrument_id.cmp(&right.instrument_id))
    });
    trim_to_limit(&mut bars, query.limit);
    Ok(bars)
}

pub fn query_bbo_transitions(
    manifest: &DerivedCacheManifest,
    output_root: impl AsRef<Path>,
    query: ArtifactTimeWindowQuery,
) -> Result<Vec<BboTransitionRecord>, DerivedCacheBuildError> {
    let artifact: BboTransitionsArtifact = read_shardable_cache_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::BboTransitions,
        Some((query.start_ts, query.end_ts)),
    )?;
    let mut transitions = artifact
        .transitions
        .into_iter()
        .filter(|transition| {
            transition.instrument_id == query.instrument_id
                && transition.ts_event >= query.start_ts
                && transition.ts_event <= query.end_ts
        })
        .collect::<Vec<_>>();
    transitions.sort_by(|left, right| {
        left.ts_event
            .cmp(&right.ts_event)
            .then_with(|| left.instrument_id.cmp(&right.instrument_id))
    });
    trim_to_limit(&mut transitions, query.limit);
    Ok(transitions)
}

pub fn query_add_on_events(
    manifest: &DerivedCacheManifest,
    output_root: impl AsRef<Path>,
    query: ArtifactTimeWindowQuery,
) -> Result<Vec<AddOnEventRecord>, DerivedCacheBuildError> {
    let artifact: AddOnEventsArtifact =
        read_cache_artifact(manifest, output_root, DerivedCacheArtifactKind::AddOnEvents)?;
    let mut events = artifact
        .events
        .into_iter()
        .filter(|event| {
            event.instrument_id == query.instrument_id
                && event.timestamp_ns >= query.start_ts
                && event.timestamp_ns <= query.end_ts
        })
        .collect::<Vec<_>>();
    events.sort_by(|left, right| {
        left.timestamp_ns
            .cmp(&right.timestamp_ns)
            .then_with(|| left.instrument_id.cmp(&right.instrument_id))
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| right.confidence_bps.cmp(&left.confidence_bps))
    });
    trim_to_limit(&mut events, query.limit);
    Ok(events)
}

pub fn query_volume_bubbles(
    manifest: &DerivedCacheManifest,
    output_root: impl AsRef<Path>,
    query: ArtifactPriceTimeWindowQuery,
) -> Result<Vec<VolumeBubbleRecord>, DerivedCacheBuildError> {
    let artifact: VolumeBubblesArtifact = read_shardable_cache_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::VolumeBubbles,
        Some((query.start_ts, query.end_ts)),
    )?;
    let mut bubbles = artifact
        .bubbles
        .into_iter()
        .filter(|bubble| {
            bubble.instrument_id == query.instrument_id
                && bubble.interval_start_ns >= query.start_ts
                && bubble.interval_start_ns <= query.end_ts
                && query
                    .min_price_fixed
                    .is_none_or(|min_price| bubble.price_fixed >= min_price)
                && query
                    .max_price_fixed
                    .is_none_or(|max_price| bubble.price_fixed <= max_price)
        })
        .collect::<Vec<_>>();
    bubbles.sort_by(|left, right| {
        left.interval_start_ns
            .cmp(&right.interval_start_ns)
            .then_with(|| left.instrument_id.cmp(&right.instrument_id))
            .then_with(|| left.price_fixed.cmp(&right.price_fixed))
    });
    trim_to_limit(&mut bubbles, query.limit);
    Ok(bubbles)
}

pub fn query_volume_bubble_activity_window(
    manifest: &DerivedCacheManifest,
    output_root: impl AsRef<Path>,
    window_ns: u64,
) -> Result<Option<DerivedActivityWindow>, DerivedCacheBuildError> {
    let windows = volume_bubble_activity_windows(manifest, output_root, window_ns)?;
    Ok(windows.into_iter().max_by(compare_volume_activity_window))
}

pub fn query_bookmap_composite_activity_window(
    manifest: &DerivedCacheManifest,
    output_root: impl AsRef<Path>,
    window_ns: u64,
) -> Result<Option<DerivedActivityWindow>, DerivedCacheBuildError> {
    let output_root = output_root.as_ref();
    let windows = volume_bubble_activity_windows(manifest, output_root, window_ns)?;
    if windows.is_empty() {
        return Ok(None);
    }
    let bbo_index = read_derived_shard_index(
        manifest,
        output_root,
        DerivedCacheArtifactKind::BboTransitions,
    )?;
    let ohlcv_index =
        read_derived_shard_index(manifest, output_root, DerivedCacheArtifactKind::OhlcvBars)?;
    let heatmap_index = read_derived_shard_index(
        manifest,
        output_root,
        DerivedCacheArtifactKind::HeatmapTiles,
    )?;
    Ok(windows
        .into_iter()
        .map(|window| BookmapCompositeActivityCandidate {
            window,
            bbo_records: bbo_index
                .as_ref()
                .map(|index| shard_record_count_for_window(index, window.start_ts, window.end_ts))
                .unwrap_or_default(),
            ohlcv_records: ohlcv_index
                .as_ref()
                .map(|index| shard_record_count_for_window(index, window.start_ts, window.end_ts))
                .unwrap_or_default(),
            heatmap_records: heatmap_index
                .as_ref()
                .map(|index| shard_record_count_for_window(index, window.start_ts, window.end_ts))
                .unwrap_or_default(),
        })
        .max_by(compare_bookmap_composite_activity_candidate)
        .map(|candidate| candidate.window))
}

fn volume_bubble_activity_windows(
    manifest: &DerivedCacheManifest,
    output_root: impl AsRef<Path>,
    window_ns: u64,
) -> Result<Vec<DerivedActivityWindow>, DerivedCacheBuildError> {
    if window_ns == 0 {
        return Ok(Vec::new());
    }
    let artifact: VolumeBubblesArtifact = read_shardable_cache_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::VolumeBubbles,
        None,
    )?;
    let mut windows = BTreeMap::<(u32, u64), (usize, u64)>::new();
    for bubble in artifact.bubbles {
        let bucket_start = bucket_start(bubble.interval_start_ns, window_ns);
        let entry = windows
            .entry((bubble.instrument_id, bucket_start))
            .or_default();
        entry.0 += 1;
        entry.1 = entry.1.saturating_add(bubble.total_volume);
    }
    Ok(windows
        .into_iter()
        .map(
            |((instrument_id, start_ts), (bubble_count, total_volume))| DerivedActivityWindow {
                instrument_id,
                start_ts,
                end_ts: start_ts.saturating_add(window_ns),
                bubble_count,
                total_volume,
            },
        )
        .collect())
}

fn compare_volume_activity_window(
    left: &DerivedActivityWindow,
    right: &DerivedActivityWindow,
) -> Ordering {
    left.total_volume
        .cmp(&right.total_volume)
        .then_with(|| left.bubble_count.cmp(&right.bubble_count))
        .then_with(|| right.start_ts.cmp(&left.start_ts))
        .then_with(|| right.instrument_id.cmp(&left.instrument_id))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BookmapCompositeActivityCandidate {
    window: DerivedActivityWindow,
    bbo_records: usize,
    ohlcv_records: usize,
    heatmap_records: usize,
}

fn compare_bookmap_composite_activity_candidate(
    left: &BookmapCompositeActivityCandidate,
    right: &BookmapCompositeActivityCandidate,
) -> Ordering {
    bookmap_composite_activity_score(left)
        .total_cmp(&bookmap_composite_activity_score(right))
        .then_with(|| compare_volume_activity_window(&left.window, &right.window))
        .then_with(|| left.ohlcv_records.cmp(&right.ohlcv_records))
        .then_with(|| left.bbo_records.cmp(&right.bbo_records))
        .then_with(|| left.heatmap_records.cmp(&right.heatmap_records))
}

fn bookmap_composite_activity_score(candidate: &BookmapCompositeActivityCandidate) -> f64 {
    let volume = score_log(candidate.window.total_volume as f64);
    let bubbles = score_log(candidate.window.bubble_count as f64);
    let ohlcv = score_log(candidate.ohlcv_records as f64);
    let bbo = score_log(candidate.bbo_records as f64);
    let heatmap = score_log(candidate.heatmap_records as f64);
    volume * 2.0 + bubbles * 2.5 + ohlcv * 5.0 + bbo * 2.0 + heatmap
}

fn score_log(value: f64) -> f64 {
    (value.max(0.0) + 1.0).ln()
}

fn read_derived_shard_index(
    manifest: &DerivedCacheManifest,
    output_root: &Path,
    kind: DerivedCacheArtifactKind,
) -> Result<Option<DerivedCacheShardIndex>, DerivedCacheBuildError> {
    let path = manifest.artifact_path(output_root, kind)?;
    let value = read_json_or_zstd_to_string(&path)?;
    let json: serde_json::Value = serde_json::from_str(&value)
        .map_err(DerivedCacheManifestError::Json)
        .map_err(DerivedCacheBuildError::Manifest)?;
    if !json
        .get("kind")
        .and_then(|kind| kind.as_str())
        .is_some_and(|value| value == DERIVED_CACHE_SHARD_INDEX_KIND)
    {
        return Ok(None);
    }
    let index: DerivedCacheShardIndex = serde_json::from_value(json)
        .map_err(DerivedCacheManifestError::Json)
        .map_err(DerivedCacheBuildError::Manifest)?;
    index.validate(kind)?;
    Ok(Some(index))
}

fn shard_record_count_for_window(
    index: &DerivedCacheShardIndex,
    start_ts: u64,
    end_ts: u64,
) -> usize {
    index
        .shards
        .iter()
        .filter(|shard| shard_overlaps_window(shard, Some((start_ts, end_ts))))
        .map(|shard| shard.record_count)
        .sum()
}

pub fn query_footprint_cells(
    manifest: &DerivedCacheManifest,
    output_root: impl AsRef<Path>,
    query: FootprintWindowQuery,
) -> Result<Vec<FootprintCellRecord>, DerivedCacheBuildError> {
    let artifact: FootprintCellsArtifact = read_shardable_cache_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::FootprintCells,
        Some((query.start_ts, query.end_ts)),
    )?;
    let mut cells = artifact
        .cells
        .into_iter()
        .filter(|cell| {
            cell.interval_start_ns >= query.start_ts
                && cell.interval_start_ns <= query.end_ts
                && query
                    .min_price_fixed
                    .is_none_or(|min_price| cell.price_fixed >= min_price)
                && query
                    .max_price_fixed
                    .is_none_or(|max_price| cell.price_fixed <= max_price)
        })
        .collect::<Vec<_>>();
    cells.sort_by(|left, right| {
        left.interval_start_ns
            .cmp(&right.interval_start_ns)
            .then_with(|| left.price_fixed.cmp(&right.price_fixed))
    });
    trim_to_limit(&mut cells, query.limit);
    Ok(cells)
}

pub fn query_dom_snapshots(
    manifest: &DerivedCacheManifest,
    output_root: impl AsRef<Path>,
    query: DomSnapshotQuery,
) -> Result<Vec<DomSnapshotRecord>, DerivedCacheBuildError> {
    let artifact: DomSnapshotsArtifact = read_shardable_cache_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::DomSnapshots,
        query.at_or_before_ts.map(|end_ts| (0, end_ts)),
    )?;
    let mut snapshots = artifact
        .snapshots
        .into_iter()
        .filter(|snapshot| {
            snapshot.instrument_id == query.instrument_id
                && query.at_or_before_ts.is_none_or(|timestamp| {
                    snapshot
                        .timestamp_ns
                        .is_some_and(|snapshot_ts| snapshot_ts <= timestamp)
                })
        })
        .map(|mut snapshot| {
            snapshot.levels.retain(|level| {
                query
                    .min_price_fixed
                    .is_none_or(|min_price| level.price_fixed >= min_price)
                    && query
                        .max_price_fixed
                        .is_none_or(|max_price| level.price_fixed <= max_price)
            });
            if query.level_limit > 0 && snapshot.levels.len() > query.level_limit {
                snapshot.levels.truncate(query.level_limit);
            }
            snapshot.max_size = snapshot
                .levels
                .iter()
                .map(|level| level.total_size)
                .max()
                .unwrap_or_default();
            snapshot
        })
        .collect::<Vec<_>>();
    snapshots.sort_by(|left, right| {
        left.timestamp_ns
            .unwrap_or_default()
            .cmp(&right.timestamp_ns.unwrap_or_default())
            .then_with(|| left.instrument_id.cmp(&right.instrument_id))
    });
    trim_to_limit(&mut snapshots, query.snapshot_limit);
    Ok(snapshots)
}

pub fn query_dom_snapshot_instrument_ids(
    manifest: &DerivedCacheManifest,
    output_root: impl AsRef<Path>,
) -> Result<Vec<u32>, DerivedCacheBuildError> {
    let artifact: DomSnapshotsArtifact = read_shardable_cache_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::DomSnapshots,
        None,
    )?;
    let mut instrument_ids = artifact
        .snapshots
        .into_iter()
        .map(|snapshot| snapshot.instrument_id)
        .collect::<Vec<_>>();
    instrument_ids.sort_unstable();
    instrument_ids.dedup();
    Ok(instrument_ids)
}

pub fn query_latest_dom_snapshot_instrument_id(
    manifest: &DerivedCacheManifest,
    output_root: impl AsRef<Path>,
    at_or_before_ts: Option<u64>,
) -> Result<Option<u32>, DerivedCacheBuildError> {
    let artifact: DomSnapshotsArtifact = read_shardable_cache_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::DomSnapshots,
        at_or_before_ts.map(|end_ts| (0, end_ts)),
    )?;
    Ok(artifact
        .snapshots
        .into_iter()
        .filter(|snapshot| {
            at_or_before_ts.is_none_or(|timestamp| {
                snapshot
                    .timestamp_ns
                    .is_some_and(|snapshot_ts| snapshot_ts <= timestamp)
            })
        })
        .max_by_key(|snapshot| snapshot.timestamp_ns.unwrap_or_default())
        .map(|snapshot| snapshot.instrument_id))
}

pub fn replay_checkpoint_at_or_before_event(
    manifest: &DerivedCacheManifest,
    output_root: impl AsRef<Path>,
    event_index: u64,
) -> Result<Option<ReplayCheckpointRecord>, DerivedCacheBuildError> {
    let artifact: ReplayCheckpointArtifact = read_cache_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::ReplayCheckpoints,
    )?;
    Ok(artifact
        .checkpoints
        .into_iter()
        .filter(|checkpoint| checkpoint.event_index <= event_index)
        .max_by_key(|checkpoint| checkpoint.event_index))
}

pub fn replay_checkpoint_at_or_before_time(
    manifest: &DerivedCacheManifest,
    output_root: impl AsRef<Path>,
    timestamp_ns: u64,
) -> Result<Option<ReplayCheckpointRecord>, DerivedCacheBuildError> {
    let artifact: ReplayCheckpointArtifact = read_cache_artifact(
        manifest,
        output_root,
        DerivedCacheArtifactKind::ReplayCheckpoints,
    )?;
    Ok(artifact
        .checkpoints
        .into_iter()
        .filter(|checkpoint| checkpoint.timestamp_ns <= timestamp_ns)
        .max_by_key(|checkpoint| checkpoint.timestamp_ns))
}

fn trim_to_limit<T>(items: &mut Vec<T>, limit: usize) {
    if limit > 0 && items.len() > limit {
        items.drain(0..items.len() - limit);
    }
}

fn replay_checkpoint_artifact_from_checkpoints(
    checkpoint_every_events: u64,
    checkpoints: Vec<super::replay::ReplayCheckpoint>,
) -> ReplayCheckpointArtifact {
    ReplayCheckpointArtifact {
        checkpoint_every_events,
        checkpoints: checkpoints
            .into_iter()
            .map(|checkpoint| ReplayCheckpointRecord {
                event_index: checkpoint.event_index,
                timestamp_ns: checkpoint.timestamp_ns,
                instrument_count: checkpoint.engine.instrument_count(),
            })
            .collect(),
    }
}

fn replay_checkpoint_artifact(state: &OrderFlowReplayState) -> ReplayCheckpointArtifact {
    replay_checkpoint_artifact_from_checkpoints(
        state.checkpoints.checkpoint_every_events(),
        state.checkpoints.checkpoints().to_vec(),
    )
}

fn ohlcv_bars_artifact_from_entries(entries: Vec<(OhlcvBarKey, OhlcvBar)>) -> OhlcvBarsArtifact {
    let mut bars = entries
        .into_iter()
        .map(|(key, bar)| OhlcvBarRecord {
            interval_ns: key.interval_ns,
            interval_start_ns: key.interval_start_ns,
            instrument_id: key.instrument_id,
            open_fixed: bar.open.fixed(),
            high_fixed: bar.high.fixed(),
            low_fixed: bar.low.fixed(),
            close_fixed: bar.close.fixed(),
            volume: bar.volume,
            buy_volume: bar.buy_volume,
            sell_volume: bar.sell_volume,
            unknown_volume: bar.unknown_volume,
            trade_count: bar.trade_count,
            fill_count: bar.fill_count,
            delta: bar.delta,
        })
        .collect::<Vec<_>>();
    bars.sort_by(|left, right| {
        left.interval_ns
            .cmp(&right.interval_ns)
            .then_with(|| left.interval_start_ns.cmp(&right.interval_start_ns))
            .then_with(|| left.instrument_id.cmp(&right.instrument_id))
    });
    OhlcvBarsArtifact { bars }
}

fn ohlcv_bars_artifact(state: &OrderFlowReplayState) -> OhlcvBarsArtifact {
    let bars = BOOKMAP_CANDLE_INTERVALS
        .iter()
        .flat_map(|interval| state.ohlcv.bars_for_interval(interval.interval_ns))
        .collect::<Vec<_>>();
    ohlcv_bars_artifact_from_entries(bars)
}

fn heatmap_tiles_artifact_from_entries(
    entries: Vec<(LiquidityCellKey, LiquidityCell)>,
) -> HeatmapTilesArtifact {
    let mut cells = entries
        .into_iter()
        .map(|(key, cell)| heatmap_cell_record(key, cell))
        .collect::<Vec<_>>();
    cells.sort_by(|left, right| {
        left.interval_start_ns
            .cmp(&right.interval_start_ns)
            .then_with(|| left.instrument_id.cmp(&right.instrument_id))
            .then_with(|| left.price_fixed.cmp(&right.price_fixed))
            .then_with(|| left.side.cmp(&right.side))
    });
    HeatmapTilesArtifact { cells }
}

fn heatmap_tiles_artifact(state: &OrderFlowReplayState) -> HeatmapTilesArtifact {
    let cells = state.liquidity_surface.cells().collect::<Vec<_>>();
    heatmap_tiles_artifact_from_entries(cells)
}

fn heatmap_cell_record(key: LiquidityCellKey, cell: LiquidityCell) -> HeatmapCellRecord {
    HeatmapCellRecord {
        interval_start_ns: key.interval_start_ns,
        instrument_id: key.instrument_id,
        price_fixed: key.price.fixed(),
        side: side_label(key.side),
        displayed_size: cell.displayed_size,
        max_displayed_size: cell.max_displayed_size,
        samples: cell.samples,
        intensity: cell.intensity,
    }
}

fn footprint_cells_artifact_from_snapshot(
    snapshot: super::footprint::FootprintSnapshot,
) -> FootprintCellsArtifact {
    let mut cells = snapshot
        .cells
        .into_iter()
        .map(|(key, cell)| FootprintCellRecord {
            interval_start_ns: key.interval_start_ns,
            price_fixed: key.price.fixed(),
            buy_volume: cell.buy_volume,
            sell_volume: cell.sell_volume,
            unknown_volume: cell.unknown_volume,
            total_volume: cell.total_volume(),
            delta: cell.delta(),
        })
        .collect::<Vec<_>>();
    cells.sort_by(|left, right| {
        left.interval_start_ns
            .cmp(&right.interval_start_ns)
            .then_with(|| left.price_fixed.cmp(&right.price_fixed))
    });
    FootprintCellsArtifact {
        interval_ns: snapshot.interval_ns,
        cells,
    }
}

fn footprint_cells_artifact(state: &OrderFlowReplayState) -> FootprintCellsArtifact {
    footprint_cells_artifact_from_snapshot(state.footprint_snapshot())
}

fn volume_bubbles_artifact_from_entries(
    entries: Vec<(super::flow::VolumeBubbleKey, super::flow::VolumeBubble)>,
    interval_ns: u64,
) -> VolumeBubblesArtifact {
    let mut bubbles = entries
        .into_iter()
        .map(|(key, bubble)| VolumeBubbleRecord {
            interval_start_ns: key.interval_start_ns,
            instrument_id: key.instrument_id,
            price_fixed: key.price.fixed(),
            buy_volume: bubble.buy_volume,
            sell_volume: bubble.sell_volume,
            unknown_volume: bubble.unknown_volume,
            total_volume: bubble.total_volume(),
            delta: bubble.delta(),
            trade_count: bubble.trade_count,
            fill_count: bubble.fill_count,
        })
        .collect::<Vec<_>>();
    bubbles.sort_by(|left, right| {
        left.interval_start_ns
            .cmp(&right.interval_start_ns)
            .then_with(|| left.instrument_id.cmp(&right.instrument_id))
            .then_with(|| left.price_fixed.cmp(&right.price_fixed))
    });
    VolumeBubblesArtifact {
        interval_ns,
        bubbles,
    }
}

fn volume_bubbles_artifact(state: &OrderFlowReplayState) -> VolumeBubblesArtifact {
    let entries = state.volume_bubbles.bubbles().collect::<Vec<_>>();
    volume_bubbles_artifact_from_entries(entries, state.volume_bubbles.interval_ns())
}

fn add_on_events_artifact(state: &OrderFlowReplayState) -> AddOnEventsArtifact {
    let heatmap_tiles = heatmap_tiles_artifact(state);
    let volume_bubbles = volume_bubbles_artifact(state);
    add_on_events_artifact_from_records(&heatmap_tiles.cells, &volume_bubbles.bubbles)
}

fn add_on_events_artifact_from_records(
    heatmap_cells: &[HeatmapCellRecord],
    volume_bubbles: &[VolumeBubbleRecord],
) -> AddOnEventsArtifact {
    AddOnEventsArtifact {
        events: heuristic_add_on_events_from_records(heatmap_cells, volume_bubbles, 2_000),
        note: "RustChart heuristic add-on stream from public MBO-derived heatmap and volume-bubble records; not proprietary Bookmap algorithm parity".to_string(),
    }
}

pub fn heuristic_add_on_events_from_records(
    heatmap_cells: &[HeatmapCellRecord],
    volume_bubbles: &[VolumeBubbleRecord],
    limit: usize,
) -> Vec<AddOnEventRecord> {
    let max_volume = volume_bubbles
        .iter()
        .map(|bubble| bubble.total_volume)
        .max()
        .unwrap_or_default();
    let max_liquidity = heatmap_cells
        .iter()
        .map(|cell| cell.displayed_size)
        .max()
        .unwrap_or_default();
    if max_volume == 0 && max_liquidity == 0 {
        return Vec::new();
    }

    let volume_threshold = (max_volume / 2).max(1);
    let liquidity_threshold = liquidity_threshold_for_addons(heatmap_cells);
    let mut liquidity_by_instrument_price = BTreeMap::<(u32, i64), u64>::new();
    for cell in heatmap_cells {
        let key = (cell.instrument_id, cell.price_fixed);
        let current = liquidity_by_instrument_price.entry(key).or_default();
        *current = (*current).max(cell.displayed_size);
    }

    let mut events = Vec::new();
    for bubble in volume_bubbles {
        if bubble.total_volume < volume_threshold {
            continue;
        }
        let side = side_from_delta(bubble.delta);
        let imbalance = if bubble.total_volume == 0 {
            0.0
        } else {
            bubble.delta.unsigned_abs() as f32 / bubble.total_volume as f32
        };
        if imbalance >= 0.65 {
            events.push(add_on_event_record(
                "rustchart.heuristic.stop_run_like",
                bubble.interval_start_ns,
                bubble.instrument_id,
                Some(bubble.price_fixed),
                side,
                imbalance,
                "volume_bubbles",
                "Stop-run-like",
                format!(
                    "one-sided volume burst total={} delta={} trades={} fills={}",
                    bubble.total_volume, bubble.delta, bubble.trade_count, bubble.fill_count
                ),
            ));
        }
        if imbalance >= 0.85 {
            events.push(add_on_event_record(
                "rustchart.heuristic.sweep_like",
                bubble.interval_start_ns,
                bubble.instrument_id,
                Some(bubble.price_fixed),
                side,
                imbalance,
                "volume_bubbles",
                "Sweep-like",
                format!(
                    "high-imbalance sweep-like burst total={} buy={} sell={}",
                    bubble.total_volume, bubble.buy_volume, bubble.sell_volume
                ),
            ));
        }
        let displayed_size = liquidity_by_instrument_price
            .get(&(bubble.instrument_id, bubble.price_fixed))
            .copied()
            .unwrap_or_default();
        if displayed_size >= liquidity_threshold && displayed_size > 0 {
            let liquidity_score = if max_liquidity == 0 {
                0.0
            } else {
                displayed_size as f32 / max_liquidity as f32
            };
            let score =
                ((bubble.total_volume as f32 / max_volume.max(1) as f32) + liquidity_score) * 0.5;
            events.push(add_on_event_record(
                "rustchart.heuristic.iceberg_like",
                bubble.interval_start_ns,
                bubble.instrument_id,
                Some(bubble.price_fixed),
                side,
                score.clamp(0.0, 1.0),
                "heatmap_tiles+volume_bubbles",
                "Iceberg-like",
                format!(
                    "large volume into displayed liquidity total={} displayed_size={}",
                    bubble.total_volume, displayed_size
                ),
            ));
        }
    }

    for cell in heatmap_cells {
        if cell.displayed_size < liquidity_threshold || cell.displayed_size == 0 {
            continue;
        }
        let score = if max_liquidity == 0 {
            cell.intensity
        } else {
            (cell.displayed_size as f32 / max_liquidity as f32).max(cell.intensity)
        };
        events.push(add_on_event_record(
            "rustchart.heuristic.large_liquidity",
            cell.interval_start_ns,
            cell.instrument_id,
            Some(cell.price_fixed),
            Some(cell.side.as_str()),
            score.clamp(0.0, 1.0),
            "heatmap_tiles",
            "Large liquidity",
            format!(
                "displayed_size={} max_displayed_size={} samples={} intensity={:.2}",
                cell.displayed_size, cell.max_displayed_size, cell.samples, cell.intensity
            ),
        ));
    }

    let buy_volume = volume_bubbles
        .iter()
        .map(|bubble| bubble.buy_volume)
        .sum::<u64>();
    let sell_volume = volume_bubbles
        .iter()
        .map(|bubble| bubble.sell_volume)
        .sum::<u64>();
    let side_volume = buy_volume + sell_volume;
    if side_volume > 0 {
        let buy_pressure = buy_volume as f32 / side_volume as f32;
        let pressure_score = (buy_pressure - 0.5).abs() * 2.0;
        if pressure_score >= 0.15 {
            let anchor = volume_bubbles
                .iter()
                .max_by_key(|bubble| bubble.total_volume)
                .expect("side_volume implies at least one bubble");
            events.push(add_on_event_record(
                "rustchart.heuristic.market_pulse_volume_pressure",
                anchor.interval_start_ns,
                anchor.instrument_id,
                Some(anchor.price_fixed),
                if buy_pressure >= 0.5 {
                    Some("buy")
                } else {
                    Some("sell")
                },
                pressure_score.clamp(0.0, 1.0),
                "volume_bubbles",
                "Market pulse volume pressure",
                format!("buy_volume={buy_volume} sell_volume={sell_volume}"),
            ));
        }
    }

    events.sort_by(|left, right| {
        left.timestamp_ns
            .cmp(&right.timestamp_ns)
            .then_with(|| left.instrument_id.cmp(&right.instrument_id))
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| right.confidence_bps.cmp(&left.confidence_bps))
    });
    if limit == 0 {
        events.clear();
    } else if events.len() > limit {
        events.truncate(limit);
    }
    events
}

fn liquidity_threshold_for_addons(cells: &[HeatmapCellRecord]) -> u64 {
    let mut sizes = cells
        .iter()
        .map(|cell| cell.displayed_size)
        .filter(|size| *size > 0)
        .collect::<Vec<_>>();
    if sizes.is_empty() {
        return 1;
    }
    sizes.sort_unstable();
    let index = ((sizes.len() - 1) * 3) / 4;
    sizes[index].max(1)
}

fn side_from_delta(delta: i64) -> Option<&'static str> {
    if delta > 0 {
        Some("buy")
    } else if delta < 0 {
        Some("sell")
    } else {
        None
    }
}

fn confidence_bps(score: f32) -> u16 {
    (score.clamp(0.0, 1.0) * 10_000.0).round() as u16
}

fn add_on_event_record(
    kind: &str,
    timestamp_ns: u64,
    instrument_id: u32,
    price_fixed: Option<i64>,
    side: Option<&str>,
    score: f32,
    source: &str,
    display_label: &str,
    note: String,
) -> AddOnEventRecord {
    AddOnEventRecord {
        kind: kind.to_string(),
        timestamp_ns,
        instrument_id,
        price_fixed,
        side: side.map(str::to_string),
        confidence_bps: confidence_bps(score),
        source: source.to_string(),
        display_label: display_label.to_string(),
        note,
    }
}

fn bbo_transitions_artifact_from_transitions(
    transitions: Vec<super::bbo::BboTransition>,
) -> BboTransitionsArtifact {
    let transitions = transitions
        .into_iter()
        .map(|transition| BboTransitionRecord {
            ts_event: transition.ts_event,
            instrument_id: transition.instrument_id,
            bid: transition.bid.map(price_level_record),
            ask: transition.ask.map(price_level_record),
            spread: transition.spread,
            mid: transition.mid,
        })
        .collect();
    BboTransitionsArtifact { transitions }
}

fn bbo_transitions_artifact(state: &OrderFlowReplayState) -> BboTransitionsArtifact {
    bbo_transitions_artifact_from_transitions(state.bbo_path.transitions().to_vec())
}

fn dom_snapshots_artifact(state: &OrderFlowReplayState) -> DomSnapshotsArtifact {
    let snapshots = state
        .engine
        .books()
        .map(|book| DomColumnSnapshot::from_book(book, 20))
        .map(|snapshot| DomSnapshotRecord {
            timestamp_ns: state.last_event_ts,
            instrument_id: snapshot.instrument_id,
            best_bid: snapshot.best_bid.map(price_level_record),
            best_ask: snapshot.best_ask.map(price_level_record),
            max_size: snapshot.max_size,
            levels: snapshot
                .levels
                .iter()
                .map(|level| DomLevelRecord {
                    price_fixed: level.price.fixed(),
                    bid_size: level.bid_size,
                    ask_size: level.ask_size,
                    total_size: level.total_size,
                    side_hint: side_label(level.side_hint),
                })
                .collect(),
        })
        .collect();
    DomSnapshotsArtifact { snapshots }
}

fn price_level_record(level: super::book::PriceLevel) -> PriceLevelRecord {
    PriceLevelRecord {
        price_fixed: level.price.fixed(),
        size: level.size,
    }
}

fn side_label(side: MboSide) -> String {
    match side {
        MboSide::Ask => "ask".to_string(),
        MboSide::Bid => "bid".to_string(),
        MboSide::None => "none".to_string(),
        MboSide::Unknown(value) => format!("unknown-{value}"),
    }
}

fn write_json_atomic<T>(path: &Path, value: &T) -> Result<(), DerivedCacheManifestError>
where
    T: Serialize,
{
    let mut json = serde_json::to_vec_pretty(value)?;
    json.push(b'\n');
    write_bytes_atomic(path, &json)
}

fn write_json_zstd_atomic<T>(path: &Path, value: &T) -> Result<(), DerivedCacheManifestError>
where
    T: Serialize,
{
    let mut json = serde_json::to_vec(value)?;
    json.push(b'\n');
    let compressed =
        zstd::stream::encode_all(json.as_slice(), 3).map_err(DerivedCacheManifestError::Io)?;
    write_bytes_atomic(path, &compressed)
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<(), DerivedCacheManifestError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = sibling_temp_path(path);
    fs::write(&temp_path, bytes)?;
    fs::rename(&temp_path, path)?;
    Ok(())
}

fn read_json_or_zstd_to_string(path: &Path) -> Result<String, DerivedCacheBuildError> {
    let bytes = fs::read(path)
        .map_err(DerivedCacheManifestError::Io)
        .map_err(DerivedCacheBuildError::Manifest)?;
    if path.extension().and_then(|value| value.to_str()) == Some("zst") {
        let decoded = zstd::stream::decode_all(bytes.as_slice())
            .map_err(DerivedCacheManifestError::Io)
            .map_err(DerivedCacheBuildError::Manifest)?;
        String::from_utf8(decoded).map_err(|error| {
            DerivedCacheBuildError::Source(format!(
                "compressed JSON artifact {} was not valid UTF-8: {error}",
                path.display()
            ))
        })
    } else {
        String::from_utf8(bytes).map_err(|error| {
            DerivedCacheBuildError::Source(format!(
                "JSON artifact {} was not valid UTF-8: {error}",
                path.display()
            ))
        })
    }
}

pub fn planned_artifacts(source_slug: &str) -> Vec<DerivedCacheArtifactManifest> {
    [
        (
            DerivedCacheArtifactKind::ReplayCheckpoints,
            "Replay checkpoints",
            "replay-checkpoints.v1.json",
        ),
        (
            DerivedCacheArtifactKind::OhlcvBars,
            "Multi-resolution OHLCV bars",
            "ohlcv-bars.v1.json",
        ),
        (
            DerivedCacheArtifactKind::HeatmapTiles,
            "Heatmap/depth LOD tiles",
            "heatmap-tiles.v1.json",
        ),
        (
            DerivedCacheArtifactKind::FootprintCells,
            "Footprint cells",
            "footprint-cells.v1.json",
        ),
        (
            DerivedCacheArtifactKind::VolumeBubbles,
            "Volume bubbles",
            "volume-bubbles.v1.json",
        ),
        (
            DerivedCacheArtifactKind::BboTransitions,
            "BBO transitions",
            "bbo-transitions.v1.json",
        ),
        (
            DerivedCacheArtifactKind::DomSnapshots,
            "DOM snapshots",
            "dom-snapshots.v1.json",
        ),
        (
            DerivedCacheArtifactKind::AddOnEvents,
            "Stops, icebergs, filters, and pulse events",
            "addon-events.v1.json",
        ),
    ]
    .into_iter()
    .map(|(kind, label, file_name)| {
        DerivedCacheArtifactManifest::new(kind, label, PathBuf::from(source_slug).join(file_name))
    })
    .collect()
}

fn source_slug(source: &DerivedCacheSourceManifest) -> String {
    sanitize_slug(&format!(
        "{}-{}-{}-{}-{}-{}-{}",
        source.dataset,
        source.symbol,
        source.schema,
        source.stype_in,
        source.stype_out,
        source.start.as_deref().unwrap_or("open"),
        source.end_exclusive.as_deref().unwrap_or("open")
    ))
}

fn sanitize_slug(value: &str) -> String {
    let mut slug = String::with_capacity(value.len());
    let mut previous_dash = false;
    for character in value.chars() {
        let next = if character.is_ascii_alphanumeric() {
            character.to_ascii_lowercase()
        } else {
            '-'
        };
        if next == '-' {
            if !previous_dash {
                slug.push(next);
            }
            previous_dash = true;
        } else {
            slug.push(next);
            previous_dash = false;
        }
    }
    slug.trim_matches('-').to_string()
}

pub fn progress_manifest_path(output_root: impl AsRef<Path>, source_slug: &str) -> PathBuf {
    output_root
        .as_ref()
        .join(source_slug)
        .join("derived-cache-progress.v1.json")
}

pub fn derived_cache_manifest_path(
    output_root: impl AsRef<Path>,
    source: &DerivedCacheSourceManifest,
) -> PathBuf {
    output_root
        .as_ref()
        .join(source_slug(source))
        .join("derived-cache.v1.json")
}

pub fn derived_cache_manifest_path_for_entry(
    output_root: impl AsRef<Path>,
    entry: &MboCacheEntry,
) -> PathBuf {
    derived_cache_manifest_path(output_root, &DerivedCacheSourceManifest::from_entry(entry))
}

fn partition_relative_path(source_slug: &str, partition_index: u64) -> PathBuf {
    PathBuf::from(source_slug)
        .join("partitions")
        .join(format!("partition-{partition_index:06}.v1.json.zst"))
}

fn sibling_temp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("derived-cache-manifest.json");
    path.with_file_name(format!(".{file_name}.tmp"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OhlcvBarKey {
    pub interval_ns: u64,
    pub interval_start_ns: u64,
    pub instrument_id: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OhlcvBar {
    pub open: MboPrice,
    pub high: MboPrice,
    pub low: MboPrice,
    pub close: MboPrice,
    pub volume: u64,
    pub buy_volume: u64,
    pub sell_volume: u64,
    pub unknown_volume: u64,
    pub trade_count: u64,
    pub fill_count: u64,
    pub delta: i64,
}

impl OhlcvBar {
    fn new(print: &MboTradePrint) -> Self {
        let mut bar = Self {
            open: print.price,
            high: print.price,
            low: print.price,
            close: print.price,
            volume: 0,
            buy_volume: 0,
            sell_volume: 0,
            unknown_volume: 0,
            trade_count: 0,
            fill_count: 0,
            delta: 0,
        };
        bar.apply_print(print);
        bar
    }

    fn apply_print(&mut self, print: &MboTradePrint) {
        self.high = max_price(self.high, print.price);
        self.low = min_price(self.low, print.price);
        self.close = print.price;
        self.volume += u64::from(print.size);
        self.delta += print.signed_size();
        match print.side {
            AggressorSide::Buy => self.buy_volume += u64::from(print.size),
            AggressorSide::Sell => self.sell_volume += u64::from(print.size),
            AggressorSide::Unknown => self.unknown_volume += u64::from(print.size),
        }
        match print.action {
            super::mbo::MboAction::Trade => self.trade_count += 1,
            super::mbo::MboAction::Fill => self.fill_count += 1,
            _ => {}
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MultiResolutionOhlcvCache {
    bars: BTreeMap<OhlcvBarKey, OhlcvBar>,
}

impl MultiResolutionOhlcvCache {
    pub fn apply_print(&mut self, print: &MboTradePrint) {
        if !print.price.is_defined() || print.size == 0 {
            return;
        }
        for interval in BOOKMAP_CANDLE_INTERVALS {
            let key = OhlcvBarKey {
                interval_ns: interval.interval_ns,
                interval_start_ns: bucket_start(print.ts_event, interval.interval_ns),
                instrument_id: print.instrument_id,
            };
            self.bars
                .entry(key)
                .and_modify(|bar| bar.apply_print(print))
                .or_insert_with(|| OhlcvBar::new(print));
        }
    }

    pub fn bar(&self, key: OhlcvBarKey) -> Option<OhlcvBar> {
        self.bars.get(&key).copied()
    }

    pub fn bars_for_interval(
        &self,
        interval_ns: u64,
    ) -> impl Iterator<Item = (OhlcvBarKey, OhlcvBar)> + '_ {
        self.bars
            .iter()
            .filter(move |(key, _)| key.interval_ns == interval_ns)
            .map(|(key, bar)| (*key, *bar))
    }

    pub fn bar_count(&self) -> usize {
        self.bars.len()
    }

    pub fn drain_ready_bars(
        &mut self,
        cutoff_ts: u64,
        include_open_bucket: bool,
    ) -> Vec<(OhlcvBarKey, OhlcvBar)> {
        let keys = self
            .bars
            .keys()
            .copied()
            .filter(|key| {
                include_open_bucket
                    || key.interval_start_ns < bucket_start(cutoff_ts, key.interval_ns)
            })
            .collect::<Vec<_>>();
        keys.into_iter()
            .filter_map(|key| self.bars.remove(&key).map(|bar| (key, bar)))
            .collect()
    }
}

pub const fn bucket_start(timestamp_ns: u64, interval_ns: u64) -> u64 {
    timestamp_ns / interval_ns * interval_ns
}

fn min_price(left: MboPrice, right: MboPrice) -> MboPrice {
    if right.fixed() < left.fixed() {
        right
    } else {
        left
    }
}

fn max_price(left: MboPrice, right: MboPrice) -> MboPrice {
    if right.fixed() > left.fixed() {
        right
    } else {
        left
    }
}

pub fn fixed_price(value: i64) -> MboPrice {
    MboPrice::from_fixed(value * FIXED_PRICE_SCALE)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::{
        ArtifactPriceTimeWindowQuery, ArtifactTimeWindowQuery, BOOKMAP_CANDLE_INTERVALS,
        BboTransitionRecord, BboTransitionsArtifact, BookmapCompositeActivityCandidate,
        DERIVED_CACHE_MANIFEST_KIND, DERIVED_CACHE_PARTITION_KIND, DERIVED_CACHE_PROGRESS_KIND,
        DERIVED_CACHE_SCHEMA_VERSION, DERIVED_CACHE_SHARD_INDEX_KIND, DerivedActivityWindow,
        DerivedCacheArtifactKind, DerivedCacheBuildConfig, DerivedCacheBuildStatus,
        DerivedCacheBuildSummary, DerivedCacheFinalizationEventKind, DerivedCacheManifest,
        DerivedCacheManifestError, DerivedCachePartitionArtifact, DerivedCachePlan,
        DerivedCacheProgressEventKind, DerivedCacheProgressManifest, DerivedCacheShardIndex,
        DomLevelRecord, DomSnapshotQuery, DomSnapshotRecord, DomSnapshotsArtifact,
        FootprintCellRecord, FootprintCellsArtifact, FootprintWindowQuery, HOUR_NS,
        HeatmapCellRecord, HeatmapTilesArtifact, MINUTE_NS, MultiResolutionOhlcvCache, OhlcvBarKey,
        OhlcvBarRecord, OhlcvBarsArtifact, OhlcvWindowQuery, PriceLevelRecord,
        ReplayCheckpointArtifact, VolumeBubbleRecord, VolumeBubblesArtifact, bucket_start,
        build_derived_cache_artifacts, build_derived_cache_resumable,
        build_derived_cache_resumable_with_progress, compare_bookmap_composite_activity_candidate,
        derived_cache_artifact_storage_report, derived_cache_manifest_path_for_entry,
        finalize_derived_cache_partitions, finalize_derived_cache_partitions_with_progress,
        fixed_price, heuristic_add_on_events_from_records, planned_artifacts,
        progress_manifest_path, query_add_on_events, query_bbo_transitions, query_dom_snapshots,
        query_footprint_cells, query_heatmap_cells, query_ohlcv_bars, query_volume_bubbles,
        read_cache_artifact, replay_checkpoint_at_or_before_event,
        replay_checkpoint_at_or_before_time, source_slug, write_shardable_artifact,
    };
    use crate::order_flow::{
        AggressorSide, FIXED_PRICE_SCALE, HistoricalDbnMboSource, MboAction, MboCacheEntry,
        MboEvent, MboPrice, MboSide, MboSourceInfo, MboTradePrint, OrderFlowReplayConfig,
        OrderFlowReplayRuntime, SECOND_NS as _SECOND_NS_FROM_DERIVED_REEXPORT, VecMboEventSource,
    };

    fn entry() -> MboCacheEntry {
        MboCacheEntry {
            dataset: "GLBX.MDP3".to_string(),
            schema: "mbo".to_string(),
            symbol: "NQ.FUT".to_string(),
            stype_in: "parent".to_string(),
            stype_out: "instrument_id".to_string(),
            start: Some("2024-11-29T00:00:00Z".to_string()),
            end_exclusive: Some("2025-03-06T00:00:00Z".to_string()),
            raw_path: PathBuf::from("E-mini/data/raw/NQ-FUT/mbo/sample.dbn.zst"),
            estimated_record_count: Some(1_267_426_546),
            estimated_cost_usd: Some(118.98),
            estimated_billable_size_bytes: Some(69_781_817_336),
            bytes_on_disk: Some(22_941_221_592),
        }
    }

    fn local_nq_entry() -> MboCacheEntry {
        MboCacheEntry {
            dataset: "GLBX.MDP3".to_string(),
            schema: "mbo".to_string(),
            symbol: "NQ.FUT".to_string(),
            stype_in: "parent".to_string(),
            stype_out: "instrument_id".to_string(),
            start: Some("2024-11-29T00:00:00Z".to_string()),
            end_exclusive: Some("2024-12-03T00:00:00Z".to_string()),
            raw_path: PathBuf::from(
                "E-mini/data/raw/NQ-FUT/mbo/GLBX-MDP3_NQ-FUT_mbo_parent_20241129T000000Z_20241203T000000Z.dbn.zst",
            ),
            estimated_record_count: Some(21_322_665),
            estimated_cost_usd: Some(2.001714549959),
            estimated_billable_size_bytes: Some(1_194_069_240),
            bytes_on_disk: Some(387_945_589),
        }
    }

    fn summary() -> DerivedCacheBuildSummary {
        DerivedCacheBuildSummary {
            events_read: 10_000,
            applied_book_events: 8_000,
            trade_prints: 500,
            first_event_ts: Some(1),
            last_event_ts: Some(9_999),
            instrument_count: 1,
            checkpoint_count: 10,
            ohlcv_bars: 11,
            footprint_cells: 20,
            heatmap_cells: 30,
            volume_bubbles: 40,
            bbo_transitions: 50,
            add_on_events: 6,
        }
    }

    fn print(ts_event: u64, price: i64, size: u32, side: AggressorSide) -> MboTradePrint {
        MboTradePrint {
            instrument_id: 1,
            order_id: ts_event,
            action: MboAction::Trade,
            side,
            price: fixed_price(price),
            size,
            ts_event,
            ts_recv: ts_event,
            sequence: ts_event as u32,
        }
    }

    fn source_info() -> MboSourceInfo {
        MboSourceInfo::new("GLBX.MDP3", "NQ.FUT", "parent", "instrument_id")
    }

    fn event(
        ts_event: u64,
        order_id: u64,
        action: MboAction,
        side: MboSide,
        price: i64,
        size: u32,
    ) -> MboEvent {
        let mut event = MboEvent::book_event(
            source_info(),
            1,
            order_id,
            action,
            side,
            MboPrice::from_fixed(price * FIXED_PRICE_SCALE),
            size,
        );
        event.ts_event = ts_event;
        event
    }

    #[test]
    fn bookmap_candle_intervals_cover_requested_timeframes() {
        let labels = BOOKMAP_CANDLE_INTERVALS
            .iter()
            .map(|interval| interval.label)
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            vec![
                "1m", "3m", "5m", "15m", "1h", "4h", "1w", "1mo", "3mo", "6mo", "12mo"
            ]
        );
    }

    #[test]
    fn cache_plan_scales_checkpoints_for_billion_event_payloads() {
        let plan = DerivedCachePlan::for_estimated_events(1_267_426_546);

        assert_eq!(plan.checkpoint_every_events, 1_000_000);
        assert_eq!(plan.event_partitions, 1_268);
        assert_eq!(plan.candle_intervals.len(), 11);
        assert!(!plan.heatmap_lods.is_empty());
    }

    #[test]
    fn ohlcv_cache_aggregates_prints_across_requested_intervals() {
        let mut cache = MultiResolutionOhlcvCache::default();
        cache.apply_print(&print(MINUTE_NS + 1, 100, 2, AggressorSide::Buy));
        cache.apply_print(&print(MINUTE_NS + 2, 103, 4, AggressorSide::Sell));
        cache.apply_print(&print(MINUTE_NS + 3, 99, 1, AggressorSide::Buy));

        let key = OhlcvBarKey {
            interval_ns: MINUTE_NS,
            interval_start_ns: MINUTE_NS,
            instrument_id: 1,
        };
        let bar = cache.bar(key).expect("1m bar");

        assert_eq!(bar.open, fixed_price(100));
        assert_eq!(bar.high, fixed_price(103));
        assert_eq!(bar.low, fixed_price(99));
        assert_eq!(bar.close, fixed_price(99));
        assert_eq!(bar.volume, 7);
        assert_eq!(bar.buy_volume, 3);
        assert_eq!(bar.sell_volume, 4);
        assert_eq!(bar.delta, -1);
        assert!(cache.bar_count() >= BOOKMAP_CANDLE_INTERVALS.len());
    }

    #[test]
    fn bucket_start_handles_large_intervals() {
        assert_eq!(bucket_start(5 * HOUR_NS + 10, 4 * HOUR_NS), 4 * HOUR_NS);
        assert_eq!(
            _SECOND_NS_FROM_DERIVED_REEXPORT,
            super::SECOND_NS,
            "derived SECOND_NS is re-exported for downstream modules"
        );
    }

    #[test]
    fn bookmap_composite_activity_prefers_integrated_layer_context_over_raw_volume() {
        let high_volume_sparse = BookmapCompositeActivityCandidate {
            window: DerivedActivityWindow {
                instrument_id: 1,
                start_ts: 0,
                end_ts: 30 * MINUTE_NS,
                bubble_count: 500,
                total_volume: 1_000_000,
            },
            bbo_records: 100,
            ohlcv_records: 3,
            heatmap_records: 1_000,
        };
        let integrated_context = BookmapCompositeActivityCandidate {
            window: DerivedActivityWindow {
                instrument_id: 2,
                start_ts: 60 * MINUTE_NS,
                end_ts: 90 * MINUTE_NS,
                bubble_count: 100,
                total_volume: 50_000,
            },
            bbo_records: 50_000,
            ohlcv_records: 200,
            heatmap_records: 200_000,
        };

        assert_eq!(
            compare_bookmap_composite_activity_candidate(&integrated_context, &high_volume_sparse),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn bookmap_composite_activity_keeps_volume_as_tie_breaker_for_equal_context() {
        let lower_volume = BookmapCompositeActivityCandidate {
            window: DerivedActivityWindow {
                instrument_id: 1,
                start_ts: 0,
                end_ts: 30 * MINUTE_NS,
                bubble_count: 20,
                total_volume: 1_000,
            },
            bbo_records: 2_000,
            ohlcv_records: 60,
            heatmap_records: 8_000,
        };
        let higher_volume = BookmapCompositeActivityCandidate {
            window: DerivedActivityWindow {
                instrument_id: 1,
                start_ts: 30 * MINUTE_NS,
                end_ts: 60 * MINUTE_NS,
                bubble_count: 20,
                total_volume: 4_000,
            },
            ..lower_volume
        };

        assert_eq!(
            compare_bookmap_composite_activity_candidate(&higher_volume, &lower_volume),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn derived_manifest_records_source_plan_summary_and_artifacts() {
        let manifest = DerivedCacheManifest::from_entry(&entry(), summary());

        assert_eq!(manifest.kind, DERIVED_CACHE_MANIFEST_KIND);
        assert_eq!(manifest.schema_version, DERIVED_CACHE_SCHEMA_VERSION);
        assert_eq!(
            manifest.source.source_key(),
            "GLBX.MDP3:NQ.FUT:mbo:parent:instrument_id:2024-11-29T00:00:00Z:2025-03-06T00:00:00Z"
        );
        assert_eq!(manifest.plan.estimated_events, 1_267_426_546);
        assert_eq!(manifest.plan.checkpoint_every_events, 1_000_000);
        assert_eq!(manifest.plan.event_partitions, 1_268);
        assert_eq!(manifest.plan.candle_intervals.len(), 11);
        assert_eq!(manifest.plan.heatmap_lods.len(), 5);
        assert_eq!(manifest.summary.ohlcv_bars, 11);
        assert_eq!(manifest.summary.volume_bubbles, 40);
        assert!(manifest.artifacts.iter().any(|artifact| {
            artifact.kind == DerivedCacheArtifactKind::HeatmapTiles
                && artifact
                    .relative_path
                    .to_string_lossy()
                    .ends_with("heatmap-tiles.v1.json")
        }));
    }

    #[test]
    fn derived_manifest_round_trips_json_and_validates_schema() {
        let manifest = DerivedCacheManifest::from_entry(&entry(), summary());
        let json = manifest.to_pretty_json().expect("json");
        let parsed = DerivedCacheManifest::from_json(&json).expect("parsed manifest");

        assert_eq!(parsed, manifest);
        assert!(json.contains("rustchart_bookmap_derived_cache"));

        let mut wrong_version = parsed;
        wrong_version.schema_version = DERIVED_CACHE_SCHEMA_VERSION + 1;
        assert!(matches!(
            wrong_version.validate_schema(),
            Err(DerivedCacheManifestError::UnsupportedSchemaVersion { found, expected })
                if found == DERIVED_CACHE_SCHEMA_VERSION + 1
                    && expected == DERIVED_CACHE_SCHEMA_VERSION
        ));

        let mut wrong_kind = manifest;
        wrong_kind.kind = "other".to_string();
        assert!(matches!(
            wrong_kind.validate_schema(),
            Err(DerivedCacheManifestError::WrongKind(kind)) if kind == "other"
        ));
    }

    #[test]
    fn derived_manifest_writes_and_reads_atomically() {
        let dir = std::env::temp_dir().join(format!(
            "rustchart-derived-cache-manifest-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let path = derived_cache_manifest_path_for_entry(&dir, &entry());
        let manifest = DerivedCacheManifest::from_entry(&entry(), summary());

        manifest
            .write_to_path_atomic(&path)
            .expect("write manifest");
        let parsed = DerivedCacheManifest::read_from_path(&path).expect("read manifest");

        assert_eq!(parsed, manifest);
        assert!(!path.with_file_name(".derived-cache.v1.json.tmp").exists());
        fs::remove_dir_all(dir).expect("cleanup manifest temp dir");
    }

    #[test]
    fn replay_state_summary_counts_derived_outputs() {
        let events = vec![
            event(1, 1, MboAction::Add, MboSide::Bid, 100, 3),
            event(2, 2, MboAction::Add, MboSide::Ask, 102, 4),
            event(3, 2, MboAction::Trade, MboSide::Bid, 102, 5),
            event(4, 2, MboAction::Cancel, MboSide::Ask, 102, 4),
        ];
        let source = VecMboEventSource::new(source_info(), events);
        let config = OrderFlowReplayConfig {
            checkpoint_every_events: 2,
            large_trade_threshold: 5,
            footprint_interval_ns: 10,
            heatmap_interval_ns: 1,
            heatmap_depth_limit: 1,
        };
        let mut runtime = OrderFlowReplayRuntime::new(source, config);
        let report = runtime.process_next_batch(10).expect("process");

        let summary = DerivedCacheBuildSummary::from_replay_state(
            runtime.state(),
            Some(1),
            report.applied_book_events as u64,
            report.trade_prints as u64,
            1,
        );

        assert_eq!(summary.events_read, 4);
        assert_eq!(summary.applied_book_events, 3);
        assert_eq!(summary.trade_prints, 1);
        assert_eq!(summary.first_event_ts, Some(1));
        assert_eq!(summary.last_event_ts, Some(4));
        assert_eq!(summary.checkpoint_count, 2);
        assert!(summary.ohlcv_bars > 0);
        assert_eq!(summary.footprint_cells, 1);
        assert!(summary.heatmap_cells > 0);
        assert_eq!(summary.volume_bubbles, 1);
        assert_eq!(summary.bbo_transitions, 3);
        assert!(summary.add_on_events > 0);
    }

    #[test]
    fn heuristic_addon_events_from_records_emit_public_labeled_streams() {
        let heatmap_cells = vec![
            HeatmapCellRecord {
                interval_start_ns: 10,
                instrument_id: 1,
                price_fixed: 100 * FIXED_PRICE_SCALE,
                side: "bid".to_string(),
                displayed_size: 50,
                max_displayed_size: 50,
                samples: 3,
                intensity: 0.95,
            },
            HeatmapCellRecord {
                interval_start_ns: 10,
                instrument_id: 1,
                price_fixed: 101 * FIXED_PRICE_SCALE,
                side: "ask".to_string(),
                displayed_size: 2,
                max_displayed_size: 2,
                samples: 1,
                intensity: 0.1,
            },
        ];
        let volume_bubbles = vec![
            VolumeBubbleRecord {
                interval_start_ns: 10,
                instrument_id: 1,
                price_fixed: 100 * FIXED_PRICE_SCALE,
                buy_volume: 20,
                sell_volume: 0,
                unknown_volume: 0,
                total_volume: 20,
                delta: 20,
                trade_count: 2,
                fill_count: 3,
            },
            VolumeBubbleRecord {
                interval_start_ns: 20,
                instrument_id: 1,
                price_fixed: 101 * FIXED_PRICE_SCALE,
                buy_volume: 5,
                sell_volume: 0,
                unknown_volume: 0,
                total_volume: 5,
                delta: 5,
                trade_count: 1,
                fill_count: 1,
            },
        ];

        let events = heuristic_add_on_events_from_records(&heatmap_cells, &volume_bubbles, 32);
        let kinds = events
            .iter()
            .map(|event| event.kind.as_str())
            .collect::<Vec<_>>();

        assert!(kinds.contains(&"rustchart.heuristic.stop_run_like"));
        assert!(kinds.contains(&"rustchart.heuristic.sweep_like"));
        assert!(kinds.contains(&"rustchart.heuristic.iceberg_like"));
        assert!(kinds.contains(&"rustchart.heuristic.large_liquidity"));
        assert!(kinds.contains(&"rustchart.heuristic.market_pulse_volume_pressure"));
        assert!(events.iter().all(|event| {
            event.kind.starts_with("rustchart.heuristic.")
                && !event.source.is_empty()
                && !event.display_label.is_empty()
                && event.confidence_bps > 0
                && event.confidence_bps <= 10_000
        }));
    }

    #[test]
    fn planned_artifacts_are_source_scoped_and_stable() {
        let source = super::DerivedCacheSourceManifest::from_entry(&entry());
        let slug = source_slug(&source);
        let artifacts = planned_artifacts(&slug);

        assert_eq!(
            slug,
            "glbx-mdp3-nq-fut-mbo-parent-instrument-id-2024-11-29t00-00-00z-2025-03-06t00-00-00z"
        );
        assert_eq!(artifacts.len(), 8);
        assert_eq!(
            artifacts[0].relative_path,
            PathBuf::from(
                "glbx-mdp3-nq-fut-mbo-parent-instrument-id-2024-11-29t00-00-00z-2025-03-06t00-00-00z"
            )
            .join("replay-checkpoints.v1.json")
        );
        assert!(
            artifacts
                .iter()
                .any(|artifact| artifact.kind == DerivedCacheArtifactKind::DomSnapshots)
        );
    }

    #[test]
    fn builder_streams_source_and_writes_queryable_artifacts() {
        let dir = std::env::temp_dir().join(format!(
            "rustchart-derived-cache-builder-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let events = vec![
            event(1, 1, MboAction::Add, MboSide::Bid, 100, 3),
            event(2, 2, MboAction::Add, MboSide::Ask, 102, 4),
            event(3, 2, MboAction::Trade, MboSide::Bid, 102, 5),
            event(4, 2, MboAction::Cancel, MboSide::Ask, 102, 4),
        ];
        let source = VecMboEventSource::new(source_info(), events);
        let config = DerivedCacheBuildConfig {
            replay: OrderFlowReplayConfig {
                checkpoint_every_events: 2,
                large_trade_threshold: 5,
                footprint_interval_ns: 10,
                heatmap_interval_ns: 1,
                heatmap_depth_limit: 1,
            },
            batch_size: 2,
            max_events: None,
        };

        let manifest =
            build_derived_cache_artifacts(&entry(), source, &dir, config).expect("build artifacts");

        assert_eq!(manifest.summary.events_read, 4);
        assert_eq!(manifest.summary.applied_book_events, 3);
        assert_eq!(manifest.summary.trade_prints, 1);
        assert_eq!(manifest.summary.checkpoint_count, 2);
        assert!(manifest.summary.ohlcv_bars > 0);
        assert!(derived_cache_manifest_path_for_entry(&dir, &entry()).exists());
        let storage_report =
            derived_cache_artifact_storage_report(&manifest, &dir).expect("storage report");
        assert_eq!(storage_report.artifact_count(), manifest.artifacts.len());
        assert_eq!(storage_report.shard_count(), 0);
        assert!(storage_report.total_bytes() > 0);

        let checkpoints: ReplayCheckpointArtifact =
            read_cache_artifact(&manifest, &dir, DerivedCacheArtifactKind::ReplayCheckpoints)
                .expect("checkpoints");
        assert_eq!(checkpoints.checkpoints.len(), 2);
        assert_eq!(checkpoints.checkpoint_every_events, 2);

        let bubbles: VolumeBubblesArtifact =
            read_cache_artifact(&manifest, &dir, DerivedCacheArtifactKind::VolumeBubbles)
                .expect("bubbles");
        assert_eq!(bubbles.bubbles.len(), 1);
        assert_eq!(bubbles.bubbles[0].total_volume, 5);

        let cells = query_heatmap_cells(
            &manifest,
            &dir,
            super::HeatmapViewportQuery {
                instrument_id: 1,
                start_ts: 1,
                end_ts: 4,
                min_price_fixed: Some(100 * FIXED_PRICE_SCALE),
                max_price_fixed: Some(102 * FIXED_PRICE_SCALE),
                limit: 8,
            },
        )
        .expect("heatmap query");
        assert!(!cells.is_empty());
        assert!(
            cells
                .iter()
                .all(|cell| cell.instrument_id == 1 && cell.interval_start_ns <= 4)
        );

        let bars = query_ohlcv_bars(
            &manifest,
            &dir,
            OhlcvWindowQuery {
                interval_ns: MINUTE_NS,
                instrument_id: 1,
                start_ts: 0,
                end_ts: 10,
                limit: 10,
            },
        )
        .expect("ohlcv query");
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].close_fixed, 102 * FIXED_PRICE_SCALE);
        assert_eq!(bars[0].volume, 5);

        let transitions = query_bbo_transitions(
            &manifest,
            &dir,
            ArtifactTimeWindowQuery {
                instrument_id: 1,
                start_ts: 1,
                end_ts: 4,
                limit: 2,
            },
        )
        .expect("bbo query");
        assert_eq!(transitions.len(), 2);
        assert!(
            transitions
                .windows(2)
                .all(|window| window[0].ts_event <= window[1].ts_event)
        );

        let queried_bubbles = query_volume_bubbles(
            &manifest,
            &dir,
            ArtifactPriceTimeWindowQuery {
                instrument_id: 1,
                start_ts: 0,
                end_ts: 10,
                min_price_fixed: Some(102 * FIXED_PRICE_SCALE),
                max_price_fixed: Some(102 * FIXED_PRICE_SCALE),
                limit: 4,
            },
        )
        .expect("bubble query");
        assert_eq!(queried_bubbles.len(), 1);
        assert_eq!(queried_bubbles[0].total_volume, 5);

        let add_on_events = query_add_on_events(
            &manifest,
            &dir,
            ArtifactTimeWindowQuery {
                instrument_id: 1,
                start_ts: 0,
                end_ts: 10,
                limit: 16,
            },
        )
        .expect("add-on event query");
        assert!(!add_on_events.is_empty());
        assert!(manifest.summary.add_on_events >= add_on_events.len());
        assert!(add_on_events.iter().all(|event| {
            event.kind.starts_with("rustchart.heuristic.")
                && !event.source.is_empty()
                && !event.display_label.is_empty()
        }));

        let queried_footprints = query_footprint_cells(
            &manifest,
            &dir,
            FootprintWindowQuery {
                start_ts: 0,
                end_ts: 10,
                min_price_fixed: Some(102 * FIXED_PRICE_SCALE),
                max_price_fixed: Some(102 * FIXED_PRICE_SCALE),
                limit: 4,
            },
        )
        .expect("footprint query");
        assert_eq!(queried_footprints.len(), 1);
        assert_eq!(queried_footprints[0].total_volume, 5);

        let dom_snapshots = query_dom_snapshots(
            &manifest,
            &dir,
            DomSnapshotQuery {
                instrument_id: 1,
                at_or_before_ts: Some(4),
                min_price_fixed: Some(100 * FIXED_PRICE_SCALE),
                max_price_fixed: Some(102 * FIXED_PRICE_SCALE),
                snapshot_limit: 1,
                level_limit: 1,
            },
        )
        .expect("dom query");
        assert_eq!(dom_snapshots.len(), 1);
        assert_eq!(dom_snapshots[0].timestamp_ns, Some(4));
        assert_eq!(dom_snapshots[0].levels.len(), 1);

        let checkpoint_by_event = replay_checkpoint_at_or_before_event(&manifest, &dir, 3)
            .expect("checkpoint event")
            .expect("event checkpoint");
        assert_eq!(checkpoint_by_event.event_index, 2);
        let checkpoint_by_time = replay_checkpoint_at_or_before_time(&manifest, &dir, 4)
            .expect("checkpoint time")
            .expect("time checkpoint");
        assert_eq!(checkpoint_by_time.event_index, 4);

        fs::remove_dir_all(dir).expect("cleanup builder temp dir");
    }

    #[test]
    fn sharded_artifacts_are_queryable_by_time_window() {
        let dir = std::env::temp_dir().join(format!(
            "rustchart-derived-cache-shards-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let manifest = DerivedCacheManifest::from_entry(&entry(), summary());

        write_shardable_artifact(
            &manifest,
            &dir,
            DerivedCacheArtifactKind::HeatmapTiles,
            &HeatmapTilesArtifact {
                cells: vec![
                    HeatmapCellRecord {
                        interval_start_ns: 10,
                        instrument_id: 1,
                        price_fixed: 100 * FIXED_PRICE_SCALE,
                        side: "bid".to_string(),
                        displayed_size: 3,
                        max_displayed_size: 3,
                        samples: 1,
                        intensity: 0.5,
                    },
                    HeatmapCellRecord {
                        interval_start_ns: 30,
                        instrument_id: 1,
                        price_fixed: 101 * FIXED_PRICE_SCALE,
                        side: "ask".to_string(),
                        displayed_size: 7,
                        max_displayed_size: 7,
                        samples: 1,
                        intensity: 1.0,
                    },
                ],
            },
            1,
        )
        .expect("write heatmap shards");
        let heatmap_index_path = manifest
            .artifact_path(&dir, DerivedCacheArtifactKind::HeatmapTiles)
            .expect("heatmap path");
        let heatmap_index: DerivedCacheShardIndex =
            serde_json::from_str(&fs::read_to_string(heatmap_index_path).expect("index json"))
                .expect("heatmap shard index");
        assert_eq!(heatmap_index.kind, DERIVED_CACHE_SHARD_INDEX_KIND);
        assert_eq!(heatmap_index.shards.len(), 2);
        assert!(
            heatmap_index
                .shards
                .iter()
                .all(|shard| shard.relative_path.to_string_lossy().ends_with(".json.zst"))
        );

        write_shardable_artifact(
            &manifest,
            &dir,
            DerivedCacheArtifactKind::OhlcvBars,
            &OhlcvBarsArtifact {
                bars: vec![
                    OhlcvBarRecord {
                        interval_ns: MINUTE_NS,
                        interval_start_ns: 10,
                        instrument_id: 1,
                        open_fixed: 100,
                        high_fixed: 101,
                        low_fixed: 99,
                        close_fixed: 100,
                        volume: 1,
                        buy_volume: 1,
                        sell_volume: 0,
                        unknown_volume: 0,
                        trade_count: 1,
                        fill_count: 0,
                        delta: 1,
                    },
                    OhlcvBarRecord {
                        interval_ns: MINUTE_NS,
                        interval_start_ns: 30,
                        instrument_id: 1,
                        open_fixed: 101,
                        high_fixed: 103,
                        low_fixed: 100,
                        close_fixed: 102,
                        volume: 9,
                        buy_volume: 8,
                        sell_volume: 1,
                        unknown_volume: 0,
                        trade_count: 2,
                        fill_count: 0,
                        delta: 7,
                    },
                ],
            },
            1,
        )
        .expect("write ohlcv shards");

        write_shardable_artifact(
            &manifest,
            &dir,
            DerivedCacheArtifactKind::BboTransitions,
            &BboTransitionsArtifact {
                transitions: vec![
                    BboTransitionRecord {
                        ts_event: 10,
                        instrument_id: 1,
                        bid: None,
                        ask: None,
                        spread: None,
                        mid: Some(100.0),
                    },
                    BboTransitionRecord {
                        ts_event: 30,
                        instrument_id: 1,
                        bid: None,
                        ask: None,
                        spread: Some(1.0),
                        mid: Some(102.0),
                    },
                ],
            },
            1,
        )
        .expect("write bbo shards");

        write_shardable_artifact(
            &manifest,
            &dir,
            DerivedCacheArtifactKind::VolumeBubbles,
            &VolumeBubblesArtifact {
                interval_ns: 10,
                bubbles: vec![
                    VolumeBubbleRecord {
                        interval_start_ns: 10,
                        instrument_id: 1,
                        price_fixed: 100 * FIXED_PRICE_SCALE,
                        buy_volume: 1,
                        sell_volume: 0,
                        unknown_volume: 0,
                        total_volume: 1,
                        delta: 1,
                        trade_count: 1,
                        fill_count: 0,
                    },
                    VolumeBubbleRecord {
                        interval_start_ns: 30,
                        instrument_id: 1,
                        price_fixed: 101 * FIXED_PRICE_SCALE,
                        buy_volume: 8,
                        sell_volume: 1,
                        unknown_volume: 0,
                        total_volume: 9,
                        delta: 7,
                        trade_count: 2,
                        fill_count: 0,
                    },
                ],
            },
            1,
        )
        .expect("write bubble shards");

        write_shardable_artifact(
            &manifest,
            &dir,
            DerivedCacheArtifactKind::FootprintCells,
            &FootprintCellsArtifact {
                interval_ns: 10,
                cells: vec![
                    FootprintCellRecord {
                        interval_start_ns: 10,
                        price_fixed: 100 * FIXED_PRICE_SCALE,
                        buy_volume: 1,
                        sell_volume: 0,
                        unknown_volume: 0,
                        total_volume: 1,
                        delta: 1,
                    },
                    FootprintCellRecord {
                        interval_start_ns: 30,
                        price_fixed: 101 * FIXED_PRICE_SCALE,
                        buy_volume: 8,
                        sell_volume: 1,
                        unknown_volume: 0,
                        total_volume: 9,
                        delta: 7,
                    },
                ],
            },
            1,
        )
        .expect("write footprint shards");

        write_shardable_artifact(
            &manifest,
            &dir,
            DerivedCacheArtifactKind::DomSnapshots,
            &DomSnapshotsArtifact {
                snapshots: vec![
                    DomSnapshotRecord {
                        timestamp_ns: Some(10),
                        instrument_id: 1,
                        best_bid: None,
                        best_ask: None,
                        max_size: 1,
                        levels: vec![DomLevelRecord {
                            price_fixed: 100 * FIXED_PRICE_SCALE,
                            bid_size: 1,
                            ask_size: 0,
                            total_size: 1,
                            side_hint: "bid".to_string(),
                        }],
                    },
                    DomSnapshotRecord {
                        timestamp_ns: Some(30),
                        instrument_id: 1,
                        best_bid: Some(PriceLevelRecord {
                            price_fixed: 101 * FIXED_PRICE_SCALE,
                            size: 8,
                        }),
                        best_ask: None,
                        max_size: 8,
                        levels: vec![DomLevelRecord {
                            price_fixed: 101 * FIXED_PRICE_SCALE,
                            bid_size: 8,
                            ask_size: 1,
                            total_size: 9,
                            side_hint: "bid".to_string(),
                        }],
                    },
                ],
            },
            1,
        )
        .expect("write dom shards");

        let heatmap = query_heatmap_cells(
            &manifest,
            &dir,
            super::HeatmapViewportQuery {
                instrument_id: 1,
                start_ts: 20,
                end_ts: 40,
                min_price_fixed: None,
                max_price_fixed: None,
                limit: 10,
            },
        )
        .expect("sharded heatmap query");
        assert_eq!(heatmap.len(), 1);
        assert_eq!(heatmap[0].displayed_size, 7);

        let bars = query_ohlcv_bars(
            &manifest,
            &dir,
            OhlcvWindowQuery {
                interval_ns: MINUTE_NS,
                instrument_id: 1,
                start_ts: 20,
                end_ts: 40,
                limit: 10,
            },
        )
        .expect("sharded ohlcv query");
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].volume, 9);

        let bbo = query_bbo_transitions(
            &manifest,
            &dir,
            ArtifactTimeWindowQuery {
                instrument_id: 1,
                start_ts: 20,
                end_ts: 40,
                limit: 10,
            },
        )
        .expect("sharded bbo query");
        assert_eq!(bbo.len(), 1);
        assert_eq!(bbo[0].mid, Some(102.0));

        let bubbles = query_volume_bubbles(
            &manifest,
            &dir,
            ArtifactPriceTimeWindowQuery {
                instrument_id: 1,
                start_ts: 20,
                end_ts: 40,
                min_price_fixed: None,
                max_price_fixed: None,
                limit: 10,
            },
        )
        .expect("sharded bubble query");
        assert_eq!(bubbles.len(), 1);
        assert_eq!(bubbles[0].total_volume, 9);

        let footprints = query_footprint_cells(
            &manifest,
            &dir,
            FootprintWindowQuery {
                start_ts: 20,
                end_ts: 40,
                min_price_fixed: None,
                max_price_fixed: None,
                limit: 10,
            },
        )
        .expect("sharded footprint query");
        assert_eq!(footprints.len(), 1);
        assert_eq!(footprints[0].delta, 7);

        let dom = query_dom_snapshots(
            &manifest,
            &dir,
            DomSnapshotQuery {
                instrument_id: 1,
                at_or_before_ts: Some(20),
                min_price_fixed: None,
                max_price_fixed: None,
                snapshot_limit: 10,
                level_limit: 10,
            },
        )
        .expect("sharded dom query");
        assert_eq!(dom.len(), 1);
        assert_eq!(dom[0].timestamp_ns, Some(10));

        fs::remove_dir_all(dir).expect("cleanup sharded artifact temp dir");
    }

    #[test]
    fn builder_respects_max_events_for_partial_artifact_runs() {
        let dir = std::env::temp_dir().join(format!(
            "rustchart-derived-cache-builder-partial-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let events = vec![
            event(1, 1, MboAction::Add, MboSide::Bid, 100, 3),
            event(2, 2, MboAction::Add, MboSide::Ask, 102, 4),
            event(3, 2, MboAction::Trade, MboSide::Bid, 102, 5),
            event(4, 2, MboAction::Cancel, MboSide::Ask, 102, 4),
        ];
        let source = VecMboEventSource::new(source_info(), events);
        let config = DerivedCacheBuildConfig {
            replay: OrderFlowReplayConfig {
                checkpoint_every_events: 2,
                large_trade_threshold: 5,
                footprint_interval_ns: 10,
                heatmap_interval_ns: 1,
                heatmap_depth_limit: 1,
            },
            batch_size: 3,
            max_events: Some(3),
        };

        let manifest =
            build_derived_cache_artifacts(&entry(), source, &dir, config).expect("partial build");

        assert_eq!(manifest.summary.events_read, 3);
        assert_eq!(manifest.summary.trade_prints, 1);
        let heatmap: HeatmapTilesArtifact =
            read_cache_artifact(&manifest, &dir, DerivedCacheArtifactKind::HeatmapTiles)
                .expect("heatmap");
        assert!(heatmap.cells.iter().all(|cell| cell.interval_start_ns <= 2));

        fs::remove_dir_all(dir).expect("cleanup partial builder temp dir");
    }

    #[test]
    fn resumable_builder_writes_progress_partitions_and_resumes() {
        let dir = std::env::temp_dir().join(format!(
            "rustchart-derived-cache-resumable-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let events = vec![
            event(1, 1, MboAction::Add, MboSide::Bid, 100, 3),
            event(2, 2, MboAction::Add, MboSide::Ask, 102, 4),
            event(3, 2, MboAction::Trade, MboSide::Bid, 102, 5),
            event(4, 2, MboAction::Cancel, MboSide::Ask, 102, 4),
        ];
        let base_config = DerivedCacheBuildConfig {
            replay: OrderFlowReplayConfig {
                checkpoint_every_events: 2,
                large_trade_threshold: 5,
                footprint_interval_ns: 10,
                heatmap_interval_ns: 1,
                heatmap_depth_limit: 1,
            },
            batch_size: 1,
            max_events: Some(2),
        };

        let first = build_derived_cache_resumable(
            &entry(),
            VecMboEventSource::new(source_info(), events.clone()),
            &dir,
            base_config,
        )
        .expect("first resumable pass");

        assert_eq!(first.processed_new_events, 2);
        assert_eq!(first.replayed_events, 0);
        assert_eq!(first.wrote_partitions, 1);
        assert!(!first.source_exhausted);
        assert_eq!(first.progress.status, DerivedCacheBuildStatus::Running);
        assert_eq!(first.progress.processed_events, 2);
        assert_eq!(first.progress.completed_partitions.len(), 1);
        assert_eq!(first.progress.kind, DERIVED_CACHE_PROGRESS_KIND);

        let source = super::DerivedCacheSourceManifest::from_entry(&entry());
        let slug = source_slug(&source);
        let progress_path = progress_manifest_path(&dir, &slug);
        let stored_progress =
            DerivedCacheProgressManifest::read_from_path(&progress_path).expect("progress");
        assert_eq!(stored_progress.processed_events, 2);
        assert!(
            !progress_path
                .with_file_name(".derived-cache-progress.v1.json.tmp")
                .exists()
        );

        let first_partition_path =
            dir.join(&stored_progress.completed_partitions[0].artifact_relative_path);
        assert!(first_partition_path.exists());
        assert!(
            first_partition_path
                .to_string_lossy()
                .ends_with(".json.zst")
        );
        let first_partition =
            super::read_partition_artifact(&dir, &stored_progress.completed_partitions[0])
                .expect("partition artifact");
        assert_eq!(first_partition.kind, DERIVED_CACHE_PARTITION_KIND);
        assert_eq!(first_partition.partition.start_event_index, 1);
        assert_eq!(first_partition.partition.end_event_index, 2);
        assert_eq!(first_partition.summary.events_read, 2);
        assert!(first_partition.artifacts.is_some());

        let resumed_config = DerivedCacheBuildConfig {
            max_events: None,
            ..base_config
        };
        let second = build_derived_cache_resumable(
            &entry(),
            VecMboEventSource::new(source_info(), events),
            &dir,
            resumed_config,
        )
        .expect("second resumable pass");

        assert_eq!(second.replayed_events, 2);
        assert_eq!(second.processed_new_events, 2);
        assert_eq!(second.wrote_partitions, 2);
        assert!(second.source_exhausted);
        assert_eq!(second.progress.status, DerivedCacheBuildStatus::Complete);
        assert_eq!(second.progress.processed_events, 4);
        assert_eq!(second.progress.completed_partitions.len(), 3);
        assert_eq!(
            second.progress.completed_partitions[1].artifact_relative_path,
            PathBuf::from(&slug)
                .join("partitions")
                .join("partition-000001.v1.json.zst")
        );
        assert_eq!(
            second.progress.completed_partitions[2].artifact_relative_path,
            PathBuf::from(&slug)
                .join("partitions")
                .join("partition-000002.v1.json.zst")
        );

        let final_progress =
            DerivedCacheProgressManifest::read_from_path(&progress_path).expect("final progress");
        assert_eq!(final_progress.status, DerivedCacheBuildStatus::Complete);
        assert_eq!(final_progress.processed_events, 4);
        let final_manifest = DerivedCacheManifest::read_from_path(
            derived_cache_manifest_path_for_entry(&dir, &entry()),
        )
        .expect("final manifest");
        assert_eq!(final_manifest.summary.events_read, 4);
        assert_eq!(final_manifest.summary.trade_prints, 1);
        let bars = query_ohlcv_bars(
            &final_manifest,
            &dir,
            OhlcvWindowQuery {
                interval_ns: MINUTE_NS,
                instrument_id: 1,
                start_ts: 0,
                end_ts: 10,
                limit: 10,
            },
        )
        .expect("final ohlcv query");
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].volume, 5);
        let heatmap = query_heatmap_cells(
            &final_manifest,
            &dir,
            super::HeatmapViewportQuery {
                instrument_id: 1,
                start_ts: 1,
                end_ts: 4,
                min_price_fixed: None,
                max_price_fixed: None,
                limit: 16,
            },
        )
        .expect("final heatmap query");
        assert!(!heatmap.is_empty());

        fs::remove_dir_all(dir).expect("cleanup resumable temp dir");
    }

    #[test]
    fn resumable_builder_emits_progress_events_for_cli_reporting() {
        let dir = std::env::temp_dir().join(format!(
            "rustchart-derived-cache-progress-events-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let events = vec![
            event(1, 1, MboAction::Add, MboSide::Bid, 100, 3),
            event(2, 2, MboAction::Add, MboSide::Ask, 102, 4),
            event(3, 2, MboAction::Trade, MboSide::Bid, 102, 5),
        ];
        let config = DerivedCacheBuildConfig {
            replay: OrderFlowReplayConfig {
                checkpoint_every_events: 2,
                large_trade_threshold: 5,
                footprint_interval_ns: 10,
                heatmap_interval_ns: 1,
                heatmap_depth_limit: 1,
            },
            batch_size: 1,
            max_events: Some(2),
        };
        let mut progress_events = Vec::new();

        let report = build_derived_cache_resumable_with_progress(
            &entry(),
            VecMboEventSource::new(source_info(), events),
            &dir,
            config,
            |event| progress_events.push(event),
        )
        .expect("progress run");

        assert_eq!(report.processed_new_events, 2);
        assert!(
            progress_events
                .iter()
                .any(|event| event.kind == DerivedCacheProgressEventKind::Started)
        );
        assert_eq!(
            progress_events
                .iter()
                .filter(|event| event.kind == DerivedCacheProgressEventKind::ProcessedBatch)
                .count(),
            2
        );
        let partition_event = progress_events
            .iter()
            .find(|event| event.kind == DerivedCacheProgressEventKind::PartitionWritten)
            .expect("partition event");
        assert_eq!(partition_event.latest_partition_index, Some(0));
        assert_eq!(partition_event.latest_partition_events, Some(2));
        assert!(partition_event.latest_partition_bytes.unwrap_or_default() > 0);
        assert!(
            progress_events
                .iter()
                .any(|event| event.kind == DerivedCacheProgressEventKind::Finished)
        );

        fs::remove_dir_all(dir).expect("cleanup progress event temp dir");
    }

    #[test]
    fn finalization_emits_progress_events_for_each_artifact() {
        let dir = std::env::temp_dir().join(format!(
            "rustchart-derived-cache-finalization-events-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let events = vec![
            event(1, 1, MboAction::Add, MboSide::Bid, 100, 3),
            event(2, 2, MboAction::Add, MboSide::Ask, 102, 4),
            event(3, 2, MboAction::Trade, MboSide::Bid, 102, 5),
        ];
        let config = DerivedCacheBuildConfig {
            replay: OrderFlowReplayConfig {
                checkpoint_every_events: 2,
                large_trade_threshold: 5,
                footprint_interval_ns: 10,
                heatmap_interval_ns: 1,
                heatmap_depth_limit: 1,
            },
            batch_size: 1,
            max_events: Some(2),
        };
        build_derived_cache_resumable(
            &entry(),
            VecMboEventSource::new(source_info(), events),
            &dir,
            config,
        )
        .expect("bounded resumable run");

        let mut finalization_events = Vec::new();
        let manifest = finalize_derived_cache_partitions_with_progress(&entry(), &dir, |event| {
            finalization_events.push(event)
        })
        .expect("finalize with progress");

        assert_eq!(manifest.summary.events_read, 2);
        assert!(
            finalization_events
                .iter()
                .any(|event| event.kind == DerivedCacheFinalizationEventKind::Started)
        );
        assert_eq!(
            finalization_events
                .iter()
                .filter(|event| event.kind == DerivedCacheFinalizationEventKind::ArtifactStarted)
                .count(),
            8
        );
        assert_eq!(
            finalization_events
                .iter()
                .filter(|event| event.kind == DerivedCacheFinalizationEventKind::ArtifactFinished)
                .count(),
            8
        );
        let heatmap_event = finalization_events
            .iter()
            .find(|event| {
                event.kind == DerivedCacheFinalizationEventKind::ArtifactFinished
                    && event.artifact_kind == Some(DerivedCacheArtifactKind::HeatmapTiles)
            })
            .expect("heatmap finalized");
        assert_eq!(heatmap_event.completed_artifacts, 3);
        assert_eq!(heatmap_event.total_artifacts, 8);
        assert!(heatmap_event.records.unwrap_or_default() > 0);
        assert!(heatmap_event.artifact_bytes.unwrap_or_default() > 0);
        assert!(finalization_events.iter().any(|event| event.kind
            == DerivedCacheFinalizationEventKind::Finished
            && event.completed_artifacts == 8));

        fs::remove_dir_all(dir).expect("cleanup finalization event temp dir");
    }

    #[test]
    #[ignore = "requires local ignored E-mini MBO DBN cache"]
    fn local_nq_resumable_cache_build_smoke() {
        let entry = local_nq_entry();
        assert!(
            entry.raw_path.exists(),
            "local ignored DBN cache is missing"
        );
        let dir = std::env::temp_dir().join(format!(
            "rustchart-derived-cache-local-smoke-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let source = HistoricalDbnMboSource::from_cache_entry(&entry).expect("DBN source");
        let config = DerivedCacheBuildConfig {
            replay: OrderFlowReplayConfig {
                checkpoint_every_events: 2,
                heatmap_depth_limit: 2,
                ..OrderFlowReplayConfig::default()
            },
            batch_size: 1,
            max_events: Some(4),
        };

        let report =
            build_derived_cache_resumable(&entry, source, &dir, config).expect("local smoke");

        assert_eq!(report.processed_new_events, 4);
        assert_eq!(report.wrote_partitions, 2);
        assert_eq!(report.progress.processed_events, 4);
        assert_eq!(report.progress.completed_partitions.len(), 2);

        fs::remove_dir_all(dir).expect("cleanup local smoke temp dir");
    }

    #[test]
    #[ignore = "requires local ignored E-mini MBO DBN cache and performs a bounded resumable artifact dry run"]
    fn local_nq_resumable_artifact_cache_dry_run() {
        const DRY_RUN_EVENTS: u64 = 50_000;

        let entry = local_nq_entry();
        assert!(
            entry.raw_path.exists(),
            "local ignored DBN cache is missing"
        );
        let dir = std::env::temp_dir().join(format!(
            "rustchart-derived-cache-local-resumable-dry-run-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let source = HistoricalDbnMboSource::from_cache_entry(&entry).expect("DBN source");
        let config = DerivedCacheBuildConfig {
            replay: OrderFlowReplayConfig {
                checkpoint_every_events: 10_000,
                heatmap_depth_limit: 12,
                ..OrderFlowReplayConfig::default()
            },
            batch_size: 10_000,
            max_events: Some(DRY_RUN_EVENTS),
        };

        let report =
            build_derived_cache_resumable(&entry, source, &dir, config).expect("local dry run");
        assert_eq!(report.processed_new_events, DRY_RUN_EVENTS);
        assert_eq!(report.progress.processed_events, DRY_RUN_EVENTS);
        assert_eq!(report.progress.completed_partitions.len(), 5);

        let manifest =
            finalize_derived_cache_partitions(&entry, &dir).expect("finalize dry-run partitions");
        let storage_report =
            derived_cache_artifact_storage_report(&manifest, &dir).expect("storage report");
        let query_instrument_id = report
            .progress
            .completed_partitions
            .iter()
            .find_map(|partition| {
                super::read_partition_artifact(&dir, partition)
                    .ok()
                    .and_then(|artifact| artifact.artifacts)
                    .and_then(|artifacts| {
                        artifacts
                            .ohlcv_bars
                            .bars
                            .first()
                            .map(|bar| bar.instrument_id)
                    })
            })
            .expect("at least one OHLCV partition bar");
        let bars = query_ohlcv_bars(
            &manifest,
            &dir,
            OhlcvWindowQuery {
                interval_ns: MINUTE_NS,
                instrument_id: query_instrument_id,
                start_ts: manifest.summary.first_event_ts.unwrap_or_default(),
                end_ts: manifest.summary.last_event_ts.unwrap_or_default(),
                limit: 16,
            },
        )
        .expect("resumable ohlcv query");

        eprintln!(
            "local NQ resumable dry run: events={} partitions={} book={} prints={} instruments={} checkpoints={} artifacts={} shards={} artifact_bytes={} first_ts={:?} last_ts={:?}",
            manifest.summary.events_read,
            report.progress.completed_partitions.len(),
            manifest.summary.applied_book_events,
            manifest.summary.trade_prints,
            manifest.summary.instrument_count,
            manifest.summary.checkpoint_count,
            storage_report.artifact_count(),
            storage_report.shard_count(),
            storage_report.total_bytes(),
            manifest.summary.first_event_ts,
            manifest.summary.last_event_ts
        );

        assert_eq!(manifest.summary.events_read, DRY_RUN_EVENTS);
        assert_eq!(manifest.summary.checkpoint_count, 5);
        assert_eq!(storage_report.artifact_count(), manifest.artifacts.len());
        assert!(storage_report.total_bytes() > 0);
        assert!(!bars.is_empty());

        fs::remove_dir_all(dir).expect("cleanup local resumable dry-run temp dir");
    }

    #[test]
    #[ignore = "requires local ignored E-mini MBO DBN cache and performs a bounded larger dry run"]
    fn local_nq_larger_artifact_cache_dry_run() {
        const DRY_RUN_EVENTS: u64 = 50_000;

        let entry = local_nq_entry();
        assert!(
            entry.raw_path.exists(),
            "local ignored DBN cache is missing"
        );
        let dir = std::env::temp_dir().join(format!(
            "rustchart-derived-cache-local-dry-run-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        let source = HistoricalDbnMboSource::from_cache_entry(&entry).expect("DBN source");
        let config = DerivedCacheBuildConfig {
            replay: OrderFlowReplayConfig {
                checkpoint_every_events: 10_000,
                heatmap_depth_limit: 12,
                ..OrderFlowReplayConfig::default()
            },
            batch_size: 10_000,
            max_events: Some(DRY_RUN_EVENTS),
        };

        let manifest =
            build_derived_cache_artifacts(&entry, source, &dir, config).expect("local dry run");
        let storage_report =
            derived_cache_artifact_storage_report(&manifest, &dir).expect("storage report");

        eprintln!(
            "local NQ dry run: events={} book={} prints={} instruments={} checkpoints={} artifacts={} shards={} artifact_bytes={} first_ts={:?} last_ts={:?}",
            manifest.summary.events_read,
            manifest.summary.applied_book_events,
            manifest.summary.trade_prints,
            manifest.summary.instrument_count,
            manifest.summary.checkpoint_count,
            storage_report.artifact_count(),
            storage_report.shard_count(),
            storage_report.total_bytes(),
            manifest.summary.first_event_ts,
            manifest.summary.last_event_ts
        );

        assert_eq!(manifest.summary.events_read, DRY_RUN_EVENTS);
        assert!(manifest.summary.applied_book_events > 0);
        assert!(manifest.summary.instrument_count > 0);
        assert!(manifest.summary.checkpoint_count >= 5);
        assert_eq!(storage_report.artifact_count(), manifest.artifacts.len());
        assert!(storage_report.total_bytes() > 0);

        fs::remove_dir_all(dir).expect("cleanup local dry-run temp dir");
    }
}
