//! Test support: an `AppState` in a temp dir, with fake-leapp as the LEAPP dev override, fake-idevice
//! as the iOS tools' dev override, and an opener that records instead of opening.
//!
//! fake-leapp and fake-idevice are `suitedfir-core` binaries: `cargo test --workspace` builds them
//! next to this test binary's `deps/` folder (`cargo build -p suitedfir-core --bins` does too).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use suitedfir_core::contracts::{ArchiveKind, LeappManifest, PlatformAsset, ToolId};
use suitedfir_core::idevice::{IdeviceConfig, ToolLookup, embedded_manifest};
use suitedfir_core::paths::AppPaths;
use suitedfir_core::{hashing, manifest, settings};

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
    /// The one default backup folder `ios_backups_find` searches (not created).
    pub backups: PathBuf,
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
    let backups = root.path().join("bk");
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
        ios_backup_dirs: vec![backups.clone()],
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
        backups,
    }
}

/// A lab with both LEAPP tools overridden and fake-idevice's `success` device.
pub fn lab_state() -> Lab {
    lab(LabOptions::default())
}

/// A minimal Finder/iTunes backup in `dir` for `ios_backups_find`: XML `Info.plist` and
/// `Manifest.plist`, and an empty `Manifest.db`. `device_name` must need no XML escaping.
pub fn write_backup(dir: &Path, device_name: &str, encrypted: bool) {
    let plist = |body: String| {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD \
             PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist \
             version=\"1.0\">\n<dict>\n{body}</dict>\n</plist>\n"
        )
    };
    fs::create_dir_all(dir).unwrap();
    let info = format!(
        "<key>Device Name</key><string>{device_name}</string>\n<key>Product Type</key>\
         <string>iPhone13,2</string>\n<key>Product Version</key><string>18.6</string>\n\
         <key>Last Backup Date</key><date>2026-09-20T21:04:33Z</date>\n"
    );
    fs::write(dir.join("Info.plist"), plist(info)).unwrap();
    let manifest = format!("<key>IsEncrypted</key><{encrypted}/>\n");
    fs::write(dir.join("Manifest.plist"), plist(manifest)).unwrap();
    fs::write(dir.join("Manifest.db"), b"").unwrap();
}

// ---- an aLEAPP stand-in for the real install pipeline ----

/// A zip of a probe-answering fake-leapp copy and the embedded manifest with aLEAPP pinned to it
/// for this host, so `tool_install` (with `download_from: zip`) installs it through the real
/// pipeline: download → verify → extract → verify → introspect.
pub struct AleappStandIn {
    pub zip: PathBuf,
    pub manifest: LeappManifest,
}

pub fn aleapp_stand_in(dir: &Path) -> AleappStandIn {
    let platform = manifest::host_platform().expect("this host has a platform key");
    let fake_path = core_binary("fake-leapp");
    let fake = fs::read(&fake_path).unwrap();
    let entry = format!("fake-leapp-probe{EXE}");
    let archive = stored_zip(&entry, &fake);
    let zip = dir.join("aleapp-replay.zip");
    fs::write(&zip, &archive).unwrap();
    let mut pinned = manifest::embedded().unwrap().clone();
    pinned
        .tools
        .get_mut(&ToolId::Aleapp)
        .unwrap()
        .platforms
        .insert(
            platform,
            PlatformAsset {
                asset_name: "aleapp-replay.zip".to_owned(),
                asset_size: archive.len() as u64,
                asset_sha256: hashing::sha256_file(&zip).unwrap(),
                archive_kind: ArchiveKind::Zip,
                entry,
                entry_sha256: Some(hashing::sha256_file(&fake_path).unwrap()),
                urls: vec!["https://example.invalid/aleapp-replay.zip".to_owned()],
            },
        );
    AleappStandIn {
        zip,
        manifest: pinned,
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// A zip with one stored (uncompressed) entry.
fn stored_zip(name: &str, data: &[u8]) -> Vec<u8> {
    let crc = crc32(data);
    let size = u32::try_from(data.len()).unwrap();
    let name_len = u16::try_from(name.len()).unwrap();
    let mut zip = Vec::new();
    let header = |zip: &mut Vec<u8>| {
        zip.extend(20u16.to_le_bytes()); // version needed
        zip.extend(0u16.to_le_bytes()); // flags
        zip.extend(0u16.to_le_bytes()); // stored
        zip.extend(0u16.to_le_bytes()); // time
        zip.extend(0x21u16.to_le_bytes()); // date: 1980-01-01
        zip.extend(crc.to_le_bytes());
        zip.extend(size.to_le_bytes());
        zip.extend(size.to_le_bytes());
        zip.extend(name_len.to_le_bytes());
        zip.extend(0u16.to_le_bytes()); // extra
    };
    zip.extend(0x0403_4b50u32.to_le_bytes());
    header(&mut zip);
    zip.extend(name.as_bytes());
    zip.extend(data);
    let directory = u32::try_from(zip.len()).unwrap();
    zip.extend(0x0201_4b50u32.to_le_bytes());
    zip.extend(20u16.to_le_bytes()); // version made by
    header(&mut zip);
    zip.extend(0u16.to_le_bytes()); // comment
    zip.extend(0u16.to_le_bytes()); // disk
    zip.extend(0u16.to_le_bytes()); // internal attributes
    zip.extend(0u32.to_le_bytes()); // external attributes
    zip.extend(0u32.to_le_bytes()); // local header offset
    zip.extend(name.as_bytes());
    let directory_size = u32::try_from(zip.len()).unwrap() - directory;
    zip.extend(0x0605_4b50u32.to_le_bytes());
    zip.extend(0u16.to_le_bytes());
    zip.extend(0u16.to_le_bytes());
    zip.extend(1u16.to_le_bytes());
    zip.extend(1u16.to_le_bytes());
    zip.extend(directory_size.to_le_bytes());
    zip.extend(directory.to_le_bytes());
    zip.extend(0u16.to_le_bytes());
    zip
}
