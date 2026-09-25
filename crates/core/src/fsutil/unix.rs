//! Unix implementations of the `fsutil` helpers.

use std::ffi::CString;
use std::fs;
use std::io;
use std::mem::MaybeUninit;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub(super) fn set_read_only(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o444))
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
