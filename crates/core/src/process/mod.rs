//! Spawning tools in their own process tree, with stdin null and stdout/stderr streamed to log
//! files; cancel with escalation; `wait` returning the exit info; per-job temp directories
//! (ARCHITECTURE.md §7, D9, D10).
//!
//! **The tree.** A LEAPP onefile binary runs as two processes (LEAPP-CLI.md Q2), so a spawn owns a
//! whole process tree, and nothing of it survives [`Handle::wait`]:
//! - Unix (`unix.rs`): the child starts a new session (`setsid`), so its process group id is its
//!   pid and it has no controlling terminal. Stopping sends SIGTERM to the group, waits up to the
//!   spec's kill grace, then sends SIGKILL. The leader is reaped, and then the group is polled until
//!   it is gone (up to 2 s).
//! - Windows (`windows.rs`): the child is created suspended and without a console window, assigned
//!   to a job object with kill-on-close, then resumed. Stopping terminates the job at once; there
//!   is no graceful signal.
//!
//! The tree is stopped on [`Handle::cancel`], when the spec's timeout elapses, and also when the
//! leader exits while other members of the tree are still running.
//!
//! **Output.** stdout and stderr are read by two threads that write them to the spec's log files
//! and pass every chunk to the optional callback. stdin is null.
//!
//! **Temp dirs.** [`create_temp_dir`] makes `<app_cache>/tmp/<id>/`; a spec's `temp_dir` points
//! `TMPDIR`, `TEMP` and `TMP` at it; [`remove_temp_dir`] removes it after the job (retrying
//! Windows sharing violations for up to 5 s); [`sweep_stale_temp`] removes leftovers.

use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::contracts::{StdStream, Timestamp};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as sys;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as sys;

/// The default time between SIGTERM and SIGKILL (Unix), as used for LEAPP runs.
pub const DEFAULT_KILL_GRACE: Duration = Duration::from_secs(10);

/// How often the supervisor checks for the leader's exit and the tree's state.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
/// How long to wait for the rest of the tree after the leader was reaped (ARCHITECTURE.md §7).
const TREE_GONE_TIMEOUT: Duration = Duration::from_secs(2);
/// How long to wait for stdout/stderr to reach end-of-file once the tree is gone.
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
/// How long [`remove_temp_dir`] retries transient Windows errors (sharing violations).
const TEMP_REMOVE_RETRY: Duration = Duration::from_secs(5);
const TEMP_REMOVE_INTERVAL: Duration = Duration::from_millis(100);
/// The variables that point a spawned tool at its temp dir.
const TEMP_VARS: [&str; 3] = ["TMPDIR", "TEMP", "TMP"];
const READ_BUFFER: usize = 64 * 1024;

/// Receives every stdout/stderr chunk as it is read, in addition to the log files. Calls are
/// serialized (never concurrent). A callback that panics is not called again.
pub type OutputCallback = Box<dyn FnMut(StdStream, &[u8]) + Send>;

/// What to run and how.
pub struct SpawnSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// The working directory (never a `\\?\` path on Windows).
    pub cwd: PathBuf,
    /// Variables added to the inherited environment.
    pub env: Vec<(OsString, OsString)>,
    /// The job's temp dir: `TMPDIR`, `TEMP` and `TMP` are set to it. It must exist (see
    /// [`create_temp_dir`]); the caller removes it after [`Handle::wait`].
    pub temp_dir: Option<PathBuf>,
    /// Created by `spawn`, which fails if the file already exists.
    pub stdout_log: PathBuf,
    /// Created by `spawn`, which fails if the file already exists.
    pub stderr_log: PathBuf,
    /// Unix: how long the tree gets between SIGTERM and SIGKILL. Unused on Windows.
    pub kill_grace: Duration,
    /// Stop the tree if it is still running after this long (`None` = no timeout).
    pub timeout: Option<Duration>,
    pub on_output: Option<OutputCallback>,
}

impl SpawnSpec {
    /// A spec with no arguments or extra env, no temp dir, the default kill grace, no timeout and
    /// no output callback.
    pub fn new(
        program: impl Into<PathBuf>,
        cwd: impl Into<PathBuf>,
        stdout_log: impl Into<PathBuf>,
        stderr_log: impl Into<PathBuf>,
    ) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            env: Vec::new(),
            temp_dir: None,
            stdout_log: stdout_log.into(),
            stderr_log: stderr_log.into(),
            kill_grace: DEFAULT_KILL_GRACE,
            timeout: None,
            on_output: None,
        }
    }
}

/// Written by hand: `args` may hold a backup password (`--itunes_password`) and `env` may hold
/// `BACKUP_PASSWORD`, so only the argument count and the variable names are shown.
impl fmt::Debug for SpawnSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpawnSpec")
            .field("program", &self.program)
            .field("args", &format_args!("<{} arguments>", self.args.len()))
            .field("cwd", &self.cwd)
            .field(
                "env",
                &self.env.iter().map(|(name, _)| name).collect::<Vec<_>>(),
            )
            .field("temp_dir", &self.temp_dir)
            .field("stdout_log", &self.stdout_log)
            .field("stderr_log", &self.stderr_log)
            .field("kill_grace", &self.kill_grace)
            .field("timeout", &self.timeout)
            .field("on_output", &self.on_output.is_some())
            .finish()
    }
}

/// How a spawned tree ended. Returned once the whole tree is gone and the log files are complete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitInfo {
    /// The leader's exit code; `None` when a signal killed it (Unix).
    pub exit_code: Option<i32>,
    /// The signal that killed the leader (Unix), else `None`.
    pub signal: Option<i32>,
    /// When the leader's exit was observed.
    pub exited_at: Timestamp,
    /// The same moment on the monotonic clock.
    pub exit_instant: Instant,
    /// [`Handle::cancel`] was called before the exit was observed.
    pub cancel_requested: bool,
    /// The spec's timeout elapsed before the exit was observed.
    pub timed_out: bool,
    /// Unix: part of the tree outlived SIGTERM for the whole kill grace and got SIGKILL. Always
    /// `false` on Windows, where terminating the job is the only way to stop the tree.
    pub escalated_to_kill: bool,
    /// Reading stdout/stderr or writing a log file failed, so a log may be incomplete.
    pub output_error: Option<String>,
}

/// A running (or finished) process tree. `Send + Sync`: share it (e.g. in an `Arc`) to cancel
/// from one thread while another waits.
#[derive(Debug)]
pub struct Handle {
    pid: u32,
    stdout_log: PathBuf,
    stderr_log: PathBuf,
    shared: Arc<Shared>,
}

impl Handle {
    /// The leader's process id (Unix: also the process group id).
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn stdout_log(&self) -> &Path {
        &self.stdout_log
    }

    pub fn stderr_log(&self) -> &Path {
        &self.stderr_log
    }

    /// Asks the supervisor to stop the tree (SIGTERM, then SIGKILL after the grace on Unix; job
    /// termination on Windows). Returns at once; idempotent; ignored once the exit was observed.
    pub fn cancel(&self) {
        self.shared.lock().cancel = true;
        self.shared.changed.notify_all();
    }

    /// Blocks until the whole tree is gone and the log files are complete.
    pub fn wait(&self) -> io::Result<ExitInfo> {
        let mut state = self.shared.lock();
        loop {
            if let Some(result) = &state.result {
                return result.clone().map_err(SharedError::into_io);
            }
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Like [`Handle::wait`], but returns `Ok(None)` if the tree is still running after `timeout`.
    pub fn wait_timeout(&self, timeout: Duration) -> io::Result<Option<ExitInfo>> {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.lock();
        loop {
            if let Some(result) = &state.result {
                return result.clone().map(Some).map_err(SharedError::into_io);
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            state = self
                .shared
                .changed
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// Whether a process with this id exists and has not exited (a zombie counts as exited on
/// Linux). For tests and diagnostics.
pub fn pid_alive(pid: u32) -> bool {
    sys::pid_alive(pid)
}

/// Spawns `spec.program` in its own process tree (see the module docs). The log files are created
/// first; a spawn error leaves them empty.
pub fn spawn(spec: SpawnSpec) -> io::Result<Handle> {
    let SpawnSpec {
        program,
        args,
        cwd,
        env,
        temp_dir,
        stdout_log,
        stderr_log,
        kill_grace,
        timeout,
        on_output,
    } = spec;
    let stdout_file = create_log(&stdout_log)?;
    let stderr_file = create_log(&stderr_log)?;
    let (stdout_read, stdout_write) = io::pipe()?;
    let (stderr_read, stderr_write) = io::pipe()?;

    // Every thread starts before the child does, so no failure after the spawn can leave a process
    // without a supervisor. Until the child exists, the threads only wait.
    let callback = on_output.map(|callback| Arc::new(Mutex::new(Some(callback))));
    let (done_tx, done_rx) = mpsc::channel();
    start_reader(
        stdout_read,
        stdout_file,
        StdStream::Stdout,
        callback.clone(),
        done_tx.clone(),
    )?;
    start_reader(
        stderr_read,
        stderr_file,
        StdStream::Stderr,
        callback,
        done_tx,
    )?;
    let shared = Arc::new(Shared::default());
    let (start_tx, start_rx) = mpsc::channel::<Started>();
    let publisher = Publisher(Arc::clone(&shared));
    thread::Builder::new()
        .name("process-supervisor".to_owned())
        .spawn(move || {
            // No child to supervise if the spawn failed (the sender is dropped then).
            if let Ok(started) = start_rx.recv() {
                let result = supervise(started, &publisher.0, done_rx, kill_grace, timeout);
                publisher.publish(result.map_err(|e| SharedError::from_io(&e)));
            }
        })?;

    let mut command = Command::new(&program);
    command
        .args(&args)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_write))
        .stderr(Stdio::from(stderr_write));
    for (name, value) in &env {
        command.env(name, value);
    }
    if let Some(dir) = &temp_dir {
        for name in TEMP_VARS {
            command.env(name, dir);
        }
    }
    sys::configure(&mut command);
    let spawned = command.spawn();
    // Close this process's copies of the pipe write ends, so the readers see end-of-file once
    // the tree has exited (or right away if the spawn failed).
    drop(command);
    let mut child = spawned?;
    let pid = child.id();
    // On failure, `attach` has already stopped the child.
    let tree = sys::Tree::attach(&mut child)?;
    if let Err(mpsc::SendError(started)) = start_tx.send(Started { child, tree }) {
        // The supervisor thread is gone (it can only have panicked): stop the tree here.
        let Started { mut child, tree } = started;
        let _ = tree.kill();
        let _ = child.wait();
        return Err(io::Error::other("the process supervisor thread stopped"));
    }
    Ok(Handle {
        pid,
        stdout_log,
        stderr_log,
        shared,
    })
}

fn create_log(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| io::Error::new(e.kind(), format!("cannot create {}: {e}", path.display())))
}

// ---- supervisor ----

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Default)]
struct State {
    cancel: bool,
    result: Option<Result<ExitInfo, SharedError>>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        // The state is two plain fields that are always consistent, so a poisoned lock is usable.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn cancel_requested(&self) -> bool {
        self.lock().cancel
    }

    /// Sleeps up to `timeout`, waking early when a cancel is requested.
    fn sleep_unless_cancelled(&self, timeout: Duration) {
        let state = self.lock();
        if !state.cancel {
            let _unused = self
                .changed
                .wait_timeout(state, timeout)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl fmt::Debug for Shared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = self.lock();
        f.debug_struct("Shared")
            .field("cancel", &state.cancel)
            .field("finished", &state.result.is_some())
            .finish()
    }
}

/// An `io::Error` that can be handed to every waiter.
#[derive(Clone, Debug)]
struct SharedError {
    kind: io::ErrorKind,
    message: String,
}

impl SharedError {
    fn from_io(error: &io::Error) -> Self {
        Self {
            kind: error.kind(),
            message: error.to_string(),
        }
    }

    fn into_io(self) -> io::Error {
        io::Error::new(self.kind, self.message)
    }
}

/// Publishes the supervisor's result. Dropping it unpublished (a panic) publishes an error, so
/// waiters never hang.
struct Publisher(Arc<Shared>);

impl Publisher {
    fn publish(&self, result: Result<ExitInfo, SharedError>) {
        let mut state = self.0.lock();
        if state.result.is_none() {
            state.result = Some(result);
        }
        drop(state);
        self.0.changed.notify_all();
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        self.publish(Err(SharedError {
            kind: io::ErrorKind::Other,
            message: "the process supervisor stopped unexpectedly".to_owned(),
        }));
    }
}

struct Started {
    child: Child,
    tree: sys::Tree,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stop {
    Cancel,
    Timeout,
}

/// When the leader's exit was observed.
#[derive(Clone, Copy)]
struct Observed {
    at: Timestamp,
    instant: Instant,
}

impl Observed {
    fn now() -> Self {
        Self {
            at: Timestamp::now(),
            instant: Instant::now(),
        }
    }
}

fn supervise(
    started: Started,
    shared: &Shared,
    readers: Receiver<Option<String>>,
    kill_grace: Duration,
    timeout: Option<Duration>,
) -> io::Result<ExitInfo> {
    let Started { mut child, tree } = started;
    let result = supervise_tree(&mut child, &tree, shared, readers, kill_grace, timeout);
    if result.is_err() {
        // Never leave a tree behind, whatever failed.
        let _ = tree.kill();
        let _ = child.wait();
    }
    result
}

fn supervise_tree(
    child: &mut Child,
    tree: &sys::Tree,
    shared: &Shared,
    readers: Receiver<Option<String>>,
    kill_grace: Duration,
    timeout: Option<Duration>,
) -> io::Result<ExitInfo> {
    let deadline = timeout.map(|timeout| Instant::now() + timeout);
    // 1. Wait for the leader to exit, a cancel or the timeout.
    let mut exited: Option<(ExitStatus, Observed)> = None;
    let mut stop = None;
    while exited.is_none() && stop.is_none() {
        if let Some(status) = child.try_wait()? {
            exited = Some((status, Observed::now()));
        } else if shared.cancel_requested() {
            stop = Some(Stop::Cancel);
        } else if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            stop = Some(Stop::Timeout);
        } else {
            shared.sleep_unless_cancelled(POLL_INTERVAL);
        }
    }

    // 2. Stop the tree when asked to, or when members outlived the leader.
    let mut escalated_to_kill = false;
    if stop.is_some() || tree.is_alive() {
        escalated_to_kill = stop_tree(child, tree, &mut exited, kill_grace)?;
    }

    // 3. Reap the leader, then make sure the rest of the tree is gone.
    let (status, observed) = match exited {
        Some(exited) => exited,
        None => (child.wait()?, Observed::now()),
    };
    if !poll_until(TREE_GONE_TIMEOUT, || !tree.is_alive()) {
        log::warn!(
            "part of the process tree of pid {} was still present {} s after the leader exited",
            child.id(),
            TREE_GONE_TIMEOUT.as_secs()
        );
    }

    // 4. The readers reach end-of-file once no process holds the pipes any more.
    let output_error = collect_readers(&readers, OUTPUT_DRAIN_TIMEOUT);
    let (exit_code, signal) = sys::exit_parts(status);
    Ok(ExitInfo {
        exit_code,
        signal,
        exited_at: observed.at,
        exit_instant: observed.instant,
        cancel_requested: stop == Some(Stop::Cancel),
        timed_out: stop == Some(Stop::Timeout),
        escalated_to_kill,
        output_error,
    })
}

/// Unix: SIGTERM to the group, then SIGKILL if anything of the tree is still there after the
/// grace. Windows: terminates the job. Records the leader's exit if it is observed on the way.
/// Returns whether SIGKILL was needed.
fn stop_tree(
    child: &mut Child,
    tree: &sys::Tree,
    exited: &mut Option<(ExitStatus, Observed)>,
    kill_grace: Duration,
) -> io::Result<bool> {
    tree.terminate()?;
    if !sys::GRACEFUL_TERMINATE {
        return Ok(false);
    }
    let deadline = Instant::now() + kill_grace;
    loop {
        if exited.is_none()
            && let Some(status) = child.try_wait()?
        {
            *exited = Some((status, Observed::now()));
        }
        if exited.is_some() && !tree.is_alive() {
            return Ok(false);
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(POLL_INTERVAL);
    }
    tree.kill()?;
    Ok(true)
}

/// Polls `done` until it returns true or `timeout` elapses; returns its last answer.
fn poll_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Waits for both reader threads and returns the first problem they reported.
fn collect_readers(readers: &Receiver<Option<String>>, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    let mut error = None;
    for _ in 0..2 {
        let left = deadline.saturating_duration_since(Instant::now());
        match readers.recv_timeout(left) {
            Ok(problem) => error = error.or(problem),
            Err(_) => {
                return Some(error.unwrap_or_else(|| {
                    format!(
                        "stdout/stderr were still open {} s after the process tree exited; \
                             the log files may be incomplete",
                        timeout.as_secs()
                    )
                }));
            }
        }
    }
    error
}

// ---- output readers ----

type SharedCallback = Arc<Mutex<Option<OutputCallback>>>;

fn start_reader(
    pipe: io::PipeReader,
    log: File,
    stream: StdStream,
    callback: Option<SharedCallback>,
    done: Sender<Option<String>>,
) -> io::Result<()> {
    thread::Builder::new()
        .name(format!("process-{stream}"))
        .spawn(move || {
            let problem = pump(pipe, log, stream, callback.as_deref());
            // The supervisor may have given up waiting; nothing else to do then.
            let _ = done.send(problem);
        })?;
    Ok(())
}

/// Copies `pipe` to `log` and the callback until end-of-file. It keeps reading after a failed
/// write, so the child never blocks on a full pipe. Returns the first problem.
fn pump(
    mut pipe: impl Read,
    mut log: impl Write,
    stream: StdStream,
    callback: Option<&Mutex<Option<OutputCallback>>>,
) -> Option<String> {
    let mut buffer = vec![0u8; READ_BUFFER];
    let mut problem = None;
    loop {
        let read = match pipe.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                problem.get_or_insert_with(|| format!("reading {stream} failed: {e}"));
                break;
            }
        };
        let chunk = &buffer[..read];
        if problem.is_none()
            && let Err(e) = log.write_all(chunk)
        {
            problem = Some(format!("writing the {stream} log failed: {e}"));
        }
        if let Some(callback) = callback {
            call(callback, stream, chunk);
        }
    }
    if problem.is_none()
        && let Err(e) = log.flush()
    {
        problem = Some(format!("writing the {stream} log failed: {e}"));
    }
    problem
}

fn call(callback: &Mutex<Option<OutputCallback>>, stream: StdStream, chunk: &[u8]) {
    let mut slot = callback.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(function) = slot.as_mut()
        && panic::catch_unwind(AssertUnwindSafe(|| function(stream, chunk))).is_err()
    {
        log::error!("the {stream} output callback panicked; it is no longer called");
        *slot = None;
    }
}

// ---- temp dirs ----

/// What [`sweep_stale_temp`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TempSweep {
    /// Entries of `<app_cache>/tmp/` removed.
    pub removed: u64,
    /// Bytes of the removed entries.
    pub freed_bytes: u64,
    /// Entries that could not be removed (each is logged).
    pub failed: Vec<PathBuf>,
}

/// `<app_cache>/tmp/<id>`, the temp dir of job `id` (a run or acquisition id). `id` must be a
/// single plain path component.
pub fn temp_dir_path(app_cache: &Path, id: &str) -> io::Result<PathBuf> {
    let mut components = Path::new(id).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) if !id.contains(['/', '\\']) => {
            Ok(app_cache.join("tmp").join(id))
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{id:?} is not a valid temp dir name"),
        )),
    }
}

/// Creates the temp dir of job `id` ([`temp_dir_path`]); it must not exist yet.
pub fn create_temp_dir(app_cache: &Path, id: &str) -> io::Result<PathBuf> {
    let dir = temp_dir_path(app_cache, id)?;
    if let Some(parent) = dir.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir(&dir)?;
    Ok(dir)
}

/// Removes a temp dir and everything in it without following symlinks. A missing dir is fine.
/// On Windows, sharing violations and similar transient errors (files of a just-terminated
/// process) are retried for up to 5 s.
pub fn remove_temp_dir(dir: &Path) -> io::Result<()> {
    let deadline = Instant::now() + TEMP_REMOVE_RETRY;
    loop {
        match remove_entry(dir) {
            Ok(()) => return Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) if sys::is_transient_removal_error(&e) && Instant::now() < deadline => {
                thread::sleep(TEMP_REMOVE_INTERVAL);
            }
            Err(e) => return Err(e),
        }
    }
}

/// Removes a file, a symlink (never its target) or a directory tree.
fn remove_entry(path: &Path) -> io::Result<()> {
    let file_type = fs::symlink_metadata(path)?.file_type();
    if file_type.is_dir() || file_type.is_symlink() {
        // Removes a symlink itself rather than following it.
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// Removes every entry of `<app_cache>/tmp/`: temp dirs left by hard kills or crashes. Call it
/// only when no job is running (at startup, after the instance lock is held, or from
/// `temp_cleanup`).
pub fn sweep_stale_temp(app_cache: &Path) -> io::Result<TempSweep> {
    let root = app_cache.join("tmp");
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(TempSweep::default()),
        Err(e) => return Err(e),
    };
    let mut sweep = TempSweep::default();
    for entry in entries {
        let path = entry?.path();
        let size = tree_size(&path);
        match remove_temp_dir(&path) {
            Ok(()) => {
                sweep.removed += 1;
                sweep.freed_bytes = sweep.freed_bytes.saturating_add(size);
            }
            Err(e) => {
                log::warn!("cannot remove stale temp entry {}: {e}", path.display());
                sweep.failed.push(path);
            }
        }
    }
    Ok(sweep)
}

/// The bytes under `path`, without following symlinks (a symlink counts as its own size).
/// Unreadable parts are skipped.
fn tree_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let mut pending = vec![path.to_path_buf()];
    while let Some(path) = pending.pop() {
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            if let Ok(entries) = fs::read_dir(&path) {
                pending.extend(entries.flatten().map(|entry| entry.path()));
            }
        } else {
            total = total.saturating_add(metadata.len());
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_dir_names_are_single_components() {
        let cache = Path::new("cache");
        assert_eq!(
            temp_dir_path(cache, "20260924-183005Z-ileapp-3f9a1c").unwrap(),
            cache.join("tmp").join("20260924-183005Z-ileapp-3f9a1c")
        );
        for bad in ["", ".", "..", "a/b", "a\\b", "/abs"] {
            let error = temp_dir_path(cache, bad).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{bad:?}");
        }
    }

    #[test]
    fn create_and_remove_temp_dir() {
        let cache = tempfile::tempdir().unwrap();
        let dir = create_temp_dir(cache.path(), "run1").unwrap();
        assert_eq!(dir, cache.path().join("tmp").join("run1"));
        assert!(dir.is_dir());
        // An existing dir is never reused.
        assert_eq!(
            create_temp_dir(cache.path(), "run1").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        fs::create_dir_all(dir.join("_MEIfake1").join("nested")).unwrap();
        fs::write(dir.join("_MEIfake1").join("nested").join("f"), "x").unwrap();
        remove_temp_dir(&dir).unwrap();
        assert!(!dir.exists());
        // Missing is fine.
        remove_temp_dir(&dir).unwrap();
    }

    #[test]
    fn sweep_removes_every_entry_and_counts_bytes() {
        let cache = tempfile::tempdir().unwrap();
        assert_eq!(
            sweep_stale_temp(cache.path()).unwrap(),
            TempSweep::default()
        );
        let a = create_temp_dir(cache.path(), "a").unwrap();
        let b = create_temp_dir(cache.path(), "b").unwrap();
        fs::write(a.join("one"), [0u8; 100]).unwrap();
        fs::create_dir(b.join("sub")).unwrap();
        fs::write(b.join("sub").join("two"), [0u8; 50]).unwrap();
        fs::write(cache.path().join("tmp").join("stray-file"), [0u8; 7]).unwrap();

        let sweep = sweep_stale_temp(cache.path()).unwrap();
        assert_eq!(sweep.removed, 3);
        assert_eq!(sweep.freed_bytes, 157);
        assert!(sweep.failed.is_empty());
        assert_eq!(fs::read_dir(cache.path().join("tmp")).unwrap().count(), 0);
    }

    #[test]
    fn sweep_does_not_follow_symlinks() {
        let cache = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("keep"), "evidence").unwrap();
        let dir = create_temp_dir(cache.path(), "run").unwrap();
        if let Err(e) = crate::fsutil::test_support::symlink_dir(outside.path(), &dir.join("link"))
        {
            if cfg!(windows) && e.raw_os_error() == Some(1314) {
                eprintln!("SKIPPED: creating symlinks needs Developer Mode or admin (error 1314)");
                return;
            }
            panic!("cannot create a symlink: {e}");
        }
        sweep_stale_temp(cache.path()).unwrap();
        assert!(!dir.exists());
        assert_eq!(
            fs::read_to_string(outside.path().join("keep")).unwrap(),
            "evidence"
        );
    }

    #[test]
    fn spec_debug_hides_arguments_and_env_values() {
        let mut spec = SpawnSpec::new("/bin/tool", "/run", "/run/out.log", "/run/err.log");
        spec.args = vec!["--itunes_password".into(), "hunter2".into()];
        spec.env = vec![("BACKUP_PASSWORD".into(), "hunter3".into())];
        let text = format!("{spec:?}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(!text.contains("hunter3"), "{text}");
        assert!(text.contains("<2 arguments>"), "{text}");
        assert!(text.contains("BACKUP_PASSWORD"), "{text}");
        assert_eq!(spec.kill_grace, DEFAULT_KILL_GRACE);
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("disk full"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn pump_copies_to_the_log_and_the_callback() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        let callback: Mutex<Option<OutputCallback>> =
            Mutex::new(Some(Box::new(move |stream, chunk: &[u8]| {
                sink.lock().unwrap().push((stream, chunk.to_vec()));
            })));
        let mut log = Vec::new();
        let input = vec![b'x'; READ_BUFFER + 10];
        let problem = pump(&input[..], &mut log, StdStream::Stderr, Some(&callback));
        assert_eq!(problem, None);
        assert_eq!(log, input);
        let seen = seen.lock().unwrap();
        assert!(seen.iter().all(|(stream, _)| *stream == StdStream::Stderr));
        assert_eq!(
            seen.iter().map(|(_, c)| c.len()).sum::<usize>(),
            input.len()
        );
    }

    #[test]
    fn pump_keeps_draining_after_a_write_error_and_survives_a_panicking_callback() {
        let calls = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&calls);
        let callback: Mutex<Option<OutputCallback>> = Mutex::new(Some(Box::new(move |_, _| {
            *counter.lock().unwrap() += 1;
            panic!("callback bug");
        })));
        let input = vec![b'y'; 3 * READ_BUFFER];
        let mut reader = &input[..];
        let problem = pump(
            &mut reader,
            FailingWriter,
            StdStream::Stdout,
            Some(&callback),
        );
        assert_eq!(
            problem.as_deref(),
            Some("writing the stdout log failed: disk full")
        );
        // Everything was read, and the panicking callback was called once, then dropped.
        assert!(reader.is_empty());
        assert_eq!(*calls.lock().unwrap(), 1);
        assert!(callback.lock().unwrap().is_none());
    }

    #[test]
    fn collect_readers_reports_the_first_problem_or_a_timeout() {
        let (tx, rx) = mpsc::channel();
        tx.send(None).unwrap();
        tx.send(Some("writing the stderr log failed".to_owned()))
            .unwrap();
        assert_eq!(
            collect_readers(&rx, Duration::from_secs(1)).as_deref(),
            Some("writing the stderr log failed")
        );
        tx.send(None).unwrap();
        let timed_out = collect_readers(&rx, Duration::from_millis(50)).unwrap();
        assert!(timed_out.contains("still open"), "{timed_out}");
    }
}
