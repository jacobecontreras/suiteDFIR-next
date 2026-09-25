//! The command layer without Tauri (ROADMAP E1b): one active job app-wide, `temp_cleanup`'s
//! refusals, recovery on `case_open`, `job_attach`, the openers and the other commands' rules. The
//! path policy is tested row by row in `policy`, the lock in `lock`, the logger in `logger`; the
//! E2 replay (`replay`) runs every command through Tauri's IPC.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use suitedfir_core::contracts::{
    AcqCancelRequest, AcqEvent, AcqFile, AcqRequest, AcqRestoreEncryptionRequest, AcqStatus,
    AppError, CaseCreateRequest, ErrorCode, InputType, JobAttachRequest, JobKind, ModuleSelection,
    OpenAcqFileRequest, OpenTextFileRequest, PathRequest, ProfileExportRequest,
    ProfileImportRequest, ProfileRef, ProfileSaveRequest, RunCancelRequest, RunEvent, RunFile,
    RunRef, RunRequest, RunStatus, SettingsDefaults, SettingsUpdateRequest, Timestamp, ToolId,
    ToolState, ToolsDirUpdate, examples,
};
use suitedfir_core::runner::RunControl;

use super::AttachSubscriber;
use crate::opener::testing::Opened;
use crate::state::{Job, JobGuard, JobHandle, Stream, Subscriber, TmpUsers};
use crate::testing::{Lab, UDID, lab_state};

const WAIT: Duration = Duration::from_secs(90);

fn new_case(lab: &Lab) -> PathBuf {
    let fields = examples::case_fields();
    let detail = lab
        .state
        .case_create(&CaseCreateRequest {
            name: fields.name,
            case_number: fields.case_number,
            examiner: fields.examiner,
            agency: fields.agency,
            description: fields.description,
            default_timezone: fields.default_timezone,
            parent_dir: None,
        })
        .unwrap();
    PathBuf::from(detail.path)
}

fn evidence(lab: &Lab) -> PathBuf {
    let dir = lab.root.path().join("ev").join("fs");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("evidence.txt"), "evidence").unwrap();
    dir
}

fn run_request(case: &Path, input: &Path) -> RunRequest {
    RunRequest {
        case_path: case.to_string_lossy().into_owned(),
        tool: ToolId::Ileapp,
        input_path: input.to_string_lossy().into_owned(),
        input_type: InputType::Fs,
        modules: ModuleSelection::All,
        timezone: None,
        itunes_password: None,
        keychain_path: None,
        hash_input: false,
        label: None,
    }
}

fn acq_request(case: &Path) -> AcqRequest {
    AcqRequest {
        case_path: case.to_string_lossy().into_owned(),
        udid: UDID.to_owned(),
        label: Some("Test iPhone".to_owned()),
        enable_encryption: false,
        encryption_password: None,
        restore_encryption: true,
    }
}

fn set_leapp_scenario(lab: &Lab, scenario: &str) {
    let mut env = lab.state.leapp_env.lock().unwrap();
    env.retain(|(name, _)| name != "FAKE_LEAPP_SCENARIO");
    env.push(("FAKE_LEAPP_SCENARIO".into(), scenario.into()));
}

/// Collects a job's events.
fn collector<E: Clone + Send + 'static>() -> (Subscriber<E>, Arc<Mutex<Vec<E>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    (
        Box::new(move |event: &E| sink.lock().unwrap().push(event.clone())),
        events,
    )
}

fn code<T: std::fmt::Debug>(result: Result<T, AppError>) -> ErrorCode {
    result.unwrap_err().code
}

/// Waits for the job's `finished` event. The job slot is freed just before that event is sent, so
/// waiting for the slot alone can be a moment early.
fn wait_finished<E>(events: &Mutex<Vec<E>>, finished: impl Fn(&E) -> bool) {
    let deadline = std::time::Instant::now() + WAIT;
    while !events.lock().unwrap().iter().any(&finished) {
        assert!(std::time::Instant::now() < deadline, "no finished event");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn run_finished(event: &RunEvent) -> bool {
    matches!(event, RunEvent::Finished { .. })
}

fn acq_finished(event: &AcqEvent) -> bool {
    matches!(event, AcqEvent::Finished { .. })
}

#[test]
fn one_active_job_app_wide() {
    let lab = lab_state();
    let state = &lab.state;
    let case = new_case(&lab);
    let input = evidence(&lab);
    set_leapp_scenario(&lab, "slow");
    let (subscriber, events) = collector::<RunEvent>();
    let run = state
        .run_start(run_request(&case, &input), subscriber)
        .unwrap();
    // A second run, an acquisition, a later restore and temp_cleanup are all refused.
    let (other, _) = collector::<RunEvent>();
    assert_eq!(
        code(state.run_start(run_request(&case, &input), other)),
        ErrorCode::RunAlreadyActive
    );
    let (acq_subscriber, _) = collector::<AcqEvent>();
    assert_eq!(
        code(state.acq_start(acq_request(&case), acq_subscriber)),
        ErrorCode::RunAlreadyActive
    );
    assert_eq!(code(state.temp_cleanup()), ErrorCode::RunAlreadyActive);
    // The refused starts created nothing.
    assert_eq!(fs::read_dir(case.join("runs")).unwrap().count(), 1);
    assert!(!case.join("acquisitions").exists());
    // The active job, then cancel.
    let active = state.job_active().unwrap();
    assert!(
        matches!(&active, suitedfir_core::contracts::ActiveJob::Run { run_id, .. } if *run_id == run.run_id)
    );
    assert_eq!(
        code(state.run_cancel(&RunCancelRequest {
            run_id: "20260924-183005Z-ileapp-000000".to_owned()
        })),
        ErrorCode::RunNotFound
    );
    state
        .run_cancel(&RunCancelRequest {
            run_id: run.run_id.clone(),
        })
        .unwrap();
    assert!(state.jobs.wait_idle(Some(WAIT)));
    wait_finished(&events, run_finished);
    let events = events.lock().unwrap();
    match events.last() {
        Some(RunEvent::Finished { status, .. }) => assert_eq!(*status, RunStatus::Cancelled),
        other => panic!("{other:?}"),
    }
    drop(events);
    assert_eq!(state.job_active(), None);
    // Cancelling a finished run: not found (it is no longer active).
    assert_eq!(
        code(state.run_cancel(&RunCancelRequest {
            run_id: run.run_id.clone()
        })),
        ErrorCode::RunNotFound
    );

    // An acquisition is a job too.
    lab.state.replace_idevice(lab.idevice_config("slow"));
    let (acq_subscriber, acq_events) = collector::<AcqEvent>();
    let acq = state.acq_start(acq_request(&case), acq_subscriber).unwrap();
    let (other, _) = collector::<RunEvent>();
    assert_eq!(
        code(state.run_start(run_request(&case, &input), other)),
        ErrorCode::RunAlreadyActive
    );
    // Its device is busy for polling and pairing.
    let devices = state.devices_list();
    assert!(
        devices.devices.iter().any(|d| d.udid == UDID && d.busy),
        "{devices:?}"
    );
    assert_eq!(
        code(
            state.device_pair(&suitedfir_core::contracts::DevicePairRequest {
                udid: UDID.to_owned()
            })
        ),
        ErrorCode::DeviceBusy
    );
    state
        .acq_cancel(&AcqCancelRequest {
            acq_id: acq.acq_id.clone(),
        })
        .unwrap();
    assert!(state.jobs.wait_idle(Some(WAIT)));
    wait_finished(&acq_events, acq_finished);
    match acq_events.lock().unwrap().last() {
        Some(AcqEvent::Finished { status, .. }) => assert_eq!(*status, AcqStatus::Cancelled),
        other => panic!("{other:?}"),
    }

    // A later restore counts as a job.
    state.jobs.lock().job = Some(Job {
        id: acq.acq_id.clone(),
        case_path: case.to_string_lossy().into_owned(),
        created_at: Timestamp::now(),
        handle: JobHandle::Restore {
            udid: UDID.to_owned(),
        },
    });
    let (other, _) = collector::<RunEvent>();
    assert_eq!(
        code(state.run_start(run_request(&case, &input), other)),
        ErrorCode::RunAlreadyActive
    );
    let (acq_subscriber, _) = collector::<AcqEvent>();
    assert_eq!(
        code(state.acq_start(acq_request(&case), acq_subscriber)),
        ErrorCode::RunAlreadyActive
    );
    assert_eq!(
        code(state.acq_restore_encryption(AcqRestoreEncryptionRequest {
            case_path: case.to_string_lossy().into_owned(),
            acq_id: acq.acq_id.clone(),
            password: "irrelevant".to_owned(),
        })),
        ErrorCode::RunAlreadyActive
    );
    // Not shown as an active job (the Case screen waits for the command's answer).
    assert_eq!(state.job_active(), None);
    state.jobs.finish(&acq.acq_id);
}

#[test]
fn temp_cleanup_waits_for_installs_and_device_commands() {
    let lab = lab_state();
    let state = &lab.state;
    let leftover = state
        .paths
        .temp_root()
        .join("20260101-000000Z-ileapp-abcdef");
    fs::create_dir_all(&leftover).unwrap();
    fs::write(leftover.join("x"), vec![0u8; 100]).unwrap();
    let install = state.installs.begin(ToolId::Ileapp, "iLEAPP").unwrap();
    assert_eq!(code(state.temp_cleanup()), ErrorCode::RunAlreadyActive);
    // One install per tool at a time.
    assert_eq!(
        code(state.installs.begin(ToolId::Ileapp, "iLEAPP").map(|_| ())),
        ErrorCode::RunAlreadyActive
    );
    drop(install);
    let op = state.device_op();
    assert_eq!(code(state.temp_cleanup()), ErrorCode::RunAlreadyActive);
    drop(op);
    // A job being started (its checks run without the slot's lock) holds the slot too.
    let reservation = state.jobs.reserve(None).unwrap();
    assert_eq!(code(state.temp_cleanup()), ErrorCode::RunAlreadyActive);
    drop(reservation);
    assert!(leftover.exists());
    let result = state.temp_cleanup().unwrap();
    assert_eq!(result.freed_bytes, 100);
    assert!(!leftover.exists());
}

/// N3: a sweep and the other users of `<app_cache>/tmp` exclude each other without a race: a
/// device command or install that starts during a sweep waits for it to end.
#[test]
fn a_sweep_holds_back_new_tmp_users() {
    let users = TmpUsers::default();
    let entered = AtomicBool::new(false);
    std::thread::scope(|scope| {
        users
            .clean(|| {
                scope.spawn(|| {
                    let _user = users.enter();
                    entered.store(true, Ordering::SeqCst);
                });
                std::thread::sleep(Duration::from_millis(100));
                assert!(
                    !entered.load(Ordering::SeqCst),
                    "a new user waits for the sweep"
                );
            })
            .unwrap();
    });
    assert!(entered.load(Ordering::SeqCst));
    let user = users.enter();
    assert!(users.clean(|| ()).is_none(), "no sweep while one runs");
    drop(user);
    assert!(users.clean(|| ()).is_some());
}

/// N2: while a job is being started, the slot is taken but not locked: other starts are refused,
/// and polling reports an acquisition's device busy.
#[test]
fn a_job_being_started_takes_the_slot_without_locking_it() {
    let lab = lab_state();
    let state = &lab.state;
    let case = new_case(&lab);
    let input = evidence(&lab);
    let reservation = state.jobs.reserve(Some(UDID.to_owned())).unwrap();
    let (other, _) = collector::<RunEvent>();
    assert_eq!(
        code(state.run_start(run_request(&case, &input), other)),
        ErrorCode::RunAlreadyActive
    );
    let (acq_subscriber, _) = collector::<AcqEvent>();
    assert_eq!(
        code(state.acq_start(acq_request(&case), acq_subscriber)),
        ErrorCode::RunAlreadyActive
    );
    assert_eq!(state.job_active(), None);
    let devices = state.devices_list();
    assert!(
        devices.devices.iter().any(|d| d.udid == UDID && d.busy),
        "{devices:?}"
    );
    // A failed start gives the slot back.
    drop(reservation);
    assert!(state.jobs.wait_idle(Some(Duration::from_millis(10))));
    assert_eq!(
        fs::read_dir(case.join("runs"))
            .map(|d| d.count())
            .unwrap_or(0),
        0
    );
}

/// N4: a job thread that panics does not leave a phantom active job: its guard frees the slot,
/// and its record is recovered as `interrupted` for the `finished` event.
#[test]
fn a_job_that_panics_frees_the_slot_and_ends_interrupted() {
    let lab = lab_state();
    let state = &lab.state;
    let case = new_case(&lab);
    let run_id = "20260924-183005Z-ileapp-3f9a1c";
    let dir = case.join("runs").join(run_id);
    fs::create_dir_all(&dir).unwrap();
    let mut record = examples::run_record_initial();
    record.run_id = run_id.to_owned();
    fs::write(dir.join("run.json"), serde_json::to_vec(&record).unwrap()).unwrap();
    let (subscriber, _) = collector::<RunEvent>();
    state.jobs.reserve(None).unwrap().activate(Job {
        id: run_id.to_owned(),
        case_path: case.to_string_lossy().into_owned(),
        created_at: Timestamp::now(),
        handle: JobHandle::Run {
            tool: ToolId::Ileapp,
            control: Arc::new(RunControl::default()),
            stream: Arc::new(Stream::new(subscriber)),
        },
    });
    let guard = JobGuard::new(Arc::clone(state), run_id.to_owned());
    let panicked = std::thread::spawn(move || {
        let _guard = guard;
        panic!("a bug in the job thread");
    })
    .join();
    assert!(panicked.is_err());
    assert!(state.jobs.wait_idle(Some(Duration::from_millis(10))));
    assert_eq!(state.job_active(), None);
    // The `finished` event a panicked run thread sends.
    match super::jobs::panicked_run_finished(&case, run_id) {
        Some(RunEvent::Finished {
            status,
            reasons,
            summary,
            ..
        }) => {
            assert_eq!(status, RunStatus::Interrupted);
            assert_eq!(reasons[0].code, "app_interrupted");
            assert_eq!(summary.run_id, run_id);
            assert_eq!(summary.status, RunStatus::Interrupted);
        }
        other => panic!("{other:?}"),
    }
    // And for an acquisition.
    let acq_id = "20260924-171200Z-ios-9c01de";
    let acq = case.join("acquisitions").join(acq_id);
    fs::create_dir_all(&acq).unwrap();
    let mut record = examples::acquisition_record();
    record.status = AcqStatus::Running;
    record.ended_at = None;
    record.duration_ms = None;
    record.output.seal.status = suitedfir_core::contracts::SealStatus::Pending;
    fs::write(
        acq.join("acquisition.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    match super::devices::panicked_acq_finished(&case, acq_id) {
        Some(AcqEvent::Finished {
            status, summary, ..
        }) => {
            assert_eq!(status, AcqStatus::Interrupted);
            assert_eq!(summary.acq_id, acq_id);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn case_open_lists_acquisitions_and_recovers_all_but_the_active_job() {
    let lab = lab_state();
    let state = &lab.state;
    let case = new_case(&lab);
    // Two runs and an acquisition left `running` by a crash.
    let mut runs = Vec::new();
    for id in [
        "20260924-183005Z-ileapp-3f9a1c",
        "20260924-183006Z-aleapp-3f9a1d",
    ] {
        let dir = case.join("runs").join(id);
        fs::create_dir_all(&dir).unwrap();
        let mut record = examples::run_record_initial();
        record.run_id = id.to_owned();
        fs::write(dir.join("run.json"), serde_json::to_vec(&record).unwrap()).unwrap();
        runs.push(id);
    }
    let acq_id = "20260924-171200Z-ios-9c01de";
    let acq = case.join("acquisitions").join(acq_id);
    fs::create_dir_all(&acq).unwrap();
    let mut record = examples::acquisition_record();
    record.status = AcqStatus::Running;
    record.ended_at = None;
    record.duration_ms = None;
    record.output.seal.status = suitedfir_core::contracts::SealStatus::Pending;
    fs::write(
        acq.join("acquisition.json"),
        serde_json::to_vec(&record).unwrap(),
    )
    .unwrap();
    // The second run is this process's active job: it is left alone.
    let (subscriber, _) = collector::<RunEvent>();
    state.jobs.lock().job = Some(Job {
        id: runs[1].to_owned(),
        case_path: case.to_string_lossy().into_owned(),
        created_at: Timestamp::now(),
        handle: JobHandle::Run {
            tool: ToolId::Aleapp,
            control: Arc::new(RunControl::default()),
            stream: Arc::new(Stream::new(subscriber)),
        },
    });
    let detail = state
        .case_open(&PathRequest {
            path: case.to_string_lossy().into_owned(),
        })
        .unwrap();
    state.jobs.finish(runs[1]);
    let mut recovered = detail.recovered.clone();
    recovered.sort();
    assert_eq!(recovered, [acq_id, runs[0]]);
    assert_eq!(detail.acquisitions.len(), 1);
    assert_eq!(detail.acquisitions[0].status, AcqStatus::Interrupted);
    let statuses: Vec<(String, RunStatus)> = detail
        .runs
        .iter()
        .map(|r| (r.run_id.clone(), r.status))
        .collect();
    assert!(statuses.contains(&(runs[0].to_owned(), RunStatus::Interrupted)));
    assert!(statuses.contains(&(runs[1].to_owned(), RunStatus::Running)));
    // Opening made it known and most recent.
    assert_eq!(
        state.settings().recent_cases[0],
        case.to_string_lossy().into_owned()
    );
}

#[test]
fn job_attach_returns_the_backlog_and_takes_over_the_events() {
    let lab = lab_state();
    let state = &lab.state;
    let case = new_case(&lab);
    let input = evidence(&lab);
    set_leapp_scenario(&lab, "slow");
    let (first_tx, first_rx) = mpsc::channel::<RunEvent>();
    let first = Mutex::new(first_tx);
    let run = state
        .run_start(
            run_request(&case, &input),
            Box::new(move |event: &RunEvent| {
                let _ = first.lock().unwrap().send(event.clone());
            }),
        )
        .unwrap();
    // Wait for the first log lines.
    loop {
        match first_rx.recv_timeout(WAIT).unwrap() {
            RunEvent::Log { .. } => break,
            _ => continue,
        }
    }
    let (subscriber, attached) = collector::<RunEvent>();
    let wrong = state.job_attach(
        &JobAttachRequest {
            kind: JobKind::Acquisition,
            id: run.run_id.clone(),
        },
        AttachSubscriber::Acquisition(Box::new(|_| {})),
    );
    assert_eq!(code(wrong), ErrorCode::AcqNotFound);
    let backlog = state
        .job_attach(
            &JobAttachRequest {
                kind: JobKind::Run,
                id: run.run_id.clone(),
            },
            AttachSubscriber::Run(subscriber),
        )
        .unwrap()
        .backlog;
    assert!(backlog[0].starts_with("Processing started"), "{backlog:?}");
    state
        .run_cancel(&RunCancelRequest {
            run_id: run.run_id.clone(),
        })
        .unwrap();
    assert!(state.jobs.wait_idle(Some(WAIT)));
    wait_finished(&attached, run_finished);
    // The new subscriber got the rest, ending with `finished`; the first one got nothing more.
    let attached = attached.lock().unwrap();
    assert!(matches!(attached.last(), Some(RunEvent::Finished { .. })));
    let late: Vec<RunEvent> = first_rx.try_iter().collect();
    assert!(
        !late.iter().any(|e| matches!(e, RunEvent::Finished { .. })),
        "{late:?}"
    );
    // After the job: not found.
    assert_eq!(
        code(state.job_attach(
            &JobAttachRequest {
                kind: JobKind::Run,
                id: run.run_id.clone(),
            },
            AttachSubscriber::Run(Box::new(|_| {})),
        )),
        ErrorCode::RunNotFound
    );
}

#[test]
fn a_subscriber_that_attaches_after_finished_still_gets_it() {
    let (subscriber, first) = collector::<RunEvent>();
    let stream = Stream::new(subscriber);
    stream.emit(&RunEvent::Log {
        lines: vec!["a".to_owned(), "b".to_owned()],
    });
    let finished = examples::run_events().pop().unwrap();
    assert!(matches!(finished, RunEvent::Finished { .. }));
    stream.emit(&finished);
    let (subscriber, second) = collector::<RunEvent>();
    assert_eq!(stream.attach(subscriber), ["a", "b"]);
    assert_eq!(*second.lock().unwrap(), [finished]);
    assert_eq!(first.lock().unwrap().len(), 2);
    // The backlog keeps the last 2,000 lines.
    let (subscriber, _) = collector::<RunEvent>();
    let stream = Stream::new(subscriber);
    for batch in 0..5 {
        stream.emit(&RunEvent::Log {
            lines: (0..500).map(|i| format!("{batch}-{i}")).collect(),
        });
    }
    let (subscriber, _) = collector::<RunEvent>();
    let backlog = stream.attach(subscriber);
    assert_eq!(backlog.len(), 2000);
    assert_eq!(backlog[0], "1-0");
    assert_eq!(backlog[1999], "4-499");
}

#[test]
fn a_run_opens_its_files_and_reveals_its_folder() {
    let lab = lab_state();
    let state = &lab.state;
    let case = new_case(&lab);
    let input = evidence(&lab);
    let (subscriber, _) = collector::<RunEvent>();
    let run = state
        .run_start(run_request(&case, &input), subscriber)
        .unwrap();
    assert!(state.jobs.wait_idle(Some(WAIT)));
    let run_ref = RunRef {
        case_path: case.to_string_lossy().into_owned(),
        run_id: run.run_id.clone(),
    };
    let record = state.run_get(&run_ref).unwrap();
    assert_eq!(record.status, RunStatus::Succeeded);
    assert_eq!(record.host, state.host);
    state.open_report(&run_ref).unwrap();
    for which in [
        RunFile::Stdout,
        RunFile::Stderr,
        RunFile::RunJson,
        RunFile::ReportManifest,
    ] {
        state
            .open_text_file(&OpenTextFileRequest {
                case_path: run_ref.case_path.clone(),
                run_id: run.run_id.clone(),
                which,
            })
            .unwrap();
    }
    state
        .reveal_path(&PathRequest {
            path: run.run_dir.clone(),
        })
        .unwrap();
    let dir = PathBuf::from(&run.run_dir);
    assert_eq!(
        lab.opener.calls(),
        [
            Opened::Open(dir.join("report").join("index.html")),
            Opened::Open(dir.join("leapp.stdout.log")),
            Opened::Open(dir.join("leapp.stderr.log")),
            Opened::Open(dir.join("run.json")),
            Opened::Open(dir.join("report.sha256")),
            Opened::Reveal(dir.clone()),
        ]
    );
    // Outside the known cases and the app dirs: refused.
    assert_eq!(
        code(state.reveal_path(&PathRequest {
            path: lab.root.path().to_string_lossy().into_owned(),
        })),
        ErrorCode::PathNotAllowed
    );
    // A run without a report.
    set_leapp_scenario(&lab, "early_exit");
    let (subscriber, _) = collector::<RunEvent>();
    let failed = state
        .run_start(run_request(&case, &input), subscriber)
        .unwrap();
    assert!(state.jobs.wait_idle(Some(WAIT)));
    let failed_ref = RunRef {
        case_path: run_ref.case_path.clone(),
        run_id: failed.run_id.clone(),
    };
    assert_eq!(
        code(state.open_report(&failed_ref)),
        ErrorCode::ReportMissing
    );
    assert_eq!(
        code(state.open_text_file(&OpenTextFileRequest {
            case_path: run_ref.case_path.clone(),
            run_id: failed.run_id,
            which: RunFile::ReportManifest,
        })),
        ErrorCode::ReportMissing
    );
    // Unknown case or run.
    assert_eq!(
        code(state.run_get(&RunRef {
            case_path: lab.root.path().to_string_lossy().into_owned(),
            run_id: run.run_id.clone(),
        })),
        ErrorCode::CaseNotFound
    );
    assert_eq!(
        code(state.run_get(&RunRef {
            case_path: run_ref.case_path.clone(),
            run_id: "20260924-183005Z-ileapp-000000".to_owned(),
        })),
        ErrorCode::RunNotFound
    );
}

#[test]
fn an_acquisition_opens_its_files() {
    let lab = lab_state();
    let state = &lab.state;
    let case = new_case(&lab);
    let (subscriber, events) = collector::<AcqEvent>();
    let acq = state.acq_start(acq_request(&case), subscriber).unwrap();
    assert!(state.jobs.wait_idle(Some(WAIT)));
    wait_finished(&events, acq_finished);
    match events.lock().unwrap().last() {
        Some(AcqEvent::Finished {
            status, summary, ..
        }) => {
            assert_eq!(*status, AcqStatus::Succeeded);
            assert_eq!(summary.acq_id, acq.acq_id);
        }
        other => panic!("{other:?}"),
    }
    let dir = PathBuf::from(&acq.acq_dir);
    for (which, file) in [
        (AcqFile::AcquisitionJson, "acquisition.json"),
        (AcqFile::BackupManifest, "backup.sha256"),
        (AcqFile::DeviceInfo, "device-info.plist"),
        (AcqFile::Stdout, "idevicebackup2.stdout.log"),
    ] {
        state
            .open_acq_file(&OpenAcqFileRequest {
                case_path: case.to_string_lossy().into_owned(),
                acq_id: acq.acq_id.clone(),
                which,
            })
            .unwrap();
        assert_eq!(
            lab.opener.calls().last(),
            Some(&Opened::Open(dir.join(file)))
        );
    }
    // The record's host is this machine's.
    let record = state
        .acq_get(&suitedfir_core::contracts::AcqRef {
            case_path: case.to_string_lossy().into_owned(),
            acq_id: acq.acq_id.clone(),
        })
        .unwrap();
    assert_eq!(record.host, state.host);
    // A later restore of an acquisition that left nothing to restore.
    assert_eq!(
        code(state.acq_restore_encryption(AcqRestoreEncryptionRequest {
            case_path: case.to_string_lossy().into_owned(),
            acq_id: acq.acq_id.clone(),
            password: "pw-1234".to_owned(),
        })),
        ErrorCode::RestoreNotApplicable
    );
    assert_eq!(state.job_active(), None);
}

#[test]
fn app_info_settings_and_tools() {
    let lab = lab_state();
    let state = &lab.state;
    let info = state.app_info();
    assert!(info.dev_override);
    assert_eq!(info.app_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(
        PathBuf::from(&info.paths.tools_dir),
        state.paths.default_tools_dir()
    );
    // Both tools are the dev override.
    let tools = state.tools_status().unwrap();
    assert!(
        tools.iter().all(|t| t.state == ToolState::DevOverride),
        "{tools:?}"
    );
    let modules = state.tool_modules(ToolId::Ileapp).unwrap();
    assert_eq!(modules.version, "dev-override");
    assert!(modules.timezones.unwrap().contains(&"UTC".to_owned()));
    // Settings: validated paths, defaults replaced, the tools dir set and reset.
    let tools_dir = lab.root.path().join("tools");
    fs::create_dir_all(&tools_dir).unwrap();
    let settings = state
        .settings_update(&SettingsUpdateRequest {
            cases_root: Some(lab.cases.to_string_lossy().into_owned()),
            defaults: Some(SettingsDefaults {
                examiner: "J. Doe".to_owned(),
                agency: "Lab".to_owned(),
                timezone: "Europe/Berlin".to_owned(),
            }),
            tools_dir: ToolsDirUpdate::Set(tools_dir.to_string_lossy().into_owned()),
        })
        .unwrap();
    assert_eq!(settings.defaults.examiner, "J. Doe");
    assert_eq!(
        settings.tools_dir.as_deref(),
        Some(tools_dir.to_string_lossy().as_ref())
    );
    assert_eq!(state.settings(), settings);
    // Saved.
    let saved: suitedfir_core::contracts::Settings =
        serde_json::from_slice(&fs::read(state.paths.settings_file()).unwrap()).unwrap();
    assert_eq!(saved, settings);
    assert_eq!(PathBuf::from(state.app_info().paths.tools_dir), tools_dir);
    // A cases root inside the app dirs, a missing folder: refused, nothing changed.
    fs::create_dir_all(&state.paths.app_data).unwrap();
    for bad in [
        state.paths.app_data.clone(),
        lab.root.path().join("missing"),
    ] {
        let update = SettingsUpdateRequest {
            cases_root: Some(bad.to_string_lossy().into_owned()),
            ..SettingsUpdateRequest::default()
        };
        assert_eq!(
            code(state.settings_update(&update)),
            ErrorCode::PathNotAllowed
        );
    }
    assert_eq!(state.settings(), settings);
    let reset = state
        .settings_update(&SettingsUpdateRequest {
            tools_dir: ToolsDirUpdate::Reset,
            ..SettingsUpdateRequest::default()
        })
        .unwrap();
    assert_eq!(reset.tools_dir, None);
    assert_eq!(reset.defaults, settings.defaults);
}

#[test]
fn tools_without_the_dev_override_are_not_installed() {
    let lab = crate::testing::lab(crate::testing::LabOptions {
        leapp_override: Vec::new(),
        ..Default::default()
    });
    let state = &lab.state;
    for status in state.tools_status().unwrap() {
        assert!(
            matches!(
                status.state,
                ToolState::NotInstalled | ToolState::UnsupportedPlatform
            ),
            "{status:?}"
        );
    }
    assert_eq!(
        code(state.tool_modules(ToolId::Aleapp)),
        if state.platform.is_some() {
            ErrorCode::ToolNotInstalled
        } else {
            ErrorCode::UnsupportedPlatform
        }
    );
    // A run needs an installed, verified tool.
    let case = new_case(&lab);
    let input = evidence(&lab);
    let (subscriber, _) = collector::<RunEvent>();
    let error = state
        .run_start(run_request(&case, &input), subscriber)
        .unwrap_err();
    assert!(
        matches!(
            error.code,
            ErrorCode::ToolNotInstalled | ErrorCode::UnsupportedPlatform
        ),
        "{error:?}"
    );
    assert!(!case.join("runs").exists());
}

/// S3: a tool installed through the real pipeline, whose entry is changed afterwards, is refused
/// right before a run (`tool_verification_failed`, ARCHITECTURE.md §6 step 1), and nothing is
/// created: no run folder, no temp dir.
#[test]
fn a_tampered_tool_is_refused_before_a_run_and_nothing_is_created() {
    let placeholder = tempfile::Builder::new().prefix("sdr").tempdir().unwrap();
    let stand_in = crate::testing::aleapp_stand_in(placeholder.path());
    let lab = crate::testing::lab(crate::testing::LabOptions {
        leapp_override: vec![ToolId::Ileapp],
        manifest: stand_in.manifest,
        download_from: Some(stand_in.zip.clone()),
        ..Default::default()
    });
    let state = &lab.state;
    let status = state
        .tool_install(ToolId::Aleapp, super::InstallFrom::Download, &mut |_| {})
        .unwrap();
    assert_eq!(status.state, ToolState::Verified, "{status:?}");
    let entry = PathBuf::from(status.install_dir.unwrap())
        .join("bin")
        .join(format!("fake-leapp-probe{}", crate::testing::EXE));
    let case = new_case(&lab);
    let input = evidence(&lab);
    let request = || RunRequest {
        tool: ToolId::Aleapp,
        timezone: None,
        ..run_request(&case, &input)
    };
    // The verified tool runs.
    let (subscriber, events) = collector::<RunEvent>();
    state.run_start(request(), subscriber).unwrap();
    assert!(state.jobs.wait_idle(Some(WAIT)));
    wait_finished(&events, run_finished);
    let runs_before = fs::read_dir(case.join("runs")).unwrap().count();
    // One byte appended to the installed entry.
    let mut file = fs::OpenOptions::new().append(true).open(&entry).unwrap();
    std::io::Write::write_all(&mut file, b"\0").unwrap();
    drop(file);
    let (subscriber, _) = collector::<RunEvent>();
    let error = state.run_start(request(), subscriber).unwrap_err();
    assert_eq!(error.code, ErrorCode::ToolVerificationFailed, "{error:?}");
    assert_eq!(
        fs::read_dir(case.join("runs")).unwrap().count(),
        runs_before
    );
    let temp_dirs = fs::read_dir(state.paths.temp_root())
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(temp_dirs, 0, "no temp dir is left or created");
    assert_eq!(state.job_active(), None);
    // No longer shown as verified; `tool_verify` reports the failure.
    let aleapp = state
        .tools_status()
        .unwrap()
        .into_iter()
        .find(|s| s.tool == ToolId::Aleapp)
        .unwrap();
    assert_eq!(aleapp.state, ToolState::InstalledUnverified, "{aleapp:?}");
    let verified = state.tool_status(ToolId::Aleapp, true).unwrap();
    assert_eq!(
        verified.state,
        ToolState::VerificationFailed,
        "{verified:?}"
    );
}

#[test]
fn profiles_and_the_recent_list() {
    let lab = lab_state();
    let state = &lab.state;
    let case = new_case(&lab);
    let saved = state
        .profile_save(&ProfileSaveRequest {
            tool: ToolId::Aleapp,
            name: "Triage".to_owned(),
            modules: vec!["callLogs".to_owned()],
        })
        .unwrap();
    assert_eq!(saved.unknown_modules, Vec::<String>::new());
    assert_eq!(
        code(state.profile_save(&ProfileSaveRequest {
            tool: ToolId::Aleapp,
            name: "Bad".to_owned(),
            modules: vec!["noSuchModule".to_owned()],
        })),
        ErrorCode::UnknownModules
    );
    let file = lab.root.path().join("Imported.alprofile");
    fs::write(
        &file,
        r#"{"leapp": "aleapp", "format_version": 1, "plugins": ["smsMms", "gone"]}"#,
    )
    .unwrap();
    let imported = state
        .profile_import(&ProfileImportRequest {
            tool: ToolId::Aleapp,
            path: file.to_string_lossy().into_owned(),
            name: None,
            overwrite: false,
        })
        .unwrap();
    assert_eq!(imported.name, "Imported");
    assert_eq!(imported.unknown_modules, ["gone"]);
    let listed = state.profiles_list(ToolId::Aleapp).unwrap();
    assert_eq!(listed.len(), 2);
    let dest = lab.root.path().join("export.alprofile");
    state
        .profile_export(&ProfileExportRequest {
            tool: ToolId::Aleapp,
            name: "Triage".to_owned(),
            dest_path: dest.to_string_lossy().into_owned(),
        })
        .unwrap();
    assert!(dest.is_file());
    fs::create_dir_all(case.join("runs")).unwrap();
    assert_eq!(
        code(
            state.profile_export(&ProfileExportRequest {
                tool: ToolId::Aleapp,
                name: "Triage".to_owned(),
                dest_path: case
                    .join("runs")
                    .join("x.alprofile")
                    .to_string_lossy()
                    .into_owned(),
            })
        ),
        ErrorCode::PathNotAllowed
    );
    state
        .profile_delete(&ProfileRef {
            tool: ToolId::Aleapp,
            name: "Imported".to_owned(),
        })
        .unwrap();
    assert_eq!(state.profiles_list(ToolId::Aleapp).unwrap().len(), 1);

    // A case whose folder is gone can still be forgotten; an unknown one cannot.
    let path = case.to_string_lossy().into_owned();
    fs::remove_dir_all(&case).unwrap();
    let listed = state.cases_list();
    assert!(!listed[0].exists);
    state
        .case_forget(&PathRequest { path: path.clone() })
        .unwrap();
    assert!(state.cases_list().is_empty());
    assert_eq!(
        code(state.case_forget(&PathRequest { path })),
        ErrorCode::CaseNotFound
    );
}
