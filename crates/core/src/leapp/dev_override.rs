//! The debug-build LEAPP dev override (DEVELOPMENT.md §2): with `SUITEDFIR_DEV_LEAPP_OVERRIDE`
//! pointing at `target/debug/fake-leapp`, both tools report `ToolState.dev_override`, their
//! modules and timezones come from `fake-leapp --list-modules-json <tool>`, verification is skipped
//! and runs record `install_source: dev_override`. The shell reads the variable; this module only
//! asks the binary for its module list. Compiled out of release builds.

use std::path::Path;
use std::process::{Command, Stdio};

use crate::contracts::{
    AppError, ErrorCode, InstallSource, ModulesFile, Timestamp, ToolId, ToolManifest, ToolModules,
    ToolState, ToolStatus, VersionedFile,
};

/// The version every dev-override tool reports.
pub const VERSION: &str = "dev-override";

fn failure(message: String, detail: Option<String>) -> AppError {
    AppError {
        code: ErrorCode::ToolNotInstalled,
        message,
        detail,
    }
}

/// `<entry> --list-modules-json <tool>` as a `modules.json` document.
pub fn modules(entry: &Path, tool: ToolId) -> Result<ModulesFile, AppError> {
    let output = Command::new(entry)
        .arg("--list-modules-json")
        .arg(tool.as_str())
        .stdin(Stdio::null())
        .output()
        .map_err(|e| {
            failure(
                format!("The dev override {} could not be run", entry.display()),
                Some(e.to_string()),
            )
        })?;
    if !output.status.success() {
        return Err(failure(
            format!(
                "The dev override {} could not list its modules",
                entry.display()
            ),
            Some(String::from_utf8_lossy(&output.stderr).into_owned()),
        ));
    }
    let listed: ToolModules = serde_json::from_slice(&output.stdout).map_err(|e| {
        failure(
            format!(
                "The dev override {} printed an invalid module list",
                entry.display()
            ),
            Some(e.to_string()),
        )
    })?;
    if listed.tool != tool {
        return Err(failure(
            format!(
                "The dev override listed the modules of {}, not {tool}",
                listed.tool
            ),
            None,
        ));
    }
    Ok(ModulesFile {
        schema_version: ModulesFile::SCHEMA_VERSION,
        tool,
        version: listed.version,
        generated_at: Timestamp::now(),
        always_run: listed.always_run,
        timezones: listed.timezones,
        modules: listed.modules,
    })
}

/// The `ToolStatus` of a dev-override tool: `dev_override`, version `dev-override`, and the module
/// count of its list (a `problem` if it cannot be listed).
pub fn status(tool: ToolId, manifest: &ToolManifest, entry: &Path) -> ToolStatus {
    let (module_count, problem) = match modules(entry, tool) {
        Ok(listed) => (u32::try_from(listed.modules.len()).ok(), None),
        Err(e) => (
            None,
            Some(format!("{}: {}", e.message, e.detail.unwrap_or_default())),
        ),
    };
    ToolStatus {
        tool,
        display_name: manifest.display_name.clone(),
        pinned_version: manifest.version.clone(),
        state: ToolState::DevOverride,
        installed_version: Some(VERSION.to_owned()),
        install_source: Some(InstallSource::DevOverride),
        module_count,
        install_dir: entry.parent().map(|dir| dir.to_string_lossy().into_owned()),
        problem,
    }
}
