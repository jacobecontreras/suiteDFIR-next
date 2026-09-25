//! Example values for every contract type. The tests use them, and `cargo xtask contracts` writes
//! them to `ui-dev/fixtures/contracts/` (one file per type, see [`fixtures`]).
//!
//! The file-format examples mirror the examples in docs/CONTRACTS.md, with the `…` placeholders
//! filled in. Hashes are SHA-256 digests of a descriptive label, so they look real but are
//! recognizably examples.

use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};

use super::*;

/// The password used by the request examples. Tests check it never leaks into `Debug` output or
/// records.
pub const EXAMPLE_PASSWORD: &str = "example-backup-password";

const CASE_PATH: &str = "/Users/examiner/Documents/suiteDFIR Cases/Operation Nightjar";
const RUN_ID: &str = "20260924-183005Z-ileapp-3f9a1c";
const ACQ_ID: &str = "20260924-171200Z-ios-9c01de";
const UDID: &str = "00008101-000A1B2C3D4E001E";
const INPUT_PATH: &str = "/Volumes/Evidence/00008101-000A1B2C3D4E";
const APP_DATA: &str = "/Users/examiner/Library/Application Support/com.suitedfir.desktop";
const ILEAPP_ASSET: &str = "ileapp-v2026.4.2-macOS_Apple_Silicon.zip";
const ILEAPP_ASSET_SHA256: &str =
    "d99f2d05dbde20ee997de477c38443d4f019d60326a8c6b9456058c6cf590386";
const IDEVICE_DIR: &str = "/Applications/suiteDFIR.app/Contents/MacOS";

/// One generated fixture file.
pub struct Fixture {
    /// File stem in `ui-dev/fixtures/contracts/`, and the export name in the generated `index.js`.
    pub name: &'static str,
    /// The type in `ui/types.d.ts` (`X[]` for a list of union variants), or `None` for file
    /// formats the UI never sees.
    pub ts_type: Option<String>,
    /// True for string enums: the fixture is the array of every value.
    pub is_enum: bool,
    /// Pretty-printed JSON with a trailing newline, fields in declaration order.
    pub text: String,
    reserialize: fn(serde_json::Value) -> serde_json::Result<serde_json::Value>,
}

impl Fixture {
    /// Reads `json` as this fixture's Rust type and serializes it again.
    pub fn reserialize(&self, json: serde_json::Value) -> serde_json::Result<serde_json::Value> {
        (self.reserialize)(json)
    }
}

/// An example that does not survive example → JSON → type unchanged.
#[derive(Debug, thiserror::Error)]
pub enum FixtureError {
    #[error("serializing example {0}: {1}")]
    Json(&'static str, #[source] serde_json::Error),
    #[error("example {0} changes in a JSON round trip")]
    RoundTrip(&'static str),
}

fn reserialize<T: Serialize + DeserializeOwned>(
    json: serde_json::Value,
) -> serde_json::Result<serde_json::Value> {
    serde_json::to_value(serde_json::from_value::<T>(json)?)
}

/// Builds a fixture and checks the round trip example → JSON → type → equal.
fn fixture<T>(name: &'static str, ts_type: Option<&str>, value: T) -> Result<Fixture, FixtureError>
where
    T: Serialize + DeserializeOwned + PartialEq,
{
    let err = |e| FixtureError::Json(name, e);
    let mut text = serde_json::to_string_pretty(&value).map_err(err)?;
    text.push('\n');
    let back: T = serde_json::from_str(&text).map_err(err)?;
    if back != value {
        return Err(FixtureError::RoundTrip(name));
    }
    let list = if text.starts_with('[') { "[]" } else { "" };
    Ok(Fixture {
        name,
        ts_type: ts_type.map(|ts| format!("{ts}{list}")),
        is_enum: false,
        text,
        reserialize: reserialize::<T>,
    })
}

/// A type the UI sees, under its own name.
fn ipc<T>(name: &'static str, value: T) -> Result<Fixture, FixtureError>
where
    T: Serialize + DeserializeOwned + PartialEq,
{
    fixture(name, Some(name), value)
}

/// A file format the UI never sees.
fn file<T>(name: &'static str, value: T) -> Result<Fixture, FixtureError>
where
    T: Serialize + DeserializeOwned + PartialEq,
{
    fixture(name, None, value)
}

/// All values of a string enum.
fn enum_fixture<T>(
    name: &'static str,
    ts_type: Option<&str>,
    all: &[T],
) -> Result<Fixture, FixtureError>
where
    T: Serialize + DeserializeOwned + PartialEq + Clone,
{
    let mut f = fixture(name, None, all.to_vec())?;
    f.ts_type = ts_type.map(str::to_owned);
    f.is_enum = true;
    Ok(f)
}

/// Every fixture, in a fixed order. Fails if any example does not round-trip.
pub fn fixtures() -> Result<Vec<Fixture>, FixtureError> {
    macro_rules! enums {
        ($($name:ident),+ $(,)?) => {
            vec![$(enum_fixture(stringify!($name), Some(stringify!($name)), $name::ALL)?),+]
        };
    }
    let mut out = vec![
        // File formats.
        file("LeappManifest", leapp_manifest())?,
        file("InstallRecord", install_record())?,
        file("ModulesFile", modules_file())?,
        ipc("Settings", settings())?,
        ipc("CaseFile", case_file())?,
        ipc("RunRecord", run_record())?,
        fixture("RunRecordInitial", Some("RunRecord"), run_record_initial())?,
        file("LeappProfile", leapp_profile())?,
        file("LeappCaseData", leapp_case_data())?,
        file("IdeviceToolsManifest", idevice_tools_manifest())?,
        ipc("AcquisitionRecord", acquisition_record())?,
        file("EncryptionRestoreRecord", encryption_restore_record())?,
        // §9 shared IPC types.
        ipc("AppError", app_error())?,
        ipc("Reason", modules_errored())?,
        ipc("ToolStatus", tool_status())?,
        ipc("ToolModules", tool_modules())?,
        ipc("ModuleInfo", call_history())?,
        ipc("CaseSummary", case_summary())?,
        ipc("CaseDetail", case_detail())?,
        ipc("RunSummary", run_summary())?,
        ipc("InputInspection", input_inspection())?,
        ipc("ModuleSelection", module_selections())?,
        ipc("RunRequest", run_request())?,
        ipc("ActiveJob", active_jobs())?,
        ipc("ProfileInfo", profile_info())?,
        ipc("IosBackup", ios_backup())?,
        // §10 requests and responses.
        ipc("AppInfo", app_info())?,
        ipc("SettingsUpdateRequest", settings_update())?,
        ipc(
            "ToolRequest",
            ToolRequest {
                tool: ToolId::Ileapp,
            },
        )?,
        ipc("ToolImportRequest", tool_import())?,
        ipc("CaseCreateRequest", case_create())?,
        ipc("PathRequest", PathRequest { path: s(CASE_PATH) })?,
        ipc("CaseUpdateRequest", case_update())?,
        ipc("CaseFields", case_fields())?,
        ipc("RunRef", run_ref())?,
        ipc("InputInspectRequest", input_inspect())?,
        ipc("ProfileSaveRequest", profile_save())?,
        ipc("ProfileRef", profile_ref())?,
        ipc("ProfileImportRequest", profile_import())?,
        ipc("ProfileExportRequest", profile_export())?,
        ipc("RunStarted", run_started())?,
        ipc("RunCancelRequest", run_cancel())?,
        ipc("JobAttachRequest", job_attach())?,
        ipc("JobBacklog", job_backlog())?,
        ipc("OpenTextFileRequest", open_text_file())?,
        ipc("TempCleanupResult", temp_cleanup())?,
        // §11 events.
        ipc("RunEvent", run_events())?,
        ipc("InstallEvent", install_events())?,
        // §13.5 acquisition.
        ipc("DeviceSummary", device_summary())?,
        ipc("DevicesResult", devices_result())?,
        ipc("DevicePairRequest", device_pair())?,
        ipc("AcqPreflightRequest", acq_preflight_request())?,
        ipc("AcqPreflight", acq_preflight())?,
        ipc("AcqRequest", acq_request())?,
        ipc("AcqStarted", acq_started())?,
        ipc("AcqCancelRequest", acq_cancel())?,
        ipc("AcqRef", acq_ref())?,
        ipc(
            "AcqRestoreEncryptionRequest",
            acq_restore_encryption_request(),
        )?,
        ipc(
            "AcqRestoreEncryptionResult",
            acq_restore_encryption_result(),
        )?,
        ipc("OpenAcqFileRequest", open_acq_file())?,
        ipc("AcqSummary", acq_summary())?,
        ipc("AcqEvent", acq_events())?,
    ];
    out.extend(enums![
        ToolId,
        PlatformKey,
        InputKind,
        InputType,
        ModuleMode,
        RunStatus,
        HashStatus,
        SealStatus,
        InstallSource,
        EntryVerifiedAgainst,
        ToolState,
        RunPhase,
        AcqStatus,
        AcqPhase,
        PairState,
        IdeviceToolSource,
        IdeviceToolsState,
        ToolVerification,
        RestoreState,
        DevicePromptKind,
        ErrorCode,
        HashAlgorithm,
        InstallStage,
        StdStream,
        JobKind,
        RunFile,
        AcqFile,
        PreflightLevel,
        AcqCommandPurpose,
        DeviceChangeKind,
        PasswordChannel,
    ]);
    out.push(enum_fixture("ArchiveKind", None, ArchiveKind::ALL)?);
    Ok(out)
}

// ---- helpers ----

fn s(text: &str) -> String {
    text.to_owned()
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| s(item)).collect()
}

/// An example timestamp. Provably infallible: every caller passes a valid string literal, and the
/// tests build every example.
fn at(text: &str) -> Timestamp {
    Timestamp::parse(text).expect("example timestamps are valid literals")
}

/// A realistic-looking example hash: the SHA-256 of `label`.
fn example_sha256(label: &str) -> String {
    crate::hashing::to_hex(&Sha256::digest(label.as_bytes()))
}

fn run_dir() -> String {
    format!("{CASE_PATH}/runs/{RUN_ID}")
}

fn acq_dir() -> String {
    format!("{CASE_PATH}/acquisitions/{ACQ_ID}")
}

fn reason(code: &str, message: &str) -> Reason {
    Reason {
        code: s(code),
        message: s(message),
    }
}

// ---- file formats ----

pub fn leapp_manifest() -> LeappManifest {
    let ileapp = ToolManifest {
        display_name: s("iLEAPP"),
        upstream_repo: s("abrignoni/iLEAPP"),
        version: s("v2026.4.2"),
        license: s("MIT"),
        profile_ext: s("ilprofile"),
        profile_leapp_id: s("ileapp"),
        input_types: InputType::ALL.to_vec(),
        supports_timezone: true,
        supports_keychain: true,
        supports_itunes_password: true,
        platforms: [(
            PlatformKey::MacosAarch64,
            PlatformAsset {
                asset_name: s(ILEAPP_ASSET),
                asset_size: 55_085_487,
                asset_sha256: s(ILEAPP_ASSET_SHA256),
                archive_kind: ArchiveKind::Zip,
                entry: s("ileapp"),
                entry_sha256: Some(example_sha256("ileapp entry")),
                urls: vec![format!(
                    "https://github.com/abrignoni/iLEAPP/releases/download/v2026.4.2/{ILEAPP_ASSET}"
                )],
            },
        )]
        .into(),
    };
    let aleapp_asset = "aleapp-v2026.4.1-Linux_x86_64.AppImage";
    let aleapp = ToolManifest {
        display_name: s("aLEAPP"),
        upstream_repo: s("abrignoni/ALEAPP"),
        version: s("v2026.4.1"),
        license: s("MIT"),
        profile_ext: s("alprofile"),
        profile_leapp_id: s("aleapp"),
        input_types: vec![
            InputType::Fs,
            InputType::Tar,
            InputType::Zip,
            InputType::Gz,
            InputType::Raw,
        ],
        supports_timezone: false,
        supports_keychain: false,
        supports_itunes_password: false,
        platforms: [(
            PlatformKey::LinuxX86_64,
            PlatformAsset {
                asset_name: s(aleapp_asset),
                asset_size: 63_834_616,
                asset_sha256: s("0344db7fce169772b807a88fde3ee2eb7ff06c12c9c360b29928d50e7fc854be"),
                archive_kind: ArchiveKind::Appimage,
                entry: s("usr/bin/aleapp"),
                entry_sha256: None,
                urls: vec![format!(
                    "https://github.com/abrignoni/ALEAPP/releases/download/v2026.4.1/{aleapp_asset}"
                )],
            },
        )]
        .into(),
    };
    LeappManifest {
        schema_version: LeappManifest::SCHEMA_VERSION,
        tools: [(ToolId::Ileapp, ileapp), (ToolId::Aleapp, aleapp)].into(),
    }
}

pub fn install_record() -> InstallRecord {
    InstallRecord {
        schema_version: InstallRecord::SCHEMA_VERSION,
        tool: ToolId::Ileapp,
        version: s("v2026.4.2"),
        platform: PlatformKey::MacosAarch64,
        asset_name: s(ILEAPP_ASSET),
        asset_sha256: s(ILEAPP_ASSET_SHA256),
        entry_path: s("bin/ileapp"),
        entry_sha256: example_sha256("ileapp entry"),
        source: InstallSource::Download,
        source_detail: format!(
            "https://github.com/abrignoni/iLEAPP/releases/download/v2026.4.2/{ILEAPP_ASSET}"
        ),
        installed_at: at("2026-09-24T18:00:00Z"),
        module_count: 1176,
    }
}

pub fn call_history() -> ModuleInfo {
    ModuleInfo {
        name: s("callHistory"),
        module_name: s("callHistory"),
        category: s("Call History"),
        display_name: s("Call History"),
        description: None,
    }
}

fn modules_list() -> Vec<ModuleInfo> {
    vec![
        call_history(),
        ModuleInfo {
            name: s("sms"),
            module_name: s("sms"),
            category: s("SMS & iMessage"),
            display_name: s("SMS & iMessage"),
            description: Some(s("Messages from sms.db")),
        },
    ]
}

fn ileapp_always_run() -> std::collections::BTreeMap<String, Vec<String>> {
    [
        (s("default"), strings(&["last_build"])),
        (
            s("itunes"),
            strings(&["itunes_backup_info", "itunes_backup_installed_applications"]),
        ),
    ]
    .into()
}

pub fn modules_file() -> ModulesFile {
    ModulesFile {
        schema_version: ModulesFile::SCHEMA_VERSION,
        tool: ToolId::Ileapp,
        version: s("v2026.4.2"),
        generated_at: at("2026-09-24T18:00:05Z"),
        always_run: ileapp_always_run(),
        timezones: Some(strings(&["Africa/Abidjan", "America/Chicago", "UTC"])),
        modules: modules_list(),
    }
}

pub fn settings() -> Settings {
    Settings {
        schema_version: Settings::SCHEMA_VERSION,
        cases_root: s("/Users/examiner/Documents/suiteDFIR Cases"),
        recent_cases: vec![s(CASE_PATH)],
        defaults: SettingsDefaults {
            examiner: s("J. Doe"),
            agency: s("County Forensics Lab"),
            timezone: s("UTC"),
        },
        tools_dir: None,
    }
}

pub fn case_file() -> CaseFile {
    CaseFile {
        schema_version: CaseFile::SCHEMA_VERSION,
        case_id: s("5b0c2f4e9a7d4b1f8c3e6a2d1f0b9e7c"),
        name: s("Operation Nightjar"),
        case_number: s("2026-0142"),
        examiner: s("J. Doe"),
        agency: s("County Forensics Lab"),
        description: s(""),
        default_timezone: Some(s("America/Chicago")),
        created_at: at("2026-09-24T18:10:00Z"),
        updated_at: at("2026-09-24T18:10:00Z"),
        created_by_app_version: s("0.2.0"),
    }
}

fn record_app() -> RecordApp {
    RecordApp {
        name: s("suiteDFIR"),
        version: s("0.2.0"),
    }
}

fn record_host() -> RecordHost {
    RecordHost {
        os: s("macos"),
        os_version: s("15.6"),
        arch: s("aarch64"),
        hostname: s("LAB-MAC-01"),
    }
}

fn case_snapshot() -> CaseSnapshot {
    let case = case_file();
    CaseSnapshot {
        case_id: case.case_id,
        name: case.name,
        case_number: case.case_number,
        examiner: case.examiner,
        agency: case.agency,
    }
}

fn pending_seal() -> Seal {
    Seal {
        status: SealStatus::Pending,
        manifest: None,
        manifest_sha256: None,
        file_count: None,
        total_bytes: None,
    }
}

/// The final record of CONTRACTS.md §7.1.
pub fn run_record() -> RunRecord {
    let run_dir = run_dir();
    let tool_bin = format!("{APP_DATA}/leapp/ileapp/v2026.4.2/bin/ileapp");
    RunRecord {
        schema_version: RunRecord::SCHEMA_VERSION,
        run_id: s(RUN_ID),
        label: Some(s("iPhone 12 Finder backup")),
        status: RunStatus::CompletedWithErrors,
        status_reasons: vec![modules_errored()],
        warnings: vec![reason(
            "stderr_traceback",
            "stderr contains a Python traceback (see leapp.stderr.log)",
        )],
        created_at: at("2026-09-24T18:30:05Z"),
        started_at: Some(at("2026-09-24T18:30:06Z")),
        ended_at: Some(at("2026-09-24T18:52:41Z")),
        recovered_at: None,
        duration_ms: Some(1_356_000),
        app: record_app(),
        host: record_host(),
        case_snapshot: case_snapshot(),
        tool: RunTool {
            id: ToolId::Ileapp,
            version: s("v2026.4.2"),
            platform: PlatformKey::MacosAarch64,
            asset_name: Some(s(ILEAPP_ASSET)),
            asset_sha256: Some(s(ILEAPP_ASSET_SHA256)),
            entry_sha256: example_sha256("ileapp entry"),
            entry_verified_against: EntryVerifiedAgainst::Manifest,
            install_source: InstallSource::Download,
        },
        input: RunInput {
            path: s(INPUT_PATH),
            kind: InputKind::Directory,
            input_type: InputType::Itunes,
            type_detected: Some(InputType::Itunes),
            size_bytes: None,
            itunes_encrypted: Some(true),
            acquisition_id: None,
            hash: InputHash {
                algorithm: HashAlgorithm::Sha256,
                status: HashStatus::NotApplicable,
                value: None,
                started_at: None,
                completed_at: None,
            },
        },
        options: RunOptions {
            timezone: Some(s("America/Chicago")),
            timezone_supported: true,
            password_supplied: true,
            keychain_path: None,
            keychain_sha256: None,
        },
        modules: RunModules {
            mode: ModuleMode::Custom,
            profile_name: None,
            requested: strings(&["callHistory", "sms"]),
            resolved: strings(&["callHistory", "sms"]),
            unknown: vec![],
            always_run: strings(&["itunes_backup_info", "itunes_backup_installed_applications"]),
            available_count: 1176,
        },
        command: RunCommand {
            argv: vec![
                tool_bin,
                s("-t"),
                s("itunes"),
                s("-i"),
                s(INPUT_PATH),
                s("-o"),
                run_dir.clone(),
                s("--custom_output_folder"),
                s("report"),
                s("-d"),
                format!("{run_dir}/case.lcasedata"),
                s("-m"),
                format!("{run_dir}/profile.ilprofile"),
                s("-tz"),
                s("America/Chicago"),
                s("--itunes_password"),
                s("<redacted>"),
            ],
            cwd: run_dir,
        },
        process: Some(RunProcess {
            exit_code: Some(0),
            signal: None,
            exited_at: at("2026-09-24T18:51:10Z"),
            cancel_requested: false,
            escalated_to_kill: false,
        }),
        leapp_result: Some(LeappResult {
            lava_data_found: true,
            processing_status: Some(s("Complete")),
            leapp_version_reported: Some(s("2026.4.2")),
            index_html_found: true,
            module_counts: Some(ModuleCounts {
                complete: 120,
                error: 2,
                no_files_found: 1054,
                other: 0,
            }),
            error_modules: strings(&["foo", "bar"]),
        }),
        output: RunOutput {
            report_dir: s("report"),
            seal: Seal {
                status: SealStatus::Sealed,
                manifest: Some(s("report.sha256")),
                manifest_sha256: Some(example_sha256("report.sha256")),
                file_count: Some(5321),
                total_bytes: Some(123_456_789),
            },
        },
        logs: RunLogs {
            stdout: s("leapp.stdout.log"),
            stderr: s("leapp.stderr.log"),
            screen_output: s("report/_HTML/_Script_Logs/Screen_Output.html"),
        },
    }
}

/// The initial record of the same run (CONTRACTS.md §7.2).
pub fn run_record_initial() -> RunRecord {
    RunRecord {
        status: RunStatus::Running,
        status_reasons: vec![],
        warnings: vec![],
        started_at: None,
        ended_at: None,
        recovered_at: None,
        duration_ms: None,
        process: None,
        leapp_result: None,
        output: RunOutput {
            report_dir: s("report"),
            seal: pending_seal(),
        },
        ..run_record()
    }
}

pub fn leapp_profile() -> LeappProfile {
    LeappProfile {
        leapp: s("ileapp"),
        format_version: 1,
        plugins: strings(&["callHistory", "sms"]),
    }
}

pub fn leapp_case_data() -> LeappCaseData {
    LeappCaseData {
        leapp: s("case_data"),
        case_data_values: CaseDataValues {
            case_number: s("2026-0142"),
            agency: s("County Forensics Lab"),
            examiner: s("J. Doe"),
        },
    }
}

fn tool_bundle(platform: &str, exe: &str, extra: &[&str]) -> ToolBundle {
    let tools = ["idevice_id", "ideviceinfo", "idevicepair", "idevicebackup2"];
    let names = tools
        .iter()
        .map(|tool| format!("{tool}{exe}"))
        .chain(extra.iter().map(|file| s(file)));
    ToolBundle {
        bundle: format!("idevice-tools-1.4.0-{platform}.zip"),
        bundle_sha256: example_sha256(&format!("idevice-tools-1.4.0-{platform}.zip")),
        files: names
            .map(|name| {
                let hash = example_sha256(&format!("{platform}/{name}"));
                (name, hash)
            })
            .collect(),
    }
}

pub fn idevice_tools_manifest() -> IdeviceToolsManifest {
    let source = |name: &str, version: &str, url: &str| SourceTarball {
        name: s(name),
        version: s(version),
        url: s(url),
        sha256: example_sha256(url),
    };
    IdeviceToolsManifest {
        schema_version: IdeviceToolsManifest::SCHEMA_VERSION,
        version: s("1.4.0"),
        sources: vec![
            source(
                "libplist",
                "2.7.0",
                "https://github.com/libimobiledevice/libplist/releases/download/2.7.0/libplist-2.7.0.tar.bz2",
            ),
            source(
                "mbedtls",
                "3.6.7",
                "https://github.com/Mbed-TLS/mbedtls/releases/download/mbedtls-3.6.7/mbedtls-3.6.7.tar.bz2",
            ),
            source(
                "libimobiledevice",
                "1.4.0",
                "https://github.com/libimobiledevice/libimobiledevice/releases/download/1.4.0/libimobiledevice-1.4.0.tar.bz2",
            ),
        ],
        platforms: [
            (
                PlatformKey::MacosAarch64,
                tool_bundle("macos-aarch64", "", &[]),
            ),
            (
                PlatformKey::MacosX86_64,
                tool_bundle("macos-x86_64", "", &[]),
            ),
            (
                PlatformKey::WindowsX86_64,
                tool_bundle("windows-x86_64", ".exe", &["libimobiledevice-1.0.dll"]),
            ),
        ]
        .into(),
        system_platforms: vec![PlatformKey::LinuxX86_64, PlatformKey::LinuxAarch64],
    }
}

fn acq_tools() -> AcqTools {
    let binary = |name: &str| ToolBinary {
        path: format!("{IDEVICE_DIR}/{name}"),
        sha256: example_sha256(&format!("signed {name}")),
        verified_against: ToolVerification::CodeSignature,
    };
    AcqTools {
        version: Some(s("1.4.0")),
        source: IdeviceToolSource::Bundled,
        binaries: IdeviceBinaries {
            idevice_id: binary("idevice_id"),
            ideviceinfo: binary("ideviceinfo"),
            idevicepair: binary("idevicepair"),
            idevicebackup2: binary("idevicebackup2"),
        },
    }
}

fn backup2_argv(args: &[&str]) -> Vec<String> {
    let mut argv = vec![format!("{IDEVICE_DIR}/idevicebackup2"), s("-u"), s(UDID)];
    argv.extend(strings(args));
    argv
}

/// The record of CONTRACTS.md §13.3.
pub fn acquisition_record() -> AcquisitionRecord {
    let change = |time: &str, change, detail: &str| DeviceChange {
        at: at(time),
        change,
        detail: s(detail),
    };
    let command = |purpose, args: &[&str], started: &str, exited: &str| AcqCommand {
        purpose,
        argv: backup2_argv(args),
        exit_code: Some(0),
        started_at: at(started),
        exited_at: Some(at(exited)),
    };
    let backup_dir = format!("{}/backup", acq_dir());
    AcquisitionRecord {
        schema_version: AcquisitionRecord::SCHEMA_VERSION,
        acq_id: s(ACQ_ID),
        label: Some(s("Suspect iPhone 12")),
        status: AcqStatus::Succeeded,
        status_reasons: vec![],
        warnings: vec![],
        created_at: at("2026-09-24T17:12:00Z"),
        started_at: Some(at("2026-09-24T17:12:04Z")),
        ended_at: Some(at("2026-09-24T17:48:51Z")),
        recovered_at: None,
        duration_ms: Some(2_211_000),
        app: record_app(),
        host: record_host(),
        case_snapshot: case_snapshot(),
        device: AcqDevice {
            udid: s(UDID),
            serial_number: Some(s("F2LXXXXXXX")),
            device_name: Some(s("Alex's iPhone")),
            product_type: Some(s("iPhone13,2")),
            product_version: Some(s("18.6")),
            build_version: Some(s("22G86")),
            captured_at: Some(at("2026-09-24T17:12:01Z")),
            info_file: Some(s("device-info.plist")),
            info_file_sha256: Some(example_sha256("device-info.plist")),
        },
        pairing: AcqPairing {
            paired_before: false,
            paired_by_app_at: Some(at("2026-09-24T17:10:40Z")),
            host_id: Some(s("5E1B7C2A-9D4F-4E8B-A3C6-1F0D2B7E9A48")),
            system_buid: Some(s("0C6F3A9E-2B7D-4C1E-8F5A-6D9B3E0A7C21")),
        },
        device_changes: vec![
            change(
                "2026-09-24T17:10:40Z",
                DeviceChangeKind::PairRecordCreated,
                "Trusted this computer via idevicepair pair",
            ),
            change(
                "2026-09-24T17:12:05Z",
                DeviceChangeKind::BackupEncryptionEnabled,
                "WillEncrypt false → true",
            ),
            change(
                "2026-09-24T17:12:09Z",
                DeviceChangeKind::SyncLockTaken,
                "idevicebackup2 holds /com.apple.itunes.lock_sync during backup",
            ),
            change(
                "2026-09-24T17:48:40Z",
                DeviceChangeKind::BackupEncryptionDisabled,
                "WillEncrypt true → false",
            ),
        ],
        tools: acq_tools(),
        encryption: AcqEncryption {
            will_encrypt_before: Some(false),
            enable_requested: true,
            enabled_by_examiner: true,
            will_encrypt_after_enable: Some(true),
            restore_requested: true,
            restored_after: RestoreState::Restored,
            will_encrypt_after_restore: Some(false),
            password_supplied: true,
            password_channel: Some(PasswordChannel::Env),
        },
        commands: vec![
            command(
                AcqCommandPurpose::EnableEncryption,
                &["encryption", "on"],
                "2026-09-24T17:12:04Z",
                "2026-09-24T17:12:05Z",
            ),
            command(
                AcqCommandPurpose::Backup,
                &["backup", "--full", &backup_dir],
                "2026-09-24T17:12:09Z",
                "2026-09-24T17:48:30Z",
            ),
            command(
                AcqCommandPurpose::RestoreEncryption,
                &["encryption", "off"],
                "2026-09-24T17:48:31Z",
                "2026-09-24T17:48:40Z",
            ),
        ],
        process: Some(AcqProcess {
            exit_code: Some(0),
            signal: None,
            cancel_requested: false,
            escalated_to_kill: false,
        }),
        backup_result: Some(BackupResult {
            final_message: Some(s("Backup Successful.")),
            udid_dir: format!("backup/{UDID}"),
            manifest_found: Some(s("Manifest.db")),
            info_plist_found: true,
            status_plist_found: true,
            snapshot_state: Some(s("finished")),
            last_progress_percent: Some(100),
            device_file_errors: 0,
            free_bytes_after: Some(81_234_567_890),
        }),
        output: AcqOutput {
            backup_dir: s("backup"),
            seal: Seal {
                status: SealStatus::Sealed,
                manifest: Some(s("backup.sha256")),
                manifest_sha256: Some(example_sha256("backup.sha256")),
                file_count: Some(48_210),
                total_bytes: Some(61_203_455_110),
            },
        },
        logs: AcqLogs {
            stdout: s("idevicebackup2.stdout.log"),
            stderr: s("idevicebackup2.stderr.log"),
        },
    }
}

pub fn encryption_restore_record() -> EncryptionRestoreRecord {
    EncryptionRestoreRecord {
        schema_version: EncryptionRestoreRecord::SCHEMA_VERSION,
        acq_id: s(ACQ_ID),
        at: at("2026-09-25T09:15:00Z"),
        argv: backup2_argv(&["encryption", "off"]),
        exit_code: Some(0),
        will_encrypt_after: Some(false),
        restored: true,
        tools: acq_tools(),
    }
}

// ---- §9 shared IPC types ----

pub fn app_error() -> AppError {
    AppError {
        code: ErrorCode::ToolNotInstalled,
        message: s("iLEAPP v2026.4.2 is not installed."),
        detail: Some(format!(
            "{APP_DATA}/leapp/ileapp/v2026.4.2/install.json does not exist"
        )),
    }
}

fn modules_errored() -> Reason {
    reason("modules_errored", "2 modules reported Error: foo, bar")
}

pub fn tool_status() -> ToolStatus {
    ToolStatus {
        tool: ToolId::Ileapp,
        display_name: s("iLEAPP"),
        pinned_version: s("v2026.4.2"),
        state: ToolState::Verified,
        installed_version: Some(s("v2026.4.2")),
        install_source: Some(InstallSource::Download),
        module_count: Some(1176),
        install_dir: Some(format!("{APP_DATA}/leapp/ileapp/v2026.4.2")),
        problem: None,
    }
}

pub fn tool_modules() -> ToolModules {
    let file = modules_file();
    ToolModules {
        tool: file.tool,
        version: file.version,
        always_run: file.always_run,
        timezones: file.timezones,
        modules: file.modules,
    }
}

pub fn case_summary() -> CaseSummary {
    CaseSummary {
        path: s(CASE_PATH),
        exists: true,
        case: Some(case_file()),
        run_count: 1,
        last_run_at: Some(at("2026-09-24T18:30:05Z")),
    }
}

pub fn case_detail() -> CaseDetail {
    CaseDetail {
        path: s(CASE_PATH),
        case: case_file(),
        runs: vec![run_summary()],
        acquisitions: vec![acq_summary()],
        recovered: vec![],
    }
}

pub fn run_summary() -> RunSummary {
    let record = run_record();
    RunSummary {
        run_id: record.run_id,
        run_dir: run_dir(),
        label: record.label,
        status: record.status,
        tool: record.tool.id,
        tool_version: record.tool.version,
        input_path: record.input.path,
        input_type: record.input.input_type,
        created_at: record.created_at,
        started_at: record.started_at,
        ended_at: record.ended_at,
        duration_ms: record.duration_ms,
        report_available: true,
    }
}

pub fn input_inspection() -> InputInspection {
    InputInspection {
        path: s(INPUT_PATH),
        kind: InputKind::Directory,
        size_bytes: None,
        detected_type: Some(InputType::Itunes),
        allowed_types: vec![InputType::Fs, InputType::Itunes],
        is_itunes_backup: true,
        itunes_encrypted: Some(true),
        hashable: false,
        warnings: vec![],
    }
}

pub fn module_selections() -> Vec<ModuleSelection> {
    vec![
        ModuleSelection::All,
        ModuleSelection::Profile {
            profile_name: s("Messaging"),
        },
        ModuleSelection::Custom {
            modules: strings(&["callHistory", "sms"]),
        },
    ]
}

pub fn run_request() -> RunRequest {
    RunRequest {
        case_path: s(CASE_PATH),
        tool: ToolId::Ileapp,
        input_path: s(INPUT_PATH),
        input_type: InputType::Itunes,
        modules: ModuleSelection::Custom {
            modules: strings(&["callHistory", "sms"]),
        },
        timezone: Some(s("America/Chicago")),
        itunes_password: Some(s(EXAMPLE_PASSWORD)),
        keychain_path: None,
        hash_input: false,
        label: Some(s("iPhone 12 Finder backup")),
    }
}

pub fn active_jobs() -> Vec<ActiveJob> {
    vec![
        ActiveJob::Run {
            case_path: s(CASE_PATH),
            run_id: s(RUN_ID),
            tool: ToolId::Ileapp,
            created_at: at("2026-09-24T18:30:05Z"),
            phase: RunPhase::Running,
        },
        ActiveJob::Acquisition {
            case_path: s(CASE_PATH),
            acq_id: s(ACQ_ID),
            udid: s(UDID),
            created_at: at("2026-09-24T17:12:00Z"),
            phase: AcqPhase::BackingUp,
        },
    ]
}

pub fn profile_info() -> ProfileInfo {
    ProfileInfo {
        tool: ToolId::Ileapp,
        name: s("Messaging"),
        modules: strings(&["callHistory", "sms", "noSuchModule"]),
        unknown_modules: strings(&["noSuchModule"]),
    }
}

pub fn ios_backup() -> IosBackup {
    IosBackup {
        path: format!("/Users/examiner/Library/Application Support/MobileSync/Backup/{UDID}"),
        device_name: Some(s("Alex's iPhone")),
        product_type: Some(s("iPhone13,2")),
        ios_version: Some(s("18.6")),
        last_backup: Some(at("2026-09-20T21:04:33Z")),
        encrypted: Some(true),
        size_bytes: Some(61_203_455_110),
    }
}

// ---- §10 requests and responses ----

pub fn app_info() -> AppInfo {
    AppInfo {
        app_version: s("0.2.0"),
        platform: Some(PlatformKey::MacosAarch64),
        os: s("macos"),
        arch: s("aarch64"),
        dev_override: false,
        paths: AppInfoPaths {
            app_data: s(APP_DATA),
            app_config: s(APP_DATA),
            app_cache: s("/Users/examiner/Library/Caches/com.suitedfir.desktop"),
            app_log: s("/Users/examiner/Library/Logs/com.suitedfir.desktop"),
            tools_dir: format!("{APP_DATA}/leapp"),
        },
    }
}

/// Changes `cases_root`, leaves `defaults` unchanged (omitted) and resets `tools_dir` (`null`).
pub fn settings_update() -> SettingsUpdateRequest {
    SettingsUpdateRequest {
        cases_root: Some(s("/Volumes/Cases/suiteDFIR Cases")),
        defaults: None,
        tools_dir: ToolsDirUpdate::Reset,
    }
}

pub fn tool_import() -> ToolImportRequest {
    ToolImportRequest {
        tool: ToolId::Ileapp,
        archive_path: format!("/Users/examiner/Downloads/{ILEAPP_ASSET}"),
    }
}

pub fn case_fields() -> CaseFields {
    let case = case_file();
    CaseFields {
        name: case.name,
        case_number: case.case_number,
        examiner: case.examiner,
        agency: case.agency,
        description: s("Seized handset, item 3"),
        default_timezone: case.default_timezone,
    }
}

pub fn case_create() -> CaseCreateRequest {
    let fields = case_fields();
    CaseCreateRequest {
        name: fields.name,
        case_number: fields.case_number,
        examiner: fields.examiner,
        agency: fields.agency,
        description: fields.description,
        default_timezone: fields.default_timezone,
        parent_dir: None,
    }
}

pub fn case_update() -> CaseUpdateRequest {
    CaseUpdateRequest {
        path: s(CASE_PATH),
        fields: case_fields(),
    }
}

pub fn run_ref() -> RunRef {
    RunRef {
        case_path: s(CASE_PATH),
        run_id: s(RUN_ID),
    }
}

pub fn input_inspect() -> InputInspectRequest {
    InputInspectRequest {
        tool: ToolId::Ileapp,
        path: s(INPUT_PATH),
        case_path: s(CASE_PATH),
    }
}

pub fn profile_save() -> ProfileSaveRequest {
    ProfileSaveRequest {
        tool: ToolId::Ileapp,
        name: s("Messaging"),
        modules: strings(&["callHistory", "sms"]),
    }
}

pub fn profile_ref() -> ProfileRef {
    ProfileRef {
        tool: ToolId::Ileapp,
        name: s("Messaging"),
    }
}

pub fn profile_import() -> ProfileImportRequest {
    ProfileImportRequest {
        tool: ToolId::Ileapp,
        path: s("/Users/examiner/Downloads/Messaging.ilprofile"),
        name: None,
        overwrite: false,
    }
}

pub fn profile_export() -> ProfileExportRequest {
    ProfileExportRequest {
        tool: ToolId::Ileapp,
        name: s("Messaging"),
        dest_path: s("/Users/examiner/Desktop/Messaging.ilprofile"),
    }
}

pub fn run_started() -> RunStarted {
    RunStarted {
        run_id: s(RUN_ID),
        run_dir: run_dir(),
    }
}

pub fn run_cancel() -> RunCancelRequest {
    RunCancelRequest { run_id: s(RUN_ID) }
}

pub fn job_attach() -> JobAttachRequest {
    JobAttachRequest {
        kind: JobKind::Run,
        id: s(RUN_ID),
    }
}

pub fn job_backlog() -> JobBacklog {
    JobBacklog {
        backlog: strings(&[
            "Processing iTunes backup",
            "callHistory [Call History] artifact executed",
        ]),
    }
}

pub fn open_text_file() -> OpenTextFileRequest {
    OpenTextFileRequest {
        case_path: s(CASE_PATH),
        run_id: s(RUN_ID),
        which: RunFile::Stderr,
    }
}

pub fn temp_cleanup() -> TempCleanupResult {
    TempCleanupResult {
        freed_byte_count: 132_120_576,
    }
}

// ---- §11 events ----

pub fn run_events() -> Vec<RunEvent> {
    let record = run_record();
    vec![
        RunEvent::Phase {
            phase: RunPhase::Running,
        },
        RunEvent::Log {
            lines: job_backlog().backlog,
        },
        RunEvent::StdioTail {
            stream: StdStream::Stderr,
            lines: strings(&["Traceback (most recent call last):", "  File \"foo.py\""]),
        },
        RunEvent::HashProgress {
            bytes_done: 1_048_576,
            bytes_total: 67_108_864,
        },
        RunEvent::SealProgress {
            files_done: 2000,
            files_total: Some(5321),
        },
        RunEvent::Finished {
            status: record.status,
            reasons: record.status_reasons,
            warnings: record.warnings,
            summary: Box::new(run_summary()),
        },
    ]
}

pub fn install_events() -> Vec<InstallEvent> {
    vec![
        InstallEvent::Stage {
            stage: InstallStage::Downloading,
        },
        InstallEvent::DownloadProgress {
            bytes_done: 10_485_760,
            bytes_total: 55_085_487,
        },
        InstallEvent::Message {
            text: s("Verifying the asset hash"),
        },
    ]
}

// ---- §13.5 acquisition ----

pub fn device_summary() -> DeviceSummary {
    DeviceSummary {
        udid: s(UDID),
        device_name: Some(s("Alex's iPhone")),
        product_type: Some(s("iPhone13,2")),
        product_version: Some(s("18.6")),
        serial_number: Some(s("F2LXXXXXXX")),
        pair_state: PairState::Paired,
        busy: false,
        will_encrypt: Some(false),
        data_used_bytes: Some(61_203_455_110),
        data_capacity_bytes: Some(118_111_600_640),
        message: None,
    }
}

pub fn devices_result() -> DevicesResult {
    DevicesResult {
        tools: IdeviceToolsStatus {
            source: Some(IdeviceToolSource::Bundled),
            version: Some(s("1.4.0")),
            state: IdeviceToolsState::Ok,
            guidance: None,
        },
        devices: vec![device_summary()],
    }
}

pub fn device_pair() -> DevicePairRequest {
    DevicePairRequest { udid: s(UDID) }
}

pub fn acq_preflight_request() -> AcqPreflightRequest {
    AcqPreflightRequest {
        case_path: s(CASE_PATH),
        udid: s(UDID),
    }
}

pub fn acq_preflight() -> AcqPreflight {
    AcqPreflight {
        free_bytes: 412_316_860_416,
        required_bytes: Some(61_203_455_110),
        level: PreflightLevel::Ok,
    }
}

pub fn acq_request() -> AcqRequest {
    AcqRequest {
        case_path: s(CASE_PATH),
        udid: s(UDID),
        label: Some(s("Suspect iPhone 12")),
        enable_encryption: true,
        encryption_password: Some(s(EXAMPLE_PASSWORD)),
        restore_encryption: true,
    }
}

pub fn acq_started() -> AcqStarted {
    AcqStarted {
        acq_id: s(ACQ_ID),
        acq_dir: acq_dir(),
    }
}

pub fn acq_cancel() -> AcqCancelRequest {
    AcqCancelRequest { acq_id: s(ACQ_ID) }
}

pub fn acq_ref() -> AcqRef {
    AcqRef {
        case_path: s(CASE_PATH),
        acq_id: s(ACQ_ID),
    }
}

pub fn acq_restore_encryption_request() -> AcqRestoreEncryptionRequest {
    AcqRestoreEncryptionRequest {
        case_path: s(CASE_PATH),
        acq_id: s(ACQ_ID),
        password: s(EXAMPLE_PASSWORD),
    }
}

pub fn acq_restore_encryption_result() -> AcqRestoreEncryptionResult {
    AcqRestoreEncryptionResult {
        restored: true,
        will_encrypt_after: Some(false),
    }
}

pub fn open_acq_file() -> OpenAcqFileRequest {
    OpenAcqFileRequest {
        case_path: s(CASE_PATH),
        acq_id: s(ACQ_ID),
        which: AcqFile::DeviceInfo,
    }
}

pub fn acq_summary() -> AcqSummary {
    let record = acquisition_record();
    AcqSummary {
        acq_id: record.acq_id,
        acq_dir: acq_dir(),
        label: record.label,
        status: record.status,
        udid: record.device.udid,
        device_name: record.device.device_name,
        product_version: record.device.product_version,
        created_at: record.created_at,
        started_at: record.started_at,
        ended_at: record.ended_at,
        duration_ms: record.duration_ms,
        backup_path: Some(format!("{}/backup/{UDID}", acq_dir())),
        warnings: vec![],
    }
}

pub fn acq_events() -> Vec<AcqEvent> {
    vec![
        AcqEvent::Phase {
            phase: AcqPhase::BackingUp,
        },
        AcqEvent::Log {
            lines: strings(&["Started \"com.apple.mobilebackup2\" service on port 49324."]),
        },
        AcqEvent::Progress { percent: 45 },
        AcqEvent::DevicePrompt {
            kind: DevicePromptKind::PasscodeForEncryption,
            text: s(
                "Please confirm enabling the backup encryption by entering the passcode on the device.",
            ),
        },
        AcqEvent::SealProgress {
            files_done: 12_000,
            files_total: Some(48_210),
        },
        AcqEvent::Finished {
            status: AcqStatus::Succeeded,
            reasons: vec![],
            warnings: vec![],
            summary: Box::new(acq_summary()),
        },
    ]
}
