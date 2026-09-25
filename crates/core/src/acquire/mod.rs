//! One iOS backup acquisition end-to-end (ARCHITECTURE.md §6b), driven through a callback of
//! [`AcqEvent`]s and without Tauri types:
//!
//! - [`preflight`] (step 3) and [`start`] (step 4: every check, then the acquisition folder; a
//!   failing check creates nothing).
//! - [`AcqJob::run`] (steps 5-11): prepare (`backup/`, `device-info.plist`, the pairing record, the
//!   initial `acquisition.json`, the per-job temp dir); enable encryption if asked (step 6); the
//!   backup through [`crate::process`] with a 30 s kill grace and a chunk callback for progress,
//!   prompts and abort causes (step 7); restore encryption (step 8); validation and status
//!   ([`status`], step 9); the seal (step 10); finalize (step 11). `acquisition.json` is rewritten
//!   atomically after every device-changing command.
//! - [`AcqControl::cancel`] follows the cancel semantics by phase: during `enabling_encryption` the
//!   command finishes and the backup is skipped; during `backing_up` the backup is stopped; during
//!   `restoring_encryption` the cancel is ignored; during `validating` and `sealing` the seal stops.
//!   Encryption is restored (when enabled and asked for) whatever the backup outcome.
//! - Discovery, listing and recovery on case open, and the `input.acquisition_id` helper: [`record`].
//! - [`restore_later`]: `acq_restore_encryption`, which writes `encryption-restore.json`.
//!
//! The backup password lives in a [`Password`] from `acq_start` until the restore step (or the
//! enable step when no restore follows), reaches the tool only through the environment, and is
//! never logged, recorded or put in an event or error.

pub mod output;
pub mod record;
pub mod status;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use sha2::Digest as _;

use crate::contracts::VersionedFile as _;
use crate::contracts::{
    AcqCommand, AcqCommandPurpose, AcqDevice, AcqEncryption, AcqEvent, AcqLogs, AcqOutput,
    AcqPairing, AcqPhase, AcqPreflight, AcqProcess, AcqRequest, AcqRestoreEncryptionResult,
    AcqStatus, AcqSummary, AcquisitionRecord, AppError, BackupResult, CaseFile, CaseSnapshot,
    ContractError, DeviceChange, DeviceChangeKind, EncryptionRestoreRecord, ErrorCode, PairState,
    PasswordChannel, PreflightLevel, Reason, RecordHost, RestoreState, Seal, SealStatus, StdStream,
    Timestamp,
};
use crate::fsutil;
use crate::hashing;
use crate::idevice::{
    CommandRun, Idevice, IdeviceError, IdeviceTools, OutputLine, Password, Session, ToolName,
    is_udid,
};
use crate::process::{self, SpawnSpec};

pub use record::{
    ACQ_FILE, ACQUISITIONS_DIR, BACKUP_DIR, BACKUP_MANIFEST, DEVICE_INFO_FILE, DiscoveredAcq,
    RESTORE_FILE, acquisition_id_for_input, discover, is_acq_id, load, new_acq_id, recover_case,
    summary,
};

/// Backup passwords are at least this many characters (ARCHITECTURE.md §6b step 4).
pub const MIN_PASSWORD_CHARS: usize = 4;
/// SIGTERM → SIGKILL grace for the backup (ARCHITECTURE.md D21).
pub const BACKUP_KILL_GRACE: Duration = Duration::from_secs(30);
/// The longest acquisition folder path the tools can use on Windows (ANSI file APIs).
pub const MAX_TOOL_PATH_CHARS: usize = 150;
/// At most 4 progress events per second.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
/// How often the job delivers the output that arrived.
const PUMP_INTERVAL: Duration = Duration::from_millis(50);
/// At most this many lines per `log` event.
const MAX_LOG_LINES: usize = 500;

// ---- errors ----

/// Errors of the acquisition commands.
#[derive(Debug, thiserror::Error)]
pub enum AcqError {
    #[error(transparent)]
    Idevice(#[from] IdeviceError),
    #[error("acquisition {0:?} was not found")]
    NotFound(String),
    #[error("the iOS tools cannot use the acquisition folder {path}: {reason}")]
    PathNotSupported { path: String, reason: String },
    #[error(
        "not enough free space for the backup: {free_bytes} bytes free, the device holds {required_bytes} bytes"
    )]
    InsufficientSpace {
        free_bytes: u64,
        required_bytes: u64,
    },
    #[error(
        "enabling backup encryption needs a password of at least {MIN_PASSWORD_CHARS} characters"
    )]
    PasswordRequired,
    #[error("backup encryption cannot be enabled: WillEncrypt is {0}")]
    EncryptionAlreadyOn(&'static str),
    #[error("device {udid} is not ready: {state}{}", .message.as_deref().map(|m| format!(" ({m})")).unwrap_or_default())]
    NotPaired {
        udid: String,
        state: PairState,
        message: Option<String>,
    },
    #[error("encryption cannot be restored for this acquisition: {0}")]
    RestoreNotApplicable(RestoreRefusal),
    #[error("acquisition {acq_id} is already finalized")]
    AlreadyFinalized { acq_id: String },
    #[error("acquisition {acq_id} cannot be written: {reason}")]
    NotFinal {
        acq_id: String,
        reason: &'static str,
    },
    #[error("the record names acquisition {acq_id:?} but the folder is {folder}")]
    FolderMismatch { acq_id: String, folder: String },
    #[error("{path}: {source}")]
    Invalid {
        path: String,
        #[source]
        source: ContractError,
    },
    #[error("no free acquisition id in {path}")]
    NoFreeId { path: String },
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
}

/// Why `acq_restore_encryption` is refused with `restore_not_applicable`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreRefusal {
    /// The record has neither `encryption_left_enabled` nor `encryption_state_unknown`.
    NoEncryptionWarning,
    /// `encryption-restore.json` already exists (it is read-only, so only one is recorded).
    AlreadyRecorded,
}

impl std::fmt::Display for RestoreRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoEncryptionWarning => {
                "the record has neither encryption_left_enabled nor encryption_state_unknown"
            }
            Self::AlreadyRecorded => "encryption-restore.json already records a later restore",
        })
    }
}

impl AcqError {
    pub(crate) fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }

    /// The `AppError` code (CONTRACTS.md §12).
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Idevice(err) => err.code(),
            Self::NotFound(_) => ErrorCode::AcqNotFound,
            Self::PathNotSupported { .. } => ErrorCode::PathNotSupportedByTool,
            Self::InsufficientSpace { .. } => ErrorCode::InsufficientSpace,
            Self::PasswordRequired => ErrorCode::EncryptionPasswordRequired,
            Self::EncryptionAlreadyOn(_) => ErrorCode::EncryptionAlreadyOn,
            Self::NotPaired { state, .. } => match state {
                PairState::Locked => ErrorCode::DeviceLocked,
                PairState::AwaitingTrust => ErrorCode::TrustPending,
                PairState::TrustDenied => ErrorCode::TrustDenied,
                PairState::PairingFailed => ErrorCode::PairingFailed,
                _ => ErrorCode::DeviceNotPaired,
            },
            Self::RestoreNotApplicable(_) => ErrorCode::RestoreNotApplicable,
            Self::AlreadyFinalized { .. }
            | Self::NotFinal { .. }
            | Self::FolderMismatch { .. }
            | Self::Invalid { .. }
            | Self::NoFreeId { .. } => ErrorCode::Internal,
            Self::Io { source, .. } => fsutil::io_error_code(source),
        }
    }

    /// A message that is safe to show.
    pub fn message(&self) -> String {
        match self {
            Self::Idevice(err) => err.message(),
            Self::NotFound(_) => "The acquisition was not found in this case.".to_owned(),
            Self::PathNotSupported { .. } => format!(
                "The iOS tools on Windows need a case folder path of plain ASCII characters, at \
                 most {MAX_TOOL_PATH_CHARS} characters long including the acquisition folder. \
                 Move the case to a shorter path without special characters."
            ),
            Self::InsufficientSpace { .. } => {
                "There is not enough free space on the case volume for this backup.".to_owned()
            }
            Self::PasswordRequired => {
                format!("Enter a backup password of at least {MIN_PASSWORD_CHARS} characters.")
            }
            Self::EncryptionAlreadyOn(_) => {
                "Backup encryption is already on (or its state could not be read), so it cannot \
                 be enabled."
                    .to_owned()
            }
            Self::NotPaired { state, .. } => match state {
                PairState::Locked => "Unlock the device and try again.".to_owned(),
                PairState::AwaitingTrust => "Tap Trust on the device, then pair again.".to_owned(),
                PairState::TrustDenied => {
                    "The device refused to trust this computer. Pair it again.".to_owned()
                }
                _ => "Pair the device with this computer first.".to_owned(),
            },
            Self::RestoreNotApplicable(refusal) => match refusal {
                RestoreRefusal::NoEncryptionWarning => "Turning backup encryption off applies \
                     only to acquisitions whose record shows that encryption was left on or in \
                     an unknown state, and this record shows neither."
                    .to_owned(),
                RestoreRefusal::AlreadyRecorded => "A later restore is already recorded for this \
                     acquisition (encryption-restore.json), and only one can be recorded. If it \
                     did not turn backup encryption off, the setting may still be on: check the \
                     device."
                    .to_owned(),
            },
            Self::Io { .. } => "The acquisition folder could not be read or written.".to_owned(),
            _ => "The acquisition record could not be written.".to_owned(),
        }
    }
}

impl From<AcqError> for AppError {
    fn from(err: AcqError) -> Self {
        AppError {
            code: err.code(),
            message: err.message(),
            detail: Some(err.to_string()),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // The guarded values are always consistent, so a poisoned lock is usable.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---- preflight and checks ----

/// The preflight level (ARCHITECTURE.md §6b step 3): `ok` if free ≥ 1.1 × required, `warn` if free
/// ≥ 0.5 × required, else `block`. An unknown requirement (the disk usage was unreadable) is `warn`.
pub fn preflight_level(free_bytes: u64, required_bytes: Option<u64>) -> PreflightLevel {
    let Some(required) = required_bytes else {
        return PreflightLevel::Warn;
    };
    let (free, required) = (u128::from(free_bytes), u128::from(required));
    if free * 10 >= required * 11 {
        PreflightLevel::Ok
    } else if free * 2 >= required {
        PreflightLevel::Warn
    } else {
        PreflightLevel::Block
    }
}

/// `acq_preflight`: free space on the case volume against the device's used data capacity. The
/// disk usage is read only from a paired device (reading it from an unpaired one would pair it).
pub fn preflight(idevice: &Idevice, case_dir: &Path, udid: &str) -> Result<AcqPreflight, AcqError> {
    if !is_udid(udid) {
        return Err(IdeviceError::InvalidUdid(udid.to_owned()).into());
    }
    let free_bytes = fsutil::free_space(case_dir).map_err(|e| AcqError::io(case_dir, e))?;
    let tools = idevice.tools().map_err(IdeviceError::Tools)?;
    let session = idevice.session(tools)?;
    let required_bytes = match session.pair_state(udid)?.0 {
        PairState::Paired => session.disk_usage(udid).map(|usage| usage.data_used()),
        _ => None,
    };
    Ok(AcqPreflight {
        free_bytes,
        required_bytes,
        level: preflight_level(free_bytes, required_bytes),
    })
}

/// Why the iOS tools on Windows cannot use `path` (they use ANSI file APIs, IDEVICE-CLI.md §5): it
/// is not plain ASCII, or it is longer than [`MAX_TOOL_PATH_CHARS`].
pub fn tool_path_problem(path: &str) -> Option<String> {
    if !path.is_ascii() {
        Some("it contains characters outside ASCII".to_owned())
    } else if path.len() > MAX_TOOL_PATH_CHARS {
        Some(format!(
            "it is {} characters long (at most {MAX_TOOL_PATH_CHARS})",
            path.len()
        ))
    } else {
        None
    }
}

/// The Windows path check of ARCHITECTURE.md §6b step 4 (`path_not_supported_by_tool`), for the
/// acquisition folder a new acquisition of `case_dir` would get.
fn check_tool_path(case_dir: &Path) -> Result<(), AcqError> {
    if !cfg!(windows) {
        return Ok(());
    }
    // Every acquisition id has the same length.
    let would_be = std::path::absolute(record::acq_dir(case_dir, "00000000-000000Z-ios-000000"))
        .map_err(|e| AcqError::io(case_dir, e))?;
    let text = would_be.to_string_lossy();
    match tool_path_problem(&text) {
        Some(reason) => Err(AcqError::PathNotSupported {
            path: text.into_owned(),
            reason,
        }),
        None => Ok(()),
    }
}

// ---- starting a job ----

/// What the shell knows about the case and the machine when an acquisition starts.
#[derive(Clone, Debug)]
pub struct AcqContext {
    /// The known case folder (`AcqRequest.case_path`, validated by the shell).
    pub case_dir: PathBuf,
    pub case: CaseFile,
    /// Filled in by the shell, as for runs.
    pub host: RecordHost,
}

/// `acq_start` (ARCHITECTURE.md §6b step 4): checks the password (at least 4 characters; on Windows
/// printable ASCII only, `invalid_input`, see [`Password::tool_problem`]), the Windows path rule, the tools
/// (verified afresh), that the device is connected and paired, the preflight level and, when
/// enabling encryption, that `WillEncrypt` is false; then creates the acquisition folder. A failing
/// check creates nothing. The one-active-job rule is the shell's.
pub fn start(idevice: &Idevice, request: AcqRequest, ctx: AcqContext) -> Result<AcqJob, AcqError> {
    let AcqRequest {
        case_path: _,
        udid,
        label,
        enable_encryption,
        encryption_password,
        restore_encryption,
    } = request;
    // A password without enabling encryption is not needed: dropping it zeroizes it.
    let password = encryption_password
        .map(Password::new)
        .filter(|_| enable_encryption);
    if !is_udid(&udid) {
        return Err(IdeviceError::InvalidUdid(udid).into());
    }
    if enable_encryption
        && password
            .as_ref()
            .is_none_or(|p| p.chars() < MIN_PASSWORD_CHARS)
    {
        return Err(AcqError::PasswordRequired);
    }
    // The same password turns encryption on and off again: refuse one the tools cannot take
    // intact (Windows ANSI code page) before anything is created or changed.
    if let Some(problem) = password.as_ref().and_then(Password::tool_problem) {
        return Err(IdeviceError::PasswordNotSupported(problem).into());
    }
    check_tool_path(&ctx.case_dir)?;
    let tools = idevice.verify_tools().map_err(IdeviceError::Tools)?;
    let session = idevice.session(Arc::clone(&tools))?;
    if !session.list_udids()?.contains(&udid) {
        return Err(IdeviceError::DeviceNotFound(udid).into());
    }
    let (state, message) = session.pair_state(&udid)?;
    if state != PairState::Paired {
        return Err(AcqError::NotPaired {
            udid,
            state,
            message,
        });
    }
    let free_bytes =
        fsutil::free_space(&ctx.case_dir).map_err(|e| AcqError::io(&ctx.case_dir, e))?;
    let required_bytes = session.disk_usage(&udid).map(|usage| usage.data_used());
    if preflight_level(free_bytes, required_bytes) == PreflightLevel::Block {
        return Err(AcqError::InsufficientSpace {
            free_bytes,
            required_bytes: required_bytes.unwrap_or_default(),
        });
    }
    let will_encrypt_before = session.will_encrypt(&udid);
    if enable_encryption && will_encrypt_before != Some(false) {
        return Err(AcqError::EncryptionAlreadyOn(match will_encrypt_before {
            Some(_) => "true",
            None => "unreadable",
        }));
    }
    drop(session);
    let created_at = Timestamp::now();
    let (acq_id, acq_dir) = record::create_acq_dir(&ctx.case_dir, created_at)?;
    Ok(AcqJob {
        acq_id,
        acq_dir,
        created_at,
        label,
        paired_by_app_at: idevice.paired_by_app_at(&udid),
        udid,
        host: ctx.host,
        case_snapshot: crate::run::record::case_snapshot(&ctx.case),
        tools,
        app_cache: idevice.app_cache().to_path_buf(),
        will_encrypt_before,
        enable_encryption,
        restore_requested: enable_encryption && restore_encryption,
        password,
        control: Arc::new(AcqControl::default()),
    })
}

// ---- control ----

#[derive(Debug)]
struct ControlState {
    phase: AcqPhase,
    /// A cancel arrived before the backup ended (preparing, enabling or backing up).
    cancel_requested: bool,
    backup: Option<Arc<process::Handle>>,
    seal_cancel: Arc<AtomicBool>,
}

/// Shared with the shell: the phase (for `job_active`) and cancel (`acq_cancel`).
#[derive(Debug)]
pub struct AcqControl {
    state: Mutex<ControlState>,
}

impl Default for AcqControl {
    fn default() -> Self {
        Self {
            state: Mutex::new(ControlState {
                phase: AcqPhase::Preparing,
                cancel_requested: false,
                backup: None,
                seal_cancel: Arc::new(AtomicBool::new(false)),
            }),
        }
    }
}

impl AcqControl {
    /// `acq_cancel`, by phase (ARCHITECTURE.md §6b): before the backup it skips the backup (an
    /// encryption change in progress finishes first); during the backup it stops it (SIGTERM, then
    /// SIGKILL after 30 s on Unix; the job is terminated at once on Windows); during the restore it
    /// is ignored; during validation and sealing it stops the seal. Idempotent.
    pub fn cancel(&self) {
        let mut state = lock(&self.state);
        match state.phase {
            AcqPhase::Preparing | AcqPhase::EnablingEncryption => state.cancel_requested = true,
            AcqPhase::BackingUp => {
                state.cancel_requested = true;
                if let Some(handle) = &state.backup {
                    handle.cancel();
                }
            }
            AcqPhase::RestoringEncryption | AcqPhase::Finalizing => {}
            AcqPhase::Validating | AcqPhase::Sealing => {
                state.seal_cancel.store(true, Ordering::SeqCst);
            }
        }
    }

    pub fn phase(&self) -> AcqPhase {
        lock(&self.state).phase
    }

    fn cancel_requested(&self) -> bool {
        lock(&self.state).cancel_requested
    }
}

// ---- the job ----

/// A started acquisition: its folder exists. [`AcqJob::run`] does the rest.
#[derive(Debug)]
pub struct AcqJob {
    acq_id: String,
    acq_dir: PathBuf,
    created_at: Timestamp,
    label: Option<String>,
    udid: String,
    paired_by_app_at: Option<Timestamp>,
    host: RecordHost,
    case_snapshot: CaseSnapshot,
    tools: Arc<IdeviceTools>,
    app_cache: PathBuf,
    will_encrypt_before: Option<bool>,
    enable_encryption: bool,
    restore_requested: bool,
    password: Option<Password>,
    control: Arc<AcqControl>,
}

/// How a job ended.
#[derive(Clone, Debug)]
pub struct AcqOutcome {
    /// The record as finalized (or as far as it got when the final write failed).
    pub record: AcquisitionRecord,
    pub summary: AcqSummary,
    /// The final write failed: the record on disk stays `running` and is recovered on the next
    /// open; `finished` reported `failed` with `record_write_failed`.
    pub write_error: Option<String>,
}

/// Emits events, batching log lines and throttling progress.
struct Events<'a> {
    on_event: &'a mut dyn FnMut(AcqEvent),
    lines: Vec<String>,
    last_progress: Option<Instant>,
    pending_progress: Option<u8>,
}

impl Events<'_> {
    fn emit(&mut self, event: AcqEvent) {
        (self.on_event)(event);
    }

    fn line(&mut self, line: String) {
        self.lines.push(line);
    }

    fn flush_lines(&mut self) {
        while !self.lines.is_empty() {
            let rest = self.lines.split_off(self.lines.len().min(MAX_LOG_LINES));
            let lines = std::mem::replace(&mut self.lines, rest);
            self.emit(AcqEvent::Log { lines });
        }
    }

    fn progress(&mut self, percent: u8) {
        self.pending_progress = Some(percent);
        self.flush_progress(false);
    }

    /// Emits pending progress if 250 ms have passed since the last one; with `wait`, waits for
    /// that moment instead of skipping.
    fn flush_progress(&mut self, wait: bool) {
        let Some(percent) = self.pending_progress else {
            return;
        };
        if let Some(last) = self.last_progress {
            let since = last.elapsed();
            if since < PROGRESS_INTERVAL {
                if !wait {
                    return;
                }
                std::thread::sleep(PROGRESS_INTERVAL - since);
            }
        }
        self.pending_progress = None;
        self.last_progress = Some(Instant::now());
        self.emit(AcqEvent::Progress { percent });
    }

    fn output_line(&mut self, line: OutputLine) {
        if let Some(kind) = line.prompt {
            self.flush_lines();
            self.emit(AcqEvent::DevicePrompt {
                kind,
                text: line.text.clone(),
            });
        }
        self.line(line.text);
    }
}

impl AcqJob {
    pub fn acq_id(&self) -> &str {
        &self.acq_id
    }

    pub fn acq_dir(&self) -> &Path {
        &self.acq_dir
    }

    pub fn udid(&self) -> &str {
        &self.udid
    }

    pub fn created_at(&self) -> Timestamp {
        self.created_at
    }

    /// For `acq_cancel` and `job_active`.
    pub fn control(&self) -> Arc<AcqControl> {
        Arc::clone(&self.control)
    }

    fn set_phase(&self, phase: AcqPhase, events: &mut Events<'_>) {
        lock(&self.control.state).phase = phase;
        events.flush_lines();
        events.emit(AcqEvent::Phase { phase });
    }

    /// Runs steps 5-11 on this thread, emitting events through `on_event`, and returns the final
    /// record. Blocking: it lasts as long as the backup.
    pub fn run(mut self, on_event: &mut dyn FnMut(AcqEvent)) -> AcqOutcome {
        let mut events = Events {
            on_event,
            lines: Vec::new(),
            last_progress: None,
            pending_progress: None,
        };
        self.set_phase(AcqPhase::Preparing, &mut events);
        let mut run = Run::default();
        let (mut record, temp_ok) = self.prepare(&mut run);
        let session = temp_ok.then(|| Session::in_dir(Arc::clone(&self.tools), &self.temp_dir()));

        if let Some(session) = &session
            && run.short.is_none()
        {
            if self.enable_encryption && !self.control.cancel_requested() {
                self.enable(session, &mut record, &mut run, &mut events);
            }
            let restore = self.restore_needed(&record);
            if !restore {
                // No restore follows: the password is not needed any more (after step 6).
                self.password = None;
            }
            if run.short.is_none() {
                if self.control.cancel_requested() {
                    run.short = Some(status::ShortCircuit::Cancelled);
                } else {
                    self.backup(&mut record, &mut run, &mut events);
                }
            }
            if restore {
                self.restore(session, &mut record, &mut events);
            }
        }
        // After step 8 (or when nothing ran) the password is not needed any more.
        self.password = None;

        self.set_phase(AcqPhase::Validating, &mut events);
        let facts = run.backup.as_ref().map(|backup| {
            let aborted = backup.output.final_message.as_deref() == Some(output::BACKUP_ABORTED);
            let device_gone =
                aborted && session.as_ref().and_then(|s| s.is_connected(&self.udid)) == Some(false);
            status::BackupFacts {
                exit_code: backup.exit_code,
                signal: backup.signal,
                final_message: backup.output.final_message.clone(),
                cancelled_on_device: backup.output.cancelled_on_device,
                device_gone,
                sync_lock_failed: backup.output.sync_lock_failed,
                layout: status::inspect_layout(&self.acq_dir.join(BACKUP_DIR).join(&self.udid)),
            }
        });
        if let (Some(facts), Some(backup)) = (&facts, &run.backup) {
            record.backup_result = Some(BackupResult {
                final_message: facts.final_message.clone(),
                udid_dir: format!("{BACKUP_DIR}/{}", self.udid),
                manifest_found: facts.layout.manifest_found.clone(),
                info_plist_found: facts.layout.info_plist_found,
                status_plist_found: facts.layout.status_plist_found,
                snapshot_state: facts.layout.snapshot_state.clone(),
                last_progress_percent: backup.output.last_percent,
                device_file_errors: backup.output.device_file_errors,
                free_bytes_after: backup.free_bytes_after,
            });
        }
        let (status, reasons) = status::evaluate(run.short.as_ref(), facts.as_ref());
        drop(session);

        self.set_phase(AcqPhase::Sealing, &mut events);
        let seal_warnings = self.seal(&mut record, &mut events);
        record.status = status;
        record.status_reasons = reasons;
        let warnings = status::warnings(
            &record.encryption,
            &status::WarningFacts {
                enable_outcome_unknown: record::enable_outcome_unknown(&record),
                device_file_errors: run
                    .backup
                    .as_ref()
                    .map_or(0, |b| b.output.device_file_errors),
                free_bytes_after: run.backup.as_ref().and_then(|b| b.free_bytes_after),
                seal: seal_warnings,
            },
        );
        record.warnings = warnings;

        self.set_phase(AcqPhase::Finalizing, &mut events);
        if temp_ok && let Err(e) = process::remove_temp_dir(&self.app_cache, &self.acq_id) {
            log::warn!(
                "cannot remove the temp dir of acquisition {}: {e}",
                self.acq_id
            );
        }
        let write_error = record::finalize(&self.acq_dir, &mut record, Timestamp::now())
            .err()
            .map(|e| {
                log::error!(
                    "the final record of acquisition {} could not be written: {e}",
                    self.acq_id
                );
                e.to_string()
            });
        let summary = record::summary(&DiscoveredAcq {
            dir: self.acq_dir.clone(),
            record: record.clone(),
        });
        let (status, reasons) = match &write_error {
            None => (record.status, record.status_reasons.clone()),
            Some(error) => (
                AcqStatus::Failed,
                vec![Reason {
                    code: "record_write_failed".to_owned(),
                    message: format!("The final acquisition.json could not be written: {error}"),
                }],
            ),
        };
        let mut finished_summary = summary.clone();
        finished_summary.status = status;
        events.flush_lines();
        events.emit(AcqEvent::Finished {
            status,
            reasons,
            warnings: record.warnings.clone(),
            summary: Box::new(finished_summary),
        });
        AcqOutcome {
            record,
            summary,
            write_error,
        }
    }

    fn temp_dir(&self) -> PathBuf {
        self.app_cache.join("tmp").join(&self.acq_id)
    }

    /// Step 5: `backup/`, the temp dir, `device-info.plist`, the pairing facts and the initial
    /// record. Returns the record and whether the temp dir exists.
    fn prepare(&self, run: &mut Run) -> (AcquisitionRecord, bool) {
        let mut failures = Vec::new();
        let backup_dir = self.acq_dir.join(BACKUP_DIR);
        if let Err(e) = fs::create_dir(&backup_dir) {
            failures.push(format!("cannot create {}: {e}", backup_dir.display()));
        }
        let temp_ok = match process::create_temp_dir(&self.app_cache, &self.acq_id) {
            Ok(_) => true,
            Err(e) => {
                failures.push(format!("cannot create the temp dir: {e}"));
                false
            }
        };
        let mut device = AcqDevice {
            udid: self.udid.clone(),
            serial_number: None,
            device_name: None,
            product_type: None,
            product_version: None,
            build_version: None,
            captured_at: None,
            info_file: None,
            info_file_sha256: None,
        };
        let mut pairing = AcqPairing {
            paired_before: self.paired_by_app_at.is_none(),
            paired_by_app_at: self.paired_by_app_at,
            host_id: None,
            system_buid: None,
        };
        if temp_ok && failures.is_empty() {
            let session = Session::in_dir(Arc::clone(&self.tools), &self.temp_dir());
            // The full values (IMEI, phone number): saved in the acquisition folder only.
            if let Some((bytes, info)) = session.full_info(&self.udid) {
                let file = self.acq_dir.join(DEVICE_INFO_FILE);
                let captured_at = Timestamp::now();
                match fsutil::write_file_atomic(&file, &bytes) {
                    Ok(()) => {
                        device.serial_number = info.serial_number;
                        device.device_name = info.device_name;
                        device.product_type = info.product_type;
                        device.product_version = info.product_version;
                        device.build_version = info.build_version;
                        device.captured_at = Some(captured_at);
                        device.info_file = Some(DEVICE_INFO_FILE.to_owned());
                        device.info_file_sha256 =
                            Some(hashing::to_hex(&sha2::Sha256::digest(&bytes)));
                    }
                    Err(e) => failures.push(format!("cannot write {}: {e}", file.display())),
                }
            }
            pairing.host_id = session.host_id(&self.udid).ok().flatten();
            pairing.system_buid = session.system_buid();
        }
        let device_changes = self
            .paired_by_app_at
            .map(|at| DeviceChange {
                at,
                change: DeviceChangeKind::PairRecordCreated,
                detail: "Trusted this computer via idevicepair pair".to_owned(),
            })
            .into_iter()
            .collect();
        let password_supplied = self.password.is_some();
        let record = AcquisitionRecord {
            schema_version: record::SCHEMA_VERSION,
            acq_id: self.acq_id.clone(),
            label: self.label.clone(),
            status: AcqStatus::Running,
            status_reasons: Vec::new(),
            warnings: Vec::new(),
            created_at: self.created_at,
            started_at: None,
            ended_at: None,
            recovered_at: None,
            duration_ms: None,
            app: crate::run::record::record_app(),
            host: self.host.clone(),
            case_snapshot: self.case_snapshot.clone(),
            device,
            pairing,
            device_changes,
            tools: self.tools.record(),
            encryption: AcqEncryption {
                will_encrypt_before: self.will_encrypt_before,
                enable_requested: self.enable_encryption,
                enabled_by_examiner: false,
                will_encrypt_after_enable: None,
                restore_requested: self.restore_requested,
                restored_after: if self.restore_requested {
                    RestoreState::NotAttempted
                } else {
                    RestoreState::NotRequested
                },
                will_encrypt_after_restore: None,
                password_supplied,
                password_channel: password_supplied.then_some(PasswordChannel::Env),
            },
            commands: Vec::new(),
            process: None,
            backup_result: None,
            output: AcqOutput {
                backup_dir: BACKUP_DIR.to_owned(),
                seal: Seal {
                    status: SealStatus::Pending,
                    manifest: None,
                    manifest_sha256: None,
                    file_count: None,
                    total_bytes: None,
                },
            },
            logs: AcqLogs {
                stdout: record::STDOUT_LOG.to_owned(),
                stderr: record::STDERR_LOG.to_owned(),
            },
        };
        if let Err(e) = record::write_initial(&self.acq_dir, &record) {
            failures.push(format!("cannot write the initial record: {e}"));
        }
        if !failures.is_empty() {
            run.short = Some(status::ShortCircuit::PrepareFailed(failures.join("; ")));
        }
        (record, temp_ok)
    }

    /// Rewrites the running record; a failure is logged (the final write is what counts).
    fn rewrite(&self, record: &AcquisitionRecord) {
        if let Err(e) = record::write_running(&self.acq_dir, record) {
            log::warn!("acquisition {}: {e}", self.acq_id);
        }
    }

    /// Records a device-changing command that is about to run (exit code and time still `null`),
    /// so a crash leaves it in the record. Returns its index.
    fn begin_command(
        &self,
        record: &mut AcquisitionRecord,
        purpose: AcqCommandPurpose,
        argv: Vec<String>,
    ) -> usize {
        let now = Timestamp::now();
        record.started_at.get_or_insert(now);
        record.commands.push(AcqCommand {
            purpose,
            argv,
            exit_code: None,
            started_at: now,
            exited_at: None,
        });
        self.rewrite(record);
        record.commands.len() - 1
    }

    /// Records how a command ended. Its `started_at` stays the time recorded when it began, so the
    /// record's `started_at` is exactly the first command's.
    fn end_command(record: &mut AcquisitionRecord, index: usize, run: &CommandRun) {
        if let Some(command) = record.commands.get_mut(index) {
            command.exit_code = run.exit.exit_code;
            command.exited_at = Some(run.exit.exited_at);
        }
    }

    /// Runs an encryption change and delivers its output lines as events. On an error the command
    /// stays in the record with a null exit code: the error may come after the tool started.
    fn run_encryption_command(
        &self,
        session: &Session,
        record: &mut AcquisitionRecord,
        enable: bool,
        password: &Password,
        events: &mut Events<'_>,
    ) -> Result<CommandRun, IdeviceError> {
        let purpose = if enable {
            AcqCommandPurpose::EnableEncryption
        } else {
            AcqCommandPurpose::RestoreEncryption
        };
        let argv = encryption_argv(&self.tools, &self.udid, enable);
        log::info!("acquisition {}: running {}", self.acq_id, argv.join(" "));
        let index = self.begin_command(record, purpose, argv);
        let result = session.set_encryption(&self.udid, enable, password, &mut |line| {
            events.output_line(line);
            events.flush_lines();
        });
        events.flush_lines();
        match &result {
            Ok(command) => {
                Self::end_command(record, index, command);
                log::info!(
                    "acquisition {}: {purpose} exited with {:?}",
                    self.acq_id,
                    command.exit.exit_code
                );
            }
            Err(e) => log::warn!(
                "acquisition {}: {purpose} did not complete: {e}",
                self.acq_id
            ),
        }
        result
    }

    /// Step 6: `encryption on`, then re-read `WillEncrypt`. The outcomes (ARCHITECTURE.md §6b):
    /// - `WillEncrypt` true: enabled by the examiner;
    /// - false after a failed command: `encryption_enable_failed` (nothing changed, no restore);
    /// - false although the tool reported success, or unreadable: unknown, so it is treated as
    ///   enabled for the restore and warns `encryption_state_unknown`
    ///   ([`record::enable_outcome_unknown`]).
    fn enable(
        &self,
        session: &Session,
        record: &mut AcquisitionRecord,
        run: &mut Run,
        events: &mut Events<'_>,
    ) {
        let Some(password) = &self.password else {
            return;
        };
        self.set_phase(AcqPhase::EnablingEncryption, events);
        let result = self.run_encryption_command(session, record, true, password, events);
        let after = session.will_encrypt(&self.udid);
        record.encryption.will_encrypt_after_enable = after;
        if after == Some(true) {
            record.encryption.enabled_by_examiner = true;
            record.device_changes.push(DeviceChange {
                at: Timestamp::now(),
                change: DeviceChangeKind::BackupEncryptionEnabled,
                detail: "WillEncrypt false → true".to_owned(),
            });
        }
        match result {
            Err(e) => {
                run.short = Some(status::ShortCircuit::SpawnFailed(format!(
                    "enable_encryption: {e}"
                )));
            }
            Ok(command) if after == Some(false) && command.exit.exit_code != Some(0) => {
                run.short = Some(status::ShortCircuit::EncryptionEnableFailed(format!(
                    "idevicebackup2 encryption on exited with {:?}; WillEncrypt is still false",
                    command.exit.exit_code
                )));
            }
            Ok(_) => {}
        }
        self.rewrite(record);
    }

    /// Whether step 8 runs: the examiner asked for the restore, and encryption was enabled or the
    /// enable outcome is unknown.
    fn restore_needed(&self, record: &AcquisitionRecord) -> bool {
        self.restore_requested
            && (record.encryption.enabled_by_examiner || record::enable_outcome_unknown(record))
    }

    /// Step 7: the backup.
    fn backup(&self, record: &mut AcquisitionRecord, run: &mut Run, events: &mut Events<'_>) {
        {
            let mut state = lock(&self.control.state);
            if state.cancel_requested {
                drop(state);
                run.short = Some(status::ShortCircuit::Cancelled);
                return;
            }
            state.phase = AcqPhase::BackingUp;
        }
        events.flush_lines();
        events.emit(AcqEvent::Phase {
            phase: AcqPhase::BackingUp,
        });
        let backup_dir = std::path::absolute(self.acq_dir.join(BACKUP_DIR))
            .unwrap_or_else(|_| self.acq_dir.join(BACKUP_DIR));
        let args: Vec<String> = [
            "-u",
            &self.udid,
            "backup",
            "--full",
            &backup_dir.to_string_lossy(),
        ]
        .iter()
        .map(|arg| (*arg).to_owned())
        .collect();
        let tool = self.tools.tool(ToolName::Idevicebackup2);
        let mut spec = SpawnSpec::new(
            &tool.program,
            &self.acq_dir,
            self.acq_dir.join(record::STDOUT_LOG),
            self.acq_dir.join(record::STDERR_LOG),
        );
        spec.args = self.tools.args(ToolName::Idevicebackup2, &args);
        spec.env.clone_from(&self.tools.env);
        spec.temp_dir = Some(self.temp_dir());
        spec.kill_grace = BACKUP_KILL_GRACE;
        spec.timeout = None;
        let (sender, receiver) = mpsc::channel::<(StdStream, Vec<u8>)>();
        spec.on_output = Some(Box::new(move |stream, chunk| {
            // The receiver lives until the process is gone.
            let _ = sender.send((stream, chunk.to_vec()));
        }));
        let handle = match process::spawn(spec) {
            Ok(handle) => Arc::new(handle),
            Err(e) => {
                run.short = Some(status::ShortCircuit::SpawnFailed(format!("backup: {e}")));
                return;
            }
        };
        {
            let mut state = lock(&self.control.state);
            state.backup = Some(Arc::clone(&handle));
            if state.cancel_requested {
                handle.cancel();
            }
        }
        let index = self.begin_command(
            record,
            AcqCommandPurpose::Backup,
            self.tools.argv(ToolName::Idevicebackup2, &args),
        );
        record.device_changes.push(DeviceChange {
            at: record.commands[index].started_at,
            change: DeviceChangeKind::SyncLockTaken,
            detail: "idevicebackup2 holds /com.apple.itunes.lock_sync during backup".to_owned(),
        });
        self.rewrite(record);

        let mut parsed_output = output::BackupOutput::default();
        let exit = loop {
            let mut parsed = Vec::new();
            for (stream, chunk) in receiver.try_iter() {
                parsed.extend(parsed_output.push(stream, &chunk));
            }
            deliver(events, parsed);
            match handle.wait_timeout(PUMP_INTERVAL) {
                Ok(Some(exit)) => break Ok(exit),
                Ok(None) => {}
                Err(e) => break Err(e),
            }
        };
        let mut parsed = Vec::new();
        for (stream, chunk) in receiver.try_iter() {
            parsed.extend(parsed_output.push(stream, &chunk));
        }
        parsed.extend(parsed_output.finish());
        deliver(events, parsed);
        events.flush_progress(true);
        lock(&self.control.state).backup = None;

        let exit = match exit {
            Ok(exit) => exit,
            Err(e) => {
                // The supervisor failed; it has already stopped the tree.
                run.short = Some(status::ShortCircuit::SpawnFailed(format!("backup: {e}")));
                return;
            }
        };
        if let Some(command) = record.commands.get_mut(index) {
            command.exit_code = exit.exit_code;
            command.exited_at = Some(exit.exited_at);
        }
        if parsed_output.sync_lock_failed
            && let Some(change) = record
                .device_changes
                .iter_mut()
                .rev()
                .find(|c| c.change == DeviceChangeKind::SyncLockTaken)
        {
            // The tool opened the lock file and posted the sync notifications, but never held
            // the lock.
            change.detail = "idevicebackup2 requested /com.apple.itunes.lock_sync, but the lock \
                             could not be taken"
                .to_owned();
        }
        record.process = Some(AcqProcess {
            exit_code: exit.exit_code,
            signal: exit.signal,
            cancel_requested: exit.cancel_requested,
            // Windows has no graceful stop: terminating the job is the kill (CONTRACTS.md §13.4).
            escalated_to_kill: exit.escalated_to_kill || (cfg!(windows) && exit.cancel_requested),
        });
        self.rewrite(record);
        if exit.cancel_requested {
            run.short = Some(status::ShortCircuit::Cancelled);
        }
        run.backup = Some(BackupRun {
            exit_code: exit.exit_code,
            signal: exit.signal,
            output: parsed_output,
            free_bytes_after: fsutil::free_space(&self.acq_dir).ok(),
        });
    }

    /// Step 8: `encryption off`, then re-read `WillEncrypt`. Runs whatever the backup outcome;
    /// a cancel does not stop it.
    fn restore(&self, session: &Session, record: &mut AcquisitionRecord, events: &mut Events<'_>) {
        let Some(password) = &self.password else {
            return;
        };
        self.set_phase(AcqPhase::RestoringEncryption, events);
        if session.is_connected(&self.udid) == Some(false) {
            // The device is gone: nothing can be restored now (encryption_left_enabled).
            record.encryption.restored_after = RestoreState::NotAttempted;
            self.rewrite(record);
            return;
        }
        let ran = self
            .run_encryption_command(session, record, false, password, events)
            .is_ok();
        let after = session.will_encrypt(&self.udid);
        record.encryption.will_encrypt_after_restore = after;
        record.encryption.restored_after = match after {
            Some(false) => RestoreState::Restored,
            Some(true) => RestoreState::Failed,
            None => RestoreState::Unknown,
        };
        // A device change is recorded only when it was observed: the command ran, and WillEncrypt
        // read true before (after the enable) and false now.
        if ran && record.encryption.will_encrypt_after_enable == Some(true) && after == Some(false)
        {
            record.device_changes.push(DeviceChange {
                at: Timestamp::now(),
                change: DeviceChangeKind::BackupEncryptionDisabled,
                detail: "WillEncrypt true → false".to_owned(),
            });
        }
        self.rewrite(record);
    }

    /// Step 10: `backup.sha256` over `backup/`; a cancel during validation or sealing stops it.
    /// Returns the seal's warnings.
    fn seal(&self, record: &mut AcquisitionRecord, events: &mut Events<'_>) -> Vec<Reason> {
        let backup_dir = self.acq_dir.join(BACKUP_DIR);
        let empty = |status| Seal {
            status,
            manifest: None,
            manifest_sha256: None,
            file_count: None,
            total_bytes: None,
        };
        if !backup_dir.is_dir() {
            record.output.seal = empty(SealStatus::SkippedNoOutput);
            return Vec::new();
        }
        let cancel = Arc::clone(&lock(&self.control.state).seal_cancel);
        let result = hashing::seal_tree(
            &backup_dir,
            &self.acq_dir.join(BACKUP_MANIFEST),
            &cancel,
            |files_done, files_total| {
                events.emit(AcqEvent::SealProgress {
                    files_done,
                    files_total,
                });
            },
        );
        let mut warnings = Vec::new();
        match result {
            Ok(outcome) => {
                record.output.seal = outcome.seal(BACKUP_MANIFEST);
                if outcome.cancelled {
                    warnings.push(Reason {
                        code: "seal_cancelled".to_owned(),
                        message: "Hashing the backup was cancelled; no backup.sha256 was written"
                            .to_owned(),
                    });
                }
                warnings.extend(outcome.warnings("symlinks_in_backup"));
            }
            Err(e) => {
                record.output.seal = empty(SealStatus::Failed);
                warnings.push(Reason {
                    code: "seal_failed".to_owned(),
                    message: format!("backup.sha256 could not be written: {e}"),
                });
            }
        }
        warnings
    }
}

/// Hands parsed backup output to the events: log lines (batched), progress (throttled), prompts.
fn deliver(events: &mut Events<'_>, parsed: Vec<output::Parsed>) {
    for item in parsed {
        match item {
            output::Parsed::Line(line) => events.line(line),
            output::Parsed::Progress(percent) => events.progress(percent),
            output::Parsed::Prompt(kind, text) => {
                events.flush_lines();
                events.emit(AcqEvent::DevicePrompt {
                    kind,
                    text: text.clone(),
                });
                events.line(text);
            }
        }
    }
    events.flush_lines();
    events.flush_progress(false);
}

/// The recorded argv of `encryption on|off` (no password: it travels via env).
fn encryption_argv(tools: &IdeviceTools, udid: &str, enable: bool) -> Vec<String> {
    let args: Vec<String> = ["-u", udid, "encryption", if enable { "on" } else { "off" }]
        .iter()
        .map(|arg| (*arg).to_owned())
        .collect();
    tools.argv(ToolName::Idevicebackup2, &args)
}

/// What the job learned along the way.
#[derive(Default)]
struct Run {
    short: Option<status::ShortCircuit>,
    backup: Option<BackupRun>,
}

struct BackupRun {
    exit_code: Option<i32>,
    signal: Option<i32>,
    output: output::BackupOutput,
    free_bytes_after: Option<u64>,
}

// ---- later restore ----

/// `acq_restore_encryption`: turns backup encryption off for an acquisition whose record has
/// `encryption_left_enabled` or `encryption_state_unknown` (else `restore_not_applicable`, as when
/// a later restore was already recorded), with the device connected and paired. Runs step 8 on
/// its own in a fresh temp dir, prompt lines go to `on_line`, and the outcome is written to a
/// read-only `encryption-restore.json`; `acquisition.json` is not modified. The one-active-job
/// rule is the shell's.
pub fn restore_later(
    idevice: &Idevice,
    case_dir: &Path,
    acq_id: &str,
    password: String,
    on_line: &mut dyn FnMut(OutputLine),
) -> Result<AcqRestoreEncryptionResult, AcqError> {
    let password = Password::new(password);
    let record = record::load(case_dir, acq_id)?;
    let applicable = record
        .warnings
        .iter()
        .any(|w| w.code == "encryption_left_enabled" || w.code == "encryption_state_unknown");
    if !applicable {
        return Err(AcqError::RestoreNotApplicable(
            RestoreRefusal::NoEncryptionWarning,
        ));
    }
    let dir = record::acq_dir(case_dir, acq_id);
    if fs::symlink_metadata(dir.join(RESTORE_FILE)).is_ok() {
        return Err(AcqError::RestoreNotApplicable(
            RestoreRefusal::AlreadyRecorded,
        ));
    }
    if password.chars() < MIN_PASSWORD_CHARS {
        return Err(AcqError::PasswordRequired);
    }
    if let Some(problem) = password.tool_problem() {
        return Err(IdeviceError::PasswordNotSupported(problem).into());
    }
    let udid = &record.device.udid;
    let tools = idevice.verify_tools().map_err(IdeviceError::Tools)?;
    let session = idevice.session(Arc::clone(&tools))?;
    if !session.list_udids()?.contains(udid) {
        return Err(IdeviceError::DeviceNotFound(udid.clone()).into());
    }
    let (state, message) = session.pair_state(udid)?;
    if state != PairState::Paired {
        return Err(AcqError::NotPaired {
            udid: udid.clone(),
            state,
            message,
        });
    }
    log::info!(
        "acquisition {acq_id}: later restore, running {}",
        encryption_argv(&tools, udid, false).join(" ")
    );
    let command = session.set_encryption(udid, false, &password, on_line)?;
    drop(password);
    let will_encrypt_after = session.will_encrypt(udid);
    let restored = will_encrypt_after == Some(false);
    log::info!(
        "acquisition {acq_id}: later restore exited with {:?}; WillEncrypt is {will_encrypt_after:?}",
        command.exit.exit_code
    );
    record::write_restore_record(
        &dir,
        &EncryptionRestoreRecord {
            schema_version: EncryptionRestoreRecord::SCHEMA_VERSION,
            acq_id: acq_id.to_owned(),
            at: command.started_at,
            argv: command.argv,
            exit_code: command.exit.exit_code,
            will_encrypt_after,
            restored,
            tools: tools.record(),
        },
    )?;
    Ok(AcqRestoreEncryptionResult {
        restored,
        will_encrypt_after,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_levels() {
        let gib = 1u64 << 30;
        assert_eq!(
            preflight_level(11 * gib, Some(10 * gib)),
            PreflightLevel::Ok
        );
        assert_eq!(
            preflight_level(11 * gib - 1, Some(10 * gib)),
            PreflightLevel::Warn
        );
        assert_eq!(
            preflight_level(5 * gib, Some(10 * gib)),
            PreflightLevel::Warn
        );
        assert_eq!(
            preflight_level(5 * gib - 1, Some(10 * gib)),
            PreflightLevel::Block
        );
        assert_eq!(preflight_level(0, Some(0)), PreflightLevel::Ok);
        assert_eq!(
            preflight_level(u64::MAX, Some(u64::MAX)),
            PreflightLevel::Warn
        );
        assert_eq!(preflight_level(0, None), PreflightLevel::Warn);
    }

    #[test]
    fn windows_tool_paths() {
        assert_eq!(
            tool_path_problem(r"C:\Cases\Op Nightjar\acquisitions\20260924-171200Z-ios-9c01de"),
            None
        );
        assert!(tool_path_problem(r"C:\Fälle\x").unwrap().contains("ASCII"));
        let long = format!(r"C:\{}", "a".repeat(MAX_TOOL_PATH_CHARS - 3));
        assert_eq!(long.len(), MAX_TOOL_PATH_CHARS);
        assert_eq!(tool_path_problem(&long), None);
        assert!(
            tool_path_problem(&format!("{long}b"))
                .unwrap()
                .contains("151")
        );
    }

    #[test]
    fn error_codes() {
        let cases: Vec<(AcqError, ErrorCode)> = vec![
            (AcqError::NotFound("x".into()), ErrorCode::AcqNotFound),
            (
                AcqError::PathNotSupported {
                    path: "p".into(),
                    reason: "r".into(),
                },
                ErrorCode::PathNotSupportedByTool,
            ),
            (
                AcqError::InsufficientSpace {
                    free_bytes: 1,
                    required_bytes: 2,
                },
                ErrorCode::InsufficientSpace,
            ),
            (
                AcqError::PasswordRequired,
                ErrorCode::EncryptionPasswordRequired,
            ),
            (
                AcqError::EncryptionAlreadyOn("true"),
                ErrorCode::EncryptionAlreadyOn,
            ),
            (
                AcqError::RestoreNotApplicable(RestoreRefusal::AlreadyRecorded),
                ErrorCode::RestoreNotApplicable,
            ),
            (
                IdeviceError::DeviceNotFound("u".into()).into(),
                ErrorCode::DeviceNotFound,
            ),
        ];
        for (err, code) in cases {
            assert_eq!(err.code(), code, "{err}");
        }
        for (state, code) in [
            (PairState::NotPaired, ErrorCode::DeviceNotPaired),
            (PairState::Locked, ErrorCode::DeviceLocked),
            (PairState::AwaitingTrust, ErrorCode::TrustPending),
            (PairState::TrustDenied, ErrorCode::TrustDenied),
            (PairState::PairingFailed, ErrorCode::PairingFailed),
            (PairState::Unknown, ErrorCode::DeviceNotPaired),
        ] {
            let err = AcqError::NotPaired {
                udid: "u".into(),
                state,
                message: None,
            };
            assert_eq!(err.code(), code);
            let app: AppError = err.into();
            assert_eq!(app.code, code);
        }
    }

    #[test]
    fn events_batch_lines_and_throttle_progress() {
        let mut seen = Vec::new();
        let mut sink = |event: AcqEvent| seen.push(event);
        let mut events = Events {
            on_event: &mut sink,
            lines: Vec::new(),
            last_progress: None,
            pending_progress: None,
        };
        for i in 0..(MAX_LOG_LINES + 3) {
            events.line(format!("line {i}"));
        }
        events.flush_lines();
        events.progress(10);
        events.progress(20);
        events.progress(30);
        let before_wait = Instant::now();
        events.flush_progress(true);
        assert!(before_wait.elapsed() >= Duration::from_millis(200));
        events.flush_progress(true);
        drop(events);
        let sizes: Vec<usize> = seen
            .iter()
            .filter_map(|e| match e {
                AcqEvent::Log { lines } => Some(lines.len()),
                _ => None,
            })
            .collect();
        assert_eq!(sizes, [MAX_LOG_LINES, 3]);
        let progress: Vec<u8> = seen
            .iter()
            .filter_map(|e| match e {
                AcqEvent::Progress { percent } => Some(*percent),
                _ => None,
            })
            .collect();
        assert_eq!(
            progress,
            [10, 30],
            "at most one per 250 ms, the latest value wins"
        );
    }
}
