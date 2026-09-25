//! One run end-to-end (ARCHITECTURE.md §6), driven through a callback of [`RunEvent`]s and without
//! Tauri types. It composes the tool check (`leapp`), the record, status, argv and profile rules
//! (`run`), inspection and the overlap rule (`inspect`), hashing, the process tree (`process`) and
//! the `Screen_Output.html` tail (`tail`):
//!
//! - [`installed_tool`] (or, in debug builds, [`dev_override_tool`]) verifies the tool before every
//!   run and loads its module list.
//! - [`start`] is lifecycle step 1: every check, each failing with its `AppError` and creating
//!   nothing; then it creates `runs/<run_id>/` so `run_start` can answer with the id.
//! - [`RunJob::run`] is steps 2-10: prepare (initial `run.json`, `case.lcasedata`, the run's
//!   profile, the per-run temp dir), hash the input on its own thread concurrently with LEAPP,
//!   spawn LEAPP, stream `Screen_Output.html` in `log` batches of at most 500 lines, drain after
//!   the exit and send each stdio tail once, wait for the input hash, analyze the report, seal it
//!   into `report.sha256`, and finalize `run.json` (atomic, then read-only).
//! - [`RunControl::cancel`]: before the exit it stops the process tree (and hashing); a cancel that
//!   arrives after the exit only stops input hashing (warning `input_hash_cancelled`).
//!
//! The one-active-job rule, the event channel and the log backlog are the shell's. The iTunes
//! backup password lives in the job only until LEAPP is spawned; it reaches LEAPP through argv
//! (its only non-interactive channel, ARCHITECTURE.md D15) and is redacted everywhere else.

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use crate::acquire;
use crate::case::{self, DiscoveredRun};
use crate::contracts::{
    AppError, CaseFile, EntryVerifiedAgainst, ErrorCode, HashStatus, InputKind, InputType,
    InstallSource, ModuleMode, ModulesFile, PlatformKey, Reason, RecordHost, RunCommand, RunEvent,
    RunInput, RunOptions, RunPhase, RunProcess, RunRecord, RunRequest, RunStatus, RunSummary,
    RunTool, Seal, SealStatus, Settings, StdStream, Timestamp, ToolId, ToolManifest, ToolState,
    VersionedFile, parse_versioned,
};
use crate::fsutil;
use crate::hashing::{self, HashOutcome};
use crate::inspect::{self, OverlapContext};
use crate::leapp::install::{self, MODULES_FILE, Pinned};
use crate::leapp::modules::{self as leapp_modules, AlwaysRunSet};
use crate::paths::AppPaths;
use crate::process::{self, ExitInfo, SpawnSpec};
use crate::run::argv::{self, ArgvSpec};
use crate::run::casedata;
use crate::run::profile::{self, ProfileFormat, ProfileStore};
use crate::run::record::{self, REPORT_DIR, REPORT_MANIFEST, RunSetup, STDERR_LOG, STDOUT_LOG};
use crate::run::status::{self, Outcome, StatusInput};
use crate::tail::{self, MAX_BATCH_LINES, STDIO_TAIL_LINES, ScreenOutputTail};

/// The longest run folder path on Windows, in UTF-16 units: `Command::current_dir` cannot take a
/// verbatim path, so the cwd must stay under `MAX_PATH` (248 for a directory; ARCHITECTURE.md §7).
pub const WINDOWS_MAX_RUN_DIR_CHARS: usize = 247;

/// The iLEAPP timezone when neither the request, the case nor the settings name one (D19).
const FALLBACK_TIMEZONE: &str = "UTC";

/// The reason of a run whose final `run.json` could not be written (ARCHITECTURE.md §6 step 10).
pub const RECORD_WRITE_FAILED: &str = "record_write_failed";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // The guarded values are plain fields that are always consistent, so a poisoned lock is usable.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn app_error(code: ErrorCode, message: impl Into<String>, detail: Option<String>) -> AppError {
    AppError {
        code,
        message: message.into(),
        detail,
    }
}

// ---- the tool ----

/// A tool ready to run: its verified entry, the `tool` part of `run.json`, its module list and its
/// pinned manifest entry (input types, profile format, supported options).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedTool {
    /// The absolute path of the executable.
    pub entry: PathBuf,
    pub record: RunTool,
    pub modules: ModulesFile,
    pub manifest: ToolManifest,
}

/// Checks the installed tool right before a run (ARCHITECTURE.md §6 step 1): it must be installed
/// and its entry must hash to the manifest's `entry_sha256` (or `install.json`'s where the manifest
/// has `null`). Then loads its `modules.json`.
pub fn installed_tool(pinned: Pinned<'_>) -> Result<PreparedTool, AppError> {
    let check = install::verify(pinned);
    let name = &pinned.manifest.display_name;
    let problem = check.status.problem.clone();
    let verified = match (check.status.state, check.verified) {
        (ToolState::Verified, Some(verified)) => verified,
        (ToolState::UnsupportedPlatform, _) => {
            return Err(app_error(
                ErrorCode::UnsupportedPlatform,
                format!("{name} has no pinned build for this platform"),
                problem,
            ));
        }
        (ToolState::VerificationFailed, _) => {
            return Err(app_error(
                ErrorCode::ToolVerificationFailed,
                format!(
                    "{name} failed verification: its executable does not match the pinned build. \
                     Reinstall it in Settings."
                ),
                problem,
            ));
        }
        _ => {
            return Err(app_error(
                ErrorCode::ToolNotInstalled,
                format!("{name} is not installed. Install it in Settings."),
                problem,
            ));
        }
    };
    let modules_path = pinned.version_dir().join(MODULES_FILE);
    let modules: ModulesFile = fs::read(&modules_path)
        .map_err(|e| e.to_string())
        .and_then(|bytes| parse_versioned(&bytes).map_err(|e| e.to_string()))
        .map_err(|why| {
            app_error(
                ErrorCode::ToolNotInstalled,
                format!("{name}'s module list is unusable. Reinstall it in Settings."),
                Some(format!("{}: {why}", modules_path.display())),
            )
        })?;
    let record = &verified.record;
    Ok(PreparedTool {
        entry: verified.entry.clone(),
        record: RunTool {
            id: pinned.tool,
            version: record.version.clone(),
            platform: record.platform,
            asset_name: Some(record.asset_name.clone()),
            asset_sha256: Some(record.asset_sha256.clone()),
            // install.json's hash: equal to the manifest's where it pins one (install checked it),
            // and just re-verified either way.
            entry_sha256: record.entry_sha256.clone(),
            entry_verified_against: verified.verified_against,
            install_source: record.source,
        },
        modules,
        manifest: pinned.manifest.clone(),
    })
}

/// The dev-override tool (debug builds only, DEVELOPMENT.md §2): `entry` is fake-leapp; its module
/// list comes from `--list-modules-json`, verification is skipped (`entry_verified_against: none`,
/// the observed hash is still recorded) and the run records `install_source: dev_override`.
#[cfg(debug_assertions)]
pub fn dev_override_tool(
    entry: &Path,
    tool: ToolId,
    manifest: &ToolManifest,
    platform: Option<PlatformKey>,
) -> Result<PreparedTool, AppError> {
    use crate::leapp::dev_override;
    let platform = platform.ok_or_else(|| {
        app_error(
            ErrorCode::UnsupportedPlatform,
            "This OS and CPU have no platform key",
            None,
        )
    })?;
    let entry = std::path::absolute(entry).map_err(|e| {
        app_error(
            ErrorCode::ToolNotInstalled,
            "The dev override path is unusable",
            Some(e.to_string()),
        )
    })?;
    let modules = dev_override::modules(&entry, tool)?;
    let entry_sha256 = hashing::sha256_file(&entry).map_err(|e| {
        app_error(
            ErrorCode::ToolNotInstalled,
            "The dev override cannot be read",
            Some(format!("{}: {e}", entry.display())),
        )
    })?;
    Ok(PreparedTool {
        record: RunTool {
            id: tool,
            version: dev_override::VERSION.to_owned(),
            platform,
            asset_name: None,
            asset_sha256: None,
            entry_sha256,
            entry_verified_against: EntryVerifiedAgainst::None,
            install_source: InstallSource::DevOverride,
        },
        entry,
        modules,
        manifest: manifest.clone(),
    })
}

// ---- step 1: validate ----

/// What the shell knows when a run starts.
#[derive(Clone, Debug)]
pub struct RunContext {
    pub paths: AppPaths,
    /// The settings in effect: known cases, defaults and the tools dir.
    pub settings: Settings,
    /// The known case folder (`RunRequest.case_path`, validated by the shell).
    pub case_dir: PathBuf,
    pub case: CaseFile,
    /// Filled in by the shell.
    pub host: RecordHost,
    /// [`installed_tool`] or [`dev_override_tool`] for the request's tool.
    pub tool: PreparedTool,
    /// Variables added to LEAPP's environment (empty in the app; tests pick fake-leapp scenarios).
    pub env: Vec<(OsString, OsString)>,
    /// The longest allowed run folder path (`path_too_long`); [`default_run_dir_limit`] in the app.
    pub max_run_dir_chars: Option<usize>,
}

/// [`WINDOWS_MAX_RUN_DIR_CHARS`] on Windows; no limit elsewhere.
pub const fn default_run_dir_limit() -> Option<usize> {
    if cfg!(windows) {
        Some(WINDOWS_MAX_RUN_DIR_CHARS)
    } else {
        None
    }
}

/// `run_start` (ARCHITECTURE.md §6 step 1): checks the paths LEAPP gets, inspects the input
/// (exists, readable, the overlap rule), that the type is allowed for this tool and input, the
/// keychain file, the module selection (no unknown names), the backup password, the timezone
/// (iLEAPP; from the request, else the case, else the settings, else UTC) and the run folder path
/// length. A failing check creates nothing. Then creates `runs/<run_id>/`. The one-active-job rule
/// is the shell's.
pub fn start(request: RunRequest, ctx: RunContext) -> Result<RunJob, AppError> {
    let RunRequest {
        case_path: _,
        tool,
        input_path,
        input_type,
        modules: selection,
        timezone,
        itunes_password,
        keychain_path,
        hash_input,
        label,
    } = request;
    if ctx.tool.record.id != tool {
        return Err(app_error(
            ErrorCode::Internal,
            "The run was prepared for another tool",
            Some(format!("request {tool}, tool {}", ctx.tool.record.id)),
        ));
    }
    let manifest = &ctx.tool.manifest;
    let input = PathBuf::from(&input_path);
    argv::check_path("tool", &ctx.tool.entry)?;
    argv::check_path("input", &input)?;

    let known: Vec<PathBuf> = ctx
        .settings
        .recent_cases
        .iter()
        .map(PathBuf::from)
        .collect();
    let app_dirs = ctx.paths.app_dirs(&ctx.settings);
    let temp_root = ctx.paths.temp_root();
    let overlap = OverlapContext {
        case_dir: &ctx.case_dir,
        known_cases: &known,
        app_dirs: &app_dirs,
        temp_root: &temp_root,
    };
    let inspection = inspect::inspect(&input, &manifest.input_types, &overlap)?;
    if !inspection.allowed_types.contains(&input_type) {
        let allowed: Vec<&str> = inspection
            .allowed_types
            .iter()
            .map(|t| t.as_str())
            .collect();
        return Err(app_error(
            ErrorCode::InputTypeNotAllowed,
            format!(
                "{} cannot read this input as type {input_type}",
                manifest.display_name
            ),
            Some(format!(
                "{}: allowed types {}",
                inspection.path,
                allowed.join(", ")
            )),
        ));
    }

    let keychain = match keychain_path {
        Some(path) => Some(check_keychain(Path::new(&path), manifest, &overlap)?),
        None => None,
    };

    let format = ProfileFormat::from_manifest(tool, manifest);
    let store = ProfileStore::new(ctx.paths.profiles_dir(tool), format.clone());
    let modules = profile::resolve(&selection, &ctx.tool.modules, input_type, &store)?;
    if !modules.unknown.is_empty() {
        return Err(profile::ProfileError::UnknownModules {
            names: modules.unknown,
        }
        .into());
    }

    // Only iLEAPP takes a backup password, and only an iTunes backup needs one.
    let password = itunes_password
        .filter(|password| !password.is_empty())
        .filter(|_| manifest.supports_itunes_password && input_type == InputType::Itunes);
    if manifest.supports_itunes_password && input_type == InputType::Itunes && password.is_none() {
        // A backup whose encryption cannot be read counts as encrypted: iLEAPP would stop at its
        // password prompt, which blocks on Windows (LEAPP-CLI.md Q5, ARCHITECTURE.md D15).
        let message = match (inspection.is_itunes_backup, inspection.itunes_encrypted) {
            (_, Some(true)) => Some("This iTunes backup is encrypted: enter its backup password"),
            (true, None) => Some(
                "This iTunes backup's encryption state could not be read, so a password is \
                 needed: enter its backup password",
            ),
            _ => None,
        };
        if let Some(message) = message {
            return Err(app_error(
                ErrorCode::PasswordRequired,
                message,
                Some(inspection.path.clone()),
            ));
        }
    }

    let timezone = resolve_timezone(timezone, &ctx)?;

    let would_be = ctx
        .case_dir
        .join(case::RUNS_DIR)
        .join(format!("00000000-000000Z-{tool}-000000"));
    check_run_dir_length(&would_be, ctx.max_run_dir_chars)?;

    let acquisition_id =
        acquire::acquisition_id_for_input(&input, std::iter::once(&ctx.case_dir).chain(&known))
            .map_err(|e| {
                app_error(
                    fsutil::io_error_code(&e),
                    "The case's acquisitions could not be read",
                    Some(e.to_string()),
                )
            })?;

    let run_input = RunInput {
        // The path as LEAPP gets it: absolute, never canonicalized (ARCHITECTURE.md §7).
        path: argv::check_path("input", &input)?,
        kind: inspection.kind,
        input_type,
        type_detected: inspection.detected_type,
        size_bytes: inspection.size_bytes,
        itunes_encrypted: inspection.itunes_encrypted,
        acquisition_id,
        hash: record::initial_hash(inspection.kind, hash_input),
    };
    let options = RunOptions {
        timezone_supported: manifest.supports_timezone,
        timezone: timezone.clone(),
        password_supplied: password.is_some(),
        keychain_path: keychain.as_ref().map(|(path, _)| path.clone()),
        keychain_sha256: keychain.as_ref().map(|(_, hash)| hash.clone()),
    };
    let always_run = leapp_modules::always_run(tool, &ctx.tool.modules.always_run, input_type);

    let created_at = Timestamp::now();
    let (run_id, run_dir) = record::create_run_dir(&ctx.case_dir, tool, created_at)?;
    Ok(RunJob {
        run_dir,
        case_dir: ctx.case_dir,
        app_cache: ctx.paths.app_cache,
        entry: ctx.tool.entry,
        format,
        password,
        timezone,
        keychain: keychain.map(|(path, _)| PathBuf::from(path)),
        env: ctx.env,
        always_run,
        setup: RunSetup {
            run_id,
            label,
            created_at,
            host: ctx.host,
            case_snapshot: record::case_snapshot(&ctx.case),
            tool: ctx.tool.record,
            input: run_input,
            options,
            modules,
            // Built in step 2, once the run folder exists.
            command: RunCommand {
                argv: Vec::new(),
                cwd: String::new(),
            },
        },
        control: Arc::new(RunControl::default()),
    })
}

/// The keychain file: iLEAPP only, a readable regular file that passes the overlap rule; always
/// hashed. Returns its path as LEAPP gets it and its SHA-256.
fn check_keychain(
    path: &Path,
    manifest: &ToolManifest,
    overlap: &OverlapContext<'_>,
) -> Result<(String, String), AppError> {
    if !manifest.supports_keychain {
        return Err(app_error(
            ErrorCode::InvalidInput,
            format!("{} has no keychain option", manifest.display_name),
            None,
        ));
    }
    let absolute = argv::check_path("keychain", path)?;
    inspect::check_overlap(path, overlap)?;
    let not_a_file = || {
        app_error(
            ErrorCode::InvalidInput,
            "The keychain must be a readable file",
            Some(absolute.clone()),
        )
    };
    match fs::metadata(path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Err(not_a_file()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(not_a_file()),
        Err(e) => {
            return Err(app_error(
                fsutil::io_error_code(&e),
                "The keychain file could not be read",
                Some(format!("{absolute}: {e}")),
            ));
        }
    }
    let hash = hashing::sha256_file(path).map_err(|e| {
        app_error(
            fsutil::io_error_code(&e),
            "The keychain file could not be read",
            Some(format!("{absolute}: {e}")),
        )
    })?;
    Ok((absolute, hash))
}

/// iLEAPP's timezone (D19): the request's, else the case's default, else the settings' default,
/// else UTC; it must be in the installed iLEAPP's own zone list. `None` for aLEAPP, which has no
/// timezone option.
fn resolve_timezone(
    requested: Option<String>,
    ctx: &RunContext,
) -> Result<Option<String>, AppError> {
    if !ctx.tool.manifest.supports_timezone {
        return Ok(None);
    }
    let non_empty = |zone: &Option<String>| zone.clone().filter(|z| !z.trim().is_empty());
    let zone = non_empty(&requested)
        .or_else(|| non_empty(&ctx.case.default_timezone))
        .or_else(|| non_empty(&Some(ctx.settings.defaults.timezone.clone())))
        .unwrap_or_else(|| FALLBACK_TIMEZONE.to_owned());
    let known = ctx
        .tool
        .modules
        .timezones
        .as_ref()
        .is_some_and(|zones| zones.contains(&zone));
    if known {
        Ok(Some(zone))
    } else {
        Err(app_error(
            ErrorCode::InvalidTimezone,
            format!(
                "{zone} is not a timezone of the installed {}",
                ctx.tool.manifest.display_name
            ),
            None,
        ))
    }
}

/// `path_too_long` when the would-be run folder is longer than `limit` UTF-16 units.
fn check_run_dir_length(would_be: &Path, limit: Option<usize>) -> Result<(), AppError> {
    let Some(limit) = limit else {
        return Ok(());
    };
    let absolute = std::path::absolute(would_be).map_err(|e| {
        app_error(
            ErrorCode::PathNotAllowed,
            "The run folder path is unusable",
            Some(e.to_string()),
        )
    })?;
    let length = absolute.to_string_lossy().encode_utf16().count();
    if length > limit {
        return Err(app_error(
            ErrorCode::PathTooLong,
            format!(
                "The run folder path would be {length} characters long; at most {limit} are \
                 allowed. Move the case to a shorter path."
            ),
            Some(absolute.display().to_string()),
        ));
    }
    Ok(())
}

// ---- control ----

#[derive(Debug)]
struct ControlState {
    phase: RunPhase,
    cancel_requested: bool,
    process: Option<Arc<process::Handle>>,
}

/// Shared with the shell: the phase (`job_active`) and cancel (`run_cancel`).
#[derive(Debug)]
pub struct RunControl {
    state: Mutex<ControlState>,
    hash_cancel: Arc<AtomicBool>,
}

impl Default for RunControl {
    fn default() -> Self {
        Self {
            state: Mutex::new(ControlState {
                phase: RunPhase::Preparing,
                cancel_requested: false,
                process: None,
            }),
            hash_cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl RunControl {
    /// `run_cancel`: stops LEAPP's process tree if it is still running (SIGTERM, then SIGKILL after
    /// 10 s on Unix; the job is terminated on Windows) and stops input hashing. A cancel after LEAPP
    /// exited only stops the hashing. Before LEAPP is spawned it keeps LEAPP from starting.
    /// Idempotent.
    pub fn cancel(&self) {
        let mut state = lock(&self.state);
        state.cancel_requested = true;
        self.hash_cancel.store(true, Ordering::SeqCst);
        if let Some(handle) = &state.process {
            handle.cancel();
        }
    }

    pub fn phase(&self) -> RunPhase {
        lock(&self.state).phase
    }

    /// LEAPP's process id (Unix: also its process group id) once it was spawned, for diagnostics
    /// and the real-LEAPP smoke tests.
    pub fn process_id(&self) -> Option<u32> {
        lock(&self.state)
            .process
            .as_ref()
            .map(|handle| handle.pid())
    }

    fn cancel_requested(&self) -> bool {
        lock(&self.state).cancel_requested
    }

    /// Makes the process cancellable; a cancel that came in while it was being spawned applies now.
    fn attach(&self, handle: &Arc<process::Handle>) {
        let mut state = lock(&self.state);
        if state.cancel_requested {
            handle.cancel();
        }
        state.process = Some(Arc::clone(handle));
    }
}

// ---- the job ----

/// A started run: its folder exists. [`RunJob::run`] does the rest.
pub struct RunJob {
    run_dir: PathBuf,
    case_dir: PathBuf,
    app_cache: PathBuf,
    entry: PathBuf,
    format: ProfileFormat,
    /// Held until LEAPP is spawned.
    password: Option<String>,
    timezone: Option<String>,
    keychain: Option<PathBuf>,
    env: Vec<(OsString, OsString)>,
    always_run: AlwaysRunSet,
    setup: RunSetup,
    control: Arc<RunControl>,
}

/// Written by hand: the job holds the backup password.
impl fmt::Debug for RunJob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunJob")
            .field("run_id", &self.setup.run_id)
            .field("run_dir", &self.run_dir)
            .field("entry", &self.entry)
            .field("password", &self.password.as_ref().map(|_| argv::REDACTED))
            .finish_non_exhaustive()
    }
}

/// How a run ended.
#[derive(Clone, Debug)]
pub struct RunOutcome {
    /// The record as finalized (or as far as it got when the final write failed).
    pub record: RunRecord,
    pub summary: RunSummary,
    /// The final write failed: the record on disk stays `running` and becomes `interrupted` on the
    /// next open; `finished` reported `failed` with `record_write_failed`.
    pub write_error: Option<String>,
}

/// What the hashing thread reports.
enum HashMessage {
    Progress { done: u64, total: u64 },
    Done(io::Result<HashOutcome>),
}

/// The input hash running on its own thread (lifecycle step 3).
struct InputHashing {
    messages: Receiver<HashMessage>,
    started_at: Timestamp,
    result: Option<(io::Result<HashOutcome>, Timestamp)>,
}

impl InputHashing {
    fn start(input: PathBuf, cancel: Arc<AtomicBool>) -> io::Result<Self> {
        let (sender, messages) = mpsc::channel();
        let started_at = Timestamp::now();
        thread::Builder::new()
            .name("run-input-hash".to_owned())
            .spawn(move || {
                let result = hashing::sha256_file_with_progress(&input, &cancel, |done, total| {
                    let _ = sender.send(HashMessage::Progress { done, total });
                });
                let _ = sender.send(HashMessage::Done(result));
            })?;
        Ok(Self {
            messages,
            started_at,
            result: None,
        })
    }

    fn handle(&mut self, message: HashMessage, events: &mut dyn FnMut(RunEvent)) {
        match message {
            HashMessage::Progress { done, total } => events(RunEvent::HashProgress {
                bytes_done: done,
                bytes_total: total,
            }),
            HashMessage::Done(result) => self.result = Some((result, Timestamp::now())),
        }
    }

    /// Emits the progress that arrived; true once the hash is done.
    fn drain(&mut self, events: &mut dyn FnMut(RunEvent)) -> bool {
        while self.result.is_none() {
            match self.messages.try_recv() {
                Ok(message) => self.handle(message, events),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.result = Some((
                        Err(io::Error::other("the hashing thread stopped unexpectedly")),
                        Timestamp::now(),
                    ));
                }
            }
        }
        self.result.is_some()
    }

    /// Waits for the hash, emitting its progress.
    fn finish(&mut self, events: &mut dyn FnMut(RunEvent)) {
        while self.result.is_none() {
            match self.messages.recv() {
                Ok(message) => self.handle(message, events),
                Err(_) => {
                    self.result = Some((
                        Err(io::Error::other("the hashing thread stopped unexpectedly")),
                        Timestamp::now(),
                    ));
                }
            }
        }
    }
}

/// What the process step produced.
struct Spawned {
    outcome: Outcome,
    exit: Option<ExitInfo>,
}

impl RunJob {
    pub fn run_id(&self) -> &str {
        &self.setup.run_id
    }

    pub fn run_dir(&self) -> &Path {
        &self.run_dir
    }

    pub fn case_dir(&self) -> &Path {
        &self.case_dir
    }

    pub fn tool(&self) -> ToolId {
        self.setup.tool.id
    }

    pub fn created_at(&self) -> Timestamp {
        self.setup.created_at
    }

    /// For `run_cancel` and `job_active`.
    pub fn control(&self) -> Arc<RunControl> {
        Arc::clone(&self.control)
    }

    fn set_phase(&self, phase: RunPhase, on_event: &mut dyn FnMut(RunEvent)) {
        lock(&self.control.state).phase = phase;
        on_event(RunEvent::Phase { phase });
    }

    fn report_dir(&self) -> PathBuf {
        self.run_dir.join(REPORT_DIR)
    }

    /// Runs steps 2-10 on this thread, emitting events through `on_event`, and returns the final
    /// record. Blocking: it lasts as long as LEAPP and the hashing.
    pub fn run(mut self, on_event: &mut dyn FnMut(RunEvent)) -> RunOutcome {
        self.set_phase(RunPhase::Preparing, on_event);
        let password = self.password.take();
        let (mut record, prepared) = self.prepare(password);
        let command = match prepared {
            Ok(command) => command,
            Err(detail) => {
                // Step 2 failed after the run folder was created: the input was never hashed.
                if record.input.hash.status == HashStatus::Pending {
                    record.input.hash.status = HashStatus::Cancelled;
                }
                let outcome = Outcome::PrepareFailed { detail };
                return self.finish(record, &outcome, &[], false, on_event);
            }
        };

        // Step 3: the input hash, on its own thread, concurrently with LEAPP.
        let mut hashing = None;
        if record.input.hash.status == HashStatus::Pending && !self.control.cancel_requested() {
            match InputHashing::start(
                PathBuf::from(&record.input.path),
                Arc::clone(&self.control.hash_cancel),
            ) {
                Ok(started) => {
                    record.input.hash.started_at = Some(started.started_at);
                    hashing = Some(started);
                }
                Err(e) => {
                    log::warn!("run {}: cannot start input hashing: {e}", self.run_id());
                    record.input.hash.status = HashStatus::Failed;
                }
            }
        }

        // Steps 4-6: LEAPP.
        let spawned = self.spawn_and_follow(command, &mut record, &mut hashing, on_event);
        if let Err(e) = process::remove_temp_dir(&self.app_cache, self.run_id()) {
            // The startup sweep removes it later.
            log::warn!("run {}: cannot remove its temp dir: {e}", self.run_id());
        }

        // Step 7: wait for the input hash.
        if let Some(hashing) = &mut hashing {
            if !hashing.drain(on_event) {
                self.set_phase(RunPhase::HashingInput, on_event);
                hashing.finish(on_event);
            }
            apply_hash(&mut record, hashing);
        } else if record.input.hash.status == HashStatus::Pending {
            // Cancelled before hashing started.
            record.input.hash.status = HashStatus::Cancelled;
        }

        // Step 8: analyze.
        self.set_phase(RunPhase::Analyzing, on_event);
        let traceback = status::stderr_has_traceback(&self.run_dir.join(STDERR_LOG))
            .unwrap_or_else(|e| {
                log::warn!("run {}: cannot read {STDERR_LOG}: {e}", self.run_id());
                false
            });
        let outcome = self.load_failure(spawned.outcome);
        if let Some(exit) = &spawned.exit {
            record.process = Some(RunProcess {
                exit_code: exit.exit_code,
                signal: exit.signal,
                exited_at: exit.exited_at,
                cancel_requested: exit.cancel_requested,
                escalated_to_kill: exit.escalated_to_kill,
            });
        }

        // Step 9: seal.
        let mut seal_warnings = Vec::new();
        record.output.seal = if self.report_dir().is_dir() {
            self.set_phase(RunPhase::SealingReport, on_event);
            self.seal(&mut seal_warnings, on_event)
        } else {
            skipped_seal()
        };

        self.finish(record, &outcome, &seal_warnings, traceback, on_event)
    }

    /// Step 2. Returns the initial record and the LEAPP command, or the record and why preparing
    /// failed (it is then finalized as `prepare_failed`).
    fn prepare(&self, password: Option<String>) -> (RunRecord, Result<argv::LeappCommand, String>) {
        let mut setup = self.setup.clone();
        setup.command = RunCommand {
            argv: Vec::new(),
            cwd: self.run_dir.to_string_lossy().into_owned(),
        };
        let profile_path = (setup.modules.mode != ModuleMode::All)
            .then(|| self.run_dir.join(format!("profile.{}", self.format.ext)));
        let spec = ArgvSpec {
            entry: &self.entry,
            input_type: setup.input.input_type,
            input: Path::new(&setup.input.path),
            run_dir: &self.run_dir,
            profile: profile_path.as_deref(),
            timezone: self.timezone.as_deref(),
            itunes_password: password.as_deref(),
            keychain: self.keychain.as_deref(),
        };
        let command = argv::build(&spec);
        drop(password);
        if let Ok(command) = &command {
            setup.command = command.recorded();
        }
        let record = match record::initial_record(setup.clone()) {
            Ok(record) => record,
            Err(e) => return (pending_record(setup), Err(e.to_string())),
        };
        let command = match command {
            Ok(command) => command,
            Err(e) => return (record, Err(e.to_string())),
        };
        if let Err(e) = record::write_initial(&self.run_dir, &record) {
            return (record, Err(format!("writing the initial run.json: {e}")));
        }
        if let Err(e) = casedata::write(&self.run_dir, &record.case_snapshot) {
            let why = format!("writing {}: {e}", casedata::CASE_DATA_FILE);
            return (record, Err(why));
        }
        if record.modules.mode != ModuleMode::All
            && let Err(e) =
                profile::write_run_profile(&self.run_dir, &self.format, &record.modules.resolved)
        {
            return (record, Err(format!("writing the run's profile: {e}")));
        }
        if let Err(e) = process::create_temp_dir(&self.app_cache, self.run_id()) {
            return (record, Err(format!("creating the run's temp dir: {e}")));
        }
        (record, Ok(command))
    }

    /// Steps 4-6: spawn LEAPP, stream its log until it exits (or is cancelled), drain the tail and
    /// send the stdio tails.
    fn spawn_and_follow(
        &self,
        command: argv::LeappCommand,
        record: &mut RunRecord,
        hashing: &mut Option<InputHashing>,
        on_event: &mut dyn FnMut(RunEvent),
    ) -> Spawned {
        if self.control.cancel_requested() {
            // Cancelled while preparing: LEAPP is never started.
            return Spawned {
                outcome: Outcome::Exited {
                    exit_code: None,
                    signal: None,
                    cancelled_before_exit: true,
                    report: status::analyze_report(&self.report_dir()),
                },
                exit: None,
            };
        }
        let mut argv = command.argv().iter();
        let program = argv.next().map(PathBuf::from).unwrap_or_default();
        let mut spec = SpawnSpec::new(
            program,
            &self.run_dir,
            self.run_dir.join(STDOUT_LOG),
            self.run_dir.join(STDERR_LOG),
        );
        spec.args = argv.map(OsString::from).collect();
        spec.env.clone_from(&self.env);
        spec.temp_dir = process::temp_dir_path(&self.app_cache, self.run_id()).ok();
        let spawned = process::spawn(spec);
        drop(command);
        let handle = match spawned {
            Ok(handle) => Arc::new(handle),
            Err(e) => {
                log::warn!("run {}: LEAPP could not be started: {e}", self.run_id());
                return Spawned {
                    outcome: Outcome::SpawnFailed {
                        detail: e.to_string(),
                    },
                    exit: None,
                };
            }
        };
        record.started_at = Some(Timestamp::now());
        self.control.attach(&handle);
        self.set_phase(RunPhase::Running, on_event);

        let mut tail = ScreenOutputTail::new(self.screen_output());
        let mut warned = false;
        let exit = loop {
            let exit = handle.wait_timeout(tail::POLL_INTERVAL);
            let finished = !matches!(exit, Ok(None));
            let lines = if finished { tail.finish() } else { tail.poll() };
            match lines {
                Ok(lines) => emit_log(lines, on_event),
                Err(e) if !warned => {
                    log::warn!(
                        "run {}: cannot read {}: {e}",
                        self.run_id(),
                        tail.path().display()
                    );
                    warned = true;
                }
                Err(_) => {}
            }
            if let Some(hashing) = hashing.as_mut() {
                hashing.drain(on_event);
            }
            match exit {
                Ok(Some(exit)) => break Some(exit),
                Ok(None) => {}
                Err(e) => {
                    log::error!("run {}: waiting for LEAPP failed: {e}", self.run_id());
                    break None;
                }
            }
        };
        for (stream, path) in [
            (StdStream::Stdout, handle.stdout_log()),
            (StdStream::Stderr, handle.stderr_log()),
        ] {
            let lines = tail::last_lines(path, STDIO_TAIL_LINES).unwrap_or_else(|e| {
                log::warn!("run {}: cannot read {}: {e}", self.run_id(), path.display());
                Vec::new()
            });
            on_event(RunEvent::StdioTail { stream, lines });
        }
        let report = status::analyze_report(&self.report_dir());
        match exit {
            Some(exit) => Spawned {
                outcome: Outcome::Exited {
                    exit_code: exit.exit_code,
                    signal: exit.signal,
                    cancelled_before_exit: exit.cancel_requested,
                    report,
                },
                exit: Some(exit),
            },
            // The supervisor failed and stopped the tree: no exit was observed.
            None => Spawned {
                outcome: Outcome::Exited {
                    exit_code: None,
                    signal: None,
                    cancelled_before_exit: self.control.cancel_requested(),
                    report,
                },
                exit: None,
            },
        }
    }

    /// A LEAPP that exited without creating its output because the dynamic loader refused it
    /// (glibc too old for the pinned Linux build, LEAPP-CLI.md §2) never started: `spawn_failed`
    /// with the same message introspection gives ([`leapp_modules::glibc_too_old`]).
    fn load_failure(&self, outcome: Outcome) -> Outcome {
        let Outcome::Exited {
            exit_code,
            cancelled_before_exit: false,
            report,
            ..
        } = &outcome
        else {
            return outcome;
        };
        if report.dir_exists || *exit_code == Some(0) {
            return outcome;
        }
        let output = [STDERR_LOG, STDOUT_LOG]
            .map(|log| {
                tail::last_lines(&self.run_dir.join(log), STDIO_TAIL_LINES)
                    .unwrap_or_default()
                    .join("\n")
            })
            .join("\n");
        match leapp_modules::glibc_too_old(&output) {
            Some(too_old) => Outcome::SpawnFailed {
                detail: format!(
                    "{} ({})",
                    too_old.message(&self.format_display_name()),
                    too_old.loader_line
                ),
            },
            None => outcome,
        }
    }

    fn format_display_name(&self) -> String {
        match self.setup.tool.id {
            ToolId::Ileapp => "iLEAPP".to_owned(),
            ToolId::Aleapp => "aLEAPP".to_owned(),
        }
    }

    fn screen_output(&self) -> PathBuf {
        record::SCREEN_OUTPUT
            .split('/')
            .fold(self.run_dir.clone(), |path, part| path.join(part))
    }

    /// Step 9: `report.sha256` over `report/**`, with `seal_progress` events. Nothing cancels it:
    /// a cancel after the exit only stops input hashing.
    fn seal(&self, warnings: &mut Vec<Reason>, on_event: &mut dyn FnMut(RunEvent)) -> Seal {
        let never = AtomicBool::new(false);
        let result = hashing::seal_tree(
            &self.report_dir(),
            &self.run_dir.join(REPORT_MANIFEST),
            &never,
            |files_done, files_total| {
                on_event(RunEvent::SealProgress {
                    files_done,
                    files_total,
                });
            },
        );
        match result {
            Ok(sealed) => {
                warnings.extend(sealed.warnings("symlinks_in_report"));
                sealed.seal(REPORT_MANIFEST)
            }
            Err(e) => {
                log::warn!("run {}: sealing the report failed: {e}", self.run_id());
                Seal {
                    status: SealStatus::Failed,
                    manifest: None,
                    manifest_sha256: None,
                    file_count: None,
                    total_bytes: None,
                }
            }
        }
    }

    /// Step 10: the status rules, then the final record (atomic, read-only) and `finished`.
    fn finish(
        &self,
        mut record: RunRecord,
        outcome: &Outcome,
        seal_warnings: &[Reason],
        stderr_traceback: bool,
        on_event: &mut dyn FnMut(RunEvent),
    ) -> RunOutcome {
        self.set_phase(RunPhase::Finalizing, on_event);
        if record.output.seal.status == SealStatus::Pending {
            record.output.seal = skipped_seal();
        }
        let verdict = status::evaluate(&StatusInput {
            outcome,
            always_run: self.always_run.for_status(),
            stderr_traceback,
            input_hash: record.input.hash.status,
            seal: record.output.seal.status,
            seal_warnings,
        });
        record.status = verdict.status;
        record.status_reasons = verdict.reasons;
        record.warnings = verdict.warnings;
        record.leapp_result = verdict.leapp_result;
        let write = record::finalize(&self.run_dir, &mut record, Timestamp::now());
        let mut summary = case::run_summary(&DiscoveredRun {
            dir: self.run_dir.clone(),
            record: record.clone(),
        });
        let (status, reasons, write_error) = match write {
            Ok(()) => (record.status, record.status_reasons.clone(), None),
            Err(e) => {
                log::error!(
                    "run {}: the final run.json could not be written: {e}",
                    self.run_id()
                );
                summary.status = RunStatus::Failed;
                (
                    RunStatus::Failed,
                    vec![Reason {
                        code: RECORD_WRITE_FAILED.to_owned(),
                        message: format!(
                            "The final run.json could not be written ({e}); the run will show \
                             as interrupted when the case is next opened"
                        ),
                    }],
                    Some(e.to_string()),
                )
            }
        };
        on_event(RunEvent::Finished {
            status,
            reasons,
            warnings: record.warnings.clone(),
            summary: Box::new(summary.clone()),
        });
        RunOutcome {
            record,
            summary,
            write_error,
        }
    }
}

/// `log` events of at most [`MAX_BATCH_LINES`] lines.
fn emit_log(lines: Vec<String>, on_event: &mut dyn FnMut(RunEvent)) {
    let mut lines = lines.into_iter().peekable();
    while lines.peek().is_some() {
        on_event(RunEvent::Log {
            lines: lines.by_ref().take(MAX_BATCH_LINES).collect(),
        });
    }
}

fn apply_hash(record: &mut RunRecord, hashing: &mut InputHashing) {
    let hash = &mut record.input.hash;
    match hashing.result.take() {
        Some((Ok(HashOutcome::Completed(value)), at)) => {
            hash.status = HashStatus::Completed;
            hash.value = Some(value);
            hash.completed_at = Some(at);
        }
        Some((Ok(HashOutcome::Cancelled), _)) => hash.status = HashStatus::Cancelled,
        Some((Err(e), _)) => {
            log::warn!("run {}: hashing the input failed: {e}", record.run_id);
            hash.status = HashStatus::Failed;
        }
        None => hash.status = HashStatus::Failed,
    }
}

fn skipped_seal() -> Seal {
    Seal {
        status: SealStatus::SkippedNoOutput,
        manifest: None,
        manifest_sha256: None,
        file_count: None,
        total_bytes: None,
    }
}

/// The record of a setup that `initial_record` refused (a program error), so the run can still be
/// finalized as `prepare_failed`: everything known, and nothing hashed.
fn pending_record(setup: RunSetup) -> RunRecord {
    let mut input = setup.input;
    if input.kind == InputKind::Directory {
        input.hash.status = HashStatus::NotApplicable;
    }
    input.hash.value = None;
    input.hash.started_at = None;
    input.hash.completed_at = None;
    RunRecord {
        schema_version: RunRecord::SCHEMA_VERSION,
        run_id: setup.run_id,
        label: setup.label,
        status: RunStatus::Running,
        status_reasons: Vec::new(),
        warnings: Vec::new(),
        created_at: setup.created_at,
        started_at: None,
        ended_at: None,
        recovered_at: None,
        duration_ms: None,
        app: record::record_app(),
        host: setup.host,
        case_snapshot: setup.case_snapshot,
        tool: setup.tool,
        input,
        options: setup.options,
        modules: setup.modules,
        command: setup.command,
        process: None,
        leapp_result: None,
        output: crate::contracts::RunOutput {
            report_dir: REPORT_DIR.to_owned(),
            seal: Seal {
                status: SealStatus::Pending,
                manifest: None,
                manifest_sha256: None,
                file_count: None,
                total_bytes: None,
            },
        },
        logs: crate::contracts::RunLogs {
            stdout: STDOUT_LOG.to_owned(),
            stderr: STDERR_LOG.to_owned(),
            screen_output: record::SCREEN_OUTPUT.to_owned(),
        },
    }
}
