//! Input inspection and type detection; iTunes backups and `IsEncrypted`; the overlap rule
//! (ARCHITECTURE.md §6 step 1). Inputs are only ever opened read-only.

use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};

use crate::case::RUNS_DIR;
use crate::contracts::{AppError, ErrorCode, InputInspection, InputKind, InputType};
use crate::fsutil;

/// Either file marks a folder as an iTunes/Finder backup.
pub const ITUNES_MARKERS: [&str; 2] = ["Manifest.db", "Manifest.plist"];

/// File extensions (lowercase) of `raw` inputs: disk images and E01, read in place.
const RAW_EXTENSIONS: [&str; 6] = ["e01", "dd", "img", "bin", "raw", "001"];

/// A real `Manifest.plist` is at most a few MiB.
const MAX_MANIFEST_PLIST_BYTES: u64 = 64 * 1024 * 1024;

/// Why an input cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum InspectError {
    #[error("{path} does not exist")]
    NotFound { path: String },
    #[error("{path}: access denied: {source}")]
    PermissionDenied {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("{path}: {reason}")]
    Overlap { path: String, reason: String },
    #[error("{path} is neither a file nor a folder")]
    NotFileOrFolder { path: String },
    #[error("{path}: this tool cannot read this kind of file")]
    UnknownFileType { path: String },
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
}

impl InspectError {
    fn from_io(path: &Path, source: io::Error) -> Self {
        let path = path.display().to_string();
        match source.kind() {
            io::ErrorKind::NotFound => Self::NotFound { path },
            io::ErrorKind::PermissionDenied => Self::PermissionDenied { path, source },
            _ => Self::Io { path, source },
        }
    }

    /// The `AppError` code (CONTRACTS.md §12).
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::NotFound { .. } | Self::NotFileOrFolder { .. } | Self::UnknownFileType { .. } => {
                ErrorCode::InvalidInput
            }
            Self::PermissionDenied { .. } => ErrorCode::PermissionDenied,
            Self::Overlap { .. } => ErrorCode::InputOverlapsCase,
            Self::Io { source, .. } => fsutil::io_error_code(source),
        }
    }
}

impl From<InspectError> for AppError {
    fn from(err: InspectError) -> Self {
        let message = match err.code() {
            ErrorCode::PermissionDenied => {
                "Access to the input was denied (on macOS, Finder backups need Full Disk Access)"
            }
            ErrorCode::InputOverlapsCase => {
                "The input overlaps the case, its runs or the app's folders"
            }
            ErrorCode::InvalidInput => "The input cannot be used with this tool",
            _ => "The input could not be read",
        };
        AppError {
            code: err.code(),
            message: message.to_owned(),
            detail: Some(err.to_string()),
        }
    }
}

// ---- overlap ----

/// The folders an input must stay clear of (ARCHITECTURE.md §6 step 1).
#[derive(Clone, Copy, Debug)]
pub struct OverlapContext<'a> {
    /// The case the run belongs to.
    pub case_dir: &'a Path,
    /// Every known case folder (`settings.recent_cases`).
    pub known_cases: &'a [PathBuf],
    /// The app's folders, including the tools directory ([`crate::paths::AppPaths::app_dirs`]).
    pub app_dirs: &'a [PathBuf],
    /// The parent of the per-run temp folders ([`crate::paths::AppPaths::temp_root`]).
    pub temp_root: &'a Path,
}

/// The overlap rule, via [`fsutil::path_within`] (so `..`, symlinks and junctions are resolved, and
/// folders that do not exist yet resolve through their existing ancestors):
/// - the case folder, the would-be run folder and the temp folder must not equal or lie inside
///   `input`;
/// - `input` must not lie inside any case's `runs/` or the app's folders.
///
/// The run folder `<case>/runs/<run_id>` does not exist yet, so it is checked through `runs/`: it
/// lands inside the input exactly when `runs/` resolves there (e.g. `runs/` is a link into the
/// evidence) or the input lies inside `runs/` (refused by the second rule). The per-run temp folder
/// is checked the same way through the temp root.
///
/// Inputs inside a case's `acquisitions/` are allowed: that is how acquired backups are parsed. Use
/// it for the keychain path too.
pub fn check_overlap(input: &Path, ctx: &OverlapContext<'_>) -> Result<(), InspectError> {
    let within =
        |a: &Path, b: &Path| fsutil::path_within(a, b).map_err(|e| InspectError::from_io(input, e));
    let overlap = |reason: String| InspectError::Overlap {
        path: input.display().to_string(),
        reason,
    };
    if within(ctx.case_dir, input)? {
        return Err(overlap(format!(
            "the case folder {} would lie inside the input",
            ctx.case_dir.display()
        )));
    }
    let case_runs = ctx.case_dir.join(RUNS_DIR);
    if within(&case_runs, input)? {
        return Err(overlap(format!(
            "the run folder would be created inside the input ({} resolves there)",
            case_runs.display()
        )));
    }
    if within(ctx.temp_root, input)? {
        return Err(overlap(format!(
            "the app's temp folder {} would lie inside the input",
            ctx.temp_root.display()
        )));
    }
    for case in std::iter::once(ctx.case_dir).chain(ctx.known_cases.iter().map(PathBuf::as_path)) {
        let runs = case.join(RUNS_DIR);
        if within(input, &runs)? {
            return Err(overlap(format!(
                "the input lies inside the runs folder {}",
                runs.display()
            )));
        }
    }
    for dir in ctx.app_dirs {
        if within(input, dir)? {
            return Err(overlap(format!(
                "the input lies inside the app folder {}",
                dir.display()
            )));
        }
    }
    Ok(())
}

// ---- detection ----

/// The types that make sense for a kind of input.
fn kind_compatible(kind: InputKind, input_type: InputType) -> bool {
    match kind {
        InputKind::Directory => matches!(input_type, InputType::Fs | InputType::Itunes),
        InputKind::File => matches!(
            input_type,
            InputType::Tar | InputType::Zip | InputType::Gz | InputType::File | InputType::Raw
        ),
    }
}

/// `allowed_types`: the tool's `input_types` (in their order) that fit the kind of input.
pub fn allowed_types(kind: InputKind, input_types: &[InputType]) -> Vec<InputType> {
    input_types
        .iter()
        .copied()
        .filter(|t| kind_compatible(kind, *t))
        .collect()
}

/// The type of a file from its extension (case-insensitive): `.zip` → zip, `.tar` → tar,
/// `.gz`/`.tgz` → gz, disk images → raw, anything else → `file` if the tool takes single files
/// (iLEAPP), else `None` (aLEAPP: not a valid input).
pub fn detect_file_type(path: &Path, input_types: &[InputType]) -> Option<InputType> {
    let ext = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase());
    let detected = match ext.as_deref() {
        Some("zip") => InputType::Zip,
        Some("tar") => InputType::Tar,
        Some("gz" | "tgz") => InputType::Gz,
        Some(ext) if RAW_EXTENSIONS.contains(&ext) => InputType::Raw,
        _ => InputType::File,
    };
    input_types.contains(&detected).then_some(detected)
}

/// Whether a folder is an iTunes/Finder backup (it has `Manifest.db` or `Manifest.plist`).
pub fn is_itunes_backup(dir: &Path) -> io::Result<bool> {
    for marker in ITUNES_MARKERS {
        if dir.join(marker).try_exists()? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `IsEncrypted` from a backup's `Manifest.plist` (XML or binary). `Ok(None)` with a warning when it
/// cannot be told; a permission error is an error.
fn itunes_encrypted(dir: &Path) -> Result<(Option<bool>, Option<String>), InspectError> {
    let path = dir.join("Manifest.plist");
    let unknown = |why: &str| Ok((None, Some(format!("Backup encryption is unknown: {why}"))));
    let meta = match fs::metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return unknown("the backup has no Manifest.plist");
        }
        Err(e) => return Err(InspectError::from_io(&path, e)),
    };
    if meta.len() > MAX_MANIFEST_PLIST_BYTES {
        return unknown("Manifest.plist is too large");
    }
    let file = File::open(&path).map_err(|e| InspectError::from_io(&path, e))?;
    let value = match plist::Value::from_reader(BufReader::new(file)) {
        Ok(value) => value,
        Err(e) => return unknown(&format!("Manifest.plist cannot be read ({e})")),
    };
    match value
        .as_dictionary()
        .and_then(|dict| dict.get("IsEncrypted"))
        .map(plist::Value::as_boolean)
    {
        Some(Some(encrypted)) => Ok((Some(encrypted), None)),
        Some(None) => unknown("IsEncrypted in Manifest.plist is not a boolean"),
        None => unknown("Manifest.plist has no IsEncrypted"),
    }
}

/// `input_inspect`: checks the overlap rule and that the input is readable, then reports its kind,
/// size, detected type (C3 table), `allowed_types` (`input_types` ∩ kind-compatible types; the tool's
/// `input_types` are passed in), and for iTunes backups `IsEncrypted`. Nothing is written.
pub fn inspect(
    path: &Path,
    input_types: &[InputType],
    overlap: &OverlapContext<'_>,
) -> Result<InputInspection, InspectError> {
    let io_err = |e| InspectError::from_io(path, e);
    let meta = fs::metadata(path).map_err(io_err)?;
    check_overlap(path, overlap)?;
    let kind = if meta.is_dir() {
        InputKind::Directory
    } else if meta.is_file() {
        InputKind::File
    } else {
        return Err(InspectError::NotFileOrFolder {
            path: path.display().to_string(),
        });
    };
    let allowed = allowed_types(kind, input_types);
    let mut warnings = Vec::new();
    let (detected_type, is_itunes, encrypted) = match kind {
        InputKind::Directory => {
            // Readable: the listing works (it is dropped unread).
            fs::read_dir(path).map_err(io_err)?;
            let is_itunes = is_itunes_backup(path).map_err(io_err)?;
            let detected = if is_itunes && allowed.contains(&InputType::Itunes) {
                InputType::Itunes
            } else {
                InputType::Fs
            };
            let encrypted = if is_itunes {
                let (encrypted, warning) = itunes_encrypted(path)?;
                warnings.extend(warning);
                encrypted
            } else {
                None
            };
            (
                allowed.contains(&detected).then_some(detected),
                is_itunes,
                encrypted,
            )
        }
        InputKind::File => {
            // Readable: opening read-only works (the handle is dropped unread).
            File::open(path).map_err(io_err)?;
            let detected = detect_file_type(path, input_types).ok_or_else(|| {
                InspectError::UnknownFileType {
                    path: path.display().to_string(),
                }
            })?;
            (Some(detected), false, None)
        }
    };
    let absolute = std::path::absolute(path).map_err(io_err)?;
    Ok(InputInspection {
        path: absolute.to_string_lossy().into_owned(),
        kind,
        size_bytes: (kind == InputKind::File).then_some(meta.len()),
        detected_type,
        allowed_types: allowed,
        is_itunes_backup: is_itunes,
        itunes_encrypted: encrypted,
        hashable: kind == InputKind::File,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::contracts::{ToolId, examples};

    /// `base` joined with a `/`-separated relative path, one component at a time, so the result
    /// is spelled with the OS separator (as `std::path::absolute` returns it on Windows).
    fn p(base: &Path, rel: &str) -> PathBuf {
        rel.split('/')
            .fold(base.to_path_buf(), |path, part| path.join(part))
    }

    fn input_types(tool: ToolId) -> Vec<InputType> {
        examples::leapp_manifest().tools[&tool].input_types.clone()
    }

    /// A lab: a case with runs and acquisitions, another known case, app folders and evidence.
    struct Lab {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        case: PathBuf,
        other_case: PathBuf,
        known: Vec<PathBuf>,
        app_dirs: Vec<PathBuf>,
        temp_root: PathBuf,
    }

    impl Lab {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().to_path_buf();
            let case = root.join("cases").join("A");
            let other_case = root.join("cases").join("B");
            for dir in [
                p(&case, "runs/r1/report"),
                p(&case, "acquisitions/q1/backup/udid"),
                case.join("notes"),
                p(&other_case, "runs/r2"),
                p(&root, "app/data/leapp"),
                p(&root, "app/config"),
                p(&root, "app/cache/tmp"),
                p(&root, "app/log"),
                root.join("approved-tools"),
                p(&root, "evidence/dir"),
            ] {
                fs::create_dir_all(dir).unwrap();
            }
            let app_dirs = [
                "app/data",
                "app/config",
                "app/cache",
                "app/log",
                "approved-tools",
            ]
            .map(|dir| root.join(dir))
            .to_vec();
            Self {
                known: vec![case.clone(), other_case.clone()],
                temp_root: p(&root, "app/cache/tmp"),
                app_dirs,
                case,
                other_case,
                root,
                _tmp: tmp,
            }
        }

        fn ctx(&self) -> OverlapContext<'_> {
            OverlapContext {
                case_dir: &self.case,
                known_cases: &self.known,
                app_dirs: &self.app_dirs,
                temp_root: &self.temp_root,
            }
        }

        fn inspect(&self, path: &Path, tool: ToolId) -> Result<InputInspection, InspectError> {
            inspect(path, &input_types(tool), &self.ctx())
        }

        fn file(&self, rel: &str) -> PathBuf {
            let path = p(&self.root.join("evidence"), rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"data").unwrap();
            path
        }
    }

    fn overlaps(lab: &Lab, path: &Path) -> bool {
        match check_overlap(path, &lab.ctx()) {
            Ok(()) => false,
            Err(InspectError::Overlap { .. }) => true,
            Err(e) => panic!("{}: {e}", path.display()),
        }
    }

    #[test]
    fn overlap_table() {
        let lab = Lab::new();
        let r = &lab.root;
        for (rel, expected) in [
            // The case (and its would-be run folder) inside the input.
            ("cases/A", true),
            ("cases", true),
            (".", true),
            ("cases/A/runs", true),
            // Inside a case's runs/: this case or another known one.
            ("cases/A/runs/r1/report", true),
            ("cases/B/runs/r2", true),
            ("cases/B/runs", true),
            // Inside the app folders, including a tools-dir override outside them.
            ("app/data/leapp", true),
            ("app/config", true),
            ("app/log/x", true),
            ("approved-tools", true),
            // The temp folder inside the input.
            ("app", true),
            ("app/cache", true),
            // Allowed: acquisitions, other parts of a case, another case's folder, evidence.
            ("cases/A/acquisitions/q1/backup/udid", false),
            ("cases/A/acquisitions", false),
            ("cases/A/notes", false),
            ("cases/B", false),
            ("cases/B/acquisitions/x", false),
            ("evidence/dir", false),
            ("evidence", false),
            ("cases/Arch", false),
            ("cases/A2/runs", false),
        ] {
            assert_eq!(overlaps(&lab, &p(r, rel)), expected, "{rel}");
        }
        // Spellings with `..` resolve first.
        assert!(overlaps(&lab, &p(r, "evidence/../cases/A/runs/r1")));
        assert!(!overlaps(&lab, &p(r, "cases/A/runs/../acquisitions/q1")));
        // The other case's folder contains only its own runs, not this case.
        assert!(!overlaps(&lab, &lab.other_case));
    }

    #[test]
    fn temp_root_rule_on_its_own() {
        let lab = Lab::new();
        let temp = p(&lab.root, "elsewhere/tmp");
        fs::create_dir_all(&temp).unwrap();
        let ctx = OverlapContext {
            case_dir: &lab.case,
            known_cases: &[],
            app_dirs: &[],
            temp_root: &temp,
        };
        let err = check_overlap(&lab.root.join("elsewhere"), &ctx).unwrap_err();
        assert!(err.to_string().contains("temp folder"), "{err}");
        assert!(check_overlap(&temp, &ctx).is_err());
        assert!(check_overlap(&lab.root.join("evidence"), &ctx).is_ok());
    }

    /// Creates a directory symlink, or returns false (with a message) where the OS does not allow
    /// it: Windows without Developer Mode or admin (ERROR_PRIVILEGE_NOT_HELD, 1314).
    fn try_symlink_dir(target: &Path, link: &Path) -> bool {
        match fsutil::test_support::symlink_dir(target, link) {
            Ok(()) => true,
            Err(e) if cfg!(windows) && e.raw_os_error() == Some(1314) => {
                eprintln!(
                    "SKIPPED symlink overlap checks: creating symlinks needs Developer Mode or \
                     admin (ERROR_PRIVILEGE_NOT_HELD)"
                );
                false
            }
            Err(e) => panic!("symlink {} -> {}: {e}", link.display(), target.display()),
        }
    }

    #[test]
    fn overlap_catches_a_linked_runs_folder() {
        // `<case>/runs` was moved elsewhere and linked back; the input contains the link's target.
        let lab = Lab::new();
        let evidence = lab.root.join("evidence");
        let target = p(&evidence, "sub");
        fs::create_dir_all(&target).unwrap();
        let case = p(&lab.root, "cases/Linked");
        fs::create_dir_all(&case).unwrap();
        if !try_symlink_dir(&target, &case.join("runs")) {
            return;
        }
        let ctx = OverlapContext {
            case_dir: &case,
            known_cases: &[],
            app_dirs: &lab.app_dirs,
            temp_root: &lab.temp_root,
        };
        // A run folder created now would land in the evidence; the case folder itself would not.
        let would_be = case.join("runs").join("20260924-183005Z-ileapp-3f9a1c");
        assert!(fsutil::path_within(&would_be, &evidence).unwrap());
        assert!(!fsutil::path_within(&case, &evidence).unwrap());
        for input in [&evidence, &target] {
            let err = check_overlap(input, &ctx).unwrap_err();
            assert_eq!(err.code(), ErrorCode::InputOverlapsCase, "{input:?}");
            assert!(err.to_string().contains("run folder"), "{err}");
        }
        // Evidence elsewhere is fine.
        assert!(check_overlap(&p(&evidence, "dir"), &ctx).is_ok());
    }

    /// The Windows form of a linked `runs/`: a directory junction. Unlike symlinks, junctions need
    /// no privilege, so this runs on every Windows machine (the symlink tests skip without
    /// Developer Mode or admin).
    #[cfg(windows)]
    #[test]
    fn overlap_catches_a_junctioned_runs_folder() {
        let lab = Lab::new();
        let evidence = lab.root.join("evidence");
        let target = p(&evidence, "sub");
        fs::create_dir_all(&target).unwrap();
        let case = p(&lab.root, "cases/Junction");
        fs::create_dir_all(&case).unwrap();
        let runs = case.join("runs");
        let output = std::process::Command::new("cmd")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(&runs)
            .arg(&target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "mklink /J failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            fs::symlink_metadata(&runs)
                .unwrap()
                .file_type()
                .is_symlink(),
            "runs is a junction (a name-surrogate reparse point)"
        );
        let ctx = OverlapContext {
            case_dir: &case,
            known_cases: &[],
            app_dirs: &lab.app_dirs,
            temp_root: &lab.temp_root,
        };
        let would_be = runs.join("20260924-183005Z-ileapp-3f9a1c");
        assert!(!fsutil::path_within(&case, &evidence).unwrap());
        // Windows may refuse to traverse a junction that a non-admin created
        // (ERROR_UNTRUSTED_MOUNT_POINT, Redirection Guard). Nothing can then be created through
        // it, and the overlap check must still refuse the input (fail closed).
        const ERROR_UNTRUSTED_MOUNT_POINT: i32 = 448;
        match fsutil::path_within(&would_be, &evidence) {
            Ok(within) => {
                assert!(within, "the run folder would land in the evidence");
                for input in [&evidence, &target] {
                    let err = check_overlap(input, &ctx).unwrap_err();
                    assert_eq!(err.code(), ErrorCode::InputOverlapsCase, "{input:?}");
                    assert!(err.to_string().contains("run folder"), "{err}");
                }
                let err = inspect(&evidence, &input_types(ToolId::Ileapp), &ctx).unwrap_err();
                assert_eq!(err.code(), ErrorCode::InputOverlapsCase);
            }
            Err(e) if e.raw_os_error() == Some(ERROR_UNTRUSTED_MOUNT_POINT) => {
                eprintln!(
                    "NOTE: this machine refuses to traverse user-created junctions \
                     (ERROR_UNTRUSTED_MOUNT_POINT); checking that the overlap rule fails closed"
                );
                for input in [&evidence, &target] {
                    let err = check_overlap(input, &ctx).unwrap_err();
                    assert!(
                        matches!(&err, InspectError::Io { source, .. }
                            if source.raw_os_error() == Some(ERROR_UNTRUSTED_MOUNT_POINT)),
                        "{input:?}: {err:?}"
                    );
                }
                assert!(inspect(&evidence, &input_types(ToolId::Ileapp), &ctx).is_err());
                let err = fs::create_dir(&would_be).unwrap_err();
                assert_eq!(err.raw_os_error(), Some(ERROR_UNTRUSTED_MOUNT_POINT));
                assert_eq!(
                    fs::read_dir(&target).unwrap().count(),
                    0,
                    "evidence untouched"
                );
            }
            Err(e) => panic!("{}: {e}", would_be.display()),
        }
        assert!(check_overlap(&p(&evidence, "dir"), &ctx).is_ok());
    }

    #[test]
    fn overlap_catches_linked_case_and_temp_folders() {
        let lab = Lab::new();
        let evidence = lab.root.join("evidence");
        fs::create_dir_all(p(&evidence, "case-home")).unwrap();
        fs::create_dir_all(p(&evidence, "temp-home")).unwrap();
        let case = p(&lab.root, "cases/LinkedCase");
        let temp = p(&lab.root, "app/linked-tmp");
        if !try_symlink_dir(&p(&evidence, "case-home"), &case) {
            return;
        }
        assert!(try_symlink_dir(&p(&evidence, "temp-home"), &temp));
        let with = |case_dir: &Path, temp_root: &Path| {
            let ctx = OverlapContext {
                case_dir,
                known_cases: &[],
                app_dirs: &[],
                temp_root,
            };
            check_overlap(&evidence, &ctx)
        };
        let err = with(&case, &lab.temp_root).unwrap_err();
        assert!(err.to_string().contains("case folder"), "{err}");
        let err = with(&lab.case, &temp).unwrap_err();
        assert!(err.to_string().contains("temp folder"), "{err}");
        assert!(with(&lab.case, &lab.temp_root).is_ok());
    }

    #[test]
    fn overlap_follows_symlinks() {
        let lab = Lab::new();
        let link = p(&lab.root, "evidence/link-to-runs");
        if !try_symlink_dir(&lab.case.join("runs"), &link) {
            return;
        }
        assert!(overlaps(&lab, &link));
        assert!(overlaps(&lab, &link.join("r1")));
        let err = lab.inspect(&link, ToolId::Ileapp).unwrap_err();
        assert_eq!(err.code(), ErrorCode::InputOverlapsCase);
    }

    #[test]
    fn inspect_reports_overlap_as_input_overlaps_case() {
        let lab = Lab::new();
        let err = lab
            .inspect(&p(&lab.case, "runs/r1/report"), ToolId::Ileapp)
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::InputOverlapsCase);
        let app: AppError = err.into();
        assert_eq!(app.code, ErrorCode::InputOverlapsCase);
        assert!(app.detail.unwrap().contains("runs folder"));
    }

    #[test]
    fn file_detection_table() {
        let lab = Lab::new();
        let ileapp = input_types(ToolId::Ileapp);
        let aleapp = input_types(ToolId::Aleapp);
        for (name, for_ileapp, for_aleapp) in [
            ("dump.zip", Some(InputType::Zip), Some(InputType::Zip)),
            ("DUMP.ZIP", Some(InputType::Zip), Some(InputType::Zip)),
            ("fs.tar", Some(InputType::Tar), Some(InputType::Tar)),
            ("fs.tar.gz", Some(InputType::Gz), Some(InputType::Gz)),
            ("fs.tgz", Some(InputType::Gz), Some(InputType::Gz)),
            ("fs.GZ", Some(InputType::Gz), Some(InputType::Gz)),
            ("disk.e01", Some(InputType::Raw), Some(InputType::Raw)),
            ("disk.E01", Some(InputType::Raw), Some(InputType::Raw)),
            ("disk.dd", Some(InputType::Raw), Some(InputType::Raw)),
            ("disk.img", Some(InputType::Raw), Some(InputType::Raw)),
            ("disk.bin", Some(InputType::Raw), Some(InputType::Raw)),
            ("disk.raw", Some(InputType::Raw), Some(InputType::Raw)),
            ("disk.001", Some(InputType::Raw), Some(InputType::Raw)),
            ("sms.db", Some(InputType::File), None),
            ("no_extension", Some(InputType::File), None),
            ("archive.zip.part", Some(InputType::File), None),
            ("disk.002", Some(InputType::File), None),
        ] {
            let path = lab.file(name);
            assert_eq!(
                detect_file_type(&path, &ileapp),
                for_ileapp,
                "iLEAPP {name}"
            );
            assert_eq!(
                detect_file_type(&path, &aleapp),
                for_aleapp,
                "aLEAPP {name}"
            );

            let inspection = lab.inspect(&path, ToolId::Ileapp).unwrap();
            assert_eq!(inspection.detected_type, for_ileapp, "{name}");
            assert_eq!(inspection.kind, InputKind::File);
            assert_eq!(inspection.size_bytes, Some(4));
            assert!(inspection.hashable);
            assert!(!inspection.is_itunes_backup);
            assert_eq!(inspection.itunes_encrypted, None);
            assert_eq!(
                inspection.allowed_types,
                [
                    InputType::Tar,
                    InputType::Zip,
                    InputType::Gz,
                    InputType::File,
                    InputType::Raw
                ]
            );
            match lab.inspect(&path, ToolId::Aleapp) {
                Ok(inspection) => {
                    assert_eq!(inspection.detected_type, for_aleapp, "{name}");
                    assert_eq!(
                        inspection.allowed_types,
                        [
                            InputType::Tar,
                            InputType::Zip,
                            InputType::Gz,
                            InputType::Raw
                        ]
                    );
                }
                Err(err) => {
                    assert_eq!(for_aleapp, None, "{name}: {err}");
                    assert_eq!(err.code(), ErrorCode::InvalidInput);
                }
            }
        }
    }

    #[test]
    fn folder_detection_table() {
        let lab = Lab::new();
        let plain = p(&lab.root, "evidence/dir");
        let with_db = p(&lab.root, "evidence/backup-db");
        let with_plist = p(&lab.root, "evidence/backup-plist");
        fs::create_dir_all(&with_db).unwrap();
        fs::create_dir_all(&with_plist).unwrap();
        fs::write(with_db.join("Manifest.db"), b"SQLite format 3\0").unwrap();
        write_manifest_plist(&with_plist, Some(false), false);

        for (dir, tool, detected, allowed, itunes) in [
            (
                &plain,
                ToolId::Ileapp,
                InputType::Fs,
                vec![InputType::Fs, InputType::Itunes],
                false,
            ),
            (
                &plain,
                ToolId::Aleapp,
                InputType::Fs,
                vec![InputType::Fs],
                false,
            ),
            (
                &with_db,
                ToolId::Ileapp,
                InputType::Itunes,
                vec![InputType::Fs, InputType::Itunes],
                true,
            ),
            (
                &with_db,
                ToolId::Aleapp,
                InputType::Fs,
                vec![InputType::Fs],
                true,
            ),
            (
                &with_plist,
                ToolId::Ileapp,
                InputType::Itunes,
                vec![InputType::Fs, InputType::Itunes],
                true,
            ),
        ] {
            let inspection = lab.inspect(dir, tool).unwrap();
            assert_eq!(inspection.kind, InputKind::Directory);
            assert_eq!(inspection.detected_type, Some(detected), "{dir:?} {tool}");
            assert_eq!(inspection.allowed_types, allowed, "{dir:?} {tool}");
            assert_eq!(inspection.is_itunes_backup, itunes);
            assert_eq!(inspection.size_bytes, None);
            assert!(!inspection.hashable);
        }
        // Only Manifest.db: encryption cannot be told.
        let inspection = lab.inspect(&with_db, ToolId::Ileapp).unwrap();
        assert_eq!(inspection.itunes_encrypted, None);
        assert_eq!(inspection.warnings.len(), 1, "{:?}", inspection.warnings);
        let inspection = lab.inspect(&with_plist, ToolId::Ileapp).unwrap();
        assert_eq!(inspection.itunes_encrypted, Some(false));
        assert_eq!(inspection.warnings, Vec::<String>::new());
        assert_eq!(inspection.path, with_plist.to_string_lossy());
    }

    fn write_manifest_plist(dir: &Path, encrypted: Option<bool>, binary: bool) {
        let mut dict = plist::Dictionary::new();
        dict.insert(
            "Version".to_owned(),
            plist::Value::String("10.0".to_owned()),
        );
        if let Some(encrypted) = encrypted {
            dict.insert("IsEncrypted".to_owned(), plist::Value::Boolean(encrypted));
        }
        let value = plist::Value::Dictionary(dict);
        let file = File::create(dir.join("Manifest.plist")).unwrap();
        if binary {
            value.to_writer_binary(file).unwrap();
        } else {
            value.to_writer_xml(file).unwrap();
        }
    }

    #[test]
    fn is_encrypted_from_manifest_plist() {
        let lab = Lab::new();
        let dir = p(&lab.root, "evidence/00008101-000A1B2C3D4E");
        fs::create_dir_all(&dir).unwrap();
        for (encrypted, binary) in [(true, false), (false, false), (true, true), (false, true)] {
            write_manifest_plist(&dir, Some(encrypted), binary);
            let inspection = lab.inspect(&dir, ToolId::Ileapp).unwrap();
            assert_eq!(
                inspection.itunes_encrypted,
                Some(encrypted),
                "binary {binary}"
            );
            assert!(inspection.is_itunes_backup);
            assert_eq!(inspection.detected_type, Some(InputType::Itunes));
        }
        // Without the key, with a wrong type, or unparsable: unknown, with a warning.
        write_manifest_plist(&dir, None, false);
        let inspection = lab.inspect(&dir, ToolId::Ileapp).unwrap();
        assert_eq!(inspection.itunes_encrypted, None);
        assert!(inspection.warnings[0].contains("no IsEncrypted"));
        let mut dict = plist::Dictionary::new();
        dict.insert(
            "IsEncrypted".to_owned(),
            plist::Value::String("yes".to_owned()),
        );
        plist::Value::Dictionary(dict)
            .to_file_xml(dir.join("Manifest.plist"))
            .unwrap();
        let inspection = lab.inspect(&dir, ToolId::Ileapp).unwrap();
        assert_eq!(inspection.itunes_encrypted, None);
        assert!(inspection.warnings[0].contains("not a boolean"));
        fs::write(dir.join("Manifest.plist"), b"garbage").unwrap();
        let inspection = lab.inspect(&dir, ToolId::Ileapp).unwrap();
        assert_eq!(inspection.itunes_encrypted, None);
        assert!(inspection.warnings[0].contains("cannot be read"));
        // The example fixture's shape.
        write_manifest_plist(&dir, Some(true), true);
        let mut expected = examples::input_inspection();
        expected.path = dir.to_string_lossy().into_owned();
        assert_eq!(lab.inspect(&dir, ToolId::Ileapp).unwrap(), expected);
    }

    #[test]
    fn inspection_never_writes() {
        let lab = Lab::new();
        let dir = p(&lab.root, "evidence/backup");
        fs::create_dir_all(&dir).unwrap();
        write_manifest_plist(&dir, Some(true), false);
        let file = lab.file("dump.zip");
        let snapshot = |path: &Path| {
            let meta = fs::metadata(path).unwrap();
            (
                meta.len(),
                meta.modified().unwrap(),
                meta.permissions().readonly(),
            )
        };
        let before = (
            snapshot(&dir.join("Manifest.plist")),
            snapshot(&file),
            fs::read_dir(&dir).unwrap().count(),
        );
        lab.inspect(&dir, ToolId::Ileapp).unwrap();
        lab.inspect(&file, ToolId::Ileapp).unwrap();
        let after = (
            snapshot(&dir.join("Manifest.plist")),
            snapshot(&file),
            fs::read_dir(&dir).unwrap().count(),
        );
        assert_eq!(before, after);
        // Read-only inputs inspect fine.
        fsutil::set_read_only(&file).unwrap();
        assert!(lab.inspect(&file, ToolId::Ileapp).is_ok());
        fsutil::test_support::make_writable(&file);
    }

    #[test]
    fn missing_inputs_are_invalid() {
        let lab = Lab::new();
        let err = lab
            .inspect(&p(&lab.root, "evidence/missing"), ToolId::Ileapp)
            .unwrap_err();
        assert!(matches!(err, InspectError::NotFound { .. }), "{err:?}");
        assert_eq!(err.code(), ErrorCode::InvalidInput);
    }

    #[cfg(unix)]
    #[test]
    fn permission_errors_are_permission_denied() {
        use std::os::unix::fs::PermissionsExt;
        let lab = Lab::new();
        let locked_dir = p(&lab.root, "evidence/locked-dir");
        fs::create_dir_all(&locked_dir).unwrap();
        let locked_file = lab.file("locked.zip");
        let hidden = lab.file("sealed/inner.zip");
        let backup = p(&lab.root, "evidence/locked-backup");
        fs::create_dir_all(&backup).unwrap();
        write_manifest_plist(&backup, Some(true), false);
        let mode = |path: &Path, mode| {
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
        };
        mode(&locked_dir, 0o000);
        mode(&locked_file, 0o000);
        mode(hidden.parent().unwrap(), 0o000);
        mode(&backup.join("Manifest.plist"), 0o000);

        if File::open(&locked_file).is_ok() {
            // Root ignores permissions; Linux CI runs unprivileged, so this is exercised there.
            eprintln!("SKIPPED: running with privileges that bypass file permissions");
        } else {
            for path in [&locked_dir, &locked_file, &hidden, &backup] {
                let err = lab.inspect(path, ToolId::Ileapp).unwrap_err();
                assert_eq!(err.code(), ErrorCode::PermissionDenied, "{path:?}: {err}");
            }
        }
        mode(&locked_dir, 0o755);
        mode(&locked_file, 0o644);
        mode(hidden.parent().unwrap(), 0o755);
        mode(&backup.join("Manifest.plist"), 0o644);
    }

    #[cfg(unix)]
    #[test]
    fn special_files_are_invalid() {
        let lab = Lab::new();
        // /dev/null is a character device: neither a file nor a folder.
        let ctx = lab.ctx();
        let err = inspect(Path::new("/dev/null"), &input_types(ToolId::Ileapp), &ctx).unwrap_err();
        assert!(
            matches!(err, InspectError::NotFileOrFolder { .. }),
            "{err:?}"
        );
        assert_eq!(err.code(), ErrorCode::InvalidInput);
    }

    #[test]
    fn allowed_types_keep_the_tool_order() {
        let ileapp = input_types(ToolId::Ileapp);
        assert_eq!(
            allowed_types(InputKind::Directory, &ileapp),
            [InputType::Fs, InputType::Itunes]
        );
        // Only what the tool takes, even if the kind would allow more.
        assert_eq!(
            allowed_types(
                InputKind::File,
                &[InputType::Raw, InputType::Zip, InputType::Fs]
            ),
            [InputType::Raw, InputType::Zip]
        );
        assert_eq!(allowed_types(InputKind::Directory, &[]), []);
    }
}
