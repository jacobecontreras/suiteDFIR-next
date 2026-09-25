//! fake-idevice as bundled tools, for the `idevice` and `acquire` integration tests
//! (DEVELOPMENT.md §4.8): copies of the binary under the four tool names (copies, never symlinks,
//! so it works on Windows without privileges), a manifest pinning their hashes, and a lab with an
//! app cache and a fake-device state dir per test.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use suitedfir_core::contracts::{IdeviceToolsManifest, PlatformKey, ToolBundle};
use suitedfir_core::hashing;
use suitedfir_core::idevice::{Idevice, IdeviceConfig, ToolLookup, ToolName, embedded_manifest};

pub const FAKE: &str = env!("CARGO_BIN_EXE_fake-idevice");
/// The fake device.
pub const UDID: &str = "00008101-000A1B2C3D4E001E";
pub const EXE: &str = if cfg!(windows) { ".exe" } else { "" };

/// This machine's platform key.
pub fn host_platform() -> PlatformKey {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => PlatformKey::MacosAarch64,
        ("macos", "x86_64") => PlatformKey::MacosX86_64,
        ("windows", "x86_64") => PlatformKey::WindowsX86_64,
        ("windows", "aarch64") => PlatformKey::WindowsAarch64,
        ("linux", "aarch64") => PlatformKey::LinuxAarch64,
        _ => PlatformKey::LinuxX86_64,
    }
}

/// Copies fake-idevice into `dir` under the four tool names.
pub fn copy_fake_tools(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    for name in ToolName::ALL {
        fs::copy(FAKE, dir.join(format!("{}{EXE}", name.as_str()))).unwrap();
    }
}

/// A manifest that pins the tools in `dir` as this platform's bundle (every platform gets one,
/// Linux included, so the bundled-tools path is exercised everywhere).
pub fn manifest_for(dir: &Path) -> IdeviceToolsManifest {
    let files = ToolName::ALL
        .into_iter()
        .map(|name| {
            let file = format!("{}{EXE}", name.as_str());
            let hash = hashing::sha256_file(&dir.join(&file)).unwrap();
            (file, hash)
        })
        .collect::<BTreeMap<_, _>>();
    let mut manifest = embedded_manifest().unwrap();
    manifest.platforms.insert(
        host_platform(),
        ToolBundle {
            bundle: "fake.zip".to_owned(),
            bundle_sha256: "0".repeat(64),
            files,
        },
    );
    manifest
}

/// The fake tools shared by the tests of this binary, under Cargo's per-target temp dir (replaced
/// on every run). If an earlier run's copy cannot be replaced (Windows keeps running executables
/// locked), a dir named after this process is used instead.
pub fn shared_tools() -> &'static (PathBuf, IdeviceToolsManifest) {
    static TOOLS: OnceLock<(PathBuf, IdeviceToolsManifest)> = OnceLock::new();
    TOOLS.get_or_init(|| {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"));
        let mut dir = root.join(format!("fake-idevice-{}", test_binary_stem()));
        let _ = fs::remove_dir_all(&dir);
        if dir.exists() {
            dir = root.join(format!(
                "fake-idevice-{}-{}",
                test_binary_stem(),
                std::process::id()
            ));
        }
        copy_fake_tools(&dir);
        let manifest = manifest_for(&dir);
        (dir, manifest)
    })
}

/// Fake tools in a dir of their own (for a test that changes them or runs in a child process).
pub fn private_tools(dir: &Path) -> IdeviceToolsManifest {
    copy_fake_tools(dir);
    manifest_for(dir)
}

fn test_binary_stem() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "tests".to_owned())
}

/// An [`Idevice`] on bundled tools in `tools_dir`.
fn idevice(
    tools_dir: &Path,
    manifest: IdeviceToolsManifest,
    cache: &Path,
    env: Vec<(OsString, OsString)>,
) -> Idevice {
    Idevice::new(IdeviceConfig {
        lookup: ToolLookup {
            platform: Some(host_platform()),
            manifest,
            bundled_dir: Some(tools_dir.to_path_buf()),
            dev_override: None,
            path_var: None,
            signed_build: false,
        },
        app_cache: cache.to_path_buf(),
        env,
    })
}

/// A temp dir with an app cache and the fake device's state dir, and an [`Idevice`] using the
/// shared fake tools with a scenario.
pub struct Lab {
    pub root: tempfile::TempDir,
    pub cache: PathBuf,
    pub state: PathBuf,
    pub idevice: Idevice,
    pub env: Vec<(OsString, OsString)>,
    tools_dir: PathBuf,
    manifest: IdeviceToolsManifest,
}

impl Lab {
    pub fn new(scenario: &str) -> Self {
        Self::with_env(scenario, &[])
    }

    pub fn with_env(scenario: &str, extra: &[(&str, &str)]) -> Self {
        let (dir, manifest) = shared_tools();
        Self::with_tools(scenario, extra, dir, manifest.clone())
    }

    pub fn with_tools(
        scenario: &str,
        extra: &[(&str, &str)],
        tools_dir: &Path,
        manifest: IdeviceToolsManifest,
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("cache");
        let state = root.path().join("state");
        fs::create_dir_all(&state).unwrap();
        let mut env: Vec<(OsString, OsString)> = vec![
            ("FAKE_IDEVICE_SCENARIO".into(), scenario.into()),
            (
                "FAKE_IDEVICE_STATE_DIR".into(),
                state.clone().into_os_string(),
            ),
            ("FAKE_IDEVICE_INTERVAL_MS".into(), "5".into()),
        ];
        env.extend(
            extra
                .iter()
                .map(|(name, value)| (OsString::from(name), OsString::from(value))),
        );
        let idevice = idevice(tools_dir, manifest.clone(), &cache, env.clone());
        Self {
            root,
            cache,
            state,
            idevice,
            env,
            tools_dir: tools_dir.to_path_buf(),
            manifest,
        }
    }

    /// A second handle on the same fake device with another scenario (like the app after a
    /// restart, or the device's owner changing something).
    pub fn reopen(&self, scenario: &str) -> Idevice {
        let mut env = self.env.clone();
        env[0].1 = scenario.into();
        idevice(&self.tools_dir, self.manifest.clone(), &self.cache, env)
    }

    /// The fake device's state (`state.json`).
    pub fn state(&self) -> serde_json::Value {
        let bytes = fs::read(self.state.join("state.json")).unwrap_or_else(|_| b"{}".to_vec());
        serde_json::from_slice(&bytes).unwrap()
    }

    /// Every tool invocation so far: the tool name, then its arguments.
    pub fn calls(&self) -> Vec<Vec<String>> {
        serde_json::from_value(self.state()["calls"].clone()).unwrap_or_default()
    }

    /// Every invocation that tried to pair.
    pub fn pairing_attempts(&self) -> Vec<String> {
        serde_json::from_value(self.state()["pairing_attempts"].clone()).unwrap_or_default()
    }

    /// No scratch or job dir is left in `<app_cache>/tmp`.
    pub fn assert_no_temp_dirs(&self) {
        let tmp = self.cache.join("tmp");
        let left: Vec<_> = fs::read_dir(&tmp)
            .map(|entries| entries.map(|e| e.unwrap().file_name()).collect())
            .unwrap_or_default();
        assert!(left.is_empty(), "left in {}: {left:?}", tmp.display());
    }
}
