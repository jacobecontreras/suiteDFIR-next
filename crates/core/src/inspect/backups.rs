//! Finder/iTunes backup discovery (`ios_backups_find`, ROADMAP S1).
//!
//! The shell resolves the user's folders and passes them in (the core never guesses OS
//! directories); [`default_backup_dirs`] names the default backup folders below them
//! (ARCHITECTURE.md §10). [`find`] lists the backups in those folders with the details from each
//! backup's `Info.plist` and `Manifest.plist`.
//!
//! Everything here is read-only: folders are listed, files are opened for reading only, and
//! nothing is ever created, written or changed in the backup folders.
//!
//! **Links are never followed below a default folder**, so the finder reads nothing outside those
//! folders:
//! - an entry that is a symlink or a junction is skipped (not listed), whatever it points to, so
//!   two links to one backup never make duplicate rows; such a backup can still be chosen with
//!   Choose folder…;
//! - an `Info.plist`, `Manifest.plist` or marker file that is a link is not opened or followed
//!   (the details it would give stay `null`);
//! - the size counts regular files only and descends into real folders only.
//!
//! A default folder itself may be a link (a backup folder moved to another disk and linked back);
//! it is followed, and a folder reached twice that way is searched once.

use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use time::OffsetDateTime;

use crate::contracts::{AppError, ErrorCode, IosBackup, Timestamp};
use crate::fsutil;

use super::ITUNES_MARKERS;

/// A backup's `Info.plist` (with the installed apps' metadata) and `Manifest.plist` are a few MiB
/// at most; anything larger is not read.
const MAX_PLIST_BYTES: u64 = 64 * 1024 * 1024;

/// The default Finder/iTunes backup folders for `os` (`std::env::consts::OS`), below the user's
/// folders as the shell resolves them:
/// - macOS: `<home>/Library/Application Support/MobileSync/Backup`;
/// - Windows: `<roaming app data>\Apple Computer\MobileSync\Backup` (`%APPDATA%`, iTunes) and
///   `<home>\Apple\MobileSync\Backup` (`%USERPROFILE%`, the Apple Devices app and the Microsoft
///   Store iTunes);
/// - any other OS: none (Linux has no default backup location).
///
/// A folder whose base is unknown (`None`) is left out.
pub fn default_backup_dirs(
    os: &str,
    home: Option<&Path>,
    roaming_app_data: Option<&Path>,
) -> Vec<PathBuf> {
    let below = |base: Option<&Path>, parts: &[&str]| {
        base.map(|base| {
            parts
                .iter()
                .fold(base.to_path_buf(), |dir, part| dir.join(part))
        })
    };
    match os {
        "macos" => below(
            home,
            &["Library", "Application Support", "MobileSync", "Backup"],
        )
        .into_iter()
        .collect(),
        "windows" => [
            below(
                roaming_app_data,
                &["Apple Computer", "MobileSync", "Backup"],
            ),
            below(home, &["Apple", "MobileSync", "Backup"]),
        ]
        .into_iter()
        .flatten()
        .collect(),
        _ => Vec::new(),
    }
}

/// Why the backup folders could not be searched.
#[derive(Debug, thiserror::Error)]
pub enum FindError {
    /// A backup folder exists but may not be listed: on macOS, the Finder backup folder is
    /// protected by the OS (TCC) until the app has Full Disk Access.
    #[error("{path}: access denied: {source}")]
    PermissionDenied {
        path: String,
        #[source]
        source: io::Error,
    },
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
}

impl FindError {
    fn from_io(path: &Path, source: io::Error) -> Self {
        let path = path.display().to_string();
        if source.kind() == io::ErrorKind::PermissionDenied {
            Self::PermissionDenied { path, source }
        } else {
            Self::Io { path, source }
        }
    }

    /// The `AppError` code (CONTRACTS.md §12).
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::PermissionDenied { .. } => ErrorCode::PermissionDenied,
            Self::Io { source, .. } => fsutil::io_error_code(source),
        }
    }
}

/// The message of a `permission_denied` search: on macOS, that Full Disk Access is needed (the UI
/// shows the steps).
const DENIED_MESSAGE: &str = if cfg!(target_os = "macos") {
    "suiteDFIR may not read the Finder backup folder: macOS protects it until suiteDFIR has Full \
     Disk Access."
} else {
    "suiteDFIR may not read the backup folder. Check that your user account can read it, then \
     search again."
};

impl From<FindError> for AppError {
    fn from(err: FindError) -> Self {
        let message = match err.code() {
            ErrorCode::PermissionDenied => DENIED_MESSAGE,
            _ => "The backup folder could not be read",
        };
        AppError {
            code: err.code(),
            message: message.to_owned(),
            detail: Some(err.to_string()),
        }
    }
}

/// `ios_backups_find`: the backups in `dirs` (the default backup folders, see
/// [`default_backup_dirs`]), newest backup first (unknown dates last, then by path).
///
/// - A folder that does not exist (or is not a folder) is skipped: it holds no backups. A folder
///   that resolves to one already searched is skipped too.
/// - A folder that cannot be listed fails the search: `permission_denied` when access is refused
///   (on macOS the protected Finder backup folder, EPERM, until the app has Full Disk Access),
///   otherwise an I/O error.
/// - In it, every real folder (not a link, see the module docs) with `Manifest.db` or
///   `Manifest.plist` is a backup (as input inspection detects one), with details from its
///   `Info.plist` and `Manifest.plist` (see `read_backup`). Files, links and other folders are
///   skipped.
/// - One bad entry never fails the search: a folder whose contents cannot be checked is listed
///   with unknown details (every folder there is a device's backup, and choosing it as the input
///   then shows why it cannot be read), and unreadable details are unknown (`null`).
pub fn find(dirs: &[PathBuf]) -> Result<Vec<IosBackup>, FindError> {
    let mut searched: Vec<PathBuf> = Vec::new();
    let mut backups = Vec::new();
    for dir in dirs {
        match fs::metadata(dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => continue,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                continue;
            }
            Err(e) => return Err(FindError::from_io(dir, e)),
        }
        let entries = fs::read_dir(dir).map_err(|e| FindError::from_io(dir, e))?;
        // Canonicalized for comparison only: a folder linked to another one is searched once.
        let resolved = fs::canonicalize(dir).unwrap_or_else(|_| dir.clone());
        if searched.contains(&resolved) {
            continue;
        }
        searched.push(resolved);
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    log::warn!("backup finder: listing {}: {e}", dir.display());
                    continue;
                }
            };
            let path = entry.path();
            // Never through a link: a symlink or junction is not a directory here, so it is
            // skipped.
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => {}
                Ok(meta) => {
                    if meta.file_type().is_symlink() {
                        log::info!("backup finder: skipped the link {}", path.display());
                    }
                    continue;
                }
                Err(e) => {
                    log::warn!("backup finder: {}: {e}", path.display());
                    continue;
                }
            }
            match has_marker(&path) {
                Ok(true) => backups.push(read_backup(&path)),
                Ok(false) => {}
                Err(e) => {
                    log::warn!("backup finder: {}: {e}", path.display());
                    backups.push(unknown_backup(&path));
                }
            }
        }
    }
    backups.sort_by(|a, b| {
        b.last_backup
            .cmp(&a.last_backup)
            .then_with(|| a.path.cmp(&b.path))
    });
    Ok(backups)
}

/// Whether `dir` has `Manifest.db` or `Manifest.plist` (the markers of input inspection),
/// checked without following a link: a marker that is a link still marks the folder, but its target
/// is never looked at.
fn has_marker(dir: &Path) -> io::Result<bool> {
    for marker in ITUNES_MARKERS {
        match fs::symlink_metadata(dir.join(marker)) {
            Ok(_) => return Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

/// A backup whose details are all unknown.
fn unknown_backup(dir: &Path) -> IosBackup {
    IosBackup {
        path: dir.to_string_lossy().into_owned(),
        device_name: None,
        product_type: None,
        ios_version: None,
        last_backup: None,
        encrypted: None,
        size_bytes: None,
    }
}

/// A backup's details:
/// - `device_name`, `product_type`, `ios_version`: `Info.plist` (`Device Name`, else
///   `Display Name`; `Product Type`; `Product Version`), else `Manifest.plist`'s `Lockdown`
///   (`DeviceName`, `ProductType`, `ProductVersion`);
/// - `last_backup`: `Info.plist` `Last Backup Date`, else `Manifest.plist` `Date` (UTC, whole
///   seconds);
/// - `encrypted`: `Manifest.plist` `IsEncrypted`, `null` when it cannot be told (as in input
///   inspection, where a run then needs the password);
/// - `size_bytes`: the total size of the files in the folder, `null` if any part is unreadable.
///
/// A value that is missing, unreadable or of the wrong type is `null`.
fn read_backup(dir: &Path) -> IosBackup {
    let info = read_dict(&dir.join("Info.plist"));
    let manifest = read_dict(&dir.join("Manifest.plist"));
    let lockdown = manifest
        .as_ref()
        .and_then(|m| m.get("Lockdown"))
        .and_then(plist::Value::as_dictionary);
    let text = |dict: Option<&plist::Dictionary>, key: &str| {
        dict.and_then(|d| d.get(key))
            .and_then(plist::Value::as_string)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let date = |dict: Option<&plist::Dictionary>, key: &str| {
        dict.and_then(|d| d.get(key))
            .and_then(plist::Value::as_date)
            .and_then(timestamp)
    };
    let info = info.as_ref();
    let manifest_ref = manifest.as_ref();
    IosBackup {
        path: dir.to_string_lossy().into_owned(),
        device_name: text(info, "Device Name")
            .or_else(|| text(info, "Display Name"))
            .or_else(|| text(lockdown, "DeviceName")),
        product_type: text(info, "Product Type").or_else(|| text(lockdown, "ProductType")),
        ios_version: text(info, "Product Version").or_else(|| text(lockdown, "ProductVersion")),
        last_backup: date(info, "Last Backup Date").or_else(|| date(manifest_ref, "Date")),
        encrypted: manifest_ref
            .and_then(|m| m.get("IsEncrypted"))
            .and_then(plist::Value::as_boolean),
        size_bytes: tree_size(dir),
    }
}

/// A plist date as a timestamp (UTC, whole seconds), or `None` outside the representable range.
fn timestamp(date: plist::Date) -> Option<Timestamp> {
    let time = SystemTime::from(date);
    let seconds = match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).ok()?,
        // Before 1970: round down to the whole second.
        Err(before) => {
            let d = before.duration();
            let whole = i64::try_from(d.as_secs()).ok()?;
            -whole - i64::from(d.subsec_nanos() > 0)
        }
    };
    let value = OffsetDateTime::from_unix_timestamp(seconds).ok()?;
    Timestamp::from_offset_date_time(value).ok()
}

/// A plist file's top-level dictionary (XML or binary), or `None` (logged) when the file is
/// missing, not a regular file, too large, unreadable, unparsable or not a dictionary. Only a
/// regular file is opened, read-only: never a link (the finder reads nothing outside the default
/// folders) and never a FIFO (it would block the open).
fn read_dict(path: &Path) -> Option<plist::Dictionary> {
    let skip = |why: &dyn std::fmt::Display| {
        log::warn!("backup finder: {}: {why}", path.display());
        None
    };
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() => return skip(&"not a regular file (links are not followed)"),
        Ok(meta) if meta.len() > MAX_PLIST_BYTES => return skip(&"too large to be a backup plist"),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => return skip(&e),
    }
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) => return skip(&e),
    };
    match plist::Value::from_reader(BufReader::new(file)) {
        Ok(plist::Value::Dictionary(dict)) => Some(dict),
        Ok(_) => skip(&"not a dictionary"),
        Err(e) => skip(&e),
    }
}

/// The total size of the regular files below `dir`, or `None` if any part cannot be read. Links
/// inside the folder are neither followed nor counted.
fn tree_size(dir: &Path) -> Option<u64> {
    let mut total: u64 = 0;
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                log::warn!("backup finder: size of {}: {e}", dir.display());
                return None;
            }
        };
        for entry in entries {
            let entry = entry.ok()?;
            let file_type = entry.file_type().ok()?;
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                total = total.checked_add(entry.metadata().ok()?.len())?;
            }
        }
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;

    use plist::{Dictionary, Value};

    const UDID_A: &str = "00008101-000A1B2C3D4E001E";
    const UDID_B: &str = "00008030-001229C01146402E";
    const UDID_C: &str = "5b0c2f4e9a7d4b1f8c3e6a2d1f0b9e7c01234567";

    fn dict(pairs: &[(&str, Value)]) -> Dictionary {
        let mut dict = Dictionary::new();
        for (key, value) in pairs {
            dict.insert((*key).to_owned(), value.clone());
        }
        dict
    }

    fn string(text: &str) -> Value {
        Value::String(text.to_owned())
    }

    fn date(text: &str) -> Value {
        Value::Date(plist::Date::from_xml_format(text).unwrap())
    }

    fn write_plist(path: &Path, dict: Dictionary, binary: bool) {
        let value = Value::Dictionary(dict);
        if binary {
            value.to_file_binary(path).unwrap();
        } else {
            value.to_file_xml(path).unwrap();
        }
    }

    /// A synthetic backup: `Info.plist` (optional), `Manifest.plist` (optional), `Manifest.db`
    /// and one hashed-name file; returns its folder and the bytes of its files.
    struct Synthetic {
        info: Option<Dictionary>,
        manifest: Option<Dictionary>,
        binary: bool,
    }

    impl Synthetic {
        fn write(self, dir: &Path) -> u64 {
            fs::create_dir_all(dir.join("3d")).unwrap();
            fs::write(dir.join("Manifest.db"), b"SQLite format 3\0").unwrap();
            fs::write(
                dir.join("3d")
                    .join("3d0d7e5fb2ce288813306e4d4636395e047a3d28"),
                b"x",
            )
            .unwrap();
            if let Some(info) = self.info {
                write_plist(&dir.join("Info.plist"), info, self.binary);
            }
            if let Some(manifest) = self.manifest {
                write_plist(&dir.join("Manifest.plist"), manifest, self.binary);
            }
            tree_size(dir).unwrap()
        }
    }

    fn info(name: &str, product: &str, version: &str, last: &str) -> Dictionary {
        dict(&[
            ("Device Name", string(name)),
            ("Display Name", string(name)),
            ("Product Type", string(product)),
            ("Product Version", string(version)),
            ("Last Backup Date", date(last)),
            ("Unique Identifier", string(UDID_A)),
        ])
    }

    fn manifest(encrypted: Option<bool>) -> Dictionary {
        let mut manifest = dict(&[
            ("Version", string("10.0")),
            ("Date", date("2026-01-02T03:04:05Z")),
            (
                "Lockdown",
                Value::Dictionary(dict(&[
                    ("DeviceName", string("Lockdown name")),
                    ("ProductType", string("iPhone9,9")),
                    ("ProductVersion", string("9.9")),
                ])),
            ),
        ]);
        if let Some(encrypted) = encrypted {
            manifest.insert("IsEncrypted".to_owned(), Value::Boolean(encrypted));
        }
        manifest
    }

    fn ts(text: &str) -> Option<Timestamp> {
        Some(Timestamp::parse(text).unwrap())
    }

    /// Every file and folder below `dir` with its size, modification time and read-only flag.
    fn snapshot(dir: &Path) -> BTreeMap<PathBuf, (u64, SystemTime, bool)> {
        let mut out = BTreeMap::new();
        let mut pending = vec![dir.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                let meta = fs::symlink_metadata(&path).unwrap();
                if meta.is_dir() {
                    pending.push(path.clone());
                }
                out.insert(
                    path,
                    (
                        meta.len(),
                        meta.modified().unwrap(),
                        meta.permissions().readonly(),
                    ),
                );
            }
        }
        out
    }

    #[test]
    fn default_folders_per_os() {
        let home = Path::new("/Users/examiner");
        let roaming = Path::new("/roaming");
        assert_eq!(
            default_backup_dirs("macos", Some(home), Some(roaming)),
            [home
                .join("Library")
                .join("Application Support")
                .join("MobileSync")
                .join("Backup")]
        );
        assert_eq!(
            default_backup_dirs("windows", Some(home), Some(roaming)),
            [
                roaming
                    .join("Apple Computer")
                    .join("MobileSync")
                    .join("Backup"),
                home.join("Apple").join("MobileSync").join("Backup"),
            ]
        );
        // Unknown bases are left out; Linux (and anything else) has no default location.
        assert_eq!(
            default_backup_dirs("windows", None, Some(roaming)),
            [roaming
                .join("Apple Computer")
                .join("MobileSync")
                .join("Backup")]
        );
        assert_eq!(
            default_backup_dirs("windows", Some(home), None),
            [home.join("Apple").join("MobileSync").join("Backup")]
        );
        assert!(default_backup_dirs("macos", None, Some(roaming)).is_empty());
        for os in ["linux", "freebsd", ""] {
            assert!(
                default_backup_dirs(os, Some(home), Some(roaming)).is_empty(),
                "{os}"
            );
        }
    }

    #[test]
    fn missing_or_empty_folders_have_no_backups() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing").join("Backup");
        let file = tmp.path().join("file");
        fs::write(&file, b"not a folder").unwrap();
        let empty = tmp.path().join("empty");
        fs::create_dir(&empty).unwrap();
        assert_eq!(find(&[]).unwrap(), []);
        assert_eq!(find(&[missing, file.clone(), empty]).unwrap(), []);
        // Below a file: not a folder either.
        assert_eq!(find(&[file.join("Backup")]).unwrap(), []);
    }

    #[test]
    fn lists_backups_with_their_details_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Backup");
        // Unencrypted, XML plists.
        let a = root.join(UDID_A);
        let a_size = Synthetic {
            info: Some(info(
                "Alex's iPhone",
                "iPhone13,2",
                "18.6",
                "2026-09-20T21:04:33Z",
            )),
            manifest: Some(manifest(Some(false))),
            binary: false,
        }
        .write(&a);
        // Encrypted, binary plists, an older backup.
        let b = root.join(UDID_B);
        let b_size = Synthetic {
            info: Some(info(
                "Evidence iPad",
                "iPad8,1",
                "17.5.1",
                "2025-03-01T08:00:00Z",
            )),
            manifest: Some(manifest(Some(true))),
            binary: true,
        }
        .write(&b);
        // Only Manifest.db: a backup whose details are unknown.
        let c = root.join(UDID_C);
        let c_size = Synthetic {
            info: None,
            manifest: None,
            binary: false,
        }
        .write(&c);
        // Not backups: a folder without Manifest.db/Manifest.plist, and a file.
        fs::create_dir_all(root.join("Not a backup")).unwrap();
        fs::write(root.join("Not a backup").join("Info.plist"), b"x").unwrap();
        fs::write(root.join(".DS_Store"), b"\0\0\0\x01Bud1").unwrap();

        let found = find(std::slice::from_ref(&root)).unwrap();
        assert_eq!(
            found,
            [
                IosBackup {
                    path: a.to_string_lossy().into_owned(),
                    device_name: Some("Alex's iPhone".to_owned()),
                    product_type: Some("iPhone13,2".to_owned()),
                    ios_version: Some("18.6".to_owned()),
                    last_backup: ts("2026-09-20T21:04:33Z"),
                    encrypted: Some(false),
                    size_bytes: Some(a_size),
                },
                IosBackup {
                    path: b.to_string_lossy().into_owned(),
                    device_name: Some("Evidence iPad".to_owned()),
                    product_type: Some("iPad8,1".to_owned()),
                    ios_version: Some("17.5.1".to_owned()),
                    last_backup: ts("2025-03-01T08:00:00Z"),
                    encrypted: Some(true),
                    size_bytes: Some(b_size),
                },
                IosBackup {
                    size_bytes: Some(c_size),
                    ..unknown_backup(&c)
                },
            ]
        );
        // The sizes are the files' bytes: Manifest.db, the hashed file and the plists.
        assert_eq!(c_size, 16 + 1);
        assert_eq!(
            a_size,
            17 + fs::metadata(a.join("Info.plist")).unwrap().len()
                + fs::metadata(a.join("Manifest.plist")).unwrap().len()
        );
    }

    #[test]
    fn missing_or_bad_plists_leave_details_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Backup");
        // No Info.plist: the details come from Manifest.plist's Lockdown and Date.
        let lockdown_only = root.join("lockdown-only");
        Synthetic {
            info: None,
            manifest: Some(manifest(None)),
            binary: false,
        }
        .write(&lockdown_only);
        let found = read_backup(&lockdown_only);
        assert_eq!(found.device_name.as_deref(), Some("Lockdown name"));
        assert_eq!(found.product_type.as_deref(), Some("iPhone9,9"));
        assert_eq!(found.ios_version.as_deref(), Some("9.9"));
        assert_eq!(found.last_backup, ts("2026-01-02T03:04:05Z"));
        // No IsEncrypted: unknown, as in input inspection.
        assert_eq!(found.encrypted, None);

        // Values of the wrong type, empty names and only a display name.
        let odd = root.join("odd");
        Synthetic {
            info: Some(dict(&[
                ("Device Name", string("")),
                ("Display Name", string("Shown name")),
                ("Product Type", Value::Integer(7_i64.into())),
                ("Last Backup Date", string("2026-09-20T21:04:33Z")),
            ])),
            manifest: Some(dict(&[("IsEncrypted", string("yes"))])),
            binary: true,
        }
        .write(&odd);
        let found = read_backup(&odd);
        assert_eq!(found.device_name.as_deref(), Some("Shown name"));
        assert_eq!(found.product_type, None);
        assert_eq!(found.ios_version, None);
        assert_eq!(found.last_backup, None);
        assert_eq!(found.encrypted, None);

        // Unparsable plists and a plist that is not a dictionary.
        let garbage = root.join("garbage");
        Synthetic {
            info: None,
            manifest: None,
            binary: false,
        }
        .write(&garbage);
        fs::write(garbage.join("Info.plist"), b"garbage").unwrap();
        Value::Array(vec![string("x")])
            .to_file_xml(garbage.join("Manifest.plist"))
            .unwrap();
        let found = read_backup(&garbage);
        assert_eq!(
            found,
            IosBackup {
                size_bytes: found.size_bytes,
                ..unknown_backup(&garbage)
            }
        );
        assert!(found.size_bytes.is_some());

        // A plist that is a folder.
        let folder_plist = root.join("folder-plist");
        Synthetic {
            info: None,
            manifest: None,
            binary: false,
        }
        .write(&folder_plist);
        fs::create_dir(folder_plist.join("Info.plist")).unwrap();
        assert_eq!(read_backup(&folder_plist).device_name, None);

        // All of them are listed; the search does not fail.
        assert_eq!(find(&[root]).unwrap().len(), 4);
    }

    #[test]
    fn dates_are_utc_whole_seconds() {
        for (xml, expected) in [
            ("2026-09-20T21:04:33Z", "2026-09-20T21:04:33Z"),
            ("2026-09-20T23:04:33+02:00", "2026-09-20T21:04:33Z"),
            ("1969-12-31T23:59:59Z", "1969-12-31T23:59:59Z"),
            ("1901-01-01T00:00:00Z", "1901-01-01T00:00:00Z"),
        ] {
            let date = plist::Date::from_xml_format(xml).unwrap();
            assert_eq!(timestamp(date), ts(expected), "{xml}");
        }
        // Fractions are cut, also before 1970.
        let after = UNIX_EPOCH + std::time::Duration::from_millis(1_758_402_273_900);
        assert_eq!(
            timestamp(plist::Date::from(after)),
            ts("2025-09-20T21:04:33Z")
        );
        let before = UNIX_EPOCH - std::time::Duration::from_millis(1_500);
        assert_eq!(
            timestamp(plist::Date::from(before)),
            ts("1969-12-31T23:59:58Z")
        );
        // Beyond what RFC 3339 can hold.
        let far = UNIX_EPOCH + std::time::Duration::from_secs(400_000_000_000);
        assert_eq!(timestamp(plist::Date::from(far)), None);
    }

    #[test]
    fn searches_every_folder_once() {
        let tmp = tempfile::tempdir().unwrap();
        let first = tmp.path().join("Apple Computer").join("Backup");
        let second = tmp.path().join("Apple").join("Backup");
        let size = Synthetic {
            info: Some(info(
                "Old iPhone",
                "iPhone10,1",
                "16.7",
                "2024-05-05T05:05:05Z",
            )),
            manifest: Some(manifest(Some(false))),
            binary: false,
        }
        .write(&first.join(UDID_A));
        Synthetic {
            info: Some(info(
                "New iPhone",
                "iPhone16,1",
                "26.0",
                "2026-05-05T05:05:05Z",
            )),
            manifest: Some(manifest(Some(true))),
            binary: false,
        }
        .write(&second.join(UDID_B));
        let found = find(&[first.clone(), second.clone(), first.clone()]).unwrap();
        let names: Vec<_> = found.iter().map(|b| b.device_name.as_deref()).collect();
        assert_eq!(names, [Some("New iPhone"), Some("Old iPhone")]);
        assert_eq!(found[1].size_bytes, Some(size));
        // Another spelling of the same folder is searched once too.
        let spelled = tmp
            .path()
            .join("Apple")
            .join("..")
            .join("Apple Computer")
            .join("Backup");
        assert_eq!(find(&[first, spelled]).unwrap().len(), 1);
    }

    #[test]
    fn links_inside_a_backup_are_not_followed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Backup");
        let dir = root.join(UDID_A);
        let size = Synthetic {
            info: Some(info(
                "Alex's iPhone",
                "iPhone13,2",
                "18.6",
                "2026-09-20T21:04:33Z",
            )),
            manifest: Some(manifest(Some(false))),
            binary: false,
        }
        .write(&dir);
        let elsewhere = tmp.path().join("elsewhere");
        fs::create_dir(&elsewhere).unwrap();
        fs::write(elsewhere.join("big"), vec![0u8; 4096]).unwrap();
        match fsutil::test_support::symlink_dir(&elsewhere, &dir.join("linked")) {
            Ok(()) => {}
            Err(e) if cfg!(windows) && e.raw_os_error() == Some(1314) => {
                eprintln!(
                    "SKIPPED link check: creating symlinks needs Developer Mode or admin \
                     (ERROR_PRIVILEGE_NOT_HELD)"
                );
                return;
            }
            Err(e) => panic!("symlink: {e}"),
        }
        assert_eq!(find(&[root]).unwrap()[0].size_bytes, Some(size));
    }

    /// Creates a link to a folder, or returns false (with a message) where the OS does not allow it:
    /// Windows without Developer Mode or admin (ERROR_PRIVILEGE_NOT_HELD, 1314).
    fn try_symlink_dir(target: &Path, link: &Path) -> bool {
        match fsutil::test_support::symlink_dir(target, link) {
            Ok(()) => true,
            Err(e) if cfg!(windows) && e.raw_os_error() == Some(1314) => {
                eprintln!(
                    "SKIPPED link checks: creating symlinks needs Developer Mode or admin \
                     (ERROR_PRIVILEGE_NOT_HELD)"
                );
                false
            }
            Err(e) => panic!("symlink {} -> {}: {e}", link.display(), target.display()),
        }
    }

    /// Creates a link to a file, or returns false (with a message) where the OS does not allow it.
    fn try_symlink_file(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(target, link);
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(target, link);
        match made {
            Ok(()) => true,
            Err(e) if cfg!(windows) && e.raw_os_error() == Some(1314) => {
                eprintln!(
                    "SKIPPED link checks: creating symlinks needs Developer Mode or admin \
                     (ERROR_PRIVILEGE_NOT_HELD)"
                );
                false
            }
            Err(e) => panic!("symlink {} -> {}: {e}", link.display(), target.display()),
        }
    }

    /// A backup outside the default folder, linked into it (twice), is not listed: the finder
    /// reads nothing through a link, and one backup never makes two rows.
    #[test]
    fn linked_backups_below_a_default_folder_are_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Backup");
        let real = root.join(UDID_A);
        Synthetic {
            info: Some(info(
                "Alex's iPhone",
                "iPhone13,2",
                "18.6",
                "2026-09-20T21:04:33Z",
            )),
            manifest: Some(manifest(Some(false))),
            binary: false,
        }
        .write(&real);
        let outside = tmp.path().join("outside").join(UDID_B);
        Synthetic {
            info: Some(info(
                "Read from outside",
                "iPad8,1",
                "17.5.1",
                "2026-09-24T00:00:00Z",
            )),
            manifest: Some(manifest(Some(true))),
            binary: false,
        }
        .write(&outside);
        if !try_symlink_dir(&outside, &root.join(UDID_B)) {
            return;
        }
        assert!(try_symlink_dir(&outside, &root.join("another-link")));
        assert!(try_symlink_file(
            &outside.join("Info.plist"),
            &root.join("file-link")
        ));
        let found = find(std::slice::from_ref(&root)).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, real.to_string_lossy());
        assert_eq!(found[0].device_name.as_deref(), Some("Alex's iPhone"));
    }

    /// Plists and markers that are links are never opened or followed: their details stay unknown,
    /// and the size counts no link.
    #[test]
    fn linked_plists_are_not_opened() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Backup");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        write_plist(
            &outside.join("Info.plist"),
            info(
                "Read from outside",
                "iPad8,1",
                "17.5.1",
                "2026-09-24T00:00:00Z",
            ),
            false,
        );
        write_plist(&outside.join("Manifest.plist"), manifest(Some(true)), false);
        // A real backup folder whose plists link outside.
        let linked_plists = root.join(UDID_A);
        let size = Synthetic {
            info: None,
            manifest: None,
            binary: false,
        }
        .write(&linked_plists);
        if !try_symlink_file(
            &outside.join("Info.plist"),
            &linked_plists.join("Info.plist"),
        ) {
            return;
        }
        assert!(try_symlink_file(
            &outside.join("Manifest.plist"),
            &linked_plists.join("Manifest.plist")
        ));
        // A folder whose only marker is a link: a backup, with nothing read through the link.
        let linked_marker = root.join(UDID_B);
        fs::create_dir_all(&linked_marker).unwrap();
        assert!(try_symlink_file(
            &outside.join("Manifest.plist"),
            &linked_marker.join("Manifest.plist")
        ));

        let found = find(std::slice::from_ref(&root)).unwrap();
        assert_eq!(found.len(), 2, "{found:?}");
        let row = |dir: &Path| {
            found
                .iter()
                .find(|b| b.path == dir.to_string_lossy())
                .unwrap_or_else(|| panic!("{} not listed: {found:?}", dir.display()))
                .clone()
        };
        assert_eq!(
            row(&linked_plists),
            IosBackup {
                size_bytes: Some(size),
                ..unknown_backup(&linked_plists)
            }
        );
        assert_eq!(
            row(&linked_marker),
            IosBackup {
                size_bytes: Some(0),
                ..unknown_backup(&linked_marker)
            }
        );
    }

    /// The Windows form of a link: a directory junction (no privilege needed to create one).
    #[cfg(windows)]
    #[test]
    fn a_junction_below_a_default_folder_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Backup");
        let real = root.join(UDID_A);
        Synthetic {
            info: Some(info(
                "Alex's iPhone",
                "iPhone13,2",
                "18.6",
                "2026-09-20T21:04:33Z",
            )),
            manifest: Some(manifest(Some(false))),
            binary: false,
        }
        .write(&real);
        let outside = tmp.path().join("outside").join(UDID_B);
        Synthetic {
            info: Some(info(
                "Read from outside",
                "iPad8,1",
                "17.5.1",
                "2026-09-24T00:00:00Z",
            )),
            manifest: Some(manifest(Some(true))),
            binary: false,
        }
        .write(&outside);
        let junction = root.join(UDID_B);
        let output = std::process::Command::new("cmd")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(&junction)
            .arg(&outside)
            .output()
            .unwrap();
        if !output.status.success() {
            eprintln!(
                "SKIPPED junction check: mklink /J failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        assert!(
            fs::symlink_metadata(&junction)
                .unwrap()
                .file_type()
                .is_symlink(),
            "a junction is a name-surrogate reparse point"
        );
        // Nothing is read through the junction, so Redirection Guard (ERROR_UNTRUSTED_MOUNT_POINT,
        // 448) never comes into play.
        let found = find(std::slice::from_ref(&root)).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, real.to_string_lossy());
    }

    #[test]
    fn finding_never_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Backup");
        Synthetic {
            info: Some(info(
                "Alex's iPhone",
                "iPhone13,2",
                "18.6",
                "2026-09-20T21:04:33Z",
            )),
            manifest: Some(manifest(Some(true))),
            binary: false,
        }
        .write(&root.join(UDID_A));
        Synthetic {
            info: None,
            manifest: None,
            binary: false,
        }
        .write(&root.join(UDID_B));
        // Read-only files are read fine.
        for name in ["Info.plist", "Manifest.plist", "Manifest.db"] {
            fsutil::set_read_only(&root.join(UDID_A).join(name)).unwrap();
        }
        let before = snapshot(tmp.path());
        let found = find(std::slice::from_ref(&root)).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].encrypted, Some(true));
        assert_eq!(snapshot(tmp.path()), before);
        for name in ["Info.plist", "Manifest.plist", "Manifest.db"] {
            fsutil::test_support::make_writable(&root.join(UDID_A).join(name));
        }
    }

    #[test]
    fn denied_folders_are_permission_denied_with_guidance() {
        let err = FindError::from_io(
            Path::new("/Users/examiner/Library/Application Support/MobileSync/Backup"),
            io::Error::from(io::ErrorKind::PermissionDenied),
        );
        assert_eq!(err.code(), ErrorCode::PermissionDenied);
        let app = AppError::from(err);
        assert_eq!(app.code, ErrorCode::PermissionDenied);
        assert_eq!(app.message, DENIED_MESSAGE);
        if cfg!(target_os = "macos") {
            assert!(app.message.contains("Full Disk Access"), "{}", app.message);
        }
        assert!(app.detail.unwrap().contains("MobileSync"));
        let other = AppError::from(FindError::from_io(
            Path::new("/x"),
            io::Error::other("disk on fire"),
        ));
        assert_eq!(other.code, ErrorCode::Io);
        assert_eq!(other.message, "The backup folder could not be read");
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_folder_is_permission_denied_and_a_bad_entry_is_listed() {
        use std::os::unix::fs::PermissionsExt;
        let mode = |path: &Path, mode| {
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
        };
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Backup");
        let readable = root.join(UDID_A);
        Synthetic {
            info: Some(info(
                "Alex's iPhone",
                "iPhone13,2",
                "18.6",
                "2026-09-20T21:04:33Z",
            )),
            manifest: Some(manifest(Some(false))),
            binary: false,
        }
        .write(&readable);
        let locked = root.join(UDID_B);
        Synthetic {
            info: Some(info(
                "Locked iPad",
                "iPad8,1",
                "17.5.1",
                "2025-03-01T08:00:00Z",
            )),
            manifest: Some(manifest(Some(true))),
            binary: false,
        }
        .write(&locked);
        let locked_info = root.join(UDID_C);
        Synthetic {
            info: Some(info(
                "Hidden name",
                "iPhone12,1",
                "17.0",
                "2024-01-01T00:00:00Z",
            )),
            manifest: Some(manifest(Some(true))),
            binary: false,
        }
        .write(&locked_info);
        mode(&locked, 0o000);
        mode(&locked_info.join("Info.plist"), 0o000);
        let privileged = fs::read_dir(&locked).is_ok();

        if privileged {
            // Root ignores permissions; Linux CI runs unprivileged, so this is exercised there.
            eprintln!("SKIPPED: running with privileges that bypass file permissions");
        } else {
            // One bad entry never fails the search: the locked backup is listed with unknown
            // details, the one with an unreadable Info.plist with Manifest.plist's.
            let found = find(std::slice::from_ref(&root)).unwrap();
            assert_eq!(found.len(), 3, "{found:?}");
            assert_eq!(found[0].device_name.as_deref(), Some("Alex's iPhone"));
            assert_eq!(found[1].path, locked_info.to_string_lossy());
            assert_eq!(found[1].device_name.as_deref(), Some("Lockdown name"));
            assert_eq!(found[1].encrypted, Some(true));
            assert!(found[1].size_bytes.is_some());
            assert_eq!(found[2], unknown_backup(&locked));

            // The whole folder unreadable (like the protected Finder folder without Full Disk
            // Access): permission_denied, even when another folder is fine.
            mode(&root, 0o000);
            let elsewhere = tmp.path().join("missing");
            let err = find(&[elsewhere, root.clone()]).unwrap_err();
            assert!(matches!(err, FindError::PermissionDenied { .. }), "{err:?}");
            let app = AppError::from(err);
            assert_eq!(app.code, ErrorCode::PermissionDenied);
            assert!(app.detail.unwrap().contains("Backup"));
            mode(&root, 0o755);
        }
        mode(&locked, 0o755);
        mode(&locked_info.join("Info.plist"), 0o644);
    }

    /// A FIFO named like a plist is never opened (opening it would block until a writer comes).
    #[cfg(unix)]
    #[test]
    fn a_fifo_plist_does_not_block_the_search() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Backup");
        let dir = root.join(UDID_A);
        Synthetic {
            info: None,
            manifest: Some(manifest(Some(true))),
            binary: false,
        }
        .write(&dir);
        let made = std::process::Command::new("mkfifo")
            .arg(dir.join("Info.plist"))
            .status();
        if !made.as_ref().is_ok_and(|status| status.success()) {
            eprintln!("SKIPPED: mkfifo is not available here ({made:?})");
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(find(&[root]));
        });
        let found = rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("the search blocked on the FIFO")
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].device_name.as_deref(), Some("Lockdown name"));
        assert_eq!(found[0].encrypted, Some(true));
    }

    #[cfg(not(unix))]
    #[test]
    fn an_unreadable_folder_is_permission_denied_and_a_bad_entry_is_listed() {
        eprintln!(
            "SKIPPED: denying read access to a folder needs an ACL change that std cannot make on \
             this OS; the mapping is checked in denied_folders_are_permission_denied_with_guidance"
        );
    }
}
