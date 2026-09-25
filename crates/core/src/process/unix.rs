//! Unix process trees: every spawn starts a new session, so the tree is one process group whose id
//! is the leader's pid (ARCHITECTURE.md §7).
//!
//! `unsafe` here is FFI only: `setsid` between fork and exec (the `pre_exec` hook), and `killpg`
//! and `kill`, each commented.

use std::io;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Child, Command, ExitStatus};

/// SIGTERM lets the tree exit gracefully; SIGKILL follows after the grace.
pub(super) const GRACEFUL_TERMINATE: bool = true;

/// Makes the child the leader of a new session: its process group id is its pid, so the whole
/// tree can be signalled, and it has no controlling terminal, so nothing can block on `/dev/tty`
/// (LEAPP-CLI.md Q5).
pub(super) fn configure(command: &mut Command) {
    // SAFETY: the hook runs in the forked child before exec, where only async-signal-safe work is
    // allowed. It calls setsid(2), which is async-signal-safe, and reads errno on failure; it does
    // not allocate or take locks. This is the only code in the process module that runs between
    // fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
}

/// A spawned process group.
pub(super) struct Tree {
    pgid: libc::pid_t,
}

impl Tree {
    /// The child's group is its own pid (see `configure`); nothing else to set up.
    pub(super) fn attach(child: &mut Child) -> io::Result<Self> {
        Ok(Self {
            pgid: child.id().cast_signed(),
        })
    }

    /// SIGTERM to the group. A group that is already gone is fine.
    pub(super) fn terminate(&self) -> io::Result<()> {
        signal_group(self.pgid, libc::SIGTERM)
    }

    /// SIGKILL to the group. A group that is already gone is fine.
    pub(super) fn kill(&self) -> io::Result<()> {
        signal_group(self.pgid, libc::SIGKILL)
    }

    /// Whether any process of the group still runs. `killpg(pgid, 0)` answers until the group is
    /// gone (ESRCH). On Linux, members that already exited but were not reaped (zombies left with an
    /// init that never reaps, as in some containers) do not count.
    pub(super) fn is_alive(&self) -> bool {
        match killpg(self.pgid, 0) {
            Err(e) if e.raw_os_error() == Some(libc::ESRCH) => false,
            _ => live_member_exists(self.pgid),
        }
    }
}

fn killpg(pgid: libc::pid_t, signal: libc::c_int) -> io::Result<()> {
    // SAFETY: FFI call with plain integer arguments. `pgid` is the group of a child this process
    // spawned (never 0 or 1, which would address our own group or init).
    if unsafe { libc::killpg(pgid, signal) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn signal_group(pgid: libc::pid_t, signal: libc::c_int) -> io::Result<()> {
    match killpg(pgid, signal) {
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => Ok(()),
        result => result,
    }
}

/// The exit code, or the signal that killed the process.
pub(super) fn exit_parts(status: ExitStatus) -> (Option<i32>, Option<i32>) {
    (status.code(), status.signal())
}

/// A watched process (see `ProcessWatch`): Unix pids are allocated in increasing order, so the pid
/// alone identifies it for the watch's short life.
#[derive(Debug)]
pub(super) struct Watch {
    pid: libc::pid_t,
}

impl Watch {
    pub(super) fn open(pid: u32) -> Self {
        Self {
            pid: libc::pid_t::try_from(pid).unwrap_or(0),
        }
    }

    pub(super) fn is_alive(&self) -> bool {
        if self.pid <= 0 {
            return false;
        }
        // SAFETY: FFI call with plain integer arguments; signal 0 only checks that the process
        // exists.
        if unsafe { libc::kill(self.pid, 0) } != 0 {
            // EPERM: it exists but belongs to someone else.
            return io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH);
        }
        !is_zombie(self.pid)
    }
}

/// Nothing to retry on Unix: removal errors are not transient.
pub(super) fn is_transient_removal_error(_error: &io::Error) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn live_member_exists(pgid: libc::pid_t) -> bool {
    // If /proc cannot be read, trust killpg: the group exists.
    proc_stat::group_has_live_member(pgid).unwrap_or(true)
}

#[cfg(not(target_os = "linux"))]
fn live_member_exists(_pgid: libc::pid_t) -> bool {
    true
}

#[cfg(target_os = "linux")]
fn is_zombie(pid: libc::pid_t) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| proc_stat::parse(&stat))
        .is_some_and(|(state, _)| proc_stat::exited(state))
}

#[cfg(not(target_os = "linux"))]
fn is_zombie(_pid: libc::pid_t) -> bool {
    false
}

/// `/proc/<pid>/stat` (Linux).
#[cfg(any(target_os = "linux", test))]
mod proc_stat {
    /// The state and process group from a `/proc/<pid>/stat` line. The command name (field 2) is
    /// in parentheses and may itself contain spaces and parentheses, so parsing starts after the
    /// last `)`.
    pub(super) fn parse(stat: &str) -> Option<(char, libc::pid_t)> {
        let rest = &stat[stat.rfind(')')? + 1..];
        let mut fields = rest.split_ascii_whitespace();
        let state = fields.next()?.chars().next()?;
        let _ppid = fields.next()?;
        let pgrp = fields.next()?.parse().ok()?;
        Some((state, pgrp))
    }

    /// Zombie (`Z`) or dead (`X`, `x`): exited, possibly not yet reaped.
    pub(super) fn exited(state: char) -> bool {
        matches!(state, 'Z' | 'X' | 'x')
    }

    #[cfg(target_os = "linux")]
    pub(super) fn group_has_live_member(pgid: libc::pid_t) -> std::io::Result<bool> {
        for entry in std::fs::read_dir("/proc")? {
            let entry = entry?;
            let name = entry.file_name();
            if !name
                .to_str()
                .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
            {
                continue;
            }
            // A process that exits while we look is simply gone.
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                continue;
            };
            if parse(&stat).is_some_and(|(state, pgrp)| pgrp == pgid && !exited(state)) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_stat_lines() {
        assert_eq!(
            proc_stat::parse("4242 (fake-leapp) S 1 4240 4240 0 -1 4194560 120"),
            Some(('S', 4240))
        );
        // The command name may contain spaces and parentheses.
        assert_eq!(proc_stat::parse("7 (a) b (c)) Z 1 9 9 0"), Some(('Z', 9)));
        assert_eq!(proc_stat::parse("garbage"), None);
        assert_eq!(proc_stat::parse("1 (x) R"), None);
        assert!(proc_stat::exited('Z'));
        assert!(!proc_stat::exited('S'));
    }

    #[test]
    fn watch_this_process_and_a_reaped_child() {
        assert!(Watch::open(std::process::id()).is_alive());
        assert!(!Watch::open(0).is_alive());
        let mut child = Command::new("sleep").arg("30").spawn().unwrap();
        let watch = Watch::open(child.id());
        assert!(watch.is_alive());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(!watch.is_alive());
    }

    #[test]
    fn terminate_stops_a_new_session() {
        let mut command = Command::new("sleep");
        command.arg("30");
        configure(&mut command);
        let mut child = command.spawn().unwrap();
        let tree = Tree::attach(&mut child).unwrap();
        assert!(tree.is_alive());
        tree.terminate().unwrap();
        let status = child.wait().unwrap();
        assert_eq!(exit_parts(status), (None, Some(libc::SIGTERM)));
        // The group can outlive its reaped leader for a moment (seen on macOS), hence the poll.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while tree.is_alive() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!tree.is_alive());
    }
}
