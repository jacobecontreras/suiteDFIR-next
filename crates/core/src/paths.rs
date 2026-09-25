//! The bundle of app directories, constructed by the shell and passed in (the core never guesses OS
//! directories). The layout below them is ARCHITECTURE.md §8.

use std::path::{Path, PathBuf};

use crate::contracts::{Settings, ToolId};

/// The app's own directories, as resolved by the shell (Tauri path APIs for the identifier
/// `com.suitedfir.desktop`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppPaths {
    pub app_data: PathBuf,
    pub app_config: PathBuf,
    pub app_cache: PathBuf,
    pub app_log: PathBuf,
}

impl AppPaths {
    /// `<app_config>/settings.json`.
    pub fn settings_file(&self) -> PathBuf {
        self.app_config.join("settings.json")
    }

    /// `<app_data>/instance.lock`.
    pub fn instance_lock(&self) -> PathBuf {
        self.app_data.join("instance.lock")
    }

    /// `<app_data>/leapp`, used unless `settings.tools_dir` overrides it.
    pub fn default_tools_dir(&self) -> PathBuf {
        self.app_data.join("leapp")
    }

    /// The tools directory in effect: the settings override, else [`Self::default_tools_dir`].
    pub fn tools_dir(&self, settings: &Settings) -> PathBuf {
        settings
            .tools_dir
            .as_ref()
            .map_or_else(|| self.default_tools_dir(), PathBuf::from)
    }

    /// `<app_data>/profiles/<tool>`, where stored profiles live.
    pub fn profiles_dir(&self, tool: ToolId) -> PathBuf {
        self.app_data.join("profiles").join(tool.as_str())
    }

    /// `<app_cache>/tmp`, the parent of the per-run temp directories.
    pub fn temp_root(&self) -> PathBuf {
        self.app_cache.join("tmp")
    }

    /// `<app_cache>/tmp/<run_id>`, a run's temp directory.
    pub fn run_temp_dir(&self, run_id: &str) -> PathBuf {
        self.temp_root().join(run_id)
    }

    /// `<app_log>/suitedfir.log`.
    pub fn log_file(&self) -> PathBuf {
        self.app_log.join("suitedfir.log")
    }

    /// Every directory the app owns, including the tools directory in effect. Inputs may not lie
    /// inside them, and case folders may not be created inside them (ARCHITECTURE.md §6, §9).
    pub fn app_dirs(&self, settings: &Settings) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = [
            &self.app_data,
            &self.app_config,
            &self.app_cache,
            &self.app_log,
        ]
        .into_iter()
        .map(|dir| dir.to_path_buf())
        .collect();
        let tools = self.tools_dir(settings);
        if !dirs.iter().any(|dir| Path::starts_with(&tools, dir)) {
            dirs.push(tools);
        }
        dirs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::contracts::examples;

    fn paths() -> AppPaths {
        AppPaths {
            app_data: PathBuf::from("/data"),
            app_config: PathBuf::from("/config"),
            app_cache: PathBuf::from("/cache"),
            app_log: PathBuf::from("/log"),
        }
    }

    #[test]
    fn layout_follows_architecture_8() {
        let p = paths();
        assert_eq!(p.settings_file(), Path::new("/config/settings.json"));
        assert_eq!(p.instance_lock(), Path::new("/data/instance.lock"));
        assert_eq!(p.default_tools_dir(), Path::new("/data/leapp"));
        assert_eq!(
            p.profiles_dir(ToolId::Aleapp),
            Path::new("/data/profiles/aleapp")
        );
        assert_eq!(p.temp_root(), Path::new("/cache/tmp"));
        assert_eq!(
            p.run_temp_dir("20260924-183005Z-ileapp-3f9a1c"),
            Path::new("/cache/tmp/20260924-183005Z-ileapp-3f9a1c")
        );
        assert_eq!(p.log_file(), Path::new("/log/suitedfir.log"));
    }

    #[test]
    fn tools_dir_override_and_app_dirs() {
        let p = paths();
        let mut settings = examples::settings();
        settings.tools_dir = None;
        assert_eq!(p.tools_dir(&settings), Path::new("/data/leapp"));
        // The default tools dir is inside app_data, so it is not listed twice.
        assert_eq!(
            p.app_dirs(&settings),
            ["/data", "/config", "/cache", "/log"].map(PathBuf::from)
        );

        settings.tools_dir = Some("/approved/tools".to_owned());
        assert_eq!(p.tools_dir(&settings), Path::new("/approved/tools"));
        assert_eq!(
            p.app_dirs(&settings),
            ["/data", "/config", "/cache", "/log", "/approved/tools"].map(PathBuf::from)
        );
    }
}
