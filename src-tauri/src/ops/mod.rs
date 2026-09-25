//! What each command does, as methods of [`AppState`] without Tauri types (the `#[tauri::command]`
//! wrappers in `commands.rs` only run them off the command thread and forward events): app info,
//! settings and tools here; cases, runs and profiles in `cases`; the job commands in `jobs`; the iOS
//! device commands in `devices`.

mod cases;
mod devices;
mod jobs;
#[cfg(test)]
mod tests;

pub use jobs::AttachSubscriber;

use std::path::Path;

use suitedfir_core::contracts::{
    AppError, AppInfo, AppInfoPaths, ErrorCode, InstallEvent, ModulesFile, Settings,
    SettingsUpdateRequest, ToolId, ToolManifest, ToolModules, ToolState, ToolStatus,
    ToolsDirUpdate, parse_versioned,
};
use suitedfir_core::leapp::install::{self, MODULES_FILE, Source};
use suitedfir_core::leapp::modules;
use suitedfir_core::runner::{self, PreparedTool};
use suitedfir_core::settings;

use crate::policy;
use crate::state::{AppState, lock};

/// `THIRD-PARTY-NOTICES.md`, embedded at build time (F2 regenerates it).
pub const LICENSES: &str = include_str!("../../../THIRD-PARTY-NOTICES.md");

pub(crate) fn app_error(
    code: ErrorCode,
    message: impl Into<String>,
    detail: Option<String>,
) -> AppError {
    AppError {
        code,
        message: message.into(),
        detail,
    }
}

/// Where `tool_install` / `tool_import` take the pinned asset from.
pub enum InstallFrom<'a> {
    /// The manifest URLs.
    Download,
    /// A local copy of the release asset (offline import).
    File(&'a Path),
}

impl AppState {
    // ---- app and settings ----

    pub fn app_info(&self) -> AppInfo {
        let settings = self.settings();
        let text = |path: &Path| path.to_string_lossy().into_owned();
        AppInfo {
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            platform: self.platform,
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            dev_override: self.dev_override_active(),
            paths: AppInfoPaths {
                app_data: text(&self.paths.app_data),
                app_config: text(&self.paths.app_config),
                app_cache: text(&self.paths.app_cache),
                app_log: text(&self.paths.app_log),
                tools_dir: text(&self.paths.tools_dir(&settings)),
            },
        }
    }

    /// `settings_update` (CONTRACTS.md §10): omitted fields are unchanged, `defaults` replaces all
    /// three, `tools_dir: null` resets it. The new `cases_root` and `tools_dir` follow the path
    /// policy, checked against the settings as they will be.
    pub fn settings_update(&self, req: &SettingsUpdateRequest) -> Result<Settings, AppError> {
        let tools_changed = !req.tools_dir.is_unchanged();
        let (settings, ()) = self.update_settings(|next| {
            settings::apply_update(next, req);
            if let ToolsDirUpdate::Set(dir) = &req.tools_dir {
                let dir = policy::tools_dir(Path::new(dir), next)?;
                next.tools_dir = Some(dir.to_string_lossy().into_owned());
            }
            if let Some(root) = &req.cases_root {
                let root = policy::cases_parent(Path::new(root), &self.paths, next)?;
                next.cases_root = root.to_string_lossy().into_owned();
            }
            Ok(())
        })?;
        if tools_changed {
            // A different tools dir: nothing there was verified in this session.
            lock(&self.verified).clear();
        }
        Ok(settings)
    }

    // ---- tools ----

    fn tool_manifest(&self, tool: ToolId) -> Result<&ToolManifest, AppError> {
        self.manifest.tools.get(&tool).ok_or_else(|| {
            app_error(
                ErrorCode::Internal,
                format!("The tool manifest has no entry for {tool}"),
                None,
            )
        })
    }

    /// A tool's state. Without `verify` it is cheap (no hashing): an installed tool is
    /// `installed_unverified` unless it was verified in this session.
    pub fn tool_status(&self, tool: ToolId, verify: bool) -> Result<ToolStatus, AppError> {
        #[cfg(debug_assertions)]
        if let Some(entry) = self.leapp_override.get(&tool) {
            let manifest = self.tool_manifest(tool)?;
            return Ok(suitedfir_core::leapp::dev_override::status(
                tool, manifest, entry,
            ));
        }
        let settings = self.settings();
        let check = self.with_pinned(tool, &settings, |pinned| {
            if verify {
                install::verify(pinned)
            } else {
                install::status(pinned)
            }
        })?;
        let mut status = check.status;
        let mut verified = lock(&self.verified);
        if verify {
            if status.state == ToolState::Verified {
                verified.insert(tool);
            } else {
                verified.remove(&tool);
            }
        } else if status.state == ToolState::InstalledUnverified && verified.contains(&tool) {
            status.state = ToolState::Verified;
        }
        Ok(status)
    }

    pub fn tools_status(&self) -> Result<Vec<ToolStatus>, AppError> {
        ToolId::ALL
            .iter()
            .map(|tool| self.tool_status(*tool, false))
            .collect()
    }

    /// The module list of an installed (or dev-override) tool.
    pub fn modules_file(&self, tool: ToolId) -> Result<ModulesFile, AppError> {
        #[cfg(debug_assertions)]
        if let Some(entry) = self.leapp_override.get(&tool) {
            return suitedfir_core::leapp::dev_override::modules(entry, tool);
        }
        let status = self.tool_status(tool, false)?;
        let dir = match (status.state, &status.install_dir) {
            (ToolState::InstalledUnverified | ToolState::Verified, Some(dir)) => dir.clone(),
            (ToolState::UnsupportedPlatform, _) => {
                return Err(app_error(
                    ErrorCode::UnsupportedPlatform,
                    format!("{} has no build for this platform", status.display_name),
                    status.problem,
                ));
            }
            _ => {
                return Err(app_error(
                    ErrorCode::ToolNotInstalled,
                    format!("{} is not installed", status.display_name),
                    status.problem,
                ));
            }
        };
        let path = Path::new(&dir).join(MODULES_FILE);
        std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| parse_versioned(&bytes).map_err(|e| e.to_string()))
            .map_err(|why| {
                app_error(
                    ErrorCode::ToolNotInstalled,
                    format!(
                        "{}'s module list is unusable. Reinstall it in Settings.",
                        status.display_name
                    ),
                    Some(format!("{}: {why}", path.display())),
                )
            })
    }

    pub fn tool_modules(&self, tool: ToolId) -> Result<ToolModules, AppError> {
        let modules = self.modules_file(tool)?;
        Ok(ToolModules {
            tool: modules.tool,
            version: modules.version,
            always_run: modules.always_run,
            timezones: modules.timezones,
            modules: modules.modules,
        })
    }

    /// `tool_install` / `tool_import`: download (or import) → verify → extract → verify the entry
    /// → introspect → `install.json` (ROADMAP A2, A3). One install per tool at a time; the quit
    /// guard can cancel it.
    pub fn tool_install(
        &self,
        tool: ToolId,
        from: InstallFrom<'_>,
        on_event: &mut dyn FnMut(InstallEvent),
    ) -> Result<ToolStatus, AppError> {
        let manifest = self.tool_manifest(tool)?;
        let slot = self.installs.begin(tool, &manifest.display_name)?;
        // Introspection keeps a temp dir in `<app_cache>/tmp` (this waits out a running sweep).
        let _tmp = self.tmp.enter();
        lock(&self.verified).remove(&tool);
        let settings = self.settings();
        let app_cache = self.paths.app_cache.clone();
        let source = match from {
            InstallFrom::Download => self.download_source(),
            InstallFrom::File(path) => Source::File(path),
        };
        let result = self.with_pinned(tool, &settings, |pinned| {
            install::install(pinned, source, &slot.cancel, on_event, &mut |entry| {
                modules::introspect(entry, tool, manifest, &app_cache).map_err(Into::into)
            })
        })?;
        drop(slot);
        match result {
            Ok(record) => {
                log::info!(
                    "{tool} {} installed from {} ({} modules)",
                    record.version,
                    record.source,
                    record.module_count
                );
                // The entry was hashed against the manifest (or recorded) a moment ago.
                lock(&self.verified).insert(tool);
                self.tool_status(tool, false)
            }
            Err(e) => {
                log::warn!("installing {tool} failed: {e}");
                Err(e.into())
            }
        }
    }

    /// The manifest URLs; in tests, the local file standing in for the network.
    fn download_source(&self) -> Source<'_> {
        #[cfg(test)]
        if let Some(file) = &self.download_from {
            return Source::File(file);
        }
        Source::Download
    }

    /// The tool for a run, checked right before it (ARCHITECTURE.md §6 step 1): the dev override,
    /// or the installed tool with its entry hash verified.
    pub fn prepared_tool(
        &self,
        tool: ToolId,
        settings: &Settings,
    ) -> Result<PreparedTool, AppError> {
        #[cfg(debug_assertions)]
        if let Some(entry) = self.leapp_override.get(&tool) {
            return runner::dev_override_tool(
                entry,
                tool,
                self.tool_manifest(tool)?,
                self.platform,
            );
        }
        let prepared = self.with_pinned(tool, settings, runner::installed_tool)?;
        let mut verified = lock(&self.verified);
        match &prepared {
            Ok(_) => verified.insert(tool),
            Err(_) => verified.remove(&tool),
        };
        prepared
    }
}
