//! The Tauri commands of CONTRACTS.md §10 and §13.5. Each takes at most a `req` object and, where
//! noted, a `Channel` named `on_event` (`onEvent` in JS), and returns `Result<T, AppError>`. They
//! are thin: the work is a method of `AppState` (`ops`), run on a blocking thread so the command
//! thread never waits for I/O, a hash or a process (DEVELOPMENT.md §4.5).

use std::sync::Arc;

use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;

use suitedfir_core::contracts::{
    AcqCancelRequest, AcqEvent, AcqPreflight, AcqPreflightRequest, AcqRef, AcqRequest,
    AcqRestoreEncryptionRequest, AcqRestoreEncryptionResult, AcqStarted, AcquisitionRecord,
    ActiveJob, AppError, AppInfo, CaseCreateRequest, CaseDetail, CaseSummary, CaseUpdateRequest,
    DevicePairRequest, DeviceSummary, DevicesResult, ErrorCode, InputInspectRequest,
    InputInspection, InstallEvent, IosBackup, JobAttachRequest, JobBacklog, JobKind,
    OpenAcqFileRequest, OpenTextFileRequest, PathRequest, ProfileExportRequest,
    ProfileImportRequest, ProfileInfo, ProfileRef, ProfileSaveRequest, RunCancelRequest, RunEvent,
    RunRecord, RunRef, RunRequest, RunStarted, Settings, SettingsUpdateRequest, TempCleanupResult,
    ToolImportRequest, ToolModules, ToolRequest, ToolStatus,
};

use crate::ops::AttachSubscriber;
use crate::ops::{InstallFrom, LICENSES};
use crate::state::AppState;

/// The managed state.
pub type Shared = Arc<AppState>;

/// Runs `work` on a blocking thread.
async fn blocking<T: Send + 'static>(
    state: &Shared,
    work: impl FnOnce(&Shared) -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    let state = Arc::clone(state);
    tauri::async_runtime::spawn_blocking(move || work(&state))
        .await
        .map_err(|e| AppError {
            code: ErrorCode::Internal,
            message: "The command stopped unexpectedly".to_owned(),
            detail: Some(e.to_string()),
        })?
}

/// A `job_attach` event: a run's or an acquisition's, sent as it is.
#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum JobEvent {
    Run(RunEvent),
    Acquisition(AcqEvent),
}

// ---- §10: app, settings, tools ----

#[tauri::command]
pub async fn app_info(state: State<'_, Shared>) -> Result<AppInfo, AppError> {
    blocking(&state, |s| Ok(s.app_info())).await
}

#[tauri::command]
pub async fn licenses_get() -> Result<String, AppError> {
    Ok(LICENSES.to_owned())
}

#[tauri::command]
pub async fn settings_get(state: State<'_, Shared>) -> Result<Settings, AppError> {
    blocking(&state, |s| Ok(s.settings())).await
}

#[tauri::command]
pub async fn settings_update(
    state: State<'_, Shared>,
    req: SettingsUpdateRequest,
) -> Result<Settings, AppError> {
    blocking(&state, move |s| s.settings_update(&req)).await
}

#[tauri::command]
pub async fn tools_status(state: State<'_, Shared>) -> Result<Vec<ToolStatus>, AppError> {
    blocking(&state, |s| s.tools_status()).await
}

#[tauri::command]
pub async fn tool_verify(
    state: State<'_, Shared>,
    req: ToolRequest,
) -> Result<ToolStatus, AppError> {
    blocking(&state, move |s| s.tool_status(req.tool, true)).await
}

#[tauri::command]
pub async fn tool_install(
    state: State<'_, Shared>,
    req: ToolRequest,
    on_event: Channel<InstallEvent>,
) -> Result<ToolStatus, AppError> {
    blocking(&state, move |s| {
        s.tool_install(req.tool, InstallFrom::Download, &mut |event| {
            let _ = on_event.send(event);
        })
    })
    .await
}

#[tauri::command]
pub async fn tool_import(
    state: State<'_, Shared>,
    req: ToolImportRequest,
    on_event: Channel<InstallEvent>,
) -> Result<ToolStatus, AppError> {
    blocking(&state, move |s| {
        let path = crate::policy::readable_file(std::path::Path::new(&req.archive_path))?;
        s.tool_install(req.tool, InstallFrom::File(&path), &mut |event| {
            let _ = on_event.send(event);
        })
    })
    .await
}

#[tauri::command]
pub async fn tool_modules(
    state: State<'_, Shared>,
    req: ToolRequest,
) -> Result<ToolModules, AppError> {
    blocking(&state, move |s| s.tool_modules(req.tool)).await
}

// ---- §10: cases and runs ----

#[tauri::command]
pub async fn cases_list(state: State<'_, Shared>) -> Result<Vec<CaseSummary>, AppError> {
    blocking(&state, |s| Ok(s.cases_list())).await
}

#[tauri::command]
pub async fn case_create(
    state: State<'_, Shared>,
    req: CaseCreateRequest,
) -> Result<CaseDetail, AppError> {
    blocking(&state, move |s| s.case_create(&req)).await
}

#[tauri::command]
pub async fn case_open(state: State<'_, Shared>, req: PathRequest) -> Result<CaseDetail, AppError> {
    blocking(&state, move |s| s.case_open(&req)).await
}

#[tauri::command]
pub async fn case_update(
    state: State<'_, Shared>,
    req: CaseUpdateRequest,
) -> Result<CaseDetail, AppError> {
    blocking(&state, move |s| s.case_update(&req)).await
}

#[tauri::command]
pub async fn case_forget(state: State<'_, Shared>, req: PathRequest) -> Result<(), AppError> {
    blocking(&state, move |s| s.case_forget(&req)).await
}

#[tauri::command]
pub async fn run_get(state: State<'_, Shared>, req: RunRef) -> Result<RunRecord, AppError> {
    blocking(&state, move |s| s.run_get(&req)).await
}

#[tauri::command]
pub async fn input_inspect(
    state: State<'_, Shared>,
    req: InputInspectRequest,
) -> Result<InputInspection, AppError> {
    blocking(&state, move |s| s.input_inspect(&req)).await
}

#[tauri::command]
pub async fn ios_backups_find(state: State<'_, Shared>) -> Result<Vec<IosBackup>, AppError> {
    blocking(&state, |s| s.ios_backups_find()).await
}

// ---- §10: profiles ----

#[tauri::command]
pub async fn profiles_list(
    state: State<'_, Shared>,
    req: ToolRequest,
) -> Result<Vec<ProfileInfo>, AppError> {
    blocking(&state, move |s| s.profiles_list(req.tool)).await
}

#[tauri::command]
pub async fn profile_save(
    state: State<'_, Shared>,
    req: ProfileSaveRequest,
) -> Result<ProfileInfo, AppError> {
    blocking(&state, move |s| s.profile_save(&req)).await
}

#[tauri::command]
pub async fn profile_delete(state: State<'_, Shared>, req: ProfileRef) -> Result<(), AppError> {
    blocking(&state, move |s| s.profile_delete(&req)).await
}

#[tauri::command]
pub async fn profile_import(
    state: State<'_, Shared>,
    req: ProfileImportRequest,
) -> Result<ProfileInfo, AppError> {
    blocking(&state, move |s| s.profile_import(&req)).await
}

#[tauri::command]
pub async fn profile_export(
    state: State<'_, Shared>,
    req: ProfileExportRequest,
) -> Result<(), AppError> {
    blocking(&state, move |s| s.profile_export(&req)).await
}

// ---- §10: jobs ----

#[tauri::command]
pub async fn run_start(
    state: State<'_, Shared>,
    req: RunRequest,
    on_event: Channel<RunEvent>,
) -> Result<RunStarted, AppError> {
    blocking(&state, move |s| {
        s.run_start(
            req,
            Box::new(move |event: &RunEvent| {
                let _ = on_event.send(event.clone());
            }),
        )
    })
    .await
}

#[tauri::command]
pub async fn run_cancel(state: State<'_, Shared>, req: RunCancelRequest) -> Result<(), AppError> {
    blocking(&state, move |s| s.run_cancel(&req)).await
}

#[tauri::command]
pub async fn job_active(state: State<'_, Shared>) -> Result<Option<ActiveJob>, AppError> {
    blocking(&state, |s| Ok(s.job_active())).await
}

#[tauri::command]
pub async fn job_attach(
    state: State<'_, Shared>,
    req: JobAttachRequest,
    on_event: Channel<JobEvent>,
) -> Result<JobBacklog, AppError> {
    blocking(&state, move |s| {
        let subscriber = match req.kind {
            JobKind::Run => AttachSubscriber::Run(Box::new(move |event: &RunEvent| {
                let _ = on_event.send(JobEvent::Run(event.clone()));
            })),
            JobKind::Acquisition => {
                AttachSubscriber::Acquisition(Box::new(move |event: &AcqEvent| {
                    let _ = on_event.send(JobEvent::Acquisition(event.clone()));
                }))
            }
        };
        s.job_attach(&req, subscriber)
    })
    .await
}

#[tauri::command]
pub async fn open_report(state: State<'_, Shared>, req: RunRef) -> Result<(), AppError> {
    blocking(&state, move |s| s.open_report(&req)).await
}

#[tauri::command]
pub async fn reveal_path(state: State<'_, Shared>, req: PathRequest) -> Result<(), AppError> {
    blocking(&state, move |s| s.reveal_path(&req)).await
}

#[tauri::command]
pub async fn open_text_file(
    state: State<'_, Shared>,
    req: OpenTextFileRequest,
) -> Result<(), AppError> {
    blocking(&state, move |s| s.open_text_file(&req)).await
}

#[tauri::command]
pub async fn temp_cleanup(state: State<'_, Shared>) -> Result<TempCleanupResult, AppError> {
    blocking(&state, |s| s.temp_cleanup()).await
}

// ---- §13.5: devices and acquisitions ----

#[tauri::command]
pub async fn devices_list(state: State<'_, Shared>) -> Result<DevicesResult, AppError> {
    blocking(&state, |s| Ok(s.devices_list())).await
}

#[tauri::command]
pub async fn device_pair(
    state: State<'_, Shared>,
    req: DevicePairRequest,
) -> Result<DeviceSummary, AppError> {
    blocking(&state, move |s| s.device_pair(&req)).await
}

#[tauri::command]
pub async fn acq_preflight(
    state: State<'_, Shared>,
    req: AcqPreflightRequest,
) -> Result<AcqPreflight, AppError> {
    blocking(&state, move |s| s.acq_preflight(&req)).await
}

#[tauri::command]
pub async fn acq_start(
    state: State<'_, Shared>,
    req: AcqRequest,
    on_event: Channel<AcqEvent>,
) -> Result<AcqStarted, AppError> {
    blocking(&state, move |s| {
        s.acq_start(
            req,
            Box::new(move |event: &AcqEvent| {
                let _ = on_event.send(event.clone());
            }),
        )
    })
    .await
}

#[tauri::command]
pub async fn acq_cancel(state: State<'_, Shared>, req: AcqCancelRequest) -> Result<(), AppError> {
    blocking(&state, move |s| s.acq_cancel(&req)).await
}

#[tauri::command]
pub async fn acq_get(state: State<'_, Shared>, req: AcqRef) -> Result<AcquisitionRecord, AppError> {
    blocking(&state, move |s| s.acq_get(&req)).await
}

#[tauri::command]
pub async fn acq_restore_encryption(
    state: State<'_, Shared>,
    req: AcqRestoreEncryptionRequest,
) -> Result<AcqRestoreEncryptionResult, AppError> {
    blocking(&state, move |s| s.acq_restore_encryption(req)).await
}

#[tauri::command]
pub async fn open_acq_file(
    state: State<'_, Shared>,
    req: OpenAcqFileRequest,
) -> Result<(), AppError> {
    blocking(&state, move |s| s.open_acq_file(&req)).await
}

/// Every command, for `Builder::invoke_handler` (the app and the E2 replay test).
pub fn handler<R: tauri::Runtime>() -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static
{
    tauri::generate_handler![
        app_info,
        licenses_get,
        settings_get,
        settings_update,
        tools_status,
        tool_verify,
        tool_install,
        tool_import,
        tool_modules,
        cases_list,
        case_create,
        case_open,
        case_update,
        case_forget,
        run_get,
        input_inspect,
        ios_backups_find,
        profiles_list,
        profile_save,
        profile_delete,
        profile_import,
        profile_export,
        run_start,
        run_cancel,
        job_active,
        job_attach,
        open_report,
        reveal_path,
        open_text_file,
        temp_cleanup,
        devices_list,
        device_pair,
        acq_preflight,
        acq_start,
        acq_cancel,
        acq_get,
        acq_restore_encryption,
        open_acq_file,
    ]
}
