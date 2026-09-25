//! fake-leapp: a test double of a LEAPP onefile CLI binary (DEVELOPMENT.md §4.8). Never bundled.
//!
//! **Process shape.** Like a PyInstaller onefile build, the process started by the caller is a
//! "bootloader": it creates `$TMPDIR/_MEIfake<pid>` (the extracted runtime), re-executes itself as
//! `fake-leapp --fake-worker <args>`, waits for that worker and removes the runtime dir on a
//! graceful exit (a normal exit, or SIGTERM on Unix). It forwards SIGTERM to the worker (Unix) and
//! exits like the worker did (the same code, or the same signal re-raised). A SIGKILL or a Windows
//! job termination leaks the runtime dir, as with the real tool (LEAPP-CLI.md Q2). With
//! `FAKE_LEAPP_PIDFILE` set, the bootloader writes `parent <pid>` and `worker <pid>` lines to that
//! file (atomically) once the worker runs.
//!
//! **Worker.** It parses LEAPP's flags (exit 2 with an argparse-style message on bad arguments),
//! writes the LEAPP output layout under `<-o>/<--custom_output_folder>` and appends one
//! `Screen_Output.html` record (`message<br>` plus the OS newline) every `FAKE_LEAPP_INTERVAL_MS`
//! (default 100) for `FAKE_LEAPP_LINES` (default 50) lines. Its stdout is fully buffered until
//! exit, so a killed worker leaves no stdout (LEAPP-CLI.md Q1). `_lava_data.lava` is written at
//! the end. The tool it imitates is iLEAPP when `-tz` is given (suiteDFIR always passes it to
//! iLEAPP) and aLEAPP otherwise; it decides the always-run entries in `_lava_data.lava`.
//!
//! **Scenarios** (`FAKE_LEAPP_SCENARIO`, default `success`), with the outcomes of CONTRACTS.md §7.4:
//! - `success`: every line, then a complete report; exit 0.
//! - `artifact_error`: as `success`, but up to two modules end with status `Error` and a Python
//!   traceback per failed module goes to stderr.
//! - `invalid_input`: a few lines and an error line, then `_lava_data.lava` with `Complete` and no
//!   modules, and no `index.html`; exit 0.
//! - `early_exit`: prints an error and exits 0 before creating the output folder.
//! - `argparse_error`: an argparse error; exit 2, nothing created.
//! - `crash`: half of the lines, then a traceback on stderr; exit 1 without `_lava_data.lava` or
//!   `index.html` (a `_lava_artifacts.db-journal` is left behind).
//! - `prompt`: a few lines, then a password prompt like Python's `getpass`: it opens `/dev/tty` if
//!   possible, otherwise reads stdin. EOF (or any answer) → traceback, exit 1.
//! - `slow`: every line, then a heartbeat line per second for up to 300 s before finishing like
//!   `success`. Meant to be cancelled.
//! - `ignore_term`: as `slow`, but both processes ignore SIGTERM (Unix).
//! - `glibc_too_old`: the bootloader fails to load the Python library, as a pinned Linux build does
//!   on a system whose glibc is too old (LEAPP-CLI.md §2): the loader's
//!   ``version `GLIBC_2.43' not found`` line on stderr, exit 255, nothing created.
//!
//! `fake-leapp --list-modules-json <ileapp|aleapp>` prints the fake module list as a `ToolModules`
//! JSON object (CONTRACTS.md §9) with version `dev-override`, for the debug-build dev override.
//!
//! **Probe copies.** A copy whose file name ends in `-probe` (`fake-leapp-probe[.exe]`) also
//! answers module introspection (LEAPP-CLI.md §5): when its profile selects `suitedfir_probe` and
//! `SUITEDFIR_PROBE_OUT` is set, it writes the always-run artifacts, its catalog and 500 filler
//! plugins (and iLEAPP's timezones) to that file, so the real install pipeline can install it
//! (the E2 replay test). Plain `fake-leapp` never answers the probe.
//!
//! The only `unsafe` code is the libc signal handling in `signals` (Unix): installing the SIGTERM
//! disposition, forwarding SIGTERM with `kill(2)` and re-raising the worker's signal.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::{env, thread};

use serde_json::{Value, json};
use suitedfir_core::contracts::{ModuleInfo, ToolId, ToolModules};

/// The first argument of the re-executed worker process.
const WORKER_FLAG: &str = "--fake-worker";
/// Exit code for a broken test setup (an invalid `FAKE_LEAPP_*` variable), distinct from LEAPP's.
const EXIT_SETUP: i32 = 64;
/// How long `slow` and `ignore_term` keep running after their lines unless they are stopped.
const SLOW_RUN: Duration = Duration::from_secs(300);
/// The newline Python's text mode writes on this OS (LEAPP-CLI.md Q8).
const NL: &str = if cfg!(windows) { "\r\n" } else { "\n" };
const INPUT_TYPES: &[&str] = &["fs", "tar", "zip", "gz", "itunes", "file", "raw"];
const TIMEZONES: &[&str] = &[
    "Africa/Abidjan",
    "America/Chicago",
    "America/Los_Angeles",
    "America/New_York",
    "Asia/Tokyo",
    "Australia/Sydney",
    "Europe/Berlin",
    "Europe/London",
    "UTC",
];
const USAGE: &str = "usage: fake-leapp [-h] [-t {fs,tar,zip,gz,itunes,file,raw}] [-o OUTPUT_PATH] \
[-i INPUT_PATH] [-tz TIMEZONE] [-w] [-m LOAD_PROFILE] [-d LOAD_CASE_DATA] \
[--custom_output_folder CUSTOM_OUTPUT_FOLDER] [--custom_artifacts_path CUSTOM_ARTIFACTS_PATH] \
[--itunes_password ITUNES_PASSWORD] [--keychain KEYCHAIN]";

fn main() {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let code = match args.first().and_then(|arg| arg.to_str()) {
        Some(WORKER_FLAG) => worker(&args[1..]),
        Some("--list-modules-json") => list_modules_json(&args[1..]),
        _ => bootloader(&args),
    };
    std::process::exit(code);
}

// ---- configuration ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    Success,
    ArtifactError,
    InvalidInput,
    EarlyExit,
    ArgparseError,
    Crash,
    Prompt,
    Slow,
    IgnoreTerm,
    GlibcTooOld,
}

impl Scenario {
    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "success" => Self::Success,
            "artifact_error" => Self::ArtifactError,
            "invalid_input" => Self::InvalidInput,
            "early_exit" => Self::EarlyExit,
            "argparse_error" => Self::ArgparseError,
            "crash" => Self::Crash,
            "prompt" => Self::Prompt,
            "slow" => Self::Slow,
            "ignore_term" => Self::IgnoreTerm,
            "glibc_too_old" => Self::GlibcTooOld,
            _ => return None,
        })
    }
}

/// The `FAKE_LEAPP_*` environment.
#[derive(Debug, PartialEq, Eq)]
struct Config {
    scenario: Scenario,
    interval: Duration,
    lines: u32,
    pidfile: Option<PathBuf>,
}

impl Config {
    fn from_env() -> Result<Self, String> {
        Self::from_vars(|name| env::var_os(name))
    }

    fn from_vars(var: impl Fn(&str) -> Option<OsString>) -> Result<Self, String> {
        let text = |name: &str| -> Result<Option<String>, String> {
            var(name)
                .map(|value| {
                    value
                        .into_string()
                        .map_err(|_| format!("{name} is not valid Unicode"))
                })
                .transpose()
        };
        let scenario = match text("FAKE_LEAPP_SCENARIO")? {
            None => Scenario::Success,
            Some(name) => Scenario::parse(&name)
                .ok_or_else(|| format!("unknown FAKE_LEAPP_SCENARIO {name:?}"))?,
        };
        let number = |name: &str, default: u64| -> Result<u64, String> {
            match text(name)? {
                None => Ok(default),
                Some(value) => value
                    .parse()
                    .map_err(|_| format!("{name} must be a non-negative integer, not {value:?}")),
            }
        };
        let lines = u32::try_from(number("FAKE_LEAPP_LINES", 50)?)
            .map_err(|_| "FAKE_LEAPP_LINES is too large".to_owned())?;
        Ok(Self {
            scenario,
            interval: Duration::from_millis(number("FAKE_LEAPP_INTERVAL_MS", 100)?),
            lines,
            pidfile: var("FAKE_LEAPP_PIDFILE").map(PathBuf::from),
        })
    }
}

// ---- bootloader (the process the caller spawned) ----

fn bootloader(args: &[OsString]) -> i32 {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("fake-leapp: {message}");
            return EXIT_SETUP;
        }
    };
    let pid = std::process::id();
    let runtime_dir = env::temp_dir().join(format!("_MEIfake{pid}"));
    if config.scenario == Scenario::GlibcTooOld {
        // What PyInstaller's bootloader prints when the dynamic loader refuses the bundled Python
        // library (LEAPP-CLI.md §2). Nothing is extracted or created.
        eprintln!(
            "[PYI-{pid}:ERROR] Failed to load Python shared library '{}': \
             /lib/x86_64-linux-gnu/libm.so.6: version `GLIBC_2.43' not found (required by {})",
            runtime_dir.join("libpython3.14.so.1.0").display(),
            runtime_dir.join("libmvec.so.1").display()
        );
        return 255;
    }
    if let Err(e) = extract_runtime(&runtime_dir) {
        eprintln!(
            "[PYI-{pid}:ERROR] Could not create temporary directory {}: {e}",
            runtime_dir.display()
        );
        return 255;
    }
    #[cfg(unix)]
    signals::install_bootloader(config.scenario == Scenario::IgnoreTerm);

    let spawned =
        env::current_exe().and_then(|exe| Command::new(exe).arg(WORKER_FLAG).args(args).spawn());
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => {
            eprintln!("[PYI-{pid}:ERROR] Failed to start the worker: {e}");
            let _ = fs::remove_dir_all(&runtime_dir);
            return 255;
        }
    };
    #[cfg(unix)]
    signals::set_worker(child.id());
    if let Some(pidfile) = &config.pidfile
        && let Err(e) = write_pidfile(pidfile, pid, child.id())
    {
        eprintln!("fake-leapp: cannot write {}: {e}", pidfile.display());
    }
    let status = child.wait();
    // Graceful exit: the runtime dir goes away, as PyInstaller's bootloader does.
    let _ = fs::remove_dir_all(&runtime_dir);
    match status {
        Ok(status) => exit_like(status),
        Err(e) => {
            eprintln!("[PYI-{pid}:ERROR] Failed to wait for the worker: {e}");
            255
        }
    }
}

/// Simulates the onefile extraction.
fn extract_runtime(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::write(dir.join("base_library.zip"), vec![0u8; 64 * 1024])
}

/// Writes `parent <pid>` and `worker <pid>` lines through a temporary file and a rename, so a
/// reader never sees a partial file.
fn write_pidfile(path: &Path, parent: u32, worker: u32) -> io::Result<()> {
    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    fs::write(&tmp, format!("parent {parent}\nworker {worker}\n"))?;
    fs::rename(&tmp, path)
}

/// The bootloader's exit code for the worker's status. On Unix a worker killed by a signal makes
/// the bootloader re-raise that signal on itself, as PyInstaller does.
fn exit_like(status: ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return signals::reraise(signal);
        }
    }
    status.code().unwrap_or(1)
}

// ---- worker ----

fn worker(args: &[OsString]) -> i32 {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("fake-leapp: {message}");
            return EXIT_SETUP;
        }
    };
    #[cfg(unix)]
    if config.scenario == Scenario::IgnoreTerm {
        signals::ignore_term();
    }
    let mut stdout = BufferedStdout::default();
    let code = match run_worker(&config, args, &mut stdout) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("fake-leapp: I/O error: {e}");
            1
        }
    };
    // Python flushes stdout at exit; a killed worker never gets here (LEAPP-CLI.md Q1).
    stdout.flush_to_stdout();
    code
}

/// stdout held in memory until the worker exits.
#[derive(Default)]
struct BufferedStdout(String);

impl BufferedStdout {
    fn line(&mut self, message: &str) {
        self.0.push_str(message);
        self.0.push('\n');
    }

    fn flush_to_stdout(&self) {
        let mut out = io::stdout().lock();
        let _ = out.write_all(self.0.as_bytes());
        let _ = out.flush();
    }
}

fn run_worker(config: &Config, args: &[OsString], stdout: &mut BufferedStdout) -> io::Result<i32> {
    let args = match parse_args(args) {
        Ok(args) if args.help => {
            stdout.line(USAGE);
            return Ok(0);
        }
        Ok(args) => args,
        Err(message) => return Ok(argparse_error(&message)),
    };
    if config.scenario == Scenario::ArgparseError {
        return Ok(argparse_error(
            "argument --custom_output_folder: simulated error (FAKE_LEAPP_SCENARIO=argparse_error)",
        ));
    }
    let run = match validate(args) {
        Ok(run) => run,
        Err(message) => return Ok(argparse_error(&message)),
    };
    if probe_mode()
        && let Some(profile) = &run.profile
        && let Some(out) = env::var_os(PROBE_OUT_VAR)
        && let Some(tool) = probe_tool(profile)
    {
        write_probe_output(tool, Path::new(&out))?;
    }
    // Invalid profile or case data content: LEAPP prints an error and exits 0 without output.
    let selected = match &run.profile {
        Some(path) => match load_profile(path, run.tool) {
            Ok(names) => select_modules(run.tool, Some(&names)),
            Err(message) => {
                stdout.line(&message);
                return Ok(0);
            }
        },
        None => select_modules(run.tool, None),
    };
    if let Some(path) = &run.case_data
        && let Err(message) = check_case_data(path)
    {
        stdout.line(&message);
        return Ok(0);
    }
    if config.scenario == Scenario::EarlyExit {
        stdout.line("Error: the input could not be opened (FAKE_LEAPP_SCENARIO=early_exit)");
        return Ok(0);
    }

    create_layout(&run.report_dir)?;
    let mut log = ScreenLog {
        report_dir: &run.report_dir,
        stdout,
    };
    let messages = progress_messages(run.tool, &selected, config.lines);
    let lines = match config.scenario {
        Scenario::Prompt | Scenario::InvalidInput => messages.len().min(3),
        Scenario::Crash => messages.len().div_ceil(2),
        _ => messages.len(),
    };
    log.messages(&messages[..lines], config.interval)?;

    let modules = match config.scenario {
        Scenario::Crash => {
            fs::write(run.report_dir.join("_lava_artifacts.db-journal"), b"")?;
            eprint!(
                "{}",
                traceback(
                    "scripts/ilapfuncs.py",
                    "RuntimeError: simulated crash (FAKE_LEAPP_SCENARIO=crash)"
                )
            );
            return Ok(1);
        }
        Scenario::Prompt => {
            let answer = read_password()?;
            let error = match answer {
                None => "EOFError: EOF when reading a line",
                Some(_) => "ValueError: the backup password is incorrect",
            };
            eprint!("{}", traceback("getpass.py", error));
            return Ok(1);
        }
        Scenario::InvalidInput => {
            log.message(&format!(
                "Error: {} is not a valid {} input",
                run.input.display(),
                run.input_type
            ))?;
            write_lava(&run, &[], None)?;
            return Ok(0);
        }
        Scenario::Slow | Scenario::IgnoreTerm => {
            let started = Instant::now();
            while started.elapsed() < SLOW_RUN {
                thread::sleep(Duration::from_secs(1));
                log.message(&format!(
                    "Still processing ({} s)",
                    started.elapsed().as_secs()
                ))?;
            }
            lava_modules(&run, &selected, 0)
        }
        Scenario::ArtifactError => {
            let modules = lava_modules(&run, &selected, 2);
            for module in modules.iter().filter(|m| m.status == STATUS_ERROR) {
                eprint!(
                    "{}",
                    traceback(
                        &format!("scripts/artifacts/{}.py", module.module_name),
                        "sqlite3.OperationalError: no such table: fake_table"
                    )
                );
            }
            modules
        }
        Scenario::Success => lava_modules(&run, &selected, 0),
        // Handled before any output was created (glibc_too_old by the bootloader).
        Scenario::EarlyExit | Scenario::ArgparseError | Scenario::GlibcTooOld => return Ok(0),
    };
    finish_report(&run, &modules)?;
    Ok(0)
}

fn argparse_error(message: &str) -> i32 {
    eprintln!("{USAGE}\nfake-leapp: error: {message}");
    2
}

fn traceback(file: &str, error: &str) -> String {
    format!(
        "Traceback (most recent call last):\n  File \"ileapp.py\", line 470, in <module>\n  \
         File \"ileapp.py\", line 402, in main\n  File \"{file}\", line 91, in fake\n{error}\n"
    )
}

/// Reads a password like Python's `getpass`: from `/dev/tty` if it can be opened (Unix), otherwise
/// from stdin after a warning. `None` means EOF.
fn read_password() -> io::Result<Option<String>> {
    #[cfg(unix)]
    if let Ok(mut tty) = OpenOptions::new().read(true).write(true).open("/dev/tty") {
        tty.write_all(b"Password: ")?;
        tty.flush()?;
        let mut answer = String::new();
        let read = io::BufReader::new(tty).read_line(&mut answer)?;
        return Ok((read > 0).then_some(answer));
    }
    eprint!(
        "GetPassWarning: Can not control echo on the terminal.\n\
         Warning: Password input may be echoed.\nPassword: "
    );
    let mut answer = String::new();
    let read = io::stdin().lock().read_line(&mut answer)?;
    Ok((read > 0).then_some(answer))
}

/// Appends `Screen_Output.html` records (one open/close per message, like LEAPP's `logfunc`) and
/// echoes them to the buffered stdout.
struct ScreenLog<'a> {
    report_dir: &'a Path,
    stdout: &'a mut BufferedStdout,
}

impl ScreenLog<'_> {
    fn message(&mut self, message: &str) -> io::Result<()> {
        append_screen_output(self.report_dir, message)?;
        self.stdout.line(message);
        Ok(())
    }

    fn messages(&mut self, messages: &[String], interval: Duration) -> io::Result<()> {
        for (i, message) in messages.iter().enumerate() {
            if i > 0 {
                thread::sleep(interval);
            }
            self.message(message)?;
        }
        Ok(())
    }
}

// ---- arguments ----

/// LEAPP's command line. The iTunes password's value is not kept (only whether one was given).
#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    input_type: Option<String>,
    output: Option<PathBuf>,
    input: Option<PathBuf>,
    timezone: Option<String>,
    profile: Option<PathBuf>,
    case_data: Option<PathBuf>,
    custom_output_folder: Option<OsString>,
    custom_artifacts_path: Option<PathBuf>,
    keychain: Option<PathBuf>,
    itunes_password_given: bool,
    wrap_text: bool,
    /// `-h`: print the usage and exit 0.
    help: bool,
}

/// Parses LEAPP's flags like argparse does (space-separated values; `-p` and `-c` are not
/// supported by the fake). An `Err` is the argparse error message.
fn parse_args(args: &[OsString]) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let Some(flag) = arg.to_str() else {
            return Err(format!("unrecognized arguments: {}", arg.to_string_lossy()));
        };
        let mut value = || {
            rest.next()
                .cloned()
                .ok_or_else(|| format!("argument {flag}: expected one argument"))
        };
        match flag {
            "-h" | "--help" => {
                return Ok(Args {
                    help: true,
                    ..Args::default()
                });
            }
            "-t" => {
                let value = value()?;
                let text = value.to_str().filter(|text| INPUT_TYPES.contains(text));
                let Some(text) = text else {
                    return Err(format!(
                        "argument -t: invalid choice: {:?} (choose from {})",
                        value.to_string_lossy(),
                        INPUT_TYPES.join(", ")
                    ));
                };
                parsed.input_type = Some(text.to_owned());
            }
            "-o" | "--output_path" => parsed.output = Some(value()?.into()),
            "-i" | "--input_path" => parsed.input = Some(value()?.into()),
            "-tz" | "--timezone" => parsed.timezone = Some(value()?.to_string_lossy().into_owned()),
            "-m" | "--load_profile" => parsed.profile = Some(value()?.into()),
            "-d" | "--load_case_data" => parsed.case_data = Some(value()?.into()),
            "--custom_output_folder" => parsed.custom_output_folder = Some(value()?),
            "--custom_artifacts_path" => parsed.custom_artifacts_path = Some(value()?.into()),
            "--keychain" => parsed.keychain = Some(value()?.into()),
            "--itunes_password" => {
                value()?;
                parsed.itunes_password_given = true;
            }
            "-w" | "--wrap_text" => parsed.wrap_text = true,
            "-p" | "--artifact_paths" | "-c" | "--create_profile_casedata" => {
                return Err(format!("argument {flag}: not supported by fake-leapp"));
            }
            _ => return Err(format!("unrecognized arguments: {flag}")),
        }
    }
    Ok(parsed)
}

/// A validated run.
#[derive(Debug)]
struct Run {
    tool: ToolId,
    input_type: String,
    input: PathBuf,
    output: PathBuf,
    report_dir: PathBuf,
    profile: Option<PathBuf>,
    case_data: Option<PathBuf>,
}

/// LEAPP's own checks that end in an argparse error (exit 2) before anything is written.
fn validate(args: Args) -> Result<Run, String> {
    let input_type = args.input_type.ok_or("No type provided (-t)")?;
    let output = args.output.ok_or("No OUTPUT folder provided (-o)")?;
    let input = args.input.ok_or("No INPUT provided (-i)")?;
    if !output.is_dir() {
        return Err("OUTPUT folder does not exist! Run the program again.".to_owned());
    }
    if !input.exists() {
        return Err("INPUT file/folder does not exist! Run the program again.".to_owned());
    }
    if input_type == "fs" {
        let empty = fs::read_dir(&input)
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(true);
        if empty {
            return Err("INPUT folder is empty or not a folder".to_owned());
        }
    }
    for (flag, file) in [("-m", &args.profile), ("-d", &args.case_data)] {
        if let Some(file) = file
            && !file.is_file()
        {
            return Err(format!(
                "argument {flag}: {} does not exist",
                file.display()
            ));
        }
    }
    let tool = match &args.timezone {
        Some(zone) if !TIMEZONES.contains(&zone.as_str()) => {
            return Err(format!("argument -tz: unknown timezone {zone:?}"));
        }
        Some(_) => ToolId::Ileapp,
        None => ToolId::Aleapp,
    };
    let report_dir = match &args.custom_output_folder {
        Some(name) => {
            let mut components = Path::new(name).components();
            let single = matches!(
                (components.next(), components.next()),
                (Some(Component::Normal(_)), None)
            );
            if !single {
                return Err(format!(
                    "argument --custom_output_folder: {:?} is not a valid folder name",
                    name.to_string_lossy()
                ));
            }
            let dir = output.join(name);
            if dir.exists() {
                return Err(format!(
                    "argument --custom_output_folder: {} already exists",
                    dir.display()
                ));
            }
            dir
        }
        None => output.join(default_report_folder(tool, SystemTime::now())),
    };
    Ok(Run {
        tool,
        input_type,
        input,
        output,
        report_dir,
        profile: args.profile,
        case_data: args.case_data,
    })
}

/// `iLEAPP_Reports_2026-09-24_Thursday_183005` (UTC; LEAPP uses local time).
fn default_report_folder(tool: ToolId, now: SystemTime) -> String {
    let seconds = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let time = i64::try_from(seconds)
        .ok()
        .and_then(|s| time::OffsetDateTime::from_unix_timestamp(s).ok())
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    format!(
        "{}_Reports_{:04}-{:02}-{:02}_{}_{:02}{:02}{:02}",
        display_name(tool),
        time.year(),
        u8::from(time.month()),
        time.day(),
        time.weekday(),
        time.hour(),
        time.minute(),
        time.second()
    )
}

fn display_name(tool: ToolId) -> &'static str {
    match tool {
        ToolId::Ileapp => "iLEAPP",
        ToolId::Aleapp => "ALEAPP",
    }
}

/// The plugin names of a LEAPP profile, or the message LEAPP prints for invalid content.
fn load_profile(path: &Path, tool: ToolId) -> Result<Vec<String>, String> {
    let invalid = || {
        format!(
            "{} is not a valid {} profile",
            path.display(),
            display_name(tool)
        )
    };
    let value: Value = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(invalid)?;
    if value["leapp"].as_str() != Some(tool.as_str()) {
        return Err(invalid());
    }
    let plugins = value["plugins"].as_array().ok_or_else(invalid)?;
    plugins
        .iter()
        .map(|plugin| plugin.as_str().map(str::to_owned).ok_or_else(invalid))
        .collect()
}

/// Checks the `.lcasedata` content (CONTRACTS.md §8).
fn check_case_data(path: &Path) -> Result<(), String> {
    let value: Option<Value> = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    match value {
        Some(value)
            if value["leapp"].as_str() == Some("case_data")
                && value["case_data_values"].is_object() =>
        {
            Ok(())
        }
        _ => Err(format!("{} is not a valid case data file", path.display())),
    }
}

// ---- the fake module catalog ----

struct FakeModule {
    name: &'static str,
    module_name: &'static str,
    category: &'static str,
    display_name: &'static str,
}

const fn module(
    name: &'static str,
    module_name: &'static str,
    category: &'static str,
    display_name: &'static str,
) -> FakeModule {
    FakeModule {
        name,
        module_name,
        category,
        display_name,
    }
}

const ILEAPP_MODULES: &[FakeModule] = &[
    module("callHistory", "callHistory", "Call History", "Call History"),
    module("sms", "sms", "Messages", "SMS & iMessage Messages"),
    module(
        "safariHistory",
        "safari",
        "Safari Browser",
        "Safari History",
    ),
    module(
        "safariBookmarks",
        "safari",
        "Safari Browser",
        "Safari Bookmarks",
    ),
    module(
        "photosMetadata",
        "photosMetadata",
        "Photos",
        "Photos Metadata",
    ),
    module(
        "applicationState",
        "applicationstate",
        "Installed Apps",
        "Application State",
    ),
    module(
        "knownNetworks",
        "wifiKnownNetworks",
        "WiFi",
        "Known Networks",
    ),
    module("accounts", "accounts", "Accounts", "Accounts"),
];
const ILEAPP_ALWAYS_RUN_DEFAULT: &[FakeModule] = &[module(
    "last_build",
    "lastBuild",
    "IOS Build",
    "iOS Build (Last Build)",
)];
const ILEAPP_ALWAYS_RUN_ITUNES: &[FakeModule] = &[
    module(
        "itunes_backup_info",
        "iTunesBackupInfo",
        "iTunes Backup",
        "iTunes Backup Information",
    ),
    module(
        "itunes_backup_installed_applications",
        "iTunesBackupInfo",
        "Installed Apps",
        "iTunes Backup Installed Applications",
    ),
];
const ALEAPP_MODULES: &[FakeModule] = &[
    module("chromeHistory", "chrome", "Browser", "Chrome History"),
    module("smsMms", "smsmms", "SMS", "SMS & MMS"),
    module("callLogs", "callLogs", "Call Logs", "Call Logs"),
    module(
        "installedApps",
        "installedAppsGass",
        "Installed Apps",
        "Installed Apps (GASS)",
    ),
    module(
        "wifiProfiles",
        "wifiProfiles",
        "WiFi Profiles",
        "WiFi Profiles",
    ),
    module("accountsDe", "accounts_de", "Accounts", "Accounts (DE)"),
];
const ALEAPP_ALWAYS_RUN: &[FakeModule] = &[module(
    "usagestats_version",
    "usagestatsVersion",
    "Usage Stats",
    "Android Usage Stats Version",
)];

fn catalog(tool: ToolId) -> &'static [FakeModule] {
    match tool {
        ToolId::Ileapp => ILEAPP_MODULES,
        ToolId::Aleapp => ALEAPP_MODULES,
    }
}

/// The always-run artifacts for an input type (LEAPP-CLI.md §5).
fn always_run(tool: ToolId, input_type: &str) -> &'static [FakeModule] {
    match tool {
        ToolId::Ileapp if input_type == "itunes" => ILEAPP_ALWAYS_RUN_ITUNES,
        ToolId::Ileapp => ILEAPP_ALWAYS_RUN_DEFAULT,
        ToolId::Aleapp => ALEAPP_ALWAYS_RUN,
    }
}

/// The selected catalog modules in catalog order. Unknown profile names are dropped silently, as
/// LEAPP does (LEAPP-CLI.md Q4).
fn select_modules(tool: ToolId, profile: Option<&[String]>) -> Vec<&'static FakeModule> {
    catalog(tool)
        .iter()
        .filter(|m| profile.is_none_or(|names| names.iter().any(|n| n == m.name)))
        .collect()
}

// ---- the introspection probe (copies named `…-probe`) ----

/// The variable naming the probe's output file (LEAPP-CLI.md §5).
const PROBE_OUT_VAR: &str = "SUITEDFIR_PROBE_OUT";
/// The probe artifact's name, which the introspection profile selects.
const PROBE_NAME: &str = "suitedfir_probe";
/// Filler plugins, so the probe lists as many modules as a real build (introspection wants 500).
const PROBE_FILLERS: u32 = 500;

/// Whether this copy answers the introspection probe: its file name ends in `-probe`
/// (`fake-leapp-probe`, `fake-leapp-probe.exe`). Plain `fake-leapp` never does, so introspection
/// against it fails, as the introspection tests expect.
fn probe_mode() -> bool {
    env::current_exe().ok().is_some_and(|exe| {
        exe.file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem.ends_with("-probe"))
    })
}

/// The tool of an introspection profile: one that selects the probe; `leapp` names the tool.
fn probe_tool(profile: &Path) -> Option<ToolId> {
    let value: Value = serde_json::from_slice(&fs::read(profile).ok()?).ok()?;
    let selects_probe = value["plugins"]
        .as_array()?
        .iter()
        .any(|plugin| plugin.as_str() == Some(PROBE_NAME));
    match value["leapp"].as_str()? {
        "ileapp" if selects_probe => Some(ToolId::Ileapp),
        "aleapp" if selects_probe => Some(ToolId::Aleapp),
        _ => None,
    }
}

/// What the real probe writes: every built-in plugin (the always-run ones, the catalog and the
/// fillers) and, for iLEAPP, the timezones; through a temporary name, then renamed.
fn write_probe_output(tool: ToolId, out: &Path) -> io::Result<()> {
    let always: Vec<&FakeModule> = match tool {
        ToolId::Ileapp => ILEAPP_ALWAYS_RUN_DEFAULT
            .iter()
            .chain(ILEAPP_ALWAYS_RUN_ITUNES)
            .collect(),
        ToolId::Aleapp => ALEAPP_ALWAYS_RUN.iter().collect(),
    };
    let mut plugins: Vec<Value> = always
        .into_iter()
        .chain(catalog(tool))
        .map(|m| {
            json!({"name": m.name, "module_name": m.module_name, "category": m.category,
                   "display_name": m.display_name, "description": null})
        })
        .collect();
    plugins.extend((1..=PROBE_FILLERS).map(|i| {
        json!({"name": format!("fakeFiller{i:03}"), "module_name": "fakeFiller",
               "category": "Filler", "display_name": format!("Filler module {i:03}"),
               "description": "A filler that makes the list as long as a real build's"})
    }));
    let timezones = match tool {
        ToolId::Ileapp => json!(TIMEZONES),
        ToolId::Aleapp => Value::Null,
    };
    let text = serde_json::to_string(&json!({"plugins": plugins, "timezones": timezones}))
        .map_err(io::Error::other)?;
    let mut partial = out.as_os_str().to_owned();
    partial.push(".partial");
    fs::write(&partial, text)?;
    fs::rename(&partial, out)
}

/// `--list-modules-json <tool>`: the catalog as a `ToolModules` object.
fn list_modules_json(args: &[OsString]) -> i32 {
    let tool = match args.first().and_then(|arg| arg.to_str()) {
        Some("ileapp") if args.len() == 1 => ToolId::Ileapp,
        Some("aleapp") if args.len() == 1 => ToolId::Aleapp,
        _ => {
            eprintln!("usage: fake-leapp --list-modules-json <ileapp|aleapp>");
            return 2;
        }
    };
    match serde_json::to_string_pretty(&tool_modules(tool)) {
        Ok(text) => {
            println!("{text}");
            0
        }
        Err(e) => {
            eprintln!("fake-leapp: {e}");
            1
        }
    }
}

fn tool_modules(tool: ToolId) -> ToolModules {
    let names = |modules: &[FakeModule]| modules.iter().map(|m| m.name.to_owned()).collect();
    let mut always = BTreeMap::new();
    match tool {
        ToolId::Ileapp => {
            always.insert("default".to_owned(), names(ILEAPP_ALWAYS_RUN_DEFAULT));
            always.insert("itunes".to_owned(), names(ILEAPP_ALWAYS_RUN_ITUNES));
        }
        ToolId::Aleapp => {
            always.insert("default".to_owned(), names(ALEAPP_ALWAYS_RUN));
        }
    }
    let mut modules: Vec<ModuleInfo> = catalog(tool)
        .iter()
        .map(|m| ModuleInfo {
            name: m.name.to_owned(),
            module_name: m.module_name.to_owned(),
            category: m.category.to_owned(),
            display_name: m.display_name.to_owned(),
            description: None,
        })
        .collect();
    // CONTRACTS.md §5: by category, then display name, case-insensitively.
    modules.sort_by_key(|m| (m.category.to_lowercase(), m.display_name.to_lowercase()));
    ToolModules {
        tool,
        version: "dev-override".to_owned(),
        always_run: always,
        timezones: match tool {
            ToolId::Ileapp => Some(TIMEZONES.iter().map(|z| (*z).to_owned()).collect()),
            ToolId::Aleapp => None,
        },
        modules,
    }
}

// ---- output layout ----

const STATUS_COMPLETE: &str = "Complete";
const STATUS_ERROR: &str = "Error";
const STATUS_NO_FILES: &str = "No files found";

fn screen_output_path(report_dir: &Path) -> PathBuf {
    report_dir
        .join("_HTML")
        .join("_Script_Logs")
        .join("Screen_Output.html")
}

/// The folders LEAPP creates when it starts, plus its artifact database.
fn create_layout(report_dir: &Path) -> io::Result<()> {
    fs::create_dir_all(report_dir.join("_HTML").join("_Script_Logs"))?;
    for dir in ["_TSV Exports", "data", "media"] {
        fs::create_dir_all(report_dir.join(dir))?;
    }
    fs::write(report_dir.join("_lava_artifacts.db"), b"SQLite format 3\0")
}

fn append_screen_output(report_dir: &Path, message: &str) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(screen_output_path(report_dir))?;
    file.write_all(format!("{message}<br>{NL}").as_bytes())
}

/// Exactly `lines` progress messages. The second one carries markup, as some LEAPP messages do.
fn progress_messages(tool: ToolId, modules: &[&FakeModule], lines: u32) -> Vec<String> {
    (0..lines)
        .map(|i| match i {
            0 => "Processing started. Please wait. This may take a few minutes...".to_owned(),
            1 => format!(
                "<b>{} v0.0.0-fake</b> (fake-leapp test double)",
                display_name(tool)
            ),
            _ => match modules.get(usize::try_from(i - 2).unwrap_or(0) % modules.len().max(1)) {
                Some(m) => format!(
                    "[{i}/{lines}] {} [{}] artifact executed",
                    m.module_name, m.name
                ),
                None => format!("[{i}/{lines}] no artifacts selected"),
            },
        })
        .collect()
}

/// One entry of `_lava_data.lava` `modules`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LavaModule {
    module_name: &'static str,
    artifact_name: &'static str,
    status: &'static str,
    file_count: u32,
}

/// Always-run entries (complete), then the selected modules: every third has no files, and the
/// first `errors` selected modules fail.
fn lava_modules(run: &Run, selected: &[&FakeModule], errors: usize) -> Vec<LavaModule> {
    let always = always_run(run.tool, &run.input_type)
        .iter()
        .map(|m| LavaModule {
            module_name: m.module_name,
            artifact_name: m.name,
            status: STATUS_COMPLETE,
            file_count: 1,
        });
    let selected = selected.iter().enumerate().map(|(i, m)| {
        let (status, file_count) = if i < errors {
            (STATUS_ERROR, 0)
        } else if i % 3 == 2 {
            (STATUS_NO_FILES, 0)
        } else {
            (STATUS_COMPLETE, u32::try_from(i % 5 + 1).unwrap_or(1))
        };
        LavaModule {
            module_name: m.module_name,
            artifact_name: m.name,
            status,
            file_count,
        }
    });
    always.chain(selected).collect()
}

fn lava_json(run: &Run, modules: &[LavaModule], start_timestamp: u64) -> Value {
    json!({
        "lava_schema_version": "1",
        "parser_info": {
            "leapp_name": display_name(run.tool),
            "leapp_version": "0.0.0-fake",
            "package": "Binary",
            "start_timestamp": start_timestamp,
        },
        "param_input": run.input.to_string_lossy(),
        "param_output": run.output.to_string_lossy(),
        "param_type": run.input_type,
        "param_profile": run.profile.as_ref().map(|p| p.to_string_lossy()),
        "processing_status": STATUS_COMPLETE,
        "modules": modules.iter().map(|m| json!({
            "module_name": m.module_name,
            "module_status": m.status,
            "artifact_name": m.artifact_name,
            "file_count": m.file_count,
        })).collect::<Vec<_>>(),
    })
}

fn write_lava(run: &Run, modules: &[LavaModule], now: Option<SystemTime>) -> io::Result<()> {
    let start = now
        .unwrap_or_else(SystemTime::now)
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let text =
        serde_json::to_string_pretty(&lava_json(run, modules, start)).map_err(io::Error::other)?;
    fs::write(run.report_dir.join("_lava_data.lava"), text)
}

/// The report LEAPP writes when it finishes: per-artifact HTML and TSV files, `index.html` and,
/// last, `_lava_data.lava`.
fn finish_report(run: &Run, modules: &[LavaModule]) -> io::Result<()> {
    let mut index =
        String::from("<!DOCTYPE html>\n<html><head><title>Report</title></head><body>\n");
    let mut seen = BTreeSet::new();
    for module in modules.iter().filter(|m| m.status == STATUS_COMPLETE) {
        if !seen.insert(module.artifact_name) {
            continue;
        }
        let name = module.artifact_name;
        fs::write(
            run.report_dir.join("_HTML").join(format!("{name}.html")),
            format!("<html><body><h1>{name}</h1></body></html>\n"),
        )?;
        fs::write(
            run.report_dir
                .join("_TSV Exports")
                .join(format!("{name}.tsv")),
            format!("artifact\tfile_count\n{name}\t{}\n", module.file_count),
        )?;
        index.push_str(&format!("<a href=\"_HTML/{name}.html\">{name}</a><br>\n"));
    }
    index.push_str("</body></html>\n");
    fs::write(run.report_dir.join("index.html"), index)?;
    write_lava(run, modules, None)
}

// ---- Unix signal handling ----

/// The only `unsafe` in this binary: libc calls for SIGTERM handling. This file is a test double
/// and is never bundled (ARCHITECTURE.md §5.1).
#[cfg(unix)]
mod signals {
    use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

    static WORKER_PID: AtomicI32 = AtomicI32::new(0);
    static TERM_RECEIVED: AtomicBool = AtomicBool::new(false);

    /// The bootloader's SIGTERM handler: forwards the signal to the worker. It only touches atomics
    /// and calls `kill`, which is async-signal-safe.
    extern "C" fn forward_term(_signal: libc::c_int) {
        TERM_RECEIVED.store(true, Ordering::SeqCst);
        let worker = WORKER_PID.load(Ordering::SeqCst);
        if worker > 0 {
            send(worker, libc::SIGTERM);
        }
    }

    fn send(pid: libc::pid_t, signal: libc::c_int) {
        // SAFETY: FFI call with plain integer arguments; kill(2) is async-signal-safe, so it may
        // run inside the signal handler. A failure (the worker is already gone) is harmless.
        unsafe {
            libc::kill(pid, signal);
        }
    }

    fn set_disposition(signal: libc::c_int, handler: libc::sighandler_t) {
        // SAFETY: FFI call. `handler` is SIG_IGN, SIG_DFL or `forward_term`, an `extern "C"`
        // function with the signature signal(3) expects that only does async-signal-safe work.
        unsafe {
            libc::signal(signal, handler);
        }
    }

    /// Forward SIGTERM to the worker, or ignore it (`ignore_term`; the worker inherits SIG_IGN).
    pub fn install_bootloader(ignore: bool) {
        let handler = if ignore {
            libc::SIG_IGN
        } else {
            forward_term as extern "C" fn(libc::c_int) as libc::sighandler_t
        };
        set_disposition(libc::SIGTERM, handler);
    }

    pub fn ignore_term() {
        set_disposition(libc::SIGTERM, libc::SIG_IGN);
    }

    /// Records the worker for forwarding. A SIGTERM that arrived before the worker existed is
    /// forwarded now.
    pub fn set_worker(pid: u32) {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return;
        };
        WORKER_PID.store(pid, Ordering::SeqCst);
        if TERM_RECEIVED.load(Ordering::SeqCst) {
            send(pid, libc::SIGTERM);
        }
    }

    /// Dies from `signal` like the worker did. Returns the shell-style code only if that failed.
    pub fn reraise(signal: libc::c_int) -> i32 {
        set_disposition(signal, libc::SIG_DFL);
        send(std::process::id().cast_signed(), signal);
        128 + signal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    fn run_args(args: &[&str]) -> Args {
        let args = parse_args(&os(args)).unwrap();
        assert!(!args.help);
        args
    }

    /// A valid `-t fs` command line: an input dir with one file and an output dir.
    fn fixture(dir: &Path) -> (PathBuf, PathBuf) {
        let input = dir.join("input");
        let output = dir.join("out");
        fs::create_dir_all(&input).unwrap();
        fs::create_dir_all(&output).unwrap();
        fs::write(input.join("evidence.txt"), "x").unwrap();
        (input, output)
    }

    fn fs_args(input: &Path, output: &Path, extra: &[&str]) -> Args {
        let mut args = vec![
            "-t",
            "fs",
            "-i",
            input.to_str().unwrap(),
            "-o",
            output.to_str().unwrap(),
            "--custom_output_folder",
            "report",
        ];
        args.extend_from_slice(extra);
        run_args(&args)
    }

    #[test]
    fn parses_the_suitedfir_invocation() {
        let args = run_args(&[
            "-t",
            "itunes",
            "-i",
            "/in",
            "-o",
            "/out",
            "--custom_output_folder",
            "report",
            "-d",
            "/out/case.lcasedata",
            "-m",
            "/out/profile.ilprofile",
            "-tz",
            "America/Chicago",
            "--itunes_password",
            "secret",
            "--keychain",
            "/k.plist",
        ]);
        assert_eq!(
            args,
            Args {
                input_type: Some("itunes".to_owned()),
                output: Some("/out".into()),
                input: Some("/in".into()),
                timezone: Some("America/Chicago".to_owned()),
                profile: Some("/out/profile.ilprofile".into()),
                case_data: Some("/out/case.lcasedata".into()),
                custom_output_folder: Some("report".into()),
                custom_artifacts_path: None,
                keychain: Some("/k.plist".into()),
                itunes_password_given: true,
                wrap_text: false,
                help: false,
            }
        );
        // The password value is not kept anywhere.
        assert!(!format!("{args:?}").contains("secret"));
    }

    #[test]
    fn long_flag_names_and_help() {
        let args = run_args(&[
            "--output_path",
            "/o",
            "--input_path",
            "/i",
            "--timezone",
            "UTC",
            "--load_profile",
            "/p",
            "--load_case_data",
            "/d",
            "--custom_artifacts_path",
            "/a",
            "-w",
        ]);
        assert_eq!(args.output, Some("/o".into()));
        assert_eq!(args.input, Some("/i".into()));
        assert_eq!(args.timezone.as_deref(), Some("UTC"));
        assert_eq!(args.custom_artifacts_path, Some("/a".into()));
        assert!(args.wrap_text);
        assert!(parse_args(&os(&["-t", "fs", "-h"])).unwrap().help);
    }

    #[test]
    fn argparse_errors() {
        for (args, expected) in [
            (&["-t", "dmg"][..], "invalid choice"),
            (&["-t"][..], "expected one argument"),
            (&["--bogus"][..], "unrecognized arguments: --bogus"),
            (&["-p"][..], "not supported"),
            (&["-c"][..], "not supported"),
        ] {
            let error = parse_args(&os(args)).unwrap_err();
            assert!(error.contains(expected), "{args:?}: {error}");
        }
    }

    #[test]
    fn validation_mirrors_leapp_exit_2_cases() {
        let dir = tempfile::tempdir().unwrap();
        let (input, output) = fixture(dir.path());

        let run = validate(fs_args(&input, &output, &["-tz", "UTC"])).unwrap();
        assert_eq!(run.tool, ToolId::Ileapp);
        assert_eq!(run.report_dir, output.join("report"));
        let run = validate(fs_args(&input, &output, &[])).unwrap();
        assert_eq!(run.tool, ToolId::Aleapp);

        let missing = dir.path().join("missing");
        let empty = dir.path().join("empty");
        fs::create_dir(&empty).unwrap();
        fs::create_dir(output.join("taken")).unwrap();
        let cases: Vec<(Args, &str)> = vec![
            (
                fs_args(&input, &missing, &[]),
                "OUTPUT folder does not exist",
            ),
            (
                fs_args(&missing, &output, &[]),
                "INPUT file/folder does not exist",
            ),
            (fs_args(&empty, &output, &[]), "empty"),
            (
                fs_args(&input, &output, &["-m", missing.to_str().unwrap()]),
                "argument -m",
            ),
            (
                fs_args(&input, &output, &["-d", missing.to_str().unwrap()]),
                "argument -d",
            ),
            (
                fs_args(&input, &output, &["-tz", "Mars/Olympus"]),
                "unknown timezone",
            ),
            (
                run_args(&[
                    "-t",
                    "fs",
                    "-i",
                    input.to_str().unwrap(),
                    "-o",
                    output.to_str().unwrap(),
                    "--custom_output_folder",
                    "taken",
                ]),
                "already exists",
            ),
            (
                run_args(&[
                    "-t",
                    "fs",
                    "-i",
                    input.to_str().unwrap(),
                    "-o",
                    output.to_str().unwrap(),
                    "--custom_output_folder",
                    "a/b",
                ]),
                "not a valid folder name",
            ),
            (run_args(&["-i", "/x", "-o", "/y"]), "No type"),
            (run_args(&["-t", "fs", "-i", "/x"]), "No OUTPUT"),
            (run_args(&["-t", "fs", "-o", "/y"]), "No INPUT"),
        ];
        for (args, expected) in cases {
            let error = validate(args).unwrap_err();
            assert!(error.contains(expected), "{expected}: {error}");
        }
    }

    #[test]
    fn default_report_folder_name() {
        let at = UNIX_EPOCH + Duration::from_secs(1_790_274_605); // 2026-09-24T18:30:05Z
        assert_eq!(
            default_report_folder(ToolId::Ileapp, at),
            "iLEAPP_Reports_2026-09-24_Thursday_183005"
        );
    }

    #[test]
    fn config_defaults_and_errors() {
        let config = Config::from_vars(|_| None).unwrap();
        assert_eq!(
            config,
            Config {
                scenario: Scenario::Success,
                interval: Duration::from_millis(100),
                lines: 50,
                pidfile: None,
            }
        );
        let config = Config::from_vars(|name| {
            match name {
                "FAKE_LEAPP_SCENARIO" => Some("ignore_term"),
                "FAKE_LEAPP_INTERVAL_MS" => Some("5"),
                "FAKE_LEAPP_LINES" => Some("7"),
                "FAKE_LEAPP_PIDFILE" => Some("/tmp/pids"),
                _ => None,
            }
            .map(OsString::from)
        })
        .unwrap();
        assert_eq!(config.scenario, Scenario::IgnoreTerm);
        assert_eq!(config.interval, Duration::from_millis(5));
        assert_eq!(config.lines, 7);
        assert_eq!(config.pidfile, Some(PathBuf::from("/tmp/pids")));
        for (name, value) in [
            ("FAKE_LEAPP_SCENARIO", "explode"),
            ("FAKE_LEAPP_LINES", "-1"),
            ("FAKE_LEAPP_INTERVAL_MS", "soon"),
        ] {
            let result = Config::from_vars(|var| (var == name).then(|| OsString::from(value)));
            assert!(result.is_err(), "{name}={value}");
        }
    }

    #[test]
    fn profiles_and_case_data() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("p.ilprofile");
        fs::write(
            &profile,
            r#"{"leapp": "ileapp", "format_version": 1, "plugins": ["sms", "noSuchModule", "callHistory"]}"#,
        )
        .unwrap();
        let names = load_profile(&profile, ToolId::Ileapp).unwrap();
        let selected: Vec<_> = select_modules(ToolId::Ileapp, Some(&names))
            .iter()
            .map(|m| m.name)
            .collect();
        // Unknown names are dropped silently; catalog order.
        assert_eq!(selected, ["callHistory", "sms"]);
        assert!(load_profile(&profile, ToolId::Aleapp).is_err());
        fs::write(&profile, "not json").unwrap();
        assert!(load_profile(&profile, ToolId::Ileapp).is_err());
        assert_eq!(
            select_modules(ToolId::Aleapp, None).len(),
            ALEAPP_MODULES.len()
        );

        let case_data = dir.path().join("case.lcasedata");
        fs::write(
            &case_data,
            r#"{"leapp": "case_data", "case_data_values": {"Case Number": "1"}}"#,
        )
        .unwrap();
        assert!(check_case_data(&case_data).is_ok());
        fs::write(&case_data, r#"{"leapp": "ileapp"}"#).unwrap();
        assert!(check_case_data(&case_data).is_err());
    }

    #[test]
    fn list_modules_json_is_a_tool_modules_object() {
        for tool in [ToolId::Ileapp, ToolId::Aleapp] {
            let text = serde_json::to_string(&tool_modules(tool)).unwrap();
            let parsed: ToolModules = serde_json::from_str(&text).unwrap();
            assert_eq!(parsed.tool, tool);
            assert_eq!(parsed.version, "dev-override");
            assert_eq!(parsed.modules.len(), catalog(tool).len());
            let keys: Vec<_> = parsed
                .modules
                .iter()
                .map(|m| (m.category.to_lowercase(), m.display_name.to_lowercase()))
                .collect();
            let mut sorted = keys.clone();
            sorted.sort();
            assert_eq!(keys, sorted);
            // Always-run artifacts are not selectable.
            for names in parsed.always_run.values() {
                for name in names {
                    assert!(parsed.modules.iter().all(|m| &m.name != name));
                }
            }
            assert_eq!(parsed.timezones.is_some(), tool == ToolId::Ileapp);
        }
    }

    fn sample_run(dir: &Path, tool: ToolId, input_type: &str) -> Run {
        Run {
            tool,
            input_type: input_type.to_owned(),
            input: dir.join("input"),
            output: dir.to_path_buf(),
            report_dir: dir.join("report"),
            profile: None,
            case_data: None,
        }
    }

    #[test]
    fn lava_modules_per_scenario() {
        let dir = tempfile::tempdir().unwrap();
        let run = sample_run(dir.path(), ToolId::Ileapp, "itunes");
        let selected = select_modules(ToolId::Ileapp, None);

        let success = lava_modules(&run, &selected, 0);
        assert_eq!(success.len(), 2 + ILEAPP_MODULES.len());
        assert_eq!(success[0].artifact_name, "itunes_backup_info");
        assert_eq!(
            success[1].artifact_name,
            "itunes_backup_installed_applications"
        );
        assert!(success.iter().all(|m| m.status != STATUS_ERROR));
        assert!(success.iter().any(|m| m.status == STATUS_NO_FILES));

        let errors = lava_modules(&run, &selected, 2);
        let failed: Vec<_> = errors
            .iter()
            .filter(|m| m.status == STATUS_ERROR)
            .map(|m| m.artifact_name)
            .collect();
        assert_eq!(failed, ["callHistory", "sms"]);

        let aleapp = sample_run(dir.path(), ToolId::Aleapp, "fs");
        assert_eq!(
            lava_modules(&aleapp, &[], 0)[0].module_name,
            "usagestatsVersion"
        );
        let fs_run = sample_run(dir.path(), ToolId::Ileapp, "fs");
        assert_eq!(lava_modules(&fs_run, &[], 0)[0].artifact_name, "last_build");
    }

    #[test]
    fn progress_messages_are_exactly_the_requested_lines() {
        let selected = select_modules(ToolId::Ileapp, None);
        let messages = progress_messages(ToolId::Ileapp, &selected, 50);
        assert_eq!(messages.len(), 50);
        assert!(messages[1].starts_with("<b>iLEAPP"));
        assert!(messages[2].contains("[callHistory]"));
        assert!(messages[3].contains("[sms]"));
        assert!(progress_messages(ToolId::Aleapp, &[], 0).is_empty());
        assert_eq!(
            progress_messages(ToolId::Aleapp, &[], 3)[2],
            "[2/3] no artifacts selected"
        );
    }

    #[test]
    fn writes_the_leapp_output_layout() {
        let dir = tempfile::tempdir().unwrap();
        let run = sample_run(dir.path(), ToolId::Ileapp, "fs");
        create_layout(&run.report_dir).unwrap();
        append_screen_output(&run.report_dir, "one").unwrap();
        append_screen_output(&run.report_dir, "<i>two</i>").unwrap();
        let modules = lava_modules(&run, &select_modules(ToolId::Ileapp, None), 0);
        finish_report(&run, &modules).unwrap();

        let report = &run.report_dir;
        for dir in ["_HTML/_Script_Logs", "_TSV Exports", "data", "media"] {
            assert!(report.join(dir).is_dir(), "{dir}");
        }
        for file in [
            "index.html",
            "_lava_artifacts.db",
            "_lava_data.lava",
            "_HTML/callHistory.html",
            "_TSV Exports/callHistory.tsv",
        ] {
            assert!(report.join(file).is_file(), "{file}");
        }
        assert_eq!(
            fs::read_to_string(screen_output_path(report)).unwrap(),
            format!("one<br>{NL}<i>two</i><br>{NL}")
        );

        let lava: Value =
            serde_json::from_slice(&fs::read(report.join("_lava_data.lava")).unwrap()).unwrap();
        assert_eq!(lava["processing_status"], "Complete");
        assert_eq!(lava["param_type"], "fs");
        assert_eq!(lava["parser_info"]["leapp_name"], "iLEAPP");
        let entries = lava["modules"].as_array().unwrap();
        assert_eq!(entries.len(), modules.len());
        assert_eq!(entries[0]["artifact_name"], "last_build");
        assert_eq!(entries[1]["module_status"], "Complete");
    }

    #[test]
    fn invalid_input_lava_has_no_modules() {
        let dir = tempfile::tempdir().unwrap();
        let run = sample_run(dir.path(), ToolId::Aleapp, "fs");
        create_layout(&run.report_dir).unwrap();
        write_lava(&run, &[], Some(UNIX_EPOCH)).unwrap();
        let lava: Value =
            serde_json::from_slice(&fs::read(run.report_dir.join("_lava_data.lava")).unwrap())
                .unwrap();
        assert_eq!(lava["modules"], json!([]));
        assert_eq!(lava["parser_info"]["start_timestamp"], 0);
        assert!(!run.report_dir.join("index.html").exists());
    }

    #[test]
    fn pidfile_format() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pids");
        write_pidfile(&path, 10, 11).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "parent 10\nworker 11\n");
        assert!(!dir.path().join("pids.tmp").exists());
    }

    #[test]
    fn tracebacks_look_like_python() {
        let text = traceback("x.py", "EOFError: EOF when reading a line");
        assert!(text.starts_with("Traceback (most recent call last):\n"));
        assert!(text.ends_with("EOFError: EOF when reading a line\n"));
    }
}
