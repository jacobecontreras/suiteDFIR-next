//! Windows implementations of the `fsutil` helpers.

use std::fs;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

pub(super) fn set_read_only(path: &Path) -> io::Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)
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
