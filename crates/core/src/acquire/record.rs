//! The `acquisition.json` lifecycle (CONTRACTS.md §13.3): acquisition ids and folders, the initial
//! write, atomic rewrites while the job runs, finalize (atomic, then read-only), recovery on case
//! open, discovery and listing summaries, the later-restore attempt files
//! (`encryption-restore[-N].json`), and the `input.acquisition_id` helper for runs.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::AcqError;
use crate::contracts::{
    AcqStatus, AcqSummary, AcquisitionRecord, EncryptionRestoreRecord, Reason, SealStatus,
    Timestamp, VersionedFile, parse_versioned,
};
use crate::fsutil;

/// The folder holding a case's acquisitions.
pub const ACQUISITIONS_DIR: &str = "acquisitions";
/// The record inside each acquisition folder.
pub const ACQ_FILE: &str = "acquisition.json";
/// The folder `idevicebackup2` writes into (`backup/<udid>/`).
pub const BACKUP_DIR: &str = "backup";
/// The manifest of `backup/**`, next to it (CONTRACTS.md §8 format).
pub const BACKUP_MANIFEST: &str = "backup.sha256";
/// The full `ideviceinfo -x` output (never logged).
pub const DEVICE_INFO_FILE: &str = "device-info.plist";
/// Written by a later `acq_restore_encryption`.
pub const RESTORE_FILE: &str = "encryption-restore.json";
pub const STDOUT_LOG: &str = "idevicebackup2.stdout.log";
pub const STDERR_LOG: &str = "idevicebackup2.stderr.log";

/// The reason recorded by recovery.
pub const APP_INTERRUPTED: &str = "app_interrupted";

/// Fresh ids tried when a folder name is taken.
const ACQ_ID_ATTEMPTS: u32 = 16;

// ---- ids and folders ----

/// A new acquisition id, `YYYYMMDD-HHMMSSZ-ios-<6 lowercase hex>` (UTC).
pub fn new_acq_id(created_at: Timestamp) -> io::Result<String> {
    crate::idevice::new_ios_id(created_at)
}

/// Whether `id` has the acquisition id format with a real date and time. Commands check this
/// before using an id as a folder name (ARCHITECTURE.md §9).
pub fn is_acq_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('-').collect();
    let [date, time, "ios", random] = parts.as_slice() else {
        return false;
    };
    // The same date, time and suffix rules as run ids.
    crate::run::record::is_run_id(&format!("{date}-{time}-ileapp-{random}"))
}

/// `<case>/acquisitions/<acq_id>`.
pub fn acq_dir(case_dir: &Path, acq_id: &str) -> PathBuf {
    case_dir.join(ACQUISITIONS_DIR).join(acq_id)
}

/// Creates `<case>/acquisitions/<acq_id>/` with a fresh id (lifecycle step 5) and returns both. A
/// plain `create_dir` is used, so an id is never reused.
pub fn create_acq_dir(
    case_dir: &Path,
    created_at: Timestamp,
) -> Result<(String, PathBuf), AcqError> {
    let root = case_dir.join(ACQUISITIONS_DIR);
    fs::create_dir_all(&root).map_err(|e| AcqError::io(&root, e))?;
    for _ in 0..ACQ_ID_ATTEMPTS {
        let acq_id = new_acq_id(created_at).map_err(|e| AcqError::io(&root, e))?;
        let dir = root.join(&acq_id);
        match fs::create_dir(&dir) {
            Ok(()) => return Ok((acq_id, dir)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(AcqError::io(&dir, e)),
        }
    }
    Err(AcqError::NoFreeId {
        path: root.display().to_string(),
    })
}

// ---- writing ----

/// The record must live in the folder named after its id.
fn check_folder(acq_dir: &Path, acq_id: &str) -> Result<(), AcqError> {
    let folder = acq_dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if folder == acq_id {
        Ok(())
    } else {
        Err(AcqError::FolderMismatch {
            acq_id: acq_id.to_owned(),
            folder,
        })
    }
}

/// Writes the initial record (`status: running`) into its new folder, atomically. An existing
/// `acquisition.json` is never replaced this way.
pub fn write_initial(acq_dir: &Path, record: &AcquisitionRecord) -> Result<(), AcqError> {
    let file = acq_dir.join(ACQ_FILE);
    match fs::symlink_metadata(&file) {
        Ok(_) => {
            return Err(AcqError::io(
                &file,
                io::Error::new(io::ErrorKind::AlreadyExists, "the record already exists"),
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(AcqError::io(&file, e)),
    }
    write_running(acq_dir, record)
}

/// Rewrites the running record atomically: after every device-changing command (enable, backup
/// start, restore), so a crash leaves a record that shows every change (CONTRACTS.md §13.3). A
/// read-only (final) record is never replaced.
pub fn write_running(acq_dir: &Path, record: &AcquisitionRecord) -> Result<(), AcqError> {
    check_folder(acq_dir, &record.acq_id)?;
    if record.status != AcqStatus::Running {
        return Err(AcqError::NotFinal {
            acq_id: record.acq_id.clone(),
            reason: "a running record has status running",
        });
    }
    let file = acq_dir.join(ACQ_FILE);
    fsutil::write_json_atomic(&file, record).map_err(|e| AcqError::io(&file, e))
}

/// Finalizes an acquisition (lifecycle step 11): sets `ended_at` and `duration_ms`, writes the
/// complete record atomically and makes it read-only. The record must carry a final status (not
/// `running`, and not `interrupted`, which only [`recover`] writes) and no `pending` or
/// `interrupted` seal. An acquisition that is already final on disk is rejected.
pub fn finalize(
    acq_dir: &Path,
    record: &mut AcquisitionRecord,
    ended_at: Timestamp,
) -> Result<(), AcqError> {
    let not_final = |reason| AcqError::NotFinal {
        acq_id: record.acq_id.clone(),
        reason,
    };
    match record.status {
        AcqStatus::Running => return Err(not_final("the status is still running")),
        AcqStatus::Interrupted => {
            return Err(not_final(
                "interrupted acquisitions are finalized by recovery",
            ));
        }
        _ => {}
    }
    match record.output.seal.status {
        SealStatus::Pending => return Err(not_final("the backup seal is still pending")),
        SealStatus::Interrupted => {
            return Err(not_final("an interrupted seal is written only by recovery"));
        }
        _ => {}
    }
    record.ended_at = Some(ended_at);
    record.recovered_at = None;
    record.duration_ms = Some(duration_ms(record.created_at, ended_at));
    write_final(acq_dir, record)
}

/// `ended_at − created_at` in milliseconds (0 if the clock went backwards).
fn duration_ms(created_at: Timestamp, ended_at: Timestamp) -> u64 {
    let millis = (ended_at.as_datetime() - created_at.as_datetime()).whole_milliseconds();
    u64::try_from(millis).unwrap_or(0)
}

/// Adds a warning unless one with the same code is there.
pub(crate) fn add_warning(warnings: &mut Vec<Reason>, code: &str, message: &str) {
    if !warnings.iter().any(|w| w.code == code) {
        warnings.push(Reason {
            code: code.to_owned(),
            message: message.to_owned(),
        });
    }
}

/// Whether the record shows an `encryption on` whose outcome is unknown (ARCHITECTURE.md §6b:
/// treated as enabled for the restore, with `encryption_state_unknown`). That is, the command was
/// started, and afterwards either
/// - `WillEncrypt` was not read or was unreadable (`will_encrypt_after_enable` null: this includes a
///   crash while the command ran), or
/// - `WillEncrypt` still read false, but the tool did not report failure: it exited 0, or no exit
///   code is known (killed by a signal, or its exit could not be observed), so it may have
///   succeeded with the device updating the value only later (IDEVICE-CLI.md §8).
///
/// Only a command that exited with an error code while `WillEncrypt` stayed false is known to have
/// changed nothing, and one that left `WillEncrypt` true is known to have enabled encryption. A
/// command that never started is not in the record.
pub(crate) fn enable_outcome_unknown(record: &AcquisitionRecord) -> bool {
    let Some(enable) = record
        .commands
        .iter()
        .find(|c| c.purpose == crate::contracts::AcqCommandPurpose::EnableEncryption)
    else {
        return false;
    };
    match record.encryption.will_encrypt_after_enable {
        None => true,
        Some(false) => !matches!(enable.exit_code, Some(code) if code != 0),
        Some(true) => false,
    }
}

pub(crate) const LEFT_ENABLED_MESSAGE: &str = "Backup encryption was turned on by the examiner and \
     was not confirmed to be off again; turn it off with the backup password";
pub(crate) const STATE_UNKNOWN_MESSAGE: &str = "The device's backup-encryption setting could not \
     be confirmed after it was changed; turn it off with the backup password if it is on";

/// Recovers an acquisition left `running` by a crash (CONTRACTS.md §13.3): `interrupted` with
/// `app_interrupted`, `recovered_at` set, a `pending` seal becomes `interrupted`, and the
/// encryption warnings are added:
/// - `encryption_left_enabled` if `enabled_by_examiner` and `restored_after` ≠ `restored`;
/// - `encryption_state_unknown` if the enable command ran and its outcome is unknown
///   ([`enable_outcome_unknown`]; ARCHITECTURE.md §6b "Recovery"). Without an enable command
///   nothing on the device was changed, so a null `will_encrypt_after_enable` is not an unknown
///   state.
///
/// Then the record is finalized (atomic write, read-only).
pub fn recover(
    acq_dir: &Path,
    record: &mut AcquisitionRecord,
    now: Timestamp,
) -> Result<(), AcqError> {
    if record.status != AcqStatus::Running {
        return Err(AcqError::AlreadyFinalized {
            acq_id: record.acq_id.clone(),
        });
    }
    record.status = AcqStatus::Interrupted;
    record.status_reasons = vec![Reason {
        code: APP_INTERRUPTED.to_owned(),
        message: "The app stopped before the acquisition finished; the record was recovered when \
                  the case was next opened"
            .to_owned(),
    }];
    record.recovered_at = Some(now);
    record.ended_at = None;
    record.duration_ms = None;
    if record.output.seal.status == SealStatus::Pending {
        record.output.seal.status = SealStatus::Interrupted;
    }
    let encryption = &record.encryption;
    let left_enabled = encryption.enabled_by_examiner
        && encryption.restored_after != crate::contracts::RestoreState::Restored;
    let unknown = enable_outcome_unknown(record);
    if left_enabled {
        add_warning(
            &mut record.warnings,
            "encryption_left_enabled",
            LEFT_ENABLED_MESSAGE,
        );
    }
    if unknown {
        add_warning(
            &mut record.warnings,
            "encryption_state_unknown",
            STATE_UNKNOWN_MESSAGE,
        );
    }
    write_final(acq_dir, record)
}

/// Recovery on `case_open`: every `running` acquisition of the case except `active_acq_id` (this
/// process's active job) is recovered. Returns the recovered ids; one that cannot be recovered is
/// logged and left as it is.
pub fn recover_case(
    case_dir: &Path,
    active_acq_id: Option<&str>,
    now: Timestamp,
) -> Result<Vec<String>, AcqError> {
    let mut recovered = Vec::new();
    for mut acq in discover(case_dir)? {
        if acq.record.status != AcqStatus::Running
            || Some(acq.record.acq_id.as_str()) == active_acq_id
        {
            continue;
        }
        match recover(&acq.dir, &mut acq.record, now) {
            Ok(()) => recovered.push(acq.record.acq_id),
            Err(e) => log::warn!("could not recover acquisition {}: {e}", acq.dir.display()),
        }
    }
    Ok(recovered)
}

/// The final write shared by [`finalize`] and [`recover`]: refuse if the record on disk is already
/// final (read-only, or any status but `running`), then write atomically and mark read-only
/// ([`AcqError::NotReadOnly`] when only that last step fails).
fn write_final(acq_dir: &Path, record: &AcquisitionRecord) -> Result<(), AcqError> {
    write_final_marking(acq_dir, record, fsutil::set_read_only)
}

/// [`write_final`] with the read-only step passed in (tests make it fail).
fn write_final_marking(
    acq_dir: &Path,
    record: &AcquisitionRecord,
    mark_read_only: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<(), AcqError> {
    check_folder(acq_dir, &record.acq_id)?;
    let file = acq_dir.join(ACQ_FILE);
    let already = || AcqError::AlreadyFinalized {
        acq_id: record.acq_id.clone(),
    };
    match fs::symlink_metadata(&file) {
        Ok(meta) if meta.permissions().readonly() => return Err(already()),
        Ok(_) => {
            let on_disk = read_record(&file)?;
            if on_disk.status != AcqStatus::Running {
                return Err(already());
            }
        }
        // The initial write failed (lifecycle step 5): the final record is still written.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(AcqError::io(&file, e)),
    }
    fsutil::write_json_atomic(&file, record).map_err(|e| AcqError::io(&file, e))?;
    mark_read_only(&file).map_err(|source| AcqError::NotReadOnly {
        path: file.display().to_string(),
        source,
    })
}

fn read_record(file: &Path) -> Result<AcquisitionRecord, AcqError> {
    let bytes = fs::read(file).map_err(|e| AcqError::io(file, e))?;
    parse_versioned(&bytes).map_err(|source| AcqError::Invalid {
        path: file.display().to_string(),
        source,
    })
}

// ---- later-restore attempts ----
//
// Every `acq_restore_encryption` attempt writes its own read-only record, with the schema of
// `EncryptionRestoreRecord`: `encryption-restore.json` for the first, then
// `encryption-restore-2.json`, `-3.json`, … (CONTRACTS.md §13.3 "Later restore"). The name is
// reserved before the device is touched ([`reserve_restore_attempt`]) and the file is created with
// no-replace semantics ([`write_restore_attempt`]), so a file is never overwritten or rewritten.
// `acquisition.json` is never touched.

/// The file name of later-restore attempt `n` (1-based): `encryption-restore.json` for 1,
/// `encryption-restore-<n>.json` after that.
pub fn restore_file_name(n: u32) -> String {
    if n <= 1 {
        RESTORE_FILE.to_owned()
    } else {
        format!("encryption-restore-{n}.json")
    }
}

/// The attempt number of a later-restore file name, if `name` is one: 1 for
/// `encryption-restore.json`, `n` for `encryption-restore-<n>.json` with `n` ≥ 2 written without
/// leading zeros. Names are compared ignoring ASCII case, since on a case-insensitive volume
/// (APFS, NTFS) `encryption-restore-2.JSON` is the same file as `encryption-restore-2.json`.
/// Anything else is not an attempt file.
pub fn restore_attempt_number(name: &str) -> Option<u32> {
    let name = name.to_ascii_lowercase();
    if name == RESTORE_FILE {
        return Some(1);
    }
    let digits = name
        .strip_prefix("encryption-restore-")?
        .strip_suffix(".json")?;
    if digits.is_empty() || digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u32>().ok().filter(|&n| n >= 2)
}

/// A later-restore attempt found in an acquisition folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoreAttempt {
    pub number: u32,
    pub file: PathBuf,
    pub record: EncryptionRestoreRecord,
}

/// Every entry named like an attempt file (in any case), readable or not, with its number, in
/// attempt order. For numbering, a name that is taken is never reused.
fn taken_attempts(acq_dir: &Path) -> Result<Vec<(u32, PathBuf)>, AcqError> {
    let entries = match fs::read_dir(acq_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(AcqError::io(acq_dir, e)),
    };
    let mut taken = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| AcqError::io(acq_dir, e))?;
        if let Some(number) = entry.file_name().to_str().and_then(restore_attempt_number) {
            taken.push((number, entry.path()));
        }
    }
    taken.sort();
    Ok(taken)
}

/// The later-restore attempts recorded for acquisition `acq_id` in `acq_dir`, in attempt order.
/// Entries that are not attempt files are ignored; attempt files that cannot be read, are not
/// valid or name another acquisition are skipped with a logged warning.
pub fn restore_attempts(acq_dir: &Path, acq_id: &str) -> Result<Vec<RestoreAttempt>, AcqError> {
    let mut attempts = Vec::new();
    for (number, file) in taken_attempts(acq_dir)? {
        let parsed = fs::read(&file)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                parse_versioned::<EncryptionRestoreRecord>(&bytes).map_err(|e| e.to_string())
            });
        match parsed {
            Ok(record) if record.acq_id == acq_id => attempts.push(RestoreAttempt {
                number,
                file,
                record,
            }),
            Ok(record) => log::warn!(
                "ignoring {}: it names acquisition {:?}",
                file.display(),
                record.acq_id
            ),
            Err(reason) => log::warn!("ignoring {}: {reason}", file.display()),
        }
    }
    Ok(attempts)
}

/// Whether a later-restore attempt of acquisition `acq_id` recorded `restored: true`. Unreadable
/// attempt files count as not restored.
pub fn later_restore_succeeded(acq_dir: &Path, acq_id: &str) -> bool {
    match restore_attempts(acq_dir, acq_id) {
        Ok(attempts) => attempts.iter().any(|a| a.record.restored),
        Err(e) => {
            log::warn!("cannot read the later restores of {acq_id}: {e}");
            false
        }
    }
}

/// The file name chosen for a later-restore attempt, checked before the device is touched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttemptSlot {
    pub number: u32,
    pub file: PathBuf,
}

/// Chooses and checks the file of the next later-restore attempt, before `encryption off` runs:
/// - the number is one above the highest attempt name already taken, in any case (so gaps are not
///   refilled and attempts stay in order); a number past `u32::MAX` is refused;
/// - nothing may exist under that name;
/// - the folder must accept a new file: a probe file is created there with no-replace semantics
///   and removed again.
///
/// An error means the attempt must not run: the device has not been touched.
pub fn reserve_restore_attempt(acq_dir: &Path) -> Result<AttemptSlot, AcqError> {
    let number = taken_attempts(acq_dir)?
        .last()
        .map_or(Some(1), |(last, _)| last.checked_add(1))
        .ok_or_else(|| AcqError::NoFreeId {
            path: acq_dir.display().to_string(),
        })?;
    let file = acq_dir.join(restore_file_name(number));
    match fs::symlink_metadata(&file) {
        Ok(_) => {
            return Err(AcqError::io(
                &file,
                io::Error::new(io::ErrorKind::AlreadyExists, "the attempt file exists"),
            ));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(AcqError::io(&file, e)),
    }
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).map_err(|e| AcqError::io(acq_dir, io::Error::other(e)))?;
    let probe = acq_dir.join(format!(
        "{}.probe-{}",
        restore_file_name(number),
        crate::hashing::to_hex(&random)
    ));
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|e| AcqError::io(&probe, e))?;
    fs::remove_file(&probe).map_err(|e| AcqError::io(&probe, e))?;
    Ok(AttemptSlot { number, file })
}

/// Writes a later-restore attempt to its reserved file, created with `create_new` (so an existing
/// file is never replaced, whatever else runs), synced, then made read-only. Returns the file.
/// A crash in between leaves a partial file, which reads as an attempt that did not restore.
pub fn write_restore_attempt(
    acq_dir: &Path,
    slot: &AttemptSlot,
    record: &EncryptionRestoreRecord,
) -> Result<PathBuf, AcqError> {
    check_folder(acq_dir, &record.acq_id)?;
    let file = &slot.file;
    let mut bytes = serde_json::to_vec_pretty(record).map_err(|e| AcqError::io(file, e.into()))?;
    bytes.push(b'\n');
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(file)
        .map_err(|e| AcqError::io(file, e))?;
    out.write_all(&bytes)
        .and_then(|()| out.sync_all())
        .map_err(|e| AcqError::io(file, e))?;
    drop(out);
    fsutil::set_read_only(file).map_err(|e| AcqError::io(file, e))?;
    Ok(file.clone())
}

/// [`reserve_restore_attempt`] then [`write_restore_attempt`].
pub fn write_restore_record(
    acq_dir: &Path,
    record: &EncryptionRestoreRecord,
) -> Result<PathBuf, AcqError> {
    check_folder(acq_dir, &record.acq_id)?;
    let slot = reserve_restore_attempt(acq_dir)?;
    write_restore_attempt(acq_dir, &slot, record)
}

// ---- reading ----

/// An acquisition found in a case folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredAcq {
    pub dir: PathBuf,
    pub record: AcquisitionRecord,
}

/// `acq_get`: the record of `acq_id` in the case. `acq_not_found` for an invalid or unknown id.
pub fn load(case_dir: &Path, acq_id: &str) -> Result<AcquisitionRecord, AcqError> {
    if !is_acq_id(acq_id) {
        return Err(AcqError::NotFound(acq_id.to_owned()));
    }
    let dir = acq_dir(case_dir, acq_id);
    let file = dir.join(ACQ_FILE);
    match fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => return Err(AcqError::NotFound(acq_id.to_owned())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(AcqError::NotFound(acq_id.to_owned()));
        }
        Err(e) => return Err(AcqError::io(&dir, e)),
    }
    let record = match read_record(&file) {
        Err(AcqError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            return Err(AcqError::NotFound(acq_id.to_owned()));
        }
        other => other?,
    };
    if record.acq_id != acq_id {
        return Err(AcqError::FolderMismatch {
            acq_id: record.acq_id,
            folder: acq_id.to_owned(),
        });
    }
    Ok(record)
}

/// Finds the acquisitions of a case by scanning `acquisitions/*/acquisition.json`, newest first.
/// Folders whose record is missing, unreadable or invalid, has an invalid id or names another
/// acquisition are skipped with a logged warning. A case without `acquisitions/` has none.
pub fn discover(case_dir: &Path) -> Result<Vec<DiscoveredAcq>, AcqError> {
    let root = case_dir.join(ACQUISITIONS_DIR);
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(AcqError::io(&root, e)),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| AcqError::io(&root, e))?;
        let dir = entry.path();
        // Only real folders: a symlink in acquisitions/ is not an acquisition of this case.
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => {}
            Ok(_) => continue,
            Err(e) => {
                log::warn!("skipping {}: {e}", dir.display());
                continue;
            }
        }
        let folder = entry.file_name().to_string_lossy().into_owned();
        let record = read_record(&dir.join(ACQ_FILE)).and_then(|record| {
            if !is_acq_id(&record.acq_id) || record.acq_id != folder {
                Err(AcqError::FolderMismatch {
                    acq_id: record.acq_id,
                    folder,
                })
            } else {
                Ok(record)
            }
        });
        match record {
            Ok(record) => found.push(DiscoveredAcq { dir, record }),
            Err(reason) => log::warn!("skipping acquisition folder {}: {reason}", dir.display()),
        }
    }
    found.sort_by(|a, b| {
        (b.record.created_at, &b.record.acq_id).cmp(&(a.record.created_at, &a.record.acq_id))
    });
    Ok(found)
}

/// The warning codes that offer "Turn backup encryption off" on the Case screen.
const RESTORE_OFFER_CODES: [&str; 2] = ["encryption_left_enabled", "encryption_state_unknown"];

/// The listing entry of an acquisition (`CaseDetail.acquisitions`, the `finished` event). Its
/// `warnings` are the record's codes, except that once a later restore recorded `restored: true`,
/// `encryption_left_enabled` and `encryption_state_unknown` are left out, so the Case screen stops
/// offering "Turn backup encryption off". `acquisition.json` itself is not changed.
pub fn summary(acq: &DiscoveredAcq) -> AcqSummary {
    let record = &acq.record;
    let restored_later = record
        .warnings
        .iter()
        .any(|w| RESTORE_OFFER_CODES.contains(&w.code.as_str()))
        && later_restore_succeeded(&acq.dir, &record.acq_id);
    let backup_path = (record.status == AcqStatus::Succeeded).then(|| {
        acq.dir
            .join(BACKUP_DIR)
            .join(&record.device.udid)
            .to_string_lossy()
            .into_owned()
    });
    AcqSummary {
        acq_id: record.acq_id.clone(),
        acq_dir: acq.dir.to_string_lossy().into_owned(),
        label: record.label.clone(),
        status: record.status,
        udid: record.device.udid.clone(),
        device_name: record.device.device_name.clone(),
        product_version: record.device.product_version.clone(),
        created_at: record.created_at,
        started_at: record.started_at,
        ended_at: record.ended_at,
        duration_ms: record.duration_ms,
        backup_path,
        warnings: record
            .warnings
            .iter()
            .filter(|w| !(restored_later && RESTORE_OFFER_CODES.contains(&w.code.as_str())))
            .map(|w| w.code.clone())
            .collect(),
    }
}

/// `run.json`'s `input.acquisition_id` (CONTRACTS.md §7.1: "the `acq_id` when the input lies inside
/// a case's `acquisitions/<acq_id>/`"): the id of the acquisition folder that holds `input`, in any
/// of `case_dirs`, else `None`.
///
/// Scope: the caller (E1a's `run_start`) passes every known case folder, the run's own case first,
/// since an acquisition of another known case is a valid input too. A folder counts only if it has
/// an acquisition id as its name and a valid `acquisition.json` naming it. Case folders that no
/// longer exist are skipped. Paths are compared as `fsutil::path_within` does (links resolved).
pub fn acquisition_id_for_input<P: AsRef<Path>>(
    input: &Path,
    case_dirs: impl IntoIterator<Item = P>,
) -> io::Result<Option<String>> {
    for case_dir in case_dirs {
        let case_dir = case_dir.as_ref();
        let root = case_dir.join(ACQUISITIONS_DIR);
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        for entry in entries {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if is_acq_id(&name)
                && fsutil::path_within(input, &entry.path())?
                && load(case_dir, &name).is_ok()
            {
                return Ok(Some(name));
            }
        }
    }
    Ok(None)
}

/// The `schema_version` written in new records.
pub(crate) const SCHEMA_VERSION: u32 = AcquisitionRecord::SCHEMA_VERSION;

#[cfg(test)]
mod tests {
    use super::*;

    use crate::contracts::{
        AcqCommand, AcqCommandPurpose, AcqPairing, ErrorCode, RestoreState, Seal, examples,
    };
    use crate::fsutil::test_support::make_writable;

    const ACQ_ID: &str = "20260924-171200Z-ios-9c01de";

    fn at(text: &str) -> Timestamp {
        Timestamp::parse(text).unwrap()
    }

    fn dir_for(case_dir: &Path, acq_id: &str) -> PathBuf {
        let dir = acq_dir(case_dir, acq_id);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The §13.3 example as it looked while running.
    fn running() -> AcquisitionRecord {
        let mut record = examples::acquisition_record();
        record.status = AcqStatus::Running;
        record.ended_at = None;
        record.duration_ms = None;
        record.backup_result = None;
        record.output.seal = Seal {
            status: SealStatus::Pending,
            manifest: None,
            manifest_sha256: None,
            file_count: None,
            total_bytes: None,
        };
        record
    }

    fn read(dir: &Path) -> AcquisitionRecord {
        read_record(&dir.join(ACQ_FILE)).unwrap()
    }

    fn is_read_only(dir: &Path) -> bool {
        fs::metadata(dir.join(ACQ_FILE))
            .unwrap()
            .permissions()
            .readonly()
    }

    #[test]
    fn acq_ids_have_the_contract_format() {
        let id = new_acq_id(at("2026-09-24T17:12:00Z")).unwrap();
        assert!(id.starts_with("20260924-171200Z-ios-"), "{id}");
        assert_eq!(id.len(), ACQ_ID.len());
        assert!(is_acq_id(&id));
        assert!(is_acq_id(ACQ_ID));
        for bad in [
            "",
            "20260924-171200Z-ileapp-9c01de",
            "20260924-171200Z-ios-9C01DE",
            "20260924-171200Z-ios-9c01d",
            "20260924-171200-ios-9c01de",
            "20261324-171200Z-ios-9c01de",
            "20260924-251200Z-ios-9c01de",
            "../20260924-171200Z-ios-9c01de",
            "20260924-171200Z-ios-9c01de/..",
            "20260924-171200Z-ios-9c01de-x",
        ] {
            assert!(!is_acq_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn create_acq_dir_makes_fresh_folders() {
        let case = tempfile::tempdir().unwrap();
        let created = at("2026-09-24T17:12:00Z");
        let (id, dir) = create_acq_dir(case.path(), created).unwrap();
        assert!(is_acq_id(&id));
        assert_eq!(dir, case.path().join("acquisitions").join(&id));
        assert!(dir.is_dir());
        let (other, _) = create_acq_dir(case.path(), created).unwrap();
        assert_ne!(id, other);
    }

    #[test]
    fn initial_rewrite_finalize_and_only_once() {
        let case = tempfile::tempdir().unwrap();
        let dir = dir_for(case.path(), ACQ_ID);
        let mut record = running();
        write_initial(&dir, &record).unwrap();
        assert!(write_initial(&dir, &record).is_err(), "never replaced");
        record.commands.pop();
        write_running(&dir, &record).unwrap();
        assert_eq!(read(&dir), record);
        assert!(!is_read_only(&dir));

        let example = examples::acquisition_record();
        record.status = AcqStatus::Succeeded;
        record.backup_result = example.backup_result.clone();
        record.output = example.output.clone();
        finalize(&dir, &mut record, at("2026-09-24T17:48:51Z")).unwrap();
        assert_eq!(record.duration_ms, Some(2_211_000));
        assert_eq!(read(&dir), record);
        assert!(is_read_only(&dir));

        let sealed = fs::read(dir.join(ACQ_FILE)).unwrap();
        let mut again = record.clone();
        assert!(matches!(
            finalize(&dir, &mut again, at("2026-09-25T00:00:00Z")).unwrap_err(),
            AcqError::AlreadyFinalized { .. }
        ));
        let mut rewrite = record.clone();
        rewrite.status = AcqStatus::Running;
        assert!(
            write_running(&dir, &rewrite).is_err(),
            "read-only is never replaced"
        );
        assert_eq!(fs::read(dir.join(ACQ_FILE)).unwrap(), sealed);
        make_writable(&dir.join(ACQ_FILE));
    }

    #[test]
    fn a_final_record_that_cannot_be_made_read_only_is_still_final() {
        let case = tempfile::tempdir().unwrap();
        let dir = dir_for(case.path(), ACQ_ID);
        write_initial(&dir, &running()).unwrap();
        let record = examples::acquisition_record();
        // E.g. a share that refuses permission changes.
        let err = write_final_marking(&dir, &record, |_| {
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        })
        .unwrap_err();
        assert!(matches!(err, AcqError::NotReadOnly { .. }), "{err:?}");
        assert_eq!(err.code(), ErrorCode::PermissionDenied);
        // The complete final record is on disk, writable, and recovery leaves it alone.
        assert_eq!(read(&dir), record);
        assert!(!is_read_only(&dir));
        assert_eq!(
            recover_case(case.path(), None, at("2026-09-25T00:00:00Z")).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(read(&dir), record);
    }

    #[test]
    fn finalize_requires_a_final_record_in_its_folder() {
        let case = tempfile::tempdir().unwrap();
        let dir = dir_for(case.path(), ACQ_ID);
        let record = running();
        write_initial(&dir, &record).unwrap();
        let mut still_running = record.clone();
        let mut interrupted = record.clone();
        interrupted.status = AcqStatus::Interrupted;
        let mut pending_seal = record.clone();
        pending_seal.status = AcqStatus::Failed;
        for r in [&mut still_running, &mut interrupted, &mut pending_seal] {
            assert!(matches!(
                finalize(&dir, r, at("2026-09-24T18:00:00Z")).unwrap_err(),
                AcqError::NotFinal { .. }
            ));
        }
        let wrong = dir_for(case.path(), "20260924-171200Z-ios-000000");
        assert!(matches!(
            write_running(&wrong, &record).unwrap_err(),
            AcqError::FolderMismatch { .. }
        ));
        assert_eq!(read(&dir), record);
    }

    fn command(purpose: AcqCommandPurpose) -> AcqCommand {
        AcqCommand {
            purpose,
            argv: vec!["idevicebackup2".to_owned()],
            exit_code: None,
            started_at: at("2026-09-24T17:12:04Z"),
            exited_at: None,
        }
    }

    #[test]
    fn enable_outcome_rules() {
        let with = |exit_code: Option<i32>, after: Option<bool>| {
            let mut record = running();
            let mut enable = command(AcqCommandPurpose::EnableEncryption);
            enable.exit_code = exit_code;
            record.commands = vec![enable];
            record.encryption.will_encrypt_after_enable = after;
            enable_outcome_unknown(&record)
        };
        // Unreadable afterwards, or no exit observed (a crash, an error while waiting).
        assert!(with(Some(0), None));
        assert!(with(Some(-1), None));
        assert!(with(None, None));
        // The tool reported success, but WillEncrypt still reads false.
        assert!(with(Some(0), Some(false)));
        // No exit code (killed by a signal, or the exit was not observed): it may have succeeded
        // with WillEncrypt updating late, so unknown too (review N-c).
        assert!(with(None, Some(false)));
        // Only a command that exited with an error code, with WillEncrypt still false, changed
        // nothing.
        assert!(!with(Some(-22), Some(false)));
        assert!(!with(Some(255), Some(false)));
        // Enabled.
        assert!(!with(Some(0), Some(true)));
        assert!(!with(Some(-1), Some(true)));
        // No enable command at all.
        let mut record = running();
        record.commands = vec![command(AcqCommandPurpose::Backup)];
        record.encryption.will_encrypt_after_enable = None;
        assert!(!enable_outcome_unknown(&record));
    }

    #[test]
    fn recovery_adds_the_encryption_warnings() {
        let case = tempfile::tempdir().unwrap();
        let now = at("2026-09-25T08:00:00Z");
        // Enabled and not restored → left enabled.
        let dir = dir_for(case.path(), ACQ_ID);
        let mut enabled = running();
        enabled.commands = vec![command(AcqCommandPurpose::EnableEncryption)];
        enabled.encryption.restored_after = RestoreState::NotAttempted;
        enabled.encryption.will_encrypt_after_restore = None;
        write_initial(&dir, &enabled).unwrap();
        // The enable outcome unknown → state unknown.
        let unknown_id = "20260924-171300Z-ios-aaaaaa";
        let unknown_dir = dir_for(case.path(), unknown_id);
        let mut unknown = enabled.clone();
        unknown.acq_id = unknown_id.to_owned();
        unknown.encryption.enabled_by_examiner = false;
        unknown.encryption.will_encrypt_after_enable = None;
        write_initial(&unknown_dir, &unknown).unwrap();
        // No encryption change at all → no warnings.
        let plain_id = "20260924-171400Z-ios-bbbbbb";
        let plain_dir = dir_for(case.path(), plain_id);
        let mut plain = running();
        plain.acq_id = plain_id.to_owned();
        plain.commands = vec![command(AcqCommandPurpose::Backup)];
        plain.device_changes.clear();
        plain.encryption.enable_requested = false;
        plain.encryption.enabled_by_examiner = false;
        plain.encryption.will_encrypt_after_enable = None;
        plain.encryption.restore_requested = false;
        plain.encryption.restored_after = RestoreState::NotRequested;
        plain.pairing = AcqPairing {
            paired_before: true,
            paired_by_app_at: None,
            host_id: None,
            system_buid: None,
        };
        write_initial(&plain_dir, &plain).unwrap();
        // The active job is skipped.
        let active_id = "20260924-171500Z-ios-cccccc";
        let active_dir = dir_for(case.path(), active_id);
        let mut active = running();
        active.acq_id = active_id.to_owned();
        write_initial(&active_dir, &active).unwrap();

        let mut recovered = recover_case(case.path(), Some(active_id), now).unwrap();
        recovered.sort();
        assert_eq!(recovered, [ACQ_ID, unknown_id, plain_id]);
        let codes = |dir: &Path| -> Vec<String> {
            read(dir).warnings.into_iter().map(|w| w.code).collect()
        };
        assert_eq!(codes(&dir), ["encryption_left_enabled"]);
        assert_eq!(codes(&unknown_dir), ["encryption_state_unknown"]);
        assert_eq!(codes(&plain_dir), Vec::<String>::new());
        for d in [&dir, &unknown_dir, &plain_dir] {
            let record = read(d);
            assert_eq!(record.status, AcqStatus::Interrupted);
            assert_eq!(record.status_reasons[0].code, APP_INTERRUPTED);
            assert_eq!(record.recovered_at, Some(now));
            assert_eq!((record.ended_at, record.duration_ms), (None, None));
            assert_eq!(record.output.seal.status, SealStatus::Interrupted);
            assert!(is_read_only(d));
        }
        assert_eq!(read(&active_dir).status, AcqStatus::Running);
        // A second open finds nothing to recover.
        assert!(
            recover_case(case.path(), None, now).unwrap().len() == 1,
            "only the active one"
        );
        for d in [&dir, &unknown_dir, &plain_dir, &active_dir] {
            make_writable(&d.join(ACQ_FILE));
        }
    }

    #[test]
    fn discovery_load_and_summary() {
        let case = tempfile::tempdir().unwrap();
        assert!(discover(case.path()).unwrap().is_empty());
        let dir = dir_for(case.path(), ACQ_ID);
        let record = examples::acquisition_record();
        fsutil::write_json_atomic(&dir.join(ACQ_FILE), &record).unwrap();
        let newer_id = "20260925-090000Z-ios-000001";
        let newer = dir_for(case.path(), newer_id);
        let mut failed = record.clone();
        failed.acq_id = newer_id.to_owned();
        failed.status = AcqStatus::Failed;
        failed.created_at = at("2026-09-25T09:00:00Z");
        failed.warnings = vec![Reason {
            code: "encryption_left_enabled".to_owned(),
            message: "m".to_owned(),
        }];
        fsutil::write_json_atomic(&newer.join(ACQ_FILE), &failed).unwrap();
        // Skipped: no record, garbage, a copied record, a file.
        dir_for(case.path(), "20260926-000000Z-ios-000000");
        let bad = dir_for(case.path(), "20260926-000001Z-ios-000000");
        fs::write(bad.join(ACQ_FILE), "{").unwrap();
        let copy = dir_for(case.path(), "copy");
        fsutil::write_json_atomic(&copy.join(ACQ_FILE), &record).unwrap();
        fs::write(case.path().join("acquisitions/notes.txt"), "x").unwrap();

        let found = discover(case.path()).unwrap();
        let ids: Vec<&str> = found.iter().map(|a| a.record.acq_id.as_str()).collect();
        assert_eq!(ids, [newer_id, ACQ_ID]);
        let summary_new = summary(&found[0]);
        assert_eq!(summary_new.backup_path, None, "only succeeded ones");
        assert_eq!(summary_new.warnings, ["encryption_left_enabled"]);
        let summary_old = summary(&found[1]);
        let mut expected = examples::acq_summary();
        expected.acq_dir = dir.to_string_lossy().into_owned();
        expected.backup_path = Some(
            dir.join("backup")
                .join(&record.device.udid)
                .to_string_lossy()
                .into_owned(),
        );
        assert_eq!(summary_old, expected);

        assert_eq!(load(case.path(), ACQ_ID).unwrap(), record);
        for missing in ["20270101-000000Z-ios-000000", "not-an-id", "../x"] {
            assert!(matches!(
                load(case.path(), missing).unwrap_err(),
                AcqError::NotFound(_)
            ));
        }
        assert!(matches!(
            load(case.path(), "20260926-000000Z-ios-000000").unwrap_err(),
            AcqError::NotFound(_)
        ));
    }

    #[test]
    fn acquisition_id_of_an_input() {
        let root = tempfile::tempdir().unwrap();
        let (own, other, gone) = (
            root.path().join("own"),
            root.path().join("other"),
            root.path().join("gone"),
        );
        let find = |input: &Path| {
            acquisition_id_for_input(input, [&own, &gone, &other])
                .unwrap()
                .map(|id| id.to_owned())
        };
        fs::create_dir_all(&own).unwrap();
        assert_eq!(find(&own), None, "no acquisitions/ yet");
        // An acquisition with a valid record, in another known case.
        let dir = dir_for(&other, ACQ_ID);
        fsutil::write_json_atomic(&dir.join(ACQ_FILE), &examples::acquisition_record()).unwrap();
        let backup = dir.join("backup").join("00008101-000A1B2C3D4E001E");
        fs::create_dir_all(&backup).unwrap();
        assert_eq!(find(&backup).as_deref(), Some(ACQ_ID));
        assert_eq!(find(&dir).as_deref(), Some(ACQ_ID));
        // An id-named folder without a valid acquisition.json does not count.
        let no_record = dir_for(&own, "20260925-000000Z-ios-000000");
        fs::create_dir_all(no_record.join("backup")).unwrap();
        assert_eq!(find(&no_record.join("backup")), None);
        fs::write(no_record.join(ACQ_FILE), "{").unwrap();
        assert_eq!(find(&no_record.join("backup")), None);
        // Nor does a record that names another acquisition, or a folder that is not an id.
        let mismatched = dir_for(&own, "20260925-000001Z-ios-000000");
        fsutil::write_json_atomic(&mismatched.join(ACQ_FILE), &examples::acquisition_record())
            .unwrap();
        assert_eq!(find(&mismatched), None);
        fs::create_dir_all(own.join("acquisitions/not-an-acq/x")).unwrap();
        assert_eq!(find(&own.join("acquisitions/not-an-acq/x")), None);
        assert_eq!(find(&own.join("runs")), None);
    }

    #[test]
    fn restore_attempt_file_names() {
        assert_eq!(restore_file_name(1), "encryption-restore.json");
        assert_eq!(restore_file_name(2), "encryption-restore-2.json");
        assert_eq!(restore_file_name(10), "encryption-restore-10.json");
        for n in [1, 2, 3, 10, 999] {
            assert_eq!(restore_attempt_number(&restore_file_name(n)), Some(n));
        }
        // Any case: on APFS or NTFS these are the same files as the lowercase names.
        assert_eq!(restore_attempt_number("encryption-restore-2.JSON"), Some(2));
        assert_eq!(restore_attempt_number("Encryption-Restore.Json"), Some(1));
        assert_eq!(
            restore_attempt_number("ENCRYPTION-RESTORE-12.JSON"),
            Some(12)
        );
        for other in [
            "encryption-restore-1.json",
            "encryption-restore-0.json",
            "encryption-restore-02.json",
            "encryption-restore-.json",
            "encryption-restore-x.json",
            "encryption-restore-2.json.tmp-0123456789abcdef",
            "encryption-restore-2.json.probe-0123456789abcdef",
            "encryption-restore-99999999999.json",
            "acquisition.json",
            "notes.txt",
        ] {
            assert_eq!(restore_attempt_number(other), None, "{other:?}");
        }
    }

    fn attempt(restored: bool) -> EncryptionRestoreRecord {
        let mut record = examples::encryption_restore_record();
        record.restored = restored;
        record.will_encrypt_after = Some(!restored);
        record
    }

    fn make_all_writable(dir: &Path) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                make_writable(&path);
            }
        }
    }

    #[test]
    fn attempts_are_numbered_read_only_and_never_overwritten() {
        let case = tempfile::tempdir().unwrap();
        let dir = dir_for(case.path(), ACQ_ID);
        assert!(restore_attempts(&dir, ACQ_ID).unwrap().is_empty());
        assert!(!later_restore_succeeded(&dir, ACQ_ID));

        let first = write_restore_record(&dir, &attempt(false)).unwrap();
        assert_eq!(first, dir.join("encryption-restore.json"));
        let second = write_restore_record(&dir, &attempt(true)).unwrap();
        assert_eq!(second, dir.join("encryption-restore-2.json"));
        for file in [&first, &second] {
            assert!(fs::metadata(file).unwrap().permissions().readonly());
        }
        let attempts = restore_attempts(&dir, ACQ_ID).unwrap();
        assert_eq!(
            attempts
                .iter()
                .map(|a| (a.number, a.record.restored))
                .collect::<Vec<_>>(),
            [(1, false), (2, true)]
        );
        assert_eq!(
            attempts[1].record,
            attempt(true),
            "the same schema, as written"
        );
        assert!(later_restore_succeeded(&dir, ACQ_ID));
        make_all_writable(&dir);
    }

    #[test]
    fn numbering_skips_gaps_and_taken_names() {
        let case = tempfile::tempdir().unwrap();
        let dir = dir_for(case.path(), ACQ_ID);
        // Attempt 1 exists, 2 is missing, 3 is a partial (unparsable) file left by a crash, and a
        // directory takes the name of attempt 5.
        write_restore_record(&dir, &attempt(false)).unwrap();
        fs::write(dir.join("encryption-restore-3.json"), "{\"schema_ver").unwrap();
        fs::create_dir(dir.join("encryption-restore-5.json")).unwrap();
        let before: Vec<(PathBuf, Vec<u8>)> =
            ["encryption-restore.json", "encryption-restore-3.json"]
                .iter()
                .map(|name| (dir.join(name), fs::read(dir.join(name)).unwrap()))
                .collect();

        let next = write_restore_record(&dir, &attempt(false)).unwrap();
        assert_eq!(
            next,
            dir.join("encryption-restore-6.json"),
            "after the highest taken name"
        );
        for (file, bytes) in &before {
            assert_eq!(
                &fs::read(file).unwrap(),
                bytes,
                "{} untouched",
                file.display()
            );
        }
        assert!(
            !dir.join("encryption-restore-2.json").exists(),
            "gaps are not refilled"
        );
        // Only the readable attempts count; none restored.
        let numbers: Vec<u32> = restore_attempts(&dir, ACQ_ID)
            .unwrap()
            .iter()
            .map(|a| a.number)
            .collect();
        assert_eq!(numbers, [1, 6]);
        assert!(!later_restore_succeeded(&dir, ACQ_ID));
        // Another acquisition's record under an attempt name does not count as a restore.
        let mut foreign = attempt(true);
        foreign.acq_id = "20260924-171200Z-ios-000000".to_owned();
        fs::write(
            dir.join("encryption-restore-7.json"),
            serde_json::to_vec(&foreign).unwrap(),
        )
        .unwrap();
        assert!(!later_restore_succeeded(&dir, ACQ_ID));
        assert_eq!(
            write_restore_record(&dir, &attempt(true)).unwrap(),
            dir.join("encryption-restore-8.json")
        );
        assert!(later_restore_succeeded(&dir, ACQ_ID));
        make_all_writable(&dir);
    }

    /// Review N-a: a stray `encryption-restore-2.JSON` is attempt 2 (on a case-insensitive volume
    /// it is the same file), so the next attempt is 3, and the stray file is untouched.
    #[test]
    fn numbering_counts_attempt_names_in_any_case() {
        let case = tempfile::tempdir().unwrap();
        let dir = dir_for(case.path(), ACQ_ID);
        write_restore_record(&dir, &attempt(false)).unwrap();
        fs::write(dir.join("encryption-restore-2.JSON"), "stray").unwrap();
        let slot = reserve_restore_attempt(&dir).unwrap();
        assert_eq!(slot.number, 3);
        let written = write_restore_attempt(&dir, &slot, &attempt(true)).unwrap();
        assert_eq!(written, dir.join("encryption-restore-3.json"));
        assert_eq!(
            fs::read_to_string(dir.join("encryption-restore-2.JSON")).unwrap(),
            "stray"
        );
        // The stray file counts as an attempt name but not as a restore.
        let numbers: Vec<u32> = restore_attempts(&dir, ACQ_ID)
            .unwrap()
            .iter()
            .map(|a| a.number)
            .collect();
        assert_eq!(numbers, [1, 3]);
        // The reservation probe leaves nothing behind.
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(!names.iter().any(|n| n.contains(".probe-")), "{names:?}");
        make_all_writable(&dir);
    }

    /// Review N-b: the final write uses no-replace semantics, so a file that appeared at the
    /// reserved name (another writer, whatever the one-job rule says) is never replaced.
    #[test]
    fn an_existing_file_at_the_reserved_name_is_never_replaced() {
        let case = tempfile::tempdir().unwrap();
        let dir = dir_for(case.path(), ACQ_ID);
        let slot = reserve_restore_attempt(&dir).unwrap();
        assert_eq!(slot.file, dir.join(RESTORE_FILE));
        fs::write(&slot.file, "written by someone else").unwrap();
        let err = write_restore_attempt(&dir, &slot, &attempt(true)).unwrap_err();
        assert!(
            matches!(&err, AcqError::Io { source, .. } if source.kind() == io::ErrorKind::AlreadyExists),
            "{err:?}"
        );
        assert_eq!(
            fs::read_to_string(&slot.file).unwrap(),
            "written by someone else"
        );
        assert!(!later_restore_succeeded(&dir, ACQ_ID));
    }

    /// Review N-a: when no attempt file can be reserved, the attempt is refused (before the
    /// device is touched, in `restore_later`) and nothing is written.
    #[test]
    fn a_reservation_that_cannot_succeed_is_refused() {
        let case = tempfile::tempdir().unwrap();
        let dir = dir_for(case.path(), ACQ_ID);
        // No number after the highest taken name.
        fs::write(dir.join("encryption-restore-4294967295.json"), "x").unwrap();
        assert!(matches!(
            reserve_restore_attempt(&dir).unwrap_err(),
            AcqError::NoFreeId { .. }
        ));
        fs::remove_file(dir.join("encryption-restore-4294967295.json")).unwrap();
        // A folder that accepts no new file (Unix permissions; unprivileged in CI).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
            let refused = reserve_restore_attempt(&dir);
            let names: Vec<_> = fs::read_dir(&dir).unwrap().collect();
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
            let err = refused.unwrap_err();
            assert!(
                matches!(&err, AcqError::Io { source, .. } if source.kind() == io::ErrorKind::PermissionDenied),
                "{err:?}"
            );
            assert!(names.is_empty(), "nothing written");
        }
    }

    #[test]
    fn discovery_recovery_and_summary_with_attempt_files() {
        let case = tempfile::tempdir().unwrap();
        let dir = dir_for(case.path(), ACQ_ID);
        let mut record = running();
        record.warnings = vec![
            Reason {
                code: "encryption_restore_failed".to_owned(),
                message: "m".to_owned(),
            },
            Reason {
                code: "encryption_left_enabled".to_owned(),
                message: "m".to_owned(),
            },
        ];
        write_initial(&dir, &record).unwrap();
        // Attempt files, a gap, and unrelated files.
        write_restore_record(&dir, &attempt(false)).unwrap();
        fs::write(dir.join("encryption-restore-4.json"), "garbage").unwrap();
        fs::write(dir.join("encryption-restore-x.json"), "{}").unwrap();
        fs::write(dir.join("notes.txt"), "x").unwrap();
        fs::create_dir(dir.join("encryption-restore-9.json")).unwrap();

        let recovered = recover_case(case.path(), None, at("2026-09-25T08:00:00Z")).unwrap();
        assert_eq!(recovered, [ACQ_ID]);
        let found = discover(case.path()).unwrap();
        assert_eq!(found.len(), 1);
        let codes = |summary: &AcqSummary| summary.warnings.clone();
        // A failed attempt keeps the offer.
        assert_eq!(
            codes(&summary(&found[0])),
            ["encryption_restore_failed", "encryption_left_enabled"]
        );
        // A successful one drops it from the listing; acquisition.json is untouched.
        let record_bytes = fs::read(dir.join(ACQ_FILE)).unwrap();
        write_restore_record(&dir, &attempt(true)).unwrap();
        assert_eq!(codes(&summary(&found[0])), ["encryption_restore_failed"]);
        assert_eq!(fs::read(dir.join(ACQ_FILE)).unwrap(), record_bytes);
        assert_eq!(discover(case.path()).unwrap()[0].record, found[0].record);
        make_all_writable(&dir);
    }
}
