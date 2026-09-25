//! Process-tree tests with fake-leapp (ROADMAP B2, B3; DEVELOPMENT.md §4.8): spawn, cancel,
//! timeout, temp dirs and log streaming, on every OS. Timing bounds are the spec's, measured with
//! deadlines rather than fixed sleeps, so they hold on a loaded machine.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use suitedfir_core::process::{self, ExitInfo, Handle, ProcessWatch, SpawnSpec};
use suitedfir_core::tail::{self, ScreenOutputTail};

const FAKE_LEAPP: &str = env!("CARGO_BIN_EXE_fake-leapp");
/// Cancel → whole tree gone (ROADMAP B2: "both PIDs dead within 12 s").
const CANCEL_BOUND: Duration = Duration::from_secs(12);
/// How long to wait for fake-leapp to start both processes.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
/// Temp dirs are named by job id (CONTRACTS.md §7.1).
const JOB_ID: &str = "20260925-090000Z-ileapp-0a1b2c";

/// A fake run: an `fs` input with one file, an output folder and an app cache, all in a temp dir.
struct Fixture {
    _dir: tempfile::TempDir,
    input: PathBuf,
    out: PathBuf,
    cache: PathBuf,
    pidfile: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("in");
        let out = dir.path().join("out");
        fs::create_dir(&input).unwrap();
        fs::create_dir(&out).unwrap();
        fs::write(input.join("evidence.txt"), "evidence").unwrap();
        Self {
            cache: dir.path().join("cache"),
            pidfile: dir.path().join("pids"),
            input,
            out,
            _dir: dir,
        }
    }

    /// The suiteDFIR-style invocation of fake-leapp with a scenario and extra variables.
    fn spec(&self, scenario: &str, vars: &[(&str, &str)]) -> SpawnSpec {
        let mut spec = SpawnSpec::new(
            FAKE_LEAPP,
            &self.out,
            self.out.join("leapp.stdout.log"),
            self.out.join("leapp.stderr.log"),
        );
        spec.args = [
            "-t".into(),
            "fs".into(),
            "-i".into(),
            self.input.clone().into_os_string(),
            "-o".into(),
            self.out.clone().into_os_string(),
            "--custom_output_folder".into(),
            "report".into(),
            "-tz".into(),
            "UTC".into(),
        ]
        .into();
        spec.env = vec![
            ("FAKE_LEAPP_SCENARIO".into(), scenario.into()),
            ("FAKE_LEAPP_PIDFILE".into(), self.pidfile.clone().into()),
        ];
        spec.env.extend(
            vars.iter()
                .map(|(name, value)| (OsString::from(name), OsString::from(value))),
        );
        spec
    }

    fn report(&self) -> PathBuf {
        self.out.join("report")
    }

    fn screen_output(&self) -> PathBuf {
        self.report()
            .join("_HTML")
            .join("_Script_Logs")
            .join("Screen_Output.html")
    }

    fn stdout(&self) -> String {
        fs::read_to_string(self.out.join("leapp.stdout.log")).unwrap()
    }

    fn stderr(&self) -> String {
        fs::read_to_string(self.out.join("leapp.stderr.log")).unwrap()
    }

    /// Both processes, from `FAKE_LEAPP_PIDFILE`, watched from the moment their pids are known.
    fn wait_for_tree(&self) -> Tree {
        let mut pids = None;
        let found = poll_until(STARTUP_TIMEOUT, || {
            pids = fs::read_to_string(&self.pidfile)
                .ok()
                .and_then(|text| parse_pids(&text));
            pids.is_some()
        });
        assert!(found, "fake-leapp did not write {}", self.pidfile.display());
        Tree::watch(pids.unwrap())
    }

    /// Like `wait_for_tree`, but `None` if the tree ended before it wrote its pids (a timeout
    /// that stopped it during a very slow first start, such as a virus scan of the new binary).
    fn wait_for_tree_unless_done(&self, handle: &Handle) -> Option<Tree> {
        let mut pids = None;
        poll_until(STARTUP_TIMEOUT, || {
            pids = fs::read_to_string(&self.pidfile)
                .ok()
                .and_then(|text| parse_pids(&text));
            pids.is_some() || matches!(handle.wait_timeout(Duration::ZERO), Ok(Some(_)))
        });
        pids.map(Tree::watch)
    }
}

/// fake-leapp's bootloader and worker. The watches hold on to these very processes (on Windows a
/// pid is reused quickly, so a later lookup by pid could find another process).
struct Tree {
    parent: u32,
    worker: u32,
    watches: [ProcessWatch; 2],
}

impl Tree {
    fn watch((parent, worker): (u32, u32)) -> Self {
        Self {
            parent,
            worker,
            watches: [ProcessWatch::open(parent), ProcessWatch::open(worker)],
        }
    }

    /// Asserts that both processes are gone by `deadline`. Polls: on Windows a terminated process
    /// can take a moment to finish exiting after its job reports no active processes.
    fn assert_gone_by(&self, deadline: Instant) {
        let left = deadline.saturating_duration_since(Instant::now());
        poll_until(left, || self.watches.iter().all(|w| !w.is_alive()));
        let [parent, worker] = &self.watches;
        assert!(
            !parent.is_alive(),
            "the bootloader {} survived",
            self.parent
        );
        assert!(!worker.is_alive(), "the worker {} survived", self.worker);
    }
}

fn parse_pids(text: &str) -> Option<(u32, u32)> {
    let pid = |role: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(role)?.trim().parse().ok())
    };
    Some((pid("parent ")?, pid("worker ")?))
}

fn poll_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Waits until the tool has created its runtime dir in the job's temp dir, which proves that
/// `TMPDIR`/`TEMP`/`TMP` reach it.
fn wait_for_runtime_dir(temp: &Path, parent: u32) -> PathBuf {
    let runtime = temp.join(format!("_MEIfake{parent}"));
    assert!(
        poll_until(STARTUP_TIMEOUT, || runtime.is_dir()),
        "{} was not created",
        runtime.display()
    );
    runtime
}

/// Spawns, cancels once both processes run, and returns the exit info, the tree, when the cancel
/// was requested and how long it took until `wait` returned.
fn spawn_and_cancel(
    fixture: &Fixture,
    spec: SpawnSpec,
    temp: &Path,
) -> (ExitInfo, Tree, Instant, Duration) {
    let handle = process::spawn(spec).unwrap();
    let tree = fixture.wait_for_tree();
    assert_eq!(tree.parent, handle.pid());
    wait_for_runtime_dir(temp, tree.parent);
    let cancelled = Instant::now();
    handle.cancel();
    let exit = handle.wait().unwrap();
    (exit, tree, cancelled, cancelled.elapsed())
}

#[test]
fn handle_is_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<Handle>();
}

#[test]
fn success_exits_0_and_captures_output() {
    let fixture = Fixture::new();
    let temp = process::create_temp_dir(&fixture.cache, JOB_ID).unwrap();
    let mut spec = fixture.spec(
        "success",
        &[("FAKE_LEAPP_LINES", "5"), ("FAKE_LEAPP_INTERVAL_MS", "200")],
    );
    spec.temp_dir = Some(temp.clone());
    let chunks = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&chunks);
    spec.on_output = Some(Box::new(move |stream, chunk: &[u8]| {
        sink.lock().unwrap().push((stream, chunk.to_vec()));
    }));

    let handle = process::spawn(spec).unwrap();
    let tree = fixture.wait_for_tree();
    let exit = handle.wait().unwrap();
    assert_eq!(exit.exit_code, Some(0));
    assert_eq!(exit.signal, None);
    assert!(!exit.cancel_requested && !exit.timed_out && !exit.escalated_to_kill);
    assert_eq!(exit.output_error, None);

    // stdout is written (by the tool, at exit) to the log file and passed to the callback.
    let stdout = fixture.stdout();
    assert_eq!(stdout.lines().count(), 5, "{stdout}");
    assert!(stdout.starts_with("Processing started."));
    assert_eq!(fixture.stderr(), "");
    let streamed: Vec<u8> = chunks
        .lock()
        .unwrap()
        .iter()
        .filter(|(stream, _)| stream.as_str() == "stdout")
        .flat_map(|(_, chunk)| chunk.clone())
        .collect();
    assert_eq!(streamed, stdout.as_bytes());

    assert!(fixture.report().join("_lava_data.lava").is_file());
    assert!(fixture.report().join("index.html").is_file());
    tree.assert_gone_by(Instant::now() + CANCEL_BOUND);
    // A graceful exit removed the tool's runtime dir; the job's temp dir goes with remove_temp_dir.
    assert_eq!(fs::read_dir(&temp).unwrap().count(), 0);
    process::remove_temp_dir(&fixture.cache, JOB_ID).unwrap();
    assert!(!temp.exists());
}

#[test]
fn slow_cancel_stops_both_processes_without_escalation() {
    let fixture = Fixture::new();
    let temp = process::create_temp_dir(&fixture.cache, JOB_ID).unwrap();
    let mut spec = fixture.spec("slow", &[]);
    spec.temp_dir = Some(temp.clone());

    let (exit, tree, cancelled, took) = spawn_and_cancel(&fixture, spec, &temp);
    assert!(took < CANCEL_BOUND, "cancel took {took:?}");
    tree.assert_gone_by(cancelled + CANCEL_BOUND);
    assert!(exit.cancel_requested);
    assert!(!exit.timed_out);
    assert!(!exit.escalated_to_kill);
    assert_eq!(exit.output_error, None);
    if cfg!(unix) {
        // SIGTERM: the bootloader forwards it, waits for the worker and dies from it too.
        assert_eq!((exit.exit_code, exit.signal), (None, Some(15)));
    } else {
        assert_eq!((exit.exit_code, exit.signal), (Some(1), None));
    }
    // The worker was stopped, so its buffered stdout never arrived (LEAPP-CLI.md Q1).
    assert_eq!(fixture.stdout(), "");

    // On Windows the terminated tool leaves its runtime dir behind; removal copes with both.
    process::remove_temp_dir(&fixture.cache, JOB_ID).unwrap();
    assert!(!temp.exists());
}

#[test]
fn timeout_stops_the_tree() {
    let fixture = Fixture::new();
    let mut spec = fixture.spec("slow", &[]);
    spec.timeout = Some(Duration::from_secs(5));
    let handle = process::spawn(spec).unwrap();
    let tree = fixture.wait_for_tree_unless_done(&handle);
    let exit = handle.wait().unwrap();
    assert!(exit.timed_out);
    assert!(!exit.cancel_requested);
    assert!(!exit.escalated_to_kill);
    match tree {
        Some(tree) => tree.assert_gone_by(Instant::now() + CANCEL_BOUND),
        None => eprintln!("the timeout stopped fake-leapp before it wrote its pids (slow start)"),
    }
}

/// A timeout the clock cannot represent means no timeout (it used to overflow `Instant`).
#[test]
fn an_unrepresentable_timeout_means_none() {
    let fixture = Fixture::new();
    let mut spec = fixture.spec("slow", &[]);
    spec.timeout = Some(Duration::MAX);
    let handle = process::spawn(spec).unwrap();
    let tree = fixture.wait_for_tree();
    assert_eq!(
        handle.wait_timeout(Duration::from_millis(200)).unwrap(),
        None
    );
    let cancelled = Instant::now();
    handle.cancel();
    let exit = handle.wait().unwrap();
    assert!(exit.cancel_requested);
    assert!(!exit.timed_out);
    tree.assert_gone_by(cancelled + CANCEL_BOUND);
    assert_eq!(handle.wait_timeout(Duration::MAX).unwrap(), Some(exit));
}

/// A kill grace the clock cannot represent means no escalation (it used to overflow `Instant`).
#[test]
fn an_unrepresentable_kill_grace_means_no_escalation() {
    let fixture = Fixture::new();
    let mut spec = fixture.spec("slow", &[]);
    spec.kill_grace = Duration::MAX;
    let handle = process::spawn(spec).unwrap();
    let tree = fixture.wait_for_tree();
    let cancelled = Instant::now();
    handle.cancel();
    let exit = handle.wait().unwrap();
    assert!(exit.cancel_requested);
    assert!(!exit.escalated_to_kill);
    tree.assert_gone_by(cancelled + CANCEL_BOUND);
}

#[test]
fn prompt_exits_within_5_seconds() {
    let fixture = Fixture::new();
    let started = Instant::now();
    let handle = process::spawn(fixture.spec("prompt", &[])).unwrap();
    let exit = handle.wait().unwrap();
    let took = started.elapsed();
    // No controlling terminal and stdin null: the prompt reads EOF instead of blocking.
    assert!(took < Duration::from_secs(5), "prompt took {took:?}");
    assert_eq!(exit.exit_code, Some(1));
    let stderr = fixture.stderr();
    assert!(
        stderr.contains("Traceback (most recent call last)"),
        "{stderr}"
    );
    assert!(stderr.contains("EOFError"), "{stderr}");
}

#[test]
fn argparse_error_exits_2_and_creates_nothing() {
    let fixture = Fixture::new();
    let handle = process::spawn(fixture.spec("argparse_error", &[])).unwrap();
    let exit = handle.wait().unwrap();
    assert_eq!(exit.exit_code, Some(2));
    assert!(!fixture.report().exists());
    assert!(fixture.stderr().contains("error: argument"));
}

#[test]
fn cancel_after_exit_changes_nothing() {
    let fixture = Fixture::new();
    let handle = process::spawn(fixture.spec("success", &[("FAKE_LEAPP_LINES", "1")])).unwrap();
    let exit = handle.wait().unwrap();
    handle.cancel();
    assert_eq!(handle.wait().unwrap(), exit);
    assert_eq!(
        handle.wait_timeout(Duration::ZERO).unwrap(),
        Some(exit.clone())
    );
    assert!(!exit.cancel_requested);
}

#[test]
fn wait_timeout_returns_none_while_running() {
    let fixture = Fixture::new();
    let handle = process::spawn(fixture.spec("slow", &[])).unwrap();
    assert_eq!(
        handle.wait_timeout(Duration::from_millis(100)).unwrap(),
        None
    );
    handle.cancel();
    assert!(handle.wait().unwrap().cancel_requested);
}

#[test]
fn cancel_from_another_thread() {
    let fixture = Fixture::new();
    let handle = Arc::new(process::spawn(fixture.spec("slow", &[])).unwrap());
    let tree = fixture.wait_for_tree();
    let waiter = {
        let handle = Arc::clone(&handle);
        thread::spawn(move || handle.wait())
    };
    let cancelled = Instant::now();
    handle.cancel();
    let exit = waiter.join().unwrap().unwrap();
    assert!(exit.cancel_requested);
    tree.assert_gone_by(cancelled + CANCEL_BOUND);
}

#[test]
fn spawn_errors_start_nothing() {
    let fixture = Fixture::new();
    let mut spec = fixture.spec("success", &[]);
    spec.program = fixture.out.join("no-such-tool");
    let error = process::spawn(spec).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    assert!(!fixture.pidfile.exists());

    // Existing logs are never overwritten.
    let spec = fixture.spec("success", &[]);
    let error = process::spawn(spec).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert!(!fixture.pidfile.exists());
}

/// ROADMAP B3: 50 lines at 100 ms; at least 40 must be delivered before the process exits.
#[test]
fn streams_screen_output_lines_before_exit() {
    let fixture = Fixture::new();
    let spec = fixture.spec(
        "success",
        &[
            ("FAKE_LEAPP_LINES", "50"),
            ("FAKE_LEAPP_INTERVAL_MS", "100"),
        ],
    );
    let handle = process::spawn(spec).unwrap();
    let mut tail = ScreenOutputTail::new(fixture.screen_output());
    let mut batches: Vec<(Instant, Vec<String>)> = Vec::new();
    let end = tail::follow(&handle, &mut tail, tail::POLL_INTERVAL, |lines| {
        batches.push((Instant::now(), lines));
    })
    .unwrap();

    assert_eq!(end.exit.exit_code, Some(0));
    let lines: Vec<&String> = batches.iter().flat_map(|(_, lines)| lines).collect();
    assert_eq!(lines.len(), 50);
    assert_eq!(
        lines[0],
        "Processing started. Please wait. This may take a few minutes..."
    );
    // Markup is stripped.
    assert_eq!(lines[1], "iLEAPP v0.0.0-fake (fake-leapp test double)");
    let before_exit: usize = batches
        .iter()
        .filter(|(at, _)| *at < end.exit.exit_instant)
        .map(|(_, lines)| lines.len())
        .sum();
    assert!(
        before_exit >= 40,
        "only {before_exit} of 50 lines arrived before the exit"
    );
    // stdout arrives only at exit (fully buffered); its tail comes with the end of the stream.
    assert_eq!(end.stdout_tail.len(), 50);
    assert_eq!(
        end.stdout_tail[1],
        "<b>iLEAPP v0.0.0-fake</b> (fake-leapp test double)"
    );
    assert!(end.stderr_tail.is_empty());
}

/// Signals a single process through `sh`, which has `kill` built in.
#[cfg(unix)]
fn kill_pid(pid: u32, signal: &str) {
    let status = std::process::Command::new("sh")
        .args(["-c", &format!("kill -{signal} {pid}")])
        .status()
        .unwrap();
    assert!(status.success());
}

#[cfg(unix)]
#[test]
fn ignore_term_cancel_escalates_to_kill() {
    let fixture = Fixture::new();
    let temp = process::create_temp_dir(&fixture.cache, JOB_ID).unwrap();
    let mut spec = fixture.spec("ignore_term", &[]);
    spec.temp_dir = Some(temp.clone());

    let (exit, tree, cancelled, took) = spawn_and_cancel(&fixture, spec, &temp);
    assert!(
        took >= process::DEFAULT_KILL_GRACE,
        "SIGKILL came before the grace: {took:?}"
    );
    assert!(took < CANCEL_BOUND, "cancel took {took:?}");
    tree.assert_gone_by(cancelled + CANCEL_BOUND);
    assert!(exit.cancel_requested);
    assert!(exit.escalated_to_kill);
    assert_eq!((exit.exit_code, exit.signal), (None, Some(9)));
    // SIGKILL leaks the tool's runtime dir; the job's temp dir removal takes it along.
    assert!(temp.join(format!("_MEIfake{}", tree.parent)).is_dir());
    process::remove_temp_dir(&fixture.cache, JOB_ID).unwrap();
    assert!(!temp.exists());
}

#[cfg(unix)]
#[test]
fn killing_only_the_parent_still_cleans_up_the_worker() {
    let fixture = Fixture::new();
    let handle = process::spawn(fixture.spec("slow", &[])).unwrap();
    let tree = fixture.wait_for_tree();
    let killed = Instant::now();
    kill_pid(tree.parent, "KILL");
    // The orphaned worker is still in the group, and the group kill reaches it.
    let exit = handle.wait().unwrap();
    assert!(killed.elapsed() < CANCEL_BOUND);
    tree.assert_gone_by(killed + CANCEL_BOUND);
    assert_eq!((exit.exit_code, exit.signal), (None, Some(9)));
    assert!(!exit.cancel_requested);
    assert!(!exit.escalated_to_kill);
}

#[cfg(unix)]
#[test]
fn sigterm_to_the_parent_is_forwarded_to_the_worker() {
    let fixture = Fixture::new();
    let handle = process::spawn(fixture.spec("slow", &[])).unwrap();
    let tree = fixture.wait_for_tree();
    let signalled = Instant::now();
    kill_pid(tree.parent, "TERM");
    let exit = handle.wait().unwrap();
    assert_eq!(exit.signal, Some(15));
    assert!(!exit.escalated_to_kill);
    tree.assert_gone_by(signalled + CANCEL_BOUND);
}
