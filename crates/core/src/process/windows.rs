//! Windows process trees: every spawn gets its own job object with kill-on-close. The child is
//! created suspended, assigned to the job and only then resumed, so no process of the tree ever
//! runs outside the job (ARCHITECTURE.md §7).
//!
//! `unsafe` here is FFI (job objects, the Toolhelp thread snapshot, thread and process handles)
//! and taking ownership of the handles those calls return (`OwnedHandle::from_raw_handle`, which
//! closes them), each commented.

use std::fs;
use std::io;
use std::mem::{offset_of, size_of};
use std::os::windows::fs::MetadataExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, ExitStatus};
use std::ptr;

use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_DIR_NOT_EMPTY, ERROR_INVALID_PARAMETER, ERROR_LOCK_VIOLATION,
    ERROR_SHARING_VIOLATION, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenProcess, OpenThread, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, ResumeThread, THREAD_SUSPEND_RESUME, TerminateProcess,
    WaitForSingleObject,
};

/// There is no graceful signal: terminating the job is final.
pub(super) const GRACEFUL_TERMINATE: bool = false;
/// The exit code of processes stopped by terminating the job (or a failed setup).
const TERMINATED_EXIT_CODE: u32 = 1;

/// Suspended, so it can join the job before running any code; no console window.
pub(super) fn configure(command: &mut Command) {
    command.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW);
}

/// The job object holding the tree. Dropping it closes the handle, which kills whatever is left.
pub(super) struct Tree {
    job: OwnedHandle,
}

impl Tree {
    /// Creates the job (kill-on-close), assigns the suspended child and resumes it. On failure the
    /// child is terminated and waited for before the error is returned.
    pub(super) fn attach(child: &mut Child) -> io::Result<Self> {
        match Self::attach_suspended(child) {
            Ok(tree) => Ok(tree),
            Err(e) => {
                // SAFETY: FFI call on the child's process handle, which `child` keeps open.
                unsafe {
                    TerminateProcess(child.as_raw_handle(), TERMINATED_EXIT_CODE);
                }
                let _ = child.wait();
                Err(e)
            }
        }
    }

    fn attach_suspended(child: &Child) -> io::Result<Self> {
        let tree = Self { job: create_job()? };
        // SAFETY: FFI call with two valid handles: the job we own and the child's process handle.
        if unsafe { AssignProcessToJobObject(tree.raw(), child.as_raw_handle()) } == 0 {
            return Err(last_error("AssignProcessToJobObject"));
        }
        if let Err(e) = resume_threads(child.id()) {
            let _ = tree.terminate();
            return Err(e);
        }
        Ok(tree)
    }

    fn raw(&self) -> RawHandle {
        self.job.as_raw_handle()
    }

    /// Terminates every process in the job.
    pub(super) fn terminate(&self) -> io::Result<()> {
        // SAFETY: FFI call on the job handle we own.
        if unsafe { TerminateJobObject(self.raw(), TERMINATED_EXIT_CODE) } == 0 {
            return Err(last_error("TerminateJobObject"));
        }
        Ok(())
    }

    /// The same as `terminate`: Windows has no stronger way to stop a process.
    pub(super) fn kill(&self) -> io::Result<()> {
        self.terminate()
    }

    /// Whether the job still has active processes. If the job cannot be queried, it is assumed to.
    pub(super) fn is_alive(&self) -> bool {
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: FFI call on the job handle we own. `info` is writable memory of exactly the size
        // passed, of the structure this information class fills; the return-length pointer may be
        // null.
        let ok = unsafe {
            QueryInformationJobObject(
                self.raw(),
                JobObjectBasicAccountingInformation,
                (&raw mut info).cast(),
                struct_size::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>(),
                ptr::null_mut(),
            )
        };
        ok == 0 || info.ActiveProcesses > 0
    }
}

/// A new, unnamed job whose processes are all killed when its last handle is closed (so also when
/// this app dies).
fn create_job() -> io::Result<OwnedHandle> {
    // SAFETY: FFI call; null security attributes and a null name are allowed.
    let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
    if raw.is_null() {
        return Err(last_error("CreateJobObjectW"));
    }
    // SAFETY: `raw` is a valid handle that nothing else owns, so `OwnedHandle` may close it.
    let job = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: FFI call on the job handle we own. `limits` is a valid structure of exactly the size
    // passed, of the type this information class expects; it is only read.
    let ok = unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            struct_size::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>(),
        )
    };
    if ok == 0 {
        return Err(last_error("SetInformationJobObject"));
    }
    Ok(job)
}

/// Resumes the threads of the suspended process `pid` (a new process has exactly one). Std offers
/// no handle to the main thread, hence the Toolhelp snapshot.
fn resume_threads(pid: u32) -> io::Result<()> {
    // SAFETY: FFI call with plain arguments.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(last_error("CreateToolhelp32Snapshot"));
    }
    // SAFETY: `raw` is a valid snapshot handle that nothing else owns.
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
    // An entry is usable only if the call filled it up to and including the owner process id.
    let owner_end = offset_of!(THREADENTRY32, th32OwnerProcessID) + size_of::<u32>();
    let mut entry = THREADENTRY32 {
        dwSize: struct_size::<THREADENTRY32>(),
        ..THREADENTRY32::default()
    };
    let mut resumed = 0;
    // SAFETY: FFI call on the snapshot handle we own; `entry` is writable, with `dwSize` set as
    // the API requires.
    let mut more = unsafe { Thread32First(snapshot.as_raw_handle(), &raw mut entry) } != 0;
    while more {
        if usize::try_from(entry.dwSize).is_ok_and(|size| size >= owner_end)
            && entry.th32OwnerProcessID == pid
        {
            resume_thread(entry.th32ThreadID)?;
            resumed += 1;
        }
        entry.dwSize = struct_size::<THREADENTRY32>();
        // SAFETY: as for Thread32First.
        more = unsafe { Thread32Next(snapshot.as_raw_handle(), &raw mut entry) } != 0;
    }
    if resumed == 0 {
        return Err(io::Error::other("the new process has no thread to resume"));
    }
    Ok(())
}

fn resume_thread(thread_id: u32) -> io::Result<()> {
    // SAFETY: FFI call with plain arguments.
    let raw = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
    if raw.is_null() {
        return Err(last_error("OpenThread"));
    }
    // SAFETY: `raw` is a valid thread handle that nothing else owns.
    let thread = unsafe { OwnedHandle::from_raw_handle(raw) };
    // SAFETY: FFI call on the thread handle we own, opened with THREAD_SUSPEND_RESUME.
    if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
        return Err(last_error("ResumeThread"));
    }
    Ok(())
}

/// The exit code; Windows has no signals.
pub(super) fn exit_parts(status: ExitStatus) -> (Option<i32>, Option<i32>) {
    (status.code(), None)
}

/// A watched process (see `ProcessWatch`). The open handle keeps the process object, and so its
/// pid, from being reused while the watch exists.
#[derive(Debug)]
pub(super) enum Watch {
    Open(OwnedHandle),
    /// There was no such process when the watch was opened.
    Gone,
    /// The process exists but may not be opened (not ours).
    Inaccessible,
}

impl Watch {
    pub(super) fn open(pid: u32) -> Self {
        if pid == 0 {
            return Self::Gone;
        }
        // SAFETY: FFI call with plain arguments.
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if raw.is_null() {
            return if io::Error::last_os_error().raw_os_error()
                == Some(win32(ERROR_INVALID_PARAMETER))
            {
                Self::Gone
            } else {
                Self::Inaccessible
            };
        }
        // SAFETY: `raw` is a valid process handle that nothing else owns.
        Self::Open(unsafe { OwnedHandle::from_raw_handle(raw) })
    }

    pub(super) fn is_alive(&self) -> bool {
        match self {
            // SAFETY: FFI call on the process handle we own, opened with SYNCHRONIZE; a timeout of
            // 0 only tests whether the process has exited.
            Self::Open(process) => unsafe {
                WaitForSingleObject(process.as_raw_handle(), 0) != WAIT_OBJECT_0
            },
            Self::Gone => false,
            Self::Inaccessible => true,
        }
    }
}

/// A real directory: not a symlink, a junction or any other reparse point (`metadata` is from
/// `symlink_metadata`).
pub(super) fn is_plain_dir(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_dir() && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
}

/// Errors that a just-terminated process's open files cause for a short while.
pub(super) fn is_transient_removal_error(error: &io::Error) -> bool {
    error.raw_os_error().is_some_and(|code| {
        [
            ERROR_SHARING_VIOLATION,
            ERROR_LOCK_VIOLATION,
            ERROR_ACCESS_DENIED,
            ERROR_DIR_NOT_EMPTY,
        ]
        .into_iter()
        .any(|transient| code == win32(transient))
    })
}

/// A Win32 error code as `io::Error::raw_os_error` reports it.
fn win32(code: u32) -> i32 {
    code.cast_signed()
}

/// The size of a Win32 structure as the `u32` the APIs take (these structures are tiny).
fn struct_size<T>() -> u32 {
    u32::try_from(size_of::<T>()).unwrap_or(u32::MAX)
}

fn last_error(function: &str) -> io::Error {
    let error = io::Error::last_os_error();
    io::Error::new(error.kind(), format!("{function} failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_this_process_and_a_killed_child() {
        assert!(Watch::open(std::process::id()).is_alive());
        assert!(!Watch::open(0).is_alive());
        let mut child = Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let watch = Watch::open(child.id());
        assert!(watch.is_alive());
        child.kill().unwrap();
        child.wait().unwrap();
        drop(child);
        // The watch's own handle keeps answering for this process.
        assert!(!watch.is_alive());
    }

    #[test]
    fn transient_removal_errors() {
        for code in [32, 33, 5, 145] {
            assert!(is_transient_removal_error(&io::Error::from_raw_os_error(
                code
            )));
        }
        assert!(!is_transient_removal_error(&io::Error::from_raw_os_error(
            2
        )));
    }
}
