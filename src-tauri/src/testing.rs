//! Test support: an `AppState` in a temp dir, with fake-leapp as the LEAPP dev override, fake-idevice
//! as the iOS tools' dev override, and an opener that records instead of opening.
//!
//! fake-leapp and fake-idevice are `suitedfir-core` binaries: `cargo test --workspace` builds them
//! next to this test binary's `deps/` folder (`cargo build -p suitedfir-core --bins` does too).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use suitedfir_core::contracts::{LeappManifest, ToolId};
use suitedfir_core::idevice::{IdeviceConfig, ToolLookup, embedded_manifest};
use suitedfir_core::manifest;
use suitedfir_core::paths::AppPaths;
use suitedfir_core::settings;

use crate::commands::Shared;
use crate::opener::testing::RecordingOpener;
use crate::state::{AppConfig, AppState};

pub const EXE: &str = if cfg!(windows) { ".exe" } else { "" };
/// fake-idevice's device.
pub const UDID: &str = "00008101-000A1B2C3D4E001E";

/// A `suitedfir-core` binary (`fake-leapp`, `fake-idevice`) of this build.
pub fn core_binary(name: &str) -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    // target/<profile>/deps/<test binary> → target/<profile>/<name>
    let dir = exe.parent().and_then(Path::parent).unwrap();
    let path = dir.join(format!("{name}{EXE}"));
    assert!(
        path.is_file(),
        "{} is missing: run `cargo test --workspace` (or `cargo build -p suitedfir-core --bins`)",
        path.display()
    );
    path
}

/// How to set up a lab.
pub struct LabOptions {
    /// Tools replaced by fake-leapp (the LEAPP dev override).
    pub leapp_override: Vec<ToolId>,
    pub manifest: LeappManifest,
    /// `tool_install` takes the pinned asset from this file instead of the network.
    pub download_from: Option<PathBuf>,
    /// fake-idevice's scenario.
    pub idevice_scenario: String,
}

impl Default for LabOptions {
    fn default() -> Self {
        Self {
            leapp_override: ToolId::ALL.to_vec(),
            manifest: manifest::embedded().unwrap().clone(),
            download_from: None,
            idevice_scenario: "success".to_owned(),
        }
    }
}

/// An app state in a temp dir (short name: the Windows test machine has long paths disabled).
pub struct Lab {
    pub root: tempfile::TempDir,
    pub state: Shared,
    pub opener: Arc<RecordingOpener>,
    /// The cases root in the settings.
    pub cases: PathBuf,
    /// fake-idevice's state dir.
    pub device_state: PathBuf,
}

impl Lab {
    /// The iOS tools config for a fake-idevice scenario (same device state).
    pub fn idevice_config(&self, scenario: &str) -> IdeviceConfig {
        idevice_config(
            &self.state.paths,
            &self.device_state,
            scenario,
            self.state.platform,
        )
    }
}

fn idevice_config(
    paths: &AppPaths,
    device_state: &Path,
    scenario: &str,
    platform: Option<suitedfir_core::contracts::PlatformKey>,
) -> IdeviceConfig {
    IdeviceConfig {
        lookup: ToolLookup {
            platform,
            manifest: embedded_manifest().unwrap(),
            bundled_dir: None,
            dev_override: Some(core_binary("fake-idevice")),
            path_var: None,
            signed_build: false,
        },
        app_cache: paths.app_cache.clone(),
        env: vec![
            ("FAKE_IDEVICE_SCENARIO".into(), scenario.into()),
            (
                "FAKE_IDEVICE_STATE_DIR".into(),
                device_state.as_os_str().to_owned(),
            ),
            ("FAKE_IDEVICE_INTERVAL_MS".into(), "5".into()),
            ("FAKE_IDEVICE_PROMPT_MS".into(), "20".into()),
        ],
    }
}

pub fn lab(options: LabOptions) -> Lab {
    let root = tempfile::Builder::new().prefix("sdt").tempdir().unwrap();
    let paths = AppPaths {
        app_data: root.path().join("data"),
        app_config: root.path().join("config"),
        app_cache: root.path().join("cache"),
        app_log: root.path().join("log"),
    };
    let cases = root.path().join("cases");
    fs::create_dir_all(&cases).unwrap();
    let device_state = root.path().join("device");
    fs::create_dir_all(&device_state).unwrap();
    let platform = manifest::host_platform();
    let opener = Arc::new(RecordingOpener::default());
    let fake_leapp = core_binary("fake-leapp");
    let leapp_env: Vec<(OsString, OsString)> = vec![
        ("FAKE_LEAPP_SCENARIO".into(), "success".into()),
        ("FAKE_LEAPP_INTERVAL_MS".into(), "5".into()),
        ("FAKE_LEAPP_LINES".into(), "20".into()),
    ];
    let config = AppConfig {
        idevice: idevice_config(&paths, &device_state, &options.idevice_scenario, platform),
        paths,
        host: crate::host::detect(),
        manifest: options.manifest,
        platform,
        leapp_override: options
            .leapp_override
            .iter()
            .map(|tool| (*tool, fake_leapp.clone()))
            .collect(),
        leapp_env,
        opener: Arc::clone(&opener) as Arc<dyn crate::opener::Opener>,
        download_from: options.download_from,
    };
    let state = Arc::new(AppState::new(config, settings::defaults(&cases)));
    Lab {
        root,
        state,
        opener,
        cases,
        device_state,
    }
}

/// A lab with both LEAPP tools overridden and fake-idevice's `success` device.
pub fn lab_state() -> Lab {
    lab(LabOptions::default())
}
