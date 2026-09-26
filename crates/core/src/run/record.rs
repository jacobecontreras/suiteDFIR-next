//! The `run.json` lifecycle: run ids, the initial record, finalize (atomic, then read-only) and
//! recovery (CONTRACTS.md §7.2).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use time::{Date, Month, Time};

use crate::case::{self, CaseError, RUN_FILE, RUNS_DIR};
use crate::contracts::{
    AppError, CaseFile, CaseSnapshot, ContractError, ErrorCode, HashAlgorithm, HashStatus,
    InputHash, InputKind, Reason, RecordApp, RecordHost, RunCommand, RunInput, RunLogs, RunModules,
    RunOptions, RunOutput, RunRecord, RunStatus, RunTool, Seal, SealStatus, Timestamp, ToolId,
    VersionedFile, parse_versioned,
};
use crate::fsutil;
use crate::hashing::to_hex;

/// The product name recorded in `app.name` (ARCHITECTURE.md D20).
pub const APP_NAME: &str = "suiteDFIR";
/// LEAPP's output folder inside the run folder (`--custom_output_folder`).
pub const REPORT_DIR: &str = "report";
/// The manifest of `report/**`, next to it (CONTRACTS.md §8).
pub const REPORT_MANIFEST: &str = "report.sha256";
pub const STDOUT_LOG: &str = "leapp.stdout.log";
pub const STDERR_LOG: &str = "leapp.stderr.log";
/// LEAPP's own log, tailed for the live view (ARCHITECTURE.md D7).
pub const SCREEN_OUTPUT: &str = "report/_HTML/_Script_Logs/Screen_Output.html";

/// The reason recorded by recovery.
pub const APP_INTERRUPTED: &str = "app_interrupted";

/// Fresh run ids tried when a folder name is taken (a collision needs the same second and the same
/// 24 random bits).
const RUN_ID_ATTEMPTS: u32 = 16;

/// Errors from writing run records.
#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("run {run_id} is already finalized")]
    AlreadyFinalized { run_id: String },
    #[error("{path} already exists")]
    AlreadyExists { path: String },
    #[error("run {run_id} cannot be finalized: {reason}")]
    NotFinal {
        run_id: String,
        reason: &'static str,
    },
    #[error("run {run_id} is not a valid initial record: {reason}")]
    NotInitial {
        run_id: String,
        reason: &'static str,
    },
    #[error("the record names run {run_id:?} but the folder is {folder}")]
    FolderMismatch { run_id: String, folder: String },
    #[error("{path}: {source}")]
    Invalid {
        path: String,
        #[source]
        source: ContractError,
    },
    #[error("no free run id in {path}")]
    NoFreeRunId { path: String },
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
    /// The final record was written (atomically, complete, with its status) but could not be
    /// made read-only, e.g. on a share that refuses permission changes. The record on disk is
    /// final: recovery never touches it.
    #[error("{path} was written but could not be made read-only: {source}")]
    NotReadOnly {
        path: String,
        #[source]
        source: io::Error,
    },
}

impl RecordError {
    fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }

    /// The `AppError` code (CONTRACTS.md §12). Everything except I/O is a program error.
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Io { source, .. } | Self::NotReadOnly { source, .. } => {
                fsutil::io_error_code(source)
            }
            _ => ErrorCode::Internal,
        }
    }
}

impl From<RecordError> for AppError {
    fn from(err: RecordError) -> Self {
        AppError {
            code: err.code(),
            message: "The run record could not be written".to_owned(),
            detail: Some(err.to_string()),
        }
    }
}

// ---- run ids ----

/// A new run id, `YYYYMMDD-HHMMSSZ-<tool>-<6 lowercase hex>` (UTC, CONTRACTS.md §7.1).
pub fn new_run_id(tool: ToolId, created_at: Timestamp) -> io::Result<String> {
    let mut random = [0u8; 3];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    let t = created_at.as_datetime();
    Ok(format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}Z-{tool}-{}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        to_hex(&random)
    ))
}

/// Whether `id` has the run id format with a real date and time and a known tool. Commands check
/// this before using an id as a folder name (ARCHITECTURE.md §9).
pub fn is_run_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('-').collect();
    let [date, time, tool, random] = parts.as_slice() else {
        return false;
    };
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    let number = |s: &str| s.parse::<u16>().ok();
    let Some(time) = time.strip_suffix('Z') else {
        return false;
    };
    if date.len() != 8 || time.len() != 6 || !digits(date) || !digits(time) {
        return false;
    }
    let valid_date = match (number(&date[..4]), number(&date[4..6]), number(&date[6..])) {
        (Some(y), Some(m), Some(d)) => Month::try_from(u8::try_from(m).unwrap_or(0))
            .ok()
            .and_then(|m| Date::from_calendar_date(i32::from(y), m, u8::try_from(d).ok()?).ok())
            .is_some(),
        _ => false,
    };
    let valid_time = match (number(&time[..2]), number(&time[2..4]), number(&time[4..])) {
        (Some(h), Some(m), Some(s)) => {
            let byte = |v: u16| u8::try_from(v).unwrap_or(u8::MAX);
            Time::from_hms(byte(h), byte(m), byte(s)).is_ok()
        }
        _ => false,
    };
    valid_date
        && valid_time
        && ToolId::ALL.iter().any(|t| t.as_str() == *tool)
        && random.len() == 6
        && random
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Creates `<case>/runs/<run_id>/` with a fresh run id (lifecycle step 2) and returns both. The
/// folder is created with a plain `create_dir`, so an id is never reused.
pub fn create_run_dir(
    case_dir: &Path,
    tool: ToolId,
    created_at: Timestamp,
) -> Result<(String, PathBuf), RecordError> {
    let runs = case_dir.join(RUNS_DIR);
    fs::create_dir_all(&runs).map_err(|e| RecordError::io(&runs, e))?;
    for _ in 0..RUN_ID_ATTEMPTS {
        let run_id = new_run_id(tool, created_at).map_err(|e| RecordError::io(&runs, e))?;
        let dir = runs.join(&run_id);
        match fs::create_dir(&dir) {
            Ok(()) => return Ok((run_id, dir)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(RecordError::io(&dir, e)),
        }
    }
    Err(RecordError::NoFreeRunId {
        path: runs.display().to_string(),
    })
}

// ---- the initial record ----

/// The app that writes the records: this build.
pub fn record_app() -> RecordApp {
    RecordApp {
        name: APP_NAME.to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
    }
}

/// The case metadata recorded with a run.
pub fn case_snapshot(case: &CaseFile) -> CaseSnapshot {
    CaseSnapshot {
        case_id: case.case_id.clone(),
        name: case.name.clone(),
        case_number: case.case_number.clone(),
        examiner: case.examiner.clone(),
        agency: case.agency.clone(),
    }
}

/// `input.hash` of a new run: `not_applicable` for directories, else `pending` when hashing was
/// requested and `not_requested` when it was not (CONTRACTS.md §7.2).
pub fn initial_hash(kind: InputKind, requested: bool) -> InputHash {
    let status = match (kind, requested) {
        (InputKind::Directory, _) => HashStatus::NotApplicable,
        (InputKind::File, true) => HashStatus::Pending,
        (InputKind::File, false) => HashStatus::NotRequested,
    };
    InputHash {
        algorithm: HashAlgorithm::Sha256,
        status,
        value: None,
        started_at: None,
        completed_at: None,
    }
}

/// Everything known about a run when it is prepared. The parts come from the case, the verified
/// tool, input inspection, module resolution ([`super::profile::resolve`]) and argv building
/// ([`super::argv`]); `input.hash` is [`initial_hash`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunSetup {
    pub run_id: String,
    pub label: Option<String>,
    pub created_at: Timestamp,
    pub host: RecordHost,
    pub case_snapshot: CaseSnapshot,
    pub tool: RunTool,
    pub input: RunInput,
    pub options: RunOptions,
    pub modules: RunModules,
    pub command: RunCommand,
}

/// The initial record (CONTRACTS.md §7.2): every known field, `status: running`, and `null` or
/// `pending` for everything that is not known yet. `input.hash` must be an initial one (see
/// [`initial_hash`]): `not_applicable` for a directory, `pending` or `not_requested` for a file, and
/// no value or times.
pub fn initial_record(setup: RunSetup) -> Result<RunRecord, RecordError> {
    let hash = &setup.input.hash;
    let status_fits = match setup.input.kind {
        InputKind::Directory => hash.status == HashStatus::NotApplicable,
        InputKind::File => matches!(hash.status, HashStatus::Pending | HashStatus::NotRequested),
    };
    let reason = if !status_fits {
        Some("input.hash.status must be not_applicable (directory) or pending/not_requested (file)")
    } else if hash.value.is_some() || hash.started_at.is_some() || hash.completed_at.is_some() {
        Some("input.hash has a value or times before hashing started")
    } else {
        None
    };
    if let Some(reason) = reason {
        return Err(RecordError::NotInitial {
            run_id: setup.run_id,
            reason,
        });
    }
    Ok(RunRecord {
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
        app: record_app(),
        host: setup.host,
        case_snapshot: setup.case_snapshot,
        tool: setup.tool,
        input: setup.input,
        options: setup.options,
        modules: setup.modules,
        command: setup.command,
        process: None,
        leapp_result: None,
        output: RunOutput {
            report_dir: REPORT_DIR.to_owned(),
            seal: Seal {
                status: SealStatus::Pending,
                manifest: None,
                manifest_sha256: None,
                file_count: None,
                total_bytes: None,
            },
        },
        logs: RunLogs {
            stdout: STDOUT_LOG.to_owned(),
            stderr: STDERR_LOG.to_owned(),
            screen_output: SCREEN_OUTPUT.to_owned(),
        },
    })
}

/// Writes the initial record into its (new) run folder, atomically. A `run.json` that already
/// exists is never replaced this way.
pub fn write_initial(run_dir: &Path, record: &RunRecord) -> Result<(), RecordError> {
    check_folder(run_dir, record)?;
    if record.status != RunStatus::Running {
        return Err(RecordError::NotInitial {
            run_id: record.run_id.clone(),
            reason: "an initial record has status running",
        });
    }
    let file = run_dir.join(RUN_FILE);
    match fs::symlink_metadata(&file) {
        Ok(_) => {
            return Err(RecordError::AlreadyExists {
                path: file.display().to_string(),
            });
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(RecordError::io(&file, e)),
    }
    fsutil::write_json_atomic(&file, record).map_err(|e| RecordError::io(&file, e))
}

// ---- finalize and recovery ----

/// Finalizes a run (lifecycle step 10): sets `ended_at` and `duration_ms`, writes the complete
/// record atomically and makes it read-only. The record must carry a final status (not `running`,
/// and not `interrupted`, which only [`recover`] writes) and no `pending` or `interrupted` hash or
/// seal status. A run that is already final on disk is rejected, so a record is never finalized
/// twice.
pub fn finalize(
    run_dir: &Path,
    record: &mut RunRecord,
    ended_at: Timestamp,
) -> Result<(), RecordError> {
    let not_final = |reason| RecordError::NotFinal {
        run_id: record.run_id.clone(),
        reason,
    };
    match record.status {
        RunStatus::Running => return Err(not_final("the status is still running")),
        RunStatus::Interrupted => {
            return Err(not_final("interrupted runs are finalized by recovery"));
        }
        _ => {}
    }
    match record.input.hash.status {
        HashStatus::Pending => return Err(not_final("the input hash is still pending")),
        HashStatus::Interrupted => {
            return Err(not_final(
                "an interrupted input hash is written only by recovery",
            ));
        }
        _ => {}
    }
    match record.output.seal.status {
        SealStatus::Pending => return Err(not_final("the report seal is still pending")),
        SealStatus::Interrupted => {
            return Err(not_final("an interrupted seal is written only by recovery"));
        }
        _ => {}
    }
    record.ended_at = Some(ended_at);
    record.recovered_at = None;
    record.duration_ms = Some(duration_ms(record.created_at, ended_at));
    write_final(run_dir, record)
}

/// `ended_at − created_at` in milliseconds (0 if the clock went backwards).
fn duration_ms(created_at: Timestamp, ended_at: Timestamp) -> u64 {
    let millis = (ended_at.as_datetime() - created_at.as_datetime()).whole_milliseconds();
    u64::try_from(millis).unwrap_or(0)
}

/// Recovers a run left `running` by a crash (CONTRACTS.md §7.2): `interrupted` with
/// `app_interrupted`, `recovered_at` set, `pending` hash and seal statuses become `interrupted`,
/// then the record is finalized (atomic write, read-only).
pub fn recover(run_dir: &Path, record: &mut RunRecord, now: Timestamp) -> Result<(), RecordError> {
    recover_with(
        run_dir,
        record,
        now,
        "The app stopped before the run finished; the record was recovered when the case was \
         next opened",
    )
}

/// The message of `app_interrupted` for a run whose thread stopped by an internal error (a
/// panic) while the app kept running; its process tree was stopped first.
pub const INTERNAL_ERROR_MESSAGE: &str = "The run stopped because of an internal error in \
     suiteDFIR; LEAPP was stopped and the record was recovered at once";

/// [`recover`] with the message of its `app_interrupted` reason (the code is the same).
pub fn recover_with(
    run_dir: &Path,
    record: &mut RunRecord,
    now: Timestamp,
    message: &str,
) -> Result<(), RecordError> {
    if record.status != RunStatus::Running {
        return Err(RecordError::AlreadyFinalized {
            run_id: record.run_id.clone(),
        });
    }
    record.status = RunStatus::Interrupted;
    record.status_reasons = vec![Reason {
        code: APP_INTERRUPTED.to_owned(),
        message: message.to_owned(),
    }];
    record.recovered_at = Some(now);
    record.ended_at = None;
    record.duration_ms = None;
    if record.input.hash.status == HashStatus::Pending {
        record.input.hash.status = HashStatus::Interrupted;
    }
    if record.output.seal.status == SealStatus::Pending {
        record.output.seal.status = SealStatus::Interrupted;
    }
    write_final(run_dir, record)
}

/// Recovery on `case_open`: every `running` run of the case except `active_run_id` (this process's
/// active run) is recovered. Returns the recovered run ids; a run that cannot be recovered is
/// logged and left as it is.
pub fn recover_case(
    case_dir: &Path,
    active_run_id: Option<&str>,
    now: Timestamp,
) -> Result<Vec<String>, CaseError> {
    let mut recovered = Vec::new();
    for mut run in case::discover_runs(case_dir)? {
        if run.record.status != RunStatus::Running
            || Some(run.record.run_id.as_str()) == active_run_id
        {
            continue;
        }
        match recover(&run.dir, &mut run.record, now) {
            Ok(()) => recovered.push(run.record.run_id),
            // Written as interrupted, only not read-only: it was recovered.
            Err(e @ RecordError::NotReadOnly { .. }) => {
                log::warn!("recovered run {}, but {e}", run.dir.display());
                recovered.push(run.record.run_id);
            }
            Err(e) => log::warn!("could not recover run {}: {e}", run.dir.display()),
        }
    }
    Ok(recovered)
}

/// The final write shared by [`finalize`] and [`recover`]: refuse if the record on disk is already
/// final (read-only, or any status but `running`), then write atomically and mark read-only
/// ([`RecordError::NotReadOnly`] when only that last step fails).
fn write_final(run_dir: &Path, record: &RunRecord) -> Result<(), RecordError> {
    #[cfg(test)]
    if tests::FAIL_READ_ONLY.with(std::cell::Cell::get) {
        return write_final_marking(run_dir, record, |_| {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        });
    }
    write_final_marking(run_dir, record, fsutil::set_read_only)
}

/// [`write_final`] with the read-only step passed in (tests make it fail).
fn write_final_marking(
    run_dir: &Path,
    record: &RunRecord,
    mark_read_only: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<(), RecordError> {
    check_folder(run_dir, record)?;
    let file = run_dir.join(RUN_FILE);
    let already = || RecordError::AlreadyFinalized {
        run_id: record.run_id.clone(),
    };
    match fs::symlink_metadata(&file) {
        Ok(meta) if meta.permissions().readonly() => return Err(already()),
        Ok(_) => {
            let bytes = fs::read(&file).map_err(|e| RecordError::io(&file, e))?;
            let on_disk: RunRecord =
                parse_versioned(&bytes).map_err(|source| RecordError::Invalid {
                    path: file.display().to_string(),
                    source,
                })?;
            if on_disk.status != RunStatus::Running {
                return Err(already());
            }
        }
        // The initial write failed (lifecycle step 2): the final record is still written.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(RecordError::io(&file, e)),
    }
    fsutil::write_json_atomic(&file, record).map_err(|e| RecordError::io(&file, e))?;
    mark_read_only(&file).map_err(|source| RecordError::NotReadOnly {
        path: file.display().to_string(),
        source,
    })
}

/// The record must live in the folder named after its run id.
fn check_folder(run_dir: &Path, record: &RunRecord) -> Result<(), RecordError> {
    let folder = run_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if folder == record.run_id {
        Ok(())
    } else {
        Err(RecordError::FolderMismatch {
            run_id: record.run_id.clone(),
            folder,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::contracts::{RunProcess, examples};
    use crate::fsutil::test_support::make_writable;

    thread_local! {
        /// This test thread's final writes cannot mark the record read-only (see `write_final`).
        pub(super) static FAIL_READ_ONLY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    const RUN_ID: &str = "20260924-183005Z-ileapp-3f9a1c";

    fn at(text: &str) -> Timestamp {
        Timestamp::parse(text).unwrap()
    }

    fn setup_from_example() -> RunSetup {
        let example = examples::run_record_initial();
        RunSetup {
            run_id: example.run_id,
            label: example.label,
            created_at: example.created_at,
            host: example.host,
            case_snapshot: case_snapshot(&examples::case_file()),
            tool: example.tool,
            input: example.input,
            options: example.options,
            modules: example.modules,
            command: example.command,
        }
    }

    fn run_dir(case_dir: &Path) -> PathBuf {
        let dir = case_dir.join(RUNS_DIR).join(RUN_ID);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A finished record as the runner would build it before finalizing.
    fn finished(mut record: RunRecord) -> RunRecord {
        let example = examples::run_record();
        record.status = example.status;
        record.status_reasons = example.status_reasons;
        record.warnings = example.warnings;
        record.started_at = example.started_at;
        record.process = example.process;
        record.leapp_result = example.leapp_result;
        record.output = example.output;
        record
    }

    fn read(dir: &Path) -> RunRecord {
        parse_versioned(&fs::read(dir.join(RUN_FILE)).unwrap()).unwrap()
    }

    fn is_read_only(dir: &Path) -> bool {
        fs::metadata(dir.join(RUN_FILE))
            .unwrap()
            .permissions()
            .readonly()
    }

    #[test]
    fn run_ids_have_the_contract_format() {
        let created = at("2026-09-24T18:30:05Z");
        let id = new_run_id(ToolId::Ileapp, created).unwrap();
        assert!(id.starts_with("20260924-183005Z-ileapp-"), "{id}");
        assert_eq!(id.len(), RUN_ID.len());
        assert!(is_run_id(&id), "{id}");
        let other = new_run_id(ToolId::Aleapp, at("2027-01-02T03:04:05Z")).unwrap();
        assert!(other.starts_with("20270102-030405Z-aleapp-"), "{other}");
        assert!(is_run_id(&other));
    }

    #[test]
    fn run_id_validation() {
        assert!(is_run_id(RUN_ID));
        assert!(is_run_id("20240229-235959Z-aleapp-000000"));
        for bad in [
            "",
            "20260924-183005Z-ileapp-3F9A1C",
            "20260924-183005Z-ileapp-3f9a1",
            "20260924-183005Z-ileapp-3f9a1cc",
            "20260924-183005Z-ileapp-3f9a1g",
            "20260924-183005-ileapp-3f9a1c",
            "20260924-183005z-ileapp-3f9a1c",
            "2026092-183005Z-ileapp-3f9a1c",
            "20260924-183005Z-xleapp-3f9a1c",
            "20260924-183005Z-ios-3f9a1c",
            "20261324-183005Z-ileapp-3f9a1c",
            "20230229-183005Z-ileapp-3f9a1c",
            "20260924-243005Z-ileapp-3f9a1c",
            "20260924-186005Z-ileapp-3f9a1c",
            "20260924-183005Z-ileapp-3f9a1c-x",
            "../20260924-183005Z-ileapp-3f9a1c",
            "20260924-183005Z-ileapp-3f9a1c/..",
            "+0260924-183005Z-ileapp-3f9a1c",
        ] {
            assert!(!is_run_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn create_run_dir_makes_runs_and_a_fresh_folder() {
        let dir = tempfile::tempdir().unwrap();
        let created = at("2026-09-24T18:30:05Z");
        let (id, run_dir) = create_run_dir(dir.path(), ToolId::Ileapp, created).unwrap();
        assert!(is_run_id(&id));
        assert_eq!(run_dir, dir.path().join("runs").join(&id));
        assert!(run_dir.is_dir());
        let (id2, _) = create_run_dir(dir.path(), ToolId::Ileapp, created).unwrap();
        assert_ne!(id, id2);
    }

    #[test]
    fn initial_hash_statuses() {
        assert_eq!(
            initial_hash(InputKind::Directory, true).status,
            HashStatus::NotApplicable
        );
        assert_eq!(
            initial_hash(InputKind::Directory, false).status,
            HashStatus::NotApplicable
        );
        assert_eq!(
            initial_hash(InputKind::File, true).status,
            HashStatus::Pending
        );
        assert_eq!(
            initial_hash(InputKind::File, false).status,
            HashStatus::NotRequested
        );
        let hash = initial_hash(InputKind::File, true);
        assert_eq!(hash.algorithm, HashAlgorithm::Sha256);
        assert_eq!(
            (hash.value, hash.started_at, hash.completed_at),
            (None, None, None)
        );
    }

    #[test]
    fn initial_record_is_7_2() {
        let record = initial_record(setup_from_example()).unwrap();
        assert_eq!(record, examples::run_record_initial());
        assert_eq!(record_app(), examples::run_record().app);
    }

    #[test]
    fn initial_record_needs_an_initial_hash() {
        let with = |kind: InputKind, hash: InputHash| {
            let mut setup = setup_from_example();
            setup.input.kind = kind;
            setup.input.hash = hash;
            initial_record(setup)
        };
        // The three §7.2 statuses, each for its kind.
        for (kind, requested) in [
            (InputKind::Directory, false),
            (InputKind::Directory, true),
            (InputKind::File, true),
            (InputKind::File, false),
        ] {
            assert!(with(kind, initial_hash(kind, requested)).is_ok());
        }
        let hash = |status| InputHash {
            status,
            ..initial_hash(InputKind::File, true)
        };
        for (kind, status) in [
            (InputKind::Directory, HashStatus::Pending),
            (InputKind::Directory, HashStatus::NotRequested),
            (InputKind::File, HashStatus::NotApplicable),
            (InputKind::File, HashStatus::Completed),
            (InputKind::File, HashStatus::Interrupted),
            (InputKind::File, HashStatus::Cancelled),
            (InputKind::File, HashStatus::Failed),
        ] {
            let err = with(kind, hash(status)).unwrap_err();
            assert!(
                matches!(err, RecordError::NotInitial { .. }),
                "{kind} {status}: {err:?}"
            );
        }
        let mut valued = initial_hash(InputKind::File, true);
        valued.value = Some("ab".repeat(32));
        assert!(with(InputKind::File, valued).is_err());
        let mut timed = initial_hash(InputKind::File, true);
        timed.started_at = Some(at("2026-09-24T18:30:05Z"));
        assert!(with(InputKind::File, timed).is_err());
    }

    #[test]
    fn write_initial_creates_run_json_once() {
        let case = tempfile::tempdir().unwrap();
        let dir = run_dir(case.path());
        let record = initial_record(setup_from_example()).unwrap();
        write_initial(&dir, &record).unwrap();
        assert_eq!(read(&dir), record);
        assert!(!is_read_only(&dir), "the running record is not final yet");
        let err = write_initial(&dir, &record).unwrap_err();
        assert!(matches!(err, RecordError::AlreadyExists { .. }), "{err:?}");

        let other = tempfile::tempdir().unwrap();
        let wrong = other.path().join("20260924-183005Z-ileapp-000000");
        fs::create_dir_all(&wrong).unwrap();
        let err = write_initial(&wrong, &record).unwrap_err();
        assert!(matches!(err, RecordError::FolderMismatch { .. }), "{err:?}");
        assert!(!wrong.join(RUN_FILE).exists());
    }

    #[test]
    fn finalize_writes_read_only_and_only_once() {
        let case = tempfile::tempdir().unwrap();
        let dir = run_dir(case.path());
        let initial = initial_record(setup_from_example()).unwrap();
        write_initial(&dir, &initial).unwrap();

        let mut record = finished(initial);
        finalize(&dir, &mut record, at("2026-09-24T18:52:41Z")).unwrap();
        assert_eq!(record, examples::run_record(), "the §7.1 example");
        assert_eq!(read(&dir), record);
        assert!(is_read_only(&dir));

        // Finalizing twice is rejected and leaves the record byte-identical.
        let sealed = fs::read(dir.join(RUN_FILE)).unwrap();
        let mut again = record.clone();
        again.status = RunStatus::Failed;
        let err = finalize(&dir, &mut again, at("2026-09-24T19:00:00Z")).unwrap_err();
        assert!(
            matches!(err, RecordError::AlreadyFinalized { .. }),
            "{err:?}"
        );
        assert_eq!(err.code(), ErrorCode::Internal);
        assert_eq!(fs::read(dir.join(RUN_FILE)).unwrap(), sealed);
        // Recovery does not touch it either.
        let mut recovered = record.clone();
        recovered.status = RunStatus::Running;
        assert!(matches!(
            recover(&dir, &mut recovered, at("2026-09-25T00:00:00Z")).unwrap_err(),
            RecordError::AlreadyFinalized { .. }
        ));
        assert_eq!(fs::read(dir.join(RUN_FILE)).unwrap(), sealed);
        make_writable(&dir.join(RUN_FILE));
    }

    #[test]
    fn finalize_twice_is_rejected_even_if_read_only_was_lost() {
        let case = tempfile::tempdir().unwrap();
        let dir = run_dir(case.path());
        let mut record = finished(initial_record(setup_from_example()).unwrap());
        finalize(&dir, &mut record, at("2026-09-24T18:52:41Z")).unwrap();
        // E.g. the read-only mark failed or was removed: the status on disk still says final.
        make_writable(&dir.join(RUN_FILE));
        let err = finalize(&dir, &mut record, at("2026-09-24T19:00:00Z")).unwrap_err();
        assert!(
            matches!(err, RecordError::AlreadyFinalized { .. }),
            "{err:?}"
        );
        assert_eq!(read(&dir).ended_at, Some(at("2026-09-24T18:52:41Z")));
    }

    #[test]
    fn a_final_record_that_cannot_be_made_read_only_is_still_final() {
        let case = tempfile::tempdir().unwrap();
        let dir = run_dir(case.path());
        let initial = initial_record(setup_from_example()).unwrap();
        write_initial(&dir, &initial).unwrap();
        let mut record = finished(initial);
        record.ended_at = Some(at("2026-09-24T18:52:41Z"));
        record.duration_ms = Some(1_356_000);
        // E.g. a share that refuses permission changes.
        let err = write_final_marking(&dir, &record, |_| {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        })
        .unwrap_err();
        assert!(matches!(err, RecordError::NotReadOnly { .. }), "{err:?}");
        assert_eq!(err.code(), ErrorCode::PermissionDenied);
        // The complete final record is on disk, writable, and recovery leaves it alone.
        assert_eq!(read(&dir), record);
        assert!(!is_read_only(&dir));
        let mut stale = record.clone();
        stale.status = RunStatus::Running;
        assert!(matches!(
            recover(&dir, &mut stale, at("2026-09-25T00:00:00Z")).unwrap_err(),
            RecordError::AlreadyFinalized { .. }
        ));
        assert_eq!(read(&dir), record);
    }

    #[test]
    fn a_recovery_that_cannot_mark_read_only_still_counts_as_recovered() {
        let case = tempfile::tempdir().unwrap();
        let dir = run_dir(case.path());
        let initial = initial_record(setup_from_example()).unwrap();
        write_initial(&dir, &initial).unwrap();
        FAIL_READ_ONLY.with(|fail| fail.set(true));
        let recovered = recover_case(case.path(), None, at("2026-09-25T00:00:00Z"));
        FAIL_READ_ONLY.with(|fail| fail.set(false));
        // On disk it is interrupted (only writable), so the case open reports it.
        assert_eq!(recovered.unwrap(), [RUN_ID]);
        assert_eq!(read(&dir).status, RunStatus::Interrupted);
        assert!(!is_read_only(&dir));
    }

    #[test]
    fn a_panic_recovery_says_so() {
        let case = tempfile::tempdir().unwrap();
        let dir = run_dir(case.path());
        let mut record = initial_record(setup_from_example()).unwrap();
        write_initial(&dir, &record).unwrap();
        recover_with(
            &dir,
            &mut record,
            at("2026-09-24T18:40:00Z"),
            INTERNAL_ERROR_MESSAGE,
        )
        .unwrap();
        let on_disk = read(&dir);
        assert_eq!(on_disk.status, RunStatus::Interrupted);
        assert_eq!(on_disk.status_reasons[0].code, APP_INTERRUPTED);
        assert_eq!(on_disk.status_reasons[0].message, INTERNAL_ERROR_MESSAGE);
        assert!(is_read_only(&dir));
        make_writable(&dir.join(RUN_FILE));
    }

    #[test]
    fn finalize_requires_a_final_record() {
        let case = tempfile::tempdir().unwrap();
        let dir = run_dir(case.path());
        let initial = initial_record(setup_from_example()).unwrap();
        write_initial(&dir, &initial).unwrap();

        let mut running = initial.clone();
        let mut interrupted = finished(initial.clone());
        interrupted.status = RunStatus::Interrupted;
        let mut hashing = finished(initial.clone());
        hashing.input.hash.status = HashStatus::Pending;
        let mut sealing = finished(initial.clone());
        sealing.output.seal.status = SealStatus::Pending;
        // Interrupted hash and seal statuses are written only by recovery.
        let mut hash_interrupted = finished(initial.clone());
        hash_interrupted.input.hash.status = HashStatus::Interrupted;
        let mut seal_interrupted = finished(initial.clone());
        seal_interrupted.output.seal.status = SealStatus::Interrupted;
        for record in [
            &mut running,
            &mut interrupted,
            &mut hashing,
            &mut sealing,
            &mut hash_interrupted,
            &mut seal_interrupted,
        ] {
            let err = finalize(&dir, record, at("2026-09-24T18:52:41Z")).unwrap_err();
            assert!(matches!(err, RecordError::NotFinal { .. }), "{err:?}");
        }
        assert_eq!(read(&dir), initial, "untouched");
        assert!(!is_read_only(&dir));
    }

    #[test]
    fn finalize_without_an_initial_record_still_writes() {
        // Lifecycle step 2: the initial write failed, then the run is finalized as prepare_failed.
        let case = tempfile::tempdir().unwrap();
        let dir = run_dir(case.path());
        let mut record = initial_record(setup_from_example()).unwrap();
        record.status = RunStatus::Failed;
        record.status_reasons = vec![Reason {
            code: "prepare_failed".to_owned(),
            message: "disk full".to_owned(),
        }];
        record.output.seal.status = SealStatus::SkippedNoOutput;
        finalize(&dir, &mut record, at("2026-09-24T18:30:05Z")).unwrap();
        assert_eq!(record.duration_ms, Some(0));
        assert_eq!(read(&dir), record);
        assert!(is_read_only(&dir));
        make_writable(&dir.join(RUN_FILE));
    }

    #[test]
    fn duration_is_ended_minus_created() {
        assert_eq!(
            duration_ms(at("2026-09-24T18:30:05Z"), at("2026-09-24T18:52:41Z")),
            1_356_000
        );
        assert_eq!(
            duration_ms(at("2026-09-24T18:30:05Z"), at("2026-09-24T18:30:00Z")),
            0
        );
    }

    #[test]
    fn a_crash_during_finalize_leaves_the_running_record() {
        let case = tempfile::tempdir().unwrap();
        let dir = run_dir(case.path());
        let initial = initial_record(setup_from_example()).unwrap();
        write_initial(&dir, &initial).unwrap();
        let file = dir.join(RUN_FILE);
        let final_bytes = serde_json::to_vec_pretty(&examples::run_record()).unwrap();
        let crash = fsutil::write_atomic(&file, &final_bytes, |_| {
            Err(io::Error::other("simulated crash"))
        });
        assert!(crash.is_err());
        // Still the complete initial record, so the next case open recovers it.
        assert_eq!(read(&dir), initial);
        let recovered = recover_case(case.path(), None, at("2026-09-25T08:00:00Z")).unwrap();
        assert_eq!(recovered, [RUN_ID]);
        make_writable(&file);
    }

    #[test]
    fn recovery_marks_interrupted_and_finalizes() {
        let case = tempfile::tempdir().unwrap();
        let dir = run_dir(case.path());
        // A file input whose hash was still running when the app died.
        let mut setup = setup_from_example();
        setup.input.kind = InputKind::File;
        setup.input.hash = initial_hash(InputKind::File, true);
        let initial = initial_record(setup).unwrap();
        assert_eq!(initial.input.hash.status, HashStatus::Pending);
        write_initial(&dir, &initial).unwrap();

        let now = at("2026-09-25T08:00:00Z");
        let recovered = recover_case(case.path(), None, now).unwrap();
        assert_eq!(recovered, [RUN_ID]);
        let record = read(&dir);
        assert_eq!(record.status, RunStatus::Interrupted);
        assert_eq!(record.status_reasons.len(), 1);
        assert_eq!(record.status_reasons[0].code, "app_interrupted");
        assert_eq!(record.recovered_at, Some(now));
        assert_eq!((record.ended_at, record.duration_ms), (None, None));
        assert_eq!(record.input.hash.status, HashStatus::Interrupted);
        assert_eq!(record.output.seal.status, SealStatus::Interrupted);
        assert_eq!(record.warnings, vec![]);
        // Everything else is as it was.
        let mut expected = initial.clone();
        expected.status = record.status;
        expected.status_reasons = record.status_reasons.clone();
        expected.recovered_at = record.recovered_at;
        expected.input.hash.status = HashStatus::Interrupted;
        expected.output.seal.status = SealStatus::Interrupted;
        assert_eq!(record, expected);
        assert!(is_read_only(&dir));

        // A second open finds nothing to recover.
        assert_eq!(
            recover_case(case.path(), None, now).unwrap(),
            Vec::<String>::new()
        );
        make_writable(&dir.join(RUN_FILE));
    }

    #[test]
    fn recovery_keeps_non_pending_statuses_and_skips_the_active_run() {
        let case = tempfile::tempdir().unwrap();
        let active = case
            .path()
            .join(RUNS_DIR)
            .join("20260925-080000Z-aleapp-aaaaaa");
        fs::create_dir_all(&active).unwrap();
        let mut active_record = initial_record(setup_from_example()).unwrap();
        active_record.run_id = "20260925-080000Z-aleapp-aaaaaa".to_owned();
        write_initial(&active, &active_record).unwrap();

        let dir = run_dir(case.path());
        let mut running = initial_record(setup_from_example()).unwrap();
        // Hashing had finished and LEAPP had exited when the app died.
        running.input.hash.status = HashStatus::Completed;
        running.started_at = Some(at("2026-09-24T18:30:06Z"));
        running.process = Some(RunProcess {
            exit_code: Some(0),
            signal: None,
            exited_at: at("2026-09-24T18:51:10Z"),
            cancel_requested: false,
            escalated_to_kill: false,
        });
        write_initial(&dir, &running).unwrap();

        let recovered = recover_case(
            case.path(),
            Some("20260925-080000Z-aleapp-aaaaaa"),
            at("2026-09-25T09:00:00Z"),
        )
        .unwrap();
        assert_eq!(recovered, [RUN_ID]);
        let record = read(&dir);
        assert_eq!(record.input.hash.status, HashStatus::Completed);
        assert_eq!(record.process, running.process);
        assert_eq!(read(&active).status, RunStatus::Running, "the active run");
        assert!(!is_read_only(&active));
        make_writable(&dir.join(RUN_FILE));
    }

    #[test]
    fn recovery_leaves_final_records_alone() {
        let case = tempfile::tempdir().unwrap();
        let dir = run_dir(case.path());
        let mut record = finished(initial_record(setup_from_example()).unwrap());
        finalize(&dir, &mut record, at("2026-09-24T18:52:41Z")).unwrap();
        let before = fs::read(dir.join(RUN_FILE)).unwrap();
        assert_eq!(
            recover_case(case.path(), None, at("2026-09-25T00:00:00Z")).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(fs::read(dir.join(RUN_FILE)).unwrap(), before);
        make_writable(&dir.join(RUN_FILE));
    }
}
