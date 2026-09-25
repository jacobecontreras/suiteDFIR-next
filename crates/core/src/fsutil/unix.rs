//! Unix implementations of the `fsutil` helpers.

use std::ffi::{CString, OsStr};
use std::fs;
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Clears the write bits only, so the read bits stay as they were (0600 becomes 0400).
pub(super) fn set_read_only(path: &Path) -> io::Result<()> {
    let mode = fs::metadata(path)?.permissions().mode();
    fs::set_permissions(path, fs::Permissions::from_mode(mode & !0o222))
}

/// Renames `from` over `to`, then syncs the parent directory so the rename survives a crash. The
/// directory sync is best effort: the rename has already happened (the target holds the complete
/// new file), and some filesystems (e.g. network shares) refuse to sync a directory.
pub(super) fn rename_replace(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)?;
    let parent = match to.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    if let Err(e) = fs::File::open(parent).and_then(|dir| dir.sync_all()) {
        log::debug!("could not sync directory {}: {e}", parent.display());
    }
    Ok(())
}

/// Unix names are byte strings and go into manifests unchanged.
pub(super) fn manifest_name_bytes(name: &OsStr) -> (Vec<u8>, bool) {
    (name.as_bytes().to_vec(), false)
}

pub(super) fn free_space(path: &Path) -> io::Result<u64> {
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))?;
    let mut stat = MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: FFI call. `c_path` is a NUL-terminated string that outlives the call, and `stat`
    // points to writable memory of the right type, which statvfs fills when it returns 0.
    if unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: statvfs returned 0, so it initialized `stat`.
    let stat = unsafe { stat.assume_init() };
    // The field types differ by platform (u32 or u64), hence the conversions.
    #[allow(clippy::useless_conversion)]
    let available = u64::from(stat.f_bavail).saturating_mul(u64::from(stat.f_frsize));
    Ok(available)
}

#[cfg(test)]
pub(crate) fn make_writable(path: &Path) {
    fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
}

#[cfg(test)]
pub(crate) fn symlink_dir(target: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_names_are_raw_bytes() {
        let name = OsStr::from_bytes(b"a\xffb\\c\nd");
        assert_eq!(manifest_name_bytes(name), (b"a\xffb\\c\nd".to_vec(), false));
    }

    #[test]
    fn set_read_only_clears_only_the_write_bits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.json");
        fs::write(&path, "sealed").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        set_read_only(&path).unwrap();
        // Not widened to world-readable.
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o400
        );
        assert!(fs::OpenOptions::new().append(true).open(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "sealed");
        make_writable(&path);
    }
}
