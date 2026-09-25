//! Parsing `idevicebackup2 backup` output while it runs (docs/IDEVICE-CLI.md §5): overall progress
//! only from `] NN% Finished` records, device prompts, the final message, abort causes (a cancel on
//! the device), the sync-lock failure and device file errors. The strings are libimobiledevice
//! 1.4.0's (`tools/idevicebackup2.c`).

use crate::contracts::{DevicePromptKind, StdStream};
use crate::idevice::{LineSplitter, parse};

/// `Backup Successful.` (`idevicebackup2.c:2570`): the device reported ErrorCode 0 and
/// `SnapshotState` is `finished`.
pub const BACKUP_SUCCESSFUL: &str = "Backup Successful.";
/// `Backup Aborted.` (`idevicebackup2.c:2573`): the quit flag was set (SIGTERM, a cancel on the
/// device, a disconnect).
pub const BACKUP_ABORTED: &str = "Backup Aborted.";
/// `Backup Failed (Error Code N).` (`idevicebackup2.c:2575`).
const BACKUP_FAILED_PREFIX: &str = "Backup Failed (Error Code ";
/// Printed when the device posts a sync-cancel notification (`idevicebackup2.c:115`).
pub const CANCELLED_ON_DEVICE: &str = "User has cancelled the backup process on the device.";
/// A device error for one file (`idevicebackup2.c:1153`), counted as `device_file_errors`.
const DEVICE_ERROR_PREFIX: &str = "Received an error message from device:";
/// The sync lock (`/com.apple.itunes.lock_sync`) could not be taken, e.g. because Finder or iTunes
/// holds it (`idevicebackup2.c:1967` and `:1973`, on stderr).
pub const SYNC_LOCK_FAILED: &str = "ERROR: could not lock file!";
pub const SYNC_LOCK_TIMEOUT: &str = "ERROR: timeout while locking for sync";

/// What a chunk of output meant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parsed {
    /// A line for the live log (progress records are not logged).
    Line(String),
    /// Overall progress in percent.
    Progress(u8),
    /// A device prompt line.
    Prompt(DevicePromptKind, String),
}

/// The overall percent of a `\r[====   ]  45% Finished` record: `\]\s+(\d+)%\s+Finished`.
pub fn overall_percent(record: &str) -> Option<u8> {
    let after = &record[record.rfind(']')? + 1..];
    let digits_start = after.len() - after.trim_start().len();
    if digits_start == 0 {
        return None;
    }
    let rest = &after[digits_start..];
    let digits_len = rest.bytes().take_while(u8::is_ascii_digit).count();
    let (digits, rest) = rest.split_at(digits_len);
    let rest = rest.strip_prefix('%')?;
    let gap = rest.len() - rest.trim_start().len();
    if gap == 0 || !rest[gap..].starts_with("Finished") {
        return None;
    }
    digits
        .parse::<u16>()
        .ok()
        .map(|p| u8::try_from(p.min(100)).unwrap_or(100))
}

/// A progress bar record (per-batch or overall), which is not logged.
fn is_progress_record(record: &str) -> bool {
    record.starts_with('[') && record.contains(']') && record.contains('%')
}

/// Everything learned from the backup's output so far.
#[derive(Debug, Default)]
pub struct BackupOutput {
    splitters: [LineSplitter; 2],
    /// The last `NN% Finished` value.
    pub last_percent: Option<u8>,
    /// The last final message seen (`Backup Successful.`, `Backup Failed (Error Code N).` or
    /// `Backup Aborted.`).
    pub final_message: Option<String>,
    cancel_line_seen: bool,
    /// `Backup Aborted.` came after the on-device cancel line.
    pub cancelled_on_device: bool,
    /// `Received an error message from device:` lines.
    pub device_file_errors: u32,
    /// The sync-lock failure message.
    pub sync_lock_failed: bool,
}

impl BackupOutput {
    /// Parses a chunk of `stream`; records split across chunks are completed later.
    pub fn push(&mut self, stream: StdStream, chunk: &[u8]) -> Vec<Parsed> {
        let records = self.splitters[usize::from(stream == StdStream::Stderr)].push(chunk);
        records
            .into_iter()
            .filter_map(|record| self.record(stream, record))
            .collect()
    }

    /// The unterminated last records, once the process has exited.
    pub fn finish(&mut self) -> Vec<Parsed> {
        let mut parsed = Vec::new();
        for stream in [StdStream::Stdout, StdStream::Stderr] {
            if let Some(record) = self.splitters[usize::from(stream == StdStream::Stderr)].finish()
                && let Some(item) = self.record(stream, record)
            {
                parsed.push(item);
            }
        }
        parsed
    }

    fn record(&mut self, stream: StdStream, record: String) -> Option<Parsed> {
        if let Some(percent) = overall_percent(&record) {
            self.last_percent = Some(percent);
            return Some(Parsed::Progress(percent));
        }
        if is_progress_record(&record) {
            // Per-batch progress: its sizes cycle per upload batch and are ignored.
            return None;
        }
        let text = record.trim();
        if let Some(kind) = parse::prompt_kind(text) {
            return Some(Parsed::Prompt(kind, text.to_owned()));
        }
        match stream {
            StdStream::Stdout => {
                if text == CANCELLED_ON_DEVICE {
                    self.cancel_line_seen = true;
                } else if text == BACKUP_ABORTED {
                    self.final_message = Some(text.to_owned());
                    self.cancelled_on_device |= self.cancel_line_seen;
                } else if text == BACKUP_SUCCESSFUL
                    || (text.starts_with(BACKUP_FAILED_PREFIX) && text.ends_with(")."))
                {
                    self.final_message = Some(text.to_owned());
                } else if text.starts_with(DEVICE_ERROR_PREFIX) {
                    self.device_file_errors = self.device_file_errors.saturating_add(1);
                }
            }
            StdStream::Stderr => {
                if text.starts_with(SYNC_LOCK_FAILED) || text == SYNC_LOCK_TIMEOUT {
                    self.sync_lock_failed = true;
                }
            }
        }
        Some(Parsed::Line(record))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overall_progress_only_from_finished_records() {
        let bar = "=".repeat(22) + &" ".repeat(28);
        assert_eq!(overall_percent(&format!("[{bar}]  45% Finished")), Some(45));
        assert_eq!(overall_percent("[====] 100% Finished"), Some(100));
        assert_eq!(overall_percent("[] 0% Finished"), Some(0));
        assert_eq!(overall_percent("[==]\t7%\tFinished"), Some(7));
        assert_eq!(overall_percent("[==] 250% Finished"), Some(100));
        // Per-batch records: their (x/y) sizes cycle per batch.
        assert_eq!(
            overall_percent(&format!("[{bar}]  45% (1.2 MB/2.7 MB)")),
            None
        );
        for no in [
            "[==]45% Finished",
            "[==] 45%Finished",
            "[==] 45 Finished",
            "45% Finished",
            "",
        ] {
            assert_eq!(overall_percent(no), None, "{no:?}");
        }
    }

    fn feed(output: &mut BackupOutput, stream: StdStream, text: &str) -> Vec<Parsed> {
        let mut parsed = output.push(stream, text.as_bytes());
        parsed.extend(output.finish());
        parsed
    }

    #[test]
    fn a_successful_backup() {
        let mut output = BackupOutput::default();
        let chunks = [
            "Backup directory is \"/c/acquisitions/x/backup\"\n",
            "Started \"com.apple.mobilebackup2\" service on port 49324.\n",
            "Full backup mode.\n*** Waiting for passcode to be entered on the device ***\n",
            "\r[=====     ]  10% (0.3 MB/2.7 MB)     \r[==",
            "===    ]  45% Finished\n",
            "\nReceived an error message from device: No such file\n",
            "\r[==========] 100% Finished\nReceived 3 files from device.\n",
            "Backup Successful.\n",
        ];
        let mut parsed = Vec::new();
        for chunk in chunks {
            parsed.extend(output.push(StdStream::Stdout, chunk.as_bytes()));
        }
        parsed.extend(output.finish());
        assert_eq!(
            parsed,
            [
                Parsed::Line("Backup directory is \"/c/acquisitions/x/backup\"".to_owned()),
                Parsed::Line(
                    "Started \"com.apple.mobilebackup2\" service on port 49324.".to_owned()
                ),
                Parsed::Line("Full backup mode.".to_owned()),
                Parsed::Prompt(
                    DevicePromptKind::PasscodeForBackup,
                    "*** Waiting for passcode to be entered on the device ***".to_owned()
                ),
                Parsed::Progress(45),
                Parsed::Line("Received an error message from device: No such file".to_owned()),
                Parsed::Progress(100),
                Parsed::Line("Received 3 files from device.".to_owned()),
                Parsed::Line("Backup Successful.".to_owned()),
            ]
        );
        assert_eq!(output.last_percent, Some(100));
        assert_eq!(output.final_message.as_deref(), Some("Backup Successful."));
        assert_eq!(output.device_file_errors, 1);
        assert!(!output.cancelled_on_device && !output.sync_lock_failed);
    }

    #[test]
    fn final_messages_and_abort_causes() {
        let mut failed = BackupOutput::default();
        feed(
            &mut failed,
            StdStream::Stdout,
            "Backup Failed (Error Code 105).\n",
        );
        assert_eq!(
            failed.final_message.as_deref(),
            Some("Backup Failed (Error Code 105).")
        );

        let mut incomplete = BackupOutput::default();
        feed(
            &mut incomplete,
            StdStream::Stdout,
            "Backup Failed (Error Code 0).",
        );
        assert_eq!(
            incomplete.final_message.as_deref(),
            Some("Backup Failed (Error Code 0).")
        );

        let mut on_device = BackupOutput::default();
        feed(
            &mut on_device,
            StdStream::Stdout,
            "User has cancelled the backup process on the device.\nReceived 1 files from device.\nBackup Aborted.\n",
        );
        assert!(on_device.cancelled_on_device);
        assert_eq!(on_device.final_message.as_deref(), Some("Backup Aborted."));

        // An abort without the on-device line (SIGTERM, a disconnect).
        let mut aborted = BackupOutput::default();
        feed(&mut aborted, StdStream::Stderr, "Exiting...\n");
        feed(&mut aborted, StdStream::Stdout, "Backup Aborted.\n");
        assert!(!aborted.cancelled_on_device);
        assert_eq!(aborted.final_message.as_deref(), Some("Backup Aborted."));
    }

    #[test]
    fn sync_lock_failures() {
        for line in [
            "ERROR: timeout while locking for sync",
            "ERROR: could not lock file! error code: -13",
        ] {
            let mut output = BackupOutput::default();
            let parsed = feed(&mut output, StdStream::Stderr, &format!("{line}\n"));
            assert!(output.sync_lock_failed, "{line}");
            assert_eq!(parsed, [Parsed::Line(line.to_owned())]);
        }
        // Only on stderr, where the tool prints it.
        let mut output = BackupOutput::default();
        feed(
            &mut output,
            StdStream::Stdout,
            "ERROR: timeout while locking for sync\n",
        );
        assert!(!output.sync_lock_failed);
    }

    #[test]
    fn streams_are_split_separately() {
        let mut output = BackupOutput::default();
        assert!(output.push(StdStream::Stdout, b"Backup Succ").is_empty());
        assert_eq!(
            output.push(StdStream::Stderr, b"Exiting...\n"),
            [Parsed::Line("Exiting...".to_owned())]
        );
        assert_eq!(
            output.push(StdStream::Stdout, b"essful.\n"),
            [Parsed::Line("Backup Successful.".to_owned())]
        );
    }
}
