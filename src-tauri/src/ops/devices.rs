//! iOS devices and acquisitions (CONTRACTS.md §13.5).

use std::sync::Arc;
use std::thread;

use suitedfir_core::acquire::{self, AcqContext};
use suitedfir_core::contracts::{
    AcqCancelRequest, AcqEvent, AcqFile, AcqPreflight, AcqPreflightRequest, AcqRef, AcqRequest,
    AcqRestoreEncryptionRequest, AcqRestoreEncryptionResult, AcqStarted, AcquisitionRecord,
    AppError, DevicePairRequest, DeviceSummary, DevicesResult, ErrorCode, OpenAcqFileRequest,
    Timestamp,
};

use super::app_error;
use crate::policy;
use crate::state::{AppState, Job, JobEventLog, JobHandle, Stream, Subscriber, job_already_active};

impl AppState {
    /// The device of the active job, which polling must not query.
    fn busy_udid(&self) -> Option<String> {
        self.jobs
            .lock()
            .as_ref()
            .and_then(|job| job.udid().map(str::to_owned))
    }

    /// `devices_list`: never pairs and never fails for tool or usbmuxd problems (reported in
    /// `tools`); the active job's device is returned busy, unqueried.
    pub fn devices_list(&self) -> DevicesResult {
        let _op = self.device_op();
        let busy = self.busy_udid();
        self.idevice().list_devices(busy.as_deref())
    }

    /// `device_pair`: the only command that pairs.
    pub fn device_pair(&self, req: &DevicePairRequest) -> Result<DeviceSummary, AppError> {
        policy::udid(&req.udid)?;
        let _op = self.device_op();
        let busy = self.busy_udid();
        let summary = self.idevice().pair(&req.udid, busy.as_deref())?;
        log::info!("device {}: pair → {}", req.udid, summary.pair_state);
        Ok(summary)
    }

    pub fn acq_preflight(&self, req: &AcqPreflightRequest) -> Result<AcqPreflight, AppError> {
        let (case_dir, _) = policy::known_case(&self.settings(), &req.case_path)?;
        policy::udid(&req.udid)?;
        let _op = self.device_op();
        Ok(acquire::preflight(&self.idevice(), &case_dir, &req.udid)?)
    }

    /// `acq_start`: refused while any job is active; `acquire::start` does the checks of
    /// ARCHITECTURE.md §6b step 4 and creates the acquisition folder. The acquisition goes on on
    /// its own thread.
    pub fn acq_start(
        self: &Arc<Self>,
        req: AcqRequest,
        subscriber: Subscriber<AcqEvent>,
    ) -> Result<AcqStarted, AppError> {
        let mut slot = self.jobs.lock();
        if slot.is_some() {
            return Err(job_already_active());
        }
        let (case_dir, case) = policy::known_case(&self.settings(), &req.case_path)?;
        let case_path = req.case_path.clone();
        let ctx = AcqContext {
            case_dir,
            case,
            host: self.host.clone(),
        };
        let job = acquire::start(&self.idevice(), req, ctx)?;
        let id = job.acq_id().to_owned();
        let started = AcqStarted {
            acq_id: id.clone(),
            acq_dir: job.acq_dir().to_string_lossy().into_owned(),
        };
        let stream = Arc::new(Stream::new(subscriber));
        *slot = Some(Job {
            id: id.clone(),
            case_path,
            created_at: job.created_at(),
            handle: JobHandle::Acquisition {
                udid: job.udid().to_owned(),
                control: job.control(),
                stream: Arc::clone(&stream),
            },
        });
        drop(slot);
        log::info!("acquisition {id} started (device {})", job.udid());
        let state = Arc::clone(self);
        let thread_id = id.clone();
        let spawned = thread::Builder::new()
            .name(format!("acq-{id}"))
            .spawn(move || {
                let mut freed = false;
                let outcome = job.run(&mut |event: AcqEvent| {
                    // The slot is free before the UI hears `finished`.
                    if event.is_finished() && !freed {
                        state.jobs.finish(&thread_id);
                        freed = true;
                    }
                    stream.emit(&event);
                });
                let codes: Vec<&str> = outcome
                    .record
                    .status_reasons
                    .iter()
                    .map(|r| r.code.as_str())
                    .collect();
                log::info!(
                    "acquisition {thread_id} finished: {} {codes:?}",
                    outcome.summary.status
                );
                if let Some(e) = &outcome.write_error {
                    log::error!(
                        "acquisition {thread_id}: the final acquisition.json was not written: {e}"
                    );
                }
                // Once only: by now another job (e.g. its later restore) may hold the slot.
                if !freed {
                    state.jobs.finish(&thread_id);
                }
            });
        if let Err(e) = spawned {
            self.jobs.finish(&id);
            log::error!("acquisition {id}: cannot start its thread: {e}");
            return Err(app_error(
                ErrorCode::Internal,
                "The acquisition could not be started",
                Some(e.to_string()),
            ));
        }
        Ok(started)
    }

    /// `acq_cancel`: idempotent, with the semantics by phase of ARCHITECTURE.md §6b;
    /// `acq_not_found` if that acquisition is not the active job.
    pub fn acq_cancel(&self, req: &AcqCancelRequest) -> Result<(), AppError> {
        let slot = self.jobs.lock();
        match slot.as_ref() {
            Some(Job {
                id,
                handle: JobHandle::Acquisition { control, .. },
                ..
            }) if *id == req.acq_id => {
                log::info!("acquisition {id}: cancel requested ({})", control.phase());
                control.cancel();
                Ok(())
            }
            _ => Err(app_error(
                ErrorCode::AcqNotFound,
                "This acquisition is not running",
                Some(req.acq_id.clone()),
            )),
        }
    }

    pub fn acq_get(&self, req: &AcqRef) -> Result<AcquisitionRecord, AppError> {
        let (case_dir, _) = policy::known_case(&self.settings(), &req.case_path)?;
        Ok(acquire::load(&case_dir, &req.acq_id)?)
    }

    /// `acq_restore_encryption`: a later restore, which counts as a job (refused while another
    /// runs, and blocks others while it runs). The password is dropped with the request.
    pub fn acq_restore_encryption(
        &self,
        req: AcqRestoreEncryptionRequest,
    ) -> Result<AcqRestoreEncryptionResult, AppError> {
        let (case_dir, _) = policy::known_case(&self.settings(), &req.case_path)?;
        let record = acquire::load(&case_dir, &req.acq_id)?;
        {
            let mut slot = self.jobs.lock();
            if slot.is_some() {
                return Err(job_already_active());
            }
            *slot = Some(Job {
                id: req.acq_id.clone(),
                case_path: req.case_path.clone(),
                created_at: Timestamp::now(),
                handle: JobHandle::Restore {
                    udid: record.device.udid.clone(),
                },
            });
        }
        let AcqRestoreEncryptionRequest {
            acq_id, password, ..
        } = req;
        log::info!("acquisition {acq_id}: later encryption restore requested");
        let result = acquire::restore_later(
            &self.idevice(),
            &case_dir,
            &acq_id,
            password,
            // Prompt lines are for the device's owner; they are never logged.
            &mut |_line| {},
        );
        self.jobs.finish(&acq_id);
        match &result {
            Ok(outcome) => log::info!(
                "acquisition {acq_id}: later restore restored={} will_encrypt_after={:?}",
                outcome.restored,
                outcome.will_encrypt_after
            ),
            Err(e) => log::warn!("acquisition {acq_id}: later restore refused or failed: {e}"),
        }
        Ok(result?)
    }

    pub fn open_acq_file(&self, req: &OpenAcqFileRequest) -> Result<(), AppError> {
        let (case_dir, _) = policy::known_case(&self.settings(), &req.case_path)?;
        let dir = policy::acq_dir(&case_dir, &req.acq_id)?;
        let file = dir.join(match req.which {
            AcqFile::Stdout => acquire::record::STDOUT_LOG,
            AcqFile::Stderr => acquire::record::STDERR_LOG,
            AcqFile::AcquisitionJson => acquire::ACQ_FILE,
            AcqFile::BackupManifest => acquire::BACKUP_MANIFEST,
            AcqFile::DeviceInfo => acquire::DEVICE_INFO_FILE,
        });
        if !file.is_file() {
            return Err(app_error(
                ErrorCode::Io,
                "The file does not exist",
                Some(file.display().to_string()),
            ));
        }
        self.open(&file)
    }
}
