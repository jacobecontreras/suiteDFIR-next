//! Acquisition integration tests with fake-idevice (ROADMAP X3a, X3b; CONTRACTS.md §13.4): every
//! scenario row ends in a finalized, read-only `acquisition.json` with the expected status, reasons
//! and warnings, and a `backup.sha256` manifest. Plus the cancel semantics by phase, crash recovery,
//! the later restore, the start checks, and a proof that the password never leaks.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use common::{Lab, UDID};
use suitedfir_core::acquire::{
    self, AcqContext, AcqControl, AcqError, AcqOutcome, BACKUP_MANIFEST, RestoreRefusal,
};
use suitedfir_core::case::{self, CreatedCase};
use suitedfir_core::contracts::{
    AcqCommandPurpose, AcqEvent, AcqPhase, AcqRequest, AcqStatus, AcquisitionRecord,
    DeviceChangeKind, DevicePromptKind, EncryptionRestoreRecord, ErrorCode, IdeviceToolSource,
    PasswordChannel, PreflightLevel, RecordHost, RestoreState, SealStatus, Timestamp,
    ToolVerification, examples, parse_versioned,
};
use suitedfir_core::hashing;
use suitedfir_core::idevice::Password;

const PASSWORD: &str = "k7-Examiner-Pw!";

fn host() -> RecordHost {
    RecordHost {
        os: "testos".to_owned(),
        os_version: "1.0".to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        hostname: "LAB-TEST-01".to_owned(),
    }
}

fn new_case(lab: &Lab) -> CreatedCase {
    case::create(lab.root.path(), &examples::case_fields()).unwrap()
}

fn context(case: &CreatedCase) -> AcqContext {
    AcqContext {
        case_dir: case.path.clone(),
        case: case.case.clone(),
        host: host(),
    }
}

fn request(case: &CreatedCase, password: Option<&str>) -> AcqRequest {
    AcqRequest {
        case_path: case.path.to_string_lossy().into_owned(),
        udid: UDID.to_owned(),
        label: Some("Test iPhone".to_owned()),
        enable_encryption: password.is_some(),
        encryption_password: password.map(str::to_owned),
        restore_encryption: true,
    }
}

/// Runs an acquisition to the end; `hook` sees every event (and may cancel through the control).
fn acquire(
    lab: &Lab,
    case: &CreatedCase,
    request: AcqRequest,
    mut hook: impl FnMut(&AcqEvent, &AcqControl),
) -> (AcqOutcome, Vec<AcqEvent>) {
    let job = acquire::start(&lab.idevice, request, context(case)).unwrap();
    assert!(job.acq_dir().is_dir());
    let control = job.control();
    let mut events = Vec::new();
    let outcome = job.run(&mut |event| {
        hook(&event, &control);
        events.push(event);
    });
    (outcome, events)
}

fn simple(lab: &Lab, password: Option<&str>) -> (CreatedCase, AcqOutcome, Vec<AcqEvent>) {
    let case = new_case(lab);
    let (outcome, events) = acquire(lab, &case, request(&case, password), |_, _| {});
    (case, outcome, events)
}

fn codes(reasons: &[suitedfir_core::contracts::Reason]) -> Vec<&str> {
    reasons.iter().map(|r| r.code.as_str()).collect()
}

fn acq_dir(outcome: &AcqOutcome) -> PathBuf {
    PathBuf::from(&outcome.summary.acq_dir)
}

fn read_record(dir: &Path) -> AcquisitionRecord {
    parse_versioned(&fs::read(dir.join("acquisition.json")).unwrap()).unwrap()
}

fn phases(events: &[AcqEvent]) -> Vec<AcqPhase> {
    events
        .iter()
        .filter_map(|e| match e {
            AcqEvent::Phase { phase } => Some(*phase),
            _ => None,
        })
        .collect()
}

fn prompts(events: &[AcqEvent]) -> Vec<DevicePromptKind> {
    events
        .iter()
        .filter_map(|e| match e {
            AcqEvent::DevicePrompt { kind, .. } => Some(*kind),
            _ => None,
        })
        .collect()
}

fn log_lines(events: &[AcqEvent]) -> Vec<String> {
    events
        .iter()
        .flat_map(|e| match e {
            AcqEvent::Log { lines } => lines.clone(),
            _ => Vec::new(),
        })
        .collect()
}

/// The checks every scenario row gets: status, reasons and warnings; a finalized, read-only
/// record equal to the outcome; a manifest that matches its recorded hash; the `finished` event;
/// no temp dirs left behind.
fn assert_final(
    lab: &Lab,
    outcome: &AcqOutcome,
    events: &[AcqEvent],
    status: AcqStatus,
    reasons: &[&str],
    warnings: &[&str],
) -> AcquisitionRecord {
    assert_eq!(outcome.write_error, None);
    let record = &outcome.record;
    let dir = acq_dir(outcome);
    let context = format!("{:?} {:?}", record.status_reasons, record.warnings);
    assert_eq!(record.status, status, "{context}");
    assert_eq!(codes(&record.status_reasons), reasons, "{context}");
    assert_eq!(codes(&record.warnings), warnings, "{context}");
    // Finalized: read-only, as returned, with the end time.
    let file = dir.join("acquisition.json");
    assert!(fs::metadata(&file).unwrap().permissions().readonly());
    assert_eq!(read_record(&dir), *record);
    assert!(record.ended_at.is_some() && record.duration_ms.is_some());
    assert_eq!(record.recovered_at, None);
    // The manifest.
    let seal = &record.output.seal;
    assert_eq!(seal.status, SealStatus::Sealed, "{seal:?}");
    assert_eq!(seal.manifest.as_deref(), Some(BACKUP_MANIFEST));
    let manifest = dir.join(BACKUP_MANIFEST);
    assert_eq!(
        seal.manifest_sha256.as_deref(),
        Some(hashing::sha256_file(&manifest).unwrap().as_str())
    );
    let lines = fs::read_to_string(&manifest).unwrap();
    assert_eq!(lines.lines().count() as u64, seal.file_count.unwrap());
    assert!(
        lines.lines().all(|l| l[66..].starts_with("backup/")),
        "{lines}"
    );
    // The last event is `finished`, with the record's outcome.
    match events.last() {
        Some(AcqEvent::Finished {
            status: finished,
            reasons: finished_reasons,
            warnings: finished_warnings,
            summary,
        }) => {
            assert_eq!(*finished, status);
            assert_eq!(finished_reasons, &record.status_reasons);
            assert_eq!(finished_warnings, &record.warnings);
            assert_eq!(**summary, outcome.summary);
        }
        other => panic!("last event: {other:?}"),
    }
    assert_eq!(phases(events).first(), Some(&AcqPhase::Preparing));
    assert_eq!(phases(events).last(), Some(&AcqPhase::Finalizing));
    lab.assert_no_temp_dirs();
    record.clone()
}

// ---- X3a: without encryption ----

#[test]
fn success() {
    let lab = Lab::new("success");
    let case = new_case(&lab);
    let mut progress_at = Vec::new();
    let (outcome, events) = acquire(&lab, &case, request(&case, None), |event, _| {
        if let AcqEvent::Progress { .. } = event {
            progress_at.push(Instant::now());
        }
    });
    let record = assert_final(&lab, &outcome, &events, AcqStatus::Succeeded, &[], &[]);
    let dir = acq_dir(&outcome);
    assert_eq!(
        phases(&events),
        [
            AcqPhase::Preparing,
            AcqPhase::BackingUp,
            AcqPhase::Validating,
            AcqPhase::Sealing,
            AcqPhase::Finalizing
        ]
    );
    // Identity from the full ideviceinfo output, saved (never logged) as device-info.plist.
    let device = &record.device;
    assert_eq!(device.udid, UDID);
    assert_eq!(device.serial_number.as_deref(), Some("F2LFAKE00001"));
    assert_eq!(device.device_name.as_deref(), Some("Fake iPhone"));
    assert_eq!(device.product_version.as_deref(), Some("18.6"));
    assert_eq!(device.build_version.as_deref(), Some("22G86"));
    assert!(device.captured_at.is_some());
    assert_eq!(device.info_file.as_deref(), Some("device-info.plist"));
    let info = dir.join("device-info.plist");
    assert!(
        fs::read_to_string(&info)
            .unwrap()
            .contains("356938035643809")
    );
    assert_eq!(
        device.info_file_sha256.as_deref(),
        Some(hashing::sha256_file(&info).unwrap().as_str())
    );
    assert!(
        !log_lines(&events)
            .iter()
            .any(|l| l.contains("356938035643809")),
        "the IMEI never reaches the log"
    );
    // Pairing and device changes.
    assert!(record.pairing.paired_before);
    assert_eq!(record.pairing.paired_by_app_at, None);
    assert_eq!(
        record.pairing.host_id.as_deref(),
        Some("5E1B7C2A-9D4F-4E8B-A3C6-1F0D2B7E9A48")
    );
    assert!(record.pairing.system_buid.is_some());
    let changes: Vec<_> = record.device_changes.iter().map(|c| c.change).collect();
    assert_eq!(changes, [DeviceChangeKind::SyncLockTaken]);
    // The backup command.
    assert_eq!(record.commands.len(), 1);
    let command = &record.commands[0];
    assert_eq!(command.purpose, AcqCommandPurpose::Backup);
    assert_eq!(command.exit_code, Some(0));
    assert!(command.exited_at.is_some());
    let backup_dir = std::path::absolute(dir.join("backup"))
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        command.argv[1..],
        ["-u", UDID, "backup", "--full", backup_dir.as_str()]
    );
    let process = record.process.as_ref().unwrap();
    assert_eq!((process.exit_code, process.signal), (Some(0), None));
    assert!(!process.cancel_requested && !process.escalated_to_kill);
    // Validation facts.
    let result = record.backup_result.as_ref().unwrap();
    assert_eq!(result.final_message.as_deref(), Some("Backup Successful."));
    assert_eq!(result.udid_dir, format!("backup/{UDID}"));
    assert_eq!(result.manifest_found.as_deref(), Some("Manifest.db"));
    assert!(result.info_plist_found && result.status_plist_found);
    assert_eq!(result.snapshot_state.as_deref(), Some("finished"));
    assert_eq!(result.last_progress_percent, Some(100));
    assert_eq!(result.device_file_errors, 0);
    assert!(result.free_bytes_after.is_some());
    // Encryption untouched; no password.
    assert_eq!(record.encryption.will_encrypt_before, Some(false));
    assert!(!record.encryption.enable_requested && !record.encryption.restore_requested);
    assert_eq!(record.encryption.restored_after, RestoreState::NotRequested);
    assert!(!record.encryption.password_supplied);
    assert_eq!(record.encryption.password_channel, None);
    // Tools, times, logs.
    assert_eq!(record.tools.source, IdeviceToolSource::Bundled);
    assert_eq!(
        record.tools.binaries.idevicebackup2.verified_against,
        ToolVerification::Manifest
    );
    assert_eq!(record.host, host());
    assert_eq!(record.case_snapshot.case_id, case.case.case_id);
    assert_eq!(record.label.as_deref(), Some("Test iPhone"));
    assert!(record.started_at.unwrap() >= record.created_at);
    assert!(
        fs::read_to_string(dir.join("idevicebackup2.stdout.log"))
            .unwrap()
            .contains("Backup Successful.")
    );
    assert_eq!(record.seal_file_count(), 6);
    // Progress: overall only, increasing, ending at 100, at most 4 per second.
    let progress: Vec<u8> = events
        .iter()
        .filter_map(|e| match e {
            AcqEvent::Progress { percent } => Some(*percent),
            _ => None,
        })
        .collect();
    assert!(!progress.is_empty());
    assert!(progress.windows(2).all(|w| w[0] <= w[1]), "{progress:?}");
    assert_eq!(progress.last(), Some(&100));
    for pair in progress_at.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            gap >= Duration::from_millis(240),
            "{gap:?} between progress events"
        );
    }
    // The listing entry.
    let summary = &outcome.summary;
    assert_eq!(summary.status, AcqStatus::Succeeded);
    assert_eq!(
        summary.backup_path.as_deref(),
        Some(dir.join("backup").join(UDID).to_string_lossy().as_ref())
    );
    let listed = acquire::discover(&case.path).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(acquire::summary(&listed[0]), *summary);
    assert_eq!(acquire::load(&case.path, &record.acq_id).unwrap(), record);
    // A run on the backup records the acquisition id.
    assert_eq!(
        acquire::acquisition_id_for_input(&dir.join("backup").join(UDID), [&case.path])
            .unwrap()
            .as_deref(),
        Some(record.acq_id.as_str())
    );
}

/// `seal.file_count`, for brevity.
trait SealCount {
    fn seal_file_count(&self) -> u64;
}

impl SealCount for AcquisitionRecord {
    fn seal_file_count(&self) -> u64 {
        self.output.seal.file_count.unwrap()
    }
}

#[test]
fn already_encrypted() {
    let lab = Lab::new("already_encrypted");
    let (_, outcome, events) = simple(&lab, None);
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        AcqStatus::Succeeded,
        &[],
        &["backup_encryption_preexisting"],
    );
    assert_eq!(record.encryption.will_encrypt_before, Some(true));
    assert!(!record.encryption.enable_requested);
}

#[test]
fn backup_fail() {
    let lab = Lab::new("backup_fail");
    let (_, outcome, events) = simple(&lab, None);
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        AcqStatus::Failed,
        &[
            "nonzero_exit",
            "success_message_missing",
            "manifest_missing",
            "snapshot_not_finished",
        ],
        &[],
    );
    let expected_exit = if cfg!(windows) { -105 } else { 151 };
    assert_eq!(record.process.unwrap().exit_code, Some(expected_exit));
    assert_eq!(
        record.backup_result.unwrap().final_message.as_deref(),
        Some("Backup Failed (Error Code 105).")
    );
    assert_eq!(outcome.summary.backup_path, None);
}

#[test]
fn incomplete() {
    let lab = Lab::new("incomplete");
    let (_, outcome, events) = simple(&lab, None);
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        AcqStatus::Failed,
        &["success_message_missing", "snapshot_not_finished"],
        &[],
    );
    assert_eq!(record.process.unwrap().exit_code, Some(0));
    let result = record.backup_result.unwrap();
    assert_eq!(result.snapshot_state.as_deref(), Some("new"));
    assert_eq!(
        result.final_message.as_deref(),
        Some("Backup Failed (Error Code 0).")
    );
}

#[test]
fn cancel_on_device() {
    let lab = Lab::new("cancel_on_device");
    let (_, outcome, events) = simple(&lab, None);
    assert_final(
        &lab,
        &outcome,
        &events,
        AcqStatus::Failed,
        &[
            "nonzero_exit",
            "cancelled_on_device",
            "success_message_missing",
            "manifest_missing",
            "snapshot_not_finished",
        ],
        &[],
    );
}

#[test]
fn disconnect() {
    let lab = Lab::new("disconnect");
    let (_, outcome, events) = simple(&lab, None);
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        AcqStatus::Failed,
        &[
            "nonzero_exit",
            "device_disconnected",
            "success_message_missing",
            "manifest_missing",
            "snapshot_not_finished",
        ],
        &[],
    );
    assert_eq!(
        record.backup_result.unwrap().final_message.as_deref(),
        Some("Backup Aborted.")
    );
}

#[test]
fn sync_lock() {
    let lab = Lab::new("sync_lock");
    let (_, outcome, events) = simple(&lab, None);
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        AcqStatus::Failed,
        &[
            "nonzero_exit",
            "sync_lock_failed",
            "success_message_missing",
            "backup_dir_missing",
        ],
        &[],
    );
    assert_eq!(record.seal_file_count(), 0, "backup/ exists but is empty");
    assert!(
        log_lines(&events)
            .iter()
            .any(|l| l == "ERROR: timeout while locking for sync")
    );
    // The lock was requested (the tool posted the sync notifications) but never held.
    let lock = &record.device_changes[0];
    assert_eq!(lock.change, DeviceChangeKind::SyncLockTaken);
    assert!(
        lock.detail.contains("could not be taken"),
        "{}",
        lock.detail
    );
}

/// Cancels once the backup is under way (its signal handlers are installed by then).
fn cancel_when_backing_up(event: &AcqEvent, control: &AcqControl) {
    if let AcqEvent::Log { lines } = event
        && lines.iter().any(|l| l == "Full backup mode.")
    {
        control.cancel();
    }
}

#[test]
fn slow_cancel() {
    let lab = Lab::new("slow");
    let case = new_case(&lab);
    let started = Instant::now();
    let (outcome, events) = acquire(&lab, &case, request(&case, None), cancel_when_backing_up);
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        AcqStatus::Cancelled,
        &["cancelled_by_user"],
        &[],
    );
    let process = record.process.unwrap();
    assert!(process.cancel_requested);
    if cfg!(windows) {
        // Terminating the job is the only stop on Windows: recorded as a kill (§13.4).
        assert!(process.escalated_to_kill);
    } else {
        // SIGTERM: the tool prints `Backup Aborted.` and exits within 2 s, no SIGKILL needed.
        assert!(!process.escalated_to_kill);
        assert_eq!(process.exit_code, Some(255));
        assert_eq!(
            record.backup_result.unwrap().final_message.as_deref(),
            Some("Backup Aborted.")
        );
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "{:?}",
            started.elapsed()
        );
    }
}

#[cfg(unix)]
#[test]
fn ignore_term_cancel_escalates_after_the_30_s_grace() {
    let lab = Lab::new("ignore_term");
    let case = new_case(&lab);
    let mut cancelled_at = None;
    let (outcome, events) = acquire(&lab, &case, request(&case, None), |event, control| {
        if cancelled_at.is_none()
            && let AcqEvent::Log { lines } = event
            && lines.iter().any(|l| l == "Full backup mode.")
        {
            cancelled_at = Some(Instant::now());
            control.cancel();
        }
    });
    let waited = cancelled_at.unwrap().elapsed();
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        AcqStatus::Cancelled,
        &["cancelled_by_user"],
        &[],
    );
    let process = record.process.unwrap();
    assert!(process.cancel_requested && process.escalated_to_kill);
    assert_eq!((process.exit_code, process.signal), (None, Some(9)));
    assert!(waited >= Duration::from_secs(30), "{waited:?}");
    assert!(waited < Duration::from_secs(45), "{waited:?}");
}

#[test]
fn info_empty() {
    let lab = Lab::new("info_empty");
    let (_, outcome, events) = simple(&lab, None);
    let record = assert_final(&lab, &outcome, &events, AcqStatus::Succeeded, &[], &[]);
    // ideviceinfo returned nothing: every device field but the UDID is null, no file written.
    let device = &record.device;
    assert_eq!(device.udid, UDID);
    assert_eq!(
        (
            &device.serial_number,
            &device.device_name,
            &device.product_type,
            &device.product_version,
            &device.build_version
        ),
        (&None, &None, &None, &None, &None)
    );
    assert_eq!(
        (
            &device.captured_at,
            &device.info_file,
            &device.info_file_sha256
        ),
        (&None, &None, &None)
    );
    assert!(!acq_dir(&outcome).join("device-info.plist").exists());
    assert_eq!(record.encryption.will_encrypt_before, None);
}

// ---- X3b: encryption ----

#[test]
fn success_encrypt() {
    let lab = Lab::new("success_encrypt");
    let (_, outcome, events) = simple(&lab, Some(PASSWORD));
    let record = assert_final(&lab, &outcome, &events, AcqStatus::Succeeded, &[], &[]);
    assert_eq!(
        phases(&events),
        [
            AcqPhase::Preparing,
            AcqPhase::EnablingEncryption,
            AcqPhase::BackingUp,
            AcqPhase::RestoringEncryption,
            AcqPhase::Validating,
            AcqPhase::Sealing,
            AcqPhase::Finalizing
        ]
    );
    assert_eq!(
        prompts(&events),
        [
            DevicePromptKind::PasscodeForEncryption,
            DevicePromptKind::PasscodeForEncryption
        ]
    );
    let encryption = &record.encryption;
    assert_eq!(encryption.will_encrypt_before, Some(false));
    assert!(encryption.enable_requested && encryption.enabled_by_examiner);
    assert_eq!(encryption.will_encrypt_after_enable, Some(true));
    assert!(encryption.restore_requested);
    assert_eq!(encryption.restored_after, RestoreState::Restored);
    assert_eq!(encryption.will_encrypt_after_restore, Some(false));
    assert!(encryption.password_supplied);
    assert_eq!(encryption.password_channel, Some(PasswordChannel::Env));
    let changes: Vec<_> = record.device_changes.iter().map(|c| c.change).collect();
    assert_eq!(
        changes,
        [
            DeviceChangeKind::BackupEncryptionEnabled,
            DeviceChangeKind::SyncLockTaken,
            DeviceChangeKind::BackupEncryptionDisabled
        ]
    );
    let purposes: Vec<_> = record.commands.iter().map(|c| c.purpose).collect();
    assert_eq!(
        purposes,
        [
            AcqCommandPurpose::EnableEncryption,
            AcqCommandPurpose::Backup,
            AcqCommandPurpose::RestoreEncryption
        ]
    );
    assert!(record.commands.iter().all(|c| c.exit_code == Some(0)));
    assert_eq!(
        record.commands[0].argv[1..],
        ["-u", UDID, "encryption", "on"]
    );
    assert_eq!(
        record.commands[2].argv[1..],
        ["-u", UDID, "encryption", "off"]
    );
    // One clock read: the record's start is the first command's recorded start.
    assert_eq!(record.started_at, Some(record.commands[0].started_at));
    assert!(
        record
            .commands
            .windows(2)
            .all(|w| w[0].started_at <= w[1].started_at)
    );
    // The backup was encrypted, and the device is back to unencrypted.
    assert_eq!(lab.state()["will_encrypt"], false);
}

#[test]
fn restore_fail() {
    let lab = Lab::new("restore_fail");
    let (_, outcome, events) = simple(&lab, Some(PASSWORD));
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        AcqStatus::Succeeded,
        &[],
        &["encryption_restore_failed", "encryption_left_enabled"],
    );
    assert_eq!(record.encryption.restored_after, RestoreState::Failed);
    assert_eq!(record.encryption.will_encrypt_after_restore, Some(true));
    assert_ne!(record.commands[2].exit_code, Some(0));
    assert_eq!(
        outcome.summary.warnings,
        ["encryption_restore_failed", "encryption_left_enabled"]
    );
}

#[test]
fn enable_fail() {
    let lab = Lab::new("enable_fail");
    let (_, outcome, events) = simple(&lab, Some(PASSWORD));
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        AcqStatus::Failed,
        &["encryption_enable_failed"],
        &[],
    );
    assert!(!record.encryption.enabled_by_examiner);
    assert_eq!(record.encryption.will_encrypt_after_enable, Some(false));
    assert_eq!(record.encryption.restored_after, RestoreState::NotAttempted);
    // Nothing else ran: no backup, no restore.
    let purposes: Vec<_> = record.commands.iter().map(|c| c.purpose).collect();
    assert_eq!(purposes, [AcqCommandPurpose::EnableEncryption]);
    assert_eq!((&record.process, &record.backup_result), (&None, &None));
    assert!(!phases(&events).contains(&AcqPhase::BackingUp));
}

#[test]
fn enable_unknown() {
    let lab = Lab::new("enable_unknown");
    let (_, outcome, events) = simple(&lab, Some(PASSWORD));
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        AcqStatus::Succeeded,
        &[],
        &["encryption_state_unknown"],
    );
    assert_eq!(record.encryption.will_encrypt_after_enable, None);
    assert!(!record.encryption.enabled_by_examiner);
    assert_ne!(record.commands[0].exit_code, Some(0));
    // Treated as enabled for the restore, which was attempted and succeeded.
    assert_eq!(
        record.commands.last().unwrap().purpose,
        AcqCommandPurpose::RestoreEncryption
    );
    assert_eq!(record.encryption.restored_after, RestoreState::Restored);
    // WillEncrypt was never observed true, so no transition is claimed either way.
    let changes: Vec<_> = record.device_changes.iter().map(|c| c.change).collect();
    assert_eq!(changes, [DeviceChangeKind::SyncLockTaken]);
}

#[test]
fn an_enable_reported_successful_but_unconfirmed_is_unknown_and_restored() {
    // `encryption on` exits 0, but the next WillEncrypt read still says false.
    let lab = Lab::new("enable_unconfirmed");
    let (_, outcome, events) = simple(&lab, Some(PASSWORD));
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        AcqStatus::Succeeded,
        &[],
        &["encryption_state_unknown"],
    );
    assert_eq!(record.commands[0].exit_code, Some(0));
    assert_eq!(record.encryption.will_encrypt_after_enable, Some(false));
    assert!(!record.encryption.enabled_by_examiner);
    // Not encryption_enable_failed: the restore was attempted, and it turned encryption off.
    let purposes: Vec<_> = record.commands.iter().map(|c| c.purpose).collect();
    assert_eq!(
        purposes,
        [
            AcqCommandPurpose::EnableEncryption,
            AcqCommandPurpose::Backup,
            AcqCommandPurpose::RestoreEncryption
        ]
    );
    assert_eq!(record.encryption.restored_after, RestoreState::Restored);
    assert_eq!(record.encryption.will_encrypt_after_restore, Some(false));
    assert_eq!(lab.state()["will_encrypt"], false);
    let changes: Vec<_> = record.device_changes.iter().map(|c| c.change).collect();
    assert_eq!(changes, [DeviceChangeKind::SyncLockTaken]);
}

#[test]
fn backup_fail_encrypted() {
    let lab = Lab::new("backup_fail_encrypted");
    let (_, outcome, events) = simple(&lab, Some(PASSWORD));
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        AcqStatus::Failed,
        &[
            "nonzero_exit",
            "success_message_missing",
            "manifest_missing",
            "snapshot_not_finished",
        ],
        &[],
    );
    // The restore still ran.
    assert_eq!(record.encryption.restored_after, RestoreState::Restored);
}

#[test]
fn cancel_during_enable() {
    let lab = Lab::new("cancel_during_enable");
    let case = new_case(&lab);
    let (outcome, events) = acquire(
        &lab,
        &case,
        request(&case, Some(PASSWORD)),
        |event, control| {
            if let AcqEvent::DevicePrompt { .. } = event
                && control.phase() == AcqPhase::EnablingEncryption
            {
                control.cancel();
            }
        },
    );
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        AcqStatus::Cancelled,
        &["cancelled_by_user"],
        &[],
    );
    // The enable finished, the backup was skipped, the restore ran.
    assert!(record.encryption.enabled_by_examiner);
    assert_eq!(record.encryption.restored_after, RestoreState::Restored);
    let purposes: Vec<_> = record.commands.iter().map(|c| c.purpose).collect();
    assert_eq!(
        purposes,
        [
            AcqCommandPurpose::EnableEncryption,
            AcqCommandPurpose::RestoreEncryption
        ]
    );
    assert_eq!(record.process, None);
    assert!(!phases(&events).contains(&AcqPhase::BackingUp));
}

#[test]
fn cancel_during_restore_is_ignored() {
    let lab = Lab::new("cancel_during_restore");
    let case = new_case(&lab);
    let (outcome, events) = acquire(
        &lab,
        &case,
        request(&case, Some(PASSWORD)),
        |event, control| {
            if matches!(
                event,
                AcqEvent::Phase {
                    phase: AcqPhase::RestoringEncryption
                }
            ) {
                control.cancel();
            }
        },
    );
    let record = assert_final(&lab, &outcome, &events, AcqStatus::Succeeded, &[], &[]);
    assert_eq!(record.encryption.restored_after, RestoreState::Restored);
    assert!(!record.process.unwrap().cancel_requested);
}

#[test]
fn encryption_can_be_enabled_when_will_encrypt_is_absent() {
    let lab = Lab::new("will_encrypt_absent");
    let (_, outcome, events) = simple(&lab, Some(PASSWORD));
    let record = assert_final(&lab, &outcome, &events, AcqStatus::Succeeded, &[], &[]);
    // Absent means false, as idevicebackup2 treats it.
    assert_eq!(record.encryption.will_encrypt_before, Some(false));
    assert!(record.encryption.enabled_by_examiner);
    assert_eq!(record.encryption.restored_after, RestoreState::Restored);
}

#[test]
fn disconnect_after_enabling_leaves_encryption_enabled() {
    let lab = Lab::new("disconnect");
    let (_, outcome, events) = simple(&lab, Some(PASSWORD));
    let record = assert_final(
        &lab,
        &outcome,
        &events,
        AcqStatus::Failed,
        &[
            "nonzero_exit",
            "device_disconnected",
            "success_message_missing",
            "manifest_missing",
            "snapshot_not_finished",
        ],
        &["encryption_left_enabled"],
    );
    assert_eq!(record.encryption.restored_after, RestoreState::NotAttempted);
    assert!(
        !record
            .commands
            .iter()
            .any(|c| c.purpose == AcqCommandPurpose::RestoreEncryption)
    );
}

#[test]
fn a_cancel_while_preparing_skips_the_device_changes() {
    let lab = Lab::new("success_encrypt");
    let case = new_case(&lab);
    let (outcome, events) = acquire(
        &lab,
        &case,
        request(&case, Some(PASSWORD)),
        |event, control| {
            if matches!(
                event,
                AcqEvent::Phase {
                    phase: AcqPhase::Preparing
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
        AcqStatus::Cancelled,
        &["cancelled_by_user"],
        &[],
    );
    // Nothing touched the device: no enable, no backup, so no restore either.
    assert!(record.commands.is_empty());
    assert!(record.device_changes.is_empty());
    assert!(!record.encryption.enabled_by_examiner);
    assert_eq!(record.encryption.restored_after, RestoreState::NotAttempted);
    assert_eq!(record.started_at, None);
    assert_eq!(record.output.seal.file_count, Some(0));
    assert_eq!(lab.state()["will_encrypt"], false);
}

#[test]
fn a_cancel_while_validating_stops_the_seal() {
    let lab = Lab::new("success");
    let case = new_case(&lab);
    let (outcome, events) = acquire(&lab, &case, request(&case, None), |event, control| {
        if matches!(
            event,
            AcqEvent::Phase {
                phase: AcqPhase::Validating
            }
        ) {
            control.cancel();
        }
    });
    let record = &outcome.record;
    assert_eq!(outcome.write_error, None);
    // The backup had finished: the status stands, only the seal stopped.
    assert_eq!(record.status, AcqStatus::Succeeded);
    assert_eq!(codes(&record.warnings), ["seal_cancelled"]);
    assert_eq!(record.output.seal.status, SealStatus::Cancelled);
    assert_eq!(record.output.seal.manifest, None);
    let dir = acq_dir(&outcome);
    assert!(!dir.join(BACKUP_MANIFEST).exists());
    assert!(
        fs::metadata(dir.join("acquisition.json"))
            .unwrap()
            .permissions()
            .readonly()
    );
    assert!(matches!(events.last(), Some(AcqEvent::Finished { .. })));
    lab.assert_no_temp_dirs();
}

// ---- crash after enable (the runner killed in a child process) ----

const CRASH_CHILD_DIR: &str = "SUITEDFIR_TEST_CRASH_CHILD_DIR";

/// The runner that `crash_after_enable_is_recovered` kills: an acquisition with encryption in the
/// `crash_after_enable` scenario (the enable succeeds, then the backup runs until stopped).
#[test]
#[ignore = "runs only as the child process of crash_after_enable_is_recovered"]
fn crash_after_enable_child() {
    let Some(root) = std::env::var_os(CRASH_CHILD_DIR).map(PathBuf::from) else {
        return;
    };
    let tools = root.join("tools");
    let manifest = common::private_tools(&tools);
    let idevice = common::idevice(
        &tools,
        manifest,
        &root.join("cache"),
        vec![
            ("FAKE_IDEVICE_SCENARIO".into(), "crash_after_enable".into()),
            (
                "FAKE_IDEVICE_STATE_DIR".into(),
                root.join("state").into_os_string(),
            ),
        ],
    );
    let case_dir = root.join("case");
    let case = CreatedCase {
        case: case::load(&case_dir).unwrap(),
        path: case_dir,
    };
    let job = acquire::start(&idevice, request(&case, Some(PASSWORD)), context(&case)).unwrap();
    job.run(&mut |_| {});
    panic!("the runner should have been killed");
}

fn running_record(case_dir: &Path) -> Option<AcquisitionRecord> {
    let entry = fs::read_dir(case_dir.join("acquisitions"))
        .ok()?
        .next()?
        .ok()?;
    parse_versioned(&fs::read(entry.path().join("acquisition.json")).ok()?).ok()
}

#[test]
fn crash_after_enable_is_recovered() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("state")).unwrap();
    let fields = suitedfir_core::contracts::CaseFields {
        name: "case".to_owned(),
        ..examples::case_fields()
    };
    let case = case::create(root.path(), &fields).unwrap();
    assert_eq!(case.path, root.path().join("case"));
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_after_enable_child", "--ignored"])
        .env(CRASH_CHILD_DIR, root.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // Kill the runner as soon as the record shows the enable (the rewrite after step 6).
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if running_record(&case.path).is_some_and(|r| r.encryption.enabled_by_examiner) {
            break;
        }
        assert!(Instant::now() < deadline, "the enable was never recorded");
        assert!(child.try_wait().unwrap().is_none(), "the child ended early");
        std::thread::sleep(Duration::from_millis(20));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    // On Unix the backup tool (in its own session) may outlive the runner, as after a real crash
    // (ARCHITECTURE.md §7); stop it. On Windows the job object took it down with the runner.
    #[cfg(unix)]
    {
        let pid_file = root.path().join("state").join("backup.pid");
        let until = Instant::now() + Duration::from_secs(3);
        while !pid_file.exists() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        if let Ok(pid) = fs::read_to_string(&pid_file) {
            let _ = Command::new("kill").args(["-KILL", pid.trim()]).status();
        }
    }

    let before = running_record(&case.path).unwrap();
    assert_eq!(before.status, AcqStatus::Running);
    let now = Timestamp::now();
    let recovered = acquire::recover_case(&case.path, None, now).unwrap();
    assert_eq!(recovered, [before.acq_id.as_str()]);
    let record = acquire::load(&case.path, &before.acq_id).unwrap();
    assert_eq!(record.status, AcqStatus::Interrupted);
    assert_eq!(codes(&record.status_reasons), ["app_interrupted"]);
    assert_eq!(codes(&record.warnings), ["encryption_left_enabled"]);
    assert_eq!(record.recovered_at, Some(now));
    assert_eq!(record.output.seal.status, SealStatus::Interrupted);
    assert_eq!(record.encryption.will_encrypt_after_enable, Some(true));
    assert_ne!(record.encryption.restored_after, RestoreState::Restored);
    let dir = acquire::discover(&case.path).unwrap().remove(0).dir;
    assert!(
        fs::metadata(dir.join("acquisition.json"))
            .unwrap()
            .permissions()
            .readonly()
    );
    assert_eq!(
        acquire::summary(&acquire::discover(&case.path).unwrap()[0]).warnings,
        ["encryption_left_enabled"]
    );
    // The crash left the job's temp dir; the startup sweep removes it.
    let sweep = suitedfir_core::process::sweep_stale_temp(&root.path().join("cache")).unwrap();
    assert!(sweep.failed.is_empty(), "{sweep:?}");
}

// ---- later restore ----

#[test]
fn a_later_restore_writes_encryption_restore_json() {
    let lab = Lab::new("restore_fail");
    let (case, outcome, _) = simple(&lab, Some(PASSWORD));
    assert!(
        outcome
            .summary
            .warnings
            .contains(&"encryption_left_enabled".to_owned())
    );
    let acq_id = outcome.record.acq_id.clone();
    let dir = acq_dir(&outcome);
    let record_bytes = fs::read(dir.join("acquisition.json")).unwrap();

    // Now the device lets encryption be turned off (as after the owner fixed whatever blocked it).
    let idevice = lab.reopen("success");
    let mut lines = Vec::new();
    let result = acquire::restore_later(
        &idevice,
        &case.path,
        &acq_id,
        PASSWORD.to_owned(),
        &mut |l| lines.push(l),
    )
    .unwrap();
    assert!(result.restored);
    assert_eq!(result.will_encrypt_after, Some(false));
    assert!(
        lines
            .iter()
            .any(|l| l.prompt == Some(DevicePromptKind::PasscodeForEncryption))
    );
    let file = dir.join("encryption-restore.json");
    assert!(fs::metadata(&file).unwrap().permissions().readonly());
    let restore: EncryptionRestoreRecord = parse_versioned(&fs::read(&file).unwrap()).unwrap();
    assert_eq!(restore.acq_id, acq_id);
    assert_eq!(restore.argv[1..], ["-u", UDID, "encryption", "off"]);
    assert_eq!(restore.exit_code, Some(0));
    assert_eq!(restore.will_encrypt_after, Some(false));
    assert!(restore.restored);
    assert_eq!(restore.tools.source, IdeviceToolSource::Bundled);
    // acquisition.json is not modified.
    assert_eq!(
        fs::read(dir.join("acquisition.json")).unwrap(),
        record_bytes
    );
    // A second later restore is not applicable: it is already recorded.
    let err = acquire::restore_later(
        &idevice,
        &case.path,
        &acq_id,
        PASSWORD.to_owned(),
        &mut |_| {},
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::RestoreNotApplicable);
    assert!(matches!(
        err,
        AcqError::RestoreNotApplicable(RestoreRefusal::AlreadyRecorded)
    ));
    // The message never claims that nothing is left to turn off.
    let message = err.message();
    assert!(message.contains("already recorded"), "{message}");
    assert!(message.contains("may still be on"), "{message}");
    lab.assert_no_temp_dirs();
}

#[test]
fn a_later_restore_is_refused_without_an_encryption_warning() {
    let lab = Lab::new("success");
    let (case, outcome, _) = simple(&lab, None);
    let err = acquire::restore_later(
        &lab.idevice,
        &case.path,
        &outcome.record.acq_id,
        PASSWORD.to_owned(),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            AcqError::RestoreNotApplicable(RestoreRefusal::NoEncryptionWarning)
        ),
        "{err}"
    );
    assert_eq!(err.code(), ErrorCode::RestoreNotApplicable);
    assert!(!acq_dir(&outcome).join("encryption-restore.json").exists());
    // No device command ran for it.
    let calls = lab.calls();
    assert!(
        !calls.iter().any(|c| c.iter().any(|a| a == "off")),
        "{calls:?}"
    );
    let err = acquire::restore_later(
        &lab.idevice,
        &case.path,
        "20270101-000000Z-ios-000000",
        PASSWORD.to_owned(),
        &mut |_| {},
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::AcqNotFound);
}

// ---- start checks (failures create nothing) and preflight ----

fn acquisitions_created(case: &CreatedCase) -> usize {
    fs::read_dir(case.path.join("acquisitions"))
        .map(|entries| entries.count())
        .unwrap_or(0)
}

#[test]
fn start_checks_create_nothing() {
    let refused = |scenario: &str, env: &[(&str, &str)], edit: &dyn Fn(&mut AcqRequest)| {
        let lab = Lab::with_env(scenario, env);
        let case = new_case(&lab);
        let mut req = request(&case, None);
        edit(&mut req);
        let err = acquire::start(&lab.idevice, req, context(&case)).unwrap_err();
        assert_eq!(acquisitions_created(&case), 0, "{scenario}: {err}");
        lab.assert_no_temp_dirs();
        (err.code(), lab)
    };
    let with_password = |password: &str| {
        let password = password.to_owned();
        move |req: &mut AcqRequest| {
            req.enable_encryption = true;
            req.encryption_password = Some(password.clone());
        }
    };
    assert_eq!(
        refused("success", &[], &with_password("abc")).0,
        ErrorCode::EncryptionPasswordRequired
    );
    assert_eq!(
        refused("success", &[], &|req| req.enable_encryption = true).0,
        ErrorCode::EncryptionPasswordRequired
    );
    assert_eq!(
        refused("already_encrypted", &[], &with_password(PASSWORD)).0,
        ErrorCode::EncryptionAlreadyOn
    );
    let (code, lab) = refused("not_paired", &[], &|_| {});
    assert_eq!(code, ErrorCode::DeviceNotPaired);
    assert!(lab.pairing_attempts().is_empty(), "acq_start never pairs");
    assert_eq!(
        refused(
            "success",
            &[("FAKE_IDEVICE_DATA_USED", "18446744073709551615")],
            &|_| {}
        )
        .0,
        ErrorCode::InsufficientSpace
    );
    assert_eq!(
        refused("usbmuxd_missing", &[], &|_| {}).0,
        ErrorCode::UsbmuxdUnavailable
    );
    assert_eq!(
        refused("success", &[], &|req| req.udid =
            "00008101-000A1B2C3D4E00FF".to_owned())
        .0,
        ErrorCode::DeviceNotFound
    );
    assert_eq!(
        refused("success", &[], &|req| req.udid = "../x".to_owned()).0,
        ErrorCode::DeviceNotFound
    );
}

/// The Windows tools read the password in the ANSI code page: a password that is not printable
/// ASCII is refused before any device change, for enabling and for a later restore.
#[cfg(windows)]
#[test]
fn windows_refuses_passwords_the_tools_cannot_read() {
    const NON_ASCII: &str = "Pässwört-1";
    let lab = Lab::new("success_encrypt");
    let case = new_case(&lab);
    let err = acquire::start(
        &lab.idevice,
        request(&case, Some(NON_ASCII)),
        context(&case),
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    let app: suitedfir_core::contracts::AppError = err.into();
    assert!(app.message.contains("ANSI code page"), "{}", app.message);
    assert!(!format!("{app:?}").contains(NON_ASCII));
    assert_eq!(acquisitions_created(&case), 0);
    assert!(lab.calls().is_empty(), "refused before any device command");

    // A later restore of an acquisition that left encryption on.
    let lab = Lab::new("restore_fail");
    let (case, outcome, _) = simple(&lab, Some(PASSWORD));
    let calls_before = lab.calls().len();
    let err = acquire::restore_later(
        &lab.reopen("success"),
        &case.path,
        &outcome.record.acq_id,
        NON_ASCII.to_owned(),
        &mut |_| {},
    )
    .unwrap_err();
    assert_eq!(err.code(), ErrorCode::InvalidInput, "{err}");
    assert_eq!(lab.calls().len(), calls_before, "no device command");
    assert!(!acq_dir(&outcome).join("encryption-restore.json").exists());
}

#[cfg(windows)]
#[test]
fn windows_refuses_paths_the_tools_cannot_use() {
    let lab = Lab::new("success");
    let long = lab.root.path().join("c".repeat(120));
    let non_ascii = lab.root.path().join("Fälle");
    for case_dir in [long, non_ascii] {
        let case = CreatedCase {
            path: case_dir.clone(),
            case: examples::case_file(),
        };
        let err = acquire::start(&lab.idevice, request(&case, None), context(&case)).unwrap_err();
        assert_eq!(err.code(), ErrorCode::PathNotSupportedByTool, "{err}");
        assert!(!case_dir.exists(), "nothing created");
    }
    assert!(lab.calls().is_empty(), "refused before any device command");
}

#[test]
fn preflight_levels_from_the_device() {
    let lab = Lab::new("success");
    let case = new_case(&lab);
    let preflight = acquire::preflight(&lab.idevice, &case.path, UDID).unwrap();
    assert_eq!(preflight.required_bytes, Some(2 * 1024 * 1024));
    assert_eq!(preflight.level, PreflightLevel::Ok);
    assert!(preflight.free_bytes > 0);

    let lab = Lab::with_env(
        "success",
        &[("FAKE_IDEVICE_DATA_USED", "18446744073709551615")],
    );
    let preflight = acquire::preflight(&lab.idevice, &case.path, UDID).unwrap();
    assert_eq!(preflight.level, PreflightLevel::Block);

    // An unpaired device is not asked for its disk usage (that would pair it).
    let lab = Lab::new("not_paired");
    let preflight = acquire::preflight(&lab.idevice, &case.path, UDID).unwrap();
    assert_eq!(preflight.required_bytes, None);
    assert_eq!(preflight.level, PreflightLevel::Warn);
    assert!(lab.pairing_attempts().is_empty());
    lab.assert_no_temp_dirs();
}

#[test]
fn an_acquisition_after_pairing_records_the_pairing() {
    let lab = Lab::new("not_paired");
    assert_eq!(
        lab.idevice.pair(UDID, None).unwrap().pair_state,
        suitedfir_core::contracts::PairState::AwaitingTrust
    );
    assert_eq!(
        lab.idevice.pair(UDID, None).unwrap().pair_state,
        suitedfir_core::contracts::PairState::Paired
    );
    let (_, outcome, events) = simple(&lab, None);
    let record = assert_final(&lab, &outcome, &events, AcqStatus::Succeeded, &[], &[]);
    assert!(!record.pairing.paired_before);
    assert_eq!(
        record.pairing.paired_by_app_at,
        lab.idevice.paired_by_app_at(UDID)
    );
    let first = &record.device_changes[0];
    assert_eq!(first.change, DeviceChangeKind::PairRecordCreated);
    assert_eq!(Some(first.at), record.pairing.paired_by_app_at);
}

// ---- the password never leaks ----

/// Captures every log record of this test binary.
struct CaptureLog;

static LOGGED: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

impl log::Log for CaptureLog {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        LOGGED
            .get_or_init(Mutex::default)
            .lock()
            .unwrap()
            .push(format!("{} {}", record.target(), record.args()));
    }
    fn flush(&self) {}
}

static LOGGER: CaptureLog = CaptureLog;

/// Every file under `dir`, with its bytes.
fn all_files(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
    }
    files
}

#[test]
fn the_password_never_leaks() {
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Trace);
    let needle = Password::new(PASSWORD.to_owned());
    let lab = Lab::new("restore_fail");
    let case = new_case(&lab);
    let req = request(&case, Some(PASSWORD));
    assert!(!needle.is_in(&format!("{req:?}")), "AcqRequest Debug");
    let job = acquire::start(&lab.idevice, req, context(&case)).unwrap();
    assert!(!needle.is_in(&format!("{job:?}")), "AcqJob Debug");
    let mut events = Vec::new();
    let outcome = job.run(&mut |event| events.push(event));
    assert_eq!(outcome.record.warnings[1].code, "encryption_left_enabled");

    // The tools never saw it in argv (the fake fails if they do) and neither did the record.
    assert_eq!(lab.state()["password_in_argv"], false);
    for call in lab.calls() {
        assert!(!needle.is_in(&call.join(" ")), "argv {call:?}");
    }
    assert!(!needle.is_in(&format!("{outcome:?}")), "AcqOutcome Debug");
    assert!(!needle.is_in(&serde_json::to_string(&outcome.record).unwrap()));
    assert!(!needle.is_in(&format!("{events:?}")), "events");
    // A later restore with it, and one refused.
    let idevice = lab.reopen("success");
    let restored = acquire::restore_later(
        &idevice,
        &case.path,
        &outcome.record.acq_id,
        PASSWORD.to_owned(),
        &mut |line| assert!(!needle.is_in(&line.text)),
    )
    .unwrap();
    assert!(!needle.is_in(&format!("{restored:?}")));
    let refused = acquire::restore_later(
        &idevice,
        &case.path,
        &outcome.record.acq_id,
        PASSWORD.to_owned(),
        &mut |_| {},
    )
    .unwrap_err();
    let app: suitedfir_core::contracts::AppError = refused.into();
    assert!(!needle.is_in(&format!("{app} {app:?}")), "errors");
    let short =
        acquire::start(&lab.idevice, request(&case, Some("pw!")), context(&case)).unwrap_err();
    assert!(!format!("{short} {short:?}").contains("pw!"));
    // Every file the acquisition wrote: records, logs, device-info.plist, the manifest.
    for (path, bytes) in all_files(&case.path) {
        assert!(
            !needle.is_in(&String::from_utf8_lossy(&bytes)),
            "{} contains the password",
            path.display()
        );
    }
    // The app log: the job and the later restore logged their encryption commands (argv and
    // exit), and none of it carries the password.
    let logged = LOGGED.get_or_init(Mutex::default).lock().unwrap().clone();
    let ours: Vec<&String> = logged
        .iter()
        .filter(|line| line.contains(&outcome.record.acq_id))
        .collect();
    assert!(
        ours.iter().any(|l| l.contains("encryption on")),
        "the enable command was logged: {ours:?}"
    );
    assert!(
        ours.iter().filter(|l| l.contains("encryption off")).count() >= 2,
        "the restore and the later restore were logged: {ours:?}"
    );
    assert!(ours.iter().any(|l| l.contains("exited with")), "{ours:?}");
    for line in &logged {
        assert!(!needle.is_in(line), "log: {line}");
    }
}
