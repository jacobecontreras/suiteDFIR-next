//! Startup and the quit guard (ARCHITECTURE.md §5.2, F9).
//!
//! **Startup** ([`setup`]): take the instance lock (a second instance shows a native message and
//! exits before touching any state) → sweep stale temp dirs → open the app log → load the settings
//! → build `AppState` → open the main window (`create: false` in `tauri.conf.json`, so a second
//! instance never shows one).
//!
//! **Quit guard** ([`on_run_event`]): closing the window (`CloseRequested`) or quitting the app
//! (`ExitRequested` from the user) while a job or a tool install runs is held back:
//! - a run: a native "cancel and quit?" dialog; on yes the run is cancelled, and the app exits once
//!   it is finalized, after at most 30 s;
//! - an acquisition: the same question; on yes it is cancelled with the semantics by phase of §6b
//!   (a pending encryption restore still runs and may wait for the device passcode), and a
//!   "finishing safely…" dialog offers "Quit anyway"; the app exits by itself once the
//!   acquisition is finalized. Quitting anyway leaves its record to be marked `interrupted` (with
//!   the encryption warnings) on the next open;
//! - a later encryption restore: it cannot be cancelled, so only the "finishing safely…" dialog;
//! - a tool install: its cancel flag is set and the app exits once it has cleaned up (at most
//!   10 s).

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use suitedfir_core::contracts::Settings;
use suitedfir_core::idevice::{IdeviceConfig, ToolLookup, embedded_manifest};
use suitedfir_core::paths::AppPaths;
use suitedfir_core::{manifest, process, settings};
use tauri::{AppHandle, Manager, RunEvent, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::commands::Shared;
use crate::lock::{self, InstanceLock, LockError};
use crate::opener::SystemOpener;
use crate::state::{AppConfig, AppState, JobHandle};
use crate::{host, logger};

/// How long quitting waits for a cancelled run to be finalized.
pub const RUN_QUIT_WAIT: Duration = Duration::from_secs(30);
/// How long quitting waits for cancelled installs to clean up.
pub const INSTALL_QUIT_WAIT: Duration = Duration::from_secs(10);

const TITLE: &str = "suiteDFIR";

/// Keeps the instance lock for the life of the process.
struct HeldLock(#[allow(dead_code)] InstanceLock);

/// The startup sequence (see the module docs). Problems that stop the app are shown natively.
pub fn setup(app: &mut tauri::App) -> Result<(), Box<dyn Error>> {
    let handle = app.handle().clone();
    let paths = match app_paths(&handle) {
        Ok(paths) => paths,
        Err(e) => {
            fatal(&handle, &format!("suiteDFIR cannot find its folders: {e}"));
            return Ok(());
        }
    };
    let held = match lock::acquire(&paths.instance_lock()) {
        Ok(held) => held,
        Err(LockError::AnotherInstance) => {
            fatal(
                &handle,
                "suiteDFIR is already running. Switch to the open window; only one instance can \
                 run at a time.",
            );
            return Ok(());
        }
        Err(e) => {
            fatal(&handle, &format!("suiteDFIR cannot start: {e}"));
            return Ok(());
        }
    };
    app.manage(HeldLock(held));

    // No job can run yet, and no other instance is live: stale temp dirs can go.
    let sweep = process::sweep_stale_temp(&paths.app_cache);
    if let Err(e) = logger::init(&paths.log_file()) {
        eprintln!("suiteDFIR: cannot open the app log: {e}");
    }
    log::info!(
        "suiteDFIR {} starting on {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    match sweep {
        Ok(sweep) => log::info!(
            "startup temp sweep: removed {} dir(s), {} bytes; {} left",
            sweep.removed,
            sweep.freed_bytes,
            sweep.failed.len()
        ),
        Err(e) => log::warn!("startup temp sweep failed: {e}"),
    }

    let default_cases_root = default_cases_root(&handle);
    let settings = load_settings(&paths, &default_cases_root);
    let manifest = manifest::embedded()?.clone();
    let platform = manifest::host_platform();
    let config = AppConfig {
        idevice: idevice_config(&paths, platform)?,
        paths,
        host: host::detect(),
        manifest,
        platform,
        #[cfg(debug_assertions)]
        leapp_override: leapp_override(),
        leapp_env: Vec::new(),
        opener: Arc::new(SystemOpener),
        #[cfg(test)]
        download_from: None,
    };
    #[cfg(debug_assertions)]
    if !config.leapp_override.is_empty() || config.idevice.lookup.dev_override.is_some() {
        log::warn!("dev override active: results are not evidence");
    }
    let state: Shared = Arc::new(AppState::new(config, settings));
    app.manage(state);

    let window = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == "main")
        .cloned()
        .ok_or("tauri.conf.json has no main window")?;
    tauri::WebviewWindowBuilder::from_config(&handle, &window)?.build()?;
    Ok(())
}

/// Shows `message` natively, then exits with code 1.
fn fatal(handle: &AppHandle, message: &str) {
    eprintln!("suiteDFIR: {message}");
    let exit = handle.clone();
    handle
        .dialog()
        .message(message)
        .title(TITLE)
        .kind(MessageDialogKind::Error)
        .show(move |_| exit.exit(1));
}

fn app_paths(handle: &AppHandle) -> tauri::Result<AppPaths> {
    let path = handle.path();
    Ok(AppPaths {
        app_data: path.app_data_dir()?,
        app_config: path.app_config_dir()?,
        app_cache: path.app_cache_dir()?,
        app_log: path.app_log_dir()?,
    })
}

/// `<Documents>/suiteDFIR Cases` (created when the first case is).
fn default_cases_root(handle: &AppHandle) -> PathBuf {
    let path = handle.path();
    let documents = path
        .document_dir()
        .or_else(|_| path.home_dir().map(|home| home.join("Documents")))
        .unwrap_or_else(|_| PathBuf::from("."));
    documents.join("suiteDFIR Cases")
}

/// `settings.json`, or the first-run defaults. An unreadable or invalid file is moved aside (kept
/// for the examiner) and the defaults are used.
fn load_settings(paths: &AppPaths, default_cases_root: &Path) -> Settings {
    let file = paths.settings_file();
    match settings::load(&file, default_cases_root) {
        Ok(settings) => settings,
        Err(e) => {
            let aside = file.with_extension("json.invalid");
            let moved = std::fs::rename(&file, &aside);
            log::error!(
                "{} is unusable ({e}); using the defaults (the file was {})",
                file.display(),
                match moved {
                    Ok(()) => format!("moved to {}", aside.display()),
                    Err(e) => format!("left in place: {e}"),
                }
            );
            settings::defaults(default_cases_root)
        }
    }
}

/// The iOS tools: the sidecars next to the app executable (release builds), else `PATH` (Linux,
/// and debug builds), or the dev override (debug builds).
fn idevice_config(
    paths: &AppPaths,
    platform: Option<suitedfir_core::contracts::PlatformKey>,
) -> Result<IdeviceConfig, Box<dyn Error>> {
    let bundled_dir = tauri::utils::platform::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));
    #[cfg(debug_assertions)]
    let dev_override = std::env::var_os("SUITEDFIR_DEV_IDEVICE_OVERRIDE").map(PathBuf::from);
    #[cfg(not(debug_assertions))]
    let dev_override = None;
    Ok(IdeviceConfig {
        lookup: ToolLookup {
            platform,
            manifest: embedded_manifest()?,
            bundled_dir,
            dev_override,
            path_var: std::env::var_os("PATH"),
            // Code-signed release builds are a later task (F1); until then the bundled tools must
            // match the pinned unsigned hashes.
            signed_build: false,
        },
        app_cache: paths.app_cache.clone(),
        env: Vec::new(),
    })
}

/// `SUITEDFIR_DEV_LEAPP_OVERRIDE`: fake-leapp for both tools (debug builds only).
#[cfg(debug_assertions)]
fn leapp_override() -> std::collections::BTreeMap<suitedfir_core::contracts::ToolId, PathBuf> {
    std::env::var_os("SUITEDFIR_DEV_LEAPP_OVERRIDE")
        .map(|path| {
            suitedfir_core::contracts::ToolId::ALL
                .iter()
                .map(|tool| (*tool, PathBuf::from(&path)))
                .collect()
        })
        .unwrap_or_default()
}

// ---- the quit guard ----

/// What quitting has to deal with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuitPlan {
    /// Nothing runs: quit.
    Now,
    /// Only tool installs run: cancel them, wait for their cleanup, quit.
    Installs,
    /// A run is active (its id).
    Run(String),
    /// An acquisition is active.
    Acquisition(String),
    /// A later encryption restore is active.
    Restore(String),
}

pub fn quit_plan(state: &AppState) -> QuitPlan {
    let slot = state.jobs.lock();
    match slot.as_ref() {
        Some(job) => match &job.handle {
            JobHandle::Run { .. } => QuitPlan::Run(job.id.clone()),
            JobHandle::Acquisition { .. } => QuitPlan::Acquisition(job.id.clone()),
            JobHandle::Restore { .. } => QuitPlan::Restore(job.id.clone()),
        },
        None if state.installs.is_active() => QuitPlan::Installs,
        None => QuitPlan::Now,
    }
}

/// A quit flow is under way (one dialog at a time).
static QUITTING: AtomicBool = AtomicBool::new(false);

/// The app's run-event handler: the quit guard.
pub fn on_run_event(app: &AppHandle, event: RunEvent) {
    match event {
        RunEvent::WindowEvent {
            event: WindowEvent::CloseRequested { api, .. },
            ..
        } if hold_quit(app) => api.prevent_close(),
        // `code` is `None` when the user asked (last window closed, Cmd+Q); our own `exit` passes.
        RunEvent::ExitRequested {
            code: None, api, ..
        } if hold_quit(app) => api.prevent_exit(),
        _ => {}
    }
}

/// True when quitting must wait; then the quit flow starts (once).
fn hold_quit(app: &AppHandle) -> bool {
    let Some(state) = app.try_state::<Shared>() else {
        return false;
    };
    let state = Arc::clone(&state);
    let plan = quit_plan(&state);
    if plan == QuitPlan::Now {
        return false;
    }
    if !QUITTING.swap(true, Ordering::SeqCst) {
        start_quit(app, state, plan);
    }
    true
}

fn start_quit(app: &AppHandle, state: Shared, plan: QuitPlan) {
    match plan {
        QuitPlan::Now => app.exit(0),
        QuitPlan::Installs => {
            log::info!("quit during a tool install: cancelling it");
            exit_when(app, move || {
                state.installs.cancel_all();
                state.installs.wait_idle(INSTALL_QUIT_WAIT);
            });
        }
        QuitPlan::Run(id) => {
            let app = app.clone();
            ask(
                &app.clone(),
                "A run is in progress. Cancel it and quit?\n\nThe run is stopped and recorded as \
                 cancelled; its partial output is kept.",
                "Cancel run and quit",
                "Keep running",
                move |yes| {
                    if !yes {
                        QUITTING.store(false, Ordering::SeqCst);
                        return;
                    }
                    log::info!("quit: cancelling run {id}");
                    cancel_active(&state, &id);
                    exit_when(&app, move || {
                        state.installs.cancel_all();
                        if !state.jobs.wait_idle(Some(RUN_QUIT_WAIT)) {
                            log::warn!("quit: run {id} was not finalized within 30 s");
                        }
                        state.installs.wait_idle(INSTALL_QUIT_WAIT);
                    });
                },
            );
        }
        QuitPlan::Acquisition(id) => {
            let app = app.clone();
            ask(
                &app.clone(),
                "An iOS acquisition is in progress. Cancel it and quit?\n\nThe backup is stopped. \
                 If backup encryption was turned on for it, suiteDFIR turns it off again first, \
                 which may wait for the device passcode.",
                "Cancel and quit",
                "Keep running",
                move |yes| {
                    if !yes {
                        QUITTING.store(false, Ordering::SeqCst);
                        return;
                    }
                    log::info!("quit: cancelling acquisition {id}");
                    cancel_active(&state, &id);
                    finish_safely(&app, state, id);
                },
            );
        }
        QuitPlan::Restore(id) => finish_safely(app, state, id),
    }
}

/// Cancels the active job if it is still `id`.
fn cancel_active(state: &AppState, id: &str) {
    let slot = state.jobs.lock();
    if let Some(job) = slot.as_ref().filter(|job| job.id == id) {
        match &job.handle {
            JobHandle::Run { control, .. } => control.cancel(),
            JobHandle::Acquisition { control, .. } => control.cancel(),
            JobHandle::Restore { .. } => {}
        }
    }
}

/// The acquisition (or later restore) finishes on its own; the app exits when it has. Meanwhile
/// "finishing safely…" offers to quit at once.
fn finish_safely(app: &AppHandle, state: Shared, id: String) {
    let waiter = Arc::clone(&state);
    exit_when(app, move || {
        waiter.installs.cancel_all();
        waiter.jobs.wait_idle(None);
        waiter.installs.wait_idle(INSTALL_QUIT_WAIT);
    });
    let quit = app.clone();
    ask(
        app,
        "Finishing safely…\n\nsuiteDFIR quits by itself as soon as the device work has stopped \
         and the record is written. Restoring the backup-encryption setting may wait for the \
         device passcode.\n\nQuitting anyway leaves the acquisition to be marked interrupted, \
         with its encryption warnings, when the case is next opened.",
        "Quit anyway",
        "Wait",
        move |yes| {
            if yes {
                log::warn!("quit anyway while {id} was finishing");
                quit.exit(0);
            }
        },
    );
}

/// A native two-button question; `answer` gets true for `yes`.
fn ask(
    app: &AppHandle,
    message: &str,
    yes: &str,
    no: &str,
    answer: impl FnOnce(bool) + Send + 'static,
) {
    app.dialog()
        .message(message)
        .title(TITLE)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            yes.to_owned(),
            no.to_owned(),
        ))
        .show(answer);
}

/// Runs `wait` on its own thread, then exits the app.
fn exit_when(app: &AppHandle, wait: impl FnOnce() + Send + 'static) {
    let app = app.clone();
    let spawned = thread::Builder::new()
        .name("quit".to_owned())
        .spawn(move || {
            wait();
            log::info!("quitting");
            app.exit(0);
        });
    if let Err(e) = spawned {
        log::error!("cannot wait for the quit: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::state::{Job, JobHandle};
    use crate::testing::lab_state;
    use suitedfir_core::contracts::{Timestamp, ToolId};

    #[test]
    fn the_quit_plan_follows_the_active_job_and_installs() {
        let lab = lab_state();
        let state = &lab.state;
        assert_eq!(quit_plan(state), QuitPlan::Now);
        let install = state.installs.begin(ToolId::Aleapp, "aLEAPP").unwrap();
        assert_eq!(quit_plan(state), QuitPlan::Installs);
        // Installs are cancelled on quit.
        state.installs.cancel_all();
        assert!(install.cancel.load(Ordering::SeqCst));
        drop(install);
        assert!(state.installs.wait_idle(Duration::from_millis(10)));
        *state.jobs.lock() = Some(Job {
            id: "20260924-171200Z-ios-9c01de".to_owned(),
            case_path: "/case".to_owned(),
            created_at: Timestamp::now(),
            handle: JobHandle::Restore {
                udid: "00008101-000A1B2C3D4E001E".to_owned(),
            },
        });
        assert_eq!(
            quit_plan(state),
            QuitPlan::Restore("20260924-171200Z-ios-9c01de".to_owned())
        );
        // A job outranks installs; waiting for it ends when it finishes.
        assert!(!state.jobs.wait_idle(Some(Duration::from_millis(20))));
        state.jobs.finish("20260924-171200Z-ios-9c01de");
        assert!(state.jobs.wait_idle(Some(Duration::from_millis(20))));
        assert_eq!(quit_plan(state), QuitPlan::Now);
    }
}
