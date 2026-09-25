//! Parsers for the libimobiledevice tools' output (docs/IDEVICE-CLI.md). Every message string here
//! is copied from libimobiledevice 1.4.0 (`tools/*.c`); IDEVICE-CLI.md §4 and §5 list them.

use std::io::Cursor;

use crate::contracts::{DevicePromptKind, PairState};

/// `idevice_id -l` when usbmuxd cannot be reached (`idevice_id.c:154`). The same message is printed
/// for every usbmuxd failure: the service missing, stopped or unreachable (IDEVICE-CLI.md §8).
pub const DEVICE_LIST_FAILED: &str = "ERROR: Unable to retrieve device list!";

/// `idevicebackup2` before a backup on iOS ≥ 16.1 when the device asks for its passcode
/// (`idevicebackup2.c:2062`).
pub const PASSCODE_WAIT: &str = "*** Waiting for passcode to be entered on the device ***";
/// `idevicebackup2 encryption on` on iOS ≥ 13 with a passcode (`idevicebackup2.c:2256`).
pub const CONFIRM_ENABLE: &str =
    "Please confirm enabling the backup encryption by entering the passcode on the device.";
/// `idevicebackup2 encryption off` on iOS ≥ 13 with a passcode (`idevicebackup2.c:2258`).
pub const CONFIRM_DISABLE: &str =
    "Please confirm disabling the backup encryption by entering the passcode on the device.";
/// `idevicebackup2 changepw` (never run by suiteDFIR; recognized for completeness,
/// `idevicebackup2.c:2254`).
pub const CONFIRM_CHANGEPW: &str =
    "Please confirm changing the backup password by entering the passcode on the device.";

/// A record longer than this without a line break is emitted in pieces.
const MAX_RECORD: usize = 64 * 1024;

/// Whether `text` is a USB UDID (ARCHITECTURE.md §9): 40 hex digits, or 8 hex digits, a dash and
/// 16 hex digits.
pub fn is_udid(text: &str) -> bool {
    let hex =
        |part: &str, len: usize| part.len() == len && part.bytes().all(|b| b.is_ascii_hexdigit());
    match text.split_once('-') {
        None => hex(text, 40),
        Some((head, tail)) => hex(head, 8) && hex(tail, 16),
    }
}

/// The UDIDs listed by `idevice_id -l` (one per line), in order, without duplicates. Lines that are
/// not UDIDs are ignored.
pub fn udid_list(stdout: &str) -> Vec<String> {
    let mut udids: Vec<String> = Vec::new();
    for line in stdout.lines() {
        let udid = line.trim();
        if is_udid(udid) && !udids.iter().any(|known| known == udid) {
            udids.push(udid.to_owned());
        }
    }
    udids
}

/// Whether `text` is a UUID like a HostID or SystemBUID: 8-4-4-4-12 hex digits.
fn is_uuid(text: &str) -> bool {
    let parts: Vec<&str> = text.split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(part, len)| part.len() == len && part.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// The HostID printed by `idevicepair hostid` or the SystemBUID printed by `idevicepair
/// systembuid`; `None` for `(null)` (no record) or anything that is not a UUID.
pub fn host_uuid(stdout: &str) -> Option<String> {
    let text = stdout.trim();
    is_uuid(text).then(|| text.to_owned())
}

/// What `idevicepair pair` or `validate` answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PairAnswer {
    /// The pairing state, with the tool's message for everything but success.
    State {
        state: PairState,
        message: Option<String>,
    },
    /// `No device found with udid …`: the device is not connected.
    DeviceNotFound,
}

/// Parses `idevicepair pair` / `validate` stdout (`idevicepair.c:114-143`, IDEVICE-CLI.md §4).
/// Anything unrecognized is `pairing_failed` with the tool's first line as the message.
pub fn pair_answer(stdout: &str) -> PairAnswer {
    let failed = |message: Option<String>| PairAnswer::State {
        state: PairState::PairingFailed,
        message,
    };
    let lines: Vec<&str> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    for line in &lines {
        if line.starts_with("No device found") {
            return PairAnswer::DeviceNotFound;
        }
        if line.starts_with("SUCCESS: Paired with device ")
            || line.starts_with("SUCCESS: Validated pairing with device ")
        {
            return PairAnswer::State {
                state: PairState::Paired,
                message: None,
            };
        }
        let Some(error) = line.strip_prefix("ERROR: ") else {
            continue;
        };
        let state = if error.starts_with("Please accept the trust dialog on the screen of device ")
        {
            PairState::AwaitingTrust
        } else if error.starts_with("Could not validate with device ")
            && error.contains(" because a passcode is set.")
        {
            PairState::Locked
        } else if error.starts_with("Device ") && error.ends_with(" is not paired with this host") {
            PairState::NotPaired
        } else if error.starts_with("Device ")
            && error.ends_with(" said that the user denied the trust dialog.")
        {
            PairState::TrustDenied
        } else {
            PairState::PairingFailed
        };
        return PairAnswer::State {
            state,
            message: Some((*line).to_owned()),
        };
    }
    failed(lines.first().map(|line| (*line).to_owned()))
}

/// The device identity fields suiteDFIR shows and records.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceInfo {
    pub device_name: Option<String>,
    pub product_type: Option<String>,
    pub product_version: Option<String>,
    pub build_version: Option<String>,
    pub serial_number: Option<String>,
}

fn plist_value(bytes: &[u8]) -> Option<plist::Value> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    plist::Value::from_reader(Cursor::new(bytes)).ok()
}

/// Parses `ideviceinfo -x` output (a dictionary). Empty output, which the tool prints with exit 0
/// when the read failed (IDEVICE-CLI.md §2), and anything that is not a dictionary are `None`.
pub fn device_info(bytes: &[u8]) -> Option<DeviceInfo> {
    let value = plist_value(bytes)?;
    let dict = value.as_dictionary()?;
    let text = |key: &str| {
        dict.get(key)
            .and_then(plist::Value::as_string)
            .map(str::to_owned)
    };
    Some(DeviceInfo {
        device_name: text("DeviceName"),
        product_type: text("ProductType"),
        product_version: text("ProductVersion"),
        build_version: text("BuildVersion"),
        serial_number: text("SerialNumber"),
    })
}

/// Parses `ideviceinfo -q com.apple.mobile.backup -k WillEncrypt -x` output (a boolean). Empty or
/// anything else is `None` (unreadable).
pub fn will_encrypt(bytes: &[u8]) -> Option<bool> {
    plist_value(bytes)?.as_boolean()
}

/// The device's data partition (`ideviceinfo -q com.apple.disk_usage -x`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskUsage {
    pub data_capacity: u64,
    pub data_available: u64,
}

impl DiskUsage {
    /// The used data capacity: what a full backup needs (ARCHITECTURE.md §6b step 3).
    pub fn data_used(self) -> u64 {
        self.data_capacity.saturating_sub(self.data_available)
    }
}

/// Parses the disk-usage dictionary: `TotalDataCapacity` and `TotalDataAvailable`.
pub fn disk_usage(bytes: &[u8]) -> Option<DiskUsage> {
    let value = plist_value(bytes)?;
    let dict = value.as_dictionary()?;
    let number = |key: &str| dict.get(key).and_then(plist::Value::as_unsigned_integer);
    Some(DiskUsage {
        data_capacity: number("TotalDataCapacity")?,
        data_available: number("TotalDataAvailable")?,
    })
}

/// The kind of an on-device prompt line, if `line` is one (IDEVICE-CLI.md §5).
pub fn prompt_kind(line: &str) -> Option<DevicePromptKind> {
    let line = line.trim();
    if line == PASSCODE_WAIT {
        Some(DevicePromptKind::PasscodeForBackup)
    } else if [CONFIRM_ENABLE, CONFIRM_DISABLE, CONFIRM_CHANGEPW].contains(&line) {
        Some(DevicePromptKind::PasscodeForEncryption)
    } else {
        None
    }
}

/// Splits a byte stream into records at `\r` and `\n`, across chunk boundaries (the tools print
/// progress with `\r` and output arrives in bursts, IDEVICE-CLI.md §5). Records are decoded as
/// UTF-8 lossily, trailing whitespace is trimmed, and empty records are dropped.
#[derive(Debug, Default)]
pub struct LineSplitter {
    partial: Vec<u8>,
}

impl LineSplitter {
    /// The records completed by `chunk`.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        let mut records = Vec::new();
        for &byte in chunk {
            if byte == b'\r' || byte == b'\n' {
                self.take(&mut records);
            } else {
                self.partial.push(byte);
                if self.partial.len() >= MAX_RECORD {
                    self.take(&mut records);
                }
            }
        }
        records
    }

    /// The last record if the stream did not end with a line break.
    pub fn finish(&mut self) -> Option<String> {
        let mut records = Vec::new();
        self.take(&mut records);
        records.pop()
    }

    fn take(&mut self, records: &mut Vec<String>) {
        let text = String::from_utf8_lossy(&self.partial);
        let text = text.trim_end();
        if !text.is_empty() {
            records.push(text.to_owned());
        }
        self.partial.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UDID: &str = "00008101-000A1B2C3D4E001E";
    const OLD_UDID: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn udids() {
        assert!(is_udid(UDID));
        assert!(is_udid(OLD_UDID));
        assert!(is_udid(&OLD_UDID.to_uppercase()));
        for bad in [
            "",
            "00008101-000A1B2C3D4E001",
            "00008101-000A1B2C3D4E001EF",
            "0000810-1000A1B2C3D4E001E",
            "00008101_000A1B2C3D4E001E",
            "00008101-000A1B2C3D4E001G",
            "0123456789abcdef0123456789abcdef0123456",
            "../../etc",
            "00008101-000A1B2C3D4E001E (USB)",
        ] {
            assert!(!is_udid(bad), "{bad:?}");
        }
    }

    #[test]
    fn udid_list_from_idevice_id() {
        let stdout = format!("{UDID}\n{OLD_UDID}\r\n{UDID}\n\nnot a udid\n");
        assert_eq!(udid_list(&stdout), [UDID, OLD_UDID]);
        assert!(udid_list("").is_empty());
    }

    #[test]
    fn host_ids() {
        assert_eq!(
            host_uuid("5E1B7C2A-9D4F-4E8B-A3C6-1F0D2B7E9A48\n").as_deref(),
            Some("5E1B7C2A-9D4F-4E8B-A3C6-1F0D2B7E9A48")
        );
        // No host record: printf("%s\n", NULL) prints "(null)" (idevicepair.c:390-404).
        assert_eq!(host_uuid("(null)\n"), None);
        assert_eq!(host_uuid(""), None);
        assert_eq!(host_uuid("5E1B7C2A-9D4F-4E8B-A3C6"), None);
    }

    fn state(stdout: &str) -> (PairState, Option<String>) {
        match pair_answer(stdout) {
            PairAnswer::State { state, message } => (state, message),
            PairAnswer::DeviceNotFound => panic!("{stdout}: device not found"),
        }
    }

    /// The exact messages of IDEVICE-CLI.md §4.
    #[test]
    fn pair_messages() {
        assert_eq!(
            state(&format!("SUCCESS: Paired with device {UDID}\n")),
            (PairState::Paired, None)
        );
        assert_eq!(
            state(&format!("SUCCESS: Validated pairing with device {UDID}\n")),
            (PairState::Paired, None)
        );
        let cases = [
            (
                format!(
                    "ERROR: Please accept the trust dialog on the screen of device {UDID}, then \
                     attempt to pair again."
                ),
                PairState::AwaitingTrust,
            ),
            (
                format!(
                    "ERROR: Could not validate with device {UDID} because a passcode is set. \
                     Please enter the passcode on the device and retry."
                ),
                PairState::Locked,
            ),
            (
                format!("ERROR: Device {UDID} is not paired with this host"),
                PairState::NotPaired,
            ),
            (
                format!("ERROR: Device {UDID} said that the user denied the trust dialog."),
                PairState::TrustDenied,
            ),
            (
                format!("ERROR: Pairing with device {UDID} failed."),
                PairState::PairingFailed,
            ),
            (
                format!("ERROR: Device {UDID} returned unhandled error code -3"),
                PairState::PairingFailed,
            ),
            (
                "ERROR: Could not connect to lockdownd, error code -8".to_owned(),
                PairState::PairingFailed,
            ),
        ];
        for (line, expected) in cases {
            assert_eq!(
                state(&format!("{line}\n")),
                (expected, Some(line.clone())),
                "{line}"
            );
        }
        assert_eq!(
            pair_answer(&format!("No device found with udid {UDID}.\n")),
            PairAnswer::DeviceNotFound
        );
        // Nothing recognizable (e.g. a timeout with no output).
        assert_eq!(state(""), (PairState::PairingFailed, None));
        assert_eq!(
            state("QueryType failed, error code -5\n"),
            (
                PairState::PairingFailed,
                Some("QueryType failed, error code -5".to_owned())
            )
        );
        // A warning before the answer is skipped.
        assert_eq!(
            state(&format!(
                "WARNING: QueryType request returned 'x'\nSUCCESS: Paired with device {UDID}\n"
            ))
            .0,
            PairState::Paired
        );
    }

    fn xml(value: &plist::Value) -> Vec<u8> {
        let mut out = Vec::new();
        value.to_writer_xml(&mut out).unwrap();
        out.push(b'\n');
        out
    }

    #[test]
    fn device_info_from_xml() {
        let mut dict = plist::Dictionary::new();
        for (key, value) in [
            ("DeviceName", "Alex's iPhone"),
            ("ProductType", "iPhone13,2"),
            ("ProductVersion", "18.6"),
            ("BuildVersion", "22G86"),
            ("SerialNumber", "F2LXXXXXXX"),
            ("InternationalMobileEquipmentIdentity", "356938035643809"),
        ] {
            dict.insert(key.to_owned(), plist::Value::String(value.to_owned()));
        }
        let info = device_info(&xml(&plist::Value::Dictionary(dict.clone()))).unwrap();
        assert_eq!(
            info,
            DeviceInfo {
                device_name: Some("Alex's iPhone".to_owned()),
                product_type: Some("iPhone13,2".to_owned()),
                product_version: Some("18.6".to_owned()),
                build_version: Some("22G86".to_owned()),
                serial_number: Some("F2LXXXXXXX".to_owned()),
            }
        );
        // The pre-session subset lacks keys: those fields are null.
        dict.remove("SerialNumber");
        dict.remove("DeviceName");
        let info = device_info(&xml(&plist::Value::Dictionary(dict))).unwrap();
        assert_eq!((info.serial_number, info.device_name), (None, None));
        // Empty output is a failure (ideviceinfo.c:235-259), so is garbage or a non-dictionary.
        assert_eq!(device_info(b""), None);
        assert_eq!(device_info(b"\n"), None);
        assert_eq!(device_info(b"garbage"), None);
        assert_eq!(device_info(&xml(&plist::Value::Boolean(true))), None);
    }

    #[test]
    fn will_encrypt_values() {
        assert_eq!(will_encrypt(&xml(&plist::Value::Boolean(true))), Some(true));
        assert_eq!(
            will_encrypt(&xml(&plist::Value::Boolean(false))),
            Some(false)
        );
        assert_eq!(will_encrypt(b""), None);
        assert_eq!(
            will_encrypt(&xml(&plist::Value::String("yes".into()))),
            None
        );
    }

    #[test]
    fn disk_usage_values() {
        let mut dict = plist::Dictionary::new();
        dict.insert(
            "TotalDataCapacity".into(),
            plist::Value::Integer(118_111_600_640u64.into()),
        );
        dict.insert(
            "TotalDataAvailable".into(),
            plist::Value::Integer(56_908_145_530u64.into()),
        );
        let usage = disk_usage(&xml(&plist::Value::Dictionary(dict.clone()))).unwrap();
        assert_eq!(usage.data_used(), 61_203_455_110);
        dict.remove("TotalDataAvailable");
        assert_eq!(disk_usage(&xml(&plist::Value::Dictionary(dict))), None);
        assert_eq!(disk_usage(b""), None);
        let odd = DiskUsage {
            data_capacity: 1,
            data_available: 5,
        };
        assert_eq!(odd.data_used(), 0);
    }

    #[test]
    fn prompts() {
        assert_eq!(
            prompt_kind("*** Waiting for passcode to be entered on the device ***"),
            Some(DevicePromptKind::PasscodeForBackup)
        );
        for line in [
            "Please confirm enabling the backup encryption by entering the passcode on the device.",
            "Please confirm disabling the backup encryption by entering the passcode on the device.",
            "Please confirm changing the backup password by entering the passcode on the device.",
        ] {
            assert_eq!(
                prompt_kind(line),
                Some(DevicePromptKind::PasscodeForEncryption),
                "{line}"
            );
        }
        assert_eq!(prompt_kind("Backup Successful."), None);
        assert_eq!(prompt_kind(""), None);
    }

    #[test]
    fn splits_records_across_chunks_and_carriage_returns() {
        let mut splitter = LineSplitter::default();
        assert_eq!(splitter.push(b"Backup dir"), Vec::<String>::new());
        assert_eq!(
            splitter.push(b"ectory is \"x\"\nStart"),
            ["Backup directory is \"x\""]
        );
        assert_eq!(
            splitter.push(b"ed\r\n\r[==   ]  45% (1.2 MB/2.7 MB)     \r[==  ]  45%"),
            ["Started", "[==   ]  45% (1.2 MB/2.7 MB)"]
        );
        assert_eq!(splitter.push(b" Finished\n"), ["[==  ]  45% Finished"]);
        assert_eq!(splitter.push(b"tail without newline"), Vec::<String>::new());
        assert_eq!(splitter.finish().as_deref(), Some("tail without newline"));
        assert_eq!(splitter.finish(), None);
    }

    #[test]
    fn splitter_is_lossy_and_bounded() {
        let mut splitter = LineSplitter::default();
        assert_eq!(splitter.push(b"caf\xc3\xa9 \xff\n"), ["café \u{fffd}"]);
        let huge = vec![b'x'; MAX_RECORD + 10];
        let records = splitter.push(&huge);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].len(), MAX_RECORD);
        assert_eq!(splitter.finish().map(|r| r.len()), Some(10));
    }
}
