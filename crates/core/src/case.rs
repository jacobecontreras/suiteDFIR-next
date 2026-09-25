//! Cases (CONTRACTS.md §6, ARCHITECTURE.md §8): create (folder naming and collision rules), open,
//! update, the recent list, and run discovery via `runs/*/run.json`.
//!
//! Opening a case marks it known ([`open`]); forgetting it only removes it from the recent list
//! ([`crate::settings::forget_recent`]). The folder is never renamed or deleted.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::contracts::{
    AppError, CaseFields, CaseFile, CaseSummary, ContractError, ErrorCode, RunRecord, RunSummary,
    Settings, Timestamp, VersionedFile, parse_versioned,
};
use crate::fsutil;
use crate::hashing::to_hex;
use crate::settings;

/// The case record inside a case folder.
pub const CASE_FILE: &str = "case.json";
/// The folder holding a case's runs.
pub const RUNS_DIR: &str = "runs";
/// The record inside each run folder.
pub const RUN_FILE: &str = "run.json";

/// `name` is 1–120 characters (§6).
pub const MAX_NAME_CHARS: usize = 120;

/// Collisions are resolved with ` (2)`, ` (3)`, … up to this suffix.
const MAX_COLLISION_SUFFIX: u32 = 999;

/// Characters that are replaced by `_` in folder and file names (besides control characters).
const RESERVED_CHARS: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Windows device names, reserved in any case and with any extension (`con.txt`, `Nul .tar`).
/// Microsoft's list includes the superscript-digit ports.
const WINDOWS_RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM0", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
    "COM8", "COM9", "COM¹", "COM²", "COM³", "LPT0", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6",
    "LPT7", "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
];

/// Errors from case operations.
#[derive(Debug, thiserror::Error)]
pub enum CaseError {
    #[error("invalid case name: {0}")]
    InvalidName(&'static str),
    #[error("{path} is not a case folder (no {CASE_FILE})")]
    NotFound { path: String },
    #[error("{path} is not a known case folder; open it first")]
    NotKnown { path: String },
    #[error("{path}: {source}")]
    Invalid {
        path: String,
        #[source]
        source: ContractError,
    },
    #[error("{path}: {reason}")]
    InvalidContent { path: String, reason: String },
    #[error("no free folder name for {base:?} in {parent}")]
    NoFreeName { parent: String, base: String },
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
}

impl CaseError {
    fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }

    /// The `AppError` code (CONTRACTS.md §12).
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidName(_) | Self::Invalid { .. } | Self::InvalidContent { .. } => {
                ErrorCode::InvalidCase
            }
            Self::NotFound { .. } | Self::NotKnown { .. } => ErrorCode::CaseNotFound,
            Self::NoFreeName { .. } => ErrorCode::CaseExists,
            Self::Io { source, .. } => fsutil::io_error_code(source),
        }
    }
}

impl From<CaseError> for AppError {
    fn from(err: CaseError) -> Self {
        let message = match err.code() {
            ErrorCode::InvalidCase => "The case is not valid",
            ErrorCode::CaseNotFound => "The case folder was not found",
            ErrorCode::CaseExists => "A case folder with this name already exists",
            ErrorCode::PermissionDenied => "Access to the case folder was denied",
            _ => "The case folder could not be read or written",
        };
        AppError {
            code: err.code(),
            message: message.to_owned(),
            detail: Some(err.to_string()),
        }
    }
}

// ---- names ----

/// Turns a name into a safe single path component (CONTRACTS.md §6, also used for profile names):
/// `<>:"/\|?*` and control characters become `_`, trailing dots and spaces are trimmed, and Windows
/// device names get a `_` after the device part (`CON` → `CON_`, `nul.txt` → `nul_.txt`). A name
/// with nothing left (`...`) becomes `_`. The rules apply on every OS, so case folders can be
/// copied between machines.
pub fn sanitize_name(name: &str) -> String {
    let replaced: String = name
        .chars()
        .map(|c| {
            if c.is_control() || RESERVED_CHARS.contains(&c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = replaced.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        return "_".to_owned();
    }
    match windows_device_prefix(trimmed) {
        Some(len) => format!("{}_{}", &trimmed[..len], &trimmed[len..]),
        None => trimmed.to_owned(),
    }
}

/// If Windows would treat `name` as a device, the byte length of the device part. Windows compares
/// the part before the first dot, ignoring case and trailing spaces.
fn windows_device_prefix(name: &str) -> Option<usize> {
    let stem = name.split('.').next().unwrap_or(name);
    let device = stem.trim_end_matches(' ');
    let reserved = WINDOWS_RESERVED_NAMES
        .iter()
        .any(|reserved| device.to_uppercase() == *reserved);
    reserved.then_some(device.len())
}

/// Checks the `case.json` fields a user can set.
fn validate_fields(fields: &CaseFields) -> Result<(), CaseError> {
    validate_name(&fields.name)
}

fn validate_name(name: &str) -> Result<(), CaseError> {
    if name.trim().is_empty() {
        return Err(CaseError::InvalidName("the name is required"));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(CaseError::InvalidName(
            "the name is longer than 120 characters",
        ));
    }
    Ok(())
}

// ---- create, open, update ----

/// A newly created case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatedCase {
    pub path: PathBuf,
    pub case: CaseFile,
}

/// Creates a case folder in `parent` (an existing directory) and writes its `case.json`. The folder
/// name is [`sanitize_name`] of `name`; if it is taken, ` (2)`, ` (3)`, … is appended. The folder is
/// created with a plain `create_dir`, so two creations can never share a folder.
/// `created_by_app_version` is this build's version, as in run records.
pub fn create(parent: &Path, fields: &CaseFields) -> Result<CreatedCase, CaseError> {
    validate_fields(fields)?;
    let case_id = new_case_id().map_err(|e| CaseError::io(parent, e))?;
    let path = create_unique_dir(parent, &sanitize_name(&fields.name))?;
    let now = Timestamp::now();
    let case = CaseFile {
        schema_version: CaseFile::SCHEMA_VERSION,
        case_id,
        name: fields.name.clone(),
        case_number: fields.case_number.clone(),
        examiner: fields.examiner.clone(),
        agency: fields.agency.clone(),
        description: fields.description.clone(),
        default_timezone: fields.default_timezone.clone(),
        created_at: now,
        updated_at: now,
        created_by_app_version: crate::run::record::record_app().version,
    };
    if let Err(e) = fsutil::write_json_atomic(&path.join(CASE_FILE), &case) {
        // The folder is ours and empty (the atomic write removes its temp file): don't leave a
        // folder without a case.json behind. Best effort; remove_dir never removes content.
        let _ = fs::remove_dir(&path);
        return Err(CaseError::io(&path, e));
    }
    Ok(CreatedCase { path, case })
}

fn create_unique_dir(parent: &Path, base: &str) -> Result<PathBuf, CaseError> {
    for n in 1..=MAX_COLLISION_SUFFIX {
        let name = if n == 1 {
            base.to_owned()
        } else {
            format!("{base} ({n})")
        };
        let path = parent.join(name);
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(CaseError::io(&path, e)),
        }
    }
    Err(CaseError::NoFreeName {
        parent: parent.display().to_string(),
        base: base.to_owned(),
    })
}

/// 32 lowercase hex characters from the OS RNG.
fn new_case_id() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(io::Error::other)?;
    Ok(to_hex(&bytes))
}

/// Reads and validates `<path>/case.json`: the schema version, the fields, a 32-hex `case_id` and a
/// valid name.
pub fn load(path: &Path) -> Result<CaseFile, CaseError> {
    let file = path.join(CASE_FILE);
    let bytes = match fs::read(&file) {
        Ok(bytes) => bytes,
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            return Err(CaseError::NotFound {
                path: path.display().to_string(),
            });
        }
        Err(e) => return Err(CaseError::io(&file, e)),
    };
    let case: CaseFile = parse_versioned(&bytes).map_err(|source| CaseError::Invalid {
        path: file.display().to_string(),
        source,
    })?;
    let invalid = |reason: String| CaseError::InvalidContent {
        path: file.display().to_string(),
        reason,
    };
    if !is_case_id(&case.case_id) {
        return Err(invalid(format!(
            "case_id {:?} is not 32 lowercase hex characters",
            case.case_id
        )));
    }
    validate_name(&case.name).map_err(|e| invalid(e.to_string()))?;
    Ok(case)
}

fn is_case_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Opens a case: validates its `case.json` and makes it known and most recent in `settings` (the
/// caller saves the settings). Run recovery is [`crate::run::record::recover_case`].
pub fn open(path: &Path, settings: &mut Settings) -> Result<CaseFile, CaseError> {
    let case = load(path)?;
    settings::touch_recent(settings, &path.to_string_lossy());
    Ok(case)
}

/// For commands that require a known case folder (ARCHITECTURE.md §9): `path` must be in the recent
/// list and its `case.json` must be valid.
pub fn ensure_known(settings: &Settings, path: &Path) -> Result<CaseFile, CaseError> {
    if !settings::is_recent(settings, &path.to_string_lossy()) {
        return Err(CaseError::NotKnown {
            path: path.display().to_string(),
        });
    }
    load(path)
}

/// Updates the editable fields of `case.json` (§6) and `updated_at`, atomically. Everything else is
/// kept, and the folder is not renamed.
pub fn update(path: &Path, fields: &CaseFields) -> Result<CaseFile, CaseError> {
    validate_fields(fields)?;
    let mut case = load(path)?;
    case.name.clone_from(&fields.name);
    case.case_number.clone_from(&fields.case_number);
    case.examiner.clone_from(&fields.examiner);
    case.agency.clone_from(&fields.agency);
    case.description.clone_from(&fields.description);
    case.default_timezone.clone_from(&fields.default_timezone);
    case.updated_at = Timestamp::now();
    let file = path.join(CASE_FILE);
    fsutil::write_json_atomic(&file, &case).map_err(|e| CaseError::io(&file, e))?;
    Ok(case)
}

// ---- runs and listings ----

/// A run found in a case folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredRun {
    pub dir: PathBuf,
    pub record: RunRecord,
}

/// Finds the runs of a case by scanning `runs/*/run.json`, newest first. Folders whose `run.json`
/// is missing, unreadable or invalid, has an invalid run id or names another run are skipped with
/// a logged warning. A case without `runs/` has no runs.
pub fn discover_runs(case_dir: &Path) -> Result<Vec<DiscoveredRun>, CaseError> {
    let runs_dir = case_dir.join(RUNS_DIR);
    let entries = match fs::read_dir(&runs_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(CaseError::io(&runs_dir, e)),
    };
    let mut runs = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| CaseError::io(&runs_dir, e))?;
        let dir = entry.path();
        // Only real folders: a symlink in runs/ is not a run of this case.
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => {}
            Ok(_) => continue,
            Err(e) => {
                log::warn!("skipping {}: {e}", dir.display());
                continue;
            }
        }
        match read_run(&dir) {
            Ok(record) => runs.push(DiscoveredRun { dir, record }),
            Err(reason) => log::warn!("skipping run folder {}: {reason}", dir.display()),
        }
    }
    runs.sort_by(|a, b| {
        (b.record.created_at, &b.record.run_id).cmp(&(a.record.created_at, &a.record.run_id))
    });
    Ok(runs)
}

/// Reads a run folder's `run.json`; the error is a reason for the log.
fn read_run(dir: &Path) -> Result<RunRecord, String> {
    let bytes = fs::read(dir.join(RUN_FILE)).map_err(|e| format!("{RUN_FILE}: {e}"))?;
    let record: RunRecord = parse_versioned(&bytes).map_err(|e| e.to_string())?;
    if !crate::run::record::is_run_id(&record.run_id) {
        return Err(format!(
            "{RUN_FILE} has an invalid run_id {:?}",
            record.run_id
        ));
    }
    let folder = dir.file_name().map(|name| name.to_string_lossy());
    if folder.as_deref() != Some(record.run_id.as_str()) {
        return Err(format!(
            "{RUN_FILE} names run {:?}, not the folder's",
            record.run_id
        ));
    }
    Ok(record)
}

/// The listing entry of a run. The report is available when `report/index.html` exists.
pub fn run_summary(run: &DiscoveredRun) -> RunSummary {
    let record = &run.record;
    RunSummary {
        run_id: record.run_id.clone(),
        run_dir: run.dir.to_string_lossy().into_owned(),
        label: record.label.clone(),
        status: record.status,
        tool: record.tool.id,
        tool_version: record.tool.version.clone(),
        input_path: record.input.path.clone(),
        input_type: record.input.input_type,
        created_at: record.created_at,
        started_at: record.started_at,
        ended_at: record.ended_at,
        duration_ms: record.duration_ms,
        report_available: run
            .dir
            .join(&record.output.report_dir)
            .join("index.html")
            .is_file(),
    }
}

/// The `cases_list` entry of a recent case. A missing folder has `exists: false`; an invalid
/// `case.json` gives `case: null` (logged). `last_run_at` is the newest run's `created_at`.
pub fn summary(path: &str) -> CaseSummary {
    let dir = Path::new(path);
    let exists = dir.is_dir();
    let case = if exists {
        load(dir)
            .inspect_err(|e| log::warn!("recent case {path}: {e}"))
            .ok()
    } else {
        None
    };
    let runs = if case.is_some() {
        discover_runs(dir)
            .inspect_err(|e| log::warn!("recent case {path}: {e}"))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    CaseSummary {
        path: path.to_owned(),
        exists,
        case,
        run_count: u32::try_from(runs.len()).unwrap_or(u32::MAX),
        last_run_at: runs.iter().map(|run| run.record.created_at).max(),
    }
}

/// `cases_list`: the recent cases, most recent first.
pub fn list(settings: &Settings) -> Vec<CaseSummary> {
    settings
        .recent_cases
        .iter()
        .map(|path| summary(path))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::contracts::{RunStatus, examples};

    fn fields(name: &str) -> CaseFields {
        CaseFields {
            name: name.to_owned(),
            ..examples::case_fields()
        }
    }

    #[test]
    fn sanitize_replaces_reserved_and_control_characters() {
        assert_eq!(sanitize_name("Operation Nightjar"), "Operation Nightjar");
        assert_eq!(
            sanitize_name(r#"a<b>c:d"e/f\g|h?i*j"#),
            "a_b_c_d_e_f_g_h_i_j"
        );
        assert_eq!(
            sanitize_name("tab\there\nnew\rline\u{7f}\u{0}"),
            "tab_here_new_line__"
        );
        assert_eq!(sanitize_name("2026/0142: Case #7"), "2026_0142_ Case #7");
        // Non-ASCII text is kept.
        assert_eq!(sanitize_name("Fall Müller – 東京"), "Fall Müller – 東京");
    }

    #[test]
    fn sanitize_trims_trailing_dots_and_spaces() {
        assert_eq!(sanitize_name("Case."), "Case");
        assert_eq!(sanitize_name("Case . . "), "Case");
        assert_eq!(sanitize_name("Case  "), "Case");
        assert_eq!(sanitize_name("  Case"), "  Case", "only trailing ones");
        assert_eq!(sanitize_name("a.b"), "a.b");
        // Nothing left, including the traversal names.
        assert_eq!(sanitize_name("..."), "_");
        assert_eq!(sanitize_name(".."), "_");
        assert_eq!(sanitize_name("."), "_");
        assert_eq!(sanitize_name(" "), "_");
        // A replaced character is not trimmed.
        assert_eq!(sanitize_name("Case?"), "Case_");
    }

    #[test]
    fn sanitize_suffixes_windows_reserved_names() {
        for (name, expected) in [
            ("CON", "CON_"),
            ("con", "con_"),
            ("Nul", "Nul_"),
            ("PRN", "PRN_"),
            ("aux", "aux_"),
            ("COM1", "COM1_"),
            ("com9", "com9_"),
            ("COM0", "COM0_"),
            ("LPT1", "LPT1_"),
            ("lpt5", "lpt5_"),
            ("COM¹", "COM¹_"),
            ("lpt³", "lpt³_"),
            // With extensions and trailing spaces before the dot.
            ("CON.txt", "CON_.txt"),
            ("nul.tar.gz", "nul_.tar.gz"),
            ("Aux .case", "Aux_ .case"),
            // Trailing dots are trimmed first.
            ("CON.", "CON_"),
            ("con . .", "con_"),
            // Not device names.
            ("CONSOLE", "CONSOLE"),
            ("COM10", "COM10"),
            ("LPT", "LPT"),
            ("NUL2", "NUL2"),
            ("my CON", "my CON"),
            ("CON (2)", "CON (2)"),
        ] {
            assert_eq!(sanitize_name(name), expected, "{name:?}");
        }
    }

    #[test]
    fn create_writes_case_json() {
        let dir = tempfile::tempdir().unwrap();
        let created = create(dir.path(), &examples::case_fields()).unwrap();
        assert_eq!(created.path, dir.path().join("Operation Nightjar"));
        let case = &created.case;
        assert_eq!(case.schema_version, 1);
        assert!(is_case_id(&case.case_id), "{}", case.case_id);
        assert_eq!(case.name, "Operation Nightjar");
        assert_eq!(case.case_number, "2026-0142");
        assert_eq!(case.examiner, "J. Doe");
        assert_eq!(case.default_timezone.as_deref(), Some("America/Chicago"));
        assert_eq!(case.created_at, case.updated_at);
        assert_eq!(case.created_by_app_version, "0.2.0");
        assert_eq!(load(&created.path).unwrap(), *case);
        // Two cases never share an id.
        let other = create(dir.path(), &examples::case_fields()).unwrap();
        assert_ne!(other.case.case_id, case.case_id);
    }

    #[test]
    fn create_names_folders_and_resolves_collisions() {
        let dir = tempfile::tempdir().unwrap();
        let name_of = |created: &CreatedCase| {
            created
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        let first = create(dir.path(), &fields("Case: A/B.")).unwrap();
        assert_eq!(name_of(&first), "Case_ A_B");
        assert_eq!(first.case.name, "Case: A/B.", "the name itself is kept");
        let second = create(dir.path(), &fields("Case: A/B")).unwrap();
        assert_eq!(name_of(&second), "Case_ A_B (2)");
        let third = create(dir.path(), &fields("Case? A|B")).unwrap();
        assert_eq!(name_of(&third), "Case_ A_B (3)");
        // An existing plain file also blocks the name (same spelling, so this holds on
        // case-sensitive and case-insensitive filesystems alike).
        fs::write(dir.path().join("con_"), "").unwrap();
        let reserved = create(dir.path(), &fields("con")).unwrap();
        assert_eq!(name_of(&reserved), "con_ (2)");
        // Whether another spelling collides is the filesystem's call: create_dir decides.
        let case_insensitive = dir.path().join("CON_").exists();
        let other = create(dir.path(), &fields("CON")).unwrap();
        let expected = if case_insensitive { "CON_ (3)" } else { "CON_" };
        assert_eq!(name_of(&other), expected);
    }

    #[test]
    fn create_rejects_invalid_names_and_missing_parents() {
        let dir = tempfile::tempdir().unwrap();
        let too_long = "x".repeat(121);
        for name in ["", "   ", too_long.as_str()] {
            let err = create(dir.path(), &fields(name)).unwrap_err();
            assert_eq!(err.code(), ErrorCode::InvalidCase, "{name:?}");
        }
        assert!(create(dir.path(), &fields(&"é".repeat(120))).is_ok());
        let err = create(&dir.path().join("missing"), &fields("A")).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Io);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn open_marks_known_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let created = create(dir.path(), &examples::case_fields()).unwrap();
        let path = created.path.to_string_lossy().into_owned();
        let mut settings = crate::settings::defaults(dir.path());
        settings.recent_cases = vec!["/elsewhere".to_owned()];

        let err = ensure_known(&settings, &created.path).unwrap_err();
        assert_eq!(err.code(), ErrorCode::CaseNotFound);

        assert_eq!(open(&created.path, &mut settings).unwrap(), created.case);
        assert_eq!(settings.recent_cases, [path.as_str(), "/elsewhere"]);
        assert_eq!(
            ensure_known(&settings, &created.path).unwrap(),
            created.case
        );

        // Not a case folder.
        let err = open(dir.path(), &mut settings).unwrap_err();
        assert_eq!(err.code(), ErrorCode::CaseNotFound);
        let err = open(&dir.path().join("missing"), &mut settings).unwrap_err();
        assert_eq!(err.code(), ErrorCode::CaseNotFound);
        assert_eq!(
            settings.recent_cases.len(),
            2,
            "failed opens are not remembered"
        );
    }

    fn write_case_json(dir: &Path, value: &serde_json::Value) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(CASE_FILE), value.to_string()).unwrap();
    }

    #[test]
    fn load_rejects_unknown_schema_versions() {
        let dir = tempfile::tempdir().unwrap();
        let mut value = serde_json::to_value(examples::case_file()).unwrap();
        value["schema_version"] = serde_json::json!(2);
        write_case_json(dir.path(), &value);
        let err = load(dir.path()).unwrap_err();
        assert!(
            matches!(
                err,
                CaseError::Invalid {
                    source: ContractError::UnsupportedSchemaVersion { found: 2, .. },
                    ..
                }
            ),
            "{err:?}"
        );
        assert_eq!(err.code(), ErrorCode::InvalidCase);
        assert!(err.to_string().contains("schema_version 2"), "{err}");

        value.as_object_mut().unwrap().remove("schema_version");
        write_case_json(dir.path(), &value);
        assert!(matches!(
            load(dir.path()).unwrap_err(),
            CaseError::Invalid {
                source: ContractError::MissingSchemaVersion { .. },
                ..
            }
        ));
    }

    #[test]
    fn load_validates_content() {
        let dir = tempfile::tempdir().unwrap();
        let good = serde_json::to_value(examples::case_file()).unwrap();
        write_case_json(dir.path(), &good);
        assert_eq!(load(dir.path()).unwrap(), examples::case_file());

        for (field, bad) in [
            (
                "case_id",
                serde_json::json!("5B0C2F4E9A7D4B1F8C3E6A2D1F0B9E7C"),
            ),
            ("case_id", serde_json::json!("5b0c")),
            ("name", serde_json::json!("")),
            ("name", serde_json::json!(null)),
            ("created_at", serde_json::json!("2026-09-24 18:10:00")),
        ] {
            let mut value = good.clone();
            value[field] = bad.clone();
            write_case_json(dir.path(), &value);
            let err = load(dir.path()).unwrap_err();
            assert_eq!(err.code(), ErrorCode::InvalidCase, "{field} = {bad}: {err}");
        }
        fs::write(dir.path().join(CASE_FILE), "not json").unwrap();
        assert_eq!(load(dir.path()).unwrap_err().code(), ErrorCode::InvalidCase);
    }

    #[test]
    fn update_changes_editable_fields_only() {
        let dir = tempfile::tempdir().unwrap();
        let created = create(dir.path(), &examples::case_fields()).unwrap();
        let new_fields = CaseFields {
            name: "Renamed".to_owned(),
            case_number: "2026-0999".to_owned(),
            examiner: "A. Smith".to_owned(),
            agency: "State Lab".to_owned(),
            description: "Second phone".to_owned(),
            default_timezone: None,
        };
        let updated = update(&created.path, &new_fields).unwrap();
        assert_eq!(updated.name, "Renamed");
        assert_eq!(updated.case_number, "2026-0999");
        assert_eq!(updated.examiner, "A. Smith");
        assert_eq!(updated.agency, "State Lab");
        assert_eq!(updated.description, "Second phone");
        assert_eq!(updated.default_timezone, None);
        assert_eq!(updated.case_id, created.case.case_id);
        assert_eq!(updated.created_at, created.case.created_at);
        assert_eq!(updated.created_by_app_version, "0.2.0");
        assert!(updated.updated_at >= created.case.updated_at);
        assert_eq!(load(&created.path).unwrap(), updated);
        // The folder keeps its name.
        assert!(created.path.is_dir());
        assert!(!dir.path().join("Renamed").exists());

        let err = update(&created.path, &fields("")).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InvalidCase);
        assert_eq!(load(&created.path).unwrap(), updated, "unchanged");
    }

    #[test]
    fn update_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let created = create(dir.path(), &examples::case_fields()).unwrap();
        update(&created.path, &fields("Again")).unwrap();
        let names: Vec<_> = fs::read_dir(&created.path)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, [CASE_FILE], "no temp files left behind");
    }

    #[test]
    fn a_crash_between_write_and_rename_never_truncates_case_json() {
        let dir = tempfile::tempdir().unwrap();
        let created = create(dir.path(), &examples::case_fields()).unwrap();
        let file = created.path.join(CASE_FILE);
        let before = fs::read(&file).unwrap();
        let mut changed = created.case.clone();
        changed.name = "Changed".to_owned();
        let bytes = serde_json::to_vec_pretty(&changed).unwrap();

        // The same write path as `update`, with the process "dying" after the new content is
        // complete and synced in the temp file but before it replaces case.json.
        let crash = fsutil::write_atomic(&file, &bytes, |tmp| {
            assert_eq!(fs::read(tmp).unwrap(), bytes);
            assert_eq!(fs::read(&file).unwrap(), before, "target untouched so far");
            Err(io::Error::other("simulated crash"))
        });
        assert!(crash.is_err());
        assert_eq!(fs::read(&file).unwrap(), before, "never truncated");
        assert_eq!(load(&created.path).unwrap(), created.case);

        // A stray temp file from a real crash does not get in the way of the next update.
        fs::write(
            file.with_file_name("case.json.tmp-0123456789abcdef"),
            b"{\"sch",
        )
        .unwrap();
        assert_eq!(
            update(&created.path, &fields("After")).unwrap().name,
            "After"
        );
        assert_eq!(load(&created.path).unwrap().name, "After");
    }

    fn write_run(case_dir: &Path, run_id: &str, created_at: &str) -> PathBuf {
        let dir = case_dir.join(RUNS_DIR).join(run_id);
        fs::create_dir_all(&dir).unwrap();
        let mut record = examples::run_record();
        record.run_id = run_id.to_owned();
        record.created_at = Timestamp::parse(created_at).unwrap();
        fsutil::write_json_atomic(&dir.join(RUN_FILE), &record).unwrap();
        dir
    }

    #[test]
    fn discovers_runs_newest_first_and_skips_bad_ones() {
        let dir = tempfile::tempdir().unwrap();
        let case_dir = dir.path();
        assert_eq!(discover_runs(case_dir).unwrap(), vec![], "no runs/ folder");

        write_run(
            case_dir,
            "20260924-183005Z-ileapp-3f9a1c",
            "2026-09-24T18:30:05Z",
        );
        let newest = write_run(
            case_dir,
            "20260925-090000Z-aleapp-000001",
            "2026-09-25T09:00:00Z",
        );
        write_run(
            case_dir,
            "20260923-120000Z-ileapp-abcdef",
            "2026-09-23T12:00:00Z",
        );
        // Skipped: no run.json, invalid JSON, unknown schema, another run's record, a plain file.
        fs::create_dir_all(case_dir.join("runs/20260926-000000Z-ileapp-000000")).unwrap();
        let bad = case_dir.join("runs/20260926-000001Z-ileapp-000000");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join(RUN_FILE), "{").unwrap();
        let newer = case_dir.join("runs/20260926-000002Z-ileapp-000000");
        fs::create_dir_all(&newer).unwrap();
        let mut value = serde_json::to_value(examples::run_record()).unwrap();
        value["schema_version"] = serde_json::json!(9);
        fs::write(newer.join(RUN_FILE), value.to_string()).unwrap();
        let copied = case_dir.join("runs/copy of a run");
        fs::create_dir_all(&copied).unwrap();
        fsutil::write_json_atomic(&copied.join(RUN_FILE), &examples::run_record()).unwrap();
        fs::write(case_dir.join("runs/notes.txt"), "x").unwrap();

        let runs = discover_runs(case_dir).unwrap();
        let ids: Vec<&str> = runs.iter().map(|r| r.record.run_id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "20260925-090000Z-aleapp-000001",
                "20260924-183005Z-ileapp-3f9a1c",
                "20260923-120000Z-ileapp-abcdef"
            ]
        );
        assert_eq!(runs[0].dir, newest);
    }

    #[test]
    fn run_summary_reflects_the_record_and_report() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = write_run(
            dir.path(),
            "20260924-183005Z-ileapp-3f9a1c",
            "2026-09-24T18:30:05Z",
        );
        let runs = discover_runs(dir.path()).unwrap();
        let summary = run_summary(&runs[0]);
        let mut expected = examples::run_summary();
        expected.run_dir = run_dir.to_string_lossy().into_owned();
        expected.report_available = false;
        assert_eq!(summary, expected);

        fs::create_dir_all(run_dir.join("report")).unwrap();
        fs::write(run_dir.join("report/index.html"), "<html>").unwrap();
        assert!(run_summary(&runs[0]).report_available);
    }

    #[test]
    fn list_summarizes_recent_cases() {
        let dir = tempfile::tempdir().unwrap();
        let created = create(dir.path(), &examples::case_fields()).unwrap();
        write_run(
            &created.path,
            "20260924-183005Z-ileapp-3f9a1c",
            "2026-09-24T18:30:05Z",
        );
        write_run(
            &created.path,
            "20260925-090000Z-aleapp-000001",
            "2026-09-25T09:00:00Z",
        );
        let broken = dir.path().join("broken");
        write_case_json(&broken, &serde_json::json!({"schema_version": 1}));
        let empty = create(dir.path(), &fields("Empty")).unwrap();

        let mut settings = crate::settings::defaults(dir.path());
        for path in [
            broken.clone(),
            dir.path().join("gone"),
            empty.path.clone(),
            created.path.clone(),
        ] {
            crate::settings::touch_recent(&mut settings, &path.to_string_lossy());
        }
        let list = list(&settings);
        assert_eq!(list.len(), 4);

        assert_eq!(list[0].path, created.path.to_string_lossy());
        assert!(list[0].exists);
        assert_eq!(list[0].case.as_ref(), Some(&created.case));
        assert_eq!(list[0].run_count, 2);
        assert_eq!(
            list[0].last_run_at,
            Some(Timestamp::parse("2026-09-25T09:00:00Z").unwrap())
        );

        assert_eq!(list[1].case.as_ref(), Some(&empty.case));
        assert_eq!((list[1].run_count, list[1].last_run_at), (0, None));

        assert!(!list[2].exists);
        assert_eq!(list[2].case, None);

        assert!(list[3].exists);
        assert_eq!(list[3].case, None, "invalid case.json");
        assert_eq!(list[3].run_count, 0);
    }

    #[test]
    fn discovered_status_is_as_recorded() {
        let dir = tempfile::tempdir().unwrap();
        write_run(
            dir.path(),
            "20260924-183005Z-ileapp-3f9a1c",
            "2026-09-24T18:30:05Z",
        );
        let runs = discover_runs(dir.path()).unwrap();
        assert_eq!(runs[0].record.status, RunStatus::CompletedWithErrors);
    }
}
