//! Module introspection through the process module (ROADMAP A3), with stand-ins for LEAPP: the
//! real builds run only in the ignored `leapp_smoke` tests.
//!
//! - fake-leapp accepts LEAPP's flags but never runs the probe: introspection fails, with the
//!   tool's exit and output in the detail, and removes its temp dir (every OS).
//! - A shell script that records how it was started and writes a prepared probe output checks the
//!   invocation and the success path (Unix).
//!
//! The tests run one at a time: a script written by one test must not be executed while another
//! test's `fork` holds an inherited handle to it (ETXTBSY on Linux).

use std::fs;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use suitedfir_core::contracts::{ToolId, ToolManifest};
use suitedfir_core::leapp::modules;
use suitedfir_core::manifest;

const FAKE_LEAPP: &str = env!("CARGO_BIN_EXE_fake-leapp");

fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

fn aleapp() -> &'static ToolManifest {
    &manifest::embedded().unwrap().tools[&ToolId::Aleapp]
}

/// Introspection's temp dirs are gone (the temp root may or may not exist).
fn assert_no_temp_left(cache: &Path) {
    let root = cache.join("tmp");
    let left: Vec<String> = fs::read_dir(&root)
        .map(|entries| {
            entries
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(left.is_empty(), "left in {}: {left:?}", root.display());
}

#[test]
fn a_tool_that_does_not_run_the_probe_fails() {
    let _serial = serial();
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let error =
        modules::introspect(Path::new(FAKE_LEAPP), ToolId::Aleapp, aleapp(), &cache).unwrap_err();
    assert!(
        error
            .message
            .contains("exited (exit code 0) without running the probe"),
        "{error}"
    );
    let detail = error.detail.unwrap();
    assert!(
        detail.starts_with("exit code 0\n--- stdout (end) ---\n"),
        "{detail}"
    );
    assert!(detail.contains("--- stderr (end) ---"), "{detail}");
    assert_no_temp_left(&cache);
}

#[test]
fn a_tool_that_cannot_start_fails() {
    let _serial = serial();
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let missing = dir.path().join("no-such-tool");
    let error = modules::introspect(&missing, ToolId::Aleapp, aleapp(), &cache).unwrap_err();
    assert!(error.message.starts_with("cannot start"), "{error}");
    assert_no_temp_left(&cache);
}

/// A probe output with `count` ordinary aLEAPP plugins plus `usagestatsVersion`.
#[cfg(unix)]
fn probe_output(count: usize) -> String {
    let mut plugins: Vec<serde_json::Value> = (0..count)
        .map(|i| {
            serde_json::json!({"name": format!("artifact{i}"), "module_name": format!("mod{i}"),
                               "category": "Category", "display_name": format!("Artifact {i}"),
                               "description": null})
        })
        .collect();
    plugins.push(serde_json::json!({"name": "usagestatsVersion",
        "module_name": "usagestatsVersion", "category": "Device", "display_name": "OS Version",
        "description": "Extracts OS Version from Usagestats"}));
    serde_json::json!({"plugins": plugins, "timezones": ["UTC"]}).to_string()
}

#[cfg(unix)]
#[test]
fn a_probe_output_becomes_the_module_list() {
    use std::os::unix::fs::PermissionsExt;

    let _serial = serial();
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let seen = dir.path().join("seen");
    fs::create_dir(&seen).unwrap();
    let prepared = dir.path().join("prepared.json");
    fs::write(&prepared, probe_output(modules::MIN_MODULES + 20)).unwrap();
    let script = dir.path().join("fake-aleapp");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$@\" > '{seen}/args'\n\
             printf '%s\\n' \"$TMPDIR\" \"$TEMP\" \"$TMP\" > '{seen}/temp'\n\
             pwd -P > '{seen}/cwd'\n\
             cp \"${{10}}/suitedfir_probe.py\" '{seen}/probe.py'\n\
             cp \"${{12}}\" '{seen}/profile'\n\
             ls \"$4\" > '{seen}/input'\n\
             cp '{prepared}' \"$SUITEDFIR_PROBE_OUT\"\n",
            seen = seen.display(),
            prepared = prepared.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let file = modules::introspect(&script, ToolId::Aleapp, aleapp(), &cache).unwrap();
    assert_eq!(file.tool, ToolId::Aleapp);
    assert_eq!(file.version, aleapp().version);
    assert_eq!(file.modules.len(), modules::MIN_MODULES + 20);
    assert_eq!(file.always_run["default"], ["usagestatsVersion"]);
    // aLEAPP's list is never reported, even if the binary has pytz.
    assert_eq!(file.timezones, None);
    assert_no_temp_left(&cache);

    // How the tool was started (LEAPP-CLI.md §5 step 2).
    let read = |name: &str| fs::read_to_string(seen.join(name)).unwrap();
    let args: Vec<String> = read("args").lines().map(str::to_owned).collect();
    let temp_dir = Path::new(&args[3]).parent().unwrap().to_path_buf();
    let job = temp_dir.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(temp_dir, cache.join("tmp").join(&job), "{args:?}");
    let at = |name: &str| temp_dir.join(name).to_string_lossy().into_owned();
    assert_eq!(
        args,
        [
            "-t".to_owned(),
            "fs".to_owned(),
            "-i".to_owned(),
            at("input"),
            "-o".to_owned(),
            at("out"),
            "--custom_output_folder".to_owned(),
            "probe".to_owned(),
            "--custom_artifacts_path".to_owned(),
            at("probe_artifacts"),
            "-m".to_owned(),
            at("probe.alprofile"),
        ]
    );
    // The temp dir is named like a run id and is the tool's temp dir and working dir.
    let parts: Vec<&str> = job.split('-').collect();
    assert_eq!(parts.len(), 4, "{job}");
    assert_eq!(parts[2], "aleapp");
    assert_eq!(read("temp"), format!("{0}\n{0}\n{0}\n", temp_dir.display()));
    assert_eq!(
        Path::new(read("cwd").trim()),
        fs::canonicalize(&cache).unwrap().join("tmp").join(&job)
    );
    assert!(read("probe.py").contains("\"function\": \"suitedfir_probe\""));
    let profile: serde_json::Value = serde_json::from_str(&read("profile")).unwrap();
    assert_eq!(
        profile,
        serde_json::json!({"leapp": "aleapp", "format_version": 1, "plugins": ["suitedfir_probe"]})
    );
    assert_eq!(read("input"), "suitedfir_probe.marker\n");
}

#[cfg(unix)]
#[test]
fn too_few_modules_fail() {
    use std::os::unix::fs::PermissionsExt;

    let _serial = serial();
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let prepared = dir.path().join("prepared.json");
    fs::write(&prepared, probe_output(12)).unwrap();
    let script = dir.path().join("fake-aleapp");
    fs::write(
        &script,
        format!(
            "#!/bin/sh\ncp '{}' \"$SUITEDFIR_PROBE_OUT\"\n",
            prepared.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let error = modules::introspect(&script, ToolId::Aleapp, aleapp(), &cache).unwrap_err();
    assert!(
        error.message.contains("only 12 selectable modules"),
        "{error}"
    );
    assert_no_temp_left(&cache);
}

/// A copy of fake-leapp named `…-probe` answers the probe like a real build (every OS), for both
/// tools: the tool rules accept it, and the fillers make the list long enough.
#[test]
fn a_probe_copy_of_fake_leapp_introspects_on_every_os() {
    let _serial = serial();
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let exe = if cfg!(windows) { ".exe" } else { "" };
    let probe = dir.path().join(format!("fake-leapp-probe{exe}"));
    fs::copy(FAKE_LEAPP, &probe).unwrap();
    for tool in ToolId::ALL {
        let manifest = &manifest::embedded().unwrap().tools[tool];
        let listed = modules::introspect(&probe, *tool, manifest, &cache).unwrap();
        assert_eq!(listed.tool, *tool);
        assert_eq!(listed.version, manifest.version);
        assert!(listed.modules.len() >= modules::MIN_MODULES);
        assert!(listed.modules.iter().any(|m| m.name == "fakeFiller500"));
        match tool {
            ToolId::Ileapp => {
                assert!(listed.modules.iter().any(|m| m.name == "callHistory"));
                assert_eq!(listed.always_run["default"], ["last_build"]);
                assert!(listed.timezones.unwrap().contains(&"UTC".to_owned()));
            }
            ToolId::Aleapp => {
                assert!(listed.modules.iter().any(|m| m.name == "callLogs"));
                assert_eq!(listed.always_run["default"], ["usagestats_version"]);
                assert_eq!(listed.timezones, None);
            }
        }
        assert_no_temp_left(&cache);
    }
}
