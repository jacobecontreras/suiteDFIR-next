//! Windows implementations of the `fsutil` helpers.

use std::fs;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION,
};
use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

/// How often a rename is retried while the target is held open, and the pause before each retry
/// (about 2.5 s in total).
const RENAME_RETRIES: u32 = 10;
const RENAME_BACKOFF: Duration = Duration::from_millis(50);

pub(super) fn set_read_only(path: &Path) -> io::Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)
}

/// Renames `from` over `to`. Antivirus scanners and the search indexer briefly open new files
/// without delete sharing, which makes the rename fail with a sharing or access error; retry for a
/// short, bounded time. (A read-only target is refused by the caller before this is reached, so an
/// access error here is transient.) NTFS journals the rename itself, so there is no directory sync.
pub(super) fn rename_replace(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if attempt < RENAME_RETRIES && is_transient_rename_error(&e) => {
                attempt += 1;
                std::thread::sleep(RENAME_BACKOFF * attempt);
            }
            Err(e) => return Err(e),
        }
    }
}

fn is_transient_rename_error(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error().and_then(|code| u32::try_from(code).ok()),
        Some(ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
    )
}

pub(super) fn free_space(path: &Path) -> io::Result<u64> {
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    if wide.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains a NUL character",
        ));
    }
    wide.push(0);
    let mut available: u64 = 0;
    // SAFETY: FFI call. `wide` is a NUL-terminated UTF-16 string that outlives the call,
    // `available` is a valid u64 to write to, and the two other out-pointers are optional (null).
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(available)
}

// On Windows `set_readonly(false)` clears only the readonly attribute; the lint is about Unix modes.
#[cfg(test)]
#[allow(clippy::permissions_set_readonly_false)]
pub(crate) fn make_writable(path: &Path) {
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions).unwrap();
}

#[cfg(test)]
pub(crate) fn symlink_dir(target: &Path, link: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::windows::fs::OpenOptionsExt;
    use std::time::Instant;

    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    #[test]
    fn rename_waits_for_a_transient_holder() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("run.json");
        let tmp = dir.path().join("run.json.tmp");
        fs::write(&target, "old").unwrap();
        fs::write(&tmp, "new").unwrap();
        // Like a scanner: the target is open without FILE_SHARE_DELETE, so it cannot be replaced.
        let holder = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&target)
            .unwrap();
        let err = fs::rename(&tmp, &target).unwrap_err();
        assert!(is_transient_rename_error(&err), "{err:?}");
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(holder);
        });
        let start = Instant::now();
        rename_replace(&tmp, &target).unwrap();
        assert!(start.elapsed() >= Duration::from_millis(200));
        release.join().unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new");
        assert!(!tmp.exists());
    }

    #[test]
    fn rename_gives_up_on_other_errors_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let err = rename_replace(&dir.path().join("missing"), &dir.path().join("x")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(start.elapsed() < Duration::from_millis(40));
    }
}
