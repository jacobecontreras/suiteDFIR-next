//! UI ↔ core IPC payloads: shared types (CONTRACTS.md §9), command requests and responses (§10,
//! §13.5), events (§11, §13.5) and errors (§12).
//!
//! Types holding a password (`RunRequest`, `AcqRequest`, `AcqRestoreEncryptionRequest`) implement
//! `Debug` by hand and print `<redacted>` instead. They derive `Serialize` only for the generated
//! examples and tests; never serialize a live request.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::{
    AcqFile, AcqPhase, AcqStatus, CaseFile, DevicePromptKind, ErrorCode, IdeviceToolSource,
    IdeviceToolsState, InputKind, InputType, InstallSource, InstallStage, JobKind, ModuleInfo,
    PairState, PlatformKey, PreflightLevel, RunFile, RunPhase, RunStatus, SettingsDefaults,
    StdStream, Timestamp, ToolId, ToolState,
};

const REDACTED: &str = "<redacted>";

fn redact(password: &Option<String>) -> Option<&'static str> {
    password.as_ref().map(|_| REDACTED)
}

// ---- §9 shared types and §12 errors ----

/// The error of every command (§12). `message` is safe to show; `detail` never holds secrets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
    pub detail: Option<String>,
}

/// A status reason or warning in records and events.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reason {
    pub code: String,
    pub message: String,
}

/// A LEAPP tool's install state (`tools_status`, `tool_verify`, `tool_install`, `tool_import`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolStatus {
    pub tool: ToolId,
    pub display_name: String,
    pub pinned_version: String,
    pub state: ToolState,
    pub installed_version: Option<String>,
    pub install_source: Option<InstallSource>,
    pub module_count: Option<u32>,
    pub install_dir: Option<String>,
    pub problem: Option<String>,
}

/// `tool_modules` response: `modules.json` without its file fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolModules {
    pub tool: ToolId,
    pub version: String,
    pub always_run: BTreeMap<String, Vec<String>>,
    pub timezones: Option<Vec<String>>,
    pub modules: Vec<ModuleInfo>,
}

/// A case in `cases_list` (from `recent_cases`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseSummary {
    pub path: String,
    pub exists: bool,
    pub case: Option<CaseFile>,
    pub run_count: u32,
    pub last_run_at: Option<Timestamp>,
}

/// `case_create`, `case_open` and `case_update` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseDetail {
    pub path: String,
    pub case: CaseFile,
    pub runs: Vec<RunSummary>,
    pub acquisitions: Vec<AcqSummary>,
    /// The run and acquisition ids marked `interrupted` by this open.
    pub recovered: Vec<String>,
}

/// A run in a case listing and in the `finished` event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    pub run_id: String,
    pub run_dir: String,
    pub label: Option<String>,
    pub status: RunStatus,
    pub tool: ToolId,
    pub tool_version: String,
    pub input_path: String,
    pub input_type: InputType,
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    pub duration_ms: Option<u64>,
    pub report_available: bool,
}

/// `input_inspect` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputInspection {
    pub path: String,
    pub kind: InputKind,
    pub size_bytes: Option<u64>,
    pub detected_type: Option<InputType>,
    pub allowed_types: Vec<InputType>,
    pub is_itunes_backup: bool,
    pub itunes_encrypted: Option<bool>,
    pub hashable: bool,
    pub warnings: Vec<String>,
}

/// How a run's modules are chosen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ModuleSelection {
    All,
    Profile { profile_name: String },
    Custom { modules: Vec<String> },
}

/// `run_start` request. `Debug` redacts `itunes_password`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRequest {
    pub case_path: String,
    pub tool: ToolId,
    pub input_path: String,
    pub input_type: InputType,
    pub modules: ModuleSelection,
    pub timezone: Option<String>,
    pub itunes_password: Option<String>,
    pub keychain_path: Option<String>,
    pub hash_input: bool,
    pub label: Option<String>,
}

impl fmt::Debug for RunRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunRequest")
            .field("case_path", &self.case_path)
            .field("tool", &self.tool)
            .field("input_path", &self.input_path)
            .field("input_type", &self.input_type)
            .field("modules", &self.modules)
            .field("timezone", &self.timezone)
            .field("itunes_password", &redact(&self.itunes_password))
            .field("keychain_path", &self.keychain_path)
            .field("hash_input", &self.hash_input)
            .field("label", &self.label)
            .finish()
    }
}

/// `job_active` response (or `null`): the one active job, app-wide.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ActiveJob {
    Run {
        case_path: String,
        run_id: String,
        tool: ToolId,
        created_at: Timestamp,
        phase: RunPhase,
    },
    Acquisition {
        case_path: String,
        acq_id: String,
        udid: String,
        created_at: Timestamp,
        phase: AcqPhase,
    },
}

/// A stored profile (`profiles_list`, `profile_save`, `profile_import`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileInfo {
    pub tool: ToolId,
    pub name: String,
    pub modules: Vec<String>,
    /// Computed against the installed module list.
    pub unknown_modules: Vec<String>,
}

/// A local Finder/iTunes backup (`ios_backups_find`, S1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IosBackup {
    pub path: String,
    pub device_name: Option<String>,
    pub product_type: Option<String>,
    pub ios_version: Option<String>,
    pub last_backup: Option<Timestamp>,
    pub encrypted: Option<bool>,
    pub size_bytes: Option<u64>,
}

// ---- §10 command requests and responses ----

/// `app_info` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    pub app_version: String,
    /// `null` on an unsupported OS/CPU pair.
    pub platform: Option<PlatformKey>,
    pub os: String,
    pub arch: String,
    pub dev_override: bool,
    pub paths: AppInfoPaths,
}

/// The app directories in `app_info`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfoPaths {
    pub app_data: String,
    pub app_config: String,
    pub app_cache: String,
    pub app_log: String,
    pub tools_dir: String,
}

/// `settings_update` request. An omitted field is left unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsUpdateRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cases_root: Option<String>,
    /// Replaces all three defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub defaults: Option<SettingsDefaults>,
    #[serde(default, skip_serializing_if = "ToolsDirUpdate::is_unchanged")]
    pub tools_dir: ToolsDirUpdate,
}

/// `settings_update` `tools_dir`: omitted = unchanged, `null` = reset to the default, a string =
/// set the override.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ToolsDirUpdate {
    #[default]
    Unchanged,
    Reset,
    Set(String),
}

impl ToolsDirUpdate {
    pub fn is_unchanged(&self) -> bool {
        matches!(self, Self::Unchanged)
    }
}

impl Serialize for ToolsDirUpdate {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            // Skipped by `skip_serializing_if`; `null` would mean Reset.
            Self::Unchanged => Err(serde::ser::Error::custom(
                "ToolsDirUpdate::Unchanged is written by omitting the field",
            )),
            Self::Reset => serializer.serialize_none(),
            Self::Set(dir) => serializer.serialize_some(dir),
        }
    }
}

impl<'de> Deserialize<'de> for ToolsDirUpdate {
    // Called only when the field is present (an omitted field takes the `Unchanged` default).
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<String>::deserialize(deserializer)? {
            None => Self::Reset,
            Some(dir) => Self::Set(dir),
        })
    }
}

/// The request of `tool_verify`, `tool_install`, `tool_modules` and `profiles_list`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRequest {
    pub tool: ToolId,
}

/// `tool_import` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolImportRequest {
    pub tool: ToolId,
    pub archive_path: String,
}

/// `case_create` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseCreateRequest {
    pub name: String,
    pub case_number: String,
    pub examiner: String,
    pub agency: String,
    pub description: String,
    pub default_timezone: Option<String>,
    /// `null` = `settings.cases_root`.
    pub parent_dir: Option<String>,
}

/// The request of `case_open`, `case_forget` and `reveal_path`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathRequest {
    pub path: String,
}

/// `case_update` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseUpdateRequest {
    pub path: String,
    pub fields: CaseFields,
}

/// The editable fields of `case.json` (§6). The folder is not renamed when `name` changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseFields {
    pub name: String,
    pub case_number: String,
    pub examiner: String,
    pub agency: String,
    pub description: String,
    pub default_timezone: Option<String>,
}

/// The request of `run_get` and `open_report`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRef {
    pub case_path: String,
    pub run_id: String,
}

/// `input_inspect` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputInspectRequest {
    pub tool: ToolId,
    pub path: String,
    pub case_path: String,
}

/// `profile_save` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSaveRequest {
    pub tool: ToolId,
    pub name: String,
    pub modules: Vec<String>,
}

/// `profile_delete` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileRef {
    pub tool: ToolId,
    pub name: String,
}

/// `profile_import` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileImportRequest {
    pub tool: ToolId,
    pub path: String,
    /// `null` = the file stem.
    pub name: Option<String>,
    pub overwrite: bool,
}

/// `profile_export` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileExportRequest {
    pub tool: ToolId,
    pub name: String,
    /// From the save dialog.
    pub dest_path: String,
}

/// `run_start` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStarted {
    pub run_id: String,
    pub run_dir: String,
}

/// `run_cancel` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCancelRequest {
    pub run_id: String,
}

/// `job_attach` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobAttachRequest {
    pub kind: JobKind,
    /// The `run_id` or `acq_id`.
    pub id: String,
}

/// `job_attach` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobBacklog {
    pub backlog: Vec<String>,
}

/// `open_text_file` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenTextFileRequest {
    pub case_path: String,
    pub run_id: String,
    pub which: RunFile,
}

/// `temp_cleanup` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TempCleanupResult {
    pub freed_byte_count: u64,
}

// ---- §11 events ----

/// Events of a run (`run_start`, `job_attach`), tagged by `type`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    Phase {
        phase: RunPhase,
    },
    /// At most 500 lines per event.
    Log {
        lines: Vec<String>,
    },
    /// Once per stream after exit: the last 200 lines.
    StdioTail {
        stream: StdStream,
        lines: Vec<String>,
    },
    HashProgress {
        bytes_done: u64,
        bytes_total: u64,
    },
    SealProgress {
        files_done: u64,
        files_total: Option<u64>,
    },
    Finished {
        status: RunStatus,
        reasons: Vec<Reason>,
        warnings: Vec<Reason>,
        summary: Box<RunSummary>,
    },
}

/// Events of `tool_install` and `tool_import`, tagged by `type`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InstallEvent {
    Stage { stage: InstallStage },
    DownloadProgress { bytes_done: u64, bytes_total: u64 },
    Message { text: String },
}

// ---- §13.5 acquisition ----

/// A connected iOS device (`devices_list`, `device_pair`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceSummary {
    pub udid: String,
    pub device_name: Option<String>,
    pub product_type: Option<String>,
    pub product_version: Option<String>,
    pub serial_number: Option<String>,
    pub pair_state: PairState,
    /// The active job's device (not queried).
    pub busy: bool,
    pub will_encrypt: Option<bool>,
    pub data_used_bytes: Option<u64>,
    pub data_capacity_bytes: Option<u64>,
    pub message: Option<String>,
}

/// `devices_list` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicesResult {
    pub tools: IdeviceToolsStatus,
    pub devices: Vec<DeviceSummary>,
}

/// Tool and usbmuxd availability in `devices_list` (reported here, never thrown).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdeviceToolsStatus {
    pub source: Option<IdeviceToolSource>,
    pub version: Option<String>,
    pub state: IdeviceToolsState,
    pub guidance: Option<String>,
}

/// `device_pair` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DevicePairRequest {
    pub udid: String,
}

/// `acq_preflight` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcqPreflightRequest {
    pub case_path: String,
    pub udid: String,
}

/// `acq_preflight` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcqPreflight {
    pub free_bytes: u64,
    pub required_bytes: Option<u64>,
    pub level: PreflightLevel,
}

/// `acq_start` request. `Debug` redacts `encryption_password`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcqRequest {
    pub case_path: String,
    pub udid: String,
    pub label: Option<String>,
    pub enable_encryption: bool,
    pub encryption_password: Option<String>,
    pub restore_encryption: bool,
}

impl fmt::Debug for AcqRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcqRequest")
            .field("case_path", &self.case_path)
            .field("udid", &self.udid)
            .field("label", &self.label)
            .field("enable_encryption", &self.enable_encryption)
            .field("encryption_password", &redact(&self.encryption_password))
            .field("restore_encryption", &self.restore_encryption)
            .finish()
    }
}

/// `acq_start` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcqStarted {
    pub acq_id: String,
    pub acq_dir: String,
}

/// `acq_cancel` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcqCancelRequest {
    pub acq_id: String,
}

/// `acq_get` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcqRef {
    pub case_path: String,
    pub acq_id: String,
}

/// `acq_restore_encryption` request. `Debug` redacts `password`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcqRestoreEncryptionRequest {
    pub case_path: String,
    pub acq_id: String,
    pub password: String,
}

impl fmt::Debug for AcqRestoreEncryptionRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcqRestoreEncryptionRequest")
            .field("case_path", &self.case_path)
            .field("acq_id", &self.acq_id)
            .field("password", &REDACTED)
            .finish()
    }
}

/// `acq_restore_encryption` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcqRestoreEncryptionResult {
    pub restored: bool,
    pub will_encrypt_after: Option<bool>,
}

/// `open_acq_file` request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenAcqFileRequest {
    pub case_path: String,
    pub acq_id: String,
    pub which: AcqFile,
}

/// An acquisition in a case listing and in the `finished` event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcqSummary {
    pub acq_id: String,
    pub acq_dir: String,
    pub label: Option<String>,
    pub status: AcqStatus,
    pub udid: String,
    pub device_name: Option<String>,
    pub product_version: Option<String>,
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    pub duration_ms: Option<u64>,
    /// The absolute path of `backup/<udid>` when succeeded.
    pub backup_path: Option<String>,
    /// Warning codes, so the Case screen can offer "Turn backup encryption off".
    pub warnings: Vec<String>,
}

/// Events of an acquisition (`acq_start`, `job_attach`), tagged by `type`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AcqEvent {
    Phase {
        phase: AcqPhase,
    },
    Log {
        lines: Vec<String>,
    },
    /// Overall percent, from `NN% Finished`; at most 4 per second.
    Progress {
        percent: u8,
    },
    /// Shown as a "watch the device" banner.
    DevicePrompt {
        kind: DevicePromptKind,
        text: String,
    },
    SealProgress {
        files_done: u64,
        files_total: Option<u64>,
    },
    Finished {
        status: AcqStatus,
        reasons: Vec<Reason>,
        warnings: Vec<Reason>,
        summary: Box<AcqSummary>,
    },
}
