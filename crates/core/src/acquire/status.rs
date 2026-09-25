//! The acquisition status rules and warnings of CONTRACTS.md §13.3, and the backup-layout checks
//! of docs/IDEVICE-CLI.md §6.

use std::fs;
use std::io::{self, BufReader};
use std::path::Path;

use crate::contracts::{AcqEncryption, AcqStatus, Reason, RestoreState};

use super::record::{LEFT_ENABLED_MESSAGE, STATE_UNKNOWN_MESSAGE};

/// Free space below this after a backup gives `disk_nearly_full` (the tool does not check its
/// writes, IDEVICE-CLI.md §5).
pub const DISK_NEARLY_FULL: u64 = 1 << 30;

fn reason(code: &str, message: impl Into<String>) -> Reason {
    Reason {
        code: code.to_owned(),
        message: message.into(),
    }
}

/// A short-circuit (§13.3 rule 1): only this reason is recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShortCircuit {
    PrepareFailed(String),
    EncryptionEnableFailed(String),
    /// The message names the command purpose.
    SpawnFailed(String),
    /// The examiner cancelled before the backup's exit was observed.
    Cancelled,
}

/// What `backup/<udid>/` holds after the backup.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub udid_dir_exists: bool,
    /// `Manifest.db`, `Manifest.mbdb`, or `None`.
    pub manifest_found: Option<String>,
    pub info_plist_found: bool,
    pub status_plist_found: bool,
    /// `SnapshotState` from `Status.plist`; `None` if missing or unreadable.
    pub snapshot_state: Option<String>,
}

/// Inspects `udid_dir` (`backup/<udid>`) read-only.
pub fn inspect_layout(udid_dir: &Path) -> Layout {
    let is_file = |name: &str| udid_dir.join(name).is_file();
    if !udid_dir.is_dir() {
        return Layout::default();
    }
    let manifest_found = ["Manifest.db", "Manifest.mbdb"]
        .into_iter()
        .find(|name| is_file(name))
        .map(str::to_owned);
    let status_plist_found = is_file("Status.plist");
    Layout {
        udid_dir_exists: true,
        manifest_found,
        info_plist_found: is_file("Info.plist"),
        status_plist_found,
        snapshot_state: status_plist_found
            .then(|| {
                snapshot_state(&udid_dir.join("Status.plist"))
                    .ok()
                    .flatten()
            })
            .flatten(),
    }
}

/// `SnapshotState` from a `Status.plist` (XML or binary).
fn snapshot_state(path: &Path) -> io::Result<Option<String>> {
    let file = fs::File::open(path)?;
    let value = plist::Value::from_reader(BufReader::new(file)).map_err(io::Error::other)?;
    Ok(value
        .as_dictionary()
        .and_then(|dict| dict.get("SnapshotState"))
        .and_then(plist::Value::as_string)
        .map(str::to_owned))
}

/// Everything the §13.3 checks look at, once the backup has exited.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BackupFacts {
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub final_message: Option<String>,
    pub cancelled_on_device: bool,
    /// The UDID was no longer listed at finalize.
    pub device_gone: bool,
    pub sync_lock_failed: bool,
    pub layout: Layout,
}

/// The status and reasons (§13.3 rules 1-3). Without a short-circuit, `facts` must be present (the
/// backup ran); each matching check adds a reason, in the order of the §13.3 table. The file checks
/// are skipped when `backup/<udid>/` is missing, and the snapshot check when `Status.plist` was not
/// read (their input is unavailable).
pub fn evaluate(
    short: Option<&ShortCircuit>,
    facts: Option<&BackupFacts>,
) -> (AcqStatus, Vec<Reason>) {
    match short {
        Some(ShortCircuit::PrepareFailed(message)) => {
            return (
                AcqStatus::Failed,
                vec![reason("prepare_failed", message.as_str())],
            );
        }
        Some(ShortCircuit::EncryptionEnableFailed(message)) => {
            return (
                AcqStatus::Failed,
                vec![reason("encryption_enable_failed", message.as_str())],
            );
        }
        Some(ShortCircuit::SpawnFailed(message)) => {
            return (
                AcqStatus::Failed,
                vec![reason("spawn_failed", message.as_str())],
            );
        }
        Some(ShortCircuit::Cancelled) => {
            return (
                AcqStatus::Cancelled,
                vec![reason(
                    "cancelled_by_user",
                    "The examiner cancelled the acquisition",
                )],
            );
        }
        None => {}
    }
    let Some(facts) = facts else {
        return (
            AcqStatus::Failed,
            vec![reason("prepare_failed", "The backup did not run")],
        );
    };
    let mut reasons = Vec::new();
    let aborted = facts.final_message.as_deref() == Some(super::output::BACKUP_ABORTED);
    if let Some(code) = facts.exit_code
        && code != 0
    {
        reasons.push(reason(
            "nonzero_exit",
            format!("idevicebackup2 exited with code {code}"),
        ));
    }
    if let Some(signal) = facts.signal {
        reasons.push(reason(
            "killed_by_signal",
            format!("idevicebackup2 was killed by signal {signal}"),
        ));
    }
    if aborted && facts.cancelled_on_device {
        reasons.push(reason(
            "cancelled_on_device",
            "The backup was cancelled on the device",
        ));
    }
    if aborted && facts.device_gone {
        reasons.push(reason(
            "device_disconnected",
            "The device was disconnected during the backup",
        ));
    }
    if facts.sync_lock_failed {
        reasons.push(reason(
            "sync_lock_failed",
            "The device's sync lock could not be taken; quit Finder, iTunes or Apple Devices and \
             try again",
        ));
    }
    if facts.final_message.as_deref() != Some(super::output::BACKUP_SUCCESSFUL) {
        reasons.push(reason(
            "success_message_missing",
            match &facts.final_message {
                Some(message) => format!("idevicebackup2 reported \"{message}\""),
                None => "idevicebackup2 printed no final message".to_owned(),
            },
        ));
    }
    let layout = &facts.layout;
    if !layout.udid_dir_exists {
        reasons.push(reason(
            "backup_dir_missing",
            "The backup folder backup/<udid> does not exist",
        ));
    } else {
        if layout.manifest_found.is_none() {
            reasons.push(reason(
                "manifest_missing",
                "The backup has neither Manifest.db nor Manifest.mbdb",
            ));
        }
        if !layout.info_plist_found {
            reasons.push(reason("info_plist_missing", "The backup has no Info.plist"));
        }
        if !layout.status_plist_found {
            reasons.push(reason(
                "status_plist_missing",
                "The backup has no Status.plist",
            ));
        }
        if let Some(state) = &layout.snapshot_state
            && state != "finished"
        {
            reasons.push(reason(
                "snapshot_not_finished",
                format!("Status.plist has SnapshotState \"{state}\", not \"finished\""),
            ));
        }
    }
    let status = if reasons.is_empty() {
        AcqStatus::Succeeded
    } else {
        AcqStatus::Failed
    };
    (status, reasons)
}

/// Inputs to the warnings that do not come from the encryption block.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WarningFacts {
    /// The enable command ran and its outcome is unknown (`record::enable_outcome_unknown`).
    pub enable_outcome_unknown: bool,
    pub device_file_errors: u32,
    pub free_bytes_after: Option<u64>,
    /// Warnings of the seal (`seal_failed`, `seal_cancelled`, `symlinks_in_backup`,
    /// `unencodable_filename`), in that order.
    pub seal: Vec<Reason>,
}

/// The warnings (§13.3 rule 4), in the order of the table:
/// - `encryption_restore_failed`: the restore ran and `WillEncrypt` stayed true;
/// - `encryption_left_enabled`: the restore failed, or encryption was enabled by the examiner and
///   the restore was not attempted or not requested;
/// - `encryption_state_unknown`: the enable ran and its outcome is unknown, or the restore outcome
///   is unknown;
/// - `backup_encryption_preexisting`: `WillEncrypt` was already true;
/// - `device_file_errors`, `disk_nearly_full`, then the seal's warnings.
pub fn warnings(encryption: &AcqEncryption, facts: &WarningFacts) -> Vec<Reason> {
    let mut warnings = Vec::new();
    let restored = encryption.restored_after;
    if restored == RestoreState::Failed {
        warnings.push(reason(
            "encryption_restore_failed",
            "Turning backup encryption off again failed",
        ));
    }
    if restored == RestoreState::Failed
        || (encryption.enabled_by_examiner
            && matches!(
                restored,
                RestoreState::NotAttempted | RestoreState::NotRequested
            ))
    {
        warnings.push(reason("encryption_left_enabled", LEFT_ENABLED_MESSAGE));
    }
    if facts.enable_outcome_unknown || restored == RestoreState::Unknown {
        warnings.push(reason("encryption_state_unknown", STATE_UNKNOWN_MESSAGE));
    }
    if encryption.will_encrypt_before == Some(true) {
        warnings.push(reason(
            "backup_encryption_preexisting",
            "Backup encryption was already on, so parsing the backup needs the owner's backup \
             password",
        ));
    }
    if facts.device_file_errors > 0 {
        warnings.push(reason(
            "device_file_errors",
            format!(
                "The device reported an error for {} file(s)",
                facts.device_file_errors
            ),
        ));
    }
    if let Some(free) = facts.free_bytes_after
        && free < DISK_NEARLY_FULL
    {
        warnings.push(reason(
            "disk_nearly_full",
            format!("Only {free} bytes are free on the case volume after the backup"),
        ));
    }
    warnings.extend(facts.seal.iter().cloned());
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::contracts::PasswordChannel;

    fn codes(reasons: &[Reason]) -> Vec<&str> {
        reasons.iter().map(|r| r.code.as_str()).collect()
    }

    fn good_layout() -> Layout {
        Layout {
            udid_dir_exists: true,
            manifest_found: Some("Manifest.db".to_owned()),
            info_plist_found: true,
            status_plist_found: true,
            snapshot_state: Some("finished".to_owned()),
        }
    }

    fn success() -> BackupFacts {
        BackupFacts {
            exit_code: Some(0),
            final_message: Some("Backup Successful.".to_owned()),
            layout: good_layout(),
            ..BackupFacts::default()
        }
    }

    #[test]
    fn short_circuits_record_only_their_reason() {
        let facts = success();
        for (short, status, code) in [
            (
                ShortCircuit::PrepareFailed("disk full".into()),
                AcqStatus::Failed,
                "prepare_failed",
            ),
            (
                ShortCircuit::EncryptionEnableFailed("x".into()),
                AcqStatus::Failed,
                "encryption_enable_failed",
            ),
            (
                ShortCircuit::SpawnFailed("backup: not found".into()),
                AcqStatus::Failed,
                "spawn_failed",
            ),
            (
                ShortCircuit::Cancelled,
                AcqStatus::Cancelled,
                "cancelled_by_user",
            ),
        ] {
            let (got, reasons) = evaluate(Some(&short), Some(&facts));
            assert_eq!((got, codes(&reasons)), (status, vec![code]));
        }
    }

    /// The rows of CONTRACTS.md §13.4 without encryption, as the fake produces them.
    #[test]
    fn contract_rows() {
        let partial = Layout {
            udid_dir_exists: true,
            manifest_found: None,
            info_plist_found: true,
            status_plist_found: true,
            snapshot_state: Some("new".to_owned()),
        };
        let rows: Vec<(&str, BackupFacts, AcqStatus, Vec<&str>)> = vec![
            ("success", success(), AcqStatus::Succeeded, vec![]),
            (
                "backup_fail",
                BackupFacts {
                    exit_code: Some(151),
                    final_message: Some("Backup Failed (Error Code 105).".into()),
                    layout: partial.clone(),
                    ..BackupFacts::default()
                },
                AcqStatus::Failed,
                vec![
                    "nonzero_exit",
                    "success_message_missing",
                    "manifest_missing",
                    "snapshot_not_finished",
                ],
            ),
            (
                "incomplete",
                BackupFacts {
                    exit_code: Some(0),
                    final_message: Some("Backup Failed (Error Code 0).".into()),
                    layout: Layout {
                        snapshot_state: Some("new".into()),
                        ..good_layout()
                    },
                    ..BackupFacts::default()
                },
                AcqStatus::Failed,
                vec!["success_message_missing", "snapshot_not_finished"],
            ),
            (
                "cancel_on_device",
                BackupFacts {
                    exit_code: Some(255),
                    final_message: Some("Backup Aborted.".into()),
                    cancelled_on_device: true,
                    layout: partial.clone(),
                    ..BackupFacts::default()
                },
                AcqStatus::Failed,
                vec![
                    "nonzero_exit",
                    "cancelled_on_device",
                    "success_message_missing",
                    "manifest_missing",
                    "snapshot_not_finished",
                ],
            ),
            (
                "disconnect",
                BackupFacts {
                    exit_code: Some(255),
                    final_message: Some("Backup Aborted.".into()),
                    device_gone: true,
                    layout: partial.clone(),
                    ..BackupFacts::default()
                },
                AcqStatus::Failed,
                vec![
                    "nonzero_exit",
                    "device_disconnected",
                    "success_message_missing",
                    "manifest_missing",
                    "snapshot_not_finished",
                ],
            ),
            (
                "sync_lock",
                BackupFacts {
                    exit_code: Some(255),
                    sync_lock_failed: true,
                    ..BackupFacts::default()
                },
                AcqStatus::Failed,
                vec![
                    "nonzero_exit",
                    "sync_lock_failed",
                    "success_message_missing",
                    "backup_dir_missing",
                ],
            ),
            (
                "killed",
                BackupFacts {
                    signal: Some(9),
                    layout: good_layout(),
                    ..BackupFacts::default()
                },
                AcqStatus::Failed,
                vec!["killed_by_signal", "success_message_missing"],
            ),
            (
                "files_missing",
                BackupFacts {
                    layout: Layout {
                        udid_dir_exists: true,
                        ..Layout::default()
                    },
                    ..success()
                },
                AcqStatus::Failed,
                vec![
                    "manifest_missing",
                    "info_plist_missing",
                    "status_plist_missing",
                ],
            ),
        ];
        for (name, facts, status, expected) in rows {
            let (got, reasons) = evaluate(None, Some(&facts));
            assert_eq!((got, codes(&reasons)), (status, expected), "{name}");
        }
        // A device gone without an abort is not a disconnect during the backup.
        let (_, reasons) = evaluate(
            None,
            Some(&BackupFacts {
                device_gone: true,
                ..success()
            }),
        );
        assert!(reasons.is_empty());
    }

    #[test]
    fn inspects_the_layout() {
        let dir = tempfile::tempdir().unwrap();
        let udid_dir = dir.path().join("udid");
        assert_eq!(inspect_layout(&udid_dir), Layout::default());
        fs::create_dir(&udid_dir).unwrap();
        fs::write(udid_dir.join("Manifest.mbdb"), "x").unwrap();
        fs::write(udid_dir.join("Info.plist"), "x").unwrap();
        let mut status = plist::Dictionary::new();
        status.insert(
            "SnapshotState".into(),
            plist::Value::String("finished".into()),
        );
        plist::Value::Dictionary(status)
            .to_file_binary(udid_dir.join("Status.plist"))
            .unwrap();
        assert_eq!(
            inspect_layout(&udid_dir),
            Layout {
                udid_dir_exists: true,
                manifest_found: Some("Manifest.mbdb".to_owned()),
                info_plist_found: true,
                status_plist_found: true,
                snapshot_state: Some("finished".to_owned()),
            }
        );
        fs::write(udid_dir.join("Manifest.db"), "x").unwrap();
        fs::write(udid_dir.join("Status.plist"), "garbage").unwrap();
        let layout = inspect_layout(&udid_dir);
        assert_eq!(layout.manifest_found.as_deref(), Some("Manifest.db"));
        assert!(layout.status_plist_found);
        assert_eq!(layout.snapshot_state, None, "unreadable");
    }

    fn encryption() -> AcqEncryption {
        AcqEncryption {
            will_encrypt_before: Some(false),
            enable_requested: true,
            enabled_by_examiner: true,
            will_encrypt_after_enable: Some(true),
            restore_requested: true,
            restored_after: RestoreState::Restored,
            will_encrypt_after_restore: Some(false),
            password_supplied: true,
            password_channel: Some(PasswordChannel::Env),
        }
    }

    #[test]
    fn encryption_warnings() {
        let attempted = WarningFacts::default();
        let outcome_unknown = WarningFacts {
            enable_outcome_unknown: true,
            ..WarningFacts::default()
        };
        // success_encrypt
        assert!(warnings(&encryption(), &attempted).is_empty());
        // restore_fail
        let mut failed = encryption();
        failed.restored_after = RestoreState::Failed;
        failed.will_encrypt_after_restore = Some(true);
        assert_eq!(
            codes(&warnings(&failed, &attempted)),
            ["encryption_restore_failed", "encryption_left_enabled"]
        );
        // enable_unknown (or an unconfirmed enable), restored afterwards
        let mut unknown = encryption();
        unknown.enabled_by_examiner = false;
        unknown.will_encrypt_after_enable = None;
        assert_eq!(
            codes(&warnings(&unknown, &outcome_unknown)),
            ["encryption_state_unknown"]
        );
        // the restore outcome unknown
        let mut restore_unknown = encryption();
        restore_unknown.restored_after = RestoreState::Unknown;
        restore_unknown.will_encrypt_after_restore = None;
        assert_eq!(
            codes(&warnings(&restore_unknown, &attempted)),
            ["encryption_state_unknown"]
        );
        // disconnect after enabling: the restore was not attempted
        let mut not_attempted = encryption();
        not_attempted.restored_after = RestoreState::NotAttempted;
        not_attempted.will_encrypt_after_restore = None;
        assert_eq!(
            codes(&warnings(&not_attempted, &attempted)),
            ["encryption_left_enabled"]
        );
        // enabled, restore not requested
        let mut kept = encryption();
        kept.restore_requested = false;
        kept.restored_after = RestoreState::NotRequested;
        assert_eq!(
            codes(&warnings(&kept, &attempted)),
            ["encryption_left_enabled"]
        );
        // enable_fail: nothing to restore, nothing left enabled
        let mut enable_failed = encryption();
        enable_failed.enabled_by_examiner = false;
        enable_failed.will_encrypt_after_enable = Some(false);
        enable_failed.restored_after = RestoreState::NotAttempted;
        assert!(warnings(&enable_failed, &attempted).is_empty());
        // already_encrypted, no change requested
        let preexisting = AcqEncryption {
            will_encrypt_before: Some(true),
            enable_requested: false,
            enabled_by_examiner: false,
            will_encrypt_after_enable: None,
            restore_requested: false,
            restored_after: RestoreState::NotRequested,
            will_encrypt_after_restore: None,
            password_supplied: false,
            password_channel: None,
        };
        assert_eq!(
            codes(&warnings(&preexisting, &WarningFacts::default())),
            ["backup_encryption_preexisting"]
        );
    }

    #[test]
    fn other_warnings_in_table_order() {
        let facts = WarningFacts {
            enable_outcome_unknown: false,
            device_file_errors: 3,
            free_bytes_after: Some(DISK_NEARLY_FULL - 1),
            seal: vec![reason("symlinks_in_backup", "1 link")],
        };
        let plain = AcqEncryption {
            will_encrypt_before: Some(false),
            enable_requested: false,
            enabled_by_examiner: false,
            will_encrypt_after_enable: None,
            restore_requested: false,
            restored_after: RestoreState::NotRequested,
            will_encrypt_after_restore: None,
            password_supplied: false,
            password_channel: None,
        };
        assert_eq!(
            codes(&warnings(&plain, &facts)),
            [
                "device_file_errors",
                "disk_nearly_full",
                "symlinks_in_backup"
            ]
        );
        let roomy = WarningFacts {
            free_bytes_after: Some(DISK_NEARLY_FULL),
            ..WarningFacts::default()
        };
        assert!(warnings(&plain, &roomy).is_empty());
    }
}
