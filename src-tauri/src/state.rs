//! `AppState` (ARCHITECTURE.md §5.2): the settings cache, the one active job (a run, an
//! acquisition or a later encryption restore; D25) with its event stream and log backlog, the
//! running tool installs, and what the shell passes to the core (app dirs, host, manifest,
//! platform, the iOS tools' location, the dev overrides).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsString;
// Only the debug-build dev override and the tests use it.
#[cfg(any(debug_assertions, test))]
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, Instant};

use suitedfir_core::acquire::AcqControl;
use suitedfir_core::contracts::{
    AcqEvent, ActiveJob, AppError, ErrorCode, LeappManifest, PlatformKey, RecordHost, RunEvent,
    Settings, Timestamp, ToolId,
};
use suitedfir_core::idevice::{Idevice, IdeviceConfig};
use suitedfir_core::leapp::install::Pinned;
use suitedfir_core::paths::AppPaths;
use suitedfir_core::runner::RunControl;
use suitedfir_core::settings;

use crate::opener::Opener;

/// Lines of the job's log kept for `job_attach` (ARCHITECTURE.md §5.2).
pub const BACKLOG_LINES: usize = 2000;

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Every value behind these locks is valid at all times, so a poisoned lock is usable.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the shell resolved at startup.
pub struct AppConfig {
    pub paths: AppPaths,
    pub host: RecordHost,
    pub manifest: LeappManifest,
    pub platform: Option<PlatformKey>,
    pub idevice: IdeviceConfig,
    /// `SUITEDFIR_DEV_LEAPP_OVERRIDE` (debug builds only): fake-leapp for these tools.
    #[cfg(debug_assertions)]
    pub leapp_override: BTreeMap<ToolId, PathBuf>,
    /// Variables added to LEAPP's environment (empty in the app; tests pick fake-leapp scenarios).
    pub leapp_env: Vec<(OsString, OsString)>,
    pub opener: Arc<dyn Opener>,
    /// Tests: `tool_install` takes the pinned asset from this file instead of the network.
    #[cfg(test)]
    pub download_from: Option<PathBuf>,
}

/// The app state shared by every command.
pub struct AppState {
    pub paths: AppPaths,
    pub host: RecordHost,
    pub manifest: LeappManifest,
    pub platform: Option<PlatformKey>,
    #[cfg(debug_assertions)]
    pub leapp_override: BTreeMap<ToolId, PathBuf>,
    /// `SUITEDFIR_DEV_IDEVICE_OVERRIDE` is in effect (debug builds only).
    #[cfg(debug_assertions)]
    pub idevice_override: bool,
    pub opener: Arc<dyn Opener>,
    #[cfg(test)]
    pub download_from: Option<PathBuf>,
    pub(crate) leapp_env: Mutex<Vec<(OsString, OsString)>>,
    idevice: RwLock<Arc<Idevice>>,
    settings: Mutex<Settings>,
    /// Tools whose entry hash was checked in this session (`tools_status` reports `verified`).
    pub(crate) verified: Mutex<BTreeSet<ToolId>>,
    pub(crate) jobs: Jobs,
    pub(crate) installs: Installs,
    /// Tool installs and device commands (`devices_list`, `device_pair`, `acq_preflight`) in
    /// flight: their scratch dirs live in `<app_cache>/tmp` (see [`TmpUsers`]).
    pub(crate) tmp: TmpUsers,
}

impl AppState {
    /// `settings` is the loaded `settings.json` (or the first-run defaults).
    pub fn new(config: AppConfig, settings: Settings) -> Self {
        Self {
            paths: config.paths,
            host: config.host,
            manifest: config.manifest,
            platform: config.platform,
            #[cfg(debug_assertions)]
            leapp_override: config.leapp_override,
            #[cfg(debug_assertions)]
            idevice_override: config.idevice.lookup.dev_override.is_some(),
            opener: config.opener,
            #[cfg(test)]
            download_from: config.download_from,
            leapp_env: Mutex::new(config.leapp_env),
            idevice: RwLock::new(Arc::new(Idevice::new(config.idevice))),
            settings: Mutex::new(settings),
            verified: Mutex::new(BTreeSet::new()),
            jobs: Jobs::default(),
            installs: Installs::default(),
            tmp: TmpUsers::default(),
        }
    }

    /// A copy of the settings in effect.
    pub fn settings(&self) -> Settings {
        lock(&self.settings).clone()
    }

    /// Changes the settings: `change` edits a copy, which is saved to `settings.json` and only then
    /// becomes the settings in effect. Returns the new settings.
    pub fn update_settings<T>(
        &self,
        change: impl FnOnce(&mut Settings) -> Result<T, AppError>,
    ) -> Result<(Settings, T), AppError> {
        let mut guard = lock(&self.settings);
        let mut next = guard.clone();
        let value = change(&mut next)?;
        settings::save(&self.paths.settings_file(), &next)?;
        *guard = next.clone();
        Ok((next, value))
    }

    /// The iOS tools handle.
    pub fn idevice(&self) -> Arc<Idevice> {
        Arc::clone(&self.idevice.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Tests: a new iOS tools handle (e.g. another fake-idevice scenario).
    #[cfg(test)]
    pub fn replace_idevice(&self, config: IdeviceConfig) {
        *self.idevice.write().unwrap_or_else(PoisonError::into_inner) =
            Arc::new(Idevice::new(config));
    }

    /// A tool as pinned for this host, in the tools dir in effect.
    pub fn with_pinned<T>(
        &self,
        tool: ToolId,
        settings: &Settings,
        f: impl FnOnce(Pinned<'_>) -> T,
    ) -> Result<T, AppError> {
        let manifest = self.manifest.tools.get(&tool).ok_or_else(|| AppError {
            code: ErrorCode::Internal,
            message: format!("The tool manifest has no entry for {tool}"),
            detail: None,
        })?;
        let tools_dir = self.paths.tools_dir(settings);
        Ok(f(Pinned {
            tools_dir: &tools_dir,
            tool,
            manifest,
            platform: self.platform,
        }))
    }

    /// Whether any dev override is active (`app_info.dev_override`).
    pub fn dev_override_active(&self) -> bool {
        #[cfg(debug_assertions)]
        {
            !self.leapp_override.is_empty() || self.idevice_override
        }
        #[cfg(not(debug_assertions))]
        {
            false
        }
    }

    /// Registers a device command while it runs (see `tmp`).
    pub(crate) fn device_op(&self) -> TmpUse<'_> {
        self.tmp.enter()
    }
}

// ---- the one active job ----

/// Something the event stream can take log lines from.
pub trait JobEventLog {
    fn log_lines(&self) -> Option<&[String]>;
    fn is_finished(&self) -> bool;
}

impl JobEventLog for RunEvent {
    fn log_lines(&self) -> Option<&[String]> {
        match self {
            RunEvent::Log { lines } => Some(lines),
            _ => None,
        }
    }

    fn is_finished(&self) -> bool {
        matches!(self, RunEvent::Finished { .. })
    }
}

impl JobEventLog for AcqEvent {
    fn log_lines(&self) -> Option<&[String]> {
        match self {
            AcqEvent::Log { lines } => Some(lines),
            _ => None,
        }
    }

    fn is_finished(&self) -> bool {
        matches!(self, AcqEvent::Finished { .. })
    }
}

/// Where a job's events go: the current subscriber (the `onEvent` channel of `run_start` /
/// `acq_start`, or of the latest `job_attach`), the last 2,000 log lines, and the `finished` event
/// once it came (so a subscriber that attaches just after it still gets it).
pub struct Stream<E> {
    inner: Mutex<StreamInner<E>>,
}

pub type Subscriber<E> = Box<dyn Fn(&E) + Send>;

struct StreamInner<E> {
    subscriber: Subscriber<E>,
    backlog: VecDeque<String>,
    finished: Option<E>,
}

impl<E: JobEventLog + Clone> Stream<E> {
    pub fn new(subscriber: Subscriber<E>) -> Self {
        Self {
            inner: Mutex::new(StreamInner {
                subscriber,
                backlog: VecDeque::new(),
                finished: None,
            }),
        }
    }

    pub fn emit(&self, event: &E) {
        let mut inner = lock(&self.inner);
        if let Some(lines) = event.log_lines() {
            inner.backlog.extend(lines.iter().cloned());
            let excess = inner.backlog.len().saturating_sub(BACKLOG_LINES);
            inner.backlog.drain(..excess);
        }
        if event.is_finished() {
            inner.finished = Some(event.clone());
        }
        (inner.subscriber)(event);
    }

    /// `job_attach`: replaces the subscriber and returns the backlog. Events after this go to the
    /// new subscriber only; a `finished` that already came is sent to it at once.
    pub fn attach(&self, subscriber: Subscriber<E>) -> Vec<String> {
        let mut inner = lock(&self.inner);
        inner.subscriber = subscriber;
        if let Some(finished) = &inner.finished {
            (inner.subscriber)(finished);
        }
        inner.backlog.iter().cloned().collect()
    }
}

/// What the active job is.
pub enum JobHandle {
    Run {
        tool: ToolId,
        control: Arc<RunControl>,
        stream: Arc<Stream<RunEvent>>,
    },
    Acquisition {
        udid: String,
        control: Arc<AcqControl>,
        stream: Arc<Stream<AcqEvent>>,
    },
    /// `acq_restore_encryption`: counts as a job; cannot be cancelled.
    Restore { udid: String },
}

/// The active job.
pub struct Job {
    /// The run id or acquisition id.
    pub id: String,
    pub case_path: String,
    pub created_at: Timestamp,
    pub handle: JobHandle,
}

impl Job {
    /// `job_active`. A later restore is not shown (the Case screen waits for its answer).
    pub fn active(&self) -> Option<ActiveJob> {
        match &self.handle {
            JobHandle::Run { tool, control, .. } => Some(ActiveJob::Run {
                case_path: self.case_path.clone(),
                run_id: self.id.clone(),
                tool: *tool,
                created_at: self.created_at,
                phase: control.phase(),
            }),
            JobHandle::Acquisition { udid, control, .. } => Some(ActiveJob::Acquisition {
                case_path: self.case_path.clone(),
                acq_id: self.id.clone(),
                udid: udid.clone(),
                created_at: self.created_at,
                phase: control.phase(),
            }),
            JobHandle::Restore { .. } => None,
        }
    }

    /// The device the job uses (`devices_list` reports it busy and does not query it).
    pub fn udid(&self) -> Option<&str> {
        match &self.handle {
            JobHandle::Run { .. } => None,
            JobHandle::Acquisition { udid, .. } | JobHandle::Restore { udid } => Some(udid),
        }
    }
}

/// The one active job slot, app-wide (D25). The lock is only ever held briefly: a job being
/// started first reserves the slot ([`Jobs::reserve`]), does its slow checks (the tool's entry
/// hash, device queries) without the lock, then becomes the active job
/// ([`Reservation::activate`]) or gives the slot back. So the quit guard (on the main thread) and
/// device polling never wait for a start.
#[derive(Default)]
pub struct Jobs {
    slot: Mutex<Slot>,
    changed: Condvar,
}

/// What the job slot holds.
#[derive(Default)]
pub struct Slot {
    /// The active job.
    pub job: Option<Job>,
    /// A job being started (or `temp_cleanup` sweeping): the slot is taken, but no job is
    /// active yet.
    pub starting: Option<Starting>,
}

impl Slot {
    /// A job is active or being started.
    pub fn is_taken(&self) -> bool {
        self.job.is_some() || self.starting.is_some()
    }
}

/// A reservation of the job slot.
pub struct Starting {
    /// The device the job will use (an acquisition): polling reports it busy from now on.
    pub udid: Option<String>,
}

pub fn job_already_active() -> AppError {
    AppError {
        code: ErrorCode::RunAlreadyActive,
        message: "Another job is running. Wait for it to finish or cancel it.".to_owned(),
        detail: None,
    }
}

impl Jobs {
    pub fn lock(&self) -> MutexGuard<'_, Slot> {
        lock(&self.slot)
    }

    /// Takes the slot for a job about to start (`run_already_active` if a job is active or being
    /// started). Dropping the reservation without [`Reservation::activate`] frees the slot again,
    /// also when the start fails or panics.
    pub fn reserve(&self, udid: Option<String>) -> Result<Reservation<'_>, AppError> {
        let mut slot = self.lock();
        if slot.is_taken() {
            return Err(job_already_active());
        }
        slot.starting = Some(Starting { udid });
        Ok(Reservation {
            jobs: self,
            active: false,
        })
    }

    /// The slot, once no job is being started (a start takes seconds at most). Recovery on
    /// `case_open` uses this, so it never marks the record of a job that is just starting.
    pub fn lock_settled(&self) -> MutexGuard<'_, Slot> {
        let mut slot = self.lock();
        while slot.starting.is_some() {
            slot = self
                .changed
                .wait(slot)
                .unwrap_or_else(PoisonError::into_inner);
        }
        slot
    }

    /// Clears the slot if it still holds job `id`, and wakes whoever waits for the job to end.
    pub fn finish(&self, id: &str) {
        let mut slot = self.lock();
        if slot.job.as_ref().is_some_and(|job| job.id == id) {
            slot.job = None;
        }
        drop(slot);
        self.changed.notify_all();
    }

    /// Waits until no job is active or being started; `false` if one still is after `timeout`
    /// (`None`: no limit).
    pub fn wait_idle(&self, timeout: Option<Duration>) -> bool {
        self.wait_until(timeout, |slot| !slot.is_taken())
    }

    /// Waits until no job is being started (one may be active); `false` after `timeout`.
    pub fn wait_settled(&self, timeout: Duration) -> bool {
        self.wait_until(Some(timeout), |slot| slot.starting.is_none())
    }

    fn wait_until(&self, timeout: Option<Duration>, done: impl Fn(&Slot) -> bool) -> bool {
        let deadline = timeout.and_then(|timeout| Instant::now().checked_add(timeout));
        let mut slot = self.lock();
        while !done(&slot) {
            match deadline {
                None => {
                    slot = self
                        .changed
                        .wait(slot)
                        .unwrap_or_else(PoisonError::into_inner)
                }
                Some(deadline) => {
                    let now = Instant::now();
                    if now >= deadline {
                        return false;
                    }
                    slot = self
                        .changed
                        .wait_timeout(slot, deadline - now)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
            }
        }
        true
    }
}

/// The job slot, reserved for a job being started (see [`Jobs::reserve`]).
pub struct Reservation<'a> {
    jobs: &'a Jobs,
    active: bool,
}

impl Reservation<'_> {
    /// The started job becomes the active job.
    pub fn activate(mut self, job: Job) {
        let mut slot = self.jobs.lock();
        slot.starting = None;
        slot.job = Some(job);
        self.active = true;
        drop(slot);
        self.jobs.changed.notify_all();
    }
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if !self.active {
            self.jobs.lock().starting = None;
            self.jobs.changed.notify_all();
        }
    }
}

/// Frees the slot of job `id` when dropped, unless already freed: a job thread that panics does
/// not leave a phantom active job. [`JobGuard::free`] frees it earlier (just before `finished`).
pub struct JobGuard {
    state: Arc<AppState>,
    id: String,
    freed: AtomicBool,
}

impl JobGuard {
    pub fn new(state: Arc<AppState>, id: String) -> Self {
        Self {
            state,
            id,
            freed: AtomicBool::new(false),
        }
    }

    /// Frees the slot once (by now another job may hold it: then it is left alone).
    pub fn free(&self) {
        if !self.freed.swap(true, Ordering::SeqCst) {
            self.state.jobs.finish(&self.id);
        }
    }

    pub fn is_freed(&self) -> bool {
        self.freed.load(Ordering::SeqCst)
    }
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        self.free();
    }
}

// ---- other users of `<app_cache>/tmp` ----

/// Tool installs (with their introspection) and device commands keep scratch dirs in
/// `<app_cache>/tmp`. `temp_cleanup` sweeps only while none runs, and none starts during the
/// sweep (it waits for the sweep to end).
#[derive(Default)]
pub struct TmpUsers {
    state: Mutex<TmpState>,
    changed: Condvar,
}

#[derive(Default)]
struct TmpState {
    users: usize,
    cleaning: bool,
}

/// One user of `<app_cache>/tmp`, until dropped.
pub struct TmpUse<'a>(&'a TmpUsers);

impl Drop for TmpUse<'_> {
    fn drop(&mut self) {
        lock(&self.0.state).users -= 1;
        self.0.changed.notify_all();
    }
}

/// Ends a sweep when dropped (also if the sweep panics).
struct Cleaning<'a>(&'a TmpUsers);

impl Drop for Cleaning<'_> {
    fn drop(&mut self) {
        lock(&self.0.state).cleaning = false;
        self.0.changed.notify_all();
    }
}

impl TmpUsers {
    /// Registers a user; waits while a sweep runs.
    pub fn enter(&self) -> TmpUse<'_> {
        let mut state = lock(&self.state);
        while state.cleaning {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        state.users += 1;
        TmpUse(self)
    }

    /// Runs `sweep` with no user and none starting meanwhile; `None` (not run) while one runs.
    pub fn clean<T>(&self, sweep: impl FnOnce() -> T) -> Option<T> {
        {
            let mut state = lock(&self.state);
            if state.users > 0 || state.cleaning {
                return None;
            }
            state.cleaning = true;
        }
        let _cleaning = Cleaning(self);
        Some(sweep())
    }
}

// ---- tool installs ----

/// The running `tool_install` / `tool_import` jobs, one per tool at most, with their cancel flags
/// (the quit guard sets them).
#[derive(Default)]
pub struct Installs {
    running: Mutex<BTreeMap<ToolId, Arc<AtomicBool>>>,
    ended: Condvar,
}

/// A running install; dropping it unregisters it.
pub struct InstallSlot<'a> {
    installs: &'a Installs,
    tool: ToolId,
    pub cancel: Arc<AtomicBool>,
}

impl Drop for InstallSlot<'_> {
    fn drop(&mut self) {
        lock(&self.installs.running).remove(&self.tool);
        self.installs.ended.notify_all();
    }
}

impl Installs {
    /// Registers an install of `tool`; refused while one of the same tool runs.
    pub fn begin(&self, tool: ToolId, display_name: &str) -> Result<InstallSlot<'_>, AppError> {
        let mut running = lock(&self.running);
        if running.contains_key(&tool) {
            return Err(AppError {
                code: ErrorCode::RunAlreadyActive,
                message: format!("{display_name} is already being installed."),
                detail: None,
            });
        }
        let cancel = Arc::new(AtomicBool::new(false));
        running.insert(tool, Arc::clone(&cancel));
        Ok(InstallSlot {
            installs: self,
            tool,
            cancel,
        })
    }

    pub fn is_active(&self) -> bool {
        !lock(&self.running).is_empty()
    }

    /// Sets the cancel flag of every running install (quit).
    pub fn cancel_all(&self) {
        for cancel in lock(&self.running).values() {
            cancel.store(true, Ordering::SeqCst);
        }
    }

    /// Waits up to `timeout` for every install to end; `false` if one still runs.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now().checked_add(timeout);
        let mut running = lock(&self.running);
        while !running.is_empty() {
            let Some(deadline) = deadline else {
                return false;
            };
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            running = self
                .ended
                .wait_timeout(running, deadline - now)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        true
    }
}
