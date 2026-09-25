//! iOS acquisition files: `idevice-tools.json` (CONTRACTS.md §13.2), `acquisition.json` (§13.3)
//! and the later-restore files `encryption-restore[-N].json` (§13.3, "Later restore").

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    AcqCommandPurpose, AcqStatus, CaseSnapshot, DeviceChangeKind, IdeviceToolSource,
    PasswordChannel, PlatformKey, Reason, RecordApp, RecordHost, RestoreState, Seal, Timestamp,
    ToolVerification, VersionedFile,
};

/// `idevice-tools.json` at the repo root, embedded at build time (§13.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdeviceToolsManifest {
    pub schema_version: u32,
    /// The libimobiledevice version.
    pub version: String,
    /// Every source tarball the build consumes.
    pub sources: Vec<SourceTarball>,
    /// Pinned hashes of the unsigned build outputs per platform.
    pub platforms: BTreeMap<PlatformKey, ToolBundle>,
    /// Platforms that use the tools found on `PATH` (hashes recorded).
    pub system_platforms: Vec<PlatformKey>,
}

impl VersionedFile for IdeviceToolsManifest {
    const FILE: &'static str = "idevice-tools.json";
}

/// A pinned upstream source tarball.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceTarball {
    pub name: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
}

/// The tool bundle built for one platform.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolBundle {
    pub bundle: String,
    pub bundle_sha256: String,
    /// File name → SHA-256 of every file in the bundle (tools and any required libraries).
    pub files: BTreeMap<String, String>,
}

/// `acquisitions/<acq_id>/acquisition.json` (§13.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcquisitionRecord {
    pub schema_version: u32,
    /// `YYYYMMDD-HHMMSSZ-ios-<6 hex>`.
    pub acq_id: String,
    pub label: Option<String>,
    pub status: AcqStatus,
    pub status_reasons: Vec<Reason>,
    pub warnings: Vec<Reason>,
    pub created_at: Timestamp,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    pub recovered_at: Option<Timestamp>,
    pub duration_ms: Option<u64>,
    pub app: RecordApp,
    pub host: RecordHost,
    pub case_snapshot: CaseSnapshot,
    pub device: AcqDevice,
    pub pairing: AcqPairing,
    /// Every change the app caused on the device, in order.
    pub device_changes: Vec<DeviceChange>,
    pub tools: AcqTools,
    pub encryption: AcqEncryption,
    pub commands: Vec<AcqCommand>,
    /// The backup command; `null` until it has exited.
    pub process: Option<AcqProcess>,
    /// `null` until the backup is validated.
    pub backup_result: Option<BackupResult>,
    pub output: AcqOutput,
    pub logs: AcqLogs,
}

impl VersionedFile for AcquisitionRecord {
    const FILE: &'static str = "acquisition.json";
}

/// The device, from `ideviceinfo -x` after pairing (saved as `device-info.plist`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcqDevice {
    pub udid: String,
    pub serial_number: Option<String>,
    pub device_name: Option<String>,
    pub product_type: Option<String>,
    pub product_version: Option<String>,
    pub build_version: Option<String>,
    /// When `device-info.plist` was captured; `null` if it could not be.
    pub captured_at: Option<Timestamp>,
    /// Relative to the acquisition folder.
    pub info_file: Option<String>,
    pub info_file_sha256: Option<String>,
}

/// The host pairing used for the acquisition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcqPairing {
    pub paired_before: bool,
    /// When this app paired the device in this app session, else `null`.
    pub paired_by_app_at: Option<Timestamp>,
    pub host_id: Option<String>,
    pub system_buid: Option<String>,
}

/// One change the app caused on the device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceChange {
    pub at: Timestamp,
    pub change: DeviceChangeKind,
    pub detail: String,
}

/// The libimobiledevice tools used (also in `encryption-restore.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcqTools {
    /// The pinned version for bundled tools; `null` when unknown (system tools).
    pub version: Option<String>,
    pub source: IdeviceToolSource,
    pub binaries: IdeviceBinaries,
}

/// The four tools, as observed at runtime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdeviceBinaries {
    pub idevice_id: ToolBinary,
    pub ideviceinfo: ToolBinary,
    pub idevicepair: ToolBinary,
    pub idevicebackup2: ToolBinary,
}

/// One tool binary as observed at runtime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolBinary {
    pub path: String,
    pub sha256: String,
    pub verified_against: ToolVerification,
}

/// Backup-encryption state and the examiner's encryption choices.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcqEncryption {
    /// `WillEncrypt` before the acquisition; `null` if unreadable.
    pub will_encrypt_before: Option<bool>,
    pub enable_requested: bool,
    pub enabled_by_examiner: bool,
    /// `WillEncrypt` after enabling; `null` if not attempted or unreadable.
    pub will_encrypt_after_enable: Option<bool>,
    pub restore_requested: bool,
    pub restored_after: RestoreState,
    /// `WillEncrypt` after restoring; `null` if not attempted or unreadable.
    pub will_encrypt_after_restore: Option<bool>,
    /// Whether an encryption password was given; the password itself is never recorded.
    pub password_supplied: bool,
    /// How the password reached the tool; `null` when none was supplied.
    pub password_channel: Option<PasswordChannel>,
}

/// A device-changing command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcqCommand {
    pub purpose: AcqCommandPurpose,
    /// Never contains a password.
    pub argv: Vec<String>,
    /// `null` while the command runs or when it was killed by a signal.
    pub exit_code: Option<i32>,
    pub started_at: Timestamp,
    /// `null` while the command runs.
    pub exited_at: Option<Timestamp>,
}

/// How the backup command ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcqProcess {
    /// `null` when the process was killed by a signal.
    pub exit_code: Option<i32>,
    /// The signal number that killed the process (Unix), else `null`.
    pub signal: Option<i32>,
    pub cancel_requested: bool,
    pub escalated_to_kill: bool,
}

/// What the backup output says (docs/IDEVICE-CLI.md §6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupResult {
    /// `Backup Successful.`, `Backup Failed (Error Code N).` or `Backup Aborted.`; `null` if none.
    pub final_message: Option<String>,
    /// `backup/<udid>`, relative to the acquisition folder.
    pub udid_dir: String,
    /// `Manifest.db`, `Manifest.mbdb` or `null`.
    pub manifest_found: Option<String>,
    pub info_plist_found: bool,
    pub status_plist_found: bool,
    /// `SnapshotState` from `Status.plist`; `null` if unreadable.
    pub snapshot_state: Option<String>,
    /// The last overall `NN% Finished` value; `null` if none was seen.
    pub last_progress_percent: Option<u8>,
    /// Count of `Received an error message from device:` lines.
    pub device_file_errors: u32,
    /// Free space on the target volume after the backup; `null` if unreadable.
    pub free_bytes_after: Option<u64>,
}

/// The acquisition's backup folder and its manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcqOutput {
    /// Relative to the acquisition folder.
    pub backup_dir: String,
    pub seal: Seal,
}

/// Log files of the backup command, relative to the acquisition folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcqLogs {
    pub stdout: String,
    pub stderr: String,
}

/// `acquisitions/<acq_id>/encryption-restore[-N].json`: one read-only file per later
/// `acq_restore_encryption` attempt, `encryption-restore.json` first, then
/// `encryption-restore-2.json`, … (§13.3). `acquisition.json` is not modified.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptionRestoreRecord {
    pub schema_version: u32,
    pub acq_id: String,
    pub at: Timestamp,
    /// Never contains a password.
    pub argv: Vec<String>,
    /// `null` when the process was killed by a signal.
    pub exit_code: Option<i32>,
    /// `WillEncrypt` afterwards; `null` if unreadable.
    pub will_encrypt_after: Option<bool>,
    pub restored: bool,
    pub tools: AcqTools,
}

impl VersionedFile for EncryptionRestoreRecord {
    const FILE: &'static str = "encryption-restore.json";
}
