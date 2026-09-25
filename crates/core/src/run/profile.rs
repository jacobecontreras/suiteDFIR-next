//! Profiles: read, write, validate, import and export (CONTRACTS.md §8), and resolving a
//! `ModuleSelection` into the requested, resolved and unknown module lists.
//!
//! Stored profiles live at `<app_data>/profiles/<tool>/<name>.<ext>` in LEAPP's native format, which
//! is also the import/export format. LEAPP silently drops unknown names (LEAPP-CLI.md Q4), so
//! profiles are validated against the installed module list: saving rejects unknown names, importing
//! keeps and reports them, and a run with unknown names is refused (ARCHITECTURE.md D12).

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::case::sanitize_name;
use crate::contracts::{
    AppError, ErrorCode, InputType, LeappProfile, ModuleInfo, ModuleMode, ModuleSelection,
    ModulesFile, ProfileInfo, RunModules, ToolId, ToolManifest,
};
use crate::fsutil;

/// Profile names are 1–80 characters (§8).
pub const MAX_NAME_CHARS: usize = 80;
/// The only profile `format_version`.
pub const FORMAT_VERSION: u32 = 1;
/// Upstream profiles are a few KiB; anything larger is not a profile.
const MAX_PROFILE_BYTES: u64 = 8 * 1024 * 1024;

/// Errors from profile operations.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("invalid profile name: {0}")]
    InvalidName(&'static str),
    #[error("{path} is not a valid profile: {reason}")]
    Invalid { path: String, reason: String },
    #[error("no profile named {name:?}")]
    NotFound { name: String },
    #[error("a profile named {name:?} already exists")]
    Exists { name: String },
    #[error("unknown modules: {}", names.join(", "))]
    UnknownModules { names: Vec<String> },
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
}

impl ProfileError {
    fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }

    fn invalid(path: &Path, reason: impl Into<String>) -> Self {
        Self::Invalid {
            path: path.display().to_string(),
            reason: reason.into(),
        }
    }

    /// The `AppError` code (CONTRACTS.md §12).
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::InvalidName(_) | Self::Invalid { .. } => ErrorCode::ProfileInvalid,
            Self::NotFound { .. } => ErrorCode::ProfileNotFound,
            Self::Exists { .. } => ErrorCode::ProfileExists,
            Self::UnknownModules { .. } => ErrorCode::UnknownModules,
            Self::Io { source, .. } => fsutil::io_error_code(source),
        }
    }
}

impl From<ProfileError> for AppError {
    fn from(err: ProfileError) -> Self {
        let message = match err.code() {
            ErrorCode::ProfileInvalid => "The profile is not valid",
            ErrorCode::ProfileNotFound => "The profile was not found",
            ErrorCode::ProfileExists => "A profile with this name already exists",
            ErrorCode::UnknownModules => {
                "The profile names modules this tool version does not have"
            }
            ErrorCode::PermissionDenied => "Access to the profile was denied",
            _ => "The profile could not be read or written",
        };
        AppError {
            code: err.code(),
            message: message.to_owned(),
            detail: Some(err.to_string()),
        }
    }
}

/// A tool's profile format: its file extension and the `leapp` value inside (from
/// `leapp-manifest.json`: `profile_ext`, `profile_leapp_id`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileFormat {
    pub tool: ToolId,
    pub ext: String,
    pub leapp_id: String,
}

impl ProfileFormat {
    pub fn from_manifest(tool: ToolId, manifest: &ToolManifest) -> Self {
        Self {
            tool,
            ext: manifest.profile_ext.clone(),
            leapp_id: manifest.profile_leapp_id.clone(),
        }
    }

    /// A profile file with these plugins.
    fn profile(&self, plugins: Vec<String>) -> LeappProfile {
        LeappProfile {
            leapp: self.leapp_id.clone(),
            format_version: FORMAT_VERSION,
            plugins,
        }
    }
}

/// Reads a profile file (read-only) and checks it is this tool's format: the right `leapp` value
/// and `format_version` 1 (else `profile_invalid`). Plugin names are not checked here.
pub fn read_file(path: &Path, format: &ProfileFormat) -> Result<LeappProfile, ProfileError> {
    let meta = fs::metadata(path).map_err(|e| ProfileError::io(path, e))?;
    if !meta.is_file() {
        return Err(ProfileError::invalid(path, "not a regular file"));
    }
    if meta.len() > MAX_PROFILE_BYTES {
        return Err(ProfileError::invalid(path, "the file is too large"));
    }
    let bytes = fs::read(path).map_err(|e| ProfileError::io(path, e))?;
    let profile: LeappProfile =
        serde_json::from_slice(&bytes).map_err(|e| ProfileError::invalid(path, e.to_string()))?;
    if profile.leapp != format.leapp_id {
        return Err(ProfileError::invalid(
            path,
            format!("it is for {:?}, not {:?}", profile.leapp, format.leapp_id),
        ));
    }
    if profile.format_version != FORMAT_VERSION {
        return Err(ProfileError::invalid(
            path,
            format!("format_version {} is not supported", profile.format_version),
        ));
    }
    Ok(profile)
}

/// The names in `plugins` that are not in `available`, once each, in order.
fn unknown_names(plugins: &[String], available: &[ModuleInfo]) -> Vec<String> {
    let known: HashSet<&str> = available.iter().map(|m| m.name.as_str()).collect();
    let mut seen = HashSet::new();
    plugins
        .iter()
        .filter(|name| !known.contains(name.as_str()) && seen.insert(name.as_str()))
        .cloned()
        .collect()
}

/// The stored profiles of one tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileStore {
    dir: PathBuf,
    format: ProfileFormat,
}

impl ProfileStore {
    /// `dir` is `<app_data>/profiles/<tool>` ([`crate::paths::AppPaths::profiles_dir`]).
    pub fn new(dir: PathBuf, format: ProfileFormat) -> Self {
        Self { dir, format }
    }

    pub fn format(&self) -> &ProfileFormat {
        &self.format
    }

    /// The stored name of `name`: 1–80 characters, sanitized like case folder names.
    pub fn stored_name(name: &str) -> Result<String, ProfileError> {
        if name.trim().is_empty() {
            return Err(ProfileError::InvalidName("the name is required"));
        }
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(ProfileError::InvalidName(
                "the name is longer than 80 characters",
            ));
        }
        Ok(sanitize_name(name))
    }

    fn path(&self, stored_name: &str) -> PathBuf {
        self.dir.join(format!("{stored_name}.{}", self.format.ext))
    }

    fn info(&self, name: String, profile: LeappProfile, available: &[ModuleInfo]) -> ProfileInfo {
        ProfileInfo {
            tool: self.format.tool,
            name,
            unknown_modules: unknown_names(&profile.plugins, available),
            modules: profile.plugins,
        }
    }

    fn write(&self, stored_name: &str, plugins: Vec<String>) -> Result<LeappProfile, ProfileError> {
        fs::create_dir_all(&self.dir).map_err(|e| ProfileError::io(&self.dir, e))?;
        let path = self.path(stored_name);
        let profile = self.format.profile(plugins);
        fsutil::write_json_atomic(&path, &profile).map_err(|e| ProfileError::io(&path, e))?;
        Ok(profile)
    }

    /// `profiles_list`: every valid stored profile, by name (case-insensitive). Invalid files are
    /// skipped with a logged warning. `unknown_modules` is computed against `available`.
    pub fn list(&self, available: &[ModuleInfo]) -> Result<Vec<ProfileInfo>, ProfileError> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(ProfileError::io(&self.dir, e)),
        };
        let mut profiles = Vec::new();
        for entry in entries {
            let path = entry.map_err(|e| ProfileError::io(&self.dir, e))?.path();
            let ours = path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case(self.format.ext.as_str()));
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if !ours || !path.is_file() {
                continue;
            }
            match read_file(&path, &self.format) {
                Ok(profile) => profiles.push(self.info(stem.to_owned(), profile, available)),
                Err(e) => log::warn!("skipping profile {}: {e}", path.display()),
            }
        }
        profiles.sort_by(|a, b| {
            (a.name.to_lowercase(), &a.name).cmp(&(b.name.to_lowercase(), &b.name))
        });
        Ok(profiles)
    }

    /// A stored profile (`profile_not_found` if there is none).
    pub fn load(&self, name: &str) -> Result<LeappProfile, ProfileError> {
        let path = self.path(&Self::stored_name(name)?);
        match read_file(&path, &self.format) {
            Err(ProfileError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                Err(ProfileError::NotFound {
                    name: name.to_owned(),
                })
            }
            other => other,
        }
    }

    /// `profile_save`: stores `modules` under `name`, replacing a profile of the same name. Unknown
    /// names are rejected (`unknown_modules`).
    pub fn save(
        &self,
        name: &str,
        modules: &[String],
        available: &[ModuleInfo],
    ) -> Result<ProfileInfo, ProfileError> {
        let stored = Self::stored_name(name)?;
        let unknown = unknown_names(modules, available);
        if !unknown.is_empty() {
            return Err(ProfileError::UnknownModules { names: unknown });
        }
        let profile = self.write(&stored, modules.to_vec())?;
        Ok(self.info(stored, profile, available))
    }

    /// `profile_delete`.
    pub fn delete(&self, name: &str) -> Result<(), ProfileError> {
        let path = self.path(&Self::stored_name(name)?);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(ProfileError::NotFound {
                name: name.to_owned(),
            }),
            Err(e) => Err(ProfileError::io(&path, e)),
        }
    }

    /// `profile_import`: reads `src` (read-only), checks its format, and stores it under `name` or,
    /// when `name` is `None`, the file stem. An existing profile is replaced only with `overwrite`
    /// (else `profile_exists`). Unknown module names are kept and reported.
    pub fn import(
        &self,
        src: &Path,
        name: Option<&str>,
        overwrite: bool,
        available: &[ModuleInfo],
    ) -> Result<ProfileInfo, ProfileError> {
        let profile = read_file(src, &self.format)?;
        let name = match name {
            Some(name) => name.to_owned(),
            None => src
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default(),
        };
        let stored = Self::stored_name(&name)?;
        let target = self.path(&stored);
        let exists = fs::symlink_metadata(&target).is_ok();
        if exists && !overwrite {
            return Err(ProfileError::Exists { name: stored });
        }
        let profile = self.write(&stored, profile.plugins)?;
        Ok(self.info(stored, profile, available))
    }

    /// `profile_export`: writes a stored profile to `dest` (from the save dialog), atomically.
    pub fn export(&self, name: &str, dest: &Path) -> Result<(), ProfileError> {
        let profile = self.load(name)?;
        fsutil::write_json_atomic(dest, &profile).map_err(|e| ProfileError::io(dest, e))
    }
}

/// Resolves a run's module selection (CONTRACTS.md §7.1 `modules`) against the installed tool's
/// `modules.json`:
/// - `all`: `requested` is empty, `resolved` is every selectable name (sorted), `unknown` is empty;
/// - `profile` / `custom`: `requested` is the profile's or the given list, `resolved` the known
///   names and `unknown` the others (each once, in order).
///
/// `always_run` is the entry for `input_type`, else `default`. A non-empty `unknown` blocks the run
/// (the caller refuses it with `unknown_modules`).
pub fn resolve(
    selection: &ModuleSelection,
    modules: &ModulesFile,
    input_type: InputType,
    store: &ProfileStore,
) -> Result<RunModules, ProfileError> {
    let (mode, profile_name, requested) = match selection {
        ModuleSelection::All => (ModuleMode::All, None, Vec::new()),
        ModuleSelection::Profile { profile_name } => (
            ModuleMode::Profile,
            Some(profile_name.clone()),
            store.load(profile_name)?.plugins,
        ),
        ModuleSelection::Custom { modules } => (ModuleMode::Custom, None, modules.clone()),
    };
    let (resolved, unknown) = if mode == ModuleMode::All {
        let mut all: Vec<String> = modules.modules.iter().map(|m| m.name.clone()).collect();
        all.sort();
        all.dedup();
        (all, Vec::new())
    } else {
        let known: HashSet<&str> = modules.modules.iter().map(|m| m.name.as_str()).collect();
        let mut seen = HashSet::new();
        let resolved = requested
            .iter()
            .filter(|name| known.contains(name.as_str()) && seen.insert(name.as_str()))
            .cloned()
            .collect();
        (resolved, unknown_names(&requested, &modules.modules))
    };
    let always_run = modules
        .always_run
        .get(input_type.as_str())
        .or_else(|| modules.always_run.get("default"))
        .cloned()
        .unwrap_or_default();
    Ok(RunModules {
        mode,
        profile_name,
        requested,
        resolved,
        unknown,
        always_run,
        available_count: u32::try_from(modules.modules.len()).unwrap_or(u32::MAX),
    })
}

/// Writes the run's `profile.<ext>` with the resolved modules (lifecycle step 2; not for mode
/// `all`) and returns its path, for `-m`.
pub fn write_run_profile(
    run_dir: &Path,
    format: &ProfileFormat,
    resolved: &[String],
) -> io::Result<PathBuf> {
    let path = run_dir.join(format!("profile.{}", format.ext));
    fsutil::write_json_atomic(&path, &format.profile(resolved.to_vec()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::contracts::examples;

    fn ileapp_format() -> ProfileFormat {
        let manifest = examples::leapp_manifest();
        ProfileFormat::from_manifest(ToolId::Ileapp, &manifest.tools[&ToolId::Ileapp])
    }

    fn aleapp_format() -> ProfileFormat {
        let manifest = examples::leapp_manifest();
        ProfileFormat::from_manifest(ToolId::Aleapp, &manifest.tools[&ToolId::Aleapp])
    }

    fn available() -> Vec<ModuleInfo> {
        ["callHistory", "sms", "accounts"]
            .into_iter()
            .map(|name| ModuleInfo {
                name: name.to_owned(),
                module_name: name.to_owned(),
                category: "C".to_owned(),
                display_name: name.to_owned(),
                description: None,
            })
            .collect()
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn store(dir: &Path) -> ProfileStore {
        ProfileStore::new(dir.join("profiles").join("ileapp"), ileapp_format())
    }

    #[test]
    fn formats_come_from_the_manifest() {
        let format = ileapp_format();
        assert_eq!(
            (format.ext.as_str(), format.leapp_id.as_str()),
            ("ilprofile", "ileapp")
        );
        let format = aleapp_format();
        assert_eq!(
            (format.ext.as_str(), format.leapp_id.as_str()),
            ("alprofile", "aleapp")
        );
    }

    #[test]
    fn save_writes_leapp_native_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let info = store
            .save("Messaging", &strings(&["callHistory", "sms"]), &available())
            .unwrap();
        assert_eq!(
            info,
            ProfileInfo {
                tool: ToolId::Ileapp,
                name: "Messaging".to_owned(),
                modules: strings(&["callHistory", "sms"]),
                unknown_modules: vec![],
            }
        );
        let path = dir.path().join("profiles/ileapp/Messaging.ilprofile");
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"leapp": "ileapp", "format_version": 1, "plugins": ["callHistory", "sms"]})
        );
        // Save overwrites the same name.
        store
            .save("Messaging", &strings(&["sms"]), &available())
            .unwrap();
        assert_eq!(store.load("Messaging").unwrap().plugins, ["sms"]);
    }

    #[test]
    fn save_rejects_unknown_names_and_bad_profile_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let err = store
            .save(
                "P",
                &strings(&["sms", "noSuchModule", "gone", "noSuchModule"]),
                &available(),
            )
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::UnknownModules);
        assert!(
            matches!(&err, ProfileError::UnknownModules { names } if *names == ["noSuchModule", "gone"]),
            "{err:?}"
        );
        assert!(!dir.path().join("profiles").exists(), "nothing written");

        let long = "p".repeat(81);
        for name in ["", "  ", long.as_str()] {
            let err = store
                .save(name, &strings(&["sms"]), &available())
                .unwrap_err();
            assert_eq!(err.code(), ErrorCode::ProfileInvalid, "{name:?}");
        }
        assert!(
            store
                .save(&"p".repeat(80), &strings(&["sms"]), &available())
                .is_ok()
        );
    }

    #[test]
    fn names_are_sanitized_like_case_folders() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let info = store
            .save("a/b: c.", &strings(&["sms"]), &available())
            .unwrap();
        assert_eq!(info.name, "a_b_ c");
        assert!(
            dir.path()
                .join("profiles/ileapp/a_b_ c.ilprofile")
                .is_file()
        );
        let info = store
            .save("../../evil", &strings(&["sms"]), &available())
            .unwrap();
        assert_eq!(info.name, ".._.._evil");
        assert!(
            dir.path()
                .join("profiles/ileapp/.._.._evil.ilprofile")
                .is_file()
        );
        let info = store.save("con", &strings(&["sms"]), &available()).unwrap();
        assert_eq!(info.name, "con_");
        // Looking up by the original name finds the stored one.
        assert_eq!(store.load("a/b: c.").unwrap().plugins, ["sms"]);
    }

    #[test]
    fn list_load_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        assert_eq!(store.list(&available()).unwrap(), vec![], "no folder yet");
        store
            .save("beta", &strings(&["sms"]), &available())
            .unwrap();
        store
            .save("Alpha", &strings(&["callHistory"]), &available())
            .unwrap();
        // Files that are not valid profiles of this tool are skipped.
        let folder = dir.path().join("profiles/ileapp");
        fs::write(folder.join("broken.ilprofile"), "{").unwrap();
        fs::write(
            folder.join("android.ilprofile"),
            r#"{"leapp": "aleapp", "format_version": 1, "plugins": []}"#,
        )
        .unwrap();
        fs::write(folder.join("notes.txt"), "x").unwrap();
        fs::create_dir(folder.join("dir.ilprofile")).unwrap();
        // An existing profile whose modules are gone from this version.
        fs::write(
            folder.join("old.ilprofile"),
            r#"{"leapp": "ileapp", "format_version": 1, "plugins": ["sms", "removed"]}"#,
        )
        .unwrap();

        let list = store.list(&available()).unwrap();
        let names: Vec<&str> = list.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "beta", "old"]);
        assert_eq!(list[2].unknown_modules, ["removed"]);
        assert_eq!(list[2].modules, ["sms", "removed"]);

        store.delete("beta").unwrap();
        let err = store.delete("beta").unwrap_err();
        assert_eq!(err.code(), ErrorCode::ProfileNotFound);
        let err = store.load("beta").unwrap_err();
        assert_eq!(err.code(), ErrorCode::ProfileNotFound);
        assert_eq!(
            store.load("broken").unwrap_err().code(),
            ErrorCode::ProfileInvalid
        );
    }

    #[test]
    fn import_validates_format_and_keeps_unknown_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let src = dir.path().join("Upstream Sample.ilprofile");
        fs::write(
            &src,
            r#"{"leapp": "ileapp", "format_version": 1, "plugins": ["sms", "noSuchModule"], "x": 1}"#,
        )
        .unwrap();
        let info = store.import(&src, None, false, &available()).unwrap();
        assert_eq!(info.name, "Upstream Sample", "defaults to the file stem");
        assert_eq!(info.modules, ["sms", "noSuchModule"]);
        assert_eq!(info.unknown_modules, ["noSuchModule"]);
        assert_eq!(
            store.load("Upstream Sample").unwrap().plugins,
            ["sms", "noSuchModule"]
        );

        // An existing name needs overwrite.
        let err = store.import(&src, None, false, &available()).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ProfileExists);
        fs::write(
            &src,
            r#"{"leapp": "ileapp", "format_version": 1, "plugins": ["accounts"]}"#,
        )
        .unwrap();
        let info = store.import(&src, None, true, &available()).unwrap();
        assert_eq!(info.modules, ["accounts"]);
        // An explicit name.
        let info = store
            .import(&src, Some("Mine"), false, &available())
            .unwrap();
        assert_eq!(info.name, "Mine");
    }

    #[test]
    fn import_rejects_wrong_tool_or_version() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let src = dir.path().join("p.ilprofile");
        for bad in [
            r#"{"leapp": "aleapp", "format_version": 1, "plugins": ["sms"]}"#,
            r#"{"leapp": "ileapp", "format_version": 2, "plugins": ["sms"]}"#,
            r#"{"leapp": "ileapp", "plugins": ["sms"]}"#,
            r#"{"leapp": "ileapp", "format_version": 1, "plugins": "sms"}"#,
            r#"{"leapp": "case_data", "case_data_values": {}}"#,
            "not json",
        ] {
            fs::write(&src, bad).unwrap();
            let err = store.import(&src, None, false, &available()).unwrap_err();
            assert_eq!(err.code(), ErrorCode::ProfileInvalid, "{bad}: {err}");
        }
        let err = store
            .import(dir.path(), Some("x"), false, &available())
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::ProfileInvalid, "a folder");
        let err = store
            .import(
                &dir.path().join("missing.ilprofile"),
                None,
                false,
                &available(),
            )
            .unwrap_err();
        assert_eq!(err.code(), ErrorCode::Io);
        assert!(!dir.path().join("profiles").exists(), "nothing written");
    }

    #[test]
    fn import_leaves_the_source_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let src = dir.path().join("evidence.ilprofile");
        let text = r#"{"leapp":"ileapp","format_version":1,"plugins":["sms"]}"#;
        fs::write(&src, text).unwrap();
        fsutil::set_read_only(&src).unwrap();
        store.import(&src, None, false, &available()).unwrap();
        assert_eq!(fs::read_to_string(&src).unwrap(), text);
        crate::fsutil::test_support::make_writable(&src);
    }

    #[test]
    fn export_writes_the_stored_profile() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        store
            .save("Messaging", &strings(&["callHistory", "sms"]), &available())
            .unwrap();
        let dest = dir.path().join("out").join("Messaging.ilprofile");
        fs::create_dir_all(dest.parent().unwrap()).unwrap();
        store.export("Messaging", &dest).unwrap();
        let exported = read_file(&dest, &ileapp_format()).unwrap();
        assert_eq!(exported, examples::leapp_profile());
        let err = store.export("missing", &dest).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ProfileNotFound);
    }

    fn modules_file() -> ModulesFile {
        let mut file = examples::modules_file();
        file.modules = available();
        file
    }

    #[test]
    fn resolve_all() {
        let dir = tempfile::tempdir().unwrap();
        let resolved = resolve(
            &ModuleSelection::All,
            &modules_file(),
            InputType::Fs,
            &store(dir.path()),
        )
        .unwrap();
        assert_eq!(
            resolved,
            RunModules {
                mode: ModuleMode::All,
                profile_name: None,
                requested: vec![],
                resolved: strings(&["accounts", "callHistory", "sms"]),
                unknown: vec![],
                always_run: strings(&["last_build"]),
                available_count: 3,
            }
        );
    }

    #[test]
    fn resolve_custom_splits_known_and_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let selection = ModuleSelection::Custom {
            modules: strings(&["sms", "noSuchModule", "callHistory", "sms"]),
        };
        let resolved = resolve(
            &selection,
            &modules_file(),
            InputType::Itunes,
            &store(dir.path()),
        )
        .unwrap();
        assert_eq!(resolved.mode, ModuleMode::Custom);
        assert_eq!(resolved.profile_name, None);
        assert_eq!(
            resolved.requested,
            ["sms", "noSuchModule", "callHistory", "sms"]
        );
        assert_eq!(resolved.resolved, ["sms", "callHistory"]);
        assert_eq!(resolved.unknown, ["noSuchModule"]);
        // The itunes entry replaces default.
        assert_eq!(
            resolved.always_run,
            ["itunes_backup_info", "itunes_backup_installed_applications"]
        );
    }

    #[test]
    fn resolve_profile() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        store
            .save("Messaging", &strings(&["callHistory", "sms"]), &available())
            .unwrap();
        let selection = ModuleSelection::Profile {
            profile_name: "Messaging".to_owned(),
        };
        let resolved = resolve(&selection, &modules_file(), InputType::Zip, &store).unwrap();
        assert_eq!(resolved.mode, ModuleMode::Profile);
        assert_eq!(resolved.profile_name.as_deref(), Some("Messaging"));
        assert_eq!(resolved.requested, ["callHistory", "sms"]);
        assert_eq!(resolved.resolved, ["callHistory", "sms"]);
        assert_eq!(resolved.unknown, Vec::<String>::new());
        assert_eq!(resolved.always_run, ["last_build"]);

        let missing = ModuleSelection::Profile {
            profile_name: "Nope".to_owned(),
        };
        let err = resolve(&missing, &modules_file(), InputType::Zip, &store).unwrap_err();
        assert_eq!(err.code(), ErrorCode::ProfileNotFound);
    }

    #[test]
    fn resolve_without_always_run_entries() {
        let dir = tempfile::tempdir().unwrap();
        let mut modules = modules_file();
        modules.always_run.clear();
        let resolved = resolve(
            &ModuleSelection::All,
            &modules,
            InputType::Fs,
            &store(dir.path()),
        )
        .unwrap();
        assert_eq!(resolved.always_run, Vec::<String>::new());
    }

    #[test]
    fn run_profile_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_run_profile(dir.path(), &aleapp_format(), &strings(&["a", "b"])).unwrap();
        assert_eq!(path, dir.path().join("profile.alprofile"));
        let profile = read_file(&path, &aleapp_format()).unwrap();
        assert_eq!(profile.plugins, ["a", "b"]);
        assert_eq!(profile.leapp, "aleapp");
    }
}
