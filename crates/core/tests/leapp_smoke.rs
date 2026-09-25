//! Real-LEAPP smoke tests (ROADMAP A3; E3 adds runs). Every test is `#[ignore]`: each downloads
//! the pinned build of a tool for this host from github.com/abrignoni, installs it through the core
//! (`leapp::install`) and introspects it (`leapp::modules`). Run them with
//! `cargo test -p suitedfir-core --test leapp_smoke --locked -- --ignored --test-threads=1`
//! (locally and in `.github/workflows/leapp-smoke.yml`).
//!
//! Each test prints a summary (module count, always-run names, timezone count, entry SHA-256) to
//! stderr directly, so it shows even though the test harness captures `println!`/`eprintln!`
//! output of passing tests.

use std::fs;
use std::io::Write;
use std::path::Path;

use sha2::{Digest, Sha256};
use suitedfir_core::contracts::{
    EntryVerifiedAgainst, InstallEvent, InstallRecord, ModulesFile, ToolId, ToolState,
    parse_versioned,
};
use suitedfir_core::leapp::install::{self, MODULES_FILE, Pinned, Source};
use suitedfir_core::leapp::modules::{self, MIN_MODULES};
use suitedfir_core::manifest;

/// Writes straight to stderr, bypassing the harness's output capture.
fn report(text: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(text.as_bytes());
    let _ = stderr.write_all(b"\n");
    let _ = stderr.flush();
}

/// Installs `tool` for this host, introspects it and checks the result. `known` is an artifact
/// name that has been stable across releases.
fn install_and_introspect(tool: ToolId, known: &str) -> (InstallRecord, ModulesFile) {
    let manifest = manifest::embedded().expect("the embedded manifest is valid");
    let tool_manifest = &manifest.tools[&tool];
    let platform = manifest::host_platform().expect("this host has pinned LEAPP builds");
    // Short names: the Windows machine has long paths disabled.
    let dir = tempfile::Builder::new().prefix("sdn").tempdir().unwrap();
    let tools_dir = dir.path().join("tools");
    let app_cache = dir.path().join("cache");
    let pinned = Pinned {
        tools_dir: &tools_dir,
        tool,
        manifest: tool_manifest,
        platform: Some(platform),
    };

    let mut stages = Vec::new();
    let record = install::install(
        pinned,
        Source::Download,
        &mut |event| match event {
            InstallEvent::Stage { stage } => stages.push(stage),
            InstallEvent::Message { text } => report(&format!("  install message: {text}")),
            InstallEvent::DownloadProgress { .. } => {}
        },
        &mut |entry| {
            modules::introspect(entry, tool, tool_manifest, &app_cache).map_err(Into::into)
        },
    )
    .unwrap_or_else(|e| panic!("installing {tool} failed: {e}\n{:?}", e));
    assert_eq!(stages.last().map(|s| s.as_str()), Some("done"));

    let version_dir = pinned.version_dir();
    let modules: ModulesFile =
        parse_versioned(&fs::read(version_dir.join(MODULES_FILE)).unwrap()).unwrap();
    assert_eq!(modules.tool, tool);
    assert_eq!(modules.version, tool_manifest.version);
    assert!(
        modules.modules.len() >= MIN_MODULES,
        "{}",
        modules.modules.len()
    );
    assert_eq!(record.module_count as usize, modules.modules.len());
    assert!(
        modules.modules.iter().any(|m| m.name == known),
        "{known} is missing"
    );
    let always: Vec<&String> = modules.always_run.values().flatten().collect();
    assert!(
        modules.modules.iter().all(|m| !always.contains(&&m.name)),
        "an always-run artifact is selectable"
    );

    let check = install::verify(pinned);
    assert_eq!(
        check.status.state,
        ToolState::Verified,
        "{:?}",
        check.status
    );
    let verified = check.verified.unwrap();
    let against = match tool_manifest.platforms[&platform].entry_sha256 {
        Some(_) => EntryVerifiedAgainst::Manifest,
        None => EntryVerifiedAgainst::InstallRecord,
    };
    assert_eq!(verified.verified_against, against);

    // Introspection removed its temp dir.
    let temp_root = app_cache.join("tmp");
    let left: Vec<_> = fs::read_dir(&temp_root).unwrap().collect();
    assert!(left.is_empty(), "{} is not empty", temp_root.display());

    // A digest of the module list, so platforms can be compared beyond the count.
    let mut listing = String::new();
    for m in &modules.modules {
        listing.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            m.name, m.module_name, m.category, m.display_name
        ));
    }
    let digest: String = Sha256::digest(listing.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let no_category = modules.modules.iter().filter(|m| m.category.is_empty());
    let no_display_name = modules.modules.iter().filter(|m| m.display_name == m.name);
    report(&format!(
        "{tool} {} on {platform}: {} selectable modules (list sha256 {digest}; {} without a \
         category, {} without a display name); always_run {:?}; timezones {}; entry {} sha256 {} \
         (verified against {})",
        modules.version,
        modules.modules.len(),
        no_category.count(),
        no_display_name.count(),
        modules.always_run,
        modules.timezones.as_ref().map_or_else(
            || "null".to_owned(),
            |zones| format!("{} zones", zones.len())
        ),
        record.entry_path,
        record.entry_sha256,
        verified.verified_against,
    ));
    assert!(Path::new(&version_dir).is_dir());
    (record, modules)
}

#[test]
#[ignore = "downloads and runs the real iLEAPP build"]
fn ileapp_installs_and_introspects() {
    let (_, modules) = install_and_introspect(ToolId::Ileapp, "callHistory");
    let zones = modules.timezones.expect("iLEAPP reports its timezones");
    assert!(zones.iter().any(|zone| zone == "America/Chicago"));
    assert!(zones.iter().any(|zone| zone == "UTC"));
    assert_eq!(modules.always_run["default"], ["last_build"]);
    assert_eq!(
        modules.always_run["itunes"],
        ["itunes_backup_info", "itunes_backup_installed_applications"]
    );
}

#[test]
#[ignore = "downloads and runs the real aLEAPP build"]
fn aleapp_installs_and_introspects() {
    let (_, modules) = install_and_introspect(ToolId::Aleapp, "accounts_ce");
    assert_eq!(modules.timezones, None);
    assert_eq!(modules.always_run["default"], ["usagestatsVersion"]);
}
