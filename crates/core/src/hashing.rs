//! SHA-256 of files (ARCHITECTURE.md D18): plain, with progress and cancel (input hashing), and
//! `seal_tree`, which writes a hash manifest of a folder (`report.sha256`, `backup.sha256`;
//! CONTRACTS.md §8). Files are only ever opened read-only.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::contracts::{Reason, Seal, SealStatus};
use crate::fsutil;

const BUFFER_SIZE: usize = 1 << 20;

/// Progress callbacks come at most once per interval (at most 10 per second), plus one at the end.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// The lowercase hex SHA-256 of a file's contents, streamed through a read-only handle (the file
/// is never opened for writing, so evidence inputs can be hashed).
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let never = AtomicBool::new(false);
    match hash_stream(&mut file, &never, &mut |_| {})? {
        Some((hash, _)) => Ok(hash),
        None => Err(io::Error::other("hashing was cancelled")),
    }
}

/// The result of a cancellable hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HashOutcome {
    /// The lowercase hex SHA-256.
    Completed(String),
    Cancelled,
}

/// [`sha256_file`] for input hashing: 1 MiB reads, `progress(bytes_done, bytes_total)` at most every
/// [`PROGRESS_INTERVAL`] and once when done, and it stops soon after `cancel` is set (without a
/// final callback). `bytes_total` is the file size when hashing starts.
pub fn sha256_file_with_progress(
    path: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, u64),
) -> io::Result<HashOutcome> {
    let mut file = File::open(path)?;
    let total = file.metadata()?.len();
    let mut throttle = Throttle::new();
    let result = hash_stream(&mut file, cancel, &mut |done| {
        if throttle.ready() {
            progress(done, total);
        }
    })?;
    Ok(match result {
        Some((hash, done)) => {
            progress(done, total);
            HashOutcome::Completed(hash)
        }
        None => HashOutcome::Cancelled,
    })
}

/// Hashes `reader` to the end: `(hash, bytes)`, or `None` if `cancel` was set. `on_chunk` gets the
/// bytes done after every read.
fn hash_stream(
    reader: &mut dyn Read,
    cancel: &AtomicBool,
    on_chunk: &mut dyn FnMut(u64),
) -> io::Result<Option<(String, u64)>> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; BUFFER_SIZE];
    let mut done: u64 = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                hasher.update(&buffer[..n]);
                done = done.saturating_add(n as u64);
                on_chunk(done);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(Some((to_hex(&hasher.finalize()), done)))
}

/// Lets a callback through at most once per [`PROGRESS_INTERVAL`].
struct Throttle {
    last: Instant,
}

impl Throttle {
    fn new() -> Self {
        Self {
            last: Instant::now(),
        }
    }

    fn ready(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.last) >= PROGRESS_INTERVAL {
            self.last = now;
            true
        } else {
            false
        }
    }
}

// ---- seal_tree ----

/// What [`seal_tree`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealOutcome {
    /// `cancel` was set: no manifest was written.
    pub cancelled: bool,
    /// The SHA-256 of the manifest file; `None` when cancelled.
    pub manifest_sha256: Option<String>,
    /// Regular files listed (hashed so far, when cancelled).
    pub file_count: u64,
    /// Their total size in bytes (so far, when cancelled).
    pub total_bytes: u64,
    /// Symlinks found; they are neither followed nor listed.
    pub symlinks: u64,
    /// Listed files whose path is not valid Unicode and was written lossily (Windows only).
    pub unencodable_names: u64,
}

impl SealOutcome {
    /// The record's `output.seal`: `sealed` with the manifest, or `cancelled` with nulls.
    pub fn seal(&self, manifest_name: &str) -> Seal {
        match &self.manifest_sha256 {
            Some(hash) if !self.cancelled => Seal {
                status: SealStatus::Sealed,
                manifest: Some(manifest_name.to_owned()),
                manifest_sha256: Some(hash.clone()),
                file_count: Some(self.file_count),
                total_bytes: Some(self.total_bytes),
            },
            _ => Seal {
                status: SealStatus::Cancelled,
                manifest: None,
                manifest_sha256: None,
                file_count: None,
                total_bytes: None,
            },
        }
    }

    /// The warnings of a seal: the symlink count under `symlinks_code` (`symlinks_in_report` or
    /// `symlinks_in_backup`), and `unencodable_filename`.
    pub fn warnings(&self, symlinks_code: &str) -> Vec<Reason> {
        let mut warnings = Vec::new();
        if self.symlinks > 0 {
            warnings.push(Reason {
                code: symlinks_code.to_owned(),
                message: format!(
                    "{} symbolic link(s) were neither followed nor listed in the manifest",
                    self.symlinks
                ),
            });
        }
        if self.unencodable_names > 0 {
            warnings.push(Reason {
                code: "unencodable_filename".to_owned(),
                message: format!(
                    "{} file name(s) are not valid Unicode and were written lossily in the manifest",
                    self.unencodable_names
                ),
            });
        }
        warnings
    }
}

/// A regular file found by the walk.
struct Listed {
    /// Relative to the manifest's folder, `/`-separated, as written in the manifest.
    rel: Vec<u8>,
    path: PathBuf,
}

/// Hashes every regular file under `dir` into a GNU `sha256sum` manifest at `manifest_path`
/// (CONTRACTS.md §8), so `sha256sum -c` works from the manifest's folder:
/// - one line per regular file, `<hash>  <path>`, with the path relative to the manifest's folder
///   and `/`-separated (`report/index.html`), sorted by the byte order of that path, LF endings;
/// - a path containing `\`, LF or CR starts its line with `\`, and those are written `\\`, `\n` and
///   `\r`;
/// - names are raw bytes on Unix and UTF-8 on Windows (lossy for invalid names, which are counted);
/// - symlinks are counted, never followed or listed.
///
/// `dir` must be a real folder strictly inside the manifest's folder, and the manifest must not
/// exist yet (it is written atomically). `progress(files_done, files_total)` comes at most every
/// [`PROGRESS_INTERVAL`] and once at the end. When `cancel` is set, hashing stops and nothing is
/// written. Any read error fails the seal.
pub fn seal_tree(
    dir: &Path,
    manifest_path: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64, Option<u64>),
) -> io::Result<SealOutcome> {
    let invalid = |message: String| io::Error::new(io::ErrorKind::InvalidInput, message);
    let base = manifest_path
        .parent()
        .ok_or_else(|| invalid(format!("{} has no folder", manifest_path.display())))?;
    let prefix = dir
        .strip_prefix(base)
        .ok()
        .filter(|prefix| !prefix.as_os_str().is_empty())
        .ok_or_else(|| {
            invalid(format!(
                "{} is not inside the manifest's folder {}",
                dir.display(),
                base.display()
            ))
        })?;
    if !fs::symlink_metadata(dir)?.is_dir() {
        return Err(invalid(format!("{} is not a folder", dir.display())));
    }
    if fs::symlink_metadata(manifest_path).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", manifest_path.display()),
        ));
    }

    let mut outcome = SealOutcome {
        cancelled: false,
        manifest_sha256: None,
        file_count: 0,
        total_bytes: 0,
        symlinks: 0,
        unencodable_names: 0,
    };
    let (mut rel_prefix, mut prefix_lossy) = (Vec::new(), false);
    for component in prefix.components() {
        let (bytes, lossy) = fsutil::manifest_name_bytes(component.as_os_str());
        if !rel_prefix.is_empty() {
            rel_prefix.push(b'/');
        }
        rel_prefix.extend(bytes);
        prefix_lossy |= lossy;
    }
    let Some(mut files) = walk(dir, rel_prefix, prefix_lossy, cancel, &mut outcome)? else {
        outcome.cancelled = true;
        return Ok(outcome);
    };
    files.sort_by(|a, b| a.rel.cmp(&b.rel));

    let files_total = files.len() as u64;
    let mut throttle = Throttle::new();
    let mut manifest = Vec::new();
    for file in &files {
        let mut handle = File::open(&file.path)?;
        let Some((hash, bytes)) = hash_stream(&mut handle, cancel, &mut |_| {})? else {
            outcome.cancelled = true;
            return Ok(outcome);
        };
        manifest.extend(manifest_line(&hash, &file.rel));
        outcome.file_count += 1;
        outcome.total_bytes = outcome.total_bytes.saturating_add(bytes);
        if throttle.ready() {
            progress(outcome.file_count, Some(files_total));
        }
    }
    if cancel.load(Ordering::Relaxed) {
        outcome.cancelled = true;
        return Ok(outcome);
    }
    fsutil::write_file_atomic(manifest_path, &manifest)?;
    outcome.manifest_sha256 = Some(to_hex(&Sha256::digest(&manifest)));
    progress(outcome.file_count, Some(files_total));
    Ok(outcome)
}

/// Lists the regular files under `dir` without following symlinks; `None` if cancelled.
fn walk(
    dir: &Path,
    rel_prefix: Vec<u8>,
    prefix_lossy: bool,
    cancel: &AtomicBool,
    outcome: &mut SealOutcome,
) -> io::Result<Option<Vec<Listed>>> {
    let mut files = Vec::new();
    let mut pending = vec![(dir.to_path_buf(), rel_prefix, prefix_lossy)];
    while let Some((folder, rel, lossy)) = pending.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        for entry in fs::read_dir(&folder)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let (name, name_lossy) = fsutil::manifest_name_bytes(&entry.file_name());
            let mut child = rel.clone();
            child.push(b'/');
            child.extend(name);
            let child_lossy = lossy || name_lossy;
            if kind.is_symlink() {
                outcome.symlinks += 1;
            } else if kind.is_dir() {
                pending.push((entry.path(), child, child_lossy));
            } else if kind.is_file() {
                if child_lossy {
                    outcome.unencodable_names += 1;
                }
                files.push(Listed {
                    rel: child,
                    path: entry.path(),
                });
            } else {
                log::warn!(
                    "{} is not a regular file; it is not listed in the manifest",
                    entry.path().display()
                );
            }
        }
    }
    Ok(Some(files))
}

/// One manifest line in GNU `sha256sum` format, escaped as coreutils does.
fn manifest_line(hash: &str, rel: &[u8]) -> Vec<u8> {
    let mut line = Vec::with_capacity(hash.len() + rel.len() + 4);
    if rel.iter().any(|b| matches!(b, b'\\' | b'\n' | b'\r')) {
        line.push(b'\\');
    }
    line.extend_from_slice(hash.as_bytes());
    line.extend_from_slice(b"  ");
    for &byte in rel {
        match byte {
            b'\\' => line.extend_from_slice(b"\\\\"),
            b'\n' => line.extend_from_slice(b"\\n"),
            b'\r' => line.extend_from_slice(b"\\r"),
            other => line.push(other),
        }
    }
    line.push(b'\n');
    line
}

/// Lowercase hex without a prefix (CONTRACTS.md §1).
pub(crate) fn to_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(char::from(DIGITS[usize::from(b >> 4)]));
        out.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{BufWriter, Write};

    fn hash_of(contents: &[u8]) -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.bin");
        std::fs::write(&path, contents).unwrap();
        sha256_file(&path).unwrap()
    }

    // FIPS 180-2 / NIST CSRC example vectors.
    #[test]
    fn known_vectors() {
        assert_eq!(
            hash_of(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hash_of(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hash_of(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            hash_of(&[b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn spans_buffer_boundaries() {
        // One byte past the buffer. Expected value from Python:
        // hashlib.sha256(b"a" * 2**20 + b"b").hexdigest()
        let mut contents = vec![b'a'; BUFFER_SIZE];
        contents.push(b'b');
        assert_eq!(
            hash_of(&contents),
            "371264331be3a89bb42c4fea3770469e9094f6ce8c8244b9ac2beb9ffd80e621"
        );
    }

    #[test]
    fn hashes_a_read_only_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("evidence.bin");
        std::fs::write(&path, b"abc").unwrap();
        crate::fsutil::set_read_only(&path).unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let never = AtomicBool::new(false);
        assert_eq!(
            sha256_file_with_progress(&path, &never, |_, _| {}).unwrap(),
            HashOutcome::Completed(
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_owned()
            )
        );
        crate::fsutil::test_support::make_writable(&path);
    }

    #[test]
    fn missing_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = sha256_file(&dir.path().join("nope")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        let never = AtomicBool::new(false);
        let err =
            sha256_file_with_progress(&dir.path().join("nope"), &never, |_, _| {}).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn hex_is_lowercase() {
        assert_eq!(to_hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
    }

    // ---- progress and cancel ----

    /// The deterministic 64 MiB test input: byte i is ((i * 2654435761) >> 13) & 0xff.
    ///
    /// Its SHA-256 was computed independently, without this code, by generating the same bytes in
    /// Python and hashing them with the system tools:
    ///
    /// ```sh
    /// python3 -c "
    /// import sys
    /// out = sys.stdout.buffer
    /// for start in range(0, 1 << 26, 1 << 20):
    ///     out.write(bytes(((i * 2654435761) >> 13) & 0xFF for i in range(start, start + (1 << 20))))
    /// " | shasum -a 256     # same result with sha256sum; wc -c gives 67108864
    /// ```
    const SIZE_64_MIB: u64 = 64 << 20;
    const SHA256_64_MIB: &str = "f85505310ac55800e8f0eaf99106e28513c68366d240f98a5c50e8c9208bba25";

    fn write_64_mib(path: &Path) {
        let mut out = BufWriter::new(File::create(path).unwrap());
        let mut chunk = vec![0u8; 1 << 20];
        for start in (0..SIZE_64_MIB).step_by(chunk.len()) {
            for (offset, byte) in chunk.iter_mut().enumerate() {
                let i = start + offset as u64;
                *byte = ((i * 2_654_435_761) >> 13) as u8;
            }
            out.write_all(&chunk).unwrap();
        }
        out.flush().unwrap();
    }

    #[test]
    fn hashes_64_mib_like_an_independent_tool() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.bin");
        write_64_mib(&path);
        assert_eq!(fs::metadata(&path).unwrap().len(), SIZE_64_MIB);

        let never = AtomicBool::new(false);
        let mut calls = Vec::new();
        let start = Instant::now();
        let outcome = sha256_file_with_progress(&path, &never, |done, total| {
            calls.push((Instant::now(), done, total));
        })
        .unwrap();
        let elapsed = start.elapsed();
        assert_eq!(outcome, HashOutcome::Completed(SHA256_64_MIB.to_owned()));
        assert_eq!(sha256_file(&path).unwrap(), SHA256_64_MIB);

        // The last callback reports completion; the rate stays within 10 per second.
        let &(_, done, total) = calls.last().unwrap();
        assert_eq!((done, total), (SIZE_64_MIB, SIZE_64_MIB));
        assert!(calls.iter().all(|&(_, _, total)| total == SIZE_64_MIB));
        assert!(calls.windows(2).all(|w| w[0].1 <= w[1].1), "monotonic");
        let allowed = elapsed.as_millis() / 100 + 1;
        assert!(
            calls.len() as u128 <= allowed,
            "{} calls in {elapsed:?}",
            calls.len()
        );
    }

    /// A reader that returns small chunks slowly, so throttling and cancel can be observed.
    struct SlowReader {
        left: usize,
    }

    impl Read for SlowReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            std::thread::sleep(Duration::from_millis(2));
            let n = buf.len().min(4096).min(self.left);
            buf[..n].fill(b'x');
            self.left -= n;
            Ok(n)
        }
    }

    #[test]
    fn progress_is_throttled_to_ten_per_second() {
        let never = AtomicBool::new(false);
        let start = Instant::now();
        let mut throttle = Throttle::new();
        let mut calls = Vec::new();
        // About 600 reads at 2+ ms each: well over a second.
        let mut reader = SlowReader { left: 600 * 4096 };
        let (_, done) = hash_stream(&mut reader, &never, &mut |done| {
            if throttle.ready() {
                calls.push((Instant::now(), done));
            }
        })
        .unwrap()
        .unwrap();
        let elapsed = start.elapsed();
        assert_eq!(done, 600 * 4096);
        assert!(elapsed >= Duration::from_secs(1), "{elapsed:?}");
        assert!(!calls.is_empty());
        for pair in calls.windows(2) {
            assert!(pair[1].0 - pair[0].0 >= PROGRESS_INTERVAL, "{calls:?}");
        }
        assert!(calls[0].0 - start >= PROGRESS_INTERVAL);
        assert!(calls.len() as u128 <= elapsed.as_millis() / 100);
    }

    #[test]
    fn cancel_stops_hashing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.bin");
        fs::write(&path, vec![7u8; 3 * BUFFER_SIZE]).unwrap();

        let cancel = AtomicBool::new(true);
        let mut calls = 0;
        let outcome = sha256_file_with_progress(&path, &cancel, |_, _| calls += 1).unwrap();
        assert_eq!(outcome, HashOutcome::Cancelled);
        assert_eq!(calls, 0, "no final callback when cancelled");

        // Cancelled midway: the read loop stops at the next chunk.
        let cancel = AtomicBool::new(false);
        let mut reads = 0;
        let mut reader = SlowReader { left: 1 << 30 };
        let outcome = hash_stream(&mut reader, &cancel, &mut |_| {
            reads += 1;
            if reads == 5 {
                cancel.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();
        assert_eq!(outcome, None);
        assert_eq!(reads, 5);
    }

    // ---- seal_tree ----

    struct Tree {
        _tmp: tempfile::TempDir,
        run: PathBuf,
    }

    impl Tree {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let run = tmp.path().join("run");
            fs::create_dir_all(run.join("report")).unwrap();
            Self { _tmp: tmp, run }
        }

        fn file(&self, rel: &str, contents: &[u8]) {
            let path = self.run.join("report").join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }

        fn seal(&self) -> io::Result<SealOutcome> {
            let never = AtomicBool::new(false);
            seal_tree(
                &self.run.join("report"),
                &self.run.join("report.sha256"),
                &never,
                |_, _| {},
            )
        }

        fn manifest(&self) -> Vec<u8> {
            fs::read(self.run.join("report.sha256")).unwrap()
        }
    }

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn manifest_lines_are_sorted_by_bytes() {
        let tree = Tree::new();
        tree.file("index.html", b"abc");
        tree.file("_HTML/x.css", b"");
        tree.file("a b/c", b"abc");
        tree.file("a/c", b"");
        tree.file("Z.txt", b"abc");
        tree.file("é.txt", b"");
        tree.file("empty/..keep", b"");
        fs::create_dir_all(tree.run.join("report/no files/deeper")).unwrap();

        let outcome = tree.seal().unwrap();
        let expected = format!(
            "{ABC}  report/Z.txt\n\
             {EMPTY}  report/_HTML/x.css\n\
             {ABC}  report/a b/c\n\
             {EMPTY}  report/a/c\n\
             {EMPTY}  report/empty/..keep\n\
             {ABC}  report/index.html\n\
             {EMPTY}  report/é.txt\n"
        );
        let manifest = tree.manifest();
        assert_eq!(String::from_utf8(manifest.clone()).unwrap(), expected);
        assert_eq!(
            outcome,
            SealOutcome {
                cancelled: false,
                manifest_sha256: Some(to_hex(&Sha256::digest(&manifest))),
                file_count: 7,
                total_bytes: 9,
                symlinks: 0,
                unencodable_names: 0,
            }
        );
        assert_eq!(
            outcome.manifest_sha256.as_deref(),
            Some(
                sha256_file(&tree.run.join("report.sha256"))
                    .unwrap()
                    .as_str()
            )
        );
        let seal = outcome.seal("report.sha256");
        assert_eq!(seal.status, SealStatus::Sealed);
        assert_eq!(seal.manifest.as_deref(), Some("report.sha256"));
        assert_eq!((seal.file_count, seal.total_bytes), (Some(7), Some(9)));
        assert_eq!(outcome.warnings("symlinks_in_report"), vec![]);
    }

    #[test]
    fn line_escaping_follows_coreutils() {
        assert_eq!(
            manifest_line(ABC, b"report/plain name.txt"),
            format!("{ABC}  report/plain name.txt\n").into_bytes()
        );
        assert_eq!(
            manifest_line(ABC, b"report/back\\slash"),
            format!("\\{ABC}  report/back\\\\slash\n").into_bytes()
        );
        assert_eq!(
            manifest_line(ABC, b"report/new\nline"),
            format!("\\{ABC}  report/new\\nline\n").into_bytes()
        );
        assert_eq!(
            manifest_line(ABC, b"report/cr\rhere\\\n"),
            format!("\\{ABC}  report/cr\\rhere\\\\\\n\n").into_bytes()
        );
        // Other bytes (including invalid UTF-8 on Unix) are written raw.
        assert_eq!(
            manifest_line(ABC, b"report/\xff\t"),
            [format!("{ABC}  report/").as_bytes(), b"\xff\t\n"].concat()
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_names_with_escapes_and_raw_bytes() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let tree = Tree::new();
        let report = tree.run.join("report");
        for name in [&b"back\\slash"[..], b"new\nline", b"cr\rname"] {
            fs::write(report.join(OsStr::from_bytes(name)), b"abc").unwrap();
        }
        let mut expected = [
            format!("\\{ABC}  report/back\\\\slash\n").into_bytes(),
            format!("\\{ABC}  report/cr\\rname\n").into_bytes(),
            format!("\\{ABC}  report/new\\nline\n").into_bytes(),
        ]
        .concat();
        // Names that are not UTF-8 are written as raw bytes. APFS (macOS) refuses to create them.
        match fs::write(report.join(OsStr::from_bytes(b"raw\xff")), b"abc") {
            Ok(()) => {
                expected.extend([format!("{ABC}  report/raw").as_bytes(), b"\xff\n"].concat())
            }
            Err(e) => eprintln!("SKIPPED the non-UTF-8 name: this filesystem refuses it ({e})"),
        }
        let outcome = tree.seal().unwrap();
        assert_eq!(outcome.unencodable_names, 0);
        assert_eq!(tree.manifest(), expected);
    }

    /// Creates a symlink, or returns false (with a message) where the OS does not allow it:
    /// Windows without Developer Mode or admin (ERROR_PRIVILEGE_NOT_HELD, 1314).
    fn try_symlink(target: &Path, link: &Path, dir: bool) -> bool {
        #[cfg(unix)]
        let result = {
            let _ = dir;
            std::os::unix::fs::symlink(target, link)
        };
        #[cfg(windows)]
        let result = if dir {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        };
        match result {
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
    fn symlinks_are_counted_not_followed_or_listed() {
        let tree = Tree::new();
        tree.file("real.txt", b"abc");
        let outside = tree.run.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), b"abc").unwrap();
        let report = tree.run.join("report");
        if !try_symlink(&outside, &report.join("linked dir"), true) {
            return;
        }
        assert!(try_symlink(
            &outside.join("secret.txt"),
            &report.join("linked.txt"),
            false
        ));
        assert!(try_symlink(
            &tree.run.join("missing"),
            &report.join("dangling"),
            false
        ));
        let outcome = tree.seal().unwrap();
        assert_eq!(
            tree.manifest(),
            format!("{ABC}  report/real.txt\n").into_bytes()
        );
        assert_eq!((outcome.file_count, outcome.symlinks), (1, 3));
        let warnings = outcome.warnings("symlinks_in_report");
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "symlinks_in_report");
        assert!(warnings[0].message.starts_with("3 symbolic link(s)"));
        assert_eq!(
            outcome.warnings("symlinks_in_backup")[0].code,
            "symlinks_in_backup"
        );
    }

    #[test]
    fn seal_warnings_and_cancelled_seal_record() {
        let outcome = SealOutcome {
            cancelled: false,
            manifest_sha256: Some(ABC.to_owned()),
            file_count: 2,
            total_bytes: 3,
            symlinks: 0,
            unencodable_names: 2,
        };
        let warnings = outcome.warnings("symlinks_in_report");
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "unencodable_filename");
        let cancelled = SealOutcome {
            cancelled: true,
            manifest_sha256: None,
            ..outcome
        };
        assert_eq!(
            cancelled.seal("report.sha256"),
            Seal {
                status: SealStatus::Cancelled,
                manifest: None,
                manifest_sha256: None,
                file_count: None,
                total_bytes: None,
            }
        );
    }

    #[test]
    fn cancel_writes_no_manifest() {
        let tree = Tree::new();
        for i in 0..5 {
            tree.file(&format!("f{i}"), b"abc");
        }
        let cancel = AtomicBool::new(true);
        let outcome = seal_tree(
            &tree.run.join("report"),
            &tree.run.join("report.sha256"),
            &cancel,
            |_, _| {},
        )
        .unwrap();
        assert!(outcome.cancelled);
        assert_eq!(outcome.manifest_sha256, None);
        assert!(!tree.run.join("report.sha256").exists());
        assert_eq!(outcome.seal("report.sha256").status, SealStatus::Cancelled);
        // Cancelled from the progress callback, after the walk.
        let cancel = AtomicBool::new(false);
        let mut big = vec![0u8; 3 * BUFFER_SIZE];
        big[0] = 1;
        tree.file("big", &big);
        let outcome = seal_tree(
            &tree.run.join("report"),
            &tree.run.join("report.sha256"),
            &cancel,
            |_, _| cancel.store(true, Ordering::Relaxed),
        )
        .unwrap();
        // Small trees finish before the first throttled callback; either way nothing is left.
        if outcome.cancelled {
            assert!(!tree.run.join("report.sha256").exists());
        }
    }

    #[test]
    fn progress_reports_files() {
        let tree = Tree::new();
        for i in 0..3 {
            tree.file(&format!("f{i}"), b"abc");
        }
        let never = AtomicBool::new(false);
        let mut calls = Vec::new();
        seal_tree(
            &tree.run.join("report"),
            &tree.run.join("report.sha256"),
            &never,
            |done, total| calls.push((done, total)),
        )
        .unwrap();
        assert_eq!(calls.last(), Some(&(3, Some(3))));
    }

    #[test]
    fn refuses_bad_layouts() {
        let tree = Tree::new();
        tree.file("a", b"abc");
        let never = AtomicBool::new(false);
        let seal = |dir: &Path, manifest: &Path| seal_tree(dir, manifest, &never, |_, _| {});
        let report = tree.run.join("report");
        // The folder must be inside the manifest's folder, and not be it.
        let err = seal(&report, &report.join("inside.sha256")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        let err = seal(&tree.run, &tree.run.join("report.sha256")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        let err = seal(&tree.run.join("missing"), &tree.run.join("m.sha256")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        fs::write(tree.run.join("plain"), b"").unwrap();
        let err = seal(&tree.run.join("plain"), &tree.run.join("m.sha256")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        // An existing manifest is never replaced.
        fs::write(tree.run.join("report.sha256"), b"old").unwrap();
        let err = tree.seal().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(tree.manifest(), b"old");
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_files_fail_the_seal() {
        use std::os::unix::fs::PermissionsExt;
        let tree = Tree::new();
        tree.file("ok", b"abc");
        tree.file("locked/secret", b"abc");
        let locked = tree.run.join("report/locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read_dir(&locked).is_ok() {
            // Root ignores permissions; Linux CI runs unprivileged, so this is exercised there.
            eprintln!("SKIPPED: running with privileges that bypass file permissions");
        } else {
            let err = tree.seal().unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
            assert!(!tree.run.join("report.sha256").exists());
        }
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// CONTRACTS.md §8: `sha256sum -c report.sha256` passes from the run folder, for names with
    /// spaces, `\` and a newline. GNU coreutils is only on Linux (the CI job runs this).
    #[test]
    fn gnu_sha256sum_verifies_the_manifest() {
        if !cfg!(target_os = "linux") {
            eprintln!("SKIPPED: GNU sha256sum -c is checked on Linux only (Linux CI)");
            return;
        }
        let tree = Tree::new();
        tree.file("index.html", b"<html></html>");
        tree.file("with space.txt", b"a b");
        tree.file("back\\slash.txt", b"\\");
        tree.file("new\nline.txt", b"\n");
        tree.file("sub dir/back\\and\nnewline", b"both");
        tree.file("_HTML/_Script_Logs/Screen_Output.html", b"log<br>\n");
        tree.seal().unwrap();
        let output = std::process::Command::new("sha256sum")
            .args(["-c", "report.sha256"])
            .current_dir(&tree.run)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "sha256sum -c failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(stdout.matches(": OK").count(), 6, "{stdout}");
        // And it notices a changed file.
        fs::write(tree.run.join("report/with space.txt"), b"changed").unwrap();
        let status = std::process::Command::new("sha256sum")
            .args(["-c", "--quiet", "report.sha256"])
            .current_dir(&tree.run)
            .status()
            .unwrap();
        assert!(!status.success());
    }
}
