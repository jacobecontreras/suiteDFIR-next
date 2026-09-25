//! LEAPP argv building (docs/LEAPP-CLI.md §4) and the redacted copy recorded in `run.json`.
//!
//! ```text
//! <entry> -t <type> -i <abs input> -o <abs run_dir> --custom_output_folder report \
//!         -d <abs run_dir>/case.lcasedata [-m <abs run_dir>/profile.<ext>] \
//!         [-tz <zone>] [--itunes_password <pw>] [--keychain <abs path>]
//! ```

use std::fmt;
use std::path::{Component, Path, PathBuf, Prefix};

use crate::contracts::{AppError, ErrorCode, InputType, RunCommand};

use super::casedata;
use super::record::REPORT_DIR;

/// The flag whose value is never recorded.
pub const PASSWORD_FLAG: &str = "--itunes_password";
/// What the recorded argv holds instead of the password.
pub const REDACTED: &str = "<redacted>";

/// What goes into a LEAPP command line. Paths are made absolute; `profile` is the run's
/// `profile.<ext>` (absent for module mode `all`); `timezone`, `itunes_password` and `keychain`
/// are iLEAPP-only, and the caller passes the timezone for every iLEAPP run.
#[derive(Clone, Copy)]
pub struct ArgvSpec<'a> {
    pub entry: &'a Path,
    pub input_type: InputType,
    pub input: &'a Path,
    pub run_dir: &'a Path,
    pub profile: Option<&'a Path>,
    pub timezone: Option<&'a str>,
    pub itunes_password: Option<&'a str>,
    pub keychain: Option<&'a Path>,
}

impl fmt::Debug for ArgvSpec<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArgvSpec")
            .field("entry", &self.entry)
            .field("input_type", &self.input_type)
            .field("input", &self.input)
            .field("run_dir", &self.run_dir)
            .field("profile", &self.profile)
            .field("timezone", &self.timezone)
            .field("itunes_password", &self.itunes_password.map(|_| REDACTED))
            .field("keychain", &self.keychain)
            .finish()
    }
}

/// A path that cannot go into the command line. The message names the argument and path only.
#[derive(Debug, thiserror::Error)]
pub enum ArgvError {
    #[error("the {what} path {path} cannot be made absolute: {source}")]
    NotAbsolute {
        what: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the {what} path {path} is not valid Unicode")]
    NotUnicode { what: &'static str, path: PathBuf },
    #[error(r"the {what} path {path} is a \\?\ or \\.\ path; LEAPP needs a plain absolute path")]
    SpecialPrefix { what: &'static str, path: String },
}

impl ArgvError {
    /// The `AppError` code (CONTRACTS.md §12): the path cannot be passed to LEAPP. [`check_path`]
    /// raises these during validation, before anything is created; from [`build`] (after the run
    /// folder exists) they end the run as `prepare_failed`.
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::NotAbsolute { .. } | Self::NotUnicode { .. } | Self::SpecialPrefix { .. } => {
                ErrorCode::PathNotAllowed
            }
        }
    }
}

impl From<ArgvError> for AppError {
    fn from(err: ArgvError) -> Self {
        AppError {
            code: err.code(),
            message: "This path cannot be passed to LEAPP".to_owned(),
            detail: Some(err.to_string()),
        }
    }
}

/// A LEAPP command line. It holds the password, so `Debug` shows only the redacted argv, and
/// [`Self::argv`] is for spawning only.
#[derive(Clone, PartialEq, Eq)]
pub struct LeappCommand {
    argv: Vec<String>,
    cwd: String,
    /// The index of the password in `argv`, remembered when it is added: redaction goes by
    /// position, never by matching argument text.
    password_at: Option<usize>,
}

impl LeappCommand {
    /// The real argv, including the password: pass it to the process spawn and nowhere else.
    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    /// The working directory: the run folder.
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// `command` as recorded in `run.json`: argv verbatim except the password.
    pub fn recorded(&self) -> RunCommand {
        RunCommand {
            argv: self.redacted_argv(),
            cwd: self.cwd.clone(),
        }
    }

    /// `argv` with the password's position (the value after `--itunes_password`) replaced by
    /// `<redacted>`.
    fn redacted_argv(&self) -> Vec<String> {
        let mut argv = self.argv.clone();
        if let Some(slot) = self.password_at.and_then(|at| argv.get_mut(at)) {
            REDACTED.clone_into(slot);
        }
        argv
    }
}

impl fmt::Debug for LeappCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeappCommand")
            .field("argv", &self.redacted_argv())
            .field("cwd", &self.cwd)
            .finish()
    }
}

/// Builds the LEAPP command line (LEAPP-CLI.md §4).
pub fn build(spec: &ArgvSpec<'_>) -> Result<LeappCommand, ArgvError> {
    let run_dir = absolute("run folder", spec.run_dir)?;
    let mut argv = vec![
        absolute("tool", spec.entry)?,
        "-t".to_owned(),
        spec.input_type.as_str().to_owned(),
        "-i".to_owned(),
        absolute("input", spec.input)?,
        "-o".to_owned(),
        run_dir.clone(),
        "--custom_output_folder".to_owned(),
        REPORT_DIR.to_owned(),
        "-d".to_owned(),
        absolute(
            "case data",
            &Path::new(&run_dir).join(casedata::CASE_DATA_FILE),
        )?,
    ];
    if let Some(profile) = spec.profile {
        argv.extend(["-m".to_owned(), absolute("profile", profile)?]);
    }
    if let Some(zone) = spec.timezone {
        argv.extend(["-tz".to_owned(), zone.to_owned()]);
    }
    let mut password_at = None;
    if let Some(password) = spec.itunes_password {
        argv.push(PASSWORD_FLAG.to_owned());
        password_at = Some(argv.len());
        argv.push(password.to_owned());
    }
    if let Some(keychain) = spec.keychain {
        argv.extend(["--keychain".to_owned(), absolute("keychain", keychain)?]);
    }
    Ok(LeappCommand {
        argv,
        cwd: run_dir,
        password_at,
    })
}

/// Checks that a path can be passed to LEAPP and returns it as `build` will write it (absolute,
/// Unicode, not a `\\?\` or `\\.\` path). Validation (`run_start`, lifecycle step 1) calls it for
/// the tool, input and keychain paths, so these errors come before anything is created.
pub fn check_path(what: &'static str, path: &Path) -> Result<String, ArgvError> {
    absolute(what, path)
}

/// `path` made absolute with `std::path::absolute` (never canonicalized, ARCHITECTURE.md §7), as
/// a string. Verbatim (`\\?\`) and device (`\\.\`) paths are refused: LEAPP adds the verbatim
/// prefix itself and checks `path[1] == ':'`.
fn absolute(what: &'static str, path: &Path) -> Result<String, ArgvError> {
    let absolute = std::path::absolute(path).map_err(|source| ArgvError::NotAbsolute {
        what,
        path: path.to_path_buf(),
        source,
    })?;
    let text = absolute.to_str().ok_or_else(|| ArgvError::NotUnicode {
        what,
        path: path.to_path_buf(),
    })?;
    if has_special_prefix(&absolute) {
        return Err(ArgvError::SpecialPrefix {
            what,
            path: text.to_owned(),
        });
    }
    Ok(text.to_owned())
}

/// Whether `path` is a Windows verbatim (`\\?\…`) or device (`\\.\…`) path. The spelling is
/// checked too, so the answer is the same on every OS.
fn has_special_prefix(path: &Path) -> bool {
    let special_component = matches!(
        path.components().next(),
        Some(Component::Prefix(prefix)) if matches!(
            prefix.kind(),
            Prefix::Verbatim(_)
                | Prefix::VerbatimUNC(..)
                | Prefix::VerbatimDisk(_)
                | Prefix::DeviceNS(_)
        )
    );
    let text = path.as_os_str().to_string_lossy();
    special_component
        || [r"\\?\", r"\\.\", "//?/", "//./"]
            .iter()
            .any(|prefix| text.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::contracts::examples;

    /// Absolute on the host OS.
    fn root() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\x")
        } else {
            PathBuf::from("/x")
        }
    }

    fn s(path: &Path) -> String {
        path.to_str().unwrap().to_owned()
    }

    #[test]
    fn full_ileapp_command_matches_leapp_cli_4() {
        let r = root();
        let entry = r.join("tools").join("ileapp");
        let input = r.join("evidence").join("backup");
        let run_dir = r
            .join("case")
            .join("runs")
            .join("20260924-183005Z-ileapp-3f9a1c");
        let profile = run_dir.join("profile.ilprofile");
        let keychain = r.join("evidence").join("keychain.plist");
        let command = build(&ArgvSpec {
            entry: &entry,
            input_type: InputType::Itunes,
            input: &input,
            run_dir: &run_dir,
            profile: Some(&profile),
            timezone: Some("America/Chicago"),
            itunes_password: Some(examples::EXAMPLE_PASSWORD),
            keychain: Some(&keychain),
        })
        .unwrap();
        let expected = vec![
            s(&entry),
            "-t".to_owned(),
            "itunes".to_owned(),
            "-i".to_owned(),
            s(&input),
            "-o".to_owned(),
            s(&run_dir),
            "--custom_output_folder".to_owned(),
            "report".to_owned(),
            "-d".to_owned(),
            s(&run_dir.join("case.lcasedata")),
            "-m".to_owned(),
            s(&profile),
            "-tz".to_owned(),
            "America/Chicago".to_owned(),
            "--itunes_password".to_owned(),
            examples::EXAMPLE_PASSWORD.to_owned(),
            "--keychain".to_owned(),
            s(&keychain),
        ];
        assert_eq!(command.argv(), expected);
        assert_eq!(command.cwd(), s(&run_dir));

        let recorded = command.recorded();
        let mut expected_recorded = expected;
        expected_recorded[16] = "<redacted>".to_owned();
        assert_eq!(recorded.argv, expected_recorded);
        assert_eq!(recorded.cwd, s(&run_dir));
    }

    #[test]
    fn minimal_aleapp_command() {
        let r = root();
        let run_dir = r
            .join("case")
            .join("runs")
            .join("20260924-183005Z-aleapp-000000");
        let command = build(&ArgvSpec {
            entry: &r.join("aleapp"),
            input_type: InputType::Zip,
            input: &r.join("dump.zip"),
            run_dir: &run_dir,
            profile: None,
            timezone: None,
            itunes_password: None,
            keychain: None,
        })
        .unwrap();
        assert_eq!(
            command.argv(),
            [
                s(&r.join("aleapp")),
                "-t".to_owned(),
                "zip".to_owned(),
                "-i".to_owned(),
                s(&r.join("dump.zip")),
                "-o".to_owned(),
                s(&run_dir),
                "--custom_output_folder".to_owned(),
                "report".to_owned(),
                "-d".to_owned(),
                s(&run_dir.join("case.lcasedata")),
            ]
        );
        assert_eq!(command.recorded().argv, command.argv());
    }

    #[test]
    fn relative_paths_become_absolute() {
        let r = root();
        let command = build(&ArgvSpec {
            entry: &r.join("ileapp"),
            input_type: InputType::Fs,
            input: Path::new("relative/input"),
            run_dir: &r.join("run"),
            profile: None,
            timezone: Some("UTC"),
            itunes_password: None,
            keychain: None,
        })
        .unwrap();
        let input = Path::new(&command.argv()[4]);
        assert!(input.is_absolute(), "{input:?}");
        assert!(input.ends_with("relative/input"));
    }

    /// An iLEAPP command with the given timezone and password, and a keychain after them.
    fn with_secrets(timezone: &str, password: Option<&str>) -> (LeappCommand, String) {
        let r = root();
        let keychain = r.join("keychain.plist");
        let command = build(&ArgvSpec {
            entry: &r.join("ileapp"),
            input_type: InputType::Itunes,
            input: &r.join("backup"),
            run_dir: &r.join("run"),
            profile: None,
            timezone: Some(timezone),
            itunes_password: password,
            keychain: Some(&keychain),
        })
        .unwrap();
        (command, s(&keychain))
    }

    /// The last six recorded arguments: `-tz`, zone, the password flag and value, the keychain.
    fn recorded_tail(command: &LeappCommand) -> Vec<String> {
        let recorded = command.recorded().argv;
        recorded[recorded.len() - 6..].to_vec()
    }

    #[test]
    fn redaction_replaces_only_the_password_value() {
        let (command, keychain) = with_secrets("UTC", Some("secret"));
        assert_eq!(
            recorded_tail(&command),
            [
                "-tz",
                "UTC",
                "--itunes_password",
                "<redacted>",
                "--keychain",
                &keychain
            ]
        );
        assert!(!format!("{command:?}").contains("secret"));
        // Without a password nothing is redacted.
        let (command, _) = with_secrets("UTC", None);
        assert_eq!(command.recorded().argv, command.argv());
    }

    #[test]
    fn a_password_spelled_like_the_flag_hides_only_itself() {
        let (command, keychain) = with_secrets("UTC", Some(PASSWORD_FLAG));
        assert_eq!(
            recorded_tail(&command),
            [
                "-tz",
                "UTC",
                "--itunes_password",
                "<redacted>",
                "--keychain",
                &keychain
            ]
        );
    }

    #[test]
    fn a_timezone_spelled_like_the_flag_never_exposes_the_password() {
        // Redaction goes by the password's position, not by the text before it.
        let (command, keychain) = with_secrets(PASSWORD_FLAG, Some(examples::EXAMPLE_PASSWORD));
        assert_eq!(
            recorded_tail(&command),
            [
                "-tz",
                "--itunes_password",
                "--itunes_password",
                "<redacted>",
                "--keychain",
                &keychain
            ]
        );
        let recorded = format!("{:?}", command.recorded());
        let debug = format!("{command:?}");
        for text in [recorded, debug] {
            assert!(!text.contains(examples::EXAMPLE_PASSWORD), "{text}");
        }
        // The spawned argv is untouched.
        assert!(
            command
                .argv()
                .iter()
                .any(|arg| arg == examples::EXAMPLE_PASSWORD)
        );
    }

    #[test]
    fn check_path_matches_build() {
        let r = root();
        let input = r.join("evidence").join("backup");
        assert_eq!(check_path("input", &input).unwrap(), s(&input));
        let err = check_path("input", Path::new("")).unwrap_err();
        assert_eq!(err.code(), ErrorCode::PathNotAllowed);
    }

    #[test]
    fn special_prefixes_are_recognized_on_every_os() {
        for path in [
            r"\\?\C:\evidence",
            r"\\?\UNC\server\share\evidence",
            r"\\.\C:\evidence",
            r"\\.\PhysicalDrive0",
            "//?/C:/evidence",
            "//./PhysicalDrive0",
        ] {
            assert!(has_special_prefix(Path::new(path)), "{path}");
        }
        for path in [
            r"C:\evidence",
            "/evidence",
            r"\\server\share\evidence",
            "C:/x",
        ] {
            assert!(!has_special_prefix(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn argv_errors_map_to_path_not_allowed() {
        let errors = [
            ArgvError::NotAbsolute {
                what: "input",
                path: PathBuf::new(),
                source: std::io::Error::from(std::io::ErrorKind::InvalidInput),
            },
            ArgvError::NotUnicode {
                what: "input",
                path: PathBuf::from("x"),
            },
            ArgvError::SpecialPrefix {
                what: "keychain",
                path: r"\\.\C:\k".to_owned(),
            },
        ];
        for err in errors {
            assert_eq!(err.code(), ErrorCode::PathNotAllowed, "{err}");
            let detail = err.to_string();
            let app = AppError::from(err);
            assert_eq!(app.code, ErrorCode::PathNotAllowed);
            assert_eq!(app.message, "This path cannot be passed to LEAPP");
            assert_eq!(app.detail.as_deref(), Some(detail.as_str()));
        }
        // An empty path cannot be made absolute.
        let r = root();
        let err = build(&ArgvSpec {
            entry: &r.join("ileapp"),
            input_type: InputType::Fs,
            input: Path::new(""),
            run_dir: &r.join("run"),
            profile: None,
            timezone: Some("UTC"),
            itunes_password: None,
            keychain: None,
        })
        .unwrap_err();
        assert!(
            matches!(err, ArgvError::NotAbsolute { what: "input", .. }),
            "{err:?}"
        );
        assert_eq!(AppError::from(err).code, ErrorCode::PathNotAllowed);
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_and_device_paths_are_refused() {
        for input in [
            r"\\?\C:\evidence",
            r"\\.\C:\evidence",
            "//?/C:/evidence",
            "//./C:/evidence",
        ] {
            let err = build(&ArgvSpec {
                entry: Path::new(r"C:\tools\ileapp.exe"),
                input_type: InputType::Fs,
                input: Path::new(input),
                run_dir: Path::new(r"C:\case\runs\20260924-183005Z-ileapp-3f9a1c"),
                profile: None,
                timezone: Some("UTC"),
                itunes_password: Some(examples::EXAMPLE_PASSWORD),
                keychain: None,
            })
            .unwrap_err();
            assert_eq!(err.code(), ErrorCode::PathNotAllowed, "{input}: {err:?}");
            assert!(!err.to_string().contains(examples::EXAMPLE_PASSWORD));
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_paths_are_refused() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let input = Path::new(OsStr::from_bytes(b"/evidence/\xff"));
        let err = build(&ArgvSpec {
            entry: Path::new("/tools/ileapp"),
            input_type: InputType::Fs,
            input,
            run_dir: Path::new("/case/runs/20260924-183005Z-ileapp-3f9a1c"),
            profile: None,
            timezone: Some("UTC"),
            itunes_password: Some(examples::EXAMPLE_PASSWORD),
            keychain: None,
        })
        .unwrap_err();
        assert!(
            matches!(err, ArgvError::NotUnicode { what: "input", .. }),
            "{err:?}"
        );
        assert!(!err.to_string().contains(examples::EXAMPLE_PASSWORD));
    }
}
