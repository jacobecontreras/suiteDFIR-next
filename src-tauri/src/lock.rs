//! The single-instance lock (ARCHITECTURE.md F9, D14, §5.2): `<app_data>/instance.lock`, held with
//! `File::try_lock` for the life of the process. The OS releases it when the process ends, even
//! after a crash, so a stale lock file never blocks a restart. A second instance cannot take it and
//! must stop before touching any state (`another_instance_running`).

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

/// Why the lock could not be taken.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("another instance of suiteDFIR is running")]
    AnotherInstance,
    #[error("the instance lock {path} cannot be used: {source}")]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
}

/// The held lock; dropping it releases the lock.
#[derive(Debug)]
pub struct InstanceLock {
    _file: File,
}

/// Takes the lock at `path`, creating the file (and its folder) if needed. The file's content is
/// never read or written.
pub fn acquire(path: &Path) -> Result<InstanceLock, LockError> {
    let io_error = |source| LockError::Io {
        path: path.display().to_string(),
        source,
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(io_error)?;
    match file.try_lock() {
        Ok(()) => Ok(InstanceLock { _file: file }),
        Err(fs::TryLockError::WouldBlock) => Err(LockError::AnotherInstance),
        Err(fs::TryLockError::Error(e)) => Err(io_error(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::process::Command;

    /// The variable that makes `second_instance_probe` try the lock at that path.
    const PROBE_VAR: &str = "SUITEDFIR_LOCK_PROBE";

    /// Takes a lock that was just released. Other tests of this binary spawn processes: a child
    /// forked while the lock was held shares it until the child executes its program (a moment
    /// later), so the first attempts may still be refused. (The app takes its lock before it
    /// spawns anything.)
    fn acquire_released(path: &Path) -> InstanceLock {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match acquire(path) {
                Ok(lock) => return lock,
                Err(LockError::AnotherInstance) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(e) => panic!("{e}"),
            }
        }
    }

    #[test]
    fn a_second_lock_in_this_process_is_refused_until_the_first_is_released() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data").join("instance.lock");
        let first = acquire(&path).unwrap();
        assert!(matches!(acquire(&path), Err(LockError::AnotherInstance)));
        drop(first);
        let again = acquire_released(&path);
        drop(again);
        // A leftover lock file from an earlier (crashed) instance does not block.
        assert!(path.is_file());
        acquire_released(&path);
    }

    /// Run by `a_second_instance_is_refused` in a child process: tries the lock named by
    /// `SUITEDFIR_LOCK_PROBE` and prints the outcome. Does nothing without it.
    #[test]
    #[ignore = "helper process for a_second_instance_is_refused"]
    fn second_instance_probe() {
        let Some(path) = std::env::var_os(PROBE_VAR) else {
            return;
        };
        match acquire(Path::new(&path)) {
            Ok(_) => println!("probe: acquired"),
            Err(LockError::AnotherInstance) => println!("probe: another instance"),
            Err(e) => println!("probe: error {e}"),
        }
    }

    fn probe(path: &Path) -> String {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "lock::tests::second_instance_probe",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(PROBE_VAR, path)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    #[test]
    fn a_second_instance_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("instance.lock");
        let held = acquire(&path).unwrap();
        let out = probe(&path);
        assert!(out.contains("probe: another instance"), "{out}");
        drop(held);
        let out = probe(&path);
        assert!(out.contains("probe: acquired"), "{out}");
    }
}
