//! The app log, `<app_log>/suitedfir.log` (ARCHITECTURE.md §8): our own `log` backend, one line per
//! record (`<UTC time> <LEVEL> <target>: <message>`). The file is truncated when it would grow past
//! 5 MB, so it never grows without bound.
//!
//! It never contains secrets. The code never logs passwords, and the types that hold them print
//! `<redacted>` (DEVELOPMENT.md §4.3). As a second line of defence every line is also passed through
//! [`redact`], which masks the value of `--itunes_password`, of the `BACKUP_PASSWORD` variables and
//! of JSON-style `…password` fields.

use std::borrow::Cow;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;

use log::{LevelFilter, Log, Metadata, Record};
use suitedfir_core::contracts::Timestamp;

/// The log is truncated when it would grow past this size.
pub const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

const REDACTED: &str = "<redacted>";

/// The file logger.
pub struct FileLogger {
    max_bytes: u64,
    level: LevelFilter,
    file: Mutex<Option<File>>,
}

impl FileLogger {
    /// Opens (creates) the log file for appending. A file already over `max_bytes` is truncated.
    pub fn open(path: &Path, max_bytes: u64, level: LevelFilter) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        if file.metadata()?.len() > max_bytes {
            file.set_len(0)?;
        }
        Ok(Self {
            max_bytes,
            level,
            file: Mutex::new(Some(file)),
        })
    }

    /// Writes one line (redacted), truncating the file first if the line would take it past the
    /// size limit. Errors are dropped: logging must never fail a command.
    pub fn write_line(&self, level: log::Level, target: &str, message: &str) {
        let line = format!("{} {level:<5} {target}: {message}", Timestamp::now());
        let line = redact(&line);
        let mut guard = self.file.lock().unwrap_or_else(|e| e.into_inner());
        let Some(file) = guard.as_mut() else {
            return;
        };
        let size = file.metadata().map(|m| m.len()).unwrap_or(0);
        let adding = line.len() as u64 + 1;
        if size + adding > self.max_bytes {
            let _ = file.set_len(0);
            let _ = writeln!(
                file,
                "{} INFO  suitedfir: the log reached {} bytes and was truncated",
                Timestamp::now(),
                self.max_bytes
            );
        }
        let _ = writeln!(file, "{line}");
    }
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            self.write_line(record.level(), record.target(), &record.args().to_string());
        }
    }

    fn flush(&self) {
        if let Some(file) = self.file.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            let _ = file.flush();
        }
    }
}

/// Installs the file logger as the `log` backend (once per process).
pub fn init(path: &Path) -> io::Result<()> {
    let level = if cfg!(debug_assertions) {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };
    let logger = FileLogger::open(path, MAX_LOG_BYTES, level)?;
    // The logger lives as long as the process.
    log::set_logger(Box::leak(Box::new(logger))).map_err(|e| io::Error::other(e.to_string()))?;
    log::set_max_level(level);
    Ok(())
}

/// Masks secrets in a log line: the argument after a `--…password` flag (`--itunes_password`),
/// and the value assigned (`=` or `:`) to a name ending in `password` or `BACKUP_PASSWORD_NEW`
/// (`BACKUP_PASSWORD=…`, `"itunes_password": "…"`, `encryption_password: Some("…")`).
/// `<redacted>`, `null` and `None` values are left as they are, and so is the word "password" in
/// prose.
pub fn redact(line: &str) -> Cow<'_, str> {
    let lower = line.to_ascii_lowercase();
    if !lower.contains("password") {
        return Cow::Borrowed(line);
    }
    let bytes = line.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b == b'-';
    let mut out = String::with_capacity(line.len());
    let mut copied = 0;
    let mut search = 0;
    while let Some(found) = lower[search..].find("password") {
        let at = search + found;
        let mut start = at;
        while start > 0 && is_ident(bytes[start - 1]) {
            start -= 1;
        }
        let mut end = at + "password".len();
        while end < bytes.len() && is_ident(bytes[end]) {
            end += 1;
        }
        search = end;
        // ASCII lowercasing keeps byte offsets, so `lower` and `line` share them.
        let ident = &lower[start..end];
        let flag = ident.starts_with("--");
        let name = ident.trim_start_matches('-');
        if !(name.ends_with("password") || name == "backup_password_new") {
            continue;
        }
        let Some((value_start, value_end)) = secret_after(line, end, flag) else {
            continue;
        };
        let value = &line[value_start..value_end];
        if value.is_empty() || value == REDACTED {
            continue;
        }
        out.push_str(&line[copied..value_start]);
        out.push_str(REDACTED);
        copied = value_end;
        search = value_end;
    }
    if copied == 0 {
        return Cow::Borrowed(line);
    }
    out.push_str(&line[copied..]);
    Cow::Owned(out)
}

/// The byte range of the value that follows a password name ending at `end`: after an optional
/// closing quote of the name and a separator run that holds `=` or `:` (or, for a `--` flag,
/// whitespace), either a quoted string (without the quotes, also inside `Some(…)`) or a bare word.
/// `None` when nothing is assigned or the value is `null` / `None`.
fn secret_after(line: &str, end: usize, flag: bool) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut i = end;
    // A closing quote of the name (`"itunes_password"`, `'password'`).
    if i < bytes.len() && (bytes[i] == b'"' || bytes[i] == b'\'') {
        i += 1;
    }
    let (mut assigned, mut spaced) = (false, false);
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b':' | b'=') {
        if matches!(bytes[i], b':' | b'=') {
            assigned = true;
        } else {
            spaced = true;
        }
        i += 1;
    }
    if !(assigned || (flag && spaced)) || i >= bytes.len() {
        return None;
    }
    if line[i..].starts_with("Some(") {
        i += "Some(".len();
    }
    if i >= bytes.len() {
        return None;
    }
    if let quote @ (b'"' | b'\'') = bytes[i] {
        let start = i + 1;
        let mut j = start;
        while j < bytes.len() && bytes[j] != quote {
            if bytes[j] == b'\\' {
                j += 1;
            }
            j += 1;
        }
        return Some((start, j.min(bytes.len())));
    }
    let start = i;
    let mut j = start;
    while j < bytes.len() && !matches!(bytes[j], b' ' | b'\t' | b',' | b'}' | b')' | b']') {
        j += 1;
    }
    let value = &line[start..j];
    (value != "null" && value != "None").then_some((start, j))
}

#[cfg(test)]
mod tests {
    use super::*;

    use suitedfir_core::contracts::examples;

    const SECRET: &str = "Hunter2-Backup!";

    #[test]
    fn redacts_every_password_form() {
        let cases = [
            (
                format!("argv: /bin/ileapp -t itunes --itunes_password {SECRET} -tz UTC"),
                "argv: /bin/ileapp -t itunes --itunes_password <redacted> -tz UTC",
            ),
            (
                format!("env BACKUP_PASSWORD={SECRET} BACKUP_PASSWORD_NEW={SECRET}"),
                "env BACKUP_PASSWORD=<redacted> BACKUP_PASSWORD_NEW=<redacted>",
            ),
            (
                format!(r#"{{"itunes_password": "{SECRET}", "label": "x"}}"#),
                r#"{"itunes_password": "<redacted>", "label": "x"}"#,
            ),
            (
                format!(r#"req {{ encryption_password: Some("{SECRET}") }}"#),
                r#"req { encryption_password: Some("<redacted>") }"#,
            ),
            (format!("password={SECRET}"), "password=<redacted>"),
        ];
        for (line, expected) in cases {
            assert_eq!(redact(&line), expected, "{line}");
        }
        // Already redacted, null or prose: unchanged.
        for line in [
            r#"{"itunes_password": null}"#,
            "--itunes_password <redacted>",
            "RunRequest { itunes_password: Some(\"<redacted>\") }",
            "a password is required.",
            "nothing secret here",
        ] {
            assert_eq!(redact(line), line);
        }
    }

    #[test]
    fn the_log_file_never_holds_a_password() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log").join("suitedfir.log");
        let logger = FileLogger::open(&path, MAX_LOG_BYTES, LevelFilter::Debug).unwrap();
        // What the code logs about requests and commands (the Debug forms redact by hand)...
        let mut request = examples::run_request();
        request.itunes_password = Some(SECRET.to_owned());
        logger.write_line(
            log::Level::Info,
            "suitedfir",
            &format!("run_start {request:?}"),
        );
        let mut acq = examples::acq_request();
        acq.encryption_password = Some(SECRET.to_owned());
        logger.write_line(log::Level::Info, "suitedfir", &format!("acq_start {acq:?}"));
        // ...and a careless line that would leak without the redaction.
        logger.write_line(
            log::Level::Error,
            "suitedfir",
            &format!("spawn failed: ileapp --itunes_password {SECRET}"),
        );
        logger.write_line(
            log::Level::Warn,
            "suitedfir",
            &format!(r#"request body {{"password": "{SECRET}"}}"#),
        );
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains(SECRET), "{text}");
        assert!(!text.contains(examples::EXAMPLE_PASSWORD), "{text}");
        assert_eq!(text.lines().count(), 4, "{text}");
        assert!(text.contains("<redacted>"));
        let first = text.lines().next().unwrap();
        assert!(
            first.contains(" INFO  suitedfir: run_start RunRequest"),
            "{first}"
        );
    }

    #[test]
    fn the_log_is_truncated_at_its_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("suitedfir.log");
        let logger = FileLogger::open(&path, 1000, LevelFilter::Info).unwrap();
        for i in 0..100 {
            logger.write_line(
                log::Level::Info,
                "t",
                &format!("line {i:03} {}", "x".repeat(40)),
            );
        }
        let size = fs::metadata(&path).unwrap().len();
        assert!(size <= 1000, "{size}");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("was truncated"), "{text}");
        assert!(text.contains("line 099"), "{text}");
        // A file already over the limit is truncated when opened.
        drop(logger);
        fs::write(&path, vec![b'x'; 2000]).unwrap();
        let _logger = FileLogger::open(&path, 1000, LevelFilter::Info).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().len(), 0);
    }
}
