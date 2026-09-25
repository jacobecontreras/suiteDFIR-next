//! The path policy of the commands (ARCHITECTURE.md §9), one function per rule:
//!
//! | Command argument | Rule | Here |
//! |---|---|---|
//! | `case_create.parent_dir`, `settings_update.cases_root` | existing writable dir, not inside the tools dir or app dirs | [`cases_parent`] |
//! | `settings_update.tools_dir` | existing writable dir, not inside any known case folder | [`tools_dir`] |
//! | `case_open.path` | any dir with a valid `case.json` (it becomes known) | `case::open` |
//! | `case_update`, `case_forget`, `run_get`, `open_report`, `open_text_file` | a known case folder; a valid, existing `run_id` | [`known_case`], [`run_dir`] |
//! | `input_inspect.path`, `run_start.input_path`, `run_start.keychain_path` | any readable path, subject to the overlap rule | `inspect`, `runner::start` |
//! | `tool_import.archive_path`, `profile_import.path` | any readable regular file (read-only) | [`readable_file`] |
//! | `profile_export.dest_path` | not inside a known case folder's `runs/` | [`export_dest`] |
//! | `reveal_path.path` | inside a known case folder or the app dirs only | [`revealable`] |
//! | `acq_*`, `open_acq_file` | a known case folder; a valid, existing `acq_id` | [`known_case`], [`acq_dir`] |
//! | `devices_list`, `device_pair`, any `udid` | no path; the UDID format | [`udid`] |
//!
//! A known case folder is a path in `settings.recent_cases` whose `case.json` parses.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use suitedfir_core::acquire;
use suitedfir_core::case::{self, RUN_FILE, RUNS_DIR};
use suitedfir_core::contracts::{AppError, CaseFile, ErrorCode, Settings};
use suitedfir_core::fsutil;
use suitedfir_core::idevice;
use suitedfir_core::paths::AppPaths;
use suitedfir_core::run::record;

fn error(code: ErrorCode, message: &str, path: &Path) -> AppError {
    AppError {
        code,
        message: message.to_owned(),
        detail: Some(path.display().to_string()),
    }
}

fn io_error(message: &str, path: &Path, e: &io::Error) -> AppError {
    AppError {
        code: fsutil::io_error_code(e),
        message: message.to_owned(),
        detail: Some(format!("{}: {e}", path.display())),
    }
}

/// `a` is `b` or inside it (links resolved); an unresolvable path counts as inside (refuse).
fn within(a: &Path, b: &Path) -> bool {
    fsutil::path_within(a, b).unwrap_or(true)
}

/// An existing folder this user can create files in: a probe file is created and removed.
pub fn existing_writable_dir(dir: &Path) -> Result<PathBuf, AppError> {
    let dir =
        std::path::absolute(dir).map_err(|e| io_error("The folder path is unusable", dir, &e))?;
    match fs::metadata(&dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return Err(error(
                ErrorCode::PathNotAllowed,
                "This is not a folder",
                &dir,
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(error(
                ErrorCode::PathNotAllowed,
                "The folder does not exist",
                &dir,
            ));
        }
        Err(e) => return Err(io_error("The folder cannot be read", &dir, &e)),
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let probe = dir.join(format!(
        ".suitedfir-write-probe-{}-{nanos}",
        std::process::id()
    ));
    let created = OpenOptions::new().write(true).create_new(true).open(&probe);
    match created {
        Ok(file) => {
            drop(file);
            let _ = fs::remove_file(&probe);
            Ok(dir)
        }
        Err(e) => Err(io_error("The folder is not writable", &dir, &e)),
    }
}

/// `case_create.parent_dir` and `settings_update.cases_root`: an existing writable folder, not
/// inside the tools dir or the app dirs.
pub fn cases_parent(
    dir: &Path,
    paths: &AppPaths,
    settings: &Settings,
) -> Result<PathBuf, AppError> {
    let dir = existing_writable_dir(dir)?;
    if paths.app_dirs(settings).iter().any(|app| within(&dir, app)) {
        return Err(error(
            ErrorCode::PathNotAllowed,
            "Cases cannot be kept inside the app's or the parsers' folders",
            &dir,
        ));
    }
    Ok(dir)
}

/// `settings_update.tools_dir`: an existing writable folder, not inside any known case folder.
pub fn tools_dir(dir: &Path, settings: &Settings) -> Result<PathBuf, AppError> {
    let dir = existing_writable_dir(dir)?;
    for case in &settings.recent_cases {
        let case = Path::new(case);
        if case::load(case).is_ok() && within(&dir, case) {
            return Err(error(
                ErrorCode::PathNotAllowed,
                "The parsers' folder cannot be inside a case folder",
                &dir,
            ));
        }
    }
    Ok(dir)
}

/// A known case folder (in the recent list, with a valid `case.json`) and its case.
pub fn known_case(settings: &Settings, path: &str) -> Result<(PathBuf, CaseFile), AppError> {
    let dir = PathBuf::from(path);
    let case = case::ensure_known(settings, &dir)?;
    Ok((dir, case))
}

/// `<case>/runs/<run_id>` of an existing run: the id must have the run id format and the folder
/// must hold a `run.json` (`run_not_found` otherwise).
pub fn run_dir(case_dir: &Path, run_id: &str) -> Result<PathBuf, AppError> {
    let not_found = || AppError {
        code: ErrorCode::RunNotFound,
        message: "The run was not found in this case".to_owned(),
        detail: Some(run_id.to_owned()),
    };
    if !record::is_run_id(run_id) {
        return Err(not_found());
    }
    let dir = case_dir.join(RUNS_DIR).join(run_id);
    if !dir.join(RUN_FILE).is_file() {
        return Err(not_found());
    }
    Ok(dir)
}

/// `<case>/acquisitions/<acq_id>` of an existing acquisition (`acq_not_found` otherwise).
pub fn acq_dir(case_dir: &Path, acq_id: &str) -> Result<PathBuf, AppError> {
    acquire::load(case_dir, acq_id)?;
    Ok(case_dir.join(acquire::ACQUISITIONS_DIR).join(acq_id))
}

/// `tool_import.archive_path` and `profile_import.path`: a readable regular file.
pub fn readable_file(path: &Path) -> Result<PathBuf, AppError> {
    match fs::metadata(path) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => {
            return Err(error(
                ErrorCode::InvalidInput,
                "This is not a regular file",
                path,
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(error(
                ErrorCode::InvalidInput,
                "The file does not exist",
                path,
            ));
        }
        Err(e) => return Err(io_error("The file cannot be read", path, &e)),
    }
    fs::File::open(path).map_err(|e| io_error("The file cannot be read", path, &e))?;
    Ok(path.to_path_buf())
}

/// `profile_export.dest_path` (from the save dialog): not inside a known case folder's `runs/`.
pub fn export_dest(dest: &Path, settings: &Settings) -> Result<PathBuf, AppError> {
    for case in &settings.recent_cases {
        let runs = Path::new(case).join(RUNS_DIR);
        if within(dest, &runs) {
            return Err(error(
                ErrorCode::PathNotAllowed,
                "Profiles cannot be exported into a case's runs folder",
                dest,
            ));
        }
    }
    Ok(dest.to_path_buf())
}

/// `reveal_path.path`: inside a known case folder or the app dirs.
pub fn revealable(path: &Path, paths: &AppPaths, settings: &Settings) -> Result<PathBuf, AppError> {
    let in_case = settings.recent_cases.iter().any(|case| {
        let case = Path::new(case);
        fsutil::path_within(path, case).unwrap_or(false) && case::load(case).is_ok()
    });
    let in_app = paths
        .app_dirs(settings)
        .iter()
        .any(|dir| fsutil::path_within(path, dir).unwrap_or(false));
    if in_case || in_app {
        Ok(path.to_path_buf())
    } else {
        Err(error(
            ErrorCode::PathNotAllowed,
            "Only paths inside a known case folder or the app's folders can be revealed",
            path,
        ))
    }
}

/// A device UDID (`device_not_found` for anything else).
pub fn udid(udid: &str) -> Result<(), AppError> {
    if idevice::is_udid(udid) {
        Ok(())
    } else {
        Err(AppError {
            code: ErrorCode::DeviceNotFound,
            message: "That is not a valid device identifier.".to_owned(),
            detail: Some(udid.to_owned()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use suitedfir_core::contracts::examples;
    use suitedfir_core::settings;

    /// App dirs and a known case with one run and one acquisition, in a temp dir.
    struct Lab {
        root: tempfile::TempDir,
        paths: AppPaths,
        settings: Settings,
        case: PathBuf,
        run_id: String,
    }

    const ACQ_ID: &str = "20260924-171200Z-ios-9c01de";

    impl Lab {
        fn new() -> Self {
            let root = tempfile::Builder::new().prefix("sdp").tempdir().unwrap();
            let paths = AppPaths {
                app_data: root.path().join("data"),
                app_config: root.path().join("config"),
                app_cache: root.path().join("cache"),
                app_log: root.path().join("log"),
            };
            for dir in [
                &paths.app_data,
                &paths.app_config,
                &paths.app_cache,
                &paths.app_log,
            ] {
                fs::create_dir_all(dir).unwrap();
            }
            let cases = root.path().join("cases");
            fs::create_dir_all(&cases).unwrap();
            let case = case::create(&cases, &examples::case_fields()).unwrap().path;
            let mut settings = settings::defaults(&cases);
            settings::touch_recent(&mut settings, &case.to_string_lossy());
            let run_id = "20260924-183005Z-ileapp-3f9a1c".to_owned();
            let run = case.join("runs").join(&run_id);
            fs::create_dir_all(&run).unwrap();
            let mut record = examples::run_record();
            record.run_id.clone_from(&run_id);
            fs::write(run.join("run.json"), serde_json::to_vec(&record).unwrap()).unwrap();
            let acq = case.join("acquisitions").join(ACQ_ID);
            fs::create_dir_all(&acq).unwrap();
            fs::write(
                acq.join("acquisition.json"),
                serde_json::to_vec(&examples::acquisition_record()).unwrap(),
            )
            .unwrap();
            Self {
                root,
                paths,
                settings,
                case,
                run_id,
            }
        }

        fn case_str(&self) -> String {
            self.case.to_string_lossy().into_owned()
        }
    }

    fn code(result: Result<impl std::fmt::Debug, AppError>) -> ErrorCode {
        result.unwrap_err().code
    }

    #[test]
    fn row_cases_parent() {
        let lab = Lab::new();
        let parent = lab.root.path().join("parent");
        fs::create_dir(&parent).unwrap();
        assert!(cases_parent(&parent, &lab.paths, &lab.settings).is_ok());
        // No probe file is left behind.
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);
        // Missing, a file, inside the app dirs or the tools dir.
        let missing = lab.root.path().join("missing");
        assert_eq!(
            code(cases_parent(&missing, &lab.paths, &lab.settings)),
            ErrorCode::PathNotAllowed
        );
        let file = lab.root.path().join("file");
        fs::write(&file, "x").unwrap();
        assert_eq!(
            code(cases_parent(&file, &lab.paths, &lab.settings)),
            ErrorCode::PathNotAllowed
        );
        for inside in [
            lab.paths.app_data.clone(),
            lab.paths.app_log.join("x"),
            lab.paths.default_tools_dir(),
        ] {
            fs::create_dir_all(&inside).unwrap();
            assert_eq!(
                code(cases_parent(&inside, &lab.paths, &lab.settings)),
                ErrorCode::PathNotAllowed,
                "{}",
                inside.display()
            );
        }
        let mut overridden = lab.settings.clone();
        let tools = lab.root.path().join("tools");
        fs::create_dir(&tools).unwrap();
        overridden.tools_dir = Some(tools.to_string_lossy().into_owned());
        assert_eq!(
            code(cases_parent(&tools, &lab.paths, &overridden)),
            ErrorCode::PathNotAllowed
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let locked = lab.root.path().join("locked");
            fs::create_dir(&locked).unwrap();
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
            assert_eq!(
                code(cases_parent(&locked, &lab.paths, &lab.settings)),
                ErrorCode::PermissionDenied
            );
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn row_tools_dir() {
        let lab = Lab::new();
        let tools = lab.root.path().join("tools");
        fs::create_dir(&tools).unwrap();
        assert!(tools_dir(&tools, &lab.settings).is_ok());
        let inside_case = lab.case.join("tools");
        fs::create_dir(&inside_case).unwrap();
        assert_eq!(
            code(tools_dir(&inside_case, &lab.settings)),
            ErrorCode::PathNotAllowed
        );
        assert_eq!(
            code(tools_dir(&lab.case, &lab.settings)),
            ErrorCode::PathNotAllowed
        );
        assert_eq!(
            code(tools_dir(&lab.root.path().join("missing"), &lab.settings)),
            ErrorCode::PathNotAllowed
        );
    }

    #[test]
    fn row_case_open_any_folder_with_a_valid_case_json() {
        let lab = Lab::new();
        let mut settings = settings::defaults(lab.root.path());
        // Not known yet: opening it makes it known.
        let case = case::open(&lab.case, &mut settings).unwrap();
        assert_eq!(case.name, examples::case_fields().name);
        assert!(settings::is_recent(&settings, &lab.case_str()));
        let empty = lab.root.path().join("empty");
        fs::create_dir(&empty).unwrap();
        let err: AppError = case::open(&empty, &mut settings).unwrap_err().into();
        assert_eq!(err.code, ErrorCode::CaseNotFound);
    }

    #[test]
    fn row_known_case_and_run_id() {
        let lab = Lab::new();
        let (dir, case) = known_case(&lab.settings, &lab.case_str()).unwrap();
        assert_eq!(dir, lab.case);
        assert_eq!(case.name, examples::case_fields().name);
        // Not in the recent list, or no longer a valid case.
        let other = case::create(lab.root.path(), &examples::case_fields()).unwrap();
        assert_eq!(
            code(known_case(&lab.settings, &other.path.to_string_lossy())),
            ErrorCode::CaseNotFound
        );
        let mut broken = lab.settings.clone();
        settings::touch_recent(&mut broken, &lab.root.path().to_string_lossy());
        assert_eq!(
            code(known_case(&broken, &lab.root.path().to_string_lossy())),
            ErrorCode::CaseNotFound
        );
        // Run ids: the format, then existence.
        assert_eq!(
            run_dir(&lab.case, &lab.run_id).unwrap(),
            lab.case.join("runs").join(&lab.run_id)
        );
        for bad in [
            "../../etc",
            "20260924-183005Z-ileapp-3f9a1",
            "20260924-183005Z-ileapp-3f9a1d",
            "",
        ] {
            assert_eq!(
                code(run_dir(&lab.case, bad)),
                ErrorCode::RunNotFound,
                "{bad}"
            );
        }
    }

    #[test]
    fn row_readable_file() {
        let lab = Lab::new();
        let file = lab.root.path().join("a.zip");
        fs::write(&file, "zip").unwrap();
        assert_eq!(readable_file(&file).unwrap(), file);
        assert_eq!(
            code(readable_file(lab.root.path())),
            ErrorCode::InvalidInput
        );
        assert_eq!(
            code(readable_file(&lab.root.path().join("missing.zip"))),
            ErrorCode::InvalidInput
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&file, fs::Permissions::from_mode(0o000)).unwrap();
            assert_eq!(code(readable_file(&file)), ErrorCode::PermissionDenied);
            fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        }
    }

    #[test]
    fn row_export_dest() {
        let lab = Lab::new();
        let fine = lab.root.path().join("Triage.alprofile");
        assert!(export_dest(&fine, &lab.settings).is_ok());
        // Next to the case (not in runs/) is fine too.
        assert!(export_dest(&lab.case.join("Triage.alprofile"), &lab.settings).is_ok());
        let in_runs = lab.case.join("runs").join(&lab.run_id).join("x.alprofile");
        assert_eq!(
            code(export_dest(&in_runs, &lab.settings)),
            ErrorCode::PathNotAllowed
        );
    }

    #[test]
    fn row_revealable() {
        let lab = Lab::new();
        let run = lab.case.join("runs").join(&lab.run_id);
        assert!(revealable(&run, &lab.paths, &lab.settings).is_ok());
        assert!(revealable(&lab.case, &lab.paths, &lab.settings).is_ok());
        assert!(revealable(&lab.paths.app_log, &lab.paths, &lab.settings).is_ok());
        for outside in [
            lab.root.path().to_path_buf(),
            lab.root.path().join("elsewhere"),
        ] {
            assert_eq!(
                code(revealable(&outside, &lab.paths, &lab.settings)),
                ErrorCode::PathNotAllowed
            );
        }
    }

    #[test]
    fn row_acq_id() {
        let lab = Lab::new();
        assert_eq!(
            acq_dir(&lab.case, ACQ_ID).unwrap(),
            lab.case.join("acquisitions").join(ACQ_ID)
        );
        for bad in ["20260924-171200Z-ios-000000", "../x", ""] {
            assert_eq!(
                code(acq_dir(&lab.case, bad)),
                ErrorCode::AcqNotFound,
                "{bad}"
            );
        }
    }

    #[test]
    fn row_udid() {
        assert!(udid("00008101-000A1B2C3D4E001E").is_ok());
        assert!(udid(&"a".repeat(40)).is_ok());
        for bad in ["", "00008101", "../../x", &"g".repeat(40)] {
            assert_eq!(code(udid(bad)), ErrorCode::DeviceNotFound, "{bad}");
        }
    }
}
