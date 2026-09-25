//! Loading and saving `settings.json` (CONTRACTS.md §6), with first-run defaults, the recent-cases
//! list and `settings_update` semantics (§10).

use std::fs;
use std::io;
use std::path::Path;

use crate::contracts::{
    AppError, ContractError, ErrorCode, Settings, SettingsDefaults, SettingsUpdateRequest,
    ToolsDirUpdate, VersionedFile, parse_versioned,
};
use crate::fsutil;

/// `recent_cases` holds at most this many entries.
pub const MAX_RECENT_CASES: usize = 50;

/// The timezone default on first run (ARCHITECTURE.md D19: case → settings → UTC).
const DEFAULT_TIMEZONE: &str = "UTC";

/// Errors from reading or writing `settings.json`.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("could not read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("could not write {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Invalid(#[from] ContractError),
}

impl From<SettingsError> for AppError {
    fn from(err: SettingsError) -> Self {
        let code = match &err {
            SettingsError::Read { source, .. } | SettingsError::Write { source, .. } => {
                fsutil::io_error_code(source)
            }
            SettingsError::Invalid(_) => ErrorCode::Io,
        };
        AppError {
            code,
            message: "The settings file could not be used".to_owned(),
            detail: Some(err.to_string()),
        }
    }
}

/// The settings of a first run. `cases_root` is passed in by the shell (`<Documents>/suiteDFIR
/// Cases`); examiner and agency are empty (not set).
pub fn defaults(cases_root: &Path) -> Settings {
    Settings {
        schema_version: Settings::SCHEMA_VERSION,
        cases_root: cases_root.to_string_lossy().into_owned(),
        recent_cases: Vec::new(),
        defaults: SettingsDefaults {
            examiner: String::new(),
            agency: String::new(),
            timezone: DEFAULT_TIMEZONE.to_owned(),
        },
        tools_dir: None,
    }
}

/// Reads `settings.json`. A missing file is a first run and yields [`defaults`] (nothing is written
/// until [`save`]). An unknown `schema_version` or an invalid file is an error.
pub fn load(path: &Path, default_cases_root: &Path) -> Result<Settings, SettingsError> {
    match fs::read(path) {
        Ok(bytes) => Ok(parse_versioned(&bytes)?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(defaults(default_cases_root)),
        Err(source) => Err(SettingsError::Read {
            path: path.display().to_string(),
            source,
        }),
    }
}

/// Writes `settings.json` atomically, creating its directory if needed.
pub fn save(path: &Path, settings: &Settings) -> Result<(), SettingsError> {
    let write_error = |source| SettingsError::Write {
        path: path.display().to_string(),
        source,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(write_error)?;
    }
    fsutil::write_json_atomic(path, settings).map_err(write_error)
}

/// Makes `case_path` the most recent case: removes any other spelling of the same path, puts it
/// first and keeps at most [`MAX_RECENT_CASES`] entries.
pub fn touch_recent(settings: &mut Settings, case_path: &str) {
    forget_recent(settings, case_path);
    settings.recent_cases.insert(0, case_path.to_owned());
    settings.recent_cases.truncate(MAX_RECENT_CASES);
}

/// Removes `case_path` from the recent list (`case_forget`); the folder is not touched.
pub fn forget_recent(settings: &mut Settings, case_path: &str) {
    settings
        .recent_cases
        .retain(|known| !same_path(known, case_path));
}

/// Whether `case_path` is in the recent list.
pub fn is_recent(settings: &Settings, case_path: &str) -> bool {
    settings
        .recent_cases
        .iter()
        .any(|known| same_path(known, case_path))
}

/// Two spellings of the same path: equal components, so `a/b/`, `a//b` and `a/./b` match `a/b`.
fn same_path(a: &str, b: &str) -> bool {
    Path::new(a) == Path::new(b)
}

/// Applies a `settings_update` request (§10): an omitted field is unchanged, `defaults` replaces all
/// three defaults, and `tools_dir: null` resets the override. Path checks (ARCHITECTURE.md §9) are
/// the caller's.
pub fn apply_update(settings: &mut Settings, update: &SettingsUpdateRequest) {
    if let Some(cases_root) = &update.cases_root {
        settings.cases_root.clone_from(cases_root);
    }
    if let Some(defaults) = &update.defaults {
        settings.defaults.clone_from(defaults);
    }
    match &update.tools_dir {
        ToolsDirUpdate::Unchanged => {}
        ToolsDirUpdate::Reset => settings.tools_dir = None,
        ToolsDirUpdate::Set(dir) => settings.tools_dir = Some(dir.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::contracts::examples;

    #[test]
    fn first_run_defaults_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config").join("settings.json");
        let root = dir.path().join("Documents").join("suiteDFIR Cases");

        let first = load(&path, &root).unwrap();
        assert_eq!(first.cases_root, root.to_string_lossy());
        assert_eq!(first.recent_cases, Vec::<String>::new());
        assert_eq!(first.defaults.examiner, "");
        assert_eq!(first.defaults.agency, "");
        assert_eq!(first.defaults.timezone, "UTC");
        assert_eq!(first.tools_dir, None);
        assert!(!path.exists(), "loading never writes");

        let mut settings = examples::settings();
        settings.tools_dir = Some("/approved/tools".to_owned());
        save(&path, &settings).unwrap();
        assert_eq!(load(&path, &root).unwrap(), settings);
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\"tools_dir\": \"/approved/tools\""),
            "{text}"
        );
    }

    #[test]
    fn rejects_unknown_schema_versions_and_invalid_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut value = serde_json::to_value(examples::settings()).unwrap();
        value["schema_version"] = serde_json::json!(2);
        fs::write(&path, value.to_string()).unwrap();
        let err = load(&path, dir.path()).unwrap_err();
        assert!(
            matches!(
                err,
                SettingsError::Invalid(ContractError::UnsupportedSchemaVersion { found: 2, .. })
            ),
            "{err:?}"
        );
        assert!(err.to_string().contains("schema_version 2"), "{err}");

        fs::write(&path, "{\"schema_version\": 1, \"cases_root\": 5}").unwrap();
        let err = load(&path, dir.path()).unwrap_err();
        assert!(matches!(err, SettingsError::Invalid(_)), "{err:?}");
        let app: AppError = err.into();
        assert_eq!(app.code, ErrorCode::Io);
    }

    #[test]
    fn unreadable_settings_map_to_permission_denied() {
        let dir = tempfile::tempdir().unwrap();
        // A directory where the file should be cannot be read as a file.
        let path = dir.path().join("settings.json");
        fs::create_dir(&path).unwrap();
        let err = load(&path, dir.path()).unwrap_err();
        assert!(matches!(err, SettingsError::Read { .. }), "{err:?}");
        let err = SettingsError::Read {
            path: "settings.json".to_owned(),
            source: io::Error::from(io::ErrorKind::PermissionDenied),
        };
        assert_eq!(AppError::from(err).code, ErrorCode::PermissionDenied);
    }

    fn recent(settings: &Settings) -> Vec<&str> {
        settings.recent_cases.iter().map(String::as_str).collect()
    }

    #[test]
    fn recent_list_dedupes_most_recent_first() {
        let mut settings = defaults(Path::new("/cases"));
        touch_recent(&mut settings, "/cases/A");
        touch_recent(&mut settings, "/cases/B");
        touch_recent(&mut settings, "/cases/C");
        assert_eq!(recent(&settings), ["/cases/C", "/cases/B", "/cases/A"]);
        // Re-opening moves to the front without duplicating, also for another spelling.
        touch_recent(&mut settings, "/cases/A");
        assert_eq!(recent(&settings), ["/cases/A", "/cases/C", "/cases/B"]);
        touch_recent(&mut settings, "/cases/B/");
        assert_eq!(recent(&settings), ["/cases/B/", "/cases/A", "/cases/C"]);
        touch_recent(&mut settings, "/cases//./C");
        assert_eq!(recent(&settings), ["/cases//./C", "/cases/B/", "/cases/A"]);
        assert!(is_recent(&settings, "/cases/C"));
        assert!(!is_recent(&settings, "/cases/D"));
        // Different folders stay distinct, even with a shared prefix.
        touch_recent(&mut settings, "/cases/A2");
        assert_eq!(settings.recent_cases.len(), 4);
    }

    #[test]
    fn recent_list_keeps_at_most_50() {
        let mut settings = defaults(Path::new("/cases"));
        for i in 0..60 {
            touch_recent(&mut settings, &format!("/cases/{i}"));
        }
        assert_eq!(settings.recent_cases.len(), MAX_RECENT_CASES);
        assert_eq!(settings.recent_cases[0], "/cases/59");
        assert_eq!(settings.recent_cases[49], "/cases/10");
        // Touching an existing entry at the limit does not drop another one.
        touch_recent(&mut settings, "/cases/30");
        assert_eq!(settings.recent_cases.len(), MAX_RECENT_CASES);
        assert_eq!(settings.recent_cases[0], "/cases/30");
        assert_eq!(settings.recent_cases[49], "/cases/10");
    }

    #[test]
    fn forget_removes_only_that_case() {
        let mut settings = defaults(Path::new("/cases"));
        for case in ["/cases/A", "/cases/B", "/cases/C"] {
            touch_recent(&mut settings, case);
        }
        forget_recent(&mut settings, "/cases/B/");
        assert_eq!(recent(&settings), ["/cases/C", "/cases/A"]);
        forget_recent(&mut settings, "/cases/unknown");
        assert_eq!(recent(&settings), ["/cases/C", "/cases/A"]);
    }

    #[test]
    fn update_is_partial_and_tools_dir_tri_state() {
        let mut settings = examples::settings();
        settings.tools_dir = Some("/old/tools".to_owned());
        let before = settings.clone();

        apply_update(&mut settings, &SettingsUpdateRequest::default());
        assert_eq!(settings, before, "an empty update changes nothing");

        let update: SettingsUpdateRequest = serde_json::from_value(serde_json::json!({
            "defaults": {"examiner": "A. Smith", "agency": "", "timezone": "Europe/Berlin"}
        }))
        .unwrap();
        apply_update(&mut settings, &update);
        assert_eq!(settings.defaults.examiner, "A. Smith");
        assert_eq!(settings.defaults.agency, "");
        assert_eq!(settings.defaults.timezone, "Europe/Berlin");
        assert_eq!(settings.cases_root, before.cases_root);
        assert_eq!(settings.tools_dir.as_deref(), Some("/old/tools"));

        let update: SettingsUpdateRequest = serde_json::from_value(serde_json::json!({
            "cases_root": "/new/root", "tools_dir": "/new/tools"
        }))
        .unwrap();
        apply_update(&mut settings, &update);
        assert_eq!(settings.cases_root, "/new/root");
        assert_eq!(settings.tools_dir.as_deref(), Some("/new/tools"));
        assert_eq!(settings.defaults.examiner, "A. Smith");

        let update: SettingsUpdateRequest =
            serde_json::from_value(serde_json::json!({"tools_dir": null})).unwrap();
        apply_update(&mut settings, &update);
        assert_eq!(settings.tools_dir, None);
        assert_eq!(settings.recent_cases, before.recent_cases);
    }
}
