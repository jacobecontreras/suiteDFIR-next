//! Runner integration tests with fake-leapp (ROADMAP E1a; ARCHITECTURE.md §6): every CONTRACTS.md
//! §7.4 scenario ends in a finalized, read-only `run.json` with the expected status, reasons and
//! warnings, plus `report.sha256` when a report exists. Also: concurrent input hashing, a cancel
//! after the exit that only stops the hashing, a cancel before the spawn, the validation checks
//! (overlap, `path_too_long`, unknown modules, password, timezone, type), `prepare_failed`,
//! `spawn_failed` (including a glibc too old for the build), module selection, keychains, inputs
//! inside a case's acquisitions, the event rules (log batches of at most 500 lines, each stdio tail
//! once, progress events) and that the backup password never leaks.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use suitedfir_core::case::{self, CreatedCase};
use suitedfir_core::contracts::{
    AppError, EntryVerifiedAgainst, ErrorCode, HashStatus, InputType, InstallSource, ModuleMode,
    ModuleSelection, Reason, RecordHost, RunEvent, RunPhase, RunRecord, RunRequest, RunStatus,
    SealStatus, Settings, StdStream, ToolId, examples, parse_versioned,
};
use suitedfir_core::hashing;
use suitedfir_core::manifest;
use suitedfir_core::paths::AppPaths;
use suitedfir_core::run::profile::{ProfileFormat, ProfileStore};
use suitedfir_core::runner::{self, RunContext, RunControl, RunOutcome};
use suitedfir_core::settings;

const FAKE_LEAPP: &str = env!("CARGO_BIN_EXE_fake-leapp");
const PASSWORD: &str = "e1a-Backup-Pw!";
/// An acquisition id for inputs inside a case's `acquisitions/`.
const ACQ_ID: &str = "20260924-171200Z-ios-9c01de";

fn host() -> RecordHost {
    RecordHost {
        os: "testos".to_owned(),
        os_version: "1.0".to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        hostname: "LAB-TEST-01".to_owned(),
    }
}

/// App dirs, a known case and an `fs` input folder, all in a short-named temp dir (the Windows test
/// machine has long paths disabled).
struct Lab {
    root: tempfile::TempDir,
    paths: AppPaths,
    settings: Settings,
    case: CreatedCase,
    input: PathBuf,
}

impl Lab {
    fn new() -> Self {
        let root = tempfile::Builder::new().prefix("sdr").tempdir().unwrap();
        let paths = AppPaths {
            app_data: root.path().join("data"),
            app_config: root.path().join("config"),
            app_cache: root.path().join("cache"),
            app_log: root.path().join("log"),
        };
        let cases = root.path().join("cases");
        fs::create_dir_all(&cases).unwrap();
        let case = case::create(&cases, &examples::case_fields()).unwrap();
        let mut settings = settings::defaults(&cases);
        settings::touch_recent(&mut settings, &case.path.to_string_lossy());
        let input = root.path().join("ev").join("fs");
        fs::create_dir_all(&input).unwrap();
        fs::write(input.join("evidence.txt"), "evidence").unwrap();
        Self {
            root,
            paths,
            settings,
            case,
            input,
        }
    }

    fn context(&self, tool: ToolId, scenario: &str, extra: &[(&str, &str)]) -> RunContext {
        let manifest = &manifest::embedded().unwrap().tools[&tool];
        let prepared = runner::dev_override_tool(
            Path::new(FAKE_LEAPP),
            tool,
            manifest,
            manifest::host_platform(),
        )
        .unwrap();
        let mut env: Vec<(OsString, OsString)> = vec![
            ("FAKE_LEAPP_SCENARIO".into(), scenario.into()),
            ("FAKE_LEAPP_INTERVAL_MS".into(), "5".into()),
            ("FAKE_LEAPP_LINES".into(), "20".into()),
        ];
        env.extend(
            extra
                .iter()
                .map(|(name, value)| (OsString::from(name), OsString::from(value))),
        );
        RunContext {
            paths: self.paths.clone(),
            settings: self.settings.clone(),
            case_dir: self.case.path.clone(),
            case: self.case.case.clone(),
            host: host(),
            tool: prepared,
            env,
            max_run_dir_chars: runner::default_run_dir_limit(),
        }
    }

    fn request(&self, tool: ToolId, input: &Path, input_type: InputType) -> RunRequest {
        RunRequest {
            case_path: self.case.path.to_string_lossy().into_owned(),
            tool,
            input_path: input.to_string_lossy().into_owned(),
            input_type,
            modules: ModuleSelection::All,
            timezone: None,
            itunes_password: None,
            keychain_path: None,
            hash_input: false,
            label: Some("Test run".to_owned()),
        }
    }

    fn fs_request(&self, tool: ToolId) -> RunRequest {
        self.request(tool, &self.input, InputType::Fs)
    }

    /// The case's run folders.
    fn run_dirs(&self) -> Vec<PathBuf> {
        fs::read_dir(self.case.path.join("runs"))
            .map(|entries| entries.map(|e| e.unwrap().path()).collect())
            .unwrap_or_default()
    }

    fn assert_no_temp_dirs(&self) {
        let tmp = self.paths.temp_root();
        let left: Vec<_> = fs::read_dir(&tmp)
            .map(|entries| entries.map(|e| e.unwrap().file_name()).collect())
            .unwrap_or_default();
        assert!(left.is_empty(), "left in {}: {left:?}", tmp.display());
    }

    /// An iTunes-style backup folder at `dir` (`Manifest.plist` with `IsEncrypted`).
    fn itunes_backup(&self, dir: &Path, encrypted: bool) {
        fs::create_dir_all(dir).unwrap();
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
             <key>IsEncrypted</key><{encrypted}/></dict></plist>\n"
        );
        fs::write(dir.join("Manifest.plist"), plist).unwrap();
        fs::write(dir.join("Manifest.db"), b"SQLite format 3\0").unwrap();
    }
}

/// Starts and runs a job to the end; `hook` sees every event (and may cancel through the control).
fn run(
    ctx: RunContext,
    request: RunRequest,
    mut hook: impl FnMut(&RunEvent, &RunControl),
) -> (RunOutcome, Vec<RunEvent>) {
    let job = runner::start(request, ctx).unwrap();
    assert!(job.run_dir().is_dir(), "start creates the run folder");
    let control = job.control();
    let mut events = Vec::new();
    let outcome = job.run(&mut |event| {
        hook(&event, &control);
        events.push(event);
    });
    (outcome, events)
}

fn codes(reasons: &[Reason]) -> Vec<&str> {
    reasons.iter().map(|r| r.code.as_str()).collect()
}

fn phases(events: &[RunEvent]) -> Vec<RunPhase> {
    events
        .iter()
        .filter_map(|e| match e {
            RunEvent::Phase { phase } => Some(*phase),
            _ => None,
        })
        .collect()
}

fn log_lines(events: &[RunEvent]) -> Vec<String> {
    events
        .iter()
        .flat_map(|e| match e {
            RunEvent::Log { lines } => lines.clone(),
            _ => Vec::new(),
        })
        .collect()
}

fn stdio_tails(events: &[RunEvent], stream: StdStream) -> Vec<Vec<String>> {
    events
        .iter()
        .filter_map(|e| match e {
            RunEvent::StdioTail { stream: s, lines } if *s == stream => Some(lines.clone()),
            _ => None,
        })
        .collect()
}

fn run_dir(outcome: &RunOutcome) -> PathBuf {
    PathBuf::from(&outcome.summary.run_dir)
}

fn read_record(dir: &Path) -> RunRecord {
    parse_versioned(&fs::read(dir.join("run.json")).unwrap()).unwrap()
}

/// The checks every scenario gets: status, reasons and warnings; a finalized, read-only record
/// equal to the outcome; `report.sha256` exactly when `report/` exists, matching its recorded hash;
/// the event rules; no temp dir left behind.
fn assert_final(
    lab: &Lab,
    outcome: &RunOutcome,
    events: &[RunEvent],
    status: RunStatus,
    reasons: &[&str],
    warnings: &[&str],
) -> RunRecord {
    assert_eq!(outcome.write_error, None);
    let record = &outcome.record;
    let dir = run_dir(outcome);
    let context = format!("{:?} {:?}", record.status_reasons, record.warnings);
    assert_eq!(record.status, status, "{context}");
    assert_eq!(codes(&record.status_reasons), reasons, "{context}");
    assert_eq!(codes(&record.warnings), warnings, "{context}");
    // Finalized: read-only, as returned, with the end time.
    let file = dir.join("run.json");
    assert!(fs::metadata(&file).unwrap().permissions().readonly());
    assert_eq!(read_record(&dir), *record);
    assert!(record.ended_at.is_some() && record.duration_ms.is_some());
    assert_eq!(record.recovered_at, None);
    assert_ne!(record.input.hash.status, HashStatus::Pending);
    // The report manifest, when there is a report.
    let seal = &record.output.seal;
    let manifest = dir.join("report.sha256");
    if dir.join("report").is_dir() {
        assert_eq!(seal.status, SealStatus::Sealed, "{seal:?}");
        assert_eq!(seal.manifest.as_deref(), Some("report.sha256"));
        assert_eq!(
            seal.manifest_sha256.as_deref(),
            Some(hashing::sha256_file(&manifest).unwrap().as_str())
        );
        let lines = fs::read_to_string(&manifest).unwrap();
        assert_eq!(lines.lines().count() as u64, seal.file_count.unwrap());
        assert!(
            lines.lines().all(|l| l[66..].starts_with("report/")),
            "{lines}"
        );
        assert!(lines.contains("  report/_HTML/_Script_Logs/Screen_Output.html\n"));
    } else {
        assert_eq!(seal.status, SealStatus::SkippedNoOutput, "{seal:?}");
        assert!(!manifest.exists());
    }
    // Events: phases in lifecycle order, log batches of at most 500 lines, each stdio tail once
    // when LEAPP ran, and `finished` last with the record's outcome.
    let seen = phases(events);
    assert_eq!(seen.first(), Some(&RunPhase::Preparing));
    assert_eq!(seen.last(), Some(&RunPhase::Finalizing));
    assert!(seen.windows(2).all(|w| w[0] < w[1]), "{seen:?}");
    for event in events {
        if let RunEvent::Log { lines } = event {
            assert!(!lines.is_empty() && lines.len() <= 500, "{}", lines.len());
        }
    }
    let spawned = record.started_at.is_some();
    for stream in [StdStream::Stdout, StdStream::Stderr] {
        assert_eq!(
            stdio_tails(events, stream).len(),
            usize::from(spawned),
            "{stream}"
        );
    }
    match events.last() {
        Some(RunEvent::Finished {
            status: finished,
            reasons: finished_reasons,
            warnings: finished_warnings,
            summary,
        }) => {
            assert_eq!(*finished, status);
            assert_eq!(finished_reasons, &record.status_reasons);
            assert_eq!(finished_warnings, &record.warnings);
            assert_eq!(**summary, outcome.summary);
            assert_eq!(summary.status, status);
            assert_eq!(
                summary.report_available,
                dir.join("report").join("index.html").is_file()
            );
        }
        other => panic!("last event: {other:?}"),
    }
    lab.assert_no_temp_dirs();
    record.clone()
}

/// One §7.4 row with an `fs` input: runs `scenario` to the end and checks the outcome.
fn scenario_row(
    tool: ToolId,
    scenario: &str,
    status: RunStatus,
    reasons: &[&str],
    warnings: &[&str],
) -> (Lab, RunRecord, Vec<RunEvent>) {
    let lab = Lab::new();
    let (outcome, events) = run(
        lab.context(tool, scenario, &[]),
        lab.fs_request(tool),
        |_, _| {},
    );
    let record = assert_final(&lab, &outcome, &events, status, reasons, warnings);
    (lab, record, events)
}

// ---- CONTRACTS.md §7.4, one test per row ----

#[test]
fn success_ileapp() {
    let (lab, record, events) =
        scenario_row(ToolId::Ileapp, "success", RunStatus::Succeeded, &[], &[]);
    assert_eq!(
        phases(&events),
        [
            RunPhase::Preparing,
            RunPhase::Running,
            RunPhase::Analyzing,
            RunPhase::SealingReport,
            RunPhase::Finalizing
        ]
    );
    // The live log is plain text from Screen_Output.html (markup stripped).
    let lines = log_lines(&events);
    assert_eq!(lines.len(), 20, "{lines:?}");
    assert_eq!(
        lines[0],
        "Processing started. Please wait. This may take a few minutes..."
    );
    assert_eq!(lines[1], "iLEAPP v0.0.0-fake (fake-leapp test double)");
    // The stdout tail holds what fake-leapp buffered until its exit.
    let stdout = &stdio_tails(&events, StdStream::Stdout)[0];
    assert!(stdout.iter().any(|l| l.starts_with("Processing started")));
    // The record: dev override, the case's timezone, the full argv and the process.
    assert_eq!(record.tool.install_source, InstallSource::DevOverride);
    assert_eq!(
        record.tool.entry_verified_against,
        EntryVerifiedAgainst::None
    );
    assert_eq!(record.tool.version, "dev-override");
    assert_eq!(
        record.tool.entry_sha256,
        hashing::sha256_file(Path::new(FAKE_LEAPP)).unwrap()
    );
    assert_eq!(record.host, host());
    assert_eq!(record.options.timezone.as_deref(), Some("America/Chicago"));
    assert!(record.options.timezone_supported);
    assert!(!record.options.password_supplied);
    assert_eq!(record.modules.mode, ModuleMode::All);
    assert_eq!(record.modules.requested, Vec::<String>::new());
    assert_eq!(record.modules.always_run, ["last_build"]);
    assert_eq!(
        record.modules.resolved.len() as u32,
        record.modules.available_count
    );
    assert_eq!(record.input.hash.status, HashStatus::NotApplicable);
    assert_eq!(record.input.acquisition_id, None);
    let dir = lab.run_dirs().pop().unwrap();
    let cwd = dir.to_string_lossy().into_owned();
    assert_eq!(record.command.cwd, cwd);
    let argv = &record.command.argv;
    assert_eq!(
        argv[0],
        std::path::absolute(FAKE_LEAPP).unwrap().to_string_lossy()
    );
    assert_eq!(
        argv[1..],
        [
            "-t".to_owned(),
            "fs".to_owned(),
            "-i".to_owned(),
            lab.input.to_string_lossy().into_owned(),
            "-o".to_owned(),
            cwd.clone(),
            "--custom_output_folder".to_owned(),
            "report".to_owned(),
            "-d".to_owned(),
            dir.join("case.lcasedata").to_string_lossy().into_owned(),
            "-tz".to_owned(),
            "America/Chicago".to_owned(),
        ]
    );
    assert!(
        !dir.join("profile.ilprofile").exists(),
        "mode all has no profile"
    );
    let process = record.process.unwrap();
    assert_eq!((process.exit_code, process.signal), (Some(0), None));
    assert!(!process.cancel_requested && !process.escalated_to_kill);
    let result = record.leapp_result.unwrap();
    assert!(result.lava_data_found && result.index_html_found);
    assert_eq!(result.processing_status.as_deref(), Some("Complete"));
    assert!(record.started_at.is_some());
}

#[test]
fn success_aleapp_has_no_timezone() {
    let (_lab, record, _) = scenario_row(ToolId::Aleapp, "success", RunStatus::Succeeded, &[], &[]);
    assert_eq!(record.options.timezone, None);
    assert!(!record.options.timezone_supported);
    assert!(!record.command.argv.iter().any(|a| a == "-tz"));
    assert_eq!(record.modules.always_run, ["usagestats_version"]);
}

#[test]
fn artifact_error() {
    let (_lab, record, _) = scenario_row(
        ToolId::Ileapp,
        "artifact_error",
        RunStatus::CompletedWithErrors,
        &["modules_errored"],
        &["stderr_traceback"],
    );
    assert_eq!(record.leapp_result.unwrap().error_modules.len(), 2);
}

#[test]
fn invalid_input() {
    scenario_row(
        ToolId::Ileapp,
        "invalid_input",
        RunStatus::Failed,
        &["no_modules_ran", "index_html_missing"],
        &[],
    );
}

#[test]
fn early_exit() {
    scenario_row(
        ToolId::Ileapp,
        "early_exit",
        RunStatus::Failed,
        &["no_output_dir"],
        &[],
    );
}

#[test]
fn argparse_error() {
    let (_lab, record, events) = scenario_row(
        ToolId::Aleapp,
        "argparse_error",
        RunStatus::Failed,
        &["no_output_dir", "nonzero_exit"],
        &[],
    );
    assert_eq!(record.process.unwrap().exit_code, Some(2));
    let stderr = &stdio_tails(&events, StdStream::Stderr)[0];
    assert!(
        stderr.iter().any(|l| l.contains("error: argument")),
        "{stderr:?}"
    );
}

#[test]
fn crash() {
    scenario_row(
        ToolId::Ileapp,
        "crash",
        RunStatus::Failed,
        &["lava_data_missing", "index_html_missing", "nonzero_exit"],
        &["stderr_traceback"],
    );
}

#[test]
fn prompt_never_blocks() {
    // No controlling terminal and stdin null: the password prompt reads EOF and LEAPP exits.
    scenario_row(
        ToolId::Ileapp,
        "prompt",
        RunStatus::Failed,
        &["lava_data_missing", "index_html_missing", "nonzero_exit"],
        &["stderr_traceback"],
    );
}

/// Cancels once the first log batch arrived.
fn cancel_on_first_log(event: &RunEvent, control: &RunControl) {
    if matches!(event, RunEvent::Log { .. }) {
        control.cancel();
    }
}

#[test]
fn slow_with_cancel() {
    let lab = Lab::new();
    let (outcome, events) = run(
        lab.context(ToolId::Ileapp, "slow", &[]),
        lab.fs_request(ToolId::Ileapp),
        cancel_on_first_log,
    );
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        RunStatus::Cancelled,
        &["cancelled_by_user"],
        &[],
    );
    let process = record.process.unwrap();
    assert!(process.cancel_requested);
    assert!(!process.escalated_to_kill);
    // The partial output is kept, sealed and marked (D16).
    assert_eq!(record.output.seal.status, SealStatus::Sealed);
}

#[cfg(unix)]
#[test]
fn ignore_term_with_cancel_escalates_to_kill() {
    let lab = Lab::new();
    let (outcome, events) = run(
        lab.context(ToolId::Ileapp, "ignore_term", &[]),
        lab.fs_request(ToolId::Ileapp),
        cancel_on_first_log,
    );
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        RunStatus::Cancelled,
        &["cancelled_by_user"],
        &[],
    );
    let process = record.process.unwrap();
    assert!(process.cancel_requested);
    assert!(process.escalated_to_kill);
}

// ---- hashing and cancel ----

/// A file input of `len` bytes (sparse where the file system allows it), detected as `zip`.
fn file_input(lab: &Lab, len: u64) -> PathBuf {
    let path = lab.root.path().join("ev").join("image.zip");
    let file = fs::File::create(&path).unwrap();
    file.set_len(len).unwrap();
    path
}

#[test]
fn the_input_is_hashed_concurrently_with_leapp() {
    let lab = Lab::new();
    let input = file_input(&lab, 16 << 20);
    let mut request = lab.request(ToolId::Ileapp, &input, InputType::Zip);
    request.hash_input = true;
    let (outcome, events) = run(
        lab.context(ToolId::Ileapp, "success", &[]),
        request,
        |_, _| {},
    );
    let record = assert_final(&lab, &outcome, &events, RunStatus::Succeeded, &[], &[]);
    let hash = &record.input.hash;
    assert_eq!(hash.status, HashStatus::Completed);
    assert_eq!(
        hash.value.as_deref(),
        Some(hashing::sha256_file(&input).unwrap().as_str())
    );
    // Hashing (step 3) starts before LEAPP is spawned (step 4) and runs beside it.
    let (started, spawned) = (hash.started_at.unwrap(), record.started_at.unwrap());
    assert!(started <= spawned, "{started} {spawned}");
    assert!(hash.completed_at.is_some());
    let progress: Vec<(u64, u64)> = events
        .iter()
        .filter_map(|e| match e {
            RunEvent::HashProgress {
                bytes_done,
                bytes_total,
            } => Some((*bytes_done, *bytes_total)),
            _ => None,
        })
        .collect();
    assert_eq!(progress.last(), Some(&(16 << 20, 16 << 20)));
    assert_eq!(record.input.size_bytes, Some(16 << 20));
}

#[test]
fn a_cancel_after_the_exit_only_stops_hashing() {
    let lab = Lab::new();
    // Big enough that hashing outlasts fake-leapp's few lines.
    let input = file_input(&lab, 1 << 30);
    let mut request = lab.request(ToolId::Ileapp, &input, InputType::Zip);
    request.hash_input = true;
    let (outcome, events) = run(
        lab.context(ToolId::Ileapp, "success", &[("FAKE_LEAPP_LINES", "3")]),
        request,
        |event, control| {
            if matches!(
                event,
                RunEvent::Phase {
                    phase: RunPhase::HashingInput
                }
            ) {
                control.cancel();
            }
        },
    );
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        RunStatus::Succeeded,
        &[],
        &["input_hash_cancelled"],
    );
    assert!(phases(&events).contains(&RunPhase::HashingInput));
    assert_eq!(record.input.hash.status, HashStatus::Cancelled);
    assert_eq!(record.input.hash.value, None);
    let process = record.process.unwrap();
    assert!(!process.cancel_requested, "LEAPP had exited");
    assert_eq!(process.exit_code, Some(0));
}

#[test]
fn a_cancel_while_preparing_never_starts_leapp() {
    let lab = Lab::new();
    let input = file_input(&lab, 4096);
    let mut request = lab.request(ToolId::Ileapp, &input, InputType::Zip);
    request.hash_input = true;
    let (outcome, events) = run(
        lab.context(ToolId::Ileapp, "success", &[]),
        request,
        |event, control| {
            if matches!(
                event,
                RunEvent::Phase {
                    phase: RunPhase::Preparing
                }
            ) {
                control.cancel();
            }
        },
    );
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        RunStatus::Cancelled,
        &["cancelled_by_user"],
        &[],
    );
    assert_eq!(record.started_at, None);
    assert_eq!(record.process, None);
    assert_eq!(record.input.hash.status, HashStatus::Cancelled);
    assert!(log_lines(&events).is_empty());
}

// ---- events ----

#[test]
fn log_batches_have_at_most_500_lines() {
    let lab = Lab::new();
    let (outcome, events) = run(
        lab.context(
            ToolId::Aleapp,
            "success",
            &[
                ("FAKE_LEAPP_LINES", "1600"),
                ("FAKE_LEAPP_INTERVAL_MS", "0"),
            ],
        ),
        lab.fs_request(ToolId::Aleapp),
        |_, _| {},
    );
    assert_final(&lab, &outcome, &events, RunStatus::Succeeded, &[], &[]);
    assert_eq!(log_lines(&events).len(), 1600);
    let batches = events
        .iter()
        .filter(|e| matches!(e, RunEvent::Log { .. }))
        .count();
    assert!(batches >= 4, "{batches}");
}

// ---- modules, options and inputs ----

#[test]
fn custom_and_profile_modules_write_the_run_profile() {
    let lab = Lab::new();
    let mut request = lab.fs_request(ToolId::Ileapp);
    request.modules = ModuleSelection::Custom {
        modules: vec!["sms".to_owned(), "callHistory".to_owned(), "sms".to_owned()],
    };
    let (outcome, events) = run(
        lab.context(ToolId::Ileapp, "success", &[]),
        request,
        |_, _| {},
    );
    let record = assert_final(&lab, &outcome, &events, RunStatus::Succeeded, &[], &[]);
    assert_eq!(record.modules.mode, ModuleMode::Custom);
    assert_eq!(record.modules.resolved, ["sms", "callHistory"]);
    let dir = run_dir(&outcome);
    let profile: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.join("profile.ilprofile")).unwrap()).unwrap();
    assert_eq!(
        profile,
        serde_json::json!({"leapp": "ileapp", "format_version": 1,
                           "plugins": ["sms", "callHistory"]})
    );
    let argv = &record.command.argv;
    let at = argv.iter().position(|a| a == "-m").unwrap();
    assert_eq!(
        argv[at + 1],
        dir.join("profile.ilprofile").to_string_lossy()
    );
    // Only the selected modules (plus the always-run one) ran.
    let counts = record.leapp_result.unwrap().module_counts.unwrap();
    assert_eq!(
        counts.complete + counts.error + counts.no_files_found + counts.other,
        3
    );

    // A stored profile.
    let manifest = &manifest::embedded().unwrap().tools[&ToolId::Aleapp];
    let store = ProfileStore::new(
        lab.paths.profiles_dir(ToolId::Aleapp),
        ProfileFormat::from_manifest(ToolId::Aleapp, manifest),
    );
    let ctx = lab.context(ToolId::Aleapp, "success", &[]);
    store
        .save(
            "Triage",
            &["callLogs".to_owned()],
            &ctx.tool.modules.modules,
        )
        .unwrap();
    let mut request = lab.fs_request(ToolId::Aleapp);
    request.modules = ModuleSelection::Profile {
        profile_name: "Triage".to_owned(),
    };
    let (outcome, events) = run(ctx, request, |_, _| {});
    let record = assert_final(&lab, &outcome, &events, RunStatus::Succeeded, &[], &[]);
    assert_eq!(record.modules.mode, ModuleMode::Profile);
    assert_eq!(record.modules.profile_name.as_deref(), Some("Triage"));
    assert_eq!(record.modules.resolved, ["callLogs"]);
    assert!(run_dir(&outcome).join("profile.alprofile").is_file());
}

#[test]
fn an_explicit_timezone_and_a_keychain() {
    let lab = Lab::new();
    let keychain = lab.root.path().join("ev").join("keychain-2.db");
    fs::write(&keychain, b"keychain bytes").unwrap();
    let mut request = lab.fs_request(ToolId::Ileapp);
    request.timezone = Some("Europe/Berlin".to_owned());
    request.keychain_path = Some(keychain.to_string_lossy().into_owned());
    let (outcome, events) = run(
        lab.context(ToolId::Ileapp, "success", &[]),
        request,
        |_, _| {},
    );
    let record = assert_final(&lab, &outcome, &events, RunStatus::Succeeded, &[], &[]);
    assert_eq!(record.options.timezone.as_deref(), Some("Europe/Berlin"));
    assert_eq!(
        record.options.keychain_path.as_deref(),
        Some(keychain.to_string_lossy().as_ref())
    );
    assert_eq!(
        record.options.keychain_sha256.as_deref(),
        Some(hashing::sha256_file(&keychain).unwrap().as_str())
    );
    let argv = &record.command.argv;
    let at = argv.iter().position(|a| a == "--keychain").unwrap();
    assert_eq!(argv[at + 1], keychain.to_string_lossy());
}

#[test]
fn an_input_inside_the_cases_acquisitions_records_its_id() {
    let lab = Lab::new();
    let acq = lab.case.path.join("acquisitions").join(ACQ_ID);
    fs::create_dir_all(&acq).unwrap();
    let mut record = examples::acquisition_record();
    record.acq_id = ACQ_ID.to_owned();
    fs::write(
        acq.join("acquisition.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    let backup = acq.join("backup").join("00008101-000A1B2C3D4E001E");
    lab.itunes_backup(&backup, false);
    let request = lab.request(ToolId::Ileapp, &backup, InputType::Itunes);
    let (outcome, events) = run(
        lab.context(ToolId::Ileapp, "success", &[]),
        request,
        |_, _| {},
    );
    let record = assert_final(&lab, &outcome, &events, RunStatus::Succeeded, &[], &[]);
    assert_eq!(record.input.acquisition_id.as_deref(), Some(ACQ_ID));
    assert_eq!(record.input.itunes_encrypted, Some(false));
    assert_eq!(record.input.type_detected, Some(InputType::Itunes));
    assert_eq!(
        record.modules.always_run,
        ["itunes_backup_info", "itunes_backup_installed_applications"]
    );
}

#[test]
fn the_backup_password_never_leaks() {
    let lab = Lab::new();
    let backup = lab.root.path().join("ev").join("bk");
    lab.itunes_backup(&backup, true);
    let mut request = lab.request(ToolId::Ileapp, &backup, InputType::Itunes);
    request.itunes_password = Some(PASSWORD.to_owned());
    let ctx = lab.context(ToolId::Ileapp, "success", &[]);
    let job = runner::start(request, ctx).unwrap();
    assert!(!format!("{job:?}").contains(PASSWORD));
    let mut events = Vec::new();
    let outcome = job.run(&mut |event| events.push(event));
    let record = assert_final(&lab, &outcome, &events, RunStatus::Succeeded, &[], &[]);
    assert!(record.options.password_supplied);
    assert_eq!(record.input.itunes_encrypted, Some(true));
    let argv = &record.command.argv;
    let at = argv.iter().position(|a| a == "--itunes_password").unwrap();
    assert_eq!(argv[at + 1], "<redacted>");
    assert!(!format!("{events:?} {outcome:?}").contains(PASSWORD));
    // Nothing in the run folder holds it.
    let mut pending = vec![run_dir(&outcome)];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let bytes = fs::read(&path).unwrap();
                assert!(
                    !bytes
                        .windows(PASSWORD.len())
                        .any(|w| w == PASSWORD.as_bytes()),
                    "{} holds the password",
                    path.display()
                );
            }
        }
    }
}

// ---- prepare and spawn failures ----

#[test]
fn prepare_failed() {
    let lab = Lab::new();
    // `<app_cache>/tmp` is a file, so the run's temp dir cannot be created.
    fs::create_dir_all(&lab.paths.app_cache).unwrap();
    fs::write(lab.paths.temp_root(), "not a folder").unwrap();
    let input = file_input(&lab, 4096);
    let mut request = lab.request(ToolId::Ileapp, &input, InputType::Zip);
    request.hash_input = true;
    let (outcome, events) = run(
        lab.context(ToolId::Ileapp, "success", &[]),
        request,
        |_, _| {},
    );
    let record = outcome.record.clone();
    fs::remove_file(lab.paths.temp_root()).unwrap();
    let record_again = assert_final(
        &lab,
        &outcome,
        &events,
        RunStatus::Failed,
        &["prepare_failed"],
        &[],
    );
    assert_eq!(record, record_again);
    assert!(record.status_reasons[0].message.contains("temp dir"));
    assert_eq!(record.process, None);
    assert_eq!(record.started_at, None);
    assert_eq!(record.leapp_result, None);
    assert_eq!(record.input.hash.status, HashStatus::Cancelled);
    assert_eq!(phases(&events), [RunPhase::Preparing, RunPhase::Finalizing]);
}

#[test]
fn spawn_failed() {
    let lab = Lab::new();
    let mut ctx = lab.context(ToolId::Aleapp, "success", &[]);
    // Not an executable.
    let entry = lab.root.path().join("aleapp.txt");
    fs::write(&entry, "not a program").unwrap();
    ctx.tool.entry = entry;
    let (outcome, events) = run(ctx, lab.fs_request(ToolId::Aleapp), |_, _| {});
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        RunStatus::Failed,
        &["spawn_failed"],
        &[],
    );
    assert_eq!(record.process, None);
    assert_eq!(record.started_at, None);
    assert!(!phases(&events).contains(&RunPhase::Running));
}

#[test]
fn a_glibc_too_old_for_the_build_is_a_spawn_failure() {
    let (_lab, record, _) = scenario_row(
        ToolId::Ileapp,
        "glibc_too_old",
        RunStatus::Failed,
        &["spawn_failed"],
        &[],
    );
    let message = &record.status_reasons[0].message;
    assert!(
        message.contains("the pinned Linux iLEAPP build needs glibc 2.43 or newer"),
        "{message}"
    );
    assert!(
        message.contains("version `GLIBC_2.43' not found"),
        "{message}"
    );
    // It did run: the exit is recorded.
    assert_eq!(record.process.unwrap().exit_code, Some(255));
}

// ---- validation (step 1): nothing is created ----

fn start_error(lab: &Lab, ctx: RunContext, request: RunRequest) -> AppError {
    let error = runner::start(request, ctx).unwrap_err();
    assert!(lab.run_dirs().is_empty(), "{error:?} created a run folder");
    error
}

#[test]
fn inputs_overlapping_the_case_or_the_app_are_refused() {
    let lab = Lab::new();
    let ctx = || lab.context(ToolId::Ileapp, "success", &[]);
    // The case folder itself (the run folder would land inside the input).
    let request = lab.request(ToolId::Ileapp, &lab.case.path, InputType::Fs);
    assert_eq!(
        start_error(&lab, ctx(), request).code,
        ErrorCode::InputOverlapsCase
    );
    // A folder that holds the case.
    let request = lab.request(ToolId::Ileapp, lab.root.path(), InputType::Fs);
    assert_eq!(
        start_error(&lab, ctx(), request).code,
        ErrorCode::InputOverlapsCase
    );
    // Inside the app's folders.
    let inside_app = lab.paths.app_data.join("x");
    fs::create_dir_all(&inside_app).unwrap();
    fs::write(inside_app.join("f"), "x").unwrap();
    let request = lab.request(ToolId::Ileapp, &inside_app, InputType::Fs);
    assert_eq!(
        start_error(&lab, ctx(), request).code,
        ErrorCode::InputOverlapsCase
    );
    // Inside a run of a known case.
    let (outcome, _) = run(ctx(), lab.fs_request(ToolId::Ileapp), |_, _| {});
    let report = run_dir(&outcome).join("report");
    let request = lab.request(ToolId::Ileapp, &report, InputType::Fs);
    let before = lab.run_dirs().len();
    let error = runner::start(request, ctx()).unwrap_err();
    assert_eq!(error.code, ErrorCode::InputOverlapsCase);
    assert_eq!(lab.run_dirs().len(), before);
    // A keychain inside the case's runs.
    let mut request = lab.fs_request(ToolId::Ileapp);
    request.keychain_path = Some(
        run_dir(&outcome)
            .join("run.json")
            .to_string_lossy()
            .into_owned(),
    );
    let error = runner::start(request, ctx()).unwrap_err();
    assert_eq!(error.code, ErrorCode::InputOverlapsCase);
    assert_eq!(lab.run_dirs().len(), before);
}

#[test]
fn a_run_folder_path_that_is_too_long_is_refused() {
    let lab = Lab::new();
    let mut ctx = lab.context(ToolId::Ileapp, "success", &[]);
    // Shorter than any real run folder path.
    ctx.max_run_dir_chars = Some(40);
    let error = start_error(&lab, ctx, lab.fs_request(ToolId::Ileapp));
    assert_eq!(error.code, ErrorCode::PathTooLong);
    assert!(!lab.case.path.join("runs").exists());
    // At the limit it is accepted.
    let mut ctx = lab.context(ToolId::Ileapp, "success", &[]);
    let would_be = std::path::absolute(lab.case.path.join("runs").join("x".repeat(30))).unwrap();
    ctx.max_run_dir_chars = Some(would_be.to_string_lossy().encode_utf16().count());
    let (outcome, events) = run(ctx, lab.fs_request(ToolId::Ileapp), |_, _| {});
    assert_final(&lab, &outcome, &events, RunStatus::Succeeded, &[], &[]);
    // The Windows limit (ARCHITECTURE.md §6 step 1).
    assert_eq!(
        runner::default_run_dir_limit(),
        cfg!(windows).then_some(runner::WINDOWS_MAX_RUN_DIR_CHARS)
    );
}

#[test]
fn unknown_modules_are_refused() {
    let lab = Lab::new();
    let mut request = lab.fs_request(ToolId::Ileapp);
    request.modules = ModuleSelection::Custom {
        modules: vec!["callHistory".to_owned(), "noSuchModule".to_owned()],
    };
    let error = start_error(&lab, lab.context(ToolId::Ileapp, "success", &[]), request);
    assert_eq!(error.code, ErrorCode::UnknownModules);
    assert!(error.detail.unwrap().contains("noSuchModule"));
    let mut request = lab.fs_request(ToolId::Ileapp);
    request.modules = ModuleSelection::Profile {
        profile_name: "missing".to_owned(),
    };
    let error = start_error(&lab, lab.context(ToolId::Ileapp, "success", &[]), request);
    assert_eq!(error.code, ErrorCode::ProfileNotFound);
}

#[test]
fn an_encrypted_backup_needs_its_password() {
    let lab = Lab::new();
    let backup = lab.root.path().join("ev").join("bk");
    lab.itunes_backup(&backup, true);
    let request = lab.request(ToolId::Ileapp, &backup, InputType::Itunes);
    let error = start_error(&lab, lab.context(ToolId::Ileapp, "success", &[]), request);
    assert_eq!(error.code, ErrorCode::PasswordRequired);
    assert!(error.message.contains("is encrypted"), "{error:?}");
    // An empty password is none.
    let mut request = lab.request(ToolId::Ileapp, &backup, InputType::Itunes);
    request.itunes_password = Some(String::new());
    let error = start_error(&lab, lab.context(ToolId::Ileapp, "success", &[]), request);
    assert_eq!(error.code, ErrorCode::PasswordRequired);
}

/// A backup whose encryption cannot be read counts as encrypted (owner decision for K8): iLEAPP
/// would otherwise stop at its password prompt, which blocks on Windows (LEAPP-CLI.md Q5).
#[test]
fn a_backup_with_unknown_encryption_needs_a_password() {
    let lab = Lab::new();
    let ev = lab.root.path().join("ev");
    // No Manifest.plist (a Manifest.db alone still makes it a backup).
    let no_plist = ev.join("no-plist");
    fs::create_dir_all(&no_plist).unwrap();
    fs::write(no_plist.join("Manifest.db"), b"SQLite format 3\0").unwrap();
    // A Manifest.plist that is not a plist.
    let unreadable = ev.join("unreadable");
    fs::create_dir_all(&unreadable).unwrap();
    fs::write(unreadable.join("Manifest.plist"), b"not a plist").unwrap();
    // A Manifest.plist without IsEncrypted.
    let no_key = ev.join("no-key");
    fs::create_dir_all(&no_key).unwrap();
    fs::write(
        no_key.join("Manifest.plist"),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
         <key>Version</key><string>10.0</string></dict></plist>\n",
    )
    .unwrap();
    for backup in [&no_plist, &unreadable, &no_key] {
        let request = lab.request(ToolId::Ileapp, backup, InputType::Itunes);
        let error = start_error(&lab, lab.context(ToolId::Ileapp, "success", &[]), request);
        assert_eq!(
            error.code,
            ErrorCode::PasswordRequired,
            "{}",
            backup.display()
        );
        assert!(
            error.message.contains("encryption state could not be read"),
            "{error:?}"
        );
    }
    // With a password, it runs (and the record keeps the unknown state).
    let mut request = lab.request(ToolId::Ileapp, &no_key, InputType::Itunes);
    request.itunes_password = Some(PASSWORD.to_owned());
    let (outcome, events) = run(
        lab.context(ToolId::Ileapp, "success", &[]),
        request,
        |_, _| {},
    );
    let record = assert_final(&lab, &outcome, &events, RunStatus::Succeeded, &[], &[]);
    assert!(record.options.password_supplied);
    assert_eq!(record.input.itunes_encrypted, None);
    // Read as a plain folder, the same backup needs no password.
    let request = lab.request(ToolId::Ileapp, &no_key, InputType::Fs);
    let (outcome, events) = run(
        lab.context(ToolId::Ileapp, "success", &[]),
        request,
        |_, _| {},
    );
    let record = assert_final(&lab, &outcome, &events, RunStatus::Succeeded, &[], &[]);
    assert!(!record.options.password_supplied);
}

/// No password is needed for an unencrypted backup, nor for `-t itunes` on a folder that is not a
/// backup (iLEAPP rejects it without a prompt; E3 runs it on real LEAPP).
#[test]
fn an_unencrypted_backup_or_a_non_backup_needs_no_password() {
    let lab = Lab::new();
    let backup = lab.root.path().join("ev").join("plain");
    lab.itunes_backup(&backup, false);
    for input in [&backup, &lab.input] {
        let request = lab.request(ToolId::Ileapp, input, InputType::Itunes);
        let (outcome, events) = run(
            lab.context(ToolId::Ileapp, "success", &[]),
            request,
            |_, _| {},
        );
        let record = assert_final(&lab, &outcome, &events, RunStatus::Succeeded, &[], &[]);
        assert!(!record.options.password_supplied);
        assert_eq!(
            record.input.itunes_encrypted,
            (input == &backup).then_some(false)
        );
    }
}

#[test]
fn other_invalid_requests_are_refused() {
    let lab = Lab::new();
    let ctx = || lab.context(ToolId::Ileapp, "success", &[]);
    // A timezone the installed iLEAPP does not have.
    let mut request = lab.fs_request(ToolId::Ileapp);
    request.timezone = Some("Mars/Olympus_Mons".to_owned());
    assert_eq!(
        start_error(&lab, ctx(), request).code,
        ErrorCode::InvalidTimezone
    );
    // A folder cannot be read as a zip.
    let request = lab.request(ToolId::Ileapp, &lab.input, InputType::Zip);
    assert_eq!(
        start_error(&lab, ctx(), request).code,
        ErrorCode::InputTypeNotAllowed
    );
    // A missing input.
    let missing = lab.root.path().join("missing");
    let request = lab.request(ToolId::Ileapp, &missing, InputType::Fs);
    assert_eq!(
        start_error(&lab, ctx(), request).code,
        ErrorCode::InvalidInput
    );
    // aLEAPP has no keychain option.
    let keychain = lab.root.path().join("ev").join("k.db");
    fs::write(&keychain, "k").unwrap();
    let mut request = lab.fs_request(ToolId::Aleapp);
    request.keychain_path = Some(keychain.to_string_lossy().into_owned());
    assert_eq!(
        start_error(&lab, lab.context(ToolId::Aleapp, "success", &[]), request).code,
        ErrorCode::InvalidInput
    );
    // A keychain that is a folder.
    let mut request = lab.fs_request(ToolId::Ileapp);
    request.keychain_path = Some(lab.input.to_string_lossy().into_owned());
    assert_eq!(
        start_error(&lab, ctx(), request).code,
        ErrorCode::InvalidInput
    );
    // The context was prepared for the other tool.
    let request = lab.fs_request(ToolId::Aleapp);
    assert_eq!(start_error(&lab, ctx(), request).code, ErrorCode::Internal);
}
