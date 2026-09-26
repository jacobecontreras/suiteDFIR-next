//! The machine a record is written on (`run.json` / `acquisition.json` `host`, CONTRACTS.md §7.1),
//! detected once at startup without FFI: the OS and CPU from std, the OS version and host name
//! from the OS's own files and tools (on Windows `cmd.exe` by its absolute system path). A value
//! that cannot be read is recorded as `unknown`. (This per-OS code lives outside the platform files of
//! DEVELOPMENT.md §4.5 as a documented exception: it has no `unsafe` and no FFI.)

use std::process::{Command, Stdio};

use suitedfir_core::contracts::RecordHost;

const UNKNOWN: &str = "unknown";

pub fn detect() -> RecordHost {
    RecordHost {
        os: std::env::consts::OS.to_owned(),
        os_version: os_version().unwrap_or_else(|| UNKNOWN.to_owned()),
        arch: std::env::consts::ARCH.to_owned(),
        hostname: hostname().unwrap_or_else(|| UNKNOWN.to_owned()),
    }
}

/// The trimmed stdout of a small system tool, if it ran successfully and printed something.
fn tool_output(program: &str, args: &[&str]) -> Option<String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: the app has no console, so none may flash up.
        command.creation_flags(0x0800_0000);
    }
    let output = command.output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !text.is_empty()).then_some(text)
}

#[cfg(target_os = "macos")]
fn os_version() -> Option<String> {
    tool_output("/usr/bin/sw_vers", &["-productVersion"])
}

#[cfg(target_os = "macos")]
fn hostname() -> Option<String> {
    tool_output("/usr/sbin/scutil", &["--get", "LocalHostName"])
        .or_else(|| tool_output("/bin/hostname", &[]))
}

/// `ver` of `%SystemRoot%\System32\cmd.exe`, by its absolute path: a bare `cmd.exe` would be
/// looked up in the app's own folder first.
#[cfg(windows)]
fn os_version() -> Option<String> {
    let cmd = system_cmd()?;
    windows_version(&tool_output(&cmd.to_string_lossy(), &["/D", "/C", "ver"])?)
}

/// `%SystemRoot%\System32\cmd.exe` (or `%windir%`), if that is an existing file.
#[cfg(windows)]
fn system_cmd() -> Option<std::path::PathBuf> {
    let root = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("windir"))?;
    let cmd = std::path::Path::new(&root).join("System32").join("cmd.exe");
    (cmd.is_absolute() && cmd.is_file()).then_some(cmd)
}

/// `Microsoft Windows [Version 10.0.26100.4652]` → `10.0.26100.4652`.
#[cfg(any(windows, test))]
fn windows_version(ver: &str) -> Option<String> {
    let start = ver.find("Version ")? + "Version ".len();
    let rest = &ver[start..];
    let end = rest.find(']').unwrap_or(rest.len());
    let version = rest[..end].trim();
    (!version.is_empty()).then(|| version.to_owned())
}

#[cfg(windows)]
fn hostname() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|name| !name.is_empty())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn os_version() -> Option<String> {
    let release = std::fs::read_to_string("/etc/os-release")
        .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
        .ok()?;
    os_release_name(&release)
}

/// `PRETTY_NAME` from os-release, e.g. `Ubuntu 24.04.1 LTS`.
#[cfg(any(not(any(target_os = "macos", windows)), test))]
fn os_release_name(release: &str) -> Option<String> {
    release.lines().find_map(|line| {
        let value = line.strip_prefix("PRETTY_NAME=")?.trim();
        let value = value.trim_matches('"').trim_matches('\'').trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

#[cfg(not(any(target_os = "macos", windows)))]
fn hostname() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .or_else(|| tool_output("hostname", &[]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_this_machine() {
        let host = detect();
        assert_eq!(host.os, std::env::consts::OS);
        assert_eq!(host.arch, std::env::consts::ARCH);
        assert!(!host.os_version.is_empty() && !host.hostname.is_empty());
        // Every supported OS reports its version and host name.
        assert_ne!(host.os_version, UNKNOWN);
        assert_ne!(host.hostname, UNKNOWN);
    }

    #[test]
    fn parses_windows_ver() {
        assert_eq!(
            windows_version("\r\nMicrosoft Windows [Version 10.0.26100.4652]\r\n").as_deref(),
            Some("10.0.26100.4652")
        );
        assert_eq!(windows_version("no version here"), None);
    }

    #[test]
    fn parses_os_release() {
        let text = "NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\n";
        assert_eq!(os_release_name(text).as_deref(), Some("Ubuntu 24.04.1 LTS"));
        assert_eq!(os_release_name("NAME=x\n"), None);
    }
}
