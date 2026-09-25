//! Real-LEAPP smoke tests (ROADMAP A3, E3). Every test is `#[ignore]`: each downloads the pinned
//! build of a tool for this host from github.com/abrignoni, installs it through the core
//! (`leapp::install`), introspects it (`leapp::modules`) and then runs it through the runner
//! (`runner`), as the app does. Run them with
//! `cargo test -p suitedfir-core --test leapp_smoke --locked -- --ignored --test-threads=1`
//! (locally and in `.github/workflows/leapp-smoke.yml`).
//!
//! **Runs** (E3), per tool:
//! - a minimal synthetic `fs` fixture that yields at least one `Complete` module;
//! - iLEAPP `-t itunes` on a folder that is not a backup → `failed` with `no_modules_ran`
//!   (aLEAPP has no `itunes` type: the runner refuses it before anything is created);
//! - iLEAPP `-t itunes` on a minimal legacy (`Manifest.mbdb`) backup: the iTunes always-run
//!   artifacts run and `last_build` does not (LEAPP-CLI.md §5, §9 item 3);
//! - a cancel mid-run → `cancelled`, no process of the tree left, the per-run temp dir (which held
//!   the onefile runtime `_MEI*`, so the bootloader honoured `TMPDIR`/`TEMP`) removed;
//! - a profile with an unknown module name → `unknown_modules` before anything is created;
//! - iLEAPP only: an encrypted backup without a password, run directly (the runner refuses it),
//!   must not block on the password prompt (LEAPP-CLI.md Q5, §9 item 4).
//!
//! **Fixture files** (written by the tests, all tiny):
//! - iLEAPP `fs`: `private/var/installd/Library/MobileInstallation/LastBuildInfo.plist`
//!   (`ProductVersion` 17.5; the always-run `last_build`) and
//!   `root/Library/Lockdown/data_ark.plist` (`-DeviceName`; the `deviceName` artifact).
//! - aLEAPP `fs`: `system/etc/hosts` (one non-default entry; `get_etc_hosts`) and
//!   `system/usagestats/0/version` (the always-run `usagestatsVersion`).
//! - Not a backup: one text file. Legacy backup: `Manifest.mbdb` (the 6-byte header, no records),
//!   `Manifest.plist` (`IsEncrypted` false) and `Info.plist`. Encrypted backup: `Manifest.plist`
//!   (`IsEncrypted` true), a `Manifest.db` that is not SQLite, and `Info.plist`.
//!
//! With `SUITEDFIR_SMOKE_CAPTURE=<dir>`, the `_lava_data.lava` and `Screen_Output.html` of the
//! fixture runs are saved to `<dir>/<tool>/<version>/<case>/` with paths replaced by `<RUN_DIR>`,
//! `<INPUT>` and `<LAB>`, plus `outcome.json` (the run's verdict); `fixtures/leapp/` holds such
//! captures and `run::status` checks its rules against them.
//!
//! Each test prints a summary to stderr directly, so it shows even though the test harness
//! captures `println!`/`eprintln!` output of passing tests.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};
use suitedfir_core::case::{self, CreatedCase};
use suitedfir_core::contracts::{
    EntryVerifiedAgainst, ErrorCode, InputType, InstallEvent, InstallRecord, ModuleSelection,
    ModulesFile, RecordHost, RunEvent, RunRecord, RunRequest, RunStatus, SealStatus, Settings,
    ToolId, ToolManifest, ToolState, examples, parse_versioned,
};
use suitedfir_core::leapp::install::{self, MODULES_FILE, Pinned, Source};
use suitedfir_core::leapp::modules::{self, MIN_MODULES};
use suitedfir_core::manifest;
use suitedfir_core::paths::AppPaths;
use suitedfir_core::process::{self, ProcessWatch, SpawnSpec};
use suitedfir_core::run::profile::{ProfileFormat, ProfileStore};
use suitedfir_core::runner::{self, RunContext, RunControl, RunOutcome};
use suitedfir_core::settings;
use suitedfir_core::tail;

/// Writes straight to stderr, bypassing the harness's output capture.
fn report(text: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(text.as_bytes());
    let _ = stderr.write_all(b"\n");
    let _ = stderr.flush();
}

// ---- install and introspect ----

/// One installed tool in a temp dir: app dirs, a known case and the installed build.
struct Smoke {
    dir: tempfile::TempDir,
    tool: ToolId,
    manifest: &'static ToolManifest,
    paths: AppPaths,
    settings: Settings,
    case: CreatedCase,
    modules: ModulesFile,
}

/// Installs `tool` for this host, introspects it and checks the result. `known` is an artifact
/// name that has been stable across releases.
fn install_and_introspect(tool: ToolId, known: &str) -> Smoke {
    let manifest = manifest::embedded().expect("the embedded manifest is valid");
    let tool_manifest = &manifest.tools[&tool];
    let platform = manifest::host_platform().expect("this host has pinned LEAPP builds");
    // Short names: the Windows machine has long paths disabled.
    let dir = tempfile::Builder::new().prefix("sdn").tempdir().unwrap();
    let paths = AppPaths {
        app_data: dir.path().join("data"),
        app_config: dir.path().join("config"),
        app_cache: dir.path().join("cache"),
        app_log: dir.path().join("log"),
    };
    let tools_dir = paths.default_tools_dir();
    let pinned = Pinned {
        tools_dir: &tools_dir,
        tool,
        manifest: tool_manifest,
        platform: Some(platform),
    };

    let mut stages = Vec::new();
    let never = AtomicBool::new(false);
    let record: InstallRecord = install::install(
        pinned,
        Source::Download,
        &never,
        &mut |event| match event {
            InstallEvent::Stage { stage } => stages.push(stage),
            InstallEvent::Message { text } => report(&format!("  install message: {text}")),
            InstallEvent::DownloadProgress { .. } => {}
        },
        &mut |entry| {
            modules::introspect(entry, tool, tool_manifest, &paths.app_cache).map_err(Into::into)
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
    assert_temp_empty(&paths);

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

    let cases = dir.path().join("cases");
    fs::create_dir_all(&cases).unwrap();
    let case = case::create(&cases, &examples::case_fields()).unwrap();
    let mut settings = settings::defaults(&cases);
    settings::touch_recent(&mut settings, &case.path.to_string_lossy());
    Smoke {
        dir,
        tool,
        manifest: tool_manifest,
        paths,
        settings,
        case,
        modules,
    }
}

fn assert_temp_empty(paths: &AppPaths) {
    let root = paths.temp_root();
    let left: Vec<_> = fs::read_dir(&root)
        .map(|entries| entries.map(|e| e.unwrap().file_name()).collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "{} is not empty: {left:?}", root.display());
}

// ---- runs ----

impl Smoke {
    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn context(&self) -> RunContext {
        let tools_dir = self.paths.default_tools_dir();
        let pinned = Pinned {
            tools_dir: &tools_dir,
            tool: self.tool,
            manifest: self.manifest,
            platform: manifest::host_platform(),
        };
        RunContext {
            paths: self.paths.clone(),
            settings: self.settings.clone(),
            case_dir: self.case.path.clone(),
            case: self.case.case.clone(),
            host: RecordHost {
                os: std::env::consts::OS.to_owned(),
                os_version: "smoke".to_owned(),
                arch: std::env::consts::ARCH.to_owned(),
                hostname: "smoke".to_owned(),
            },
            tool: runner::installed_tool(pinned).unwrap(),
            env: Vec::new(),
            max_run_dir_chars: runner::default_run_dir_limit(),
        }
    }

    fn request(&self, input: &Path, input_type: InputType, modules: ModuleSelection) -> RunRequest {
        RunRequest {
            case_path: self.case.path.to_string_lossy().into_owned(),
            tool: self.tool,
            input_path: input.to_string_lossy().into_owned(),
            input_type,
            modules,
            timezone: None,
            itunes_password: None,
            keychain_path: None,
            hash_input: false,
            label: Some("smoke".to_owned()),
        }
    }

    /// Runs a job to the end; `hook` sees every event and may cancel.
    fn run(
        &self,
        request: RunRequest,
        mut hook: impl FnMut(&RunEvent, &RunControl),
    ) -> (RunOutcome, Vec<RunEvent>) {
        let job = runner::start(request, self.context()).unwrap();
        let control = job.control();
        let mut events = Vec::new();
        let outcome = job.run(&mut |event| {
            hook(&event, &control);
            events.push(event);
        });
        assert_eq!(outcome.write_error, None);
        (outcome, events)
    }

    fn run_dirs(&self) -> usize {
        fs::read_dir(self.case.path.join("runs"))
            .map(|entries| entries.count())
            .unwrap_or(0)
    }

    fn custom(&self, names: &[&str]) -> ModuleSelection {
        for name in names {
            assert!(
                self.modules.modules.iter().any(|m| m.name == *name),
                "{} has no module {name}",
                self.tool
            );
        }
        ModuleSelection::Custom {
            modules: names.iter().map(|n| (*n).to_owned()).collect(),
        }
    }
}

fn codes(record: &RunRecord) -> Vec<&str> {
    record
        .status_reasons
        .iter()
        .map(|r| r.code.as_str())
        .collect()
}

/// The `(artifact_name or module_name, module_status)` entries of the run's `_lava_data.lava`.
fn lava_modules(run_dir: &Path) -> Vec<(String, String)> {
    let lava: serde_json::Value =
        serde_json::from_slice(&fs::read(run_dir.join("report").join("_lava_data.lava")).unwrap())
            .unwrap();
    lava["modules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            let name = m["artifact_name"]
                .as_str()
                .or(m["module_name"].as_str())
                .unwrap_or("?");
            (
                name.to_owned(),
                m["module_status"].as_str().unwrap_or("?").to_owned(),
            )
        })
        .collect()
}

fn screen_output_path(run_dir: &Path) -> PathBuf {
    run_dir
        .join("report")
        .join("_HTML")
        .join("_Script_Logs")
        .join("Screen_Output.html")
}

/// How `Screen_Output.html` ends its records (LEAPP-CLI.md Q8): `\r\n`, `\n` or mixed.
fn newline_style(run_dir: &Path) -> &'static str {
    let bytes = fs::read(screen_output_path(run_dir)).unwrap();
    let crlf = bytes.windows(6).filter(|w| w == b"<br>\r\n").count();
    let lf = bytes.windows(5).filter(|w| w == b"<br>\n").count();
    match (crlf, lf) {
        (0, 0) => "none",
        (_, 0) => "\\r\\n",
        (0, _) => "\\n",
        _ => "mixed",
    }
}

/// Every `Screen_Output.html` record becomes one or more log lines: the tail parses the real file.
fn assert_tail_reads(run_dir: &Path, events: &[RunEvent]) {
    let mut tail = tail::ScreenOutputTail::new(screen_output_path(run_dir));
    let lines = tail.finish().unwrap();
    let streamed: Vec<&String> = events
        .iter()
        .flat_map(|e| match e {
            RunEvent::Log { lines } => lines.iter().collect(),
            _ => Vec::new(),
        })
        .collect();
    assert_eq!(lines.len(), streamed.len(), "streamed vs parsed lines");
    assert!(lines.iter().all(|l| !l.contains("<br>")));
}

fn log_text(events: &[RunEvent]) -> String {
    events
        .iter()
        .flat_map(|e| match e {
            RunEvent::Log { lines } => lines.clone(),
            _ => Vec::new(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Saves the lava data and `Screen_Output.html` of a fixture run, with paths replaced
/// (`SUITEDFIR_SMOKE_CAPTURE`).
fn capture(smoke: &Smoke, name: &str, input: &Path, outcome: &RunOutcome) {
    let Some(root) = std::env::var_os("SUITEDFIR_SMOKE_CAPTURE") else {
        return;
    };
    let record = &outcome.record;
    let run_dir = PathBuf::from(&outcome.summary.run_dir);
    let dir = PathBuf::from(root)
        .join(smoke.tool.as_str())
        .join(&smoke.manifest.version)
        .join(name);
    fs::create_dir_all(&dir).unwrap();
    let replacements = [
        (run_dir.to_string_lossy().into_owned(), "<RUN_DIR>"),
        (input.to_string_lossy().into_owned(), "<INPUT>"),
        (smoke.root().to_string_lossy().into_owned(), "<LAB>"),
    ];
    let sanitize = |text: String| {
        let mut out = text;
        for (path, placeholder) in &replacements {
            // As written, and as a JSON string escapes it (Windows backslashes).
            let escaped = serde_json::to_string(path).unwrap();
            out = out.replace(&escaped[1..escaped.len() - 1], placeholder);
            out = out.replace(path.as_str(), placeholder);
        }
        out
    };
    let report_dir = run_dir.join("report");
    let lava = fs::read_to_string(report_dir.join("_lava_data.lava")).unwrap();
    fs::write(dir.join("_lava_data.lava"), sanitize(lava)).unwrap();
    let screen =
        String::from_utf8_lossy(&fs::read(screen_output_path(&run_dir)).unwrap()).into_owned();
    fs::write(dir.join("Screen_Output.html"), sanitize(screen)).unwrap();
    let outcome_json = serde_json::json!({
        "tool": smoke.tool,
        "version": smoke.manifest.version,
        "input_type": record.input.input_type,
        "exit_code": record.process.as_ref().and_then(|p| p.exit_code),
        "index_html": report_dir.join("index.html").is_file(),
        "always_run": record.modules.always_run,
        "status": record.status,
        "reasons": codes(record),
    });
    fs::write(
        dir.join("outcome.json"),
        format!("{}\n", serde_json::to_string_pretty(&outcome_json).unwrap()),
    )
    .unwrap();
    report(&format!("  captured {name} into {}", dir.display()));
}

// ---- fixtures ----

/// An XML plist dict of string values.
fn write_plist(path: &Path, entries: &[(&str, &str)]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut body = String::new();
    for (key, value) in entries {
        body.push_str(&format!("<key>{key}</key><string>{value}</string>"));
    }
    fs::write(
        path,
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \
             \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict>{body}</dict></plist>\n"
        ),
    )
    .unwrap();
}

fn write_bool_plist(path: &Path, key: &str, value: bool) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
             <key>{key}</key><{value}/></dict></plist>\n"
        ),
    )
    .unwrap();
}

fn ios_fs_fixture(root: &Path) -> PathBuf {
    let input = root.join("ev").join("ios-fs");
    write_plist(
        &input
            .join("private/var/installd/Library/MobileInstallation")
            .join("LastBuildInfo.plist"),
        &[
            ("ProductName", "iPhone OS"),
            ("ProductVersion", "17.5"),
            ("ProductBuildVersion", "21F79"),
        ],
    );
    write_plist(
        &input.join("root/Library/Lockdown").join("data_ark.plist"),
        &[("-DeviceName", "suiteDFIR smoke iPhone")],
    );
    input
}

fn android_fs_fixture(root: &Path) -> PathBuf {
    let input = root.join("ev").join("android-fs");
    let etc = input.join("system").join("etc");
    fs::create_dir_all(&etc).unwrap();
    fs::write(
        etc.join("hosts"),
        "127.0.0.1 localhost\n::1 ip6-localhost\n10.20.30.40 evidence.example\n",
    )
    .unwrap();
    let usagestats = input.join("system").join("usagestats").join("0");
    fs::create_dir_all(&usagestats).unwrap();
    fs::write(
        usagestats.join("version"),
        "14;UpsideDownCake;UP1A.231005.007;US\n",
    )
    .unwrap();
    input
}

fn not_a_backup(root: &Path) -> PathBuf {
    let input = root.join("ev").join("not-a-backup");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("readme.txt"), "not an iTunes backup\n").unwrap();
    input
}

fn itunes_info_plist(dir: &Path, version: &str) {
    write_plist(
        &dir.join("Info.plist"),
        &[
            ("Device Name", "suiteDFIR smoke iPhone"),
            ("Display Name", "suiteDFIR smoke iPhone"),
            ("Product Type", "iPhone12,1"),
            ("Product Version", version),
            ("Serial Number", "SMOKE0000001"),
        ],
    );
}

/// A legacy (iOS 4-9 style) backup: `Manifest.mbdb` with its header and no records, unencrypted.
fn legacy_backup(root: &Path) -> PathBuf {
    let input = root.join("ev").join("legacy-backup");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("Manifest.mbdb"), b"mbdb\x05\x00").unwrap();
    write_bool_plist(&input.join("Manifest.plist"), "IsEncrypted", false);
    itunes_info_plist(&input, "12.4");
    input
}

/// An encrypted backup (as iLEAPP sees one: `IsEncrypted` true and a `Manifest.db` that is not
/// SQLite), to see what LEAPP does without a password.
fn encrypted_backup(root: &Path) -> PathBuf {
    let input = root.join("ev").join("encrypted-backup");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("Manifest.db"), b"encrypted, not SQLite\n").unwrap();
    write_bool_plist(&input.join("Manifest.plist"), "IsEncrypted", true);
    itunes_info_plist(&input, "17.5");
    input
}

// ---- checks shared by both tools ----

/// The fixture run: `succeeded`, at least one selected module `Complete`, the always-run entry
/// in the lava data, a sealed report, and the live log equal to `Screen_Output.html`.
fn fs_fixture_run(smoke: &Smoke, input: &Path, selected: &[&str]) -> (RunRecord, Vec<RunEvent>) {
    let (outcome, events) = smoke.run(
        smoke.request(input, InputType::Fs, smoke.custom(selected)),
        |_, _| {},
    );
    let record = outcome.record.clone();
    let run_dir = PathBuf::from(&outcome.summary.run_dir);
    let lava = lava_modules(&run_dir);
    report(&format!(
        "  {} fs fixture: {} {:?}; lava modules {lava:?}; Screen_Output records end with {}",
        smoke.tool,
        record.status,
        codes(&record),
        newline_style(&run_dir)
    ));
    assert_eq!(record.status, RunStatus::Succeeded, "{:?}", codes(&record));
    for name in selected {
        assert!(
            lava.iter().any(|(n, s)| n == name && s == "Complete"),
            "{name} is not Complete: {lava:?}"
        );
    }
    // The always-run artifact ran (LEAPP-CLI.md §5, §9 item 3).
    for name in &record.modules.always_run {
        assert!(
            lava.iter().any(|(n, _)| n == name),
            "always-run {name} is not in the lava data: {lava:?}"
        );
    }
    assert_eq!(record.output.seal.status, SealStatus::Sealed);
    assert!(record.leapp_result.as_ref().unwrap().index_html_found);
    assert_tail_reads(&run_dir, &events);
    assert_temp_empty(&smoke.paths);
    capture(smoke, "fs-fixture", input, &outcome);
    (record, events)
}

/// A profile naming a module this build does not have is refused before anything is created.
fn unknown_profile_is_refused(smoke: &Smoke, input: &Path, known: &str) {
    let format = ProfileFormat::from_manifest(smoke.tool, smoke.manifest);
    let file = smoke.root().join(format!("unknown.{}", format.ext));
    fs::write(
        &file,
        serde_json::json!({"leapp": smoke.tool, "format_version": 1,
                           "plugins": [known, "suitedfirNoSuchModule"]})
        .to_string(),
    )
    .unwrap();
    let store = ProfileStore::new(smoke.paths.profiles_dir(smoke.tool), format);
    let info = store
        .import(&file, Some("Unknown"), false, &smoke.modules.modules)
        .unwrap();
    assert_eq!(info.unknown_modules, ["suitedfirNoSuchModule"]);
    let before = smoke.run_dirs();
    let request = smoke.request(
        input,
        InputType::Fs,
        ModuleSelection::Profile {
            profile_name: "Unknown".to_owned(),
        },
    );
    let error = runner::start(request, smoke.context()).unwrap_err();
    assert_eq!(error.code, ErrorCode::UnknownModules, "{error:?}");
    assert_eq!(smoke.run_dirs(), before, "a refused run created a folder");
    report(&format!(
        "  {} unknown profile name: refused ({})",
        smoke.tool, error.code
    ));
}

/// Processes whose process group is `pgid` (Unix), zombies excluded.
#[cfg(target_os = "linux")]
fn group_members(pgid: u32) -> Vec<u32> {
    let mut members = Vec::new();
    for entry in fs::read_dir("/proc").unwrap().flatten() {
        let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        // pid (comm) state ppid pgrp …: parse after the last ')'.
        let Some(rest) = stat.rsplit_once(')').map(|(_, rest)| rest) else {
            continue;
        };
        let fields: Vec<&str> = rest.split_whitespace().collect();
        if fields.len() > 2 && fields[0] != "Z" && fields[2] == pgid.to_string() {
            members.push(entry.file_name().to_string_lossy().parse().unwrap_or(0));
        }
    }
    members
}

#[cfg(target_os = "macos")]
fn group_members(pgid: u32) -> Vec<u32> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-A", "-o", "pid=,pgid=,stat="])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            (fields.len() >= 3 && fields[1] == pgid.to_string() && !fields[2].starts_with('Z'))
                .then(|| fields[0].parse().unwrap_or(0))
        })
        .collect()
}

/// What the cancel hook saw while LEAPP ran: its pid, the per-run temp dir's entries, and a watch
/// on the process.
struct MidRun {
    pid: u32,
    runtime: Vec<String>,
    watch: ProcessWatch,
}

/// A run cancelled once LEAPP is processing: `cancelled`, the per-run temp dir held the onefile
/// runtime (`_MEI*`) while it ran and is gone now, and nothing of the process tree is left.
fn cancel_mid_run(smoke: &Smoke, input: &Path) {
    let seen: Arc<Mutex<Option<MidRun>>> = Arc::default();
    let observed = Arc::clone(&seen);
    let temp_root = smoke.paths.temp_root();
    let (outcome, events) = smoke.run(
        smoke.request(input, InputType::Fs, ModuleSelection::All),
        move |event, control| {
            let RunEvent::Log { lines } = event else {
                return;
            };
            let mut observed = observed.lock().unwrap();
            if observed.is_some() || !lines.iter().any(|l| l.contains("artifact completed")) {
                return;
            }
            let pid = control.process_id().unwrap();
            // The runtime the onefile bootloader extracted into the run's temp dir.
            let runtime: Vec<String> = fs::read_dir(&temp_root)
                .unwrap()
                .flatten()
                .flat_map(|run| fs::read_dir(run.path()).into_iter().flatten().flatten())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            let watch = ProcessWatch::open(pid);
            *observed = Some(MidRun {
                pid,
                runtime,
                watch,
            });
            control.cancel();
        },
    );
    let record = &outcome.record;
    let MidRun {
        pid,
        runtime,
        watch,
    } = seen.lock().unwrap().take().expect("LEAPP logged artifacts");
    report(&format!(
        "  {} cancel mid-run: {} {:?}; temp dir held {runtime:?}; escalated_to_kill {}",
        smoke.tool,
        record.status,
        codes(record),
        record.process.as_ref().unwrap().escalated_to_kill
    ));
    assert_eq!(record.status, RunStatus::Cancelled);
    assert_eq!(codes(record), ["cancelled_by_user"]);
    assert!(record.process.as_ref().unwrap().cancel_requested);
    assert!(
        runtime.iter().any(|name| name.starts_with("_MEI")),
        "the onefile runtime was not in the run's temp dir: {runtime:?}"
    );
    assert_temp_empty(&smoke.paths);
    assert!(!watch.is_alive(), "LEAPP (pid {pid}) is still running");
    #[cfg(unix)]
    assert_eq!(group_members(pid), Vec::<u32>::new(), "left in group {pid}");
    assert!(log_text(&events).contains("artifact completed"));
}

// ---- the tests ----

#[test]
#[ignore = "downloads and runs the real iLEAPP build"]
fn ileapp_installs_introspects_and_runs() {
    let smoke = install_and_introspect(ToolId::Ileapp, "callHistory");
    let modules = &smoke.modules;
    let zones = modules
        .timezones
        .as_ref()
        .expect("iLEAPP reports its timezones");
    assert!(zones.iter().any(|zone| zone == "America/Chicago"));
    assert!(zones.iter().any(|zone| zone == "UTC"));
    assert_eq!(modules.always_run["default"], ["last_build"]);
    assert_eq!(
        modules.always_run["itunes"],
        ["itunes_backup_info", "itunes_backup_installed_applications"]
    );

    // A synthetic fs fixture with one Complete module (and the always-run last_build).
    let fs_input = ios_fs_fixture(smoke.root());
    let (record, events) = fs_fixture_run(&smoke, &fs_input, &["deviceName"]);
    assert_eq!(record.modules.always_run, ["last_build"]);
    assert!(log_text(&events).contains("iOS version: 17.5"));

    // -t itunes on a folder that is not a backup.
    let input = not_a_backup(smoke.root());
    let (outcome, _) = smoke.run(
        smoke.request(&input, InputType::Itunes, smoke.custom(&["deviceName"])),
        |_, _| {},
    );
    let record = &outcome.record;
    report(&format!(
        "  ileapp -t itunes on a non-backup folder: {} {:?}",
        record.status,
        codes(record)
    ));
    assert_eq!(record.status, RunStatus::Failed);
    assert!(
        codes(record).contains(&"no_modules_ran"),
        "{:?}",
        codes(record)
    );
    capture(&smoke, "itunes-not-a-backup", &input, &outcome);

    // -t itunes on a legacy backup: the iTunes always-run artifacts run, last_build does not.
    let input = legacy_backup(smoke.root());
    let (outcome, events) = smoke.run(
        smoke.request(&input, InputType::Itunes, smoke.custom(&["deviceName"])),
        |_, _| {},
    );
    let record = &outcome.record;
    let run_dir = PathBuf::from(&outcome.summary.run_dir);
    let lava = lava_modules(&run_dir);
    report(&format!(
        "  ileapp -t itunes on a legacy backup: {} {:?}; lava modules {lava:?}",
        record.status,
        codes(record)
    ));
    assert_eq!(record.status, RunStatus::Succeeded, "{:?}", codes(record));
    assert_eq!(
        record.modules.always_run,
        ["itunes_backup_info", "itunes_backup_installed_applications"]
    );
    // itunes_backup_info read Info.plist ("iOS version: 12.4"); last_build was not run.
    assert!(log_text(&events).contains("iOS version: 12.4"));
    assert!(
        !lava.iter().any(|(name, _)| name == "last_build"),
        "{lava:?}"
    );
    capture(&smoke, "itunes-legacy-backup", &input, &outcome);

    unknown_profile_is_refused(&smoke, &fs_input, "deviceName");
    cancel_mid_run(&smoke, &fs_input);
    password_prompt_does_not_block(&smoke);
}

#[test]
#[ignore = "downloads and runs the real aLEAPP build"]
fn aleapp_installs_introspects_and_runs() {
    let smoke = install_and_introspect(ToolId::Aleapp, "accounts_ce");
    assert_eq!(smoke.modules.timezones, None);
    assert_eq!(smoke.modules.always_run["default"], ["usagestatsVersion"]);

    let fs_input = android_fs_fixture(smoke.root());
    let (record, _) = fs_fixture_run(&smoke, &fs_input, &["get_etc_hosts"]);
    assert_eq!(record.modules.always_run, ["usagestatsVersion"]);
    assert_eq!(record.options.timezone, None);

    // aLEAPP has no itunes type: refused before anything is created.
    let before = smoke.run_dirs();
    let request = smoke.request(
        &not_a_backup(smoke.root()),
        InputType::Itunes,
        ModuleSelection::All,
    );
    let error = runner::start(request, smoke.context()).unwrap_err();
    assert_eq!(error.code, ErrorCode::InputTypeNotAllowed, "{error:?}");
    assert_eq!(smoke.run_dirs(), before);
    report(&format!("  aleapp -t itunes: refused ({})", error.code));

    unknown_profile_is_refused(&smoke, &fs_input, "get_etc_hosts");
    cancel_mid_run(&smoke, &fs_input);
}

/// iLEAPP on an encrypted backup without a password asks for one (LEAPP-CLI.md Q5). The runner
/// never starts it that way; this runs it directly, as a spawn does (own session or job, stdin
/// null, no window), and checks that the prompt cannot block (§9 item 4).
fn password_prompt_does_not_block(smoke: &Smoke) {
    let input = encrypted_backup(smoke.root());
    let prepared = smoke.context().tool;
    let out = smoke.root().join("prompt");
    fs::create_dir_all(&out).unwrap();
    let job = "20260101-000000Z-ileapp-0f0f0f";
    let temp = process::create_temp_dir(&smoke.paths.app_cache, job).unwrap();
    let mut spec = SpawnSpec::new(
        &prepared.entry,
        &out,
        out.join("stdout.log"),
        out.join("stderr.log"),
    );
    spec.args = [
        "-t",
        "itunes",
        "-i",
        "",
        "-o",
        "",
        "--custom_output_folder",
        "report",
        "-tz",
        "UTC",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    spec.args[3] = input.clone().into_os_string();
    spec.args[5] = out.clone().into_os_string();
    spec.temp_dir = Some(temp);
    spec.timeout = Some(Duration::from_secs(180));
    let handle = process::spawn(spec).unwrap();
    let exit = handle.wait().unwrap();
    process::remove_temp_dir(&smoke.paths.app_cache, job).unwrap();
    let tail = |name: &str| tail::last_lines(&out.join(name), 8).unwrap().join(" | ");
    report(&format!(
        "  ileapp encrypted backup without a password: exit {:?} signal {:?} timed out {}; \
         stdout: {}; stderr: {}",
        exit.exit_code,
        exit.signal,
        exit.timed_out,
        tail("stdout.log"),
        tail("stderr.log")
    ));
    assert!(!exit.timed_out, "the password prompt blocked");
    assert!(!out.join("report").join("index.html").exists());
}
