#![allow(dead_code)] // Persistence helpers are staged before runtime load/save is wired in PERSIST-001C/D.

use std::{
    collections::BTreeMap,
    env, fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    DrawingTool, DrawingToolAccess, DrawingToolPolicy,
    chart_core::{
        ChartTheme, DrawingGroupSpec, DrawingPayload, DrawingSpec, DrawingStyle,
        FibRetracementSettings,
    },
    widget_api::{WidgetHostMode, WidgetId, WidgetKind, WidgetLifecycle},
    workspace_shell::{
        WorkspaceLayout, WorkspaceLayoutRect, WorkspaceLayoutRestoreInput, WorkspaceLayoutWidget,
    },
};

pub(crate) const STATE_DIR_ENV_VAR: &str = "RUSTCHART_STATE_DIR";
pub(crate) const USER_SETTINGS_SCHEMA_VERSION: u32 = 1;
pub(crate) const DRAWING_DOCUMENT_SCHEMA_VERSION: u32 = 1;
pub(crate) const LAYOUT_DOCUMENT_SCHEMA_VERSION: u32 = 1;
pub(crate) const WORKSPACE_LAYOUT_DOCUMENT_SCHEMA_VERSION: u32 = 1;
pub(crate) const LIVE_CHART_SESSIONS_SCHEMA_VERSION: u32 = 1;
pub(crate) const DRAWING_TOOL_TEMPLATES_SCHEMA_VERSION: u32 = 1;
pub(crate) const FIB_PRESETS_SCHEMA_VERSION: u32 = 1;
pub(crate) const CHART_PRESETS_SCHEMA_VERSION: u32 = 1;
pub(crate) const DEFAULT_DRAWING_SCOPE_ID: &str = "default-chart";
pub(crate) const DEFAULT_CHART_THEME_ID: &str = "default-dark";
pub(crate) const DEFAULT_LAYOUT_ID: &str = "default-layout";
pub(crate) const DEFAULT_WORKSPACE_LAYOUT_ID: &str = "default-workspace-layout";

const APP_DIR_NAME: &str = "rustchart";
const SETTINGS_DIR_NAME: &str = "settings";
const USER_SETTINGS_FILE_NAME: &str = "user-settings.v1.json";
const DRAWINGS_DIR_NAME: &str = "drawings";
const DRAWING_VERSION_DIR_NAME: &str = "v1";
const LAYOUTS_DIR_NAME: &str = "layouts";
const DEFAULT_LAYOUT_FILE_NAME: &str = "default-layout.v1.json";
const DEFAULT_WORKSPACE_LAYOUT_FILE_NAME: &str = "default-workspace-layout.v1.json";
const LIVE_CHART_SESSIONS_FILE_NAME: &str = "live-chart-sessions.v1.json";
const TEMPLATES_DIR_NAME: &str = "templates";
const DRAWING_TOOL_TEMPLATES_FILE_NAME: &str = "drawing-tools.v1.json";
const FIB_PRESETS_FILE_NAME: &str = "fib-presets.v1.json";
const CHART_PRESETS_FILE_NAME: &str = "chart-presets.v1.json";
const TEMP_FILE_COUNTER_START: u64 = 1;

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(TEMP_FILE_COUNTER_START);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct UserSettingsDocument {
    pub schema_version: u32,
    pub drawing_tool_policy: DrawingToolPolicyDocument,
    pub chart_theme: String,
    pub volume_visible: bool,
}

impl UserSettingsDocument {
    pub fn from_drawing_tool_policy(policy: &DrawingToolPolicy, volume_visible: bool) -> Self {
        Self {
            schema_version: USER_SETTINGS_SCHEMA_VERSION,
            drawing_tool_policy: DrawingToolPolicyDocument::from_policy(policy),
            chart_theme: DEFAULT_CHART_THEME_ID.to_string(),
            volume_visible,
        }
    }

    pub fn validate_schema(&self) -> Result<(), PersistenceSchemaError> {
        validate_schema_version(
            "user-settings",
            self.schema_version,
            USER_SETTINGS_SCHEMA_VERSION,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DrawingToolPolicyDocument {
    pub favorites: Vec<DrawingTool>,
    pub access: BTreeMap<DrawingTool, DrawingToolAccess>,
}

impl DrawingToolPolicyDocument {
    pub fn from_policy(policy: &DrawingToolPolicy) -> Self {
        Self {
            favorites: policy.favorites.clone(),
            access: policy.access_overrides.iter().copied().collect(),
        }
    }

    pub fn to_policy(&self) -> DrawingToolPolicy {
        DrawingToolPolicy {
            favorites: self.favorites.clone(),
            access_overrides: self
                .access
                .iter()
                .filter_map(|(tool, access)| {
                    (*access != DrawingToolAccess::Available).then_some((*tool, *access))
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DrawingToolTemplatesDocument {
    pub schema_version: u32,
    pub tools: BTreeMap<DrawingTool, DrawingToolTemplateDocument>,
}

impl DrawingToolTemplatesDocument {
    pub fn validate_schema(&self) -> Result<(), PersistenceSchemaError> {
        validate_schema_version(
            "drawing-tool-templates",
            self.schema_version,
            DRAWING_TOOL_TEMPLATES_SCHEMA_VERSION,
        )
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DrawingToolTemplateDocument {
    pub style: DrawingStyle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<DrawingPayload>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct FibPresetDocument {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_preset_id: Option<String>,
    pub presets: Vec<FibPresetDocumentEntry>,
}

impl FibPresetDocument {
    pub fn validate_schema(&self) -> Result<(), PersistenceSchemaError> {
        validate_schema_version(
            "fib-presets",
            self.schema_version,
            FIB_PRESETS_SCHEMA_VERSION,
        )
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct FibPresetDocumentEntry {
    pub id: String,
    pub name: String,
    pub settings: FibRetracementSettings,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ChartPresetDocument {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_preset_id: Option<String>,
    pub presets: Vec<ChartPresetDocumentEntry>,
}

impl ChartPresetDocument {
    pub fn validate_schema(&self) -> Result<(), PersistenceSchemaError> {
        validate_schema_version(
            "chart-presets",
            self.schema_version,
            CHART_PRESETS_SCHEMA_VERSION,
        )
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct ChartPresetDocumentEntry {
    pub id: String,
    pub name: String,
    pub theme: ChartTheme,
    pub volume_visible: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct WorkspaceLayoutDocument {
    pub schema_version: u32,
    pub layout_id: String,
    pub next_widget_id: u64,
    pub next_z_order: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_widget_id: Option<u64>,
    pub widgets: Vec<WorkspaceLayoutWidgetDocument>,
}

impl WorkspaceLayoutDocument {
    pub fn from_layout(layout: &WorkspaceLayout) -> Self {
        Self {
            schema_version: WORKSPACE_LAYOUT_DOCUMENT_SCHEMA_VERSION,
            layout_id: DEFAULT_WORKSPACE_LAYOUT_ID.to_string(),
            next_widget_id: layout.next_widget_id,
            next_z_order: layout.next_z_order,
            active_widget_id: layout.active_widget_id.map(WidgetId::get),
            widgets: layout
                .widgets
                .iter()
                .map(WorkspaceLayoutWidgetDocument::from_layout_widget)
                .collect(),
        }
    }

    pub fn validate_schema(&self) -> Result<(), PersistenceSchemaError> {
        validate_schema_version(
            "workspace-layout",
            self.schema_version,
            WORKSPACE_LAYOUT_DOCUMENT_SCHEMA_VERSION,
        )
    }

    pub fn to_restore_input(&self) -> Result<WorkspaceLayoutRestoreInput, WorkspaceLayoutError> {
        self.validate_schema()
            .map_err(WorkspaceLayoutError::UnsupportedSchema)?;
        let widgets = self
            .widgets
            .iter()
            .map(WorkspaceLayoutWidgetDocument::to_layout_widget)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(WorkspaceLayoutRestoreInput {
            next_widget_id: self.next_widget_id,
            next_z_order: self.next_z_order,
            active_widget_id: self.active_widget_id.map(WidgetId::new),
            widgets,
        })
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct WorkspaceLayoutWidgetDocument {
    pub id: u64,
    pub kind: String,
    pub lifecycle: String,
    pub host_mode: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub z_order: u64,
}

impl WorkspaceLayoutWidgetDocument {
    fn from_layout_widget(widget: &WorkspaceLayoutWidget) -> Self {
        Self {
            id: widget.id.get(),
            kind: widget.kind.stable_key().to_string(),
            lifecycle: widget.lifecycle.stable_key().to_string(),
            host_mode: widget.host_mode.stable_key().to_string(),
            x: widget.rect.x,
            y: widget.rect.y,
            width: widget.rect.width,
            height: widget.rect.height,
            z_order: widget.z_order,
        }
    }

    fn to_layout_widget(&self) -> Result<WorkspaceLayoutWidget, WorkspaceLayoutError> {
        let kind = WidgetKind::from_stable_key(&self.kind)
            .ok_or_else(|| WorkspaceLayoutError::UnknownWidgetKind(self.kind.clone()))?;
        let lifecycle = WidgetLifecycle::from_stable_key(&self.lifecycle)
            .ok_or_else(|| WorkspaceLayoutError::UnknownWidgetLifecycle(self.lifecycle.clone()))?;
        let host_mode = WidgetHostMode::from_stable_key(&self.host_mode)
            .ok_or_else(|| WorkspaceLayoutError::UnknownWidgetHostMode(self.host_mode.clone()))?;
        Ok(WorkspaceLayoutWidget {
            id: WidgetId::new(self.id),
            kind,
            lifecycle,
            host_mode,
            rect: WorkspaceLayoutRect::new(self.x, self.y, self.width, self.height),
            z_order: self.z_order,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct LiveChartSessionDocument {
    pub schema_version: u32,
    pub sessions: Vec<LiveChartSessionDocumentEntry>,
}

impl LiveChartSessionDocument {
    pub fn from_entries(sessions: Vec<LiveChartSessionDocumentEntry>) -> Self {
        Self {
            schema_version: LIVE_CHART_SESSIONS_SCHEMA_VERSION,
            sessions,
        }
    }

    pub fn validate_schema(&self) -> Result<(), PersistenceSchemaError> {
        validate_schema_version(
            "live-chart-sessions",
            self.schema_version,
            LIVE_CHART_SESSIONS_SCHEMA_VERSION,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct LiveChartSessionDocumentEntry {
    pub widget_id: u64,
    pub source: String,
    pub symbol: String,
    pub timeframe: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspaceLayoutError {
    UnsupportedSchema(PersistenceSchemaError),
    UnknownWidgetKind(String),
    UnknownWidgetLifecycle(String),
    UnknownWidgetHostMode(String),
}

impl fmt::Display for WorkspaceLayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSchema(source) => write!(formatter, "{source}"),
            Self::UnknownWidgetKind(value) => {
                write!(formatter, "unknown workspace widget kind {value:?}")
            }
            Self::UnknownWidgetLifecycle(value) => {
                write!(formatter, "unknown workspace widget lifecycle {value:?}")
            }
            Self::UnknownWidgetHostMode(value) => {
                write!(formatter, "unknown workspace widget host mode {value:?}")
            }
        }
    }
}

impl std::error::Error for WorkspaceLayoutError {}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DrawingDocument {
    pub schema_version: u32,
    pub scope: DrawingDocumentScope,
    pub next_drawing_id: u64,
    pub next_group_id: u64,
    pub drawings: Vec<DrawingSpec>,
    pub groups: Vec<DrawingGroupSpec>,
}

impl DrawingDocument {
    pub fn validate_schema(&self) -> Result<(), PersistenceSchemaError> {
        validate_schema_version(
            "drawing-document",
            self.schema_version,
            DRAWING_DOCUMENT_SCHEMA_VERSION,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct DrawingDocumentScope {
    pub source: String,
    pub symbol_key: String,
    pub scope_id: String,
}

impl DrawingDocumentScope {
    pub fn default_chart(source: impl Into<String>, symbol_key: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            symbol_key: symbol_key.into(),
            scope_id: DEFAULT_DRAWING_SCOPE_ID.to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoragePaths {
    root: PathBuf,
}

impl StoragePaths {
    pub fn resolve() -> Result<Self, PersistenceError> {
        Ok(Self {
            root: resolve_storage_root()?,
        })
    }

    pub fn from_root(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn user_settings_path(&self) -> PathBuf {
        self.root
            .join(SETTINGS_DIR_NAME)
            .join(USER_SETTINGS_FILE_NAME)
    }

    pub fn default_layout_path(&self) -> PathBuf {
        self.root
            .join(LAYOUTS_DIR_NAME)
            .join(DEFAULT_LAYOUT_FILE_NAME)
    }

    pub fn workspace_layout_path(&self) -> PathBuf {
        self.root
            .join(LAYOUTS_DIR_NAME)
            .join(DEFAULT_WORKSPACE_LAYOUT_FILE_NAME)
    }

    pub fn live_chart_sessions_path(&self) -> PathBuf {
        self.root
            .join(LAYOUTS_DIR_NAME)
            .join(LIVE_CHART_SESSIONS_FILE_NAME)
    }

    pub fn drawing_tool_templates_path(&self) -> PathBuf {
        self.root
            .join(TEMPLATES_DIR_NAME)
            .join(DRAWING_TOOL_TEMPLATES_FILE_NAME)
    }

    pub fn fib_presets_path(&self) -> PathBuf {
        self.root
            .join(TEMPLATES_DIR_NAME)
            .join(FIB_PRESETS_FILE_NAME)
    }

    pub fn chart_presets_path(&self) -> PathBuf {
        self.root
            .join(TEMPLATES_DIR_NAME)
            .join(CHART_PRESETS_FILE_NAME)
    }

    pub fn drawing_document_path(
        &self,
        scope: &DrawingDocumentScope,
    ) -> Result<PathBuf, PersistenceError> {
        Ok(self
            .root
            .join(DRAWINGS_DIR_NAME)
            .join(DRAWING_VERSION_DIR_NAME)
            .join(keyed_path_component("source", &scope.source)?)
            .join(keyed_path_component("symbol", &scope.symbol_key)?)
            .join(format!(
                "scope={}.json",
                encode_path_component("scope", &scope.scope_id)?
            )))
    }
}

pub(crate) fn resolve_storage_root() -> Result<PathBuf, PersistenceError> {
    storage_root_from_env_values(
        current_storage_platform(),
        &StorageEnvValues::from_process(),
    )
}

pub(crate) trait PersistenceDocument: serde::de::DeserializeOwned {
    fn validate_schema(&self) -> Result<(), PersistenceSchemaError>;
}

impl PersistenceDocument for UserSettingsDocument {
    fn validate_schema(&self) -> Result<(), PersistenceSchemaError> {
        UserSettingsDocument::validate_schema(self)
    }
}

impl PersistenceDocument for DrawingDocument {
    fn validate_schema(&self) -> Result<(), PersistenceSchemaError> {
        DrawingDocument::validate_schema(self)
    }
}

impl PersistenceDocument for DrawingToolTemplatesDocument {
    fn validate_schema(&self) -> Result<(), PersistenceSchemaError> {
        DrawingToolTemplatesDocument::validate_schema(self)
    }
}

impl PersistenceDocument for FibPresetDocument {
    fn validate_schema(&self) -> Result<(), PersistenceSchemaError> {
        FibPresetDocument::validate_schema(self)
    }
}

impl PersistenceDocument for ChartPresetDocument {
    fn validate_schema(&self) -> Result<(), PersistenceSchemaError> {
        ChartPresetDocument::validate_schema(self)
    }
}

impl PersistenceDocument for WorkspaceLayoutDocument {
    fn validate_schema(&self) -> Result<(), PersistenceSchemaError> {
        WorkspaceLayoutDocument::validate_schema(self)
    }
}

impl PersistenceDocument for LiveChartSessionDocument {
    fn validate_schema(&self) -> Result<(), PersistenceSchemaError> {
        LiveChartSessionDocument::validate_schema(self)
    }
}

pub(crate) fn read_json_document<T>(path: impl AsRef<Path>) -> Result<T, PersistenceError>
where
    T: PersistenceDocument,
{
    let path = path.as_ref();
    let bytes = fs::read(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            PersistenceError::MissingFile {
                path: path.to_path_buf(),
            }
        } else {
            io_persistence_error("read", path, error)
        }
    })?;
    let document: T =
        serde_json::from_slice(&bytes).map_err(|error| PersistenceError::InvalidJson {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    document
        .validate_schema()
        .map_err(|source| PersistenceError::UnsupportedSchema {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(document)
}

pub(crate) fn write_json_document_atomic<T>(
    path: impl AsRef<Path>,
    document: &T,
) -> Result<(), PersistenceError>
where
    T: serde::Serialize,
{
    let path = path.as_ref();
    let mut json = serde_json::to_vec_pretty(document).map_err(|error| {
        PersistenceError::JsonSerialization {
            path: path.to_path_buf(),
            message: error.to_string(),
        }
    })?;
    json.push(b'\n');
    write_bytes_atomic(path, &json)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PersistenceSchemaError {
    UnsupportedVersion {
        document: &'static str,
        found: u32,
        expected: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PersistenceError {
    MissingFile {
        path: PathBuf,
    },
    InvalidJson {
        path: PathBuf,
        message: String,
    },
    UnsupportedSchema {
        path: PathBuf,
        source: PersistenceSchemaError,
    },
    PathEncoding {
        component: &'static str,
        value: String,
        reason: &'static str,
    },
    Io {
        operation: &'static str,
        path: PathBuf,
        kind: io::ErrorKind,
        message: String,
    },
    JsonSerialization {
        path: PathBuf,
        message: String,
    },
    StorageRootUnavailable {
        platform: &'static str,
        missing: &'static str,
    },
}

impl fmt::Display for PersistenceSchemaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion {
                document,
                found,
                expected,
            } => write!(
                formatter,
                "{document} schema version {found} is unsupported; expected {expected}"
            ),
        }
    }
}

impl std::error::Error for PersistenceSchemaError {}

impl fmt::Display for PersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFile { path } => {
                write!(formatter, "missing persistence file {}", path.display())
            }
            Self::InvalidJson { path, message } => {
                write!(formatter, "invalid JSON in {}: {message}", path.display())
            }
            Self::UnsupportedSchema { path, source } => {
                write!(
                    formatter,
                    "unsupported schema in {}: {source}",
                    path.display()
                )
            }
            Self::PathEncoding {
                component,
                value,
                reason,
            } => write!(
                formatter,
                "cannot encode persistence path component {component}={value:?}: {reason}"
            ),
            Self::Io {
                operation,
                path,
                kind,
                message,
            } => write!(
                formatter,
                "persistence {operation} failed for {} ({kind:?}): {message}",
                path.display()
            ),
            Self::JsonSerialization { path, message } => {
                write!(
                    formatter,
                    "cannot serialize persistence JSON for {}: {message}",
                    path.display()
                )
            }
            Self::StorageRootUnavailable { platform, missing } => write!(
                formatter,
                "cannot resolve {platform} persistence root because {missing} is unavailable"
            ),
        }
    }
}

impl std::error::Error for PersistenceError {}

fn validate_schema_version(
    document: &'static str,
    found: u32,
    expected: u32,
) -> Result<(), PersistenceSchemaError> {
    if found == expected {
        Ok(())
    } else {
        Err(PersistenceSchemaError::UnsupportedVersion {
            document,
            found,
            expected,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StoragePlatform {
    Macos,
    Unix,
    Windows,
}

impl StoragePlatform {
    fn name(self) -> &'static str {
        match self {
            Self::Macos => "macOS",
            Self::Unix => "Unix",
            Self::Windows => "Windows",
        }
    }
}

#[derive(Debug, Clone, Default)]
struct StorageEnvValues {
    state_dir: Option<PathBuf>,
    home: Option<PathBuf>,
    xdg_data_home: Option<PathBuf>,
    appdata: Option<PathBuf>,
}

impl StorageEnvValues {
    fn from_process() -> Self {
        Self {
            state_dir: env::var_os(STATE_DIR_ENV_VAR).map(PathBuf::from),
            home: env::var_os("HOME").map(PathBuf::from),
            xdg_data_home: env::var_os("XDG_DATA_HOME").map(PathBuf::from),
            appdata: env::var_os("APPDATA").map(PathBuf::from),
        }
    }
}

fn current_storage_platform() -> StoragePlatform {
    #[cfg(target_os = "macos")]
    {
        StoragePlatform::Macos
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        StoragePlatform::Unix
    }
    #[cfg(windows)]
    {
        StoragePlatform::Windows
    }
    #[cfg(not(any(unix, windows)))]
    {
        StoragePlatform::Unix
    }
}

fn storage_root_from_env_values(
    platform: StoragePlatform,
    env_values: &StorageEnvValues,
) -> Result<PathBuf, PersistenceError> {
    if let Some(path) = &env_values.state_dir {
        if path.as_os_str().is_empty() {
            return Err(PersistenceError::StorageRootUnavailable {
                platform: platform.name(),
                missing: STATE_DIR_ENV_VAR,
            });
        }
        return Ok(path.clone());
    }

    match platform {
        StoragePlatform::Macos => env_path(&env_values.home)
            .map(|home| {
                home.join("Library")
                    .join("Application Support")
                    .join(APP_DIR_NAME)
            })
            .ok_or(PersistenceError::StorageRootUnavailable {
                platform: platform.name(),
                missing: "HOME",
            }),
        StoragePlatform::Unix => {
            if let Some(xdg_data_home) = env_path(&env_values.xdg_data_home) {
                Ok(xdg_data_home.join(APP_DIR_NAME))
            } else {
                env_path(&env_values.home)
                    .map(|home| home.join(".local").join("share").join(APP_DIR_NAME))
                    .ok_or(PersistenceError::StorageRootUnavailable {
                        platform: platform.name(),
                        missing: "XDG_DATA_HOME or HOME",
                    })
            }
        }
        StoragePlatform::Windows => env_path(&env_values.appdata)
            .map(|appdata| appdata.join(APP_DIR_NAME))
            .ok_or(PersistenceError::StorageRootUnavailable {
                platform: platform.name(),
                missing: "APPDATA",
            }),
    }
}

fn env_path(value: &Option<PathBuf>) -> Option<&PathBuf> {
    value.as_ref().filter(|path| !path.as_os_str().is_empty())
}

fn keyed_path_component(component: &'static str, value: &str) -> Result<String, PersistenceError> {
    Ok(format!(
        "{component}={}",
        encode_path_component(component, value)?
    ))
}

fn encode_path_component(component: &'static str, value: &str) -> Result<String, PersistenceError> {
    if value.is_empty() {
        return Err(PersistenceError::PathEncoding {
            component,
            value: value.to_string(),
            reason: "value is empty",
        });
    }

    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => {
                encoded.push(byte as char);
            }
            _ => {
                use std::fmt::Write as _;
                write!(&mut encoded, "%{byte:02X}").expect("write to String");
            }
        }
    }

    Ok(encoded)
}

fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<(), PersistenceError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| PersistenceError::Io {
            operation: "resolve parent",
            path: path.to_path_buf(),
            kind: io::ErrorKind::InvalidInput,
            message: "target path must include a parent directory".to_string(),
        })?;
    fs::create_dir_all(parent)
        .map_err(|error| io_persistence_error("create directory", parent, error))?;

    let temp_path = sibling_temp_path(path);
    let write_result = (|| -> Result<(), PersistenceError> {
        let mut temp_file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|error| io_persistence_error("create temp file", &temp_path, error))?;
        temp_file
            .write_all(bytes)
            .map_err(|error| io_persistence_error("write temp file", &temp_path, error))?;
        temp_file
            .flush()
            .map_err(|error| io_persistence_error("flush temp file", &temp_path, error))?;
        temp_file
            .sync_all()
            .map_err(|error| io_persistence_error("sync temp file", &temp_path, error))?;
        drop(temp_file);
        fs::rename(&temp_path, path)
            .map_err(|error| io_persistence_error("rename temp file", path, error))?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }

    write_result
}

fn sibling_temp_path(target: &Path) -> PathBuf {
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let file_name = target
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .unwrap_or("document.json");
    let now_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".{file_name}.tmp-{}-{now_nanos}-{counter}",
        std::process::id()
    ))
}

fn io_persistence_error(
    operation: &'static str,
    path: &Path,
    error: io::Error,
) -> PersistenceError {
    PersistenceError::Io {
        operation,
        path: path.to_path_buf(),
        kind: error.kind(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DrawingTool,
        chart_core::{
            DrawingAnchor, DrawingGroupId, DrawingId, DrawingKind, DrawingPayload, PaneId,
        },
    };
    use std::{fs, path::Component};

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(1);

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let counter = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!(
                "rustchart-persistence-{name}-{}-{counter}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create test dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn sample_user_settings(volume_visible: bool) -> UserSettingsDocument {
        UserSettingsDocument::from_drawing_tool_policy(
            &DrawingToolPolicy::default(),
            volume_visible,
        )
    }

    #[test]
    fn persist_user_settings_round_trips_drawing_tool_policy() {
        let mut policy = DrawingToolPolicy::default();
        policy.set_favorite(DrawingTool::Ray, true);
        policy.set_access(DrawingTool::Text, DrawingToolAccess::Disabled);
        policy.set_access(DrawingTool::PriceNote, DrawingToolAccess::Hidden);

        let document = UserSettingsDocument::from_drawing_tool_policy(&policy, false);
        let json = serde_json::to_string_pretty(&document).expect("serialize settings");
        let parsed: UserSettingsDocument =
            serde_json::from_str(&json).expect("deserialize settings");

        parsed.validate_schema().expect("schema version");
        assert_eq!(parsed, document);
        assert_eq!(parsed.drawing_tool_policy.to_policy(), policy);
        assert!(json.contains("PriceNote"));
        assert!(json.contains("Hidden"));
    }

    #[test]
    fn persist_drawing_document_round_trips_scope_drawings_and_groups() {
        let first = DrawingId::new(7);
        let second = DrawingId::new(8);
        let group = DrawingGroupId::new(2);
        let first_spec = DrawingSpec::new(
            first,
            DrawingKind::Rectangle,
            PaneId::Main,
            vec![
                DrawingAnchor::LogicalPrice {
                    logical: 1.0,
                    price: 100.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 3.0,
                    price: 105.0,
                },
            ],
        )
        .with_symbol("YFinance:AAPL")
        .with_z_order(4);
        let second_spec = DrawingSpec::new(
            second,
            DrawingKind::HorizontalLine,
            PaneId::Main,
            vec![DrawingAnchor::PriceOnly { price: 102.0 }],
        )
        .with_symbol("YFinance:AAPL")
        .with_z_order(5);
        let document = DrawingDocument {
            schema_version: DRAWING_DOCUMENT_SCHEMA_VERSION,
            scope: DrawingDocumentScope::default_chart("YFinance", "YFinance:AAPL"),
            next_drawing_id: 9,
            next_group_id: 3,
            drawings: vec![first_spec, second_spec],
            groups: vec![DrawingGroupSpec {
                id: group,
                pane: PaneId::Main,
                symbol: Some("YFinance:AAPL".to_string()),
                drawing_ids: vec![first, second],
            }],
        };

        let json = serde_json::to_string_pretty(&document).expect("serialize drawings");
        let parsed: DrawingDocument = serde_json::from_str(&json).expect("deserialize drawings");

        parsed.validate_schema().expect("schema version");
        assert_eq!(parsed, document);
        assert_eq!(parsed.scope.scope_id, DEFAULT_DRAWING_SCOPE_ID);
        assert_eq!(parsed.drawings.len(), 2);
        assert_eq!(parsed.groups[0].drawing_ids, vec![first, second]);
    }

    #[test]
    fn persist_drawing_document_round_trips_brush_geometry() {
        let brush = DrawingSpec::new(
            DrawingId::new(12),
            DrawingKind::Brush,
            PaneId::Main,
            vec![
                DrawingAnchor::LogicalPrice {
                    logical: 1.0,
                    price: 100.0,
                },
                DrawingAnchor::TimePrice {
                    time: 1_700_000_060,
                    price: 101.5,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 2.5,
                    price: 99.8,
                },
            ],
        );
        let document = DrawingDocument {
            schema_version: DRAWING_DOCUMENT_SCHEMA_VERSION,
            scope: DrawingDocumentScope::default_chart("YFinance", "YFinance:AAPL"),
            next_drawing_id: 13,
            next_group_id: 1,
            drawings: vec![brush.clone()],
            groups: Vec::new(),
        };

        let json = serde_json::to_string_pretty(&document).expect("serialize brush drawing");
        let parsed: DrawingDocument = serde_json::from_str(&json).expect("deserialize brush");

        assert_eq!(parsed.drawings, vec![brush]);
    }

    #[test]
    fn persist_drawing_document_round_trips_fib_retracement_payload() {
        let mut fib = DrawingSpec::new(
            DrawingId::new(14),
            DrawingKind::FibRetracement,
            PaneId::Main,
            vec![
                DrawingAnchor::LogicalPrice {
                    logical: 1.0,
                    price: 100.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 3.0,
                    price: 110.0,
                },
            ],
        );
        let Some(DrawingPayload::FibRetracement(settings)) = fib.payload.as_mut() else {
            panic!("fib settings");
        };
        settings.reverse = true;
        settings.label_position = crate::chart_core::FibRetracementLabelPosition::Below;
        settings.label_side = crate::chart_core::FibRetracementLabelSide::Right;
        settings.background_visible = true;
        settings.background_color = crate::chart_core::ChartColor::rgba(41, 98, 255, 42);
        settings
            .levels
            .push(crate::chart_core::FibRetracementLevel::new(
                1.272,
                crate::chart_core::ChartColor::rgb(255, 152, 0),
            ));
        assert!(matches!(
            &fib.payload,
            Some(DrawingPayload::FibRetracement(settings))
                if settings.reverse && settings.background_visible && settings.levels.len() >= 8
        ));
        let document = DrawingDocument {
            schema_version: DRAWING_DOCUMENT_SCHEMA_VERSION,
            scope: DrawingDocumentScope::default_chart("YFinance", "YFinance:AAPL"),
            next_drawing_id: 15,
            next_group_id: 1,
            drawings: vec![fib.clone()],
            groups: Vec::new(),
        };

        let json = serde_json::to_string_pretty(&document).expect("serialize fib drawing");
        let parsed: DrawingDocument = serde_json::from_str(&json).expect("deserialize fib");

        assert_eq!(parsed.drawings, vec![fib]);
        assert!(json.contains("FibRetracement"));
        assert!(json.contains("background_visible"));
    }

    #[test]
    fn persist_drawing_document_round_trips_position_payload() {
        let mut position = DrawingSpec::new(
            DrawingId::new(16),
            DrawingKind::LongPosition,
            PaneId::Main,
            vec![
                DrawingAnchor::LogicalPrice {
                    logical: 2.0,
                    price: 100.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 12.0,
                    price: 112.0,
                },
                DrawingAnchor::LogicalPrice {
                    logical: 12.0,
                    price: 94.0,
                },
            ],
        );
        position.payload = Some(DrawingPayload::Position(
            crate::chart_core::PositionDrawingSettings {
                labels_visible: true,
                prices_visible: false,
                profit_fill_color: crate::chart_core::ChartColor::rgba(10, 180, 120, 64),
                risk_fill_color: crate::chart_core::ChartColor::rgba(220, 60, 80, 72),
            },
        ));
        let document = DrawingDocument {
            schema_version: DRAWING_DOCUMENT_SCHEMA_VERSION,
            scope: DrawingDocumentScope::default_chart("YFinance", "YFinance:AAPL"),
            next_drawing_id: 17,
            next_group_id: 1,
            drawings: vec![position.clone()],
            groups: Vec::new(),
        };

        let json = serde_json::to_string_pretty(&document).expect("serialize position drawing");
        let parsed: DrawingDocument = serde_json::from_str(&json).expect("deserialize position");

        assert_eq!(parsed.drawings, vec![position]);
        assert!(json.contains("LongPosition"));
        assert!(json.contains("Position"));
    }

    #[test]
    fn persist_drawing_tool_templates_round_trip_styles() {
        let mut style = DrawingStyle::default();
        style.stroke_color = crate::chart_core::ChartColor::rgb(41, 98, 255);
        style.stroke_width = 3.0;
        style.fill_color = Some(crate::chart_core::ChartColor::rgba(41, 98, 255, 64));
        let document = DrawingToolTemplatesDocument {
            schema_version: DRAWING_TOOL_TEMPLATES_SCHEMA_VERSION,
            tools: BTreeMap::from([(
                DrawingTool::Rectangle,
                DrawingToolTemplateDocument {
                    style: style.clone(),
                    payload: None,
                },
            )]),
        };

        let json = serde_json::to_string_pretty(&document).expect("serialize templates");
        let parsed: DrawingToolTemplatesDocument =
            serde_json::from_str(&json).expect("deserialize templates");

        parsed.validate_schema().expect("schema version");
        assert_eq!(parsed, document);
        assert_eq!(
            parsed
                .tools
                .get(&DrawingTool::Rectangle)
                .map(|template| &template.style),
            Some(&style)
        );
    }

    #[test]
    fn persist_fib_presets_round_trip_named_settings() {
        let mut settings = FibRetracementSettings {
            reverse: true,
            background_visible: true,
            ..Default::default()
        };
        settings
            .levels
            .push(crate::chart_core::FibRetracementLevel::new(
                1.618,
                crate::chart_core::ChartColor::rgb(255, 152, 0),
            ));
        let document = FibPresetDocument {
            schema_version: FIB_PRESETS_SCHEMA_VERSION,
            active_preset_id: Some("fib-1".to_string()),
            presets: vec![FibPresetDocumentEntry {
                id: "fib-1".to_string(),
                name: "Extension set".to_string(),
                settings: settings.clone(),
                created_at: 1_700_000_000,
                updated_at: 1_700_000_100,
            }],
        };

        let json = serde_json::to_string_pretty(&document).expect("serialize fib presets");
        let parsed: FibPresetDocument =
            serde_json::from_str(&json).expect("deserialize fib presets");

        parsed.validate_schema().expect("schema version");
        assert_eq!(parsed, document);
        assert!(json.contains("Extension set"));
        assert!(json.contains("1.618"));
    }

    #[test]
    fn persist_chart_presets_round_trip_theme_settings() {
        let mut theme = ChartTheme::default();
        theme.candles.up_body = crate::chart_core::ChartColor::rgb(12, 34, 56);
        theme.appearance.grid.visible = false;
        let document = ChartPresetDocument {
            schema_version: CHART_PRESETS_SCHEMA_VERSION,
            active_preset_id: Some("chart-1".to_string()),
            presets: vec![ChartPresetDocumentEntry {
                id: "chart-1".to_string(),
                name: "Muted chart".to_string(),
                theme: theme.clone(),
                volume_visible: false,
                created_at: 1_700_000_000,
                updated_at: 1_700_000_100,
            }],
        };

        let json = serde_json::to_string_pretty(&document).expect("serialize chart presets");
        let parsed: ChartPresetDocument =
            serde_json::from_str(&json).expect("deserialize chart presets");

        parsed.validate_schema().expect("schema version");
        assert_eq!(parsed, document);
        assert!(json.contains("Muted chart"));
        assert!(json.contains("up_body"));
        assert!(json.contains("volume_visible"));
    }

    #[test]
    fn persist_workspace_layout_round_trips_restore_input() {
        let layout = WorkspaceLayout {
            next_widget_id: 3,
            next_z_order: 8,
            active_widget_id: Some(WidgetId::new(2)),
            widgets: vec![
                WorkspaceLayoutWidget {
                    id: WidgetId::new(1),
                    kind: WidgetKind::LiveChart,
                    lifecycle: WidgetLifecycle::Minimized,
                    host_mode: WidgetHostMode::InShellWorkspaceSurface,
                    rect: WorkspaceLayoutRect::new(64.0, 78.0, 760.0, 440.0),
                    z_order: 1,
                },
                WorkspaceLayoutWidget {
                    id: WidgetId::new(2),
                    kind: WidgetKind::SyncedTimeframes,
                    lifecycle: WidgetLifecycle::Open,
                    host_mode: WidgetHostMode::DetachedNativeViewport,
                    rect: WorkspaceLayoutRect::new(92.0, 106.0, 1_160.0, 520.0),
                    z_order: 7,
                },
            ],
        };
        let document = WorkspaceLayoutDocument::from_layout(&layout);
        let json = serde_json::to_string_pretty(&document).expect("serialize workspace layout");
        let parsed: WorkspaceLayoutDocument =
            serde_json::from_str(&json).expect("deserialize workspace layout");

        parsed.validate_schema().expect("schema version");
        assert_eq!(parsed.layout_id, DEFAULT_WORKSPACE_LAYOUT_ID);
        assert!(json.contains("live-chart"));
        assert!(json.contains("synced-timeframes"));
        assert!(json.contains("detached-native-viewport"));
        assert!(!json.contains("market_data"));
        assert_eq!(
            WorkspaceLayout::from(parsed.to_restore_input().expect("restore input")),
            layout
        );
    }

    #[test]
    fn persist_workspace_layout_rejects_unknown_restore_keys() {
        let layout = WorkspaceLayout {
            next_widget_id: 2,
            next_z_order: 2,
            active_widget_id: Some(WidgetId::new(1)),
            widgets: vec![WorkspaceLayoutWidget {
                id: WidgetId::new(1),
                kind: WidgetKind::LiveChart,
                lifecycle: WidgetLifecycle::Open,
                host_mode: WidgetHostMode::InShellWorkspaceSurface,
                rect: WorkspaceLayoutRect::new(64.0, 78.0, 760.0, 440.0),
                z_order: 1,
            }],
        };

        let mut document = WorkspaceLayoutDocument::from_layout(&layout);
        document.widgets[0].kind = "missing-widget-kind".to_string();
        assert_eq!(
            document.to_restore_input(),
            Err(WorkspaceLayoutError::UnknownWidgetKind(
                "missing-widget-kind".to_string()
            ))
        );

        let mut document = WorkspaceLayoutDocument::from_layout(&layout);
        document.widgets[0].lifecycle = "hidden".to_string();
        assert_eq!(
            document.to_restore_input(),
            Err(WorkspaceLayoutError::UnknownWidgetLifecycle(
                "hidden".to_string()
            ))
        );

        let mut document = WorkspaceLayoutDocument::from_layout(&layout);
        document.widgets[0].host_mode = "external-display".to_string();
        assert_eq!(
            document.to_restore_input(),
            Err(WorkspaceLayoutError::UnknownWidgetHostMode(
                "external-display".to_string()
            ))
        );
    }

    #[test]
    fn persist_live_chart_sessions_round_trips_widget_local_state() {
        let document = LiveChartSessionDocument::from_entries(vec![
            LiveChartSessionDocumentEntry {
                widget_id: 1,
                source: "yfinance".to_string(),
                symbol: "NQH6".to_string(),
                timeframe: "5m".to_string(),
            },
            LiveChartSessionDocumentEntry {
                widget_id: 2,
                source: "binance".to_string(),
                symbol: "BTCUSDT".to_string(),
                timeframe: "1h".to_string(),
            },
        ]);
        let json = serde_json::to_string_pretty(&document).expect("serialize sessions");
        let parsed: LiveChartSessionDocument =
            serde_json::from_str(&json).expect("deserialize sessions");

        parsed.validate_schema().expect("schema version");
        assert!(json.contains("NQH6"));
        assert!(json.contains("BTCUSDT"));
        assert_eq!(parsed, document);
    }

    #[test]
    fn persist_schema_versions_are_rejected_at_document_boundary() {
        let settings = UserSettingsDocument {
            schema_version: USER_SETTINGS_SCHEMA_VERSION + 1,
            drawing_tool_policy: DrawingToolPolicyDocument::from_policy(
                &DrawingToolPolicy::default(),
            ),
            chart_theme: DEFAULT_CHART_THEME_ID.to_string(),
            volume_visible: true,
        };
        assert_eq!(
            settings.validate_schema(),
            Err(PersistenceSchemaError::UnsupportedVersion {
                document: "user-settings",
                found: USER_SETTINGS_SCHEMA_VERSION + 1,
                expected: USER_SETTINGS_SCHEMA_VERSION,
            })
        );

        let drawing_document = DrawingDocument {
            schema_version: DRAWING_DOCUMENT_SCHEMA_VERSION + 1,
            scope: DrawingDocumentScope::default_chart("YFinance", "YFinance:AAPL"),
            next_drawing_id: 1,
            next_group_id: 1,
            drawings: Vec::new(),
            groups: Vec::new(),
        };
        assert_eq!(
            drawing_document.validate_schema(),
            Err(PersistenceSchemaError::UnsupportedVersion {
                document: "drawing-document",
                found: DRAWING_DOCUMENT_SCHEMA_VERSION + 1,
                expected: DRAWING_DOCUMENT_SCHEMA_VERSION,
            })
        );

        let drawing_tool_templates = DrawingToolTemplatesDocument {
            schema_version: DRAWING_TOOL_TEMPLATES_SCHEMA_VERSION + 1,
            tools: BTreeMap::new(),
        };
        assert_eq!(
            drawing_tool_templates.validate_schema(),
            Err(PersistenceSchemaError::UnsupportedVersion {
                document: "drawing-tool-templates",
                found: DRAWING_TOOL_TEMPLATES_SCHEMA_VERSION + 1,
                expected: DRAWING_TOOL_TEMPLATES_SCHEMA_VERSION,
            })
        );

        let fib_presets = FibPresetDocument {
            schema_version: FIB_PRESETS_SCHEMA_VERSION + 1,
            active_preset_id: None,
            presets: Vec::new(),
        };
        assert_eq!(
            fib_presets.validate_schema(),
            Err(PersistenceSchemaError::UnsupportedVersion {
                document: "fib-presets",
                found: FIB_PRESETS_SCHEMA_VERSION + 1,
                expected: FIB_PRESETS_SCHEMA_VERSION,
            })
        );

        let chart_presets = ChartPresetDocument {
            schema_version: CHART_PRESETS_SCHEMA_VERSION + 1,
            active_preset_id: None,
            presets: Vec::new(),
        };
        assert_eq!(
            chart_presets.validate_schema(),
            Err(PersistenceSchemaError::UnsupportedVersion {
                document: "chart-presets",
                found: CHART_PRESETS_SCHEMA_VERSION + 1,
                expected: CHART_PRESETS_SCHEMA_VERSION,
            })
        );

        let workspace_layout = WorkspaceLayoutDocument {
            schema_version: WORKSPACE_LAYOUT_DOCUMENT_SCHEMA_VERSION + 1,
            layout_id: DEFAULT_WORKSPACE_LAYOUT_ID.to_string(),
            next_widget_id: 1,
            next_z_order: 1,
            active_widget_id: None,
            widgets: Vec::new(),
        };
        assert_eq!(
            workspace_layout.validate_schema(),
            Err(PersistenceSchemaError::UnsupportedVersion {
                document: "workspace-layout",
                found: WORKSPACE_LAYOUT_DOCUMENT_SCHEMA_VERSION + 1,
                expected: WORKSPACE_LAYOUT_DOCUMENT_SCHEMA_VERSION,
            })
        );

        let live_chart_sessions = LiveChartSessionDocument {
            schema_version: LIVE_CHART_SESSIONS_SCHEMA_VERSION + 1,
            sessions: Vec::new(),
        };
        assert_eq!(
            live_chart_sessions.validate_schema(),
            Err(PersistenceSchemaError::UnsupportedVersion {
                document: "live-chart-sessions",
                found: LIVE_CHART_SESSIONS_SCHEMA_VERSION + 1,
                expected: LIVE_CHART_SESSIONS_SCHEMA_VERSION,
            })
        );
    }

    #[test]
    fn persist_storage_root_prefers_override_before_platform_defaults() {
        let test_dir = TestDir::new("root-override");
        let env_values = StorageEnvValues {
            state_dir: Some(test_dir.path().to_path_buf()),
            home: Some(PathBuf::from("/Users/example")),
            xdg_data_home: Some(PathBuf::from("/tmp/xdg")),
            appdata: Some(PathBuf::from("C:\\Users\\example\\AppData\\Roaming")),
        };

        assert_eq!(
            storage_root_from_env_values(StoragePlatform::Macos, &env_values),
            Ok(test_dir.path().to_path_buf())
        );
        assert_eq!(
            storage_root_from_env_values(StoragePlatform::Unix, &env_values),
            Ok(test_dir.path().to_path_buf())
        );
        assert_eq!(
            storage_root_from_env_values(StoragePlatform::Windows, &env_values),
            Ok(test_dir.path().to_path_buf())
        );
    }

    #[test]
    fn persist_storage_root_uses_platform_defaults() {
        let home = PathBuf::from("/Users/example");
        let xdg_data_home = PathBuf::from("/tmp/rustchart-xdg");
        let appdata = PathBuf::from("C:\\Users\\example\\AppData\\Roaming");
        let env_values = StorageEnvValues {
            state_dir: None,
            home: Some(home.clone()),
            xdg_data_home: Some(xdg_data_home.clone()),
            appdata: Some(appdata.clone()),
        };

        assert_eq!(
            storage_root_from_env_values(StoragePlatform::Macos, &env_values),
            Ok(home
                .join("Library")
                .join("Application Support")
                .join(APP_DIR_NAME))
        );
        assert_eq!(
            storage_root_from_env_values(StoragePlatform::Unix, &env_values),
            Ok(xdg_data_home.join(APP_DIR_NAME))
        );
        assert_eq!(
            storage_root_from_env_values(StoragePlatform::Windows, &env_values),
            Ok(appdata.join(APP_DIR_NAME))
        );

        let without_xdg = StorageEnvValues {
            xdg_data_home: None,
            ..env_values
        };
        assert_eq!(
            storage_root_from_env_values(StoragePlatform::Unix, &without_xdg),
            Ok(PathBuf::from("/Users/example")
                .join(".local")
                .join("share")
                .join(APP_DIR_NAME))
        );
    }

    #[test]
    fn persist_path_builders_encode_storage_key_components() {
        let test_dir = TestDir::new("paths");
        let paths = StoragePaths::from_root(test_dir.path());
        assert_eq!(
            paths.user_settings_path(),
            test_dir
                .path()
                .join(SETTINGS_DIR_NAME)
                .join(USER_SETTINGS_FILE_NAME)
        );
        assert_eq!(
            paths.default_layout_path(),
            test_dir
                .path()
                .join(LAYOUTS_DIR_NAME)
                .join(DEFAULT_LAYOUT_FILE_NAME)
        );
        assert_eq!(
            paths.workspace_layout_path(),
            test_dir
                .path()
                .join(LAYOUTS_DIR_NAME)
                .join(DEFAULT_WORKSPACE_LAYOUT_FILE_NAME)
        );
        assert_eq!(
            paths.live_chart_sessions_path(),
            test_dir
                .path()
                .join(LAYOUTS_DIR_NAME)
                .join(LIVE_CHART_SESSIONS_FILE_NAME)
        );
        assert_eq!(
            paths.drawing_tool_templates_path(),
            test_dir
                .path()
                .join(TEMPLATES_DIR_NAME)
                .join(DRAWING_TOOL_TEMPLATES_FILE_NAME)
        );
        assert_eq!(
            paths.fib_presets_path(),
            test_dir
                .path()
                .join(TEMPLATES_DIR_NAME)
                .join(FIB_PRESETS_FILE_NAME)
        );
        assert_eq!(
            paths.chart_presets_path(),
            test_dir
                .path()
                .join(TEMPLATES_DIR_NAME)
                .join(CHART_PRESETS_FILE_NAME)
        );

        let scope = DrawingDocumentScope {
            source: "Y/Finance\n".to_string(),
            symbol_key: "YFinance:AAPL/../../MSFT".to_string(),
            scope_id: "default chart/1".to_string(),
        };
        let drawing_path = paths
            .drawing_document_path(&scope)
            .expect("drawing document path");
        let relative_parts: Vec<String> = drawing_path
            .strip_prefix(test_dir.path())
            .expect("relative path")
            .components()
            .filter_map(|component| match component {
                Component::Normal(value) => Some(value.to_string_lossy().to_string()),
                _ => None,
            })
            .collect();

        assert_eq!(
            relative_parts,
            vec![
                "drawings",
                "v1",
                "source=Y%2FFinance%0A",
                "symbol=YFinance%3AAAPL%2F..%2F..%2FMSFT",
                "scope=default%20chart%2F1.json",
            ]
        );
    }

    #[test]
    fn persist_path_builders_reject_empty_storage_key_components() {
        let paths = StoragePaths::from_root("/tmp/rustchart-state");
        let scope = DrawingDocumentScope {
            source: "YFinance".to_string(),
            symbol_key: String::new(),
            scope_id: DEFAULT_DRAWING_SCOPE_ID.to_string(),
        };

        assert!(matches!(
            paths.drawing_document_path(&scope),
            Err(PersistenceError::PathEncoding {
                component: "symbol",
                reason: "value is empty",
                ..
            })
        ));
    }

    #[test]
    fn persist_atomic_write_leaves_valid_target_document() {
        let test_dir = TestDir::new("atomic-write");
        let path = StoragePaths::from_root(test_dir.path()).user_settings_path();
        let first = sample_user_settings(false);
        let second = sample_user_settings(true);

        write_json_document_atomic(&path, &first).expect("write first settings");
        assert_eq!(
            read_json_document::<UserSettingsDocument>(&path).expect("read first settings"),
            first
        );

        write_json_document_atomic(&path, &second).expect("write second settings");
        assert_eq!(
            read_json_document::<UserSettingsDocument>(&path).expect("read second settings"),
            second
        );

        let temp_files: Vec<_> = fs::read_dir(path.parent().expect("settings parent"))
            .expect("read settings dir")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(temp_files.is_empty());
    }

    #[test]
    fn persist_read_distinguishes_missing_invalid_json_and_schema_errors() {
        let test_dir = TestDir::new("read-errors");
        let paths = StoragePaths::from_root(test_dir.path());
        let missing_path = paths.user_settings_path();
        assert!(matches!(
            read_json_document::<UserSettingsDocument>(&missing_path),
            Err(PersistenceError::MissingFile { path }) if path == missing_path
        ));

        let invalid_path = test_dir.path().join("settings").join("invalid.json");
        fs::create_dir_all(invalid_path.parent().expect("invalid parent")).expect("create parent");
        fs::write(&invalid_path, b"{not-json").expect("write invalid JSON");
        assert!(matches!(
            read_json_document::<UserSettingsDocument>(&invalid_path),
            Err(PersistenceError::InvalidJson { path, .. }) if path == invalid_path
        ));

        let schema_path = test_dir.path().join("settings").join("schema.json");
        let mut wrong_schema = sample_user_settings(true);
        wrong_schema.schema_version = USER_SETTINGS_SCHEMA_VERSION + 1;
        write_json_document_atomic(&schema_path, &wrong_schema).expect("write schema document");
        assert!(matches!(
            read_json_document::<UserSettingsDocument>(&schema_path),
            Err(PersistenceError::UnsupportedSchema {
                path,
                source: PersistenceSchemaError::UnsupportedVersion {
                    document: "user-settings",
                    found,
                    expected,
                },
            }) if path == schema_path
                && found == USER_SETTINGS_SCHEMA_VERSION + 1
                && expected == USER_SETTINGS_SCHEMA_VERSION
        ));
    }
}
