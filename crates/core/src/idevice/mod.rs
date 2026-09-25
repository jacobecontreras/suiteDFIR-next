//! The libimobiledevice tools (`idevice_id`, `ideviceinfo`, `idevicepair`, `idevicebackup2`;
//! ARCHITECTURE.md D21, docs/IDEVICE-CLI.md): locating and verifying them ([`tools`]), running
//! them through [`crate::process`], and parsing their output ([`parse`]).
//!
//! - [`Idevice`] is the app-wide handle: `devices_list` ([`Idevice::list_devices`], single-flight,
//!   never pairs), `device_pair` ([`Idevice::pair`], the only code path that pairs) and the
//!   `paired_by_app_at` memory of this app session.
//! - A [`Session`] runs individual commands in a scratch directory: a fresh
//!   `<app_cache>/tmp/<id>/` (named like an acquisition id, so the startup sweep removes leftovers)
//!   or an acquisition's own temp dir. Output is captured to files there, read, and deleted; it is
//!   never written to the app log.
//! - Every command has a 20 s timeout ([`COMMAND_TIMEOUT`]), except the encryption changes
//!   ([`Session::set_encryption`]), which may wait for the device passcode without a time limit.
//!   Passwords reach the tool only through `BACKUP_PASSWORD_NEW` / `BACKUP_PASSWORD`.

pub mod parse;
pub mod tools;

use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::contracts::{
    AppError, DevicePromptKind, DeviceSummary, DevicesResult, ErrorCode, IdeviceToolsState,
    IdeviceToolsStatus, PairState, PlatformKey, StdStream, Timestamp,
};
use crate::fsutil;
use crate::process::{self, ExitInfo, SpawnSpec};

pub use parse::{DeviceInfo, DiskUsage, LineSplitter, PairAnswer, is_udid};
pub use tools::{IdeviceTools, Tool, ToolLookup, ToolName, ToolsProblem, embedded_manifest};

/// The timeout of every tool command except the encryption changes (ROADMAP X2).
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);

/// How many fresh scratch-dir ids are tried when one is taken.
const SCRATCH_ATTEMPTS: u32 = 16;

/// Guidance for `usbmuxd_unavailable`. `idevice_id` prints the same message for every usbmuxd
/// failure, so it covers a missing service and one that is installed but not reachable.
const USBMUXD_GUIDANCE: &str = "The USB device service (usbmuxd) could not be reached. On \
     Windows, install Apple Devices (or iTunes) and make sure the Apple Mobile Device Service is \
     running. On Linux, install usbmuxd and start it (`sudo systemctl start usbmuxd`). On macOS, \
     reconnect the device; if that does not help, restart the Mac. Then try again.";

// ---- errors ----

/// Errors from the device commands.
#[derive(Debug, thiserror::Error)]
pub enum IdeviceError {
    #[error("{0:?} is not a device UDID")]
    InvalidUdid(String),
    #[error("device {0} is in use by the active job")]
    DeviceBusy(String),
    #[error("device {0} is already paired with this computer")]
    AlreadyPaired(String),
    #[error("device {0} is not connected")]
    DeviceNotFound(String),
    #[error("the iOS tools cannot be used: {0}")]
    Tools(ToolsProblem),
    #[error("usbmuxd could not be reached ({0})")]
    UsbmuxdUnavailable(String),
    /// The password has characters the tools cannot pass on intact on this OS
    /// ([`Password::tool_problem`]). The password itself is never part of the error.
    #[error("the backup password cannot be used with the iOS tools: {0}")]
    PasswordNotSupported(&'static str),
    #[error("{what}: {source}")]
    Io {
        what: String,
        #[source]
        source: io::Error,
    },
}

impl IdeviceError {
    fn io(what: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            what: what.into(),
            source,
        }
    }

    /// The `AppError` code (CONTRACTS.md §12).
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidUdid(_) | Self::DeviceNotFound(_) => ErrorCode::DeviceNotFound,
            Self::DeviceBusy(_) => ErrorCode::DeviceBusy,
            Self::AlreadyPaired(_) => ErrorCode::AlreadyPaired,
            Self::Tools(problem) => match problem.state {
                IdeviceToolsState::Missing => ErrorCode::IdeviceToolsMissing,
                IdeviceToolsState::VerificationFailed => ErrorCode::IdeviceToolsVerificationFailed,
                IdeviceToolsState::UnsupportedPlatform => ErrorCode::UnsupportedPlatform,
                IdeviceToolsState::UsbmuxdUnavailable => ErrorCode::UsbmuxdUnavailable,
                IdeviceToolsState::Ok => ErrorCode::Internal,
            },
            Self::UsbmuxdUnavailable(_) => ErrorCode::UsbmuxdUnavailable,
            Self::PasswordNotSupported(_) => ErrorCode::InvalidInput,
            Self::Io { source, .. } => fsutil::io_error_code(source),
        }
    }

    /// A message that is safe to show.
    pub fn message(&self) -> String {
        match self {
            Self::InvalidUdid(_) => "That is not a valid device identifier.".to_owned(),
            Self::DeviceBusy(_) => "The device is in use by the current acquisition.".to_owned(),
            Self::AlreadyPaired(_) => "The device is already paired with this computer.".to_owned(),
            Self::DeviceNotFound(_) => {
                "The device is not connected. Connect it with a USB cable and unlock it.".to_owned()
            }
            Self::Tools(problem) => problem.guidance.clone(),
            Self::UsbmuxdUnavailable(_) => USBMUXD_GUIDANCE.to_owned(),
            Self::PasswordNotSupported(_) => "On Windows, the iOS tools read the backup password \
                 in the ANSI code page, so characters outside plain ASCII could reach the device \
                 changed, and the backup could then not be decrypted with the password you typed. \
                 Use only printable ASCII characters (letters A-Z and a-z, digits, spaces and \
                 punctuation)."
                .to_owned(),
            Self::Io { .. } => "A device command could not be run.".to_owned(),
        }
    }
}

impl From<IdeviceError> for AppError {
    fn from(err: IdeviceError) -> Self {
        AppError {
            code: err.code(),
            message: err.message(),
            detail: Some(err.to_string()),
        }
    }
}

// ---- passwords ----

/// A backup password. It is handed to a tool only through its environment, its `Debug` output is
/// redacted, and its bytes are overwritten when it is dropped (best effort: the OS copies the
/// environment of the spawned process, and std keeps a copy until the spawn returns).
pub struct Password(Vec<u8>);

impl Password {
    pub fn new(password: String) -> Self {
        Self(password.into_bytes())
    }

    /// The length in characters.
    pub fn chars(&self) -> usize {
        String::from_utf8_lossy(&self.0).chars().count()
    }

    /// Whether `text` contains the password (for tests that check it never leaks).
    pub fn is_in(&self, text: &str) -> bool {
        !self.0.is_empty() && text.as_bytes().windows(self.0.len()).any(|w| w == self.0)
    }

    /// Why the tools on this OS cannot take this password intact, if they cannot. On Windows they
    /// read `BACKUP_PASSWORD(_NEW)` with the C runtime's `getenv`, which converts the environment
    /// to the ANSI code page: anything but printable ASCII may reach the device as other bytes (or
    /// `?`), while iLEAPP later gets the Unicode password (IDEVICE-CLI.md §5).
    pub fn tool_problem(&self) -> Option<&'static str> {
        password_problem(&self.0, cfg!(windows))
    }

    fn env_value(&self) -> OsString {
        OsString::from(String::from_utf8_lossy(&self.0).into_owned())
    }
}

/// [`Password::tool_problem`] for `bytes` on Windows (`windows`) or elsewhere.
fn password_problem(bytes: &[u8], windows: bool) -> Option<&'static str> {
    let printable_ascii = bytes.iter().all(|b| (0x20..=0x7e).contains(b));
    (windows && !printable_ascii).then_some(
        "on Windows the tools read it in the ANSI code page, so only printable ASCII is supported",
    )
}

impl Drop for Password {
    fn drop(&mut self) {
        self.0.fill(0);
        std::hint::black_box(&self.0);
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(<redacted>)")
    }
}

// ---- the app-wide handle ----

/// What the shell passes in to use the tools.
#[derive(Clone, Debug)]
pub struct IdeviceConfig {
    pub lookup: ToolLookup,
    /// The app cache: scratch dirs are `<app_cache>/tmp/<id>/`.
    pub app_cache: PathBuf,
    /// Variables added to every tool's environment (empty in the app; tests select fake-idevice
    /// scenarios with it).
    pub env: Vec<(OsString, OsString)>,
}

/// The app-wide device handle: tool lookup (cached), `devices_list`, `device_pair` and the
/// devices this app session paired.
#[derive(Debug)]
pub struct Idevice {
    config: IdeviceConfig,
    tools: Mutex<Option<Arc<IdeviceTools>>>,
    flight: Mutex<Option<Arc<Flight>>>,
    /// Serializes polling and pairing, so their commands never interleave on a device.
    device_ops: Mutex<()>,
    paired_by_app: Mutex<HashMap<String, Timestamp>>,
    /// The last summary of each device, returned for the busy device.
    last_seen: Mutex<HashMap<String, DeviceSummary>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Every value behind these locks is valid at all times, so a poisoned lock is usable.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Idevice {
    pub fn new(config: IdeviceConfig) -> Self {
        Self {
            config,
            tools: Mutex::new(None),
            flight: Mutex::new(None),
            device_ops: Mutex::new(()),
            paired_by_app: Mutex::new(HashMap::new()),
            last_seen: Mutex::new(HashMap::new()),
        }
    }

    pub fn app_cache(&self) -> &Path {
        &self.config.app_cache
    }

    pub fn platform(&self) -> Option<PlatformKey> {
        self.config.lookup.platform
    }

    /// The located tools, verified when first located; later calls reuse them. A failure is not
    /// cached, so installing the tools (Linux) takes effect on the next call.
    pub fn tools(&self) -> Result<Arc<IdeviceTools>, ToolsProblem> {
        if let Some(tools) = lock(&self.tools).as_ref() {
            return Ok(Arc::clone(tools));
        }
        self.verify_tools()
    }

    /// Locates and verifies the tools afresh (before an acquisition, whose record carries the
    /// observed hashes), and caches the result.
    pub fn verify_tools(&self) -> Result<Arc<IdeviceTools>, ToolsProblem> {
        let result = tools::locate(&self.config.lookup).map(|mut tools| {
            tools.env.clone_from(&self.config.env);
            Arc::new(tools)
        });
        *lock(&self.tools) = result.as_ref().ok().map(Arc::clone);
        result
    }

    /// A session with a fresh scratch dir, removed when the session is dropped.
    pub fn session(&self, tools: Arc<IdeviceTools>) -> Result<Session, IdeviceError> {
        Session::scratch(tools, &self.config.app_cache)
    }

    /// When this app session paired `udid`, if it did.
    pub fn paired_by_app_at(&self, udid: &str) -> Option<Timestamp> {
        lock(&self.paired_by_app).get(udid).copied()
    }

    /// How many `devices_list` calls are waiting for the poll in flight (0 when none runs); for
    /// diagnostics and the single-flight test.
    pub fn polls_waiting(&self) -> u32 {
        lock(&self.flight)
            .as_ref()
            .map_or(0, |flight| flight.waiters.load(Ordering::SeqCst))
    }

    /// `devices_list` (ARCHITECTURE.md §6b step 1). Single-flight: a call made while another is
    /// running returns that call's result. It never pairs: `validate` runs only when `hostid`
    /// shows a host record, and `WillEncrypt` and disk usage are read only from paired devices.
    /// `busy` (the active job's device) is not queried; its last summary is returned with
    /// `busy: true`. Tool and usbmuxd problems are reported in `tools`, never as errors, and poll
    /// output is never logged.
    pub fn list_devices(&self, busy: Option<&str>) -> DevicesResult {
        let (flight, leader) = {
            let mut slot = lock(&self.flight);
            match slot.as_ref() {
                Some(flight) => (Arc::clone(flight), false),
                None => {
                    let flight = Arc::new(Flight::default());
                    *slot = Some(Arc::clone(&flight));
                    (flight, true)
                }
            }
        };
        if !leader {
            return flight.wait();
        }
        // Published even if polling panics, so waiters never hang.
        let mut guard = FlightGuard {
            idevice: self,
            flight: &flight,
            result: None,
        };
        let result = self.poll(busy);
        guard.result = Some(result.clone());
        drop(guard);
        result
    }

    fn poll(&self, busy: Option<&str>) -> DevicesResult {
        let _ops = lock(&self.device_ops);
        let tools = match self.tools() {
            Ok(tools) => tools,
            Err(problem) => {
                return DevicesResult {
                    tools: problem.status(),
                    devices: Vec::new(),
                };
            }
        };
        let status = |state, guidance: Option<&str>| IdeviceToolsStatus {
            source: Some(tools.source),
            version: tools.version.clone(),
            state,
            guidance: guidance.map(str::to_owned),
        };
        let session = match self.session(Arc::clone(&tools)) {
            Ok(session) => session,
            Err(err) => {
                return DevicesResult {
                    tools: status(IdeviceToolsState::Ok, Some(&err.message())),
                    devices: Vec::new(),
                };
            }
        };
        let udids = match session.list_udids() {
            Ok(udids) => udids,
            Err(IdeviceError::UsbmuxdUnavailable(_)) => {
                return DevicesResult {
                    tools: status(
                        IdeviceToolsState::UsbmuxdUnavailable,
                        Some(USBMUXD_GUIDANCE),
                    ),
                    devices: Vec::new(),
                };
            }
            Err(err) => {
                return DevicesResult {
                    tools: status(IdeviceToolsState::Ok, Some(&err.message())),
                    devices: Vec::new(),
                };
            }
        };
        let mut devices = Vec::with_capacity(udids.len());
        for udid in &udids {
            if busy == Some(udid.as_str()) {
                let mut summary = lock(&self.last_seen)
                    .get(udid)
                    .cloned()
                    .unwrap_or_else(|| bare_summary(udid, PairState::Unknown, None));
                summary.busy = true;
                devices.push(summary);
            } else {
                let summary = query_device(&session, udid, None);
                lock(&self.last_seen).insert(udid.clone(), summary.clone());
                devices.push(summary);
            }
        }
        DevicesResult {
            tools: status(IdeviceToolsState::Ok, None),
            devices,
        }
    }

    /// `device_pair` (ARCHITECTURE.md §6b step 2): the only code path that pairs. Refused for the
    /// busy device (`device_busy`) and a device that is already paired (`already_paired`); then
    /// runs `idevicepair pair`. Trust and lock outcomes are states in the returned summary, not
    /// errors. A successful pairing is remembered as `paired_by_app_at`.
    pub fn pair(&self, udid: &str, busy: Option<&str>) -> Result<DeviceSummary, IdeviceError> {
        if !is_udid(udid) {
            return Err(IdeviceError::InvalidUdid(udid.to_owned()));
        }
        if busy == Some(udid) {
            return Err(IdeviceError::DeviceBusy(udid.to_owned()));
        }
        let _ops = lock(&self.device_ops);
        let tools = self.tools().map_err(IdeviceError::Tools)?;
        let session = self.session(tools)?;
        if !session.list_udids()?.iter().any(|known| known == udid) {
            return Err(IdeviceError::DeviceNotFound(udid.to_owned()));
        }
        if session.host_id(udid)?.is_some() && session.validate(udid)?.0 == PairState::Paired {
            return Err(IdeviceError::AlreadyPaired(udid.to_owned()));
        }
        let (state, message) = session.pair(udid)?;
        if state == PairState::Paired {
            lock(&self.paired_by_app).insert(udid.to_owned(), Timestamp::now());
        }
        let summary = query_device(&session, udid, Some((state, message)));
        lock(&self.last_seen).insert(udid.to_owned(), summary.clone());
        Ok(summary)
    }
}

/// A summary with only the UDID and a pairing state.
fn bare_summary(udid: &str, pair_state: PairState, message: Option<String>) -> DeviceSummary {
    DeviceSummary {
        udid: udid.to_owned(),
        device_name: None,
        product_type: None,
        product_version: None,
        serial_number: None,
        pair_state,
        busy: false,
        will_encrypt: None,
        data_used_bytes: None,
        data_capacity_bytes: None,
        message,
    }
}

/// Queries one device for `devices_list` or after `device_pair` (whose outcome is `pairing`).
/// Never pairs: without a known pairing state, `validate` runs only when a host record exists, and
/// only a paired device is asked for `WillEncrypt` and disk usage.
fn query_device(
    session: &Session,
    udid: &str,
    pairing: Option<(PairState, Option<String>)>,
) -> DeviceSummary {
    let identity = session.identity(udid);
    let (pair_state, pair_message) = match pairing {
        Some(pairing) => pairing,
        None => match session.pair_state(udid) {
            Ok(pairing) => pairing,
            Err(err) => (PairState::Unknown, Some(err.message())),
        },
    };
    let mut summary = bare_summary(udid, pair_state, None);
    let mut messages: Vec<String> = pair_message.into_iter().collect();
    match identity {
        Some(info) => {
            summary.device_name = info.device_name;
            summary.product_type = info.product_type;
            summary.product_version = info.product_version;
            summary.serial_number = info.serial_number;
        }
        None => messages.push("The device did not report its identity.".to_owned()),
    }
    if pair_state == PairState::Paired {
        summary.will_encrypt = session.will_encrypt(udid);
        if let Some(usage) = session.disk_usage(udid) {
            summary.data_used_bytes = Some(usage.data_used());
            summary.data_capacity_bytes = Some(usage.data_capacity);
        }
    }
    if !messages.is_empty() {
        summary.message = Some(messages.join(" "));
    }
    summary
}

// ---- single-flight ----

#[derive(Debug, Default)]
struct Flight {
    result: Mutex<Option<DevicesResult>>,
    done: Condvar,
    /// Calls waiting for this flight's result.
    waiters: AtomicU32,
}

impl Flight {
    fn wait(&self) -> DevicesResult {
        let mut result = lock(&self.result);
        self.waiters.fetch_add(1, Ordering::SeqCst);
        loop {
            if let Some(result) = result.as_ref() {
                return result.clone();
            }
            result = self
                .done
                .wait(result)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// Publishes the leader's result (or, if polling panicked, an empty one) and ends the flight.
struct FlightGuard<'a> {
    idevice: &'a Idevice,
    flight: &'a Arc<Flight>,
    result: Option<DevicesResult>,
}

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        let result = self.result.take().unwrap_or_else(|| DevicesResult {
            tools: IdeviceToolsStatus {
                source: None,
                version: None,
                state: IdeviceToolsState::Ok,
                guidance: Some("Listing the devices failed unexpectedly.".to_owned()),
            },
            devices: Vec::new(),
        });
        *lock(&self.idevice.flight) = None;
        *lock(&self.flight.result) = Some(result);
        self.flight.done.notify_all();
    }
}

// ---- sessions ----

/// A new id in the acquisition id format, `YYYYMMDD-HHMMSSZ-ios-<6 lowercase hex>` (UTC,
/// CONTRACTS.md §13.3): acquisition ids, and the names of scratch dirs, so that the startup temp
/// sweep recognizes leftovers.
pub fn new_ios_id(at: Timestamp) -> io::Result<String> {
    let mut random = [0u8; 3];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    let t = at.as_datetime();
    Ok(format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}Z-ios-{}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        crate::hashing::to_hex(&random)
    ))
}

/// Where a session's command output goes.
#[derive(Debug)]
enum Scratch {
    /// A fresh `<app_cache>/tmp/<id>/`, removed with the session.
    Owned {
        app_cache: PathBuf,
        id: String,
        dir: PathBuf,
    },
    /// A directory owned by someone else (an acquisition's temp dir).
    Borrowed(PathBuf),
}

impl Scratch {
    fn dir(&self) -> &Path {
        match self {
            Scratch::Owned { dir, .. } | Scratch::Borrowed(dir) => dir,
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Scratch::Owned { app_cache, id, .. } = self
            && let Err(e) = process::remove_temp_dir(app_cache, id)
        {
            log::warn!("cannot remove the scratch dir {id}: {e}");
        }
    }
}

/// The captured result of a short command.
#[derive(Debug)]
pub struct Output {
    pub exit: ExitInfo,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Output {
    fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    fn succeeded(&self) -> bool {
        self.exit.exit_code == Some(0) && !self.exit.timed_out
    }
}

/// A line of a long-running command's output: the stream, the text and, for a device prompt, its
/// kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputLine {
    pub stream: StdStream,
    pub text: String,
    pub prompt: Option<DevicePromptKind>,
}

/// A device-changing command that ran.
#[derive(Clone, Debug)]
pub struct CommandRun {
    /// As recorded: never holds a password.
    pub argv: Vec<String>,
    pub started_at: Timestamp,
    pub exit: ExitInfo,
}

/// Runs tool commands with captured output in a scratch directory.
#[derive(Debug)]
pub struct Session {
    tools: Arc<IdeviceTools>,
    scratch: Scratch,
    counter: AtomicU32,
}

impl Session {
    /// A session with a fresh `<app_cache>/tmp/<id>/`, removed when the session is dropped.
    pub fn scratch(tools: Arc<IdeviceTools>, app_cache: &Path) -> Result<Self, IdeviceError> {
        let mut last = None;
        for _ in 0..SCRATCH_ATTEMPTS {
            let id = new_ios_id(Timestamp::now())
                .map_err(|e| IdeviceError::io("choosing a scratch dir", e))?;
            match process::create_temp_dir(app_cache, &id) {
                Ok(dir) => {
                    return Ok(Self {
                        tools,
                        scratch: Scratch::Owned {
                            app_cache: app_cache.to_path_buf(),
                            id,
                            dir,
                        },
                        counter: AtomicU32::new(0),
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
                Err(e) => return Err(IdeviceError::io("creating a scratch dir", e)),
            }
        }
        Err(IdeviceError::io(
            "creating a scratch dir",
            last.unwrap_or_else(|| io::Error::other("no free scratch dir name")),
        ))
    }

    /// A session writing its output into `dir` (an acquisition's temp dir), which it does not
    /// remove.
    pub fn in_dir(tools: Arc<IdeviceTools>, dir: &Path) -> Self {
        Self {
            tools,
            scratch: Scratch::Borrowed(dir.to_path_buf()),
            counter: AtomicU32::new(0),
        }
    }

    pub fn tools(&self) -> &Arc<IdeviceTools> {
        &self.tools
    }

    /// A spawn spec for `name` with `args`, its logs in the scratch dir.
    fn spec(&self, name: ToolName, args: &[String]) -> (SpawnSpec, PathBuf, PathBuf) {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        let dir = self.scratch.dir();
        let stdout_log = dir.join(format!("idevice-{n}-{}.out", name.as_str()));
        let stderr_log = dir.join(format!("idevice-{n}-{}.err", name.as_str()));
        let tool = self.tools.tool(name);
        let mut spec = SpawnSpec::new(&tool.program, dir, &stdout_log, &stderr_log);
        spec.args = self.tools.args(name, args);
        spec.env.clone_from(&self.tools.env);
        spec.temp_dir = Some(dir.to_path_buf());
        (spec, stdout_log, stderr_log)
    }

    /// Runs a short command (20 s timeout) and returns its output; the log files are deleted.
    pub fn run(&self, name: ToolName, args: &[String]) -> Result<Output, IdeviceError> {
        let (mut spec, stdout_log, stderr_log) = self.spec(name, args);
        spec.timeout = Some(COMMAND_TIMEOUT);
        let result = process::spawn(spec).and_then(|handle| handle.wait());
        let stdout = fs::read(&stdout_log).unwrap_or_default();
        let stderr = fs::read(&stderr_log).unwrap_or_default();
        // The output may identify the device (IMEI, phone number): it does not stay on disk.
        let _ = fs::remove_file(&stdout_log);
        let _ = fs::remove_file(&stderr_log);
        let exit = result.map_err(|e| IdeviceError::io(format!("running {}", name.as_str()), e))?;
        Ok(Output {
            exit,
            stdout,
            stderr,
        })
    }

    /// `idevice_id -l`: the connected USB devices. A failure means usbmuxd could not be reached.
    pub fn list_udids(&self) -> Result<Vec<String>, IdeviceError> {
        let output = self.run(ToolName::IdeviceId, &["-l".to_owned()])?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.succeeded() || stderr.contains(parse::DEVICE_LIST_FAILED) {
            let reason = stderr.lines().next().unwrap_or_default().trim().to_owned();
            return Err(IdeviceError::UsbmuxdUnavailable(if reason.is_empty() {
                format!(
                    "idevice_id -l failed (exit {:?}{})",
                    output.exit.exit_code,
                    if output.exit.timed_out {
                        ", timed out"
                    } else {
                        ""
                    }
                )
            } else {
                reason
            }));
        }
        Ok(parse::udid_list(&output.stdout_text()))
    }

    /// Whether `udid` is connected (listed by `idevice_id -l`); `None` if the list is unavailable.
    pub fn is_connected(&self, udid: &str) -> Option<bool> {
        self.list_udids()
            .ok()
            .map(|udids| udids.iter().any(|known| known == udid))
    }

    fn udid_args(udid: &str, rest: &[&str]) -> Vec<String> {
        ["-u", udid]
            .iter()
            .chain(rest)
            .map(|arg| (*arg).to_owned())
            .collect()
    }

    /// `ideviceinfo -u <udid> -s -x`: the pre-session identity (no pairing needed; fields may be
    /// missing). `None` on failure, including empty output.
    pub fn identity(&self, udid: &str) -> Option<DeviceInfo> {
        let output = self
            .run(ToolName::Ideviceinfo, &Self::udid_args(udid, &["-s", "-x"]))
            .ok()?;
        output
            .succeeded()
            .then(|| parse::device_info(&output.stdout))
            .flatten()
    }

    /// `ideviceinfo -u <udid> -x` on a paired device: the full values, as the plist bytes saved in
    /// `device-info.plist`. `None` on failure, including empty output. Never call it for a device
    /// that is not paired: the handshake would pair it.
    pub fn full_info(&self, udid: &str) -> Option<(Vec<u8>, DeviceInfo)> {
        let output = self
            .run(ToolName::Ideviceinfo, &Self::udid_args(udid, &["-x"]))
            .ok()?;
        if !output.succeeded() {
            return None;
        }
        let info = parse::device_info(&output.stdout)?;
        Some((output.stdout, info))
    }

    /// `idevicepair -u <udid> hostid`: the host's pair record for the device, read through
    /// usbmuxd without a device session. `None` when there is no record.
    pub fn host_id(&self, udid: &str) -> Result<Option<String>, IdeviceError> {
        let output = self.run(ToolName::Idevicepair, &Self::udid_args(udid, &["hostid"]))?;
        let stdout = output.stdout_text();
        if stdout.trim_start().starts_with("No device found") {
            return Err(IdeviceError::DeviceNotFound(udid.to_owned()));
        }
        Ok(parse::host_uuid(&stdout))
    }

    /// `idevicepair systembuid`; `None` when unreadable.
    pub fn system_buid(&self) -> Option<String> {
        let output = self
            .run(ToolName::Idevicepair, &["systembuid".to_owned()])
            .ok()?;
        parse::host_uuid(&output.stdout_text())
    }

    fn pair_command(
        &self,
        udid: &str,
        command: &str,
    ) -> Result<(PairState, Option<String>), IdeviceError> {
        let output = self.run(ToolName::Idevicepair, &Self::udid_args(udid, &[command]))?;
        match parse::pair_answer(&output.stdout_text()) {
            PairAnswer::DeviceNotFound => Err(IdeviceError::DeviceNotFound(udid.to_owned())),
            PairAnswer::State { state, message } => {
                let message = message.or_else(|| {
                    (state != PairState::Paired && output.exit.timed_out).then(|| {
                        format!(
                            "idevicepair {command} did not answer within {} s",
                            COMMAND_TIMEOUT.as_secs()
                        )
                    })
                });
                Ok((state, message))
            }
        }
    }

    /// `idevicepair -u <udid> validate`. Only call it when [`Session::host_id`] found a record:
    /// without one, `validate` pairs.
    pub fn validate(&self, udid: &str) -> Result<(PairState, Option<String>), IdeviceError> {
        self.pair_command(udid, "validate")
    }

    /// `idevicepair -u <udid> pair`: shows the Trust dialog on the device. Only
    /// [`Idevice::pair`] calls it.
    fn pair(&self, udid: &str) -> Result<(PairState, Option<String>), IdeviceError> {
        self.pair_command(udid, "pair")
    }

    /// The pairing state without ever pairing: `hostid` first, `validate` only when a host record
    /// exists (ARCHITECTURE.md §6b step 1).
    pub fn pair_state(&self, udid: &str) -> Result<(PairState, Option<String>), IdeviceError> {
        match self.host_id(udid)? {
            None => Ok((PairState::NotPaired, None)),
            Some(_) => self.validate(udid),
        }
    }

    /// `WillEncrypt` of a paired device, from the `com.apple.mobile.backup` domain: a readable
    /// domain without the key means `false`, as `idevicebackup2` assumes; a failed read (a tool
    /// error, or empty output) is `None`. (`-k WillEncrypt` prints nothing both for an absent key
    /// and for a failed read, so the whole domain is read.)
    pub fn will_encrypt(&self, udid: &str) -> Option<bool> {
        let args = Self::udid_args(udid, &["-q", "com.apple.mobile.backup", "-x"]);
        let output = self.run(ToolName::Ideviceinfo, &args).ok()?;
        output
            .succeeded()
            .then(|| parse::will_encrypt(&output.stdout))
            .flatten()
    }

    /// The disk usage of a paired device; `None` when unreadable.
    pub fn disk_usage(&self, udid: &str) -> Option<DiskUsage> {
        let args = Self::udid_args(udid, &["-q", "com.apple.disk_usage", "-x"]);
        let output = self.run(ToolName::Ideviceinfo, &args).ok()?;
        output
            .succeeded()
            .then(|| parse::disk_usage(&output.stdout))
            .flatten()
    }

    /// `idevicebackup2 -u <udid> encryption on|off` with the password in `BACKUP_PASSWORD_NEW` (on)
    /// or `BACKUP_PASSWORD` (off), never in argv. There is no timeout: on iOS ≥ 13 with a passcode
    /// the tool waits for it to be entered on the device. Every output line goes to `on_line` (on
    /// this thread, while the command runs), with prompt lines marked. The caller re-reads
    /// `WillEncrypt` to learn the outcome.
    pub fn set_encryption(
        &self,
        udid: &str,
        enable: bool,
        password: &Password,
        on_line: &mut dyn FnMut(OutputLine),
    ) -> Result<CommandRun, IdeviceError> {
        // Callers refuse such passwords before any device change; this is the last guard.
        if let Some(problem) = password.tool_problem() {
            return Err(IdeviceError::PasswordNotSupported(problem));
        }
        let args = Self::udid_args(udid, &["encryption", if enable { "on" } else { "off" }]);
        let argv = self.tools.argv(ToolName::Idevicebackup2, &args);
        let (mut spec, stdout_log, stderr_log) = self.spec(ToolName::Idevicebackup2, &args);
        let variable = if enable {
            "BACKUP_PASSWORD_NEW"
        } else {
            "BACKUP_PASSWORD"
        };
        spec.env
            .push((OsString::from(variable), password.env_value()));
        spec.timeout = None;
        let lines = LineChannel::new();
        spec.on_output = Some(lines.callback());
        let started_at = Timestamp::now();
        let result = process::spawn(spec).and_then(|handle| lines.pump(&handle, on_line));
        let _ = fs::remove_file(&stdout_log);
        let _ = fs::remove_file(&stderr_log);
        let exit = result.map_err(|e| IdeviceError::io("running idevicebackup2 encryption", e))?;
        Ok(CommandRun {
            argv,
            started_at,
            exit,
        })
    }
}

/// Carries output lines from the process reader threads to the thread that waits for the
/// process, so callbacks run on the caller's thread while the process runs.
struct LineChannel {
    splitters: Arc<Mutex<[LineSplitter; 2]>>,
    sender: std::sync::mpsc::Sender<OutputLine>,
    receiver: std::sync::mpsc::Receiver<OutputLine>,
}

/// How often a waiting thread delivers the lines that arrived.
const LINE_PUMP_INTERVAL: Duration = Duration::from_millis(50);

impl LineChannel {
    fn new() -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        Self {
            splitters: Arc::new(Mutex::new([
                LineSplitter::default(),
                LineSplitter::default(),
            ])),
            sender,
            receiver,
        }
    }

    fn callback(&self) -> process::OutputCallback {
        let splitters = Arc::clone(&self.splitters);
        let sender = self.sender.clone();
        Box::new(move |stream, chunk| {
            let mut splitters = lock(&splitters);
            for text in splitters[usize::from(stream == StdStream::Stderr)].push(chunk) {
                let prompt = parse::prompt_kind(&text);
                // The receiver lives until the process is gone.
                let _ = sender.send(OutputLine {
                    stream,
                    text,
                    prompt,
                });
            }
        })
    }

    /// Waits for the process, delivering lines as they arrive, then the unterminated last ones.
    fn pump(
        &self,
        handle: &process::Handle,
        on_line: &mut dyn FnMut(OutputLine),
    ) -> io::Result<ExitInfo> {
        let exit = loop {
            for line in self.receiver.try_iter() {
                on_line(line);
            }
            if let Some(exit) = handle.wait_timeout(LINE_PUMP_INTERVAL)? {
                break exit;
            }
        };
        // The readers are done once `wait` returns: everything is in the channel or a splitter.
        for line in self.receiver.try_iter() {
            on_line(line);
        }
        let mut splitters = lock(&self.splitters);
        for (stream, splitter) in [StdStream::Stdout, StdStream::Stderr]
            .into_iter()
            .zip(splitters.iter_mut())
        {
            if let Some(text) = splitter.finish() {
                let prompt = parse::prompt_kind(&text);
                on_line(OutputLine {
                    stream,
                    text,
                    prompt,
                });
            }
        }
        Ok(exit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_debug_is_redacted_and_bytes_are_cleared() {
        let password = Password::new("hunter22".to_owned());
        assert_eq!(format!("{password:?}"), "Password(<redacted>)");
        assert_eq!(password.chars(), 8);
        assert!(password.is_in("xx hunter22 yy"));
        assert!(!password.is_in("hunter2"));
        assert_eq!(password.env_value(), OsString::from("hunter22"));
        assert_eq!(Password::new("pässwörd".to_owned()).chars(), 8);
        let mut bytes = Password::new("secret".to_owned());
        // What drop does, observed before the memory is freed.
        bytes.0.fill(0);
        assert!(bytes.0.iter().all(|&b| b == 0));
    }

    #[test]
    fn windows_passwords_must_be_printable_ascii() {
        for ok in ["hunter22", "Tr0ub4dor & 3!", " ~spaces and tildes~ "] {
            assert_eq!(password_problem(ok.as_bytes(), true), None, "{ok:?}");
        }
        for bad in [
            "pässwört",
            "密码密码",
            "tab\there",
            "new\nline",
            "euro€1234",
        ] {
            assert!(password_problem(bad.as_bytes(), true).is_some(), "{bad:?}");
            // Elsewhere the tools get the bytes as they are.
            assert_eq!(password_problem(bad.as_bytes(), false), None, "{bad:?}");
        }
        let password = Password::new("pässwört".to_owned());
        assert_eq!(password.tool_problem().is_some(), cfg!(windows));
        let err = IdeviceError::PasswordNotSupported("x");
        assert_eq!(err.code(), ErrorCode::InvalidInput);
        assert!(
            err.message().contains("ANSI code page"),
            "{}",
            err.message()
        );
    }

    #[test]
    fn error_codes() {
        let problem = ToolsProblem {
            state: IdeviceToolsState::VerificationFailed,
            source: None,
            version: None,
            detail: "d".to_owned(),
            guidance: "g".to_owned(),
        };
        for (err, code) in [
            (
                IdeviceError::InvalidUdid("x".into()),
                ErrorCode::DeviceNotFound,
            ),
            (IdeviceError::DeviceBusy("u".into()), ErrorCode::DeviceBusy),
            (
                IdeviceError::AlreadyPaired("u".into()),
                ErrorCode::AlreadyPaired,
            ),
            (
                IdeviceError::DeviceNotFound("u".into()),
                ErrorCode::DeviceNotFound,
            ),
            (
                IdeviceError::Tools(problem),
                ErrorCode::IdeviceToolsVerificationFailed,
            ),
            (
                IdeviceError::UsbmuxdUnavailable("x".into()),
                ErrorCode::UsbmuxdUnavailable,
            ),
            (
                IdeviceError::io("x", io::Error::from(io::ErrorKind::PermissionDenied)),
                ErrorCode::PermissionDenied,
            ),
        ] {
            assert_eq!(err.code(), code, "{err}");
            let app: AppError = err.into();
            assert_eq!(app.code, code);
            assert!(app.detail.is_some());
        }
    }

    #[test]
    fn usbmuxd_guidance_covers_the_service_and_other_failures() {
        let err = IdeviceError::UsbmuxdUnavailable(parse::DEVICE_LIST_FAILED.to_owned());
        let message = err.message();
        assert!(message.contains("Apple Mobile Device Service"), "{message}");
        assert!(message.contains("usbmuxd"), "{message}");
        assert!(message.contains("running"), "{message}");
    }
}
