//! `settings.json` and `case.json` (CONTRACTS.md §6), and the LEAPP-native profile and case-data
//! files (§8).

use serde::{Deserialize, Serialize};

use super::{Timestamp, VersionedFile};

/// `<app_config>/settings.json` (§6). Also the `settings_get` / `settings_update` response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub schema_version: u32,
    pub cases_root: String,
    /// Deduplicated, most recent first, at most 50.
    pub recent_cases: Vec<String>,
    pub defaults: SettingsDefaults,
    /// Tools-directory override; `null` = the default `<app_data>/leapp`.
    pub tools_dir: Option<String>,
}

impl VersionedFile for Settings {
    const FILE: &'static str = "settings.json";
}

/// Defaults for new cases and runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsDefaults {
    pub examiner: String,
    pub agency: String,
    pub timezone: String,
}

/// `<case>/case.json` (§6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseFile {
    pub schema_version: u32,
    /// 32 lowercase hex characters from the OS RNG.
    pub case_id: String,
    /// 1–120 characters.
    pub name: String,
    pub case_number: String,
    pub examiner: String,
    pub agency: String,
    pub description: String,
    /// `null` = fall back to settings.
    pub default_timezone: Option<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub created_by_app_version: String,
}

impl VersionedFile for CaseFile {
    const FILE: &'static str = "case.json";
}

/// A LEAPP profile (`.ilprofile` / `.alprofile`), LEAPP's native format (§8). It has no
/// `schema_version`; importers reject a wrong `leapp` value or a `format_version` other than 1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeappProfile {
    /// The tool's `profile_leapp_id` (`ileapp` or `aleapp`).
    pub leapp: String,
    pub format_version: u32,
    pub plugins: Vec<String>,
}

/// `case.lcasedata`, LEAPP's native case-data format passed with `-d` (§8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeappCaseData {
    /// Always `case_data`.
    pub leapp: String,
    pub case_data_values: CaseDataValues,
}

/// The values LEAPP shows in its report header. The keys are LEAPP's, not snake_case.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseDataValues {
    #[serde(rename = "Case Number")]
    pub case_number: String,
    #[serde(rename = "Agency")]
    pub agency: String,
    #[serde(rename = "Examiner")]
    pub examiner: String,
}
