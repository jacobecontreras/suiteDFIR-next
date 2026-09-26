//! Runs and the one active job (CONTRACTS.md §10): `run_start`, `run_cancel`, `job_active`,
//! `job_attach`, the open/reveal commands and `temp_cleanup`.

use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::Arc;
use std::thread;

use suitedfir_core::case;
use suitedfir_core::contracts::{
    ActiveJob, AppError, ErrorCode, JobAttachRequest, JobBacklog, JobKind, OpenTextFileRequest,
    PathRequest, RunCancelRequest, RunEvent, RunFile, RunRef, RunRequest, RunStarted,
    TempCleanupResult, Timestamp,
};
use suitedfir_core::process;
use suitedfir_core::run::record::{self, REPORT_DIR, REPORT_MANIFEST, STDERR_LOG, STDOUT_LOG};
use suitedfir_core::run::status::INDEX_HTML;
use suitedfir_core::runner::{self, RunContext, RunControl};

use super::app_error;
use crate::policy;
use crate::state::{
    AppState, Job, JobEventLog, JobGuard, JobHandle, Stream, Subscriber, lock, panic_message,
};

/// The event subscriber of `job_attach`, for either kind of job.
pub enum AttachSubscriber {
    Run(Subscriber<RunEvent>),
    Acquisition(Subscriber<suitedfir_core::contracts::AcqEvent>),
}

impl AppState {
    /// `run_start`: refused while any job is active or being started (`run_already_active`). The
    /// case must be known; the tool is verified right before the run; `runner::start` does every
    /// other check of ARCHITECTURE.md §6 step 1 and creates the run folder. The slot is reserved
    /// meanwhile, without holding its lock. The run itself goes on on its own thread, its events
    /// to `subscriber` (and later subscribers of `job_attach`).
    pub fn run_start(
        self: &Arc<Self>,
        req: RunRequest,
        subscriber: Subscriber<RunEvent>,
    ) -> Result<RunStarted, AppError> {
        let reservation = self.jobs.reserve(None)?;
        let settings = self.settings();
        let (case_dir, case) = policy::known_case(&settings, &req.case_path)?;
        let tool = req.tool;
        let prepared = self.prepared_tool(tool, &settings)?;
        let ctx = RunContext {
            paths: self.paths.clone(),
            settings,
            case_dir,
            case,
            host: self.host.clone(),
            tool: prepared,
            env: lock(&self.leapp_env).clone(),
            max_run_dir_chars: runner::default_run_dir_limit(),
        };
        let case_path = req.case_path.clone();
        let job = runner::start(req, ctx)?;
        let id = job.run_id().to_owned();
        let started = RunStarted {
            run_id: id.clone(),
            run_dir: job.run_dir().to_string_lossy().into_owned(),
        };
        let stream = Arc::new(Stream::new(subscriber));
        let case_dir = job.case_dir().to_path_buf();
        reservation.activate(Job {
            id: id.clone(),
            case_path,
            created_at: job.created_at(),
            handle: JobHandle::Run {
                tool,
                control: job.control(),
                stream: Arc::clone(&stream),
            },
        });
        log::info!("run {id} started ({tool}) in {}", case_dir.display());
        let guard = JobGuard::new(Arc::clone(self), id.clone());
        let control = job.control();
        let thread_id = id.clone();
        let spawned = thread::Builder::new()
            .name(format!("run-{id}"))
            .spawn(move || {
                let ran = panic::catch_unwind(AssertUnwindSafe(|| {
                    job.run(&mut |event: RunEvent| {
                        // The slot is free before the UI hears `finished` (ARCHITECTURE.md §6
                        // step 10).
                        if event.is_finished() {
                            guard.free();
                        }
                        stream.emit(&event);
                    })
                }));
                match ran {
                    Ok(outcome) => {
                        let codes: Vec<&str> = outcome
                            .record
                            .status_reasons
                            .iter()
                            .map(|r| r.code.as_str())
                            .collect();
                        log::info!(
                            "run {thread_id} finished: {} {codes:?}",
                            outcome.summary.status
                        );
                        if let Some(e) = &outcome.write_error {
                            log::error!("run {thread_id}: the final run.json was not written: {e}");
                        }
                    }
                    Err(payload) => {
                        log::error!(
                            "run {thread_id}: its thread panicked: {}",
                            panic_message(payload.as_ref())
                        );
                        // Already finished (the record is final): nothing is left to stop.
                        if !guard.is_freed() {
                            end_panicked_run(&control, &guard, &stream, &case_dir, &thread_id);
                        }
                    }
                }
                // Dropping the guard frees the slot if nothing did or kept it (once only: by
                // now another job may hold it).
                drop(guard);
            });
        if let Err(e) = spawned {
            // Dropping the closure dropped its guard too; this only clears a slot still ours.
            self.jobs.finish(&id);
            log::error!("run {id}: cannot start its thread: {e}");
            return Err(app_error(
                ErrorCode::Internal,
                "The run could not be started",
                Some(e.to_string()),
            ));
        }
        Ok(started)
    }

    /// `run_cancel`: idempotent; `run_not_found` if that run is not the active job.
    pub fn run_cancel(&self, req: &RunCancelRequest) -> Result<(), AppError> {
        let slot = self.jobs.lock();
        match slot.job.as_ref() {
            Some(Job {
                id,
                handle: JobHandle::Run { control, .. },
                ..
            }) if *id == req.run_id => {
                log::info!("run {id}: cancel requested");
                control.cancel();
                Ok(())
            }
            _ => Err(app_error(
                ErrorCode::RunNotFound,
                "This run is not running",
                Some(req.run_id.clone()),
            )),
        }
    }

    pub fn job_active(&self) -> Option<ActiveJob> {
        self.jobs.lock().job.as_ref().and_then(Job::active)
    }

    /// `job_attach`: the active job's log backlog; its events now go to `subscriber` (replacing
    /// the previous one).
    pub fn job_attach(
        &self,
        req: &JobAttachRequest,
        subscriber: AttachSubscriber,
    ) -> Result<JobBacklog, AppError> {
        let stream = {
            let slot = self.jobs.lock();
            match (slot.job.as_ref(), req.kind) {
                (
                    Some(Job {
                        id,
                        handle: JobHandle::Run { stream, .. },
                        ..
                    }),
                    JobKind::Run,
                ) if *id == req.id => Ok(StreamRef::Run(Arc::clone(stream))),
                (
                    Some(Job {
                        id,
                        handle: JobHandle::Acquisition { stream, .. },
                        ..
                    }),
                    JobKind::Acquisition,
                ) if *id == req.id => Ok(StreamRef::Acquisition(Arc::clone(stream))),
                (_, JobKind::Run) => Err(app_error(
                    ErrorCode::RunNotFound,
                    "This run is not running",
                    Some(req.id.clone()),
                )),
                (_, JobKind::Acquisition) => Err(app_error(
                    ErrorCode::AcqNotFound,
                    "This acquisition is not running",
                    Some(req.id.clone()),
                )),
            }
        }?;
        let backlog = match (stream, subscriber) {
            (StreamRef::Run(stream), AttachSubscriber::Run(subscriber)) => {
                stream.attach(subscriber)
            }
            (StreamRef::Acquisition(stream), AttachSubscriber::Acquisition(subscriber)) => {
                stream.attach(subscriber)
            }
            _ => {
                return Err(app_error(
                    ErrorCode::Internal,
                    "The subscriber does not match the job",
                    None,
                ));
            }
        };
        Ok(JobBacklog { backlog })
    }

    /// `open_report`: the run's `report/index.html` in the default browser, never in the app's
    /// webview (D17).
    pub fn open_report(&self, req: &RunRef) -> Result<(), AppError> {
        let (case_dir, _) = policy::known_case(&self.settings(), &req.case_path)?;
        let run_dir = policy::run_dir(&case_dir, &req.run_id)?;
        let index = run_dir.join(REPORT_DIR).join(INDEX_HTML);
        if !index.is_file() {
            return Err(app_error(
                ErrorCode::ReportMissing,
                "This run has no report (report/index.html is missing)",
                Some(index.display().to_string()),
            ));
        }
        self.open(&index)
    }

    pub fn reveal_path(&self, req: &PathRequest) -> Result<(), AppError> {
        let path = policy::revealable(Path::new(&req.path), &self.paths, &self.settings())?;
        self.opener.reveal(&path).map_err(|e| {
            app_error(
                ErrorCode::Io,
                "The folder could not be shown",
                Some(format!("{}: {e}", path.display())),
            )
        })
    }

    pub fn open_text_file(&self, req: &OpenTextFileRequest) -> Result<(), AppError> {
        let (case_dir, _) = policy::known_case(&self.settings(), &req.case_path)?;
        let run_dir = policy::run_dir(&case_dir, &req.run_id)?;
        let file = run_dir.join(match req.which {
            RunFile::Stdout => STDOUT_LOG,
            RunFile::Stderr => STDERR_LOG,
            RunFile::RunJson => suitedfir_core::case::RUN_FILE,
            RunFile::ReportManifest => REPORT_MANIFEST,
        });
        if !file.is_file() {
            let (code, message) = match req.which {
                RunFile::ReportManifest => (
                    ErrorCode::ReportMissing,
                    "This run has no report manifest (report.sha256)",
                ),
                _ => (ErrorCode::Io, "The file does not exist"),
            };
            return Err(app_error(code, message, Some(file.display().to_string())));
        }
        self.open(&file)
    }

    /// Opens a checked path in its default app.
    pub(crate) fn open(&self, path: &Path) -> Result<(), AppError> {
        self.opener.open(path).map_err(|e| {
            app_error(
                ErrorCode::Io,
                "The file could not be opened",
                Some(format!("{}: {e}", path.display())),
            )
        })
    }

    /// `temp_cleanup`: removes leftover per-job temp dirs. Refused while any job, tool install
    /// (with its introspection) or device command runs, since those use `<app_cache>/tmp`.
    ///
    /// No lock is held during the sweep: the job slot is reserved (no job starts meanwhile), and
    /// installs and device commands wait for the sweep to end before they start (`TmpUsers`).
    pub fn temp_cleanup(&self) -> Result<TempCleanupResult, AppError> {
        let busy = || {
            app_error(
                ErrorCode::RunAlreadyActive,
                "Temporary files cannot be cleaned while a job, a parser installation or a \
                 device command is running",
                None,
            )
        };
        let reservation = self.jobs.reserve(None).map_err(|_| busy())?;
        if self.installs.is_active() {
            return Err(busy());
        }
        let sweep = self
            .tmp
            .clean(|| process::sweep_stale_temp(&self.paths.app_cache))
            .ok_or_else(busy)?;
        drop(reservation);
        let sweep = sweep.map_err(|e| {
            app_error(
                suitedfir_core::fsutil::io_error_code(&e),
                "The temporary files could not be cleaned",
                Some(e.to_string()),
            )
        })?;
        log::info!(
            "temp cleanup removed {} dir(s), {} bytes; {} could not be removed",
            sweep.removed,
            sweep.freed_bytes,
            sweep.failed.len()
        );
        Ok(TempCleanupResult {
            freed_bytes: sweep.freed_bytes,
        })
    }
}

/// After a run thread panicked (before `finished`): cancel, and wait (bounded) until LEAPP's
/// process tree is gone and the input hashing has stopped. Only then is the record recovered as
/// `interrupted`, the slot freed and `finished` sent, so nothing of the run writes into its
/// folder or temp dir after its record is final and another job may start. If that cannot be
/// confirmed, the slot stays taken (logged) until the app restarts.
fn end_panicked_run(
    control: &RunControl,
    guard: &JobGuard,
    stream: &Stream<RunEvent>,
    case_dir: &Path,
    run_id: &str,
) {
    let stopped = control.stop_after_panic(runner::RUN_STOP_WAIT);
    if !stopped {
        log::error!("run {run_id}: LEAPP's process tree or the input hashing is still running");
    }
    guard.end_after_panic(stopped, stream, || panicked_run_finished(case_dir, run_id));
}

/// The run's record recovered as `interrupted`, with the internal-error message (not "when the
/// case was next opened"), and its `finished` event. `None` (logged) if the record cannot be read
/// or recovered.
pub(super) fn panicked_run_finished(case_dir: &Path, run_id: &str) -> Option<RunEvent> {
    let runs = case::discover_runs(case_dir)
        .map_err(|e| log::error!("run {run_id}: its record cannot be read: {e}"))
        .ok()?;
    let Some(mut run) = runs.into_iter().find(|run| run.record.run_id == run_id) else {
        log::error!("run {run_id}: its record was not found");
        return None;
    };
    match record::recover_with(
        &run.dir,
        &mut run.record,
        Timestamp::now(),
        record::INTERNAL_ERROR_MESSAGE,
    ) {
        Ok(()) => {}
        // Written as interrupted, only not read-only.
        Err(e @ record::RecordError::NotReadOnly { .. }) => {
            log::warn!("run {run_id}: recovered, but {e}");
        }
        Err(e) => {
            log::error!("run {run_id}: recovering its record failed: {e}");
            return None;
        }
    }
    Some(RunEvent::Finished {
        status: run.record.status,
        reasons: run.record.status_reasons.clone(),
        warnings: run.record.warnings.clone(),
        summary: Box::new(case::run_summary(&run)),
    })
}

enum StreamRef {
    Run(Arc<Stream<RunEvent>>),
    Acquisition(Arc<Stream<suitedfir_core::contracts::AcqEvent>>),
}
