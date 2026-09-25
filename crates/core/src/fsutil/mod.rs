//! Filesystem helpers: atomic JSON writes, read-only marking, path containment and free space
//! (ARCHITECTURE.md §5.1). OS-specific code lives in `unix.rs` and `windows.rs`.

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::contracts::ErrorCode;
use crate::hashing::to_hex;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as sys;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as sys;

/// Writes `value` as pretty JSON (with a trailing newline) to `path` atomically (CONTRACTS.md
/// §1): `<name>.tmp-<rand>` in the same directory, flush and `sync_all`, then rename over `path`.
/// A crash leaves the old or the new file, never a truncated one (at worst a stray temp file).
///
/// A read-only target (a finalized record) is never replaced, on any OS: that fails with
/// `PermissionDenied` and leaves the target untouched. (A Unix rename would otherwise succeed,
/// because it needs write access to the directory, not the file.)
pub fn write_json_atomic<T: Serialize + ?Sized>(path: &Path, value: &T) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    write_atomic(path, &bytes, |_| Ok(()))
}

/// Writes `bytes` to `path` atomically, with the same guarantees as [`write_json_atomic`] (used for
/// hash manifests).
pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic(path, bytes, |_| Ok(()))
}

/// The atomic write, with a hook between the synced temp file and the rename (tests simulate a
/// crash there).
///
/// After the rename, the parent directory is synced on Unix (best effort) so the rename itself is
/// durable. On Windows the rename is retried briefly while another process (antivirus, the search
/// indexer) holds the target open.
pub(crate) fn write_atomic(
    path: &Path,
    bytes: &[u8],
    before_rename: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} has no file name", path.display()),
        )
    })?;
    match fs::metadata(path) {
        Ok(meta) if meta.permissions().readonly() => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("{} is read-only; refusing to replace it", path.display()),
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut random = [0u8; 8];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    let mut tmp_name = name.to_os_string();
    tmp_name.push(format!(".tmp-{}", to_hex(&random)));
    let tmp = path.with_file_name(tmp_name);

    let result = write_new_synced(&tmp, bytes)
        .and_then(|()| before_rename(&tmp))
        .and_then(|()| sys::rename_replace(&tmp, path));
    if result.is_err() {
        // Best effort: the original error matters more than a leftover temp file.
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn write_new_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()
}

/// The `AppError` code for an I/O error (CONTRACTS.md §12): `permission_denied` when access was
/// refused (including macOS privacy protection, EPERM), otherwise `io`.
pub fn io_error_code(err: &io::Error) -> ErrorCode {
    if err.kind() == io::ErrorKind::PermissionDenied {
        ErrorCode::PermissionDenied
    } else {
        ErrorCode::Io
    }
}

/// Runs `op` (a rename or removal), retrying it for about 2.5 s on Windows while it fails with a
/// transient sharing, lock or access error: an antivirus scanner or the search indexer holding a
/// just-written or just-run file open. On Unix `op` runs once.
pub(crate) fn retry_transient(op: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    sys::retry_transient_briefly(op)
}

/// Makes a file read-only: clears the write bits on Unix (0644 becomes 0444, 0600 becomes 0400),
/// sets the readonly attribute on Windows.
pub fn set_read_only(path: &Path) -> io::Result<()> {
    sys::set_read_only(path)
}

/// A file name as written in hash manifests (CONTRACTS.md §8): the raw bytes on Unix, UTF-8 on
/// Windows. The flag is true when the name is not valid Unicode and was converted lossily (Windows
/// only; the manifest then warns `unencodable_filename`).
pub fn manifest_name_bytes(name: &OsStr) -> (Vec<u8>, bool) {
    sys::manifest_name_bytes(name)
}

/// Free bytes available to this user on the volume holding `path` (an existing directory).
pub fn free_space(path: &Path) -> io::Result<u64> {
    sys::free_space(path)
}

/// Whether `a` is `b` or lies inside it. Paths are canonicalized for the comparison only.
///
/// A path that does not exist yet (such as a would-be run folder) is resolved through its longest
/// existing ancestor, and the remaining components are applied to it lexically. Symlinks and `..`
/// in the existing part are resolved by the OS. On case-insensitive filesystems, existing
/// components compare by their on-disk spelling; components that do not exist compare as spelled.
pub fn path_within(a: &Path, b: &Path) -> io::Result<bool> {
    Ok(resolve(a)?.starts_with(resolve(b)?))
}

/// `path` made absolute and canonical as far as it exists, with the rest appended.
fn resolve(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let components: Vec<Component<'_>> = absolute.components().collect();
    for split in (1..=components.len()).rev() {
        let prefix: PathBuf = components[..split].iter().collect();
        let mut resolved = match fs::canonicalize(&prefix) {
            Ok(canonical) => canonical,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                continue;
            }
            Err(e) => return Err(e),
        };
        let rest = &components[split..];
        let has_parent = rest.iter().any(|c| matches!(c, Component::ParentDir));
        for component in rest {
            match component {
                Component::ParentDir => {
                    resolved.pop();
                }
                other => resolved.push(other),
            }
        }
        // A `..` after a missing component may lead back into existing, possibly symlinked,
        // directories: resolve the lexically normalized path again (it has no `..` left).
        return if has_parent {
            resolve(&resolved)
        } else {
            Ok(resolved)
        };
    }
    // Nothing exists, not even the root (e.g. a missing drive): compare as spelled.
    Ok(absolute)
}

#[cfg(test)]
pub(crate) mod test_support {
    pub(crate) use super::sys::{make_writable, symlink_dir};
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    #[test]
    fn write_json_atomic_writes_pretty_json_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("case.json");
        write_json_atomic(&path, &serde_json::json!({"a": 1})).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\n  \"a\": 1\n}\n");
        write_json_atomic(&path, &serde_json::json!({"a": 2})).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\n  \"a\": 2\n}\n");
        // No temp files are left behind.
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["case.json"]);
    }

    #[test]
    fn crash_before_rename_never_truncates_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.json");
        fs::write(&path, "old complete record\n").unwrap();
        let mut seen_tmp = None;
        let result = write_atomic(&path, b"new complete record\n", |tmp| {
            // The moment of the crash: the new data is complete and synced in the temp file next
            // to the target, and the target is untouched.
            assert_eq!(tmp.parent(), path.parent());
            let tmp_name = tmp.file_name().unwrap().to_string_lossy().into_owned();
            assert!(tmp_name.starts_with("run.json.tmp-"), "{tmp_name}");
            assert_eq!(tmp_name.len(), "run.json.tmp-".len() + 16, "{tmp_name}");
            assert_eq!(fs::read_to_string(tmp).unwrap(), "new complete record\n");
            assert_eq!(fs::read_to_string(&path).unwrap(), "old complete record\n");
            seen_tmp = Some(tmp.to_path_buf());
            Err(io::Error::other("simulated crash"))
        });
        assert_eq!(result.unwrap_err().to_string(), "simulated crash");
        assert_eq!(fs::read_to_string(&path).unwrap(), "old complete record\n");
        assert!(!seen_tmp.unwrap().exists(), "the temp file is cleaned up");
    }

    #[test]
    fn write_json_atomic_never_replaces_a_read_only_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.json");
        write_json_atomic(&path, &serde_json::json!({"status": "succeeded"})).unwrap();
        set_read_only(&path).unwrap();
        let sealed = fs::read(&path).unwrap();

        let err = write_json_atomic(&path, &serde_json::json!({"status": "tampered"})).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(&path).unwrap(), sealed, "byte-identical");
        assert!(fs::metadata(&path).unwrap().permissions().readonly());
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["run.json"], "no temp file left behind");
        test_support::make_writable(&path);
    }

    #[test]
    fn write_file_atomic_writes_bytes_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.sha256");
        write_file_atomic(&path, b"line\r\n\\raw\n").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"line\r\n\\raw\n");
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["report.sha256"]);
    }

    #[test]
    fn io_error_codes() {
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        assert_eq!(io_error_code(&denied), ErrorCode::PermissionDenied);
        let missing = io::Error::from(io::ErrorKind::NotFound);
        assert_eq!(io_error_code(&missing), ErrorCode::Io);
        #[cfg(unix)]
        assert_eq!(
            io_error_code(&io::Error::from_raw_os_error(libc::EPERM)),
            ErrorCode::PermissionDenied
        );
    }

    #[test]
    fn write_json_atomic_needs_a_file_name() {
        let dir = tempfile::tempdir().unwrap();
        let err = write_json_atomic(&dir.path().join(".."), &1).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn read_only_files_refuse_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.json");
        fs::write(&path, "sealed").unwrap();
        set_read_only(&path).unwrap();
        assert!(fs::metadata(&path).unwrap().permissions().readonly());
        let err = OpenOptions::new().append(true).open(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert!(fs::write(&path, "tampered").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "sealed");
        test_support::make_writable(&path);
    }

    #[test]
    fn free_space_reports_the_volume() {
        let dir = tempfile::tempdir().unwrap();
        assert!(free_space(dir.path()).unwrap() > 0);
        let err = free_space(&dir.path().join("missing")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    fn within(a: &Path, b: &Path) -> bool {
        path_within(a, b).unwrap()
    }

    #[test]
    fn path_within_equal_nested_and_siblings() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        fs::create_dir_all(d.join("b/sub")).unwrap();
        fs::create_dir_all(d.join("bc")).unwrap();

        assert!(within(d, d));
        assert!(within(&d.join("b"), &d.join("b")));
        assert!(within(&d.join("b/./sub"), &d.join("b")));
        assert!(within(&d.join("b/sub"), &d.join("b")));
        assert!(!within(&d.join("b"), &d.join("b/sub")));
        // Not yet existing (a would-be run folder) and deeper.
        assert!(within(&d.join("b/runs/new-run"), &d.join("b")));
        assert!(within(&d.join("b/missing"), &d.join("b/missing")));
        // Siblings with a shared name prefix are not inside each other.
        assert!(!within(&d.join("bc"), &d.join("b")));
        assert!(!within(&d.join("b"), &d.join("bc")));
        assert!(!within(&d.join("bc/new"), &d.join("b")));
        assert!(!within(&d.join("bcd"), &d.join("b")));
    }

    #[test]
    fn path_within_resolves_parent_components() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        let b = d.join("b");
        fs::create_dir_all(b.join("sub")).unwrap();

        assert!(!within(&b.join("sub/../.."), &b));
        assert!(within(&b.join("sub/../other"), &b));
        assert!(within(&b.join("sub/.."), &b));
        // `..` after components that do not exist.
        assert!(within(&b.join("missing/../sub"), &b));
        assert!(!within(&b.join("missing/../../x"), &b));
        assert!(within(&d.join("x/../b/new"), &b));
    }

    /// Creates a directory symlink, or returns false (with a message) where the OS does not allow
    /// it: Windows without Developer Mode or admin (ERROR_PRIVILEGE_NOT_HELD, 1314).
    fn try_symlink_dir(target: &Path, link: &Path) -> bool {
        match test_support::symlink_dir(target, link) {
            Ok(()) => true,
            Err(e) if cfg!(windows) && e.raw_os_error() == Some(1314) => {
                eprintln!(
                    "SKIPPED symlink checks: creating symlinks needs Developer Mode or admin \
                     (ERROR_PRIVILEGE_NOT_HELD)"
                );
                false
            }
            Err(e) => panic!("symlink {} -> {}: {e}", link.display(), target.display()),
        }
    }

    #[test]
    fn path_within_follows_symlinks() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        let inside = d.join("case");
        let outside = d.join("evidence");
        fs::create_dir_all(&inside).unwrap();
        fs::create_dir_all(&outside).unwrap();
        if !try_symlink_dir(&outside, &inside.join("link")) {
            return;
        }
        assert!(try_symlink_dir(&inside, &outside.join("back")));

        // A link inside `case` that points out of it is not inside it.
        assert!(!within(&inside.join("link"), &inside));
        assert!(!within(&inside.join("link/new-run"), &inside));
        assert!(within(&inside.join("link/new-run"), &outside));
        // A path outside that leads into `case` through a link is inside it.
        assert!(within(&outside.join("back/runs/new"), &inside));
        // `..` after a missing component, then through a link.
        assert!(!within(&inside.join("missing/../link/x"), &inside));
    }

    #[test]
    fn path_within_follows_filesystem_case_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path();
        fs::create_dir_all(d.join("CaseDir")).unwrap();
        let case_insensitive = d.join("casedir").exists();
        // On a case-insensitive filesystem another spelling of an existing folder is that folder;
        // on a case-sensitive one it is a different (missing) folder.
        assert_eq!(
            within(&d.join("casedir/runs/new"), &d.join("CaseDir")),
            case_insensitive
        );
        assert_eq!(
            within(&d.join("CASEDIR"), &d.join("casedir")),
            case_insensitive
        );
        eprintln!("temp filesystem is case-insensitive: {case_insensitive}");
    }
}
