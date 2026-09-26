//! Cases, run records, input inspection and profiles (CONTRACTS.md §10).

use std::fs;
use std::path::{Path, PathBuf};

use suitedfir_core::acquire;
use suitedfir_core::case::{self, RUN_FILE};
use suitedfir_core::contracts::{
    AppError, CaseCreateRequest, CaseDetail, CaseFields, CaseFile, CaseSummary, CaseUpdateRequest,
    ErrorCode, InputInspectRequest, InputInspection, IosBackup, ModuleInfo, PathRequest,
    ProfileExportRequest, ProfileImportRequest, ProfileInfo, ProfileRef, ProfileSaveRequest,
    RunRecord, RunRef, Timestamp, ToolId, parse_versioned,
};
use suitedfir_core::fsutil;
use suitedfir_core::inspect::{self, OverlapContext, backups};
use suitedfir_core::run::profile::{ProfileFormat, ProfileStore};
use suitedfir_core::run::record;
use suitedfir_core::settings;

use super::app_error;
use crate::policy;
use crate::state::{AppState, JobHandle};

impl AppState {
    /// A case with its runs and acquisitions (newest first) and the ids recovered by this open.
    fn case_detail(
        &self,
        dir: &Path,
        case: CaseFile,
        recovered: Vec<String>,
    ) -> Result<CaseDetail, AppError> {
        let runs = case::discover_runs(dir)?
            .iter()
            .map(case::run_summary)
            .collect();
        let acquisitions = acquire::discover(dir)?
            .iter()
            .map(acquire::summary)
            .collect();
        Ok(CaseDetail {
            path: dir.to_string_lossy().into_owned(),
            case,
            runs,
            acquisitions,
            recovered,
        })
    }

    pub fn cases_list(&self) -> Vec<CaseSummary> {
        case::list(&self.settings())
    }

    /// `case_create`: in `parent_dir`, or the cases root when it is `null` (the default root is
    /// created on first use). The case becomes known and most recent.
    pub fn case_create(&self, req: &CaseCreateRequest) -> Result<CaseDetail, AppError> {
        let settings = self.settings();
        let parent = match &req.parent_dir {
            Some(dir) => policy::cases_parent(Path::new(dir), &self.paths, &settings)?,
            None => {
                let root = PathBuf::from(&settings.cases_root);
                fs::create_dir_all(&root).map_err(|e| {
                    app_error(
                        fsutil::io_error_code(&e),
                        "The cases folder could not be created",
                        Some(format!("{}: {e}", root.display())),
                    )
                })?;
                policy::cases_parent(&root, &self.paths, &settings)?
            }
        };
        let fields = CaseFields {
            name: req.name.clone(),
            case_number: req.case_number.clone(),
            examiner: req.examiner.clone(),
            agency: req.agency.clone(),
            description: req.description.clone(),
            default_timezone: req.default_timezone.clone(),
        };
        let created = case::create(&parent, &fields)?;
        let path = created.path.to_string_lossy().into_owned();
        self.update_settings(|next| {
            settings::touch_recent(next, &path);
            Ok(())
        })?;
        log::info!("case created: {path}");
        self.case_detail(&created.path, created.case, Vec::new())
    }

    /// `case_open`: any folder with a valid `case.json`, which becomes known and most recent. Runs
    /// and acquisitions left `running` by a crash are marked `interrupted` (all but this
    /// process's active job), with the job slot locked so no job of the case starts meanwhile
    /// (after any start under way has settled: its new record is never taken for a crash's).
    pub fn case_open(&self, req: &PathRequest) -> Result<CaseDetail, AppError> {
        let dir = PathBuf::from(&req.path);
        let (_, case) = self.update_settings(|next| Ok(case::open(&dir, next)?))?;
        let jobs = self.jobs.lock_settled();
        let (active_run, active_acq) = match jobs.job.as_ref().map(|job| (&job.handle, &job.id)) {
            Some((JobHandle::Run { .. }, id)) => (Some(id.as_str()), None),
            Some((JobHandle::Acquisition { .. }, id)) => (None, Some(id.as_str())),
            _ => (None, None),
        };
        let now = Timestamp::now();
        let mut recovered = record::recover_case(&dir, active_run, now)?;
        recovered.extend(acquire::recover_case(&dir, active_acq, now)?);
        drop(jobs);
        for id in &recovered {
            log::warn!(
                "case {}: {id} was left running; marked interrupted",
                req.path
            );
        }
        self.case_detail(&dir, case, recovered)
    }

    pub fn case_update(&self, req: &CaseUpdateRequest) -> Result<CaseDetail, AppError> {
        let (dir, _) = policy::known_case(&self.settings(), &req.path)?;
        let case = case::update(&dir, &req.fields)?;
        self.case_detail(&dir, case, Vec::new())
    }

    /// `case_forget`: removes the case from the recent list only. A case in the list whose folder
    /// is gone can be forgotten too.
    pub fn case_forget(&self, req: &PathRequest) -> Result<(), AppError> {
        self.update_settings(|next| {
            if !settings::is_recent(next, &req.path) {
                return Err(app_error(
                    ErrorCode::CaseNotFound,
                    "This case is not in the recent list",
                    Some(req.path.clone()),
                ));
            }
            settings::forget_recent(next, &req.path);
            Ok(())
        })?;
        Ok(())
    }

    pub fn run_get(&self, req: &RunRef) -> Result<RunRecord, AppError> {
        let (case_dir, _) = policy::known_case(&self.settings(), &req.case_path)?;
        let dir = policy::run_dir(&case_dir, &req.run_id)?;
        let file = dir.join(RUN_FILE);
        let bytes = fs::read(&file).map_err(|e| {
            app_error(
                fsutil::io_error_code(&e),
                "The run record could not be read",
                Some(format!("{}: {e}", file.display())),
            )
        })?;
        let run: RunRecord = parse_versioned(&bytes).map_err(|e| {
            app_error(
                ErrorCode::Io,
                "The run record is not valid",
                Some(format!("{}: {e}", file.display())),
            )
        })?;
        if run.run_id != req.run_id {
            return Err(app_error(
                ErrorCode::RunNotFound,
                "The run record names another run",
                Some(file.display().to_string()),
            ));
        }
        Ok(run)
    }

    /// The folders an input must stay clear of (ARCHITECTURE.md §6 step 1), for `input_inspect`.
    pub fn input_inspect(&self, req: &InputInspectRequest) -> Result<InputInspection, AppError> {
        let settings = self.settings();
        let (case_dir, _) = policy::known_case(&settings, &req.case_path)?;
        let manifest = self.manifest.tools.get(&req.tool).ok_or_else(|| {
            app_error(
                ErrorCode::Internal,
                "Unknown tool",
                Some(req.tool.to_string()),
            )
        })?;
        let known: Vec<PathBuf> = settings.recent_cases.iter().map(PathBuf::from).collect();
        let app_dirs = self.paths.app_dirs(&settings);
        let temp_root = self.paths.temp_root();
        let overlap = OverlapContext {
            case_dir: &case_dir,
            known_cases: &known,
            app_dirs: &app_dirs,
            temp_root: &temp_root,
        };
        Ok(inspect::inspect(
            Path::new(&req.path),
            &manifest.input_types,
            &overlap,
        )?)
    }

    /// `ios_backups_find` (S1): the backups in the OS's default Finder/iTunes backup folders
    /// (none on Linux), read-only. A folder the app may not read is `permission_denied`, with the
    /// Full Disk Access guidance on macOS.
    pub fn ios_backups_find(&self) -> Result<Vec<IosBackup>, AppError> {
        Ok(backups::find(&self.ios_backup_dirs)?)
    }

    // ---- profiles ----

    fn profile_store(&self, tool: ToolId) -> Result<ProfileStore, AppError> {
        let manifest = self.manifest.tools.get(&tool).ok_or_else(|| {
            app_error(ErrorCode::Internal, "Unknown tool", Some(tool.to_string()))
        })?;
        Ok(ProfileStore::new(
            self.paths.profiles_dir(tool),
            ProfileFormat::from_manifest(tool, manifest),
        ))
    }

    /// The installed tool's module names; empty when it is not installed (every name is then
    /// unknown).
    fn available_modules(&self, tool: ToolId) -> Vec<ModuleInfo> {
        self.modules_file(tool)
            .map(|modules| modules.modules)
            .unwrap_or_default()
    }

    pub fn profiles_list(&self, tool: ToolId) -> Result<Vec<ProfileInfo>, AppError> {
        Ok(self
            .profile_store(tool)?
            .list(&self.available_modules(tool))?)
    }

    pub fn profile_save(&self, req: &ProfileSaveRequest) -> Result<ProfileInfo, AppError> {
        let available = self.available_modules(req.tool);
        Ok(self
            .profile_store(req.tool)?
            .save(&req.name, &req.modules, &available)?)
    }

    pub fn profile_delete(&self, req: &ProfileRef) -> Result<(), AppError> {
        Ok(self.profile_store(req.tool)?.delete(&req.name)?)
    }

    pub fn profile_import(&self, req: &ProfileImportRequest) -> Result<ProfileInfo, AppError> {
        let path = policy::readable_file(Path::new(&req.path))?;
        let available = self.available_modules(req.tool);
        Ok(self.profile_store(req.tool)?.import(
            &path,
            req.name.as_deref(),
            req.overwrite,
            &available,
        )?)
    }

    pub fn profile_export(&self, req: &ProfileExportRequest) -> Result<(), AppError> {
        let dest = policy::export_dest(Path::new(&req.dest_path), &self.settings())?;
        Ok(self.profile_store(req.tool)?.export(&req.name, &dest)?)
    }
}
