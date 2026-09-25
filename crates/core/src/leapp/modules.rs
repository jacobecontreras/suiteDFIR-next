//! Module introspection: the pinned binary lists its own modules, always-run artifacts and
//! timezones, which become `modules.json` (ROADMAP A3; docs/LEAPP-CLI.md §5; CONTRACTS.md §5;
//! ARCHITECTURE.md D11).
//!
//! **Procedure** ([`introspect`]):
//! 1. Create a per-job temp dir `<app_cache>/tmp/<id>` through [`process::create_temp_dir`], named
//!    like a run id (`YYYYMMDD-HHMMSSZ-<tool>-<6 hex>`). It is only a temp-dir name, so it never
//!    collides with a real run's folder. It holds the probe artifact
//!    (`probe_artifacts/suitedfir_probe.py`), a one-file `fs` input, an empty output dir and a
//!    profile selecting only the probe; it is also the tool's `TMPDIR`/`TEMP`/`TMP`.
//! 2. Run `<entry> -t fs -i … -o … --custom_output_folder probe --custom_artifacts_path … -m …`
//!    through [`process`] (own session/job, stdin null) with `SUITEDFIR_PROBE_OUT` set, and a
//!    180 s timeout. LEAPP loads the probe with its own `PluginLoader`; the probe writes every
//!    built-in plugin (and, if the binary has it, `pytz.all_timezones`) to that file.
//! 3. Apply the tool's rules (below) and require at least [`MIN_MODULES`] selectable modules.
//! 4. Remove the temp dir.
//!
//! If the tool exits without running the probe because the system's glibc is too old for the
//! pinned Linux build (the loader's `version 'GLIBC_x.y' not found`, LEAPP-CLI.md §2), the
//! `introspection_failed` message says which glibc is needed ([`glibc_too_old`]), and the loader's
//! line leads the detail.
//!
//! **Tool rules** (verified against the source at the pinned tags; see LEAPP-CLI.md §5):
//! - iLEAPP v2026.4.2 (`ileapp.py:237-241, 455-491`): the selectable list excludes
//!   `module_name == "iTunesBackupInfo"`, `name == "last_build"`, and `module_name == "logarchive"`
//!   unless `name == "logarchive"`. `last_build` always runs, except with `-t itunes`, where
//!   `itunes_backup_info` and `itunes_backup_installed_applications` run instead. Timezones are
//!   required.
//! - aLEAPP v2026.4.1 (`aleapp.py:206-212, 329`): plugins with `module_name ==
//!   "usagestatsVersion"` are not selectable and always run first, for every input type.
//!   `timezones` is `null` (aLEAPP has no timezone option).
//!
//! The rules also name the always-run plugins' **module** names ([`always_run`]), which the status
//! rules match lava entries by (CONTRACTS.md §7.3) but `modules.json` does not store. Introspection
//! checks the binary against them: every always-run artifact must exist with the encoded module
//! name, and no selectable plugin may share such a module name. A mismatch means the encoded
//! rules are stale for this binary and fails with `introspection_failed`.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::contracts::{
    AppError, ErrorCode, InputType, ModuleInfo, ModulesFile, Timestamp, ToolId, ToolManifest,
    VersionedFile,
};
use crate::hashing::to_hex;
use crate::leapp::install::InstallError;
use crate::process::{self, ExitInfo, SpawnSpec};
use crate::run::status::AlwaysRun;

/// Introspection is stopped and fails after this long (LEAPP-CLI.md §5 step 2).
pub const TIMEOUT: Duration = Duration::from_secs(180);
/// Fewer selectable modules than this means the introspection went wrong (LEAPP-CLI.md §5 step 4).
pub const MIN_MODULES: usize = 500;
/// The probe artifact's key, module file stem and profile entry.
pub const PROBE_NAME: &str = "suitedfir_probe";
const PROBE_OUT_VAR: &str = "SUITEDFIR_PROBE_OUT";
/// The probe output is a few hundred KB; anything much larger is not ours.
const MAX_PROBE_OUTPUT: u64 = 32 << 20;
/// How much of stdout/stderr goes into an error's detail.
const TAIL_BYTES: u64 = 4096;

/// The probe artifact (LEAPP-CLI.md §5). Its `__artifacts_v2__` keys follow the current upstream
/// artifacts at the pinned tags (e.g. iLEAPP `scripts/artifacts/lastBuild.py`, aLEAPP
/// `scripts/artifacts/usagestatsVersion.py`), plus `function`, which registers an undecorated
/// function. It is called with 5 positional arguments by iLEAPP and 4 by aLEAPP. The output is
/// written to a temporary name and renamed, so a killed run never leaves a partial file.
const PROBE_SOURCE: &str = r#"__artifacts_v2__ = {
    "suitedfir_probe": {
        "name": "suiteDFIR probe",
        "description": "Lists the artifacts of this build for suiteDFIR",
        "author": "suiteDFIR",
        "creation_date": "2026-09-25",
        "last_update_date": "2026-09-25",
        "requirements": "none",
        "category": "suiteDFIR",
        "notes": "",
        "paths": ("*/suitedfir_probe.marker",),
        "output_types": [],
        "artifact_icon": "list",
        "function": "suitedfir_probe",
    }
}


def suitedfir_probe(files_found, report_folder, seeker, wrap_text, *args):
    import json, os
    from scripts.plugin_loader import PluginLoader
    out = {"plugins": [
        {"name": p.name, "module_name": p.module_name, "category": p.category,
         "display_name": (p.artifact_info or {}).get("name"),
         "description": (p.artifact_info or {}).get("description")}
        for p in PluginLoader().plugins]}
    try:
        import pytz
        out["timezones"] = list(pytz.all_timezones)
    except Exception:
        out["timezones"] = None
    path = os.environ["SUITEDFIR_PROBE_OUT"]
    with open(path + ".partial", "w", encoding="utf-8") as f:
        json.dump(out, f)
    os.replace(path + ".partial", path)
"#;

/// Why introspection failed (`introspection_failed`). `detail` carries the tool's exit and the
/// tails of its stdout/stderr when it ran.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct IntrospectionError {
    pub message: String,
    pub detail: Option<String>,
}

impl IntrospectionError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            detail: None,
        }
    }
}

impl From<IntrospectionError> for InstallError {
    fn from(error: IntrospectionError) -> Self {
        InstallError::Introspection {
            message: error.message,
            detail: error.detail,
        }
    }
}

impl From<IntrospectionError> for AppError {
    fn from(error: IntrospectionError) -> Self {
        AppError {
            code: ErrorCode::IntrospectionFailed,
            message: format!("Module introspection failed: {}", error.message),
            detail: error.detail,
        }
    }
}

// ---- tool rules ----

/// The always-run artifacts of a tool, per input type (`None` = every other type, `default`).
struct AlwaysRunRule {
    input_type: Option<InputType>,
    /// The always-run artifact names; `None` for aLEAPP, whose names are the plugins of
    /// `module_names`.
    names: Option<&'static [&'static str]>,
    module_names: &'static [&'static str],
}

struct ToolRules {
    tool: ToolId,
    always_run: &'static [AlwaysRunRule],
    timezones: bool,
}

const ILEAPP: ToolRules = ToolRules {
    tool: ToolId::Ileapp,
    always_run: &[
        AlwaysRunRule {
            input_type: None,
            names: Some(&["last_build"]),
            module_names: &["lastBuild"],
        },
        AlwaysRunRule {
            input_type: Some(InputType::Itunes),
            names: Some(&["itunes_backup_info", "itunes_backup_installed_applications"]),
            module_names: &["iTunesBackupInfo"],
        },
    ],
    timezones: true,
};

const ALEAPP: ToolRules = ToolRules {
    tool: ToolId::Aleapp,
    always_run: &[AlwaysRunRule {
        input_type: None,
        names: None,
        module_names: &["usagestatsVersion"],
    }],
    timezones: false,
};

fn rules(tool: ToolId) -> &'static ToolRules {
    match tool {
        ToolId::Ileapp => &ILEAPP,
        ToolId::Aleapp => &ALEAPP,
    }
}

impl ToolRules {
    /// Whether the tool's own `main()` keeps the plugin out of the selectable list.
    fn excluded(&self, plugin: &ProbePlugin) -> bool {
        match self.tool {
            ToolId::Ileapp => {
                plugin.module_name == "iTunesBackupInfo"
                    || plugin.name == "last_build"
                    || (plugin.module_name == "logarchive" && plugin.name != "logarchive")
            }
            ToolId::Aleapp => plugin.module_name == "usagestatsVersion",
        }
    }

    fn rule_for(&self, input_type: InputType) -> Option<&AlwaysRunRule> {
        self.always_run
            .iter()
            .find(|rule| rule.input_type == Some(input_type))
            .or_else(|| {
                self.always_run
                    .iter()
                    .find(|rule| rule.input_type.is_none())
            })
    }
}

/// The always-run entries of a run: what the status rules treat as always-run
/// (`run::status`'s `AlwaysRun`) and what `run.json` records as `modules.always_run`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AlwaysRunSet {
    /// Artifact names, from `modules.json` `always_run` (the entry for the input type, else
    /// `default`).
    pub names: Vec<String>,
    /// The always-run plugins' module names, from the encoded tool rules.
    pub module_names: Vec<String>,
}

impl AlwaysRunSet {
    /// The set as the status rules take it.
    pub fn for_status(&self) -> AlwaysRun<'_> {
        AlwaysRun {
            names: &self.names,
            module_names: &self.module_names,
        }
    }
}

/// The always-run entries for a run of `tool` with `input_type`, given the tool's
/// `modules.json` `always_run` map.
pub fn always_run(
    tool: ToolId,
    always_run: &BTreeMap<String, Vec<String>>,
    input_type: InputType,
) -> AlwaysRunSet {
    let names = always_run
        .get(input_type.as_str())
        .or_else(|| always_run.get(DEFAULT_KEY))
        .cloned()
        .unwrap_or_default();
    let module_names = rules(tool)
        .rule_for(input_type)
        .map(|rule| {
            rule.module_names
                .iter()
                .map(|name| (*name).to_owned())
                .collect()
        })
        .unwrap_or_default();
    AlwaysRunSet {
        names,
        module_names,
    }
}

/// The `always_run` key for input types without their own entry.
const DEFAULT_KEY: &str = "default";

// ---- the probe's output ----

#[derive(Debug, Deserialize)]
struct ProbeOutput {
    plugins: Vec<ProbePlugin>,
    timezones: Option<Vec<String>>,
}

/// One plugin as the probe saw it. The text fields come from artifact dicts, so anything that is
/// not a string is treated as missing.
#[derive(Clone, Debug, Deserialize)]
struct ProbePlugin {
    name: String,
    module_name: String,
    #[serde(default)]
    category: serde_json::Value,
    #[serde(default)]
    display_name: serde_json::Value,
    #[serde(default)]
    description: serde_json::Value,
}

fn text(value: &serde_json::Value) -> Option<&str> {
    value.as_str().filter(|text| !text.trim().is_empty())
}

impl ProbePlugin {
    fn module_info(&self) -> ModuleInfo {
        ModuleInfo {
            name: self.name.clone(),
            module_name: self.module_name.clone(),
            category: text(&self.category).unwrap_or_default().to_owned(),
            display_name: text(&self.display_name).unwrap_or(&self.name).to_owned(),
            description: text(&self.description).map(str::to_owned),
        }
    }
}

/// Applies the tool rules to the probe output (step 3).
fn modules_file(
    tool: ToolId,
    version: &str,
    generated_at: Timestamp,
    output: ProbeOutput,
) -> Result<ModulesFile, IntrospectionError> {
    let rules = rules(tool);
    let plugins: Vec<ProbePlugin> = output
        .plugins
        .into_iter()
        .filter(|plugin| plugin.name != PROBE_NAME)
        .collect();
    let mut seen = BTreeSet::new();
    for plugin in &plugins {
        if plugin.name.is_empty() || !seen.insert(plugin.name.as_str()) {
            return Err(IntrospectionError::new(format!(
                "the probe listed an empty or duplicate artifact name {:?}",
                plugin.name
            )));
        }
    }
    let stale = |what: String| {
        IntrospectionError::new(format!(
            "{tool} {version} does not match suiteDFIR's rules for it: {what}"
        ))
    };

    let mut always_run = BTreeMap::new();
    for rule in rules.always_run {
        let key = rule
            .input_type
            .map_or(DEFAULT_KEY, InputType::as_str)
            .to_owned();
        let names: Vec<String> = match rule.names {
            Some(names) => {
                for name in names {
                    let plugin = plugins.iter().find(|p| p.name == *name);
                    match plugin {
                        Some(p) if rule.module_names.contains(&p.module_name.as_str()) => {}
                        Some(p) => {
                            return Err(stale(format!(
                                "always-run artifact {name} is in module {}, expected {}",
                                p.module_name,
                                rule.module_names.join(", ")
                            )));
                        }
                        None => {
                            return Err(stale(format!("always-run artifact {name} is missing")));
                        }
                    }
                }
                names.iter().map(|name| (*name).to_owned()).collect()
            }
            None => {
                let mut names: Vec<String> = plugins
                    .iter()
                    .filter(|p| rule.module_names.contains(&p.module_name.as_str()))
                    .map(|p| p.name.clone())
                    .collect();
                names.sort();
                if names.is_empty() {
                    return Err(stale(format!(
                        "no artifact in always-run module {}",
                        rule.module_names.join(", ")
                    )));
                }
                names
            }
        };
        always_run.insert(key, names);
    }

    let mut modules: Vec<ModuleInfo> = Vec::new();
    for plugin in plugins.iter().filter(|p| !rules.excluded(p)) {
        let shares_module = rules
            .always_run
            .iter()
            .any(|rule| rule.module_names.contains(&plugin.module_name.as_str()));
        if shares_module {
            return Err(stale(format!(
                "selectable artifact {} is in always-run module {}",
                plugin.name, plugin.module_name
            )));
        }
        modules.push(plugin.module_info());
    }
    let always: BTreeSet<&String> = always_run.values().flatten().collect();
    if let Some(module) = modules.iter().find(|m| always.contains(&m.name)) {
        return Err(stale(format!(
            "always-run artifact {} is selectable",
            module.name
        )));
    }
    modules.sort_by_cached_key(|m| {
        (
            m.category.to_lowercase(),
            m.display_name.to_lowercase(),
            m.name.clone(),
        )
    });
    if modules.len() < MIN_MODULES {
        return Err(IntrospectionError::new(format!(
            "{tool} {version} lists only {} selectable modules; at least {MIN_MODULES} are expected",
            modules.len()
        )));
    }

    let timezones = if rules.timezones {
        match output.timezones {
            Some(zones) if !zones.is_empty() => Some(zones),
            _ => {
                return Err(IntrospectionError::new(format!(
                    "{tool} {version} reported no timezone list"
                )));
            }
        }
    } else {
        None
    };
    Ok(ModulesFile {
        schema_version: ModulesFile::SCHEMA_VERSION,
        tool,
        version: version.to_owned(),
        generated_at,
        always_run,
        timezones,
        modules,
    })
}

// ---- running the probe ----

/// Runs the introspection for the installed (or staged) `entry` of `tool` (see the module docs).
/// `manifest` supplies the version and the profile format; `app_cache` holds the per-job temp dir.
/// LEAPP's exit code is not used (it exits 0 on most failures, LEAPP-CLI.md Q3): the probe's
/// output decides. Blocks until the tool has exited (at most [`TIMEOUT`] plus the kill grace).
///
/// **`entry` must be hash-verified**: this runs whatever it is given. `install` calls it only after
/// the entry hash was checked; any other caller (e.g. a later re-introspection) must first pass
/// `install::verify` and use its `VerifiedTool::entry`. A relative `entry` is made absolute
/// against the current dir, so it does not depend on the tool's working dir.
pub fn introspect(
    entry: &Path,
    tool: ToolId,
    manifest: &ToolManifest,
    app_cache: &Path,
) -> Result<ModulesFile, IntrospectionError> {
    let entry = std::path::absolute(entry)
        .map_err(|e| IntrospectionError::new(format!("cannot resolve {}: {e}", entry.display())))?;
    let id = job_id(tool, Timestamp::now())?;
    let dir = process::create_temp_dir(app_cache, &id)
        .map_err(|e| IntrospectionError::new(format!("cannot create the temp dir: {e}")))?;
    let result = std::path::absolute(&dir)
        .map_err(|e| IntrospectionError::new(format!("cannot resolve {}: {e}", dir.display())))
        .and_then(|dir| run_probe(&entry, tool, manifest, &dir))
        .and_then(|output| modules_file(tool, &manifest.version, Timestamp::now(), output));
    if let Err(e) = process::remove_temp_dir(app_cache, &id) {
        // The startup sweep removes it later.
        log::warn!("cannot remove the introspection temp dir {id}: {e}");
    }
    result
}

/// `YYYYMMDD-HHMMSSZ-<tool>-<6 lowercase hex>`: shaped like a run id, as `process` requires for
/// temp dirs (ARCHITECTURE.md §8).
fn job_id(tool: ToolId, at: Timestamp) -> Result<String, IntrospectionError> {
    let mut random = [0u8; 3];
    getrandom::fill(&mut random)
        .map_err(|e| IntrospectionError::new(format!("cannot get random bytes: {e}")))?;
    let t = at.as_datetime();
    Ok(format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}Z-{tool}-{}",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        to_hex(&random)
    ))
}

/// The files in the temp dir `dir` (step 1).
struct ProbeLayout {
    artifacts: PathBuf,
    input: PathBuf,
    out: PathBuf,
    profile: PathBuf,
    output: PathBuf,
    stdout: PathBuf,
    stderr: PathBuf,
}

fn write_probe(dir: &Path, manifest: &ToolManifest) -> io::Result<ProbeLayout> {
    let layout = ProbeLayout {
        artifacts: dir.join("probe_artifacts"),
        input: dir.join("input"),
        out: dir.join("out"),
        profile: dir.join(format!("probe.{}", manifest.profile_ext)),
        output: dir.join("probe.json"),
        stdout: dir.join("probe.stdout.log"),
        stderr: dir.join("probe.stderr.log"),
    };
    fs::create_dir(&layout.artifacts)?;
    fs::write(
        layout.artifacts.join(format!("{PROBE_NAME}.py")),
        PROBE_SOURCE,
    )?;
    // A non-empty input dir (an empty `fs` input is an argparse error, exit 2).
    fs::create_dir(&layout.input)?;
    fs::write(
        layout.input.join(format!("{PROBE_NAME}.marker")),
        "suiteDFIR introspection probe\n",
    )?;
    fs::create_dir(&layout.out)?;
    let profile = serde_json::json!({
        "leapp": manifest.profile_leapp_id,
        "format_version": 1,
        "plugins": [PROBE_NAME],
    });
    fs::write(&layout.profile, profile.to_string())?;
    Ok(layout)
}

fn probe_args(layout: &ProbeLayout) -> Vec<OsString> {
    let mut args: Vec<OsString> = Vec::new();
    let mut push = |flag: &str, value: &Path| {
        args.push(flag.into());
        args.push(value.as_os_str().to_owned());
    };
    push("-t", Path::new("fs"));
    push("-i", &layout.input);
    push("-o", &layout.out);
    push("--custom_output_folder", Path::new("probe"));
    push("--custom_artifacts_path", &layout.artifacts);
    push("-m", &layout.profile);
    args
}

/// Steps 1–2: writes the probe files into `dir`, runs the tool and reads the probe output.
fn run_probe(
    entry: &Path,
    tool: ToolId,
    manifest: &ToolManifest,
    dir: &Path,
) -> Result<ProbeOutput, IntrospectionError> {
    let layout = write_probe(dir, manifest)
        .map_err(|e| IntrospectionError::new(format!("cannot write the probe files: {e}")))?;
    let mut spec = SpawnSpec::new(entry, dir, &layout.stdout, &layout.stderr);
    spec.args = probe_args(&layout);
    spec.env = vec![(PROBE_OUT_VAR.into(), layout.output.clone().into())];
    spec.temp_dir = Some(dir.to_path_buf());
    spec.timeout = Some(TIMEOUT);
    let handle = process::spawn(spec)
        .map_err(|e| IntrospectionError::new(format!("cannot start {}: {e}", entry.display())))?;
    let exit = handle.wait().map_err(|e| {
        IntrospectionError::new(format!("waiting for {} failed: {e}", entry.display()))
    })?;
    let failure = |message: String| IntrospectionError {
        message,
        detail: Some(detail(&exit, &layout)),
    };
    if exit.timed_out {
        return Err(failure(format!(
            "{tool} did not finish within {} s",
            TIMEOUT.as_secs()
        )));
    }
    let bytes = match read_capped(&layout.output) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(probe_not_run(tool, &manifest.display_name, &exit, &layout));
        }
        Err(e) => return Err(failure(format!("cannot read the probe output: {e}"))),
    };
    serde_json::from_slice(&bytes)
        .map_err(|e| failure(format!("the probe output is not valid: {e}")))
}

/// The error when the tool exited without writing the probe output. A glibc too old for the
/// pinned Linux build gets its own message, with the loader's line first in the detail.
fn probe_not_run(
    tool: ToolId,
    display_name: &str,
    exit: &ExitInfo,
    layout: &ProbeLayout,
) -> IntrospectionError {
    let output = [&layout.stderr, &layout.stdout]
        .map(|path| tail(path).unwrap_or_default())
        .join("\n");
    match glibc_too_old(&output) {
        Some(too_old) => IntrospectionError {
            message: too_old.message(display_name),
            detail: Some(format!("{}\n{}", too_old.loader_line, detail(exit, layout))),
        },
        None => IntrospectionError {
            message: format!(
                "{tool} exited ({}) without running the probe",
                describe_exit(exit)
            ),
            detail: Some(detail(exit, layout)),
        },
    }
}

/// A dynamic-loader error saying that the system's glibc lacks a symbol version the tool needs,
/// e.g. `…/libpython3.14.so.1.0: /lib/x86_64-linux-gnu/libm.so.6: version `GLIBC_2.38' not found
/// (required by …)` (LEAPP-CLI.md §2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlibcTooOld {
    /// The highest missing version, e.g. `2.38`.
    pub required: String,
    /// The loader's line naming it, as printed.
    pub loader_line: String,
}

impl GlibcTooOld {
    /// The message for the examiner, e.g. for `introspection_failed`.
    pub fn message(&self, tool_name: &str) -> String {
        format!(
            "the pinned Linux {tool_name} build needs glibc {} or newer; this system's glibc is \
             too old",
            self.required
        )
    }
}

/// Finds loader errors `version `GLIBC_x.y' not found` (quoted with a backtick or an apostrophe,
/// then an apostrophe) in a tool's output, and returns the highest missing version. Usable on the
/// output of any LEAPP spawn.
pub fn glibc_too_old(output: &str) -> Option<GlibcTooOld> {
    const MARK: &str = "GLIBC_";
    let mut best: Option<(Vec<u32>, GlibcTooOld)> = None;
    for line in output.lines() {
        for (at, _) in line.match_indices(MARK) {
            let before = &line[..at];
            if !(before.ends_with("version `") || before.ends_with("version '")) {
                continue;
            }
            let after = &line[at + MARK.len()..];
            let end = after
                .find(|c: char| !(c.is_ascii_digit() || c == '.'))
                .unwrap_or(after.len());
            let version = &after[..end];
            let parts: Option<Vec<u32>> = version.split('.').map(|p| p.parse().ok()).collect();
            let Some(parts) = parts.filter(|parts| parts.len() >= 2) else {
                continue;
            };
            if !after[end..].starts_with("' not found") {
                continue;
            }
            if best.as_ref().is_none_or(|(highest, _)| parts > *highest) {
                let found = GlibcTooOld {
                    required: version.to_owned(),
                    loader_line: line.trim().to_owned(),
                };
                best = Some((parts, found));
            }
        }
    }
    best.map(|(_, found)| found)
}

fn read_capped(path: &Path) -> io::Result<Vec<u8>> {
    let file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.take(MAX_PROBE_OUTPUT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PROBE_OUTPUT {
        return Err(io::Error::other("the probe output is too large"));
    }
    Ok(bytes)
}

fn describe_exit(exit: &ExitInfo) -> String {
    match (exit.exit_code, exit.signal) {
        (Some(code), _) => format!("exit code {code}"),
        (None, Some(signal)) => format!("killed by signal {signal}"),
        (None, None) => "no exit code".to_owned(),
    }
}

/// The exit and the ends of stdout/stderr, for an error's detail.
fn detail(exit: &ExitInfo, layout: &ProbeLayout) -> String {
    let mut text = describe_exit(exit);
    if exit.timed_out {
        text.push_str(" after the timeout");
    }
    for (name, path) in [("stdout", &layout.stdout), ("stderr", &layout.stderr)] {
        let tail = tail(path).unwrap_or_else(|e| format!("(unreadable: {e})"));
        text.push_str(&format!("\n--- {name} (end) ---\n{}", tail.trim_end()));
    }
    text
}

fn tail(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let len = file.metadata()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES)))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::contracts::examples;

    fn plugin(name: &str, module_name: &str, category: &str) -> serde_json::Value {
        json!({"name": name, "module_name": module_name, "category": category,
               "display_name": format!("Display {name}"), "description": null})
    }

    /// A probe output with `count` ordinary plugins plus the tool's special ones.
    fn probe(tool: ToolId, count: usize) -> serde_json::Value {
        let mut plugins: Vec<serde_json::Value> = (0..count)
            .map(|i| plugin(&format!("art{i:04}"), &format!("mod{}", i / 3), "General"))
            .collect();
        plugins.push(plugin(PROBE_NAME, PROBE_NAME, "suiteDFIR"));
        let timezones = match tool {
            ToolId::Ileapp => {
                plugins.push(plugin("last_build", "lastBuild", "IOS Build"));
                plugins.push(plugin("itunes_backup_info", "iTunesBackupInfo", "Device"));
                plugins.push(plugin(
                    "itunes_backup_installed_applications",
                    "iTunesBackupInfo",
                    "Installed Apps",
                ));
                plugins.push(plugin("logarchive", "logarchive", "Unified Logs"));
                plugins.push(plugin("logarchive_artifacts", "logarchive", "Unified Logs"));
                plugins.push(plugin("logarchive_wifi", "logarchive", "Unified Logs"));
                json!(["Africa/Abidjan", "America/Chicago", "UTC"])
            }
            ToolId::Aleapp => {
                plugins.push(plugin("usagestatsVersion", "usagestatsVersion", "Device"));
                // aLEAPP bundles pytz too; its list is still not reported.
                json!(["UTC"])
            }
        };
        json!({"plugins": plugins, "timezones": timezones})
    }

    fn apply(tool: ToolId, output: serde_json::Value) -> Result<ModulesFile, IntrospectionError> {
        let output: ProbeOutput = serde_json::from_value(output).unwrap();
        let at = Timestamp::parse("2026-09-25T10:00:00Z").unwrap();
        modules_file(tool, "v2026.4.2", at, output)
    }

    fn plugins_mut(output: &mut serde_json::Value) -> &mut Vec<serde_json::Value> {
        output["plugins"].as_array_mut().unwrap()
    }

    #[test]
    fn ileapp_rules() {
        let file = apply(ToolId::Ileapp, probe(ToolId::Ileapp, 600)).unwrap();
        assert_eq!(file.schema_version, 1);
        assert_eq!(file.tool, ToolId::Ileapp);
        assert_eq!(file.version, "v2026.4.2");
        assert_eq!(file.generated_at.to_string(), "2026-09-25T10:00:00Z");
        let expected: BTreeMap<String, Vec<String>> = [
            ("default".to_owned(), vec!["last_build".to_owned()]),
            (
                "itunes".to_owned(),
                vec![
                    "itunes_backup_info".to_owned(),
                    "itunes_backup_installed_applications".to_owned(),
                ],
            ),
        ]
        .into();
        assert_eq!(file.always_run, expected);
        assert_eq!(
            file.timezones.as_deref().unwrap(),
            ["Africa/Abidjan", "America/Chicago", "UTC"]
        );
        let names: BTreeSet<&str> = file.modules.iter().map(|m| m.name.as_str()).collect();
        // 600 ordinary plugins plus logarchive itself; no probe, always-run or logarchive children.
        assert_eq!(file.modules.len(), 601);
        assert!(names.contains("logarchive"));
        for gone in [
            PROBE_NAME,
            "last_build",
            "itunes_backup_info",
            "itunes_backup_installed_applications",
            "logarchive_artifacts",
            "logarchive_wifi",
        ] {
            assert!(!names.contains(gone), "{gone}");
        }
    }

    #[test]
    fn aleapp_rules() {
        let file = apply(ToolId::Aleapp, probe(ToolId::Aleapp, 600)).unwrap();
        assert_eq!(file.tool, ToolId::Aleapp);
        let expected: BTreeMap<String, Vec<String>> =
            [("default".to_owned(), vec!["usagestatsVersion".to_owned()])].into();
        assert_eq!(file.always_run, expected);
        assert_eq!(file.timezones, None);
        assert_eq!(file.modules.len(), 600);
        assert!(
            file.modules
                .iter()
                .all(|m| m.module_name != "usagestatsVersion")
        );
        // aLEAPP excludes nothing else (logarchive-like names are selectable there).
        let mut output = probe(ToolId::Aleapp, 600);
        plugins_mut(&mut output).push(plugin("logarchive_wifi", "logarchive", "X"));
        assert_eq!(apply(ToolId::Aleapp, output).unwrap().modules.len(), 601);
    }

    #[test]
    fn modules_are_sorted_by_category_then_display_name_case_insensitively() {
        let mut output = probe(ToolId::Aleapp, 500);
        let extra = [
            json!({"name": "b", "module_name": "m1", "category": "alpha", "display_name": "Zeta"}),
            json!({"name": "a", "module_name": "m2", "category": "Alpha", "display_name": "eta"}),
            json!({"name": "c", "module_name": "m3", "category": "ALPHA", "display_name": "Eta"}),
            json!({"name": "d", "module_name": "m4", "category": "Beta", "display_name": "x"}),
        ];
        plugins_mut(&mut output).extend(extra);
        let file = apply(ToolId::Aleapp, output).unwrap();
        let order: Vec<&str> = file
            .modules
            .iter()
            .filter(|m| m.name.len() == 1)
            .map(|m| m.name.as_str())
            .collect();
        assert_eq!(order, ["a", "c", "b", "d"]);
        let mut sorted = file.modules.clone();
        sorted.sort_by_key(|m| (m.category.to_lowercase(), m.display_name.to_lowercase()));
        let keys = |modules: &[ModuleInfo]| {
            modules
                .iter()
                .map(|m| (m.category.to_lowercase(), m.display_name.to_lowercase()))
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(&file.modules), keys(&sorted));
    }

    #[test]
    fn missing_text_falls_back() {
        let mut output = probe(ToolId::Aleapp, 500);
        plugins_mut(&mut output).push(json!({
            "name": "v1Artifact", "module_name": "v1mod", "category": null,
            "display_name": null, "description": ""
        }));
        plugins_mut(&mut output).push(json!({
            "name": "odd", "module_name": "oddmod", "category": "Odd",
            "display_name": ["not", "text"], "description": "Parses odd things"
        }));
        let file = apply(ToolId::Aleapp, output).unwrap();
        let find = |name: &str| file.modules.iter().find(|m| m.name == name).unwrap();
        let v1 = find("v1Artifact");
        assert_eq!(
            (v1.category.as_str(), v1.display_name.as_str()),
            ("", "v1Artifact")
        );
        assert_eq!(v1.description, None);
        let odd = find("odd");
        assert_eq!(odd.display_name, "odd");
        assert_eq!(odd.description.as_deref(), Some("Parses odd things"));
    }

    #[test]
    fn fewer_than_500_modules_fail() {
        let error = apply(ToolId::Ileapp, probe(ToolId::Ileapp, 498)).unwrap_err();
        assert!(
            error.message.contains("only 499 selectable modules"),
            "{error}"
        );
        assert!(apply(ToolId::Aleapp, probe(ToolId::Aleapp, 500)).is_ok());
        assert!(apply(ToolId::Aleapp, probe(ToolId::Aleapp, 499)).is_err());
        let app: AppError = error.into();
        assert_eq!(app.code, ErrorCode::IntrospectionFailed);
    }

    #[test]
    fn stale_rules_fail() {
        let drop = |tool, name: &str| {
            let mut output = probe(tool, 600);
            plugins_mut(&mut output).retain(|p| p["name"] != name);
            apply(tool, output).unwrap_err().message
        };
        assert!(drop(ToolId::Ileapp, "last_build").contains("last_build is missing"));
        assert!(drop(ToolId::Ileapp, "itunes_backup_info").contains("itunes_backup_info"));
        assert!(drop(ToolId::Aleapp, "usagestatsVersion").contains("no artifact"));

        // An always-run artifact in another module.
        let mut output = probe(ToolId::Ileapp, 600);
        for p in plugins_mut(&mut output) {
            if p["name"] == "last_build" {
                p["module_name"] = json!("buildInfo");
            }
        }
        let error = apply(ToolId::Ileapp, output).unwrap_err();
        assert!(error.message.contains("in module buildInfo"), "{error}");

        // A selectable artifact sharing an always-run module would be misread as always-run.
        let mut output = probe(ToolId::Ileapp, 600);
        plugins_mut(&mut output).push(plugin("build_extra", "lastBuild", "IOS Build"));
        let error = apply(ToolId::Ileapp, output).unwrap_err();
        assert!(error.message.contains("build_extra"), "{error}");

        // Duplicate names.
        let mut output = probe(ToolId::Aleapp, 600);
        plugins_mut(&mut output).push(plugin("art0001", "other", "X"));
        assert!(
            apply(ToolId::Aleapp, output)
                .unwrap_err()
                .message
                .contains("duplicate")
        );
    }

    #[test]
    fn ileapp_needs_timezones() {
        for zones in [json!(null), json!([])] {
            let mut output = probe(ToolId::Ileapp, 600);
            output["timezones"] = zones;
            let error = apply(ToolId::Ileapp, output).unwrap_err();
            assert!(error.message.contains("no timezone list"), "{error}");
        }
        let mut output = probe(ToolId::Aleapp, 600);
        output["timezones"] = json!(null);
        assert_eq!(apply(ToolId::Aleapp, output).unwrap().timezones, None);
    }

    #[test]
    fn always_run_sets_for_the_status_rules() {
        let ileapp = apply(ToolId::Ileapp, probe(ToolId::Ileapp, 600)).unwrap();
        let set = always_run(ToolId::Ileapp, &ileapp.always_run, InputType::Fs);
        assert_eq!(set.names, ["last_build"]);
        assert_eq!(set.module_names, ["lastBuild"]);
        let set = always_run(ToolId::Ileapp, &ileapp.always_run, InputType::Itunes);
        assert_eq!(
            set.names,
            ["itunes_backup_info", "itunes_backup_installed_applications"]
        );
        assert_eq!(set.module_names, ["iTunesBackupInfo"]);
        let status = set.for_status();
        assert_eq!(status.names, set.names.as_slice());
        assert_eq!(status.module_names, set.module_names.as_slice());
        for input_type in [InputType::Tar, InputType::Zip, InputType::Raw] {
            assert_eq!(
                always_run(ToolId::Ileapp, &ileapp.always_run, input_type).names,
                ["last_build"]
            );
        }
        let aleapp = apply(ToolId::Aleapp, probe(ToolId::Aleapp, 600)).unwrap();
        for input_type in [InputType::Fs, InputType::Gz] {
            let set = always_run(ToolId::Aleapp, &aleapp.always_run, input_type);
            assert_eq!(set.names, ["usagestatsVersion"]);
            assert_eq!(set.module_names, ["usagestatsVersion"]);
        }
        // The contract example's map works the same way.
        let example = examples::modules_file();
        let set = always_run(ToolId::Ileapp, &example.always_run, InputType::Itunes);
        assert_eq!(set.names.len(), 2);
        assert_eq!(
            always_run(ToolId::Aleapp, &BTreeMap::new(), InputType::Fs),
            AlwaysRunSet {
                names: Vec::new(),
                module_names: vec!["usagestatsVersion".to_owned()]
            }
        );
    }

    #[test]
    fn job_ids_are_run_id_shaped() {
        let at = Timestamp::parse("2026-09-05T08:07:06Z").unwrap();
        let id = job_id(ToolId::Aleapp, at).unwrap();
        assert!(id.starts_with("20260905-080706Z-aleapp-"), "{id}");
        let suffix = &id["20260905-080706Z-aleapp-".len()..];
        assert_eq!(suffix.len(), 6);
        assert!(
            suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        // process accepts it as a temp dir name.
        assert!(process::temp_dir_path(Path::new("cache"), &id).is_ok());
    }

    #[test]
    fn the_probe_files_and_arguments() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = &examples::leapp_manifest().tools[&ToolId::Aleapp];
        let layout = write_probe(dir.path(), manifest).unwrap();
        let source = fs::read_to_string(layout.artifacts.join("suitedfir_probe.py")).unwrap();
        assert_eq!(source, PROBE_SOURCE);
        assert!(source.contains("\"function\": \"suitedfir_probe\""));
        assert!(source.contains(PROBE_OUT_VAR));
        assert!(layout.input.join("suitedfir_probe.marker").is_file());
        assert!(fs::read_dir(&layout.out).unwrap().next().is_none());
        assert_eq!(layout.profile, dir.path().join("probe.alprofile"));
        let profile: serde_json::Value =
            serde_json::from_slice(&fs::read(&layout.profile).unwrap()).unwrap();
        assert_eq!(
            profile,
            json!({"leapp": "aleapp", "format_version": 1, "plugins": ["suitedfir_probe"]})
        );
        let args: Vec<String> = probe_args(&layout)
            .into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let path = |p: &Path| p.to_string_lossy().into_owned();
        assert_eq!(
            args,
            [
                "-t".to_owned(),
                "fs".to_owned(),
                "-i".to_owned(),
                path(&layout.input),
                "-o".to_owned(),
                path(&layout.out),
                "--custom_output_folder".to_owned(),
                "probe".to_owned(),
                "--custom_artifacts_path".to_owned(),
                path(&layout.artifacts),
                "-m".to_owned(),
                path(&layout.profile),
            ]
        );
    }

    fn exit(exit_code: Option<i32>, signal: Option<i32>, timed_out: bool) -> ExitInfo {
        ExitInfo {
            exit_code,
            signal,
            exited_at: Timestamp::now(),
            exit_instant: std::time::Instant::now(),
            cancel_requested: false,
            timed_out,
            escalated_to_kill: false,
            output_error: None,
        }
    }

    fn logs_layout(dir: &Path) -> ProbeLayout {
        ProbeLayout {
            artifacts: dir.join("a"),
            input: dir.join("i"),
            out: dir.join("o"),
            profile: dir.join("p"),
            output: dir.join("probe.json"),
            stdout: dir.join("out.log"),
            stderr: dir.join("err.log"),
        }
    }

    /// As printed by the pinned iLEAPP build in ubuntu:22.04 (leapp-smoke run 36170550467).
    const LOADER_ERROR: &str = "[PYI-4965:ERROR] Failed to load Python shared library \
        '/tmp/sdnxFSj25/cache/tmp/20260925-180054Z-ileapp-b741b8/_MEI6badXH/libpython3.14.so.1.0': \
        /lib/x86_64-linux-gnu/libm.so.6: version `GLIBC_2.38' not found (required by \
        /tmp/sdnxFSj25/cache/tmp/20260925-180054Z-ileapp-b741b8/_MEI6badXH/libpython3.14.so.1.0)";

    #[test]
    fn glibc_loader_errors_are_recognized() {
        let found = glibc_too_old(LOADER_ERROR).unwrap();
        assert_eq!(found.required, "2.38");
        assert_eq!(found.loader_line, LOADER_ERROR);
        assert_eq!(
            found.message("iLEAPP"),
            "the pinned Linux iLEAPP build needs glibc 2.38 or newer; this system's glibc is too old"
        );
        // The highest missing version wins, compared numerically (2.9 < 2.38 < 2.43).
        let several = format!(
            "noise\n{LOADER_ERROR}\n./x: /lib/libm.so.6: version 'GLIBC_2.43' not found \
             (required by ./libmvec.so.1)\n./y: version `GLIBC_2.9' not found\n\
             ./z: version `GLIBC_2.3.4' not found (required by ./z)"
        );
        let found = glibc_too_old(&several).unwrap();
        assert_eq!(found.required, "2.43");
        assert!(found.loader_line.contains("libmvec"), "{found:?}");
        for text in [
            "",
            "GLIBC_2.38",
            "version `GLIBC_2.38' found",
            "needs GLIBC_2.40 not found",
            "version `GLIBC_PRIVATE' not found (required by ./libmvec.so.1)",
            "version `GLIBC_2' not found",
            "version `GLIBC_2.x' not found",
        ] {
            assert_eq!(glibc_too_old(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_glibc_too_old_for_the_build_is_explained() {
        let dir = tempfile::tempdir().unwrap();
        let layout = logs_layout(dir.path());
        fs::write(&layout.stdout, "").unwrap();
        fs::write(&layout.stderr, format!("{LOADER_ERROR}\n")).unwrap();
        let error = probe_not_run(
            ToolId::Ileapp,
            "iLEAPP",
            &exit(Some(255), None, false),
            &layout,
        );
        assert_eq!(
            error.message,
            "the pinned Linux iLEAPP build needs glibc 2.38 or newer; this system's glibc is too old"
        );
        let detail = error.detail.clone().unwrap();
        assert!(
            detail.starts_with(&format!(
                "{LOADER_ERROR}\nexit code 255\n--- stdout (end) ---"
            )),
            "{detail}"
        );
        let app: AppError = error.into();
        assert_eq!(app.code, ErrorCode::IntrospectionFailed);

        // Any other failure to run the probe keeps the generic message.
        fs::write(&layout.stderr, "Traceback (most recent call last):\n").unwrap();
        let error = probe_not_run(
            ToolId::Ileapp,
            "iLEAPP",
            &exit(Some(1), None, false),
            &layout,
        );
        assert_eq!(
            error.message,
            "ileapp exited (exit code 1) without running the probe"
        );
    }

    #[test]
    fn exits_are_described() {
        assert_eq!(describe_exit(&exit(Some(2), None, false)), "exit code 2");
        assert_eq!(
            describe_exit(&exit(None, Some(9), true)),
            "killed by signal 9"
        );
        let dir = tempfile::tempdir().unwrap();
        let layout = logs_layout(dir.path());
        let long = format!("{}\nlast line\n", "x".repeat(10_000));
        fs::write(&layout.stdout, long).unwrap();
        let text = detail(&exit(None, Some(15), true), &layout);
        assert!(
            text.starts_with("killed by signal 15 after the timeout"),
            "{text}"
        );
        assert!(text.contains("--- stdout (end) ---\n"), "{text}");
        assert!(text.contains("last line"), "{text}");
        assert!(text.contains("--- stderr (end) ---\n(unreadable"), "{text}");
        assert!(text.len() < 5000, "{}", text.len());
    }

    #[test]
    fn introspection_errors_become_install_errors() {
        let error: InstallError = IntrospectionError {
            message: "m".into(),
            detail: Some("d".into()),
        }
        .into();
        assert_eq!(error.code(), ErrorCode::IntrospectionFailed);
    }
}
