//! `run.json`, the audit record of a run (CONTRACTS.md §7), and the record parts it shares with
//! `acquisition.json`.

use serde::{Deserialize, Serialize};

use super::{
    EntryVerifiedAgainst, HashAlgorithm, HashStatus, InputKind, InputType, InstallSource,
    ModuleMode, PlatformKey, Reason, RunStatus, SealStatus, Timestamp, ToolId, VersionedFile,
};

/// `runs/<run_id>/run.json` (§7.1). The initial record (§7.2) has the same shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRecord {
    pub schema_version: u32,
    /// `YYYYMMDD-HHMMSSZ-<tool>-<6 lowercase hex>` (UTC).
    pub run_id: String,
    pub label: Option<String>,
    pub status: RunStatus,
    pub status_reasons: Vec<Reason>,
    pub warnings: Vec<Reason>,
    /// When `run_start` was accepted.
    pub created_at: Timestamp,
    /// When LEAPP was spawned; `null` if it never was.
    pub started_at: Option<Timestamp>,
    /// Finalize time; `null` for `interrupted` (which sets `recovered_at` instead).
    pub ended_at: Option<Timestamp>,
    pub recovered_at: Option<Timestamp>,
    /// `ended_at − created_at`; `null` if `ended_at` is `null`.
    pub duration_ms: Option<u64>,
    pub app: RecordApp,
    pub host: RecordHost,
    pub case_snapshot: CaseSnapshot,
    pub tool: RunTool,
    pub input: RunInput,
    pub options: RunOptions,
    pub modules: RunModules,
    pub command: RunCommand,
    /// `null` until the process exit is recorded.
    pub process: Option<RunProcess>,
    /// `null` until the output is analyzed.
    pub leapp_result: Option<LeappResult>,
    pub output: RunOutput,
    pub logs: RunLogs,
}

impl VersionedFile for RunRecord {
    const FILE: &'static str = "run.json";
}

/// The app that wrote a record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordApp {
    pub name: String,
    pub version: String,
}

/// The machine a record was written on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordHost {
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub hostname: String,
}

/// The case metadata at the time of the run or acquisition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseSnapshot {
    pub case_id: String,
    pub name: String,
    pub case_number: String,
    pub examiner: String,
    pub agency: String,
}

/// The tool binary that ran.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunTool {
    pub id: ToolId,
    pub version: String,
    pub platform: PlatformKey,
    /// `null` for the dev override.
    pub asset_name: Option<String>,
    /// `null` for the dev override.
    pub asset_sha256: Option<String>,
    pub entry_sha256: String,
    pub entry_verified_against: EntryVerifiedAgainst,
    pub install_source: InstallSource,
}

/// The run's input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunInput {
    pub path: String,
    pub kind: InputKind,
    #[serde(rename = "type")]
    pub input_type: InputType,
    /// The auto-detected type; `null` if detection found none.
    pub type_detected: Option<InputType>,
    /// File inputs only.
    pub size_bytes: Option<u64>,
    /// `null` unless the input is an iTunes backup.
    pub itunes_encrypted: Option<bool>,
    /// The `acq_id` when the input lies inside a case's `acquisitions/<acq_id>/`.
    pub acquisition_id: Option<String>,
    pub hash: InputHash,
}

/// Input hashing (file inputs only, when requested).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputHash {
    pub algorithm: HashAlgorithm,
    pub status: HashStatus,
    pub value: Option<String>,
    pub started_at: Option<Timestamp>,
    pub completed_at: Option<Timestamp>,
}

/// Run options.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunOptions {
    /// Passed with `-tz` (iLEAPP); `null` when the tool has no timezone option (aLEAPP).
    pub timezone: Option<String>,
    pub timezone_supported: bool,
    /// Whether an iTunes backup password was given; the password itself is never recorded.
    pub password_supplied: bool,
    pub keychain_path: Option<String>,
    /// The keychain file, if given, is always hashed.
    pub keychain_sha256: Option<String>,
}

/// Module selection and resolution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunModules {
    pub mode: ModuleMode,
    pub profile_name: Option<String>,
    /// `[]` for mode `all`.
    pub requested: Vec<String>,
    /// Every selectable name (sorted) for mode `all`.
    pub resolved: Vec<String>,
    pub unknown: Vec<String>,
    /// The `always_run` entry for this input type (or `default`).
    pub always_run: Vec<String>,
    pub available_count: u32,
}

/// The LEAPP command line.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCommand {
    /// Verbatim, except the value after `--itunes_password`, which is `<redacted>`.
    pub argv: Vec<String>,
    pub cwd: String,
}

/// How the LEAPP process ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunProcess {
    /// `null` when the process was killed by a signal.
    pub exit_code: Option<i32>,
    /// The signal number that killed the process (Unix), else `null`.
    pub signal: Option<i32>,
    pub exited_at: Timestamp,
    pub cancel_requested: bool,
    pub escalated_to_kill: bool,
}

/// What LEAPP's output says (§7.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeappResult {
    pub lava_data_found: bool,
    /// From `_lava_data.lava`; `null` when the file is missing or unparsable.
    pub processing_status: Option<String>,
    /// From `_lava_data.lava`; `null` when the file is missing or unparsable.
    pub leapp_version_reported: Option<String>,
    pub index_html_found: bool,
    /// From `_lava_data.lava`; `null` when the file is missing or unparsable.
    pub module_counts: Option<ModuleCounts>,
    pub error_modules: Vec<String>,
}

/// Lava module entries by `module_status`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleCounts {
    pub complete: u32,
    pub error: u32,
    pub no_files_found: u32,
    pub other: u32,
}

/// The run's output folder and its manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunOutput {
    /// Relative to the run folder.
    pub report_dir: String,
    pub seal: Seal,
}

/// An output manifest (`report.sha256` or `backup.sha256`, CONTRACTS.md §8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seal {
    pub status: SealStatus,
    /// Relative to the record's folder.
    pub manifest: Option<String>,
    pub manifest_sha256: Option<String>,
    pub file_count: Option<u64>,
    pub total_bytes: Option<u64>,
}

/// Log files, relative to the run folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLogs {
    pub stdout: String,
    pub stderr: String,
    pub screen_output: String,
}
