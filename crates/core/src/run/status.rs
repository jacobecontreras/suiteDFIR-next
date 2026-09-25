//! Run status rules (CONTRACTS.md §7.3): the status, reasons and warnings of a finished run, and
//! the `leapp_result` they are derived from. The status never comes from the exit code alone
//! (ARCHITECTURE.md D8).

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path;

use serde::Deserialize;

use crate::contracts::{HashStatus, LeappResult, ModuleCounts, Reason, RunStatus, SealStatus};

/// LEAPP's result file inside `report/`.
pub const LAVA_FILE: &str = "_lava_data.lava";
/// LEAPP's report entry point inside `report/`.
pub const INDEX_HTML: &str = "index.html";

/// What a Python traceback starts with; its presence in stderr is a warning.
const TRACEBACK_MARKER: &[u8] = b"Traceback (most recent call last)";

// Lava `module_status` values.
const COMPLETE: &str = "Complete";
const ERROR: &str = "Error";
const NO_FILES_FOUND: &str = "No files found";

/// How the run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Preparing the run failed (lifecycle step 2); `detail` is the cause.
    PrepareFailed { detail: String },
    /// LEAPP could not be started (step 4).
    SpawnFailed { detail: String },
    /// LEAPP was started and its exit was observed (or it was cancelled first). An observed exit
    /// always comes with the analysis of `report/` (step 8), so the status never rests on the exit
    /// code alone (ARCHITECTURE.md D8).
    Exited {
        /// `None` when the process was killed by a signal.
        exit_code: Option<i32>,
        /// The signal that killed the process (Unix).
        signal: Option<i32>,
        /// A cancel was requested before the exit was observed.
        cancelled_before_exit: bool,
        /// [`analyze_report`] of the run's `report/`.
        report: ReportAnalysis,
    },
}

/// `_lava_data.lava` as far as the status rules need it (LEAPP-CLI.md §6). Unknown fields are
/// ignored; missing ones are `null`/empty.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct LavaData {
    #[serde(default)]
    pub processing_status: Option<String>,
    #[serde(default)]
    pub parser_info: Option<ParserInfo>,
    #[serde(default)]
    pub modules: Vec<LavaModule>,
}

/// `parser_info` in `_lava_data.lava`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct ParserInfo {
    #[serde(default)]
    pub leapp_version: Option<String>,
}

/// One entry of `modules` in `_lava_data.lava`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct LavaModule {
    #[serde(default)]
    pub module_name: Option<String>,
    #[serde(default)]
    pub module_status: Option<String>,
    #[serde(default)]
    pub artifact_name: Option<String>,
}

impl LavaModule {
    /// The name shown in reasons: `artifact_name`, else `module_name`.
    fn name(&self) -> &str {
        self.artifact_name
            .as_deref()
            .or(self.module_name.as_deref())
            .unwrap_or("(unnamed)")
    }
}

/// The state of `_lava_data.lava`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lava {
    Missing,
    /// Present but not readable as lava data; the text says why.
    Unparsable(String),
    Parsed(LavaData),
}

/// What `report/` contains (lifecycle step 8).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportAnalysis {
    pub dir_exists: bool,
    pub lava: Lava,
    pub index_html_found: bool,
}

/// Looks at a run's `report/` folder: whether it exists, `_lava_data.lava` and `index.html`.
pub fn analyze_report(report_dir: &Path) -> ReportAnalysis {
    let dir_exists = report_dir.is_dir();
    let lava = if dir_exists {
        match fs::read(report_dir.join(LAVA_FILE)) {
            Ok(bytes) => match serde_json::from_slice::<LavaData>(&bytes) {
                Ok(data) => Lava::Parsed(data),
                Err(e) => Lava::Unparsable(e.to_string()),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => Lava::Missing,
            Err(e) => Lava::Unparsable(e.to_string()),
        }
    } else {
        Lava::Missing
    };
    ReportAnalysis {
        dir_exists,
        lava,
        index_html_found: dir_exists && report_dir.join(INDEX_HTML).is_file(),
    }
}

/// Whether stderr contains a Python traceback. The file is streamed, so its size does not matter.
pub fn stderr_has_traceback(stderr_log: &Path) -> io::Result<bool> {
    let mut file = File::open(stderr_log)?;
    let mut buffer = vec![0u8; 64 * 1024];
    // Keep the end of the previous chunk so a marker split across reads is still found.
    let keep = TRACEBACK_MARKER.len() - 1;
    let mut filled = 0;
    loop {
        let n = match file.read(&mut buffer[filled..]) {
            Ok(0) => return Ok(false),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let end = filled + n;
        if buffer[..end]
            .windows(TRACEBACK_MARKER.len())
            .any(|w| w == TRACEBACK_MARKER)
        {
            return Ok(true);
        }
        let tail = end.min(keep);
        buffer.copy_within(end - tail..end, 0);
        filled = tail;
    }
}

/// The always-run set for the run's input type (CONTRACTS.md §5, §7.3). A lava entry is always-run
/// if its `artifact_name` (or `module_name` when `artifact_name` is absent) is in `names`, or its
/// `module_name` is in `module_names` (the `module_name`s of the always-run plugins).
#[derive(Clone, Copy, Debug, Default)]
pub struct AlwaysRun<'a> {
    pub names: &'a [String],
    pub module_names: &'a [String],
}

impl AlwaysRun<'_> {
    fn contains(&self, entry: &LavaModule) -> bool {
        let name = entry
            .artifact_name
            .as_deref()
            .or(entry.module_name.as_deref());
        let named = name.is_some_and(|name| self.names.iter().any(|n| n == name));
        let same_module = entry
            .module_name
            .as_deref()
            .is_some_and(|module| self.module_names.iter().any(|m| m == module));
        named || same_module
    }
}

/// The inputs of the status rules.
#[derive(Clone, Copy, Debug)]
pub struct StatusInput<'a> {
    pub outcome: &'a Outcome,
    pub always_run: AlwaysRun<'a>,
    /// See [`stderr_has_traceback`].
    pub stderr_traceback: bool,
    /// The final `input.hash.status`.
    pub input_hash: HashStatus,
    /// The final `output.seal.status`.
    pub seal: SealStatus,
    /// From sealing the report: `symlinks_in_report` and `unencodable_filename`
    /// ([`crate::hashing::SealOutcome::warnings`]).
    pub seal_warnings: &'a [Reason],
}

/// The result of the status rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub status: RunStatus,
    pub reasons: Vec<Reason>,
    pub warnings: Vec<Reason>,
    /// `None` when LEAPP never ran (prepare or spawn failed), so there was no output to analyze.
    pub leapp_result: Option<LeappResult>,
}

fn reason(code: &str, message: impl Into<String>) -> Reason {
    Reason {
        code: code.to_owned(),
        message: message.into(),
    }
}

/// Applies CONTRACTS.md §7.3 exactly.
pub fn evaluate(input: &StatusInput<'_>) -> Verdict {
    // 1. Short-circuits record only their reason; otherwise 2. and 3. evaluate every check.
    let (status, reasons, analysis) = match input.outcome {
        Outcome::PrepareFailed { detail } => (
            RunStatus::Failed,
            vec![reason(
                "prepare_failed",
                format!("Preparing the run failed: {detail}"),
            )],
            None,
        ),
        Outcome::SpawnFailed { detail } => (
            RunStatus::Failed,
            vec![reason(
                "spawn_failed",
                format!("LEAPP could not be started: {detail}"),
            )],
            None,
        ),
        Outcome::Exited {
            cancelled_before_exit: true,
            report,
            ..
        } => (
            RunStatus::Cancelled,
            vec![reason("cancelled_by_user", "The run was cancelled")],
            Some(report),
        ),
        Outcome::Exited {
            exit_code,
            signal,
            report,
            ..
        } => {
            let (status, reasons) = checks(*exit_code, *signal, report, input.always_run);
            (status, reasons, Some(report))
        }
    };
    Verdict {
        status,
        reasons,
        warnings: warnings(input, status, analysis.and_then(parsed_lava)),
        leapp_result: analysis.map(leapp_result),
    }
}

fn parsed_lava(analysis: &ReportAnalysis) -> Option<&LavaData> {
    match &analysis.lava {
        Lava::Parsed(data) => Some(data),
        Lava::Missing | Lava::Unparsable(_) => None,
    }
}

/// Steps 2 and 3: every check 3–8b, then the status. Checks 5 and 6 are skipped when the lava
/// data was not parsed.
fn checks(
    exit_code: Option<i32>,
    signal: Option<i32>,
    analysis: &ReportAnalysis,
    always_run: AlwaysRun<'_>,
) -> (RunStatus, Vec<Reason>) {
    let lava = parsed_lava(analysis);
    let mut reasons = Vec::new();
    if !analysis.dir_exists {
        reasons.push(reason(
            "no_output_dir",
            "LEAPP exited before creating output; see stdout",
        ));
    }
    if analysis.dir_exists && lava.is_none() {
        let detail = match &analysis.lava {
            Lava::Unparsable(why) => format!("report/{LAVA_FILE} is unparsable: {why}"),
            _ => format!("report/{LAVA_FILE} is missing"),
        };
        reasons.push(reason("lava_data_missing", detail));
    }
    if let Some(data) = lava {
        let status = data.processing_status.as_deref();
        if status != Some(COMPLETE) {
            reasons.push(reason(
                "processing_incomplete",
                format!(
                    "LEAPP's processing status is {}, not Complete",
                    status.map_or_else(|| "missing".to_owned(), |s| format!("{s:?}"))
                ),
            ));
        }
        if data.modules.iter().all(|m| always_run.contains(m)) {
            reasons.push(reason(
                "no_modules_ran",
                "No modules ran besides the always-run ones (typical for invalid input or a \
                 wrong backup password)",
            ));
        }
    }
    if analysis.dir_exists && !analysis.index_html_found {
        reasons.push(reason(
            "index_html_missing",
            format!("report/{INDEX_HTML} is missing"),
        ));
    }
    if let Some(code) = exit_code.filter(|code| *code != 0) {
        let note = if code == 2 {
            " (LEAPP rejected its arguments)"
        } else {
            ""
        };
        reasons.push(reason(
            "nonzero_exit",
            format!("LEAPP exited with code {code}{note}"),
        ));
    }
    if let Some(signal) = signal {
        reasons.push(reason(
            "killed_by_signal",
            format!("LEAPP was killed by signal {signal}"),
        ));
    }
    if !reasons.is_empty() {
        return (RunStatus::Failed, reasons);
    }
    let errored = error_modules(lava);
    if errored.is_empty() {
        (RunStatus::Succeeded, reasons)
    } else {
        let noun = if errored.len() == 1 {
            "module"
        } else {
            "modules"
        };
        reasons.push(reason(
            "modules_errored",
            format!(
                "{} {noun} reported Error: {}",
                errored.len(),
                errored.join(", ")
            ),
        ));
        (RunStatus::CompletedWithErrors, reasons)
    }
}

/// 4. Warnings (they never change the status).
fn warnings(input: &StatusInput<'_>, status: RunStatus, lava: Option<&LavaData>) -> Vec<Reason> {
    let mut warnings = Vec::new();
    if input.stderr_traceback {
        warnings.push(reason(
            "stderr_traceback",
            "stderr contains a Python traceback (see leapp.stderr.log)",
        ));
    }
    match input.input_hash {
        HashStatus::Failed => warnings.push(reason(
            "input_hash_failed",
            "Hashing the input failed; no input hash was recorded",
        )),
        // A cancel after the exit only stops input hashing. A cancel before it already made the
        // run `cancelled`, which says why the hash is missing; a run that failed to prepare or to
        // start never had an exit, and its hash was never run to the end.
        HashStatus::Cancelled
            if status != RunStatus::Cancelled
                && matches!(input.outcome, Outcome::Exited { .. }) =>
        {
            warnings.push(reason(
                "input_hash_cancelled",
                "A cancel after LEAPP finished stopped input hashing; no input hash was recorded",
            ))
        }
        _ => {}
    }
    if input.seal == SealStatus::Failed {
        warnings.push(reason(
            "seal_failed",
            "Hashing the report failed; report.sha256 was not written",
        ));
    }
    warnings.extend(input.seal_warnings.iter().cloned());
    let other: Vec<String> = lava
        .map(|data| {
            data.modules
                .iter()
                .filter(|m| is_other_status(m.module_status.as_deref()))
                .map(|m| {
                    format!(
                        "{} ({})",
                        m.name(),
                        m.module_status.as_deref().unwrap_or("no status")
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if !other.is_empty() {
        warnings.push(reason(
            "modules_other_status",
            format!(
                "{} lava module entries have another status: {}",
                other.len(),
                other.join(", ")
            ),
        ));
    }
    warnings
}

fn is_other_status(status: Option<&str>) -> bool {
    !matches!(status, Some(COMPLETE | ERROR | NO_FILES_FOUND))
}

fn error_modules(lava: Option<&LavaData>) -> Vec<String> {
    lava.map(|data| {
        data.modules
            .iter()
            .filter(|m| m.module_status.as_deref() == Some(ERROR))
            .map(|m| m.name().to_owned())
            .collect()
    })
    .unwrap_or_default()
}

/// `leapp_result` of the record.
fn leapp_result(analysis: &ReportAnalysis) -> LeappResult {
    let lava = parsed_lava(analysis);
    LeappResult {
        lava_data_found: !matches!(analysis.lava, Lava::Missing),
        processing_status: lava.and_then(|data| data.processing_status.clone()),
        leapp_version_reported: lava
            .and_then(|data| data.parser_info.as_ref())
            .and_then(|info| info.leapp_version.clone()),
        index_html_found: analysis.index_html_found,
        module_counts: lava.map(|data| {
            let mut counts = ModuleCounts {
                complete: 0,
                error: 0,
                no_files_found: 0,
                other: 0,
            };
            for module in &data.modules {
                let slot = match module.module_status.as_deref() {
                    Some(COMPLETE) => &mut counts.complete,
                    Some(ERROR) => &mut counts.error,
                    Some(NO_FILES_FOUND) => &mut counts.no_files_found,
                    _ => &mut counts.other,
                };
                *slot = slot.saturating_add(1);
            }
            counts
        }),
        error_modules: error_modules(lava),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    fn names(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn module(artifact: Option<&str>, module_name: &str, status: &str) -> serde_json::Value {
        let mut value = serde_json::json!({
            "module_name": module_name, "module_status": status, "file_count": 1
        });
        if let Some(artifact) = artifact {
            value["artifact_name"] = serde_json::json!(artifact);
        }
        value
    }

    /// iLEAPP's always-run set for a non-iTunes input.
    fn ileapp_always_run() -> (Vec<String>, Vec<String>) {
        (names(&["last_build"]), names(&["lastBuild"]))
    }

    /// A synthetic `report/` folder like the fake-leapp scenarios produce.
    struct Report {
        _tmp: tempfile::TempDir,
        dir: PathBuf,
    }

    impl Report {
        fn missing() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let dir = tmp.path().join("report");
            Self { _tmp: tmp, dir }
        }

        fn empty() -> Self {
            let report = Self::missing();
            fs::create_dir_all(report.dir.join("_HTML/_Script_Logs")).unwrap();
            report
        }

        fn with_lava(self, processing_status: &str, modules: &[serde_json::Value]) -> Self {
            let lava = serde_json::json!({
                "lava_schema_version": "1",
                "parser_info": {"leapp_name": "iLEAPP", "leapp_version": "2026.4.2",
                                "package": "Binary", "start_timestamp": 1727202605},
                "param_input": "/in", "param_output": "/out", "param_type": "fs",
                "param_profile": null,
                "processing_status": processing_status,
                "modules": modules,
            });
            fs::write(self.dir.join(LAVA_FILE), lava.to_string()).unwrap();
            self
        }

        fn with_raw_lava(self, text: &str) -> Self {
            fs::write(self.dir.join(LAVA_FILE), text).unwrap();
            self
        }

        fn with_index(self) -> Self {
            fs::write(self.dir.join(INDEX_HTML), "<html></html>").unwrap();
            self
        }

        fn analysis(&self) -> ReportAnalysis {
            analyze_report(&self.dir)
        }
    }

    /// The process side of a test case. `Case` adds the report analysis to an exit, as the runner
    /// does.
    #[derive(Clone)]
    enum Proc {
        PrepareFailed {
            detail: String,
        },
        SpawnFailed {
            detail: String,
        },
        Exited {
            exit_code: Option<i32>,
            signal: Option<i32>,
            cancelled_before_exit: bool,
        },
    }

    fn exited(code: i32) -> Proc {
        Proc::Exited {
            exit_code: Some(code),
            signal: None,
            cancelled_before_exit: false,
        }
    }

    struct Case {
        proc: Proc,
        analysis: Option<ReportAnalysis>,
        traceback: bool,
        input_hash: HashStatus,
        seal: SealStatus,
        seal_warnings: Vec<Reason>,
        always_run: (Vec<String>, Vec<String>),
    }

    impl Case {
        fn new(proc: Proc, report: Option<&Report>) -> Self {
            Self {
                proc,
                analysis: report.map(Report::analysis),
                traceback: false,
                input_hash: HashStatus::NotApplicable,
                seal: SealStatus::Sealed,
                seal_warnings: Vec::new(),
                always_run: ileapp_always_run(),
            }
        }

        fn outcome(&self) -> Outcome {
            match self.proc.clone() {
                Proc::PrepareFailed { detail } => Outcome::PrepareFailed { detail },
                Proc::SpawnFailed { detail } => Outcome::SpawnFailed { detail },
                Proc::Exited {
                    exit_code,
                    signal,
                    cancelled_before_exit,
                } => Outcome::Exited {
                    exit_code,
                    signal,
                    cancelled_before_exit,
                    report: self
                        .analysis
                        .clone()
                        .expect("an observed exit always has a report analysis"),
                },
            }
        }

        fn run(&self) -> Verdict {
            evaluate(&StatusInput {
                outcome: &self.outcome(),
                always_run: AlwaysRun {
                    names: &self.always_run.0,
                    module_names: &self.always_run.1,
                },
                stderr_traceback: self.traceback,
                input_hash: self.input_hash,
                seal: self.seal,
                seal_warnings: &self.seal_warnings,
            })
        }
    }

    fn codes(reasons: &[Reason]) -> Vec<&str> {
        reasons.iter().map(|r| r.code.as_str()).collect()
    }

    fn assert_verdict(verdict: &Verdict, status: RunStatus, reasons: &[&str], warnings: &[&str]) {
        assert_eq!(verdict.status, status, "{verdict:#?}");
        assert_eq!(codes(&verdict.reasons), reasons, "{verdict:#?}");
        assert_eq!(codes(&verdict.warnings), warnings, "{verdict:#?}");
    }

    fn good_modules() -> Vec<serde_json::Value> {
        vec![
            module(Some("last_build"), "lastBuild", "Complete"),
            module(Some("callHistory"), "callHistory", "Complete"),
            module(Some("sms"), "sms", "No files found"),
        ]
    }

    // ---- CONTRACTS.md §7.4, one test per row (synthetic outputs shaped like fake-leapp's) ----

    #[test]
    fn row_success() {
        let report = Report::empty()
            .with_lava("Complete", &good_modules())
            .with_index();
        let verdict = Case::new(exited(0), Some(&report)).run();
        assert_verdict(&verdict, RunStatus::Succeeded, &[], &[]);
        let result = verdict.leapp_result.unwrap();
        assert!(result.lava_data_found && result.index_html_found);
        assert_eq!(result.processing_status.as_deref(), Some("Complete"));
        assert_eq!(result.leapp_version_reported.as_deref(), Some("2026.4.2"));
        assert_eq!(
            result.module_counts,
            Some(ModuleCounts {
                complete: 2,
                error: 0,
                no_files_found: 1,
                other: 0
            })
        );
        assert_eq!(result.error_modules, Vec::<String>::new());
    }

    #[test]
    fn row_artifact_error() {
        let mut modules = good_modules();
        modules.push(module(Some("foo"), "fooModule", "Error"));
        modules.push(module(None, "bar", "Error"));
        let report = Report::empty().with_lava("Complete", &modules).with_index();
        let mut case = Case::new(exited(0), Some(&report));
        case.traceback = true;
        let verdict = case.run();
        assert_verdict(
            &verdict,
            RunStatus::CompletedWithErrors,
            &["modules_errored"],
            &["stderr_traceback"],
        );
        assert_eq!(
            verdict.reasons[0].message,
            "2 modules reported Error: foo, bar"
        );
        assert_eq!(verdict.leapp_result.unwrap().error_modules, ["foo", "bar"]);
    }

    #[test]
    fn row_invalid_input() {
        // "not a valid iTunes backup": Complete, only the always-run entry, no index.html.
        let report = Report::empty().with_lava(
            "Complete",
            &[module(Some("last_build"), "lastBuild", "Complete")],
        );
        let verdict = Case::new(exited(0), Some(&report)).run();
        assert_verdict(
            &verdict,
            RunStatus::Failed,
            &["no_modules_ran", "index_html_missing"],
            &[],
        );
        // Also with no module entries at all.
        let report = Report::empty().with_lava("Complete", &[]);
        let verdict = Case::new(exited(0), Some(&report)).run();
        assert_verdict(
            &verdict,
            RunStatus::Failed,
            &["no_modules_ran", "index_html_missing"],
            &[],
        );
    }

    #[test]
    fn row_early_exit() {
        let report = Report::missing();
        let verdict = Case::new(exited(0), Some(&report)).run();
        assert_verdict(&verdict, RunStatus::Failed, &["no_output_dir"], &[]);
        assert_eq!(
            verdict.reasons[0].message,
            "LEAPP exited before creating output; see stdout"
        );
        let result = verdict.leapp_result.unwrap();
        assert!(!result.lava_data_found && !result.index_html_found);
        assert_eq!(result.module_counts, None);
    }

    #[test]
    fn row_argparse_error() {
        let report = Report::missing();
        let verdict = Case::new(exited(2), Some(&report)).run();
        assert_verdict(
            &verdict,
            RunStatus::Failed,
            &["no_output_dir", "nonzero_exit"],
            &[],
        );
        assert_eq!(
            verdict.reasons[1].message,
            "LEAPP exited with code 2 (LEAPP rejected its arguments)"
        );
    }

    #[test]
    fn row_crash_and_prompt() {
        // Both: a traceback, exit 1, report/ with neither lava data nor index.html.
        for scenario in ["crash", "prompt"] {
            let report = Report::empty();
            let mut case = Case::new(exited(1), Some(&report));
            case.traceback = true;
            let verdict = case.run();
            assert_verdict(
                &verdict,
                RunStatus::Failed,
                &["lava_data_missing", "index_html_missing", "nonzero_exit"],
                &["stderr_traceback"],
            );
            assert_eq!(
                verdict.reasons[2].message, "LEAPP exited with code 1",
                "{scenario}"
            );
            let result = verdict.leapp_result.unwrap();
            assert!(!result.lava_data_found);
            assert_eq!(result.processing_status, None);
        }
    }

    #[test]
    fn rows_slow_and_ignore_term_with_cancel() {
        // The cancel arrived before the exit; the partial output is kept but ignored for status.
        for (exit_code, signal) in [(None, Some(15)), (None, Some(9)), (Some(1), None)] {
            let report = Report::empty();
            let verdict = Case::new(
                Proc::Exited {
                    exit_code,
                    signal,
                    cancelled_before_exit: true,
                },
                Some(&report),
            )
            .run();
            assert_verdict(&verdict, RunStatus::Cancelled, &["cancelled_by_user"], &[]);
        }
    }

    // ---- every rule ----

    #[test]
    fn short_circuits_record_only_their_reason() {
        // prepare_failed and spawn_failed, even with failing output around.
        let report = Report::missing();
        for (outcome, code) in [
            (
                Proc::PrepareFailed {
                    detail: "disk full".to_owned(),
                },
                "prepare_failed",
            ),
            (
                Proc::SpawnFailed {
                    detail: "permission denied".to_owned(),
                },
                "spawn_failed",
            ),
        ] {
            let verdict = Case::new(outcome.clone(), None).run();
            assert_verdict(&verdict, RunStatus::Failed, &[code], &[]);
            assert_eq!(verdict.leapp_result, None, "not analyzed");
            let verdict = Case::new(outcome, Some(&report)).run();
            assert_verdict(&verdict, RunStatus::Failed, &[code], &[]);
        }
        let verdict = Case::new(
            Proc::PrepareFailed {
                detail: "disk full".to_owned(),
            },
            None,
        )
        .run();
        assert_eq!(
            verdict.reasons[0].message,
            "Preparing the run failed: disk full"
        );
        // A cancel before exit wins over a nonzero exit and every failing check.
        let verdict = Case::new(
            Proc::Exited {
                exit_code: Some(2),
                signal: None,
                cancelled_before_exit: true,
            },
            Some(&report),
        )
        .run();
        assert_verdict(&verdict, RunStatus::Cancelled, &["cancelled_by_user"], &[]);
    }

    #[test]
    fn check_4_lava_missing_or_unparsable() {
        for report in [
            Report::empty().with_index(),
            Report::empty()
                .with_raw_lava("{\"modules\": [")
                .with_index(),
            Report::empty()
                .with_raw_lava("{\"modules\": 5}")
                .with_index(),
        ] {
            let verdict = Case::new(exited(0), Some(&report)).run();
            assert_verdict(&verdict, RunStatus::Failed, &["lava_data_missing"], &[]);
            // Checks 5 and 6 are skipped: their input is unavailable.
            let result = verdict.leapp_result.unwrap();
            assert_eq!(result.module_counts, None);
            assert_eq!(result.processing_status, None);
            assert_eq!(result.leapp_version_reported, None);
        }
        let unparsable = Report::empty().with_raw_lava("not json").with_index();
        let verdict = Case::new(exited(0), Some(&unparsable)).run();
        assert!(verdict.leapp_result.unwrap().lava_data_found);
        assert!(verdict.reasons[0].message.contains("unparsable"));
    }

    #[test]
    fn check_5_processing_incomplete() {
        for status in ["Processing", "complete", ""] {
            let report = Report::empty()
                .with_lava(status, &good_modules())
                .with_index();
            let verdict = Case::new(exited(0), Some(&report)).run();
            assert_verdict(&verdict, RunStatus::Failed, &["processing_incomplete"], &[]);
        }
        // A lava file without processing_status is parsed; the status is missing.
        let report = Report::empty()
            .with_raw_lava(&serde_json::json!({"modules": good_modules()}).to_string())
            .with_index();
        let verdict = Case::new(exited(0), Some(&report)).run();
        assert_verdict(&verdict, RunStatus::Failed, &["processing_incomplete"], &[]);
        assert_eq!(
            verdict.reasons[0].message,
            "LEAPP's processing status is missing, not Complete"
        );
    }

    #[test]
    fn check_6_always_run_matching() {
        let always = (names(&["itunes_backup_info"]), names(&["iTunesBackupInfo"]));
        let only = |entries: Vec<serde_json::Value>| {
            let report = Report::empty().with_lava("Complete", &entries).with_index();
            let mut case = Case::new(exited(0), Some(&report));
            case.always_run = always.clone();
            case.run()
        };
        // By artifact_name.
        let verdict = only(vec![module(
            Some("itunes_backup_info"),
            "other",
            "Complete",
        )]);
        assert_verdict(&verdict, RunStatus::Failed, &["no_modules_ran"], &[]);
        // By module_name when artifact_name is absent.
        let verdict = only(vec![module(None, "itunes_backup_info", "Complete")]);
        assert_verdict(&verdict, RunStatus::Failed, &["no_modules_ran"], &[]);
        // By the module_name of an always-run plugin, whatever the artifact name.
        let verdict = only(vec![module(
            Some("itunes_backup_installed_applications"),
            "iTunesBackupInfo",
            "Complete",
        )]);
        assert_verdict(&verdict, RunStatus::Failed, &["no_modules_ran"], &[]);
        // An artifact_name that is not always-run counts, even if module_name matches a name.
        let verdict = only(vec![module(Some("sms"), "itunes_backup_info", "Complete")]);
        assert_verdict(&verdict, RunStatus::Succeeded, &[], &[]);
        // Any other entry counts, whatever its status.
        let verdict = only(vec![
            module(Some("itunes_backup_info"), "iTunesBackupInfo", "Complete"),
            module(Some("sms"), "sms", "No files found"),
        ]);
        assert_verdict(&verdict, RunStatus::Succeeded, &[], &[]);
        // aLEAPP: the always-run plugins share module_name usagestatsVersion.
        let report = Report::empty()
            .with_lava(
                "Complete",
                &[module(
                    Some("usagestats_version"),
                    "usagestatsVersion",
                    "Complete",
                )],
            )
            .with_index();
        let mut case = Case::new(exited(0), Some(&report));
        case.always_run = (names(&[]), names(&["usagestatsVersion"]));
        assert_verdict(&case.run(), RunStatus::Failed, &["no_modules_ran"], &[]);
    }

    #[test]
    fn check_7_index_html_missing() {
        let report = Report::empty().with_lava("Complete", &good_modules());
        let verdict = Case::new(exited(0), Some(&report)).run();
        assert_verdict(&verdict, RunStatus::Failed, &["index_html_missing"], &[]);
        assert!(!verdict.leapp_result.unwrap().index_html_found);
    }

    #[test]
    fn checks_8_and_8b_exit_code_and_signal() {
        let report = Report::empty()
            .with_lava("Complete", &good_modules())
            .with_index();
        let verdict = Case::new(exited(3), Some(&report)).run();
        assert_verdict(&verdict, RunStatus::Failed, &["nonzero_exit"], &[]);
        assert_eq!(verdict.reasons[0].message, "LEAPP exited with code 3");
        let verdict = Case::new(exited(-1), Some(&report)).run();
        assert_verdict(&verdict, RunStatus::Failed, &["nonzero_exit"], &[]);
        let killed = Proc::Exited {
            exit_code: None,
            signal: Some(9),
            cancelled_before_exit: false,
        };
        let verdict = Case::new(killed, Some(&report)).run();
        assert_verdict(&verdict, RunStatus::Failed, &["killed_by_signal"], &[]);
        assert_eq!(verdict.reasons[0].message, "LEAPP was killed by signal 9");
    }

    #[test]
    fn exit_zero_alone_never_succeeds() {
        // D8: an observed exit always carries the report analysis, and exit 0 without a complete
        // report is a failure.
        for report in [
            Report::missing(),
            Report::empty(),
            Report::empty().with_index(),
            Report::empty().with_lava("Complete", &[]).with_index(),
        ] {
            let verdict = Case::new(exited(0), Some(&report)).run();
            assert_eq!(verdict.status, RunStatus::Failed, "{verdict:#?}");
            assert!(verdict.leapp_result.is_some(), "the result is recorded");
        }
    }

    #[test]
    fn every_matching_check_is_recorded_in_order() {
        let report = Report::empty().with_lava("Aborted", &[]);
        let killed = Proc::Exited {
            exit_code: Some(1),
            signal: Some(6),
            cancelled_before_exit: false,
        };
        let verdict = Case::new(killed, Some(&report)).run();
        assert_verdict(
            &verdict,
            RunStatus::Failed,
            &[
                "processing_incomplete",
                "no_modules_ran",
                "index_html_missing",
                "nonzero_exit",
                "killed_by_signal",
            ],
            &[],
        );
    }

    #[test]
    fn failed_checks_hide_module_errors() {
        let modules = vec![module(Some("foo"), "foo", "Error")];
        let report = Report::empty().with_lava("Complete", &modules);
        let verdict = Case::new(exited(0), Some(&report)).run();
        assert_verdict(&verdict, RunStatus::Failed, &["index_html_missing"], &[]);
        // The record still lists them.
        assert_eq!(verdict.leapp_result.unwrap().error_modules, ["foo"]);
    }

    #[test]
    fn a_single_errored_module() {
        let mut modules = good_modules();
        modules.push(module(Some("last_build"), "lastBuild", "Error"));
        let report = Report::empty().with_lava("Complete", &modules).with_index();
        let verdict = Case::new(exited(0), Some(&report)).run();
        assert_verdict(
            &verdict,
            RunStatus::CompletedWithErrors,
            &["modules_errored"],
            &[],
        );
        assert_eq!(
            verdict.reasons[0].message,
            "1 module reported Error: last_build"
        );
    }

    #[test]
    fn warnings_never_change_the_status() {
        let mut modules = good_modules();
        modules.push(module(Some("odd"), "odd", "Skipped"));
        modules.push(serde_json::json!({"module_name": "nostatus"}));
        let report = Report::empty().with_lava("Complete", &modules).with_index();
        let mut case = Case::new(exited(0), Some(&report));
        case.traceback = true;
        case.input_hash = HashStatus::Failed;
        case.seal = SealStatus::Failed;
        case.seal_warnings = vec![
            reason("symlinks_in_report", "2 symlinks"),
            reason("unencodable_filename", "1 name"),
        ];
        let verdict = case.run();
        assert_verdict(
            &verdict,
            RunStatus::Succeeded,
            &[],
            &[
                "stderr_traceback",
                "input_hash_failed",
                "seal_failed",
                "symlinks_in_report",
                "unencodable_filename",
                "modules_other_status",
            ],
        );
        assert_eq!(
            verdict.warnings[5].message,
            "2 lava module entries have another status: odd (Skipped), nostatus (no status)"
        );
        assert_eq!(
            verdict.leapp_result.unwrap().module_counts.unwrap().other,
            2
        );
    }

    #[test]
    fn input_hash_cancelled_warning() {
        let report = Report::empty()
            .with_lava("Complete", &good_modules())
            .with_index();
        // A cancel after the exit: the run result stands, hashing was stopped.
        let mut case = Case::new(exited(0), Some(&report));
        case.input_hash = HashStatus::Cancelled;
        assert_verdict(
            &case.run(),
            RunStatus::Succeeded,
            &[],
            &["input_hash_cancelled"],
        );
        // A cancel before the exit: the run is cancelled, which already explains the hash.
        case.proc = Proc::Exited {
            exit_code: None,
            signal: Some(15),
            cancelled_before_exit: true,
        };
        assert_verdict(
            &case.run(),
            RunStatus::Cancelled,
            &["cancelled_by_user"],
            &[],
        );
        // A run that failed to prepare or to start had no exit: its unfinished hash is no warning.
        for proc in [
            Proc::PrepareFailed {
                detail: "x".to_owned(),
            },
            Proc::SpawnFailed {
                detail: "x".to_owned(),
            },
        ] {
            case.proc = proc;
            let verdict = case.run();
            assert_eq!(verdict.status, RunStatus::Failed);
            assert_eq!(codes(&verdict.warnings), Vec::<&str>::new());
        }
        // Other hash statuses are not warnings.
        for status in [
            HashStatus::Completed,
            HashStatus::NotRequested,
            HashStatus::NotApplicable,
        ] {
            case.proc = exited(0);
            case.input_hash = status;
            assert_verdict(&case.run(), RunStatus::Succeeded, &[], &[]);
        }
    }

    #[test]
    fn warnings_are_kept_on_short_circuits() {
        let mut case = Case::new(
            Proc::SpawnFailed {
                detail: "x".to_owned(),
            },
            None,
        );
        case.seal = SealStatus::SkippedNoOutput;
        case.input_hash = HashStatus::Failed;
        assert_verdict(
            &case.run(),
            RunStatus::Failed,
            &["spawn_failed"],
            &["input_hash_failed"],
        );
    }

    #[test]
    fn traceback_detection_streams() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leapp.stderr.log");
        fs::write(&path, "").unwrap();
        assert!(!stderr_has_traceback(&path).unwrap());
        fs::write(&path, "warning: something\n").unwrap();
        assert!(!stderr_has_traceback(&path).unwrap());
        fs::write(
            &path,
            "x\nTraceback (most recent call last):\n  File \"a.py\"\nValueError\n",
        )
        .unwrap();
        assert!(stderr_has_traceback(&path).unwrap());
        // The marker straddles the 64 KiB read boundary.
        let mut big = vec![b'.'; 64 * 1024 - 10];
        big.extend_from_slice(TRACEBACK_MARKER);
        fs::write(&path, &big).unwrap();
        assert!(stderr_has_traceback(&path).unwrap());
        // A near miss at the boundary is not a match.
        let mut near = vec![b'.'; 64 * 1024 - 10];
        near.extend_from_slice(b"Traceback (most recent call first)");
        near.extend(vec![b'.'; 200_000]);
        fs::write(&path, &near).unwrap();
        assert!(!stderr_has_traceback(&path).unwrap());
        assert!(stderr_has_traceback(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn lava_parsing_ignores_unknown_fields() {
        let text = r#"{"lava_schema_version": "2", "processing_status": "Complete",
            "extra": {"x": 1},
            "modules": [{"module_name": "a", "module_status": "Complete", "future": true}]}"#;
        let data: LavaData = serde_json::from_str(text).unwrap();
        assert_eq!(data.processing_status.as_deref(), Some("Complete"));
        assert_eq!(data.parser_info, None);
        assert_eq!(data.modules.len(), 1);
        assert_eq!(data.modules[0].artifact_name, None);
    }
}
