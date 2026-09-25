//! LEAPP tool files: `leapp-manifest.json` (CONTRACTS.md §3), `install.json` (§4) and
//! `modules.json` (§5).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{ArchiveKind, InputType, InstallSource, PlatformKey, Timestamp, ToolId, VersionedFile};

/// `leapp-manifest.json` at the repo root, embedded at build time (§3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeappManifest {
    pub schema_version: u32,
    pub tools: BTreeMap<ToolId, ToolManifest>,
}

impl VersionedFile for LeappManifest {
    const FILE: &'static str = "leapp-manifest.json";
}

/// One tool's pinned release in `leapp-manifest.json`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolManifest {
    pub display_name: String,
    pub upstream_repo: String,
    pub version: String,
    pub license: String,
    pub profile_ext: String,
    pub profile_leapp_id: String,
    pub input_types: Vec<InputType>,
    pub supports_timezone: bool,
    pub supports_keychain: bool,
    pub supports_itunes_password: bool,
    /// A platform missing here is unsupported.
    pub platforms: BTreeMap<PlatformKey, PlatformAsset>,
}

/// The pinned release asset of a tool for one platform.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformAsset {
    pub asset_name: String,
    pub asset_size: u64,
    pub asset_sha256: String,
    pub archive_kind: ArchiveKind,
    pub entry: String,
    /// Required for `zip`; may be `null` for `appimage` until E3 reports it.
    pub entry_sha256: Option<String>,
    /// Tried in order; each is `https://`.
    pub urls: Vec<String>,
}

/// `<tools_dir>/<tool>/<version>/install.json` (§4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallRecord {
    pub schema_version: u32,
    pub tool: ToolId,
    pub version: String,
    pub platform: PlatformKey,
    pub asset_name: String,
    pub asset_sha256: String,
    pub entry_path: String,
    pub entry_sha256: String,
    pub source: InstallSource,
    /// The download URL or the imported file's path.
    pub source_detail: String,
    pub installed_at: Timestamp,
    pub module_count: u32,
}

impl VersionedFile for InstallRecord {
    const FILE: &'static str = "install.json";
}

/// `modules.json`, next to `install.json` (§5).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModulesFile {
    pub schema_version: u32,
    pub tool: ToolId,
    pub version: String,
    pub generated_at: Timestamp,
    /// Maps an `InputType` value (or `default`) to artifact names that always run.
    pub always_run: BTreeMap<String, Vec<String>>,
    /// `pytz.all_timezones` from the binary (iLEAPP); `null` for aLEAPP.
    pub timezones: Option<Vec<String>>,
    /// Selectable modules, sorted by `category`, then `display_name` (case-insensitive).
    pub modules: Vec<ModuleInfo>,
}

impl VersionedFile for ModulesFile {
    const FILE: &'static str = "modules.json";
}

/// One selectable module (§5, §9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleInfo {
    /// The artifact key: what profiles contain and LEAPP matches.
    pub name: String,
    pub module_name: String,
    pub category: String,
    pub display_name: String,
    pub description: Option<String>,
}
