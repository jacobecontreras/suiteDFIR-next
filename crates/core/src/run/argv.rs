//! LEAPP argv building (docs/LEAPP-CLI.md §4) and the redacted copy recorded in `run.json`.
//!
//! ```text
//! <entry> -t <type> -i <abs input> -o <abs run_dir> --custom_output_folder report \
//!         -d <abs run_dir>/case.lcasedata [-m <abs run_dir>/profile.<ext>] \
//!         [-tz <zone>] [--itunes_password <pw>] [--keychain <abs path>]
//! ```

use std::fmt;
use std::path::{Path, PathBuf};

use crate::contracts::{InputType, RunCommand};

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
    #[error(r"the {what} path {path} is \\?\-prefixed; LEAPP needs a plain absolute path")]
    Verbatim { what: &'static str, path: String },
}

/// A LEAPP command line. It holds the password, so `Debug` shows only the redacted argv, and
/// [`Self::argv`] is for spawning only.
#[derive(Clone, PartialEq, Eq)]
pub struct LeappCommand {
    argv: Vec<String>,
    cwd: String,
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
            argv: redact(&self.argv),
            cwd: self.cwd.clone(),
        }
    }
}

impl fmt::Debug for LeappCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LeappCommand")
            .field("argv", &redact(&self.argv))
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
    if let Some(password) = spec.itunes_password {
        argv.extend([PASSWORD_FLAG.to_owned(), password.to_owned()]);
    }
    if let Some(keychain) = spec.keychain {
        argv.extend(["--keychain".to_owned(), absolute("keychain", keychain)?]);
    }
    Ok(LeappCommand { argv, cwd: run_dir })
}

/// A copy of `argv` with the value after `--itunes_password` replaced by `<redacted>`.
pub fn redact(argv: &[String]) -> Vec<String> {
    let mut redacted = Vec::with_capacity(argv.len());
    let mut hide_next = false;
    for arg in argv {
        redacted.push(if hide_next {
            REDACTED.to_owned()
        } else {
            arg.clone()
        });
        hide_next = arg == PASSWORD_FLAG;
    }
    redacted
}

/// `path` made absolute with `std::path::absolute` (never canonicalized, so never `\\?\`-prefixed
/// on Windows, ARCHITECTURE.md §7), as a string.
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
    if text.starts_with(r"\\?\") {
        return Err(ArgvError::Verbatim {
            what,
            path: text.to_owned(),
        });
    }
    Ok(text.to_owned())
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

    #[test]
    fn redaction_replaces_only_the_password_value() {
        let argv: Vec<String> = ["x", "--itunes_password", "secret", "-tz", "UTC"]
            .map(str::to_owned)
            .to_vec();
        assert_eq!(
            redact(&argv),
            ["x", "--itunes_password", "<redacted>", "-tz", "UTC"]
        );
        // A trailing flag without a value, and no flag at all.
        let argv: Vec<String> = ["x", "--itunes_password"].map(str::to_owned).to_vec();
        assert_eq!(redact(&argv), argv);
        let argv: Vec<String> = ["x", "-t", "fs"].map(str::to_owned).to_vec();
        assert_eq!(redact(&argv), argv);
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_paths_are_refused() {
        let err = build(&ArgvSpec {
            entry: Path::new(r"C:\tools\ileapp.exe"),
            input_type: InputType::Fs,
            input: Path::new(r"\\?\C:\evidence"),
            run_dir: Path::new(r"C:\case\runs\20260924-183005Z-ileapp-3f9a1c"),
            profile: None,
            timezone: Some("UTC"),
            itunes_password: Some(examples::EXAMPLE_PASSWORD),
            keychain: None,
        })
        .unwrap_err();
        assert!(
            matches!(err, ArgvError::Verbatim { what: "input", .. }),
            "{err:?}"
        );
        assert!(!err.to_string().contains(examples::EXAMPLE_PASSWORD));
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
