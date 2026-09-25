//! fake-idevice: a test double of the four libimobiledevice tools that suiteDFIR runs
//! (`idevice_id`, `ideviceinfo`, `idevicepair`, `idevicebackup2`; DEVELOPMENT.md §4.8). Never
//! bundled.
//!
//! **Selection.** The tool is the file stem of argv[0] (tests copy this binary to the tool names,
//! never symlink) or, failing that, the first argument (`fake-idevice idevicebackup2 …`, which is how
//! the debug-build dev override runs it).
//!
//! **One fake device.** Its state (connected, host pair record, `WillEncrypt`, the backup password)
//! is kept as `state.json` in `FAKE_IDEVICE_STATE_DIR`, so invocations see each other's effects.
//! Without that variable every invocation starts from the scenario's initial state. Every
//! invocation is appended to `calls`, and every attempt to pair to `pairing_attempts`, so tests can
//! prove what was (and was not) run.
//!
//! **Output** follows docs/IDEVICE-CLI.md, with the message strings of libimobiledevice 1.4.0:
//! UDID lines from `idevice_id -l`, XML plists from `ideviceinfo -x`, `idevicepair` message lines on
//! stdout, and for backups `\r[====   ]  45% (x/y)` batch progress plus `\r[…]  45% Finished` overall
//! lines (flushed explicitly), the final message and a tiny valid backup layout (`Info.plist`,
//! `Manifest.plist`, `Manifest.db`, `Status.plist` with `SnapshotState`, hashed-name files).
//! Exit codes are the tools': `idevicebackup2` returns negative codes (`(-N) & 0xFF` on Unix).
//!
//! **Pairing** mirrors the real tools: `hostid` prints `(null)` without a host record, and
//! `validate` without a record starts pairing (as `pair` does); so does `ideviceinfo` without `-s`.
//!
//! **Passwords** are read only from `BACKUP_PASSWORD_NEW` / `BACKUP_PASSWORD`. An invocation whose
//! argv contains a password (from those variables or the device's current one), or passes one as
//! the `encryption on|off` argument, fails with exit 64 and is recorded as `password_in_argv`.
//!
//! **Signals** (Unix): SIGTERM/SIGINT/SIGQUIT set a quit flag like the real tool; a running backup
//! then prints `Exiting...` (stderr) and `Backup Aborted.`, and exits. `ignore_term` ignores
//! SIGTERM. There are no signals on Windows, where the job object terminates the process.
//!
//! **Scenarios** (`FAKE_IDEVICE_SCENARIO`, default `success`) implement CONTRACTS.md §13.4:
//! `success`, `success_encrypt`, `already_encrypted`, `restore_fail`, `enable_fail`,
//! `enable_unknown`, `backup_fail`, `backup_fail_encrypted`, `incomplete`, `cancel_on_device`,
//! `disconnect`, `sync_lock`, `slow`, `ignore_term`, `cancel_during_enable`,
//! `cancel_during_restore`, `crash_after_enable`, `not_paired`, `locked`, `trust_denied`,
//! `pairing_failed`, `usbmuxd_missing` and `info_empty`.
//!
//! **Pacing and sizes:** `FAKE_IDEVICE_INTERVAL_MS` (default 20) between progress records;
//! `FAKE_IDEVICE_PROMPT_MS` (default 200, or 1500 for `cancel_during_enable` and
//! `cancel_during_restore`) that an encryption change waits after its passcode prompt;
//! `FAKE_IDEVICE_DATA_USED` (bytes, default 2 MiB) for the device's used data capacity. A slow
//! backup (`slow`, `ignore_term`, `crash_after_enable`) writes its pid to `backup.pid` in the state
//! dir and runs for up to 300 s.
//!
//! The only `unsafe` code is the libc signal handling in `signals` (Unix).

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Exit code for a broken test setup or a forbidden invocation, distinct from the tools' own.
const EXIT_SETUP: i32 = 64;
/// The fake device.
const UDID: &str = "00008101-000A1B2C3D4E001E";
/// The host pair record's HostID, once paired.
const HOST_ID: &str = "5E1B7C2A-9D4F-4E8B-A3C6-1F0D2B7E9A48";
const SYSTEM_BUID: &str = "0C6F3A9E-2B7D-4C1E-8F5A-6D9B3E0A7C21";
/// The device owner's backup password in `already_encrypted` (unknown to the examiner).
const OWNER_PASSWORD: &str = "fake-owner-backup-password";
/// How long a slow backup runs unless it is stopped.
const SLOW_RUN: Duration = Duration::from_secs(300);
const DEFAULT_DATA_USED: u64 = 2 * 1024 * 1024;
const DATA_CAPACITY: u64 = 64 * 1024 * 1024 * 1024;
const STATE_FILE: &str = "state.json";
const PID_FILE: &str = "backup.pid";

fn main() {
    let args: Vec<String> = env::args_os()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let code = match run(&args) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("fake-idevice: {message}");
            EXIT_SETUP
        }
    };
    let _ = io::stdout().flush();
    process::exit(code);
}

fn run(args: &[String]) -> Result<i32, String> {
    let (tool, rest) = select_tool(args).ok_or_else(|| {
        "cannot tell which tool to be: name the binary after a tool or pass the tool name first"
            .to_owned()
    })?;
    let scenario = match env::var("FAKE_IDEVICE_SCENARIO") {
        Ok(name) => Scenario::parse(&name).ok_or_else(|| format!("unknown scenario {name:?}"))?,
        Err(_) => Scenario::Success,
    };
    let mut store = Store::open(scenario)?;
    store.state.calls.push(
        std::iter::once(tool.name().to_owned())
            .chain(rest.iter().cloned())
            .collect(),
    );
    if password_in_argv(rest, &store.state) {
        store.state.password_in_argv = true;
        store.save()?;
        // Never print the password itself.
        return Err("a backup password appeared on the command line".to_owned());
    }
    store.save()?;
    let mut fake = Fake {
        scenario,
        store,
        pacing: Pacing::from_env(scenario)?,
    };
    let code = match tool {
        Tool::IdeviceId => fake.idevice_id(rest)?,
        Tool::Ideviceinfo => fake.ideviceinfo(rest)?,
        Tool::Idevicepair => fake.idevicepair(rest)?,
        Tool::Idevicebackup2 => fake.idevicebackup2(rest)?,
    };
    fake.store.save()?;
    Ok(code)
}

// ---- tools and scenarios ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    IdeviceId,
    Ideviceinfo,
    Idevicepair,
    Idevicebackup2,
}

impl Tool {
    const ALL: [Tool; 4] = [
        Tool::IdeviceId,
        Tool::Ideviceinfo,
        Tool::Idevicepair,
        Tool::Idevicebackup2,
    ];

    fn name(self) -> &'static str {
        match self {
            Tool::IdeviceId => "idevice_id",
            Tool::Ideviceinfo => "ideviceinfo",
            Tool::Idevicepair => "idevicepair",
            Tool::Idevicebackup2 => "idevicebackup2",
        }
    }

    fn from_name(name: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|tool| tool.name() == name)
    }
}

/// The tool named by argv[0]'s file stem, else by the first argument; and the tool's arguments.
fn select_tool(args: &[String]) -> Option<(Tool, &[String])> {
    // Either separator, so a Windows argv[0] is understood everywhere.
    let stem = args
        .first()
        .and_then(|arg0| arg0.rsplit(['/', '\\']).next())
        .and_then(|name| Path::new(name).file_stem())
        .map(|stem| stem.to_string_lossy());
    if let Some(tool) = stem.as_deref().and_then(Tool::from_name) {
        return Some((tool, &args[1..]));
    }
    let tool = args.get(1).and_then(|first| Tool::from_name(first))?;
    Some((tool, &args[2..]))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    Success,
    SuccessEncrypt,
    AlreadyEncrypted,
    RestoreFail,
    EnableFail,
    EnableUnknown,
    BackupFail,
    BackupFailEncrypted,
    Incomplete,
    CancelOnDevice,
    Disconnect,
    SyncLock,
    Slow,
    IgnoreTerm,
    CancelDuringEnable,
    CancelDuringRestore,
    CrashAfterEnable,
    NotPaired,
    Locked,
    TrustDenied,
    PairingFailed,
    UsbmuxdMissing,
    InfoEmpty,
}

const SCENARIOS: [(&str, Scenario); 23] = [
    ("success", Scenario::Success),
    ("success_encrypt", Scenario::SuccessEncrypt),
    ("already_encrypted", Scenario::AlreadyEncrypted),
    ("restore_fail", Scenario::RestoreFail),
    ("enable_fail", Scenario::EnableFail),
    ("enable_unknown", Scenario::EnableUnknown),
    ("backup_fail", Scenario::BackupFail),
    ("backup_fail_encrypted", Scenario::BackupFailEncrypted),
    ("incomplete", Scenario::Incomplete),
    ("cancel_on_device", Scenario::CancelOnDevice),
    ("disconnect", Scenario::Disconnect),
    ("sync_lock", Scenario::SyncLock),
    ("slow", Scenario::Slow),
    ("ignore_term", Scenario::IgnoreTerm),
    ("cancel_during_enable", Scenario::CancelDuringEnable),
    ("cancel_during_restore", Scenario::CancelDuringRestore),
    ("crash_after_enable", Scenario::CrashAfterEnable),
    ("not_paired", Scenario::NotPaired),
    ("locked", Scenario::Locked),
    ("trust_denied", Scenario::TrustDenied),
    ("pairing_failed", Scenario::PairingFailed),
    ("usbmuxd_missing", Scenario::UsbmuxdMissing),
    ("info_empty", Scenario::InfoEmpty),
];

impl Scenario {
    fn parse(name: &str) -> Option<Scenario> {
        SCENARIOS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, scenario)| *scenario)
    }

    /// Scenarios that start without a host pair record.
    fn starts_unpaired(self) -> bool {
        matches!(
            self,
            Scenario::NotPaired
                | Scenario::Locked
                | Scenario::TrustDenied
                | Scenario::PairingFailed
        )
    }

    /// Scenarios whose backup runs until it is stopped.
    fn slow_backup(self) -> bool {
        matches!(
            self,
            Scenario::Slow | Scenario::IgnoreTerm | Scenario::CrashAfterEnable
        )
    }
}

/// Timing and sizes from the environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pacing {
    interval: Duration,
    prompt_wait: Duration,
    data_used: u64,
}

impl Pacing {
    fn from_env(scenario: Scenario) -> Result<Pacing, String> {
        let number = |name: &str| -> Result<Option<u64>, String> {
            match env::var(name) {
                Ok(text) => text
                    .parse::<u64>()
                    .map(Some)
                    .map_err(|_| format!("{name} must be a number, not {text:?}")),
                Err(_) => Ok(None),
            }
        };
        let default_prompt = match scenario {
            Scenario::CancelDuringEnable | Scenario::CancelDuringRestore => 1500,
            _ => 200,
        };
        Ok(Pacing {
            interval: Duration::from_millis(number("FAKE_IDEVICE_INTERVAL_MS")?.unwrap_or(20)),
            prompt_wait: Duration::from_millis(
                number("FAKE_IDEVICE_PROMPT_MS")?.unwrap_or(default_prompt),
            ),
            data_used: number("FAKE_IDEVICE_DATA_USED")?.unwrap_or(DEFAULT_DATA_USED),
        })
    }
}

// ---- state ----

/// The fake device, as seen by every invocation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct State {
    connected: bool,
    /// A host pair record exists.
    paired: bool,
    will_encrypt: bool,
    /// `false` while `WillEncrypt` cannot be read (`enable_unknown`).
    will_encrypt_readable: bool,
    /// The device's backup password while encryption is on.
    password: Option<String>,
    /// `pair` (and `validate` without a record) calls so far, for the trust flow of `not_paired`.
    pair_calls: u32,
    /// Every invocation that tried to pair: `pair`, `validate` or `ideviceinfo` without a record.
    pairing_attempts: Vec<String>,
    /// An invocation carried a password on its command line.
    password_in_argv: bool,
    /// Every invocation: the tool name, then its arguments.
    calls: Vec<Vec<String>>,
}

impl State {
    fn initial(scenario: Scenario) -> State {
        let encrypted = scenario == Scenario::AlreadyEncrypted;
        State {
            connected: true,
            paired: !scenario.starts_unpaired(),
            will_encrypt: encrypted,
            will_encrypt_readable: true,
            password: encrypted.then(|| OWNER_PASSWORD.to_owned()),
            pair_calls: 0,
            pairing_attempts: Vec::new(),
            password_in_argv: false,
            calls: Vec::new(),
        }
    }
}

/// The state and where it is persisted.
struct Store {
    dir: Option<PathBuf>,
    state: State,
}

impl Store {
    fn open(scenario: Scenario) -> Result<Store, String> {
        let dir = env::var_os("FAKE_IDEVICE_STATE_DIR").map(PathBuf::from);
        let state = match &dir {
            Some(dir) => {
                let path = dir.join(STATE_FILE);
                match fs::read(&path) {
                    Ok(bytes) => serde_json::from_slice(&bytes)
                        .map_err(|e| format!("{}: {e}", path.display()))?,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => State::initial(scenario),
                    Err(e) => return Err(format!("{}: {e}", path.display())),
                }
            }
            None => State::initial(scenario),
        };
        Ok(Store { dir, state })
    }

    /// Writes the state (atomically: a temp file renamed over the old one).
    fn save(&self) -> Result<(), String> {
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(&self.state).map_err(|e| e.to_string())?;
        let path = dir.join(STATE_FILE);
        let tmp = dir.join(format!("{STATE_FILE}.tmp-{}", process::id()));
        fs::write(&tmp, bytes)
            .and_then(|()| fs::rename(&tmp, &path))
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    fn write_pid(&self) -> Result<(), String> {
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        fs::write(dir.join(PID_FILE), format!("{}\n", process::id())).map_err(|e| e.to_string())
    }
}

/// Whether an argument contains a password: one from the password variables, or the device's.
fn password_in_argv(args: &[String], state: &State) -> bool {
    let secrets: Vec<String> = ["BACKUP_PASSWORD_NEW", "BACKUP_PASSWORD"]
        .iter()
        .filter_map(|name| env::var(name).ok())
        .chain(state.password.clone())
        .filter(|secret| !secret.is_empty())
        .collect();
    args.iter()
        .any(|arg| secrets.iter().any(|secret| arg.contains(secret.as_str())))
}

// ---- argument parsing ----

/// Options and positional arguments, getopt-style: `-u VALUE` or `--udid VALUE` for the options in
/// `with_value`, everything else starting with `-` is a flag.
#[derive(Debug, Default, PartialEq, Eq)]
struct Parsed {
    values: Vec<(String, String)>,
    flags: Vec<String>,
    positional: Vec<String>,
}

impl Parsed {
    fn parse(args: &[String], with_value: &[&str]) -> Result<Parsed, String> {
        let mut parsed = Parsed::default();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            if with_value.contains(&arg.as_str()) {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("option {arg} requires an argument"))?;
                parsed.values.push((arg.clone(), value.clone()));
            } else if arg.starts_with('-') && arg.len() > 1 {
                parsed.flags.push(arg.clone());
            } else {
                parsed.positional.push(arg.clone());
            }
        }
        Ok(parsed)
    }

    fn value(&self, names: &[&str]) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(name, _)| names.contains(&name.as_str()))
            .map(|(_, value)| value.as_str())
    }

    fn has(&self, names: &[&str]) -> bool {
        self.flags.iter().any(|flag| names.contains(&flag.as_str()))
    }
}

// ---- the tools ----

struct Fake {
    scenario: Scenario,
    store: Store,
    pacing: Pacing,
}

impl Fake {
    fn state(&mut self) -> &mut State {
        &mut self.store.state
    }

    /// Whether usbmuxd can see the device named by `udid` (any device when `None`).
    fn device_present(&self, udid: Option<&str>) -> bool {
        self.scenario != Scenario::UsbmuxdMissing
            && self.store.state.connected
            && udid.is_none_or(|udid| udid == UDID)
    }

    // -- idevice_id --

    fn idevice_id(&mut self, args: &[String]) -> Result<i32, String> {
        let parsed = Parsed::parse(args, &[])?;
        if parsed.has(&["-v", "--version"]) {
            println!("idevice_id 1.4.0");
            return Ok(0);
        }
        if !parsed.has(&["-l", "--list"]) || !parsed.positional.is_empty() {
            return Err("suiteDFIR only runs `idevice_id -l`".to_owned());
        }
        if self.scenario == Scenario::UsbmuxdMissing {
            eprintln!("ERROR: Unable to retrieve device list!");
            return Ok(-1);
        }
        if self.store.state.connected {
            println!("{UDID}");
        }
        Ok(0)
    }

    // -- ideviceinfo --

    fn ideviceinfo(&mut self, args: &[String]) -> Result<i32, String> {
        let parsed = Parsed::parse(args, &["-u", "--udid", "-q", "--domain", "-k", "--key"])?;
        if parsed.has(&["-v", "--version"]) {
            println!("ideviceinfo 1.4.0");
            return Ok(0);
        }
        if !parsed.has(&["-x", "--xml"]) {
            return Err("suiteDFIR always runs ideviceinfo with -x".to_owned());
        }
        let udid = parsed.value(&["-u", "--udid"]);
        if !self.device_present(udid) {
            match udid {
                Some(udid) => eprintln!("ERROR: Device {udid} not found!"),
                None => eprintln!("ERROR: No device found!"),
            }
            return Ok(-1);
        }
        let simple = parsed.has(&["-s", "--simple"]);
        if !simple && !self.store.state.paired {
            // lockdownd_client_new_with_handshake pairs when there is no record, which shows the
            // Trust dialog on the device.
            self.state().pairing_attempts.push("ideviceinfo".to_owned());
            eprintln!(
                "ERROR: Could not connect to lockdownd: Pairing dialog response pending (-19)"
            );
            return Ok(-1);
        }
        if self.scenario == Scenario::InfoEmpty {
            // The value read failed: exit 0 with empty stdout (IDEVICE-CLI.md §2).
            return Ok(0);
        }
        let state = &self.store.state;
        let value = match (
            parsed.value(&["-q", "--domain"]),
            parsed.value(&["-k", "--key"]),
        ) {
            (None, None) => Some(device_info(simple)),
            (Some("com.apple.mobile.backup"), Some("WillEncrypt")) => state
                .will_encrypt_readable
                .then_some(plist::Value::Boolean(state.will_encrypt)),
            (Some("com.apple.disk_usage"), None) => Some(disk_usage(self.pacing.data_used)),
            _ => None,
        };
        if let Some(value) = value {
            let mut out = io::stdout().lock();
            value.to_writer_xml(&mut out).map_err(|e| e.to_string())?;
            writeln!(out).map_err(|e| e.to_string())?;
        }
        Ok(0)
    }

    // -- idevicepair --

    fn idevicepair(&mut self, args: &[String]) -> Result<i32, String> {
        let parsed = Parsed::parse(args, &["-u", "--udid"])?;
        if parsed.has(&["-v", "--version"]) {
            println!("idevicepair 1.4.0");
            return Ok(0);
        }
        let [command] = parsed.positional.as_slice() else {
            return Err("idevicepair needs exactly one command".to_owned());
        };
        if command == "systembuid" {
            println!("{SYSTEM_BUID}");
            return Ok(0);
        }
        let udid = parsed.value(&["-u", "--udid"]);
        if !self.device_present(udid) {
            match udid {
                Some(udid) => println!("No device found with udid {udid}."),
                None => println!("No device found."),
            }
            return Ok(1);
        }
        match command.as_str() {
            "hostid" => {
                // The host's record is read through usbmuxd; no device session is opened.
                if self.store.state.paired {
                    println!("{HOST_ID}");
                } else {
                    println!("(null)");
                }
                Ok(0)
            }
            "validate" if self.store.state.paired => {
                println!("SUCCESS: Validated pairing with device {UDID}");
                Ok(0)
            }
            // Without a record, validate's handshake pairs, exactly like `pair`.
            "validate" | "pair" => Ok(self.pair(command)),
            other => Err(format!("unsupported idevicepair command {other:?}")),
        }
    }

    /// An attempt to pair, as `command`: the scenario decides the device's answer.
    fn pair(&mut self, command: &str) -> i32 {
        let scenario = self.scenario;
        let state = self.state();
        state.pairing_attempts.push(command.to_owned());
        state.pair_calls += 1;
        let first_call = state.pair_calls == 1;
        match scenario {
            Scenario::Locked => {
                println!(
                    "ERROR: Could not validate with device {UDID} because a passcode is set. \
                     Please enter the passcode on the device and retry."
                );
                1
            }
            Scenario::TrustDenied => {
                println!("ERROR: Device {UDID} said that the user denied the trust dialog.");
                1
            }
            Scenario::PairingFailed => {
                println!("ERROR: Pairing with device {UDID} failed.");
                1
            }
            Scenario::NotPaired if first_call => {
                println!(
                    "ERROR: Please accept the trust dialog on the screen of device {UDID}, then \
                     attempt to pair again."
                );
                1
            }
            _ => {
                state.paired = true;
                if command == "validate" {
                    println!("SUCCESS: Validated pairing with device {UDID}");
                } else {
                    println!("SUCCESS: Paired with device {UDID}");
                }
                0
            }
        }
    }

    // -- idevicebackup2 --

    fn idevicebackup2(&mut self, args: &[String]) -> Result<i32, String> {
        let parsed = Parsed::parse(args, &["-u", "--udid", "-s", "--source"])?;
        if parsed.has(&["-v", "--version"]) {
            println!("idevicebackup2 1.4.0");
            return Ok(0);
        }
        if parsed.has(&["-i", "--interactive"]) {
            return Err("suiteDFIR never runs idevicebackup2 in interactive mode".to_owned());
        }
        if parsed.has(&["-n", "--network"]) {
            return Err("suiteDFIR never uses network devices".to_owned());
        }
        signals::install(self.scenario == Scenario::IgnoreTerm);
        let udid = parsed.value(&["-u", "--udid"]);
        let positional: Vec<&str> = parsed.positional.iter().map(String::as_str).collect();
        match positional.as_slice() {
            ["backup", dir] => {
                if !parsed.flags.iter().all(|flag| flag == "--full") {
                    return Err(format!("unsupported backup options {:?}", parsed.flags));
                }
                if !self.device_present(udid) {
                    println!("No device found with udid {}.", udid.unwrap_or(UDID));
                    return Ok(-1);
                }
                self.backup(Path::new(dir))
            }
            ["encryption", mode @ ("on" | "off")] => {
                if !self.device_present(udid) {
                    println!("No device found with udid {}.", udid.unwrap_or(UDID));
                    return Ok(-1);
                }
                self.encryption(*mode == "on")
            }
            ["encryption", _, _, ..] => {
                self.state().password_in_argv = true;
                Err("a password was passed as an argument of `encryption`".to_owned())
            }
            other => Err(format!("unsupported idevicebackup2 command {other:?}")),
        }
    }

    fn encryption(&mut self, enable: bool) -> Result<i32, String> {
        let env_password = |name: &str| env::var(name).ok().filter(|p| !p.is_empty());
        // `encryption on` reads BACKUP_PASSWORD_NEW, then BACKUP_PASSWORD; `off` reads
        // BACKUP_PASSWORD (idevicebackup2.c:1756-1768, 2167-2195).
        let password = if enable {
            env_password("BACKUP_PASSWORD_NEW").or_else(|| env_password("BACKUP_PASSWORD"))
        } else {
            env_password("BACKUP_PASSWORD")
        };
        let Some(password) = password else {
            println!(
                "ERROR: Can't get password input in non-interactive mode. Either pass password(s) \
                 on the command line, or enable interactive mode with -i or --interactive."
            );
            return Ok(-1);
        };
        println!("Started \"com.apple.mobilebackup2\" service on port 49324.");
        println!("Negotiated Protocol Version 2.1");
        let state = &self.store.state;
        if enable && state.will_encrypt {
            println!("ERROR: Backup encryption is already enabled. Aborting.");
            return Ok(-1);
        }
        if !enable && !state.will_encrypt {
            println!("ERROR: Backup encryption is not enabled. Aborting.");
            return Ok(-1);
        }
        let action = if enable { "enabling" } else { "disabling" };
        println!(
            "Please confirm {action} the backup encryption by entering the passcode on the device."
        );
        flush();
        // The tool waits for the device without a time limit; the fake's examiner is quick.
        thread::sleep(self.pacing.prompt_wait);
        let scenario = self.scenario;
        let state = self.state();
        if enable {
            match scenario {
                Scenario::EnableFail => {
                    println!(
                        "ErrorCode 22: Could not change the backup password (MBErrorDomain/22)"
                    );
                    println!("Could not enable backup encryption.");
                    Ok(-22)
                }
                Scenario::EnableUnknown => {
                    // The change reached the device, but the tool failed and WillEncrypt cannot be
                    // read until encryption is turned off again.
                    state.will_encrypt = true;
                    state.will_encrypt_readable = false;
                    state.password = Some(password);
                    println!("ERROR: Could not receive from mobilebackup2 (-4)");
                    println!("Could not enable backup encryption.");
                    Ok(-1)
                }
                _ => {
                    state.will_encrypt = true;
                    state.password = Some(password);
                    println!("Backup encryption has been enabled successfully.");
                    Ok(0)
                }
            }
        } else if scenario == Scenario::RestoreFail || state.password.as_deref() != Some(&password)
        {
            println!("ErrorCode 207: Wrong password (MBErrorDomain/207)");
            println!("Could not disable backup encryption.");
            Ok(-207)
        } else {
            state.will_encrypt = false;
            state.will_encrypt_readable = true;
            state.password = None;
            println!("Backup encryption has been disabled successfully.");
            Ok(0)
        }
    }

    fn backup(&mut self, dir: &Path) -> Result<i32, String> {
        if !dir.is_dir() {
            eprintln!(
                "ERROR: Backup directory \"{}\" does not exist!",
                dir.display()
            );
            return Ok(-1);
        }
        println!("Backup directory is \"{}\"", dir.display());
        println!("Started \"com.apple.mobilebackup2\" service on port 49324.");
        println!("Negotiated Protocol Version 2.1");
        if self.scenario == Scenario::SyncLock {
            // Finder or iTunes holds /com.apple.itunes.lock_sync (idevicebackup2.c:1973).
            eprintln!("ERROR: timeout while locking for sync");
            return Ok(-1);
        }
        let encrypted = self.store.state.will_encrypt;
        println!("Starting backup...");
        println!("Enforcing full backup from device.");
        if encrypted {
            println!("Backup will be encrypted.");
        } else {
            println!("Backup will be unencrypted.");
        }
        println!("Requesting backup from device...");
        println!("Full backup mode.");
        flush();
        let udid_dir = dir.join(UDID);
        let layout = |manifest: bool, snapshot: &str| Layout {
            encrypted,
            manifest,
            snapshot: snapshot.to_owned(),
        };
        let interval = self.pacing.interval;
        match self.scenario {
            scenario if scenario.slow_backup() => {
                layout(false, "new").write(&udid_dir)?;
                self.store.write_pid()?;
                Ok(self.slow_backup(&udid_dir, encrypted))
            }
            Scenario::BackupFail | Scenario::BackupFailEncrypted => {
                layout(false, "new").write(&udid_dir)?;
                if stream_progress(0, 60, interval) {
                    return Ok(aborted(1));
                }
                println!(
                    "ErrorCode 105: Insufficient free disk space on drive to upload files. \
                     (MBErrorDomain/105)"
                );
                println!("Received 1 files from device.");
                println!("Backup Failed (Error Code 105).");
                Ok(-105)
            }
            Scenario::Incomplete => {
                layout(true, "new").write(&udid_dir)?;
                if stream_progress(0, 100, interval) {
                    return Ok(aborted(3));
                }
                println!("Received 3 files from device.");
                println!("Backup Failed (Error Code 0).");
                Ok(0)
            }
            Scenario::CancelOnDevice => {
                layout(false, "new").write(&udid_dir)?;
                if stream_progress(0, 30, interval) {
                    return Ok(aborted(1));
                }
                println!("User has cancelled the backup process on the device.");
                println!("Received 1 files from device.");
                println!("Backup Aborted.");
                Ok(-1)
            }
            Scenario::Disconnect => {
                layout(false, "new").write(&udid_dir)?;
                if stream_progress(0, 40, interval) {
                    return Ok(aborted(1));
                }
                // The device vanished: the receive fails, and it is gone from usbmuxd's list.
                self.state().connected = false;
                println!("ERROR: Could not receive from mobilebackup2 (-4)");
                println!("Received 1 files from device.");
                println!("Backup Aborted.");
                Ok(-1)
            }
            _ => {
                layout(false, "new").write(&udid_dir)?;
                if stream_progress(0, 100, interval) {
                    return Ok(aborted(1));
                }
                layout(true, "finished").write(&udid_dir)?;
                println!("Received 3 files from device.");
                println!("Backup Successful.");
                Ok(0)
            }
        }
    }

    /// A backup that crawls until it is stopped (or `SLOW_RUN` elapses, then succeeds).
    fn slow_backup(&mut self, udid_dir: &Path, encrypted: bool) -> i32 {
        let started = Instant::now();
        let heartbeat = self.pacing.interval.max(Duration::from_millis(100));
        let mut batch: u64 = 0;
        while started.elapsed() < SLOW_RUN {
            if signals::quit_requested() {
                return aborted(1);
            }
            let overall = u8::try_from(started.elapsed().as_secs() / 3 + 1)
                .unwrap_or(99)
                .min(99);
            batch = (batch + 7) % 100;
            print_batch(batch);
            if batch < 7 {
                print_overall(overall);
            }
            thread::sleep(heartbeat);
        }
        let layout = Layout {
            encrypted,
            manifest: true,
            snapshot: "finished".to_owned(),
        };
        if let Err(e) = layout.write(udid_dir) {
            eprintln!("fake-idevice: {e}");
            return EXIT_SETUP;
        }
        print_overall(100);
        println!("Received 3 files from device.");
        println!("Backup Successful.");
        0
    }
}

/// The quit flag was set (SIGTERM): what the real tool prints and returns.
fn aborted(files: u32) -> i32 {
    eprintln!("Exiting...");
    println!("Received {files} files from device.");
    println!("Backup Aborted.");
    flush();
    -1
}

fn flush() {
    let _ = io::stdout().flush();
}

// ---- progress output ----

/// The 50-character bar of `print_progress_real` (idevicebackup2.c:665-683).
fn bar(percent: u8) -> String {
    (0..50)
        .map(|i| {
            if f64::from(i) < f64::from(percent) / 2.0 {
                '='
            } else {
                ' '
            }
        })
        .collect()
}

/// A per-batch progress record: `\r[bar] NN% (x/y)` with trailing spaces, flushed, no newline.
fn batch_record(percent: u8) -> String {
    let total_kb = 2700u64;
    let done_kb = total_kb * u64::from(percent) / 100;
    format!(
        "\r[{}] {percent:3}% ({}.{} MB/2.7 MB)     ",
        bar(percent),
        done_kb / 1000,
        done_kb % 1000 / 100
    )
}

/// The overall progress record: `\r[bar] NN%` followed by ` Finished` and a newline.
fn overall_record(percent: u8) -> String {
    format!("\r[{}] {percent:3}% Finished\n", bar(percent))
}

fn print_batch(percent: u64) {
    print!("{}", batch_record(u8::try_from(percent).unwrap_or(100)));
    flush();
}

fn print_overall(percent: u8) {
    print!("{}", overall_record(percent));
    flush();
}

/// Streams batch and overall progress from `from` to `to` percent. Returns true if a quit signal
/// arrived.
fn stream_progress(from: u8, to: u8, interval: Duration) -> bool {
    let mut overall = from;
    while overall < to {
        overall = (overall + 10).min(to);
        for batch in [0u64, 50, 100] {
            if signals::quit_requested() {
                return true;
            }
            print_batch(batch);
            thread::sleep(interval);
        }
        print_overall(overall);
    }
    signals::quit_requested()
}

// ---- the backup layout ----

/// What `<dir>/<udid>/` gets.
struct Layout {
    encrypted: bool,
    /// Whether `Manifest.plist` and `Manifest.db` exist yet.
    manifest: bool,
    snapshot: String,
}

impl Layout {
    fn write(&self, udid_dir: &Path) -> Result<(), String> {
        let fail = |e: &dyn std::fmt::Display| format!("{}: {e}", udid_dir.display());
        fs::create_dir_all(udid_dir).map_err(|e| fail(&e))?;
        let text = |value: &str| plist::Value::String(value.to_owned());
        let dict = |items: Vec<(&str, plist::Value)>| {
            plist::Value::Dictionary(
                items
                    .into_iter()
                    .map(|(key, value)| (key.to_owned(), value))
                    .collect(),
            )
        };
        dict(vec![
            ("Device Name", text("Fake iPhone")),
            ("Display Name", text("Fake iPhone")),
            ("Product Type", text("iPhone13,2")),
            ("Product Version", text("18.6")),
            ("Build Version", text("22G86")),
            ("Unique Identifier", text(UDID)),
            ("Target Identifier", text(UDID)),
            ("Target Type", text("Device")),
        ])
        .to_file_xml(udid_dir.join("Info.plist"))
        .map_err(|e| fail(&e))?;
        dict(vec![
            ("SnapshotState", text(&self.snapshot)),
            ("IsFullBackup", plist::Value::Boolean(true)),
            ("Version", text("3.3")),
            ("BackupState", text("new")),
        ])
        .to_file_binary(udid_dir.join("Status.plist"))
        .map_err(|e| fail(&e))?;
        if self.manifest {
            dict(vec![
                ("IsEncrypted", plist::Value::Boolean(self.encrypted)),
                ("Version", text("10.0")),
                ("WasPasscodeSet", plist::Value::Boolean(true)),
            ])
            .to_file_binary(udid_dir.join("Manifest.plist"))
            .map_err(|e| fail(&e))?;
            let mut db = b"SQLite format 3\0".to_vec();
            db.resize(4096, 0);
            fs::write(udid_dir.join("Manifest.db"), db).map_err(|e| fail(&e))?;
        }
        for (hash, content) in [
            ("3d0d7e5fb2ce288813306e4d4636395e047a3d28", "fake sms.db"),
            (
                "31bb7ba8914766d4ba40d6dfb6113c8b614be442",
                "fake AddressBook.sqlitedb",
            ),
        ] {
            let folder = udid_dir.join(&hash[..2]);
            fs::create_dir_all(&folder).map_err(|e| fail(&e))?;
            fs::write(folder.join(hash), content).map_err(|e| fail(&e))?;
        }
        Ok(())
    }
}

// ---- ideviceinfo values ----

fn device_info(simple: bool) -> plist::Value {
    let text = |value: &str| plist::Value::String(value.to_owned());
    // The pre-session subset (`-s`) lacks the serial number and the phone identifiers.
    let mut items = vec![
        ("BuildVersion", text("22G86")),
        ("DeviceClass", text("iPhone")),
        ("DeviceName", text("Fake iPhone")),
        ("ProductType", text("iPhone13,2")),
        ("ProductVersion", text("18.6")),
        ("UniqueDeviceID", text(UDID)),
    ];
    if !simple {
        items.extend([
            ("SerialNumber", text("F2LFAKE00001")),
            (
                "InternationalMobileEquipmentIdentity",
                text("356938035643809"),
            ),
            ("PhoneNumber", text("+1 (555) 010-0199")),
            ("WiFiAddress", text("a4:c3:f0:00:00:01")),
        ]);
    }
    plist::Value::Dictionary(
        items
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
    )
}

/// The data partition grows with `data_used` beyond the default capacity.
fn disk_usage(data_used: u64) -> plist::Value {
    let number = |value: u64| plist::Value::Integer(value.into());
    let capacity = DATA_CAPACITY.max(data_used);
    let system = 8 * 1024 * 1024 * 1024;
    plist::Value::Dictionary(
        [
            ("TotalDiskCapacity", number(capacity.saturating_add(system))),
            ("TotalSystemCapacity", number(system)),
            ("TotalDataCapacity", number(capacity)),
            ("TotalDataAvailable", number(capacity - data_used)),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value))
        .collect(),
    )
}

// ---- Unix signal handling ----

/// The only `unsafe` in this binary: installing signal dispositions with libc. This file is a test
/// double and is never bundled (ARCHITECTURE.md §5.1).
#[cfg(unix)]
mod signals {
    use std::sync::atomic::{AtomicBool, Ordering};

    static QUIT: AtomicBool = AtomicBool::new(false);

    /// Like the real tool's `clean_exit`, but it only sets the flag (async-signal-safe); the main
    /// loop prints `Exiting...`.
    extern "C" fn quit(_signal: libc::c_int) {
        QUIT.store(true, Ordering::SeqCst);
    }

    fn set_disposition(signal: libc::c_int, handler: libc::sighandler_t) {
        // SAFETY: FFI call. `handler` is SIG_IGN or `quit`, an `extern "C"` function with the
        // signature signal(3) expects that only stores to an atomic.
        unsafe {
            libc::signal(signal, handler);
        }
    }

    /// SIGINT, SIGTERM and SIGQUIT set the quit flag, as in idevicebackup2.c:1539-1542; with
    /// `ignore_term`, SIGTERM is ignored instead.
    pub fn install(ignore_term: bool) {
        let handler = quit as extern "C" fn(libc::c_int) as libc::sighandler_t;
        set_disposition(libc::SIGINT, handler);
        set_disposition(libc::SIGQUIT, handler);
        set_disposition(
            libc::SIGTERM,
            if ignore_term { libc::SIG_IGN } else { handler },
        );
    }

    pub fn quit_requested() -> bool {
        QUIT.load(Ordering::SeqCst)
    }
}

/// Windows has no signals: the job object terminates the process.
#[cfg(not(unix))]
mod signals {
    pub fn install(_ignore_term: bool) {}

    pub fn quit_requested() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn selects_the_tool_by_file_stem_or_first_argument() {
        let args = strings(&["/tools/idevicepair", "-u", UDID, "hostid"]);
        let (tool, rest) = select_tool(&args).unwrap();
        assert_eq!(tool, Tool::Idevicepair);
        assert_eq!(rest, &args[1..]);
        let args = strings(&[r"C:\tools\idevice_id.exe", "-l"]);
        assert_eq!(select_tool(&args).unwrap().0, Tool::IdeviceId);
        let args = strings(&["target/debug/fake-idevice", "idevicebackup2", "backup"]);
        let (tool, rest) = select_tool(&args).unwrap();
        assert_eq!(tool, Tool::Idevicebackup2);
        assert_eq!(rest, &args[2..]);
        assert!(select_tool(&strings(&["fake-idevice"])).is_none());
        assert!(select_tool(&strings(&["fake-idevice", "idevicerestore"])).is_none());
    }

    #[test]
    fn every_contract_scenario_parses() {
        for (name, scenario) in SCENARIOS {
            assert_eq!(Scenario::parse(name), Some(scenario));
        }
        assert_eq!(Scenario::parse("nope"), None);
        assert!(Scenario::NotPaired.starts_unpaired());
        assert!(!Scenario::Success.starts_unpaired());
        assert!(Scenario::CrashAfterEnable.slow_backup());
    }

    #[test]
    fn parses_getopt_style_arguments() {
        let parsed = Parsed::parse(
            &strings(&["-u", UDID, "-q", "com.apple.disk_usage", "-x", "-s"]),
            &["-u", "-q", "-k"],
        )
        .unwrap();
        assert_eq!(parsed.value(&["-u", "--udid"]), Some(UDID));
        assert_eq!(parsed.value(&["-q"]), Some("com.apple.disk_usage"));
        assert!(parsed.has(&["-x"]) && parsed.has(&["-s", "--simple"]));
        assert!(parsed.positional.is_empty());
        let parsed =
            Parsed::parse(&strings(&["-u", UDID, "backup", "--full", "/x"]), &["-u"]).unwrap();
        assert_eq!(parsed.positional, ["backup", "/x"]);
        assert_eq!(parsed.flags, ["--full"]);
        assert!(Parsed::parse(&strings(&["-u"]), &["-u"]).is_err());
    }

    #[test]
    fn detects_passwords_in_arguments() {
        let mut state = State::initial(Scenario::Success);
        assert!(!password_in_argv(&strings(&["encryption", "on"]), &state));
        state.password = Some("s3cret-pw".to_owned());
        assert!(password_in_argv(
            &strings(&["encryption", "off", "s3cret-pw"]),
            &state
        ));
        assert!(password_in_argv(
            &strings(&["--password=s3cret-pw"]),
            &state
        ));
        assert!(!password_in_argv(&strings(&["backup", "/cases/x"]), &state));
    }

    #[test]
    fn progress_records_match_the_real_format() {
        let batch = batch_record(45);
        assert!(batch.starts_with("\r["), "{batch:?}");
        assert!(batch.contains("]  45% (1.2 MB/2.7 MB)"), "{batch:?}");
        assert!(!batch.contains('\n'));
        let overall = overall_record(100);
        assert!(overall.ends_with("] 100% Finished\n"), "{overall:?}");
        assert_eq!(bar(100), "=".repeat(50));
        assert_eq!(bar(0), " ".repeat(50));
        assert_eq!(bar(45).matches('=').count(), 23);
    }

    #[test]
    fn writes_a_valid_backup_layout() {
        let dir = tempfile::tempdir().unwrap();
        let udid_dir = dir.path().join(UDID);
        Layout {
            encrypted: true,
            manifest: true,
            snapshot: "finished".to_owned(),
        }
        .write(&udid_dir)
        .unwrap();
        for name in [
            "Info.plist",
            "Manifest.plist",
            "Manifest.db",
            "Status.plist",
        ] {
            assert!(udid_dir.join(name).is_file(), "{name}");
        }
        let status = plist::Value::from_file(udid_dir.join("Status.plist")).unwrap();
        let snapshot = status
            .as_dictionary()
            .and_then(|d| d.get("SnapshotState"))
            .and_then(plist::Value::as_string);
        assert_eq!(snapshot, Some("finished"));
        let manifest = plist::Value::from_file(udid_dir.join("Manifest.plist")).unwrap();
        let encrypted = manifest
            .as_dictionary()
            .and_then(|d| d.get("IsEncrypted"))
            .and_then(plist::Value::as_boolean);
        assert_eq!(encrypted, Some(true));
        assert!(
            udid_dir
                .join("3d")
                .join("3d0d7e5fb2ce288813306e4d4636395e047a3d28")
                .is_file()
        );

        // A partial layout has no manifest yet.
        let partial = dir.path().join("partial");
        Layout {
            encrypted: false,
            manifest: false,
            snapshot: "new".to_owned(),
        }
        .write(&partial)
        .unwrap();
        assert!(!partial.join("Manifest.db").exists());
        assert!(partial.join("Status.plist").is_file());
    }

    #[test]
    fn device_info_values() {
        let full = device_info(false);
        let dict = full.as_dictionary().unwrap();
        assert!(dict.contains_key("SerialNumber"));
        assert!(dict.contains_key("InternationalMobileEquipmentIdentity"));
        let simple = device_info(true);
        assert!(!simple.as_dictionary().unwrap().contains_key("SerialNumber"));
        let usage = disk_usage(1000);
        let usage = usage.as_dictionary().unwrap();
        let number = |key: &str| usage.get(key).unwrap().as_unsigned_integer().unwrap();
        let (capacity, available) = (number("TotalDataCapacity"), number("TotalDataAvailable"));
        assert_eq!(capacity - available, 1000);
    }
}
