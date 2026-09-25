//! Installing and verifying a pinned LEAPP build (ROADMAP A2; CONTRACTS.md §3, §4; ARCHITECTURE.md
//! §7 "Linux AppImage", §9 "Downloads").
//!
//! **Layout.** A tool lives in `<tools_dir>/<tool>/<version>/` with `install.json`, `modules.json`
//! and its entry: `bin/<entry>` for a zip build, `squashfs-root/<entry>` for an AppImage. The
//! caller passes the effective tools dir (the settings override or `<app_data>/leapp`).
//!
//! **Install pipeline** ([`install`]), all inside `<tools_dir>/<tool>/.staging-<rand>/`:
//! 1. Get the asset: download it from the manifest URLs in order (HTTPS only, redirects included;
//!    the body is capped at `asset_size`) or copy the offline-import file (opened read-only). The
//!    SHA-256 is computed on the way and checked before anything is extracted. A download that
//!    receives nothing for 60 s fails as stalled (`download_failed`).
//! 2. Extract: a zip yields **only** `entry` (a regular file with a plain relative name; absolute
//!    paths, `..` and symlinks are rejected), mode 0755 on Unix. An AppImage is made executable and
//!    run once with `--appimage-extract` (Linux only, without the environment variables that
//!    would make the runtime extract a different file), and `squashfs-root/<entry>` must exist.
//! 3. Hash the entry and check it against the manifest `entry_sha256` (`hash_mismatch`). Where the
//!    manifest has `null` (AppImages until ROADMAP E3), the hash is recorded in `install.json`.
//! 4. Introspect through the caller's callback (ROADMAP A3), then write `modules.json` and
//!    `install.json`.
//! 5. Rename the staging dir to `<version>` (replacing an earlier install of that version, which is
//!    moved to `.old-<rand>` first and removed afterwards).
//!
//! Any failure removes the staging dir, and the tool dirs if the install created them, so a failed
//! install leaves nothing behind. The cancel flag is checked while downloading, copying and
//! extracting and between the steps; a cancelled install also leaves nothing behind.
//!
//! **Leftovers.** An install interrupted by a crash or a quit can leave a `.staging-<rand>` dir, and a
//! failed removal an `.old-<rand>` dir. Each install or import first removes those leftovers from
//! `<tools_dir>/<tool>/`; it never touches a version dir. Installs of the same tool must not run
//! concurrently (the command layer runs one at a time).
//!
//! **Checks.** [`status`] is cheap (no hashing): `installed_unverified` when the tool is installed
//! per CONTRACTS.md §4. [`verify`] re-hashes the entry: `verified` or `verification_failed`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::contracts::{
    AppError, ArchiveKind, EntryVerifiedAgainst, ErrorCode, InstallEvent, InstallRecord,
    InstallSource, InstallStage, ModulesFile, PlatformAsset, PlatformKey, Timestamp, ToolId,
    ToolManifest, ToolState, ToolStatus, VersionedFile, parse_versioned,
};
use crate::fsutil::{retry_transient, write_json_atomic};
use crate::hashing::{sha256_file, to_hex};
use crate::manifest::{asset_for, relative_components};

/// `install.json`, next to `modules.json` in the version dir (CONTRACTS.md §4).
pub const INSTALL_FILE: &str = "install.json";
/// `modules.json` (CONTRACTS.md §5), written from the introspection result.
pub const MODULES_FILE: &str = "modules.json";

const READ_BUFFER: usize = 64 * 1024;
/// Download progress is reported at most once per this many bytes (and at the end).
const PROGRESS_STEP: u64 = 1 << 20;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
/// A download that receives no body bytes for this long fails as stalled. Unit tests use a short
/// value so the stall test is quick.
const STALL_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_secs(2)
} else {
    Duration::from_secs(60)
};
/// How often a waiting download checks the cancel flag and the stall clock.
const DOWNLOAD_POLL: Duration = Duration::from_millis(100);
/// The body budget given to ureq, so that a reader left blocked on a stalled connection (after
/// the download already failed as stalled or cancelled) ends eventually: at least 10 minutes, or
/// the asset at 16 KiB/s. Slower links should use offline import.
const MIN_BODY_BUDGET: Duration = Duration::from_secs(600);
const MIN_BODY_RATE: u64 = 16 * 1024;
/// The names of an install's staging dir and of a moved-aside earlier install: a prefix and 6
/// lowercase hex digits.
const STAGING_PREFIX: &str = ".staging-";
const OLD_PREFIX: &str = ".old-";
/// How long a rename or removal in the tools dir is retried on Windows while a file is still held
/// open: antivirus scanners can hold a just-introspected executable for a few seconds.
const TRANSIENT_RETRY: Duration = Duration::from_secs(5);

/// Why an install or a check failed. Each variant maps to one CONTRACTS.md §12 code.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("{tool} has no pinned build for this platform ({platform})")]
    UnsupportedPlatform { tool: ToolId, platform: String },
    #[error("the download failed: {0}")]
    Download(String),
    #[error("{what}: SHA-256 {actual} does not match the pinned {expected}")]
    HashMismatch {
        what: String,
        expected: String,
        actual: String,
    },
    #[error("extracting the tool failed: {0}")]
    Extract(String),
    #[error("module introspection failed: {message}")]
    Introspection {
        message: String,
        detail: Option<String>,
    },
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
    /// The cancel flag was set. CONTRACTS.md §12 has no cancel code; this reports as
    /// `download_failed` with a message saying the install was cancelled.
    #[error("the installation was cancelled")]
    Cancelled,
}

impl InstallError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::UnsupportedPlatform { .. } => ErrorCode::UnsupportedPlatform,
            Self::Download(_) | Self::Cancelled => ErrorCode::DownloadFailed,
            Self::HashMismatch { .. } => ErrorCode::HashMismatch,
            Self::Extract(_) => ErrorCode::ExtractFailed,
            Self::Introspection { .. } => ErrorCode::IntrospectionFailed,
            Self::Io { source, .. } if source.kind() == io::ErrorKind::PermissionDenied => {
                ErrorCode::PermissionDenied
            }
            Self::Io { .. } => ErrorCode::Io,
        }
    }

    fn io(context: impl Into<String>) -> impl FnOnce(io::Error) -> Self {
        let context = context.into();
        move |source| Self::Io { context, source }
    }
}

impl From<InstallError> for AppError {
    fn from(error: InstallError) -> Self {
        let code = error.code();
        let detail = match &error {
            InstallError::Introspection { detail, .. } => detail.clone(),
            _ => None,
        };
        AppError {
            code,
            message: error.to_string(),
            detail,
        }
    }
}

/// A tool as pinned for this host: where it is installed and what it must match.
#[derive(Clone, Copy, Debug)]
pub struct Pinned<'a> {
    /// The effective tools dir (the settings override or `<app_data>/leapp`).
    pub tools_dir: &'a Path,
    pub tool: ToolId,
    pub manifest: &'a ToolManifest,
    /// `None` when the host has no [`PlatformKey`].
    pub platform: Option<PlatformKey>,
}

impl Pinned<'_> {
    /// `<tools_dir>/<tool>`, absolute: a relative tools dir is resolved against the current dir,
    /// so the entry path handed to introspection and runs never depends on a child's cwd.
    pub fn tool_dir(&self) -> PathBuf {
        let tools_dir =
            std::path::absolute(self.tools_dir).unwrap_or_else(|_| self.tools_dir.to_path_buf());
        tools_dir.join(self.tool.as_str())
    }

    /// `<tools_dir>/<tool>/<version>`.
    pub fn version_dir(&self) -> PathBuf {
        self.tool_dir().join(&self.manifest.version)
    }

    fn asset(&self) -> Result<(PlatformKey, &PlatformAsset), InstallError> {
        match (self.platform, asset_for(self.manifest, self.platform)) {
            (Some(platform), Some(asset)) => Ok((platform, asset)),
            (platform, _) => Err(InstallError::UnsupportedPlatform {
                tool: self.tool,
                platform: platform.map_or_else(|| "unknown".to_owned(), |p| p.to_string()),
            }),
        }
    }
}

/// Where the asset comes from.
#[derive(Clone, Copy, Debug)]
pub enum Source<'a> {
    /// The manifest URLs, tried in order.
    Download,
    /// A local copy of the release asset (offline import), opened read-only.
    File(&'a Path),
}

/// Installs the pinned build (see the module docs). `introspect` receives the absolute path of the
/// staged entry, after its hash was checked, and returns the tool's module list. Setting `cancel`
/// stops the install (`InstallError::Cancelled`). Events: `stage` in pipeline order,
/// `download_progress` while downloading, a `message` for each URL that failed, and one for
/// removed leftovers of an interrupted install.
pub fn install(
    pinned: Pinned<'_>,
    source: Source<'_>,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(InstallEvent),
    introspect: &mut dyn FnMut(&Path) -> Result<ModulesFile, InstallError>,
) -> Result<InstallRecord, InstallError> {
    let (platform, asset) = pinned.asset()?;
    let tool_dir = pinned.tool_dir();
    let tools_dir = tool_dir.parent().unwrap_or(&tool_dir).to_path_buf();
    let created: Vec<PathBuf> = [tools_dir, tool_dir.clone()]
        .into_iter()
        .filter(|dir| !dir.exists())
        .collect();
    fs::create_dir_all(&tool_dir)
        .map_err(InstallError::io(format!("creating {}", tool_dir.display())))?;
    let removed = remove_leftovers(&tool_dir, &pinned.manifest.version);
    if removed > 0 {
        on_event(InstallEvent::Message {
            text: format!("removed {removed} leftover folder(s) of an interrupted install"),
        });
    }
    let staging = tool_dir.join(format!("{STAGING_PREFIX}{}", random_hex()?));
    let result = fs::create_dir(&staging)
        .map_err(InstallError::io(format!("creating {}", staging.display())))
        .and_then(|()| {
            let job = Staging {
                pinned,
                platform,
                asset,
                dir: &staging,
                cancel,
            };
            let record = job.run(source, on_event, introspect)?;
            job.check_cancel()?;
            commit(&staging, &pinned.version_dir())?;
            Ok(record)
        });
    match result {
        Ok(record) => {
            on_event(stage_event(InstallStage::Done));
            Ok(record)
        }
        Err(error) => {
            if let Err(e) = remove_tree(&staging)
                && e.kind() != io::ErrorKind::NotFound
            {
                log::warn!("cannot remove {}: {e}", staging.display());
            }
            // Only if empty: the tool dirs may hold other versions.
            for dir in created.iter().rev() {
                let _ = fs::remove_dir(dir);
            }
            Err(error)
        }
    }
}

fn stage_event(stage: InstallStage) -> InstallEvent {
    InstallEvent::Stage { stage }
}

/// Removes `.staging-<hex>` and `.old-<hex>` dirs left in `tool_dir` by an interrupted install or
/// a failed removal; never the live `version` dir or anything else. Failures are logged. Returns
/// how many were removed.
fn remove_leftovers(tool_dir: &Path, version: &str) -> usize {
    let is_leftover = |name: &str| {
        [STAGING_PREFIX, OLD_PREFIX].iter().any(|prefix| {
            name.strip_prefix(prefix).is_some_and(|suffix| {
                suffix.len() == 6
                    && suffix
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            })
        }) && name != version
    };
    let entries = match fs::read_dir(tool_dir) {
        Ok(entries) => entries,
        Err(e) => {
            log::warn!("cannot list {}: {e}", tool_dir.display());
            return 0;
        }
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        if !entry.file_name().to_str().is_some_and(is_leftover) {
            continue;
        }
        let path = entry.path();
        match remove_tree(&path) {
            Ok(()) => removed += 1,
            Err(e) => log::warn!("cannot remove the leftover {}: {e}", path.display()),
        }
    }
    removed
}

/// One install's staging dir and what it must produce there.
struct Staging<'a> {
    pinned: Pinned<'a>,
    platform: PlatformKey,
    asset: &'a PlatformAsset,
    dir: &'a Path,
    cancel: &'a AtomicBool,
}

impl Staging<'_> {
    fn check_cancel(&self) -> Result<(), InstallError> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(InstallError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// Steps 1–4 of the pipeline, inside the staging dir.
    fn run(
        &self,
        source: Source<'_>,
        on_event: &mut dyn FnMut(InstallEvent),
        introspect: &mut dyn FnMut(&Path) -> Result<ModulesFile, InstallError>,
    ) -> Result<InstallRecord, InstallError> {
        let (pinned, asset, staging) = (self.pinned, self.asset, self.dir);
        self.check_cancel()?;
        let asset_path = staging.join(&asset.asset_name);
        let (install_source, source_detail) = match source {
            Source::Download => (
                InstallSource::Download,
                download(asset, &asset_path, self.cancel, on_event)?,
            ),
            Source::File(path) => {
                on_event(stage_event(InstallStage::Verifying));
                copy_verified(path, asset, &asset_path, self.cancel)?;
                (
                    InstallSource::OfflineImport,
                    path.to_string_lossy().into_owned(),
                )
            }
        };

        self.check_cancel()?;
        on_event(stage_event(InstallStage::Extracting));
        let entry_path = extract(asset, &asset_path, staging, self.cancel)?;
        retry_transient(TRANSIENT_RETRY, || fs::remove_file(&asset_path)).map_err(
            InstallError::io(format!("removing {}", asset_path.display())),
        )?;

        self.check_cancel()?;
        on_event(stage_event(InstallStage::Hashing));
        // Native separators: this path is what introspection runs.
        let entry = join_parts(staging, &entry_path.split('/').collect::<Vec<_>>());
        let entry_sha256 = sha256_file(&entry)
            .map_err(InstallError::io(format!("hashing {}", entry.display())))?;
        if let Some(expected) = &asset.entry_sha256
            && *expected != entry_sha256
        {
            return Err(InstallError::HashMismatch {
                what: format!("{} (extracted from {})", asset.entry, asset.asset_name),
                expected: expected.clone(),
                actual: entry_sha256,
            });
        }

        self.check_cancel()?;
        on_event(stage_event(InstallStage::Introspecting));
        let modules = introspect(&entry)?;
        if modules.tool != pinned.tool || modules.version != pinned.manifest.version {
            return Err(InstallError::Introspection {
                message: format!(
                    "introspection returned modules of {} {}, expected {} {}",
                    modules.tool, modules.version, pinned.tool, pinned.manifest.version
                ),
                detail: None,
            });
        }
        let module_count =
            u32::try_from(modules.modules.len()).map_err(|_| InstallError::Introspection {
                message: "introspection returned too many modules".to_owned(),
                detail: None,
            })?;
        let record = InstallRecord {
            schema_version: InstallRecord::SCHEMA_VERSION,
            tool: pinned.tool,
            version: pinned.manifest.version.clone(),
            platform: self.platform,
            asset_name: asset.asset_name.clone(),
            asset_sha256: asset.asset_sha256.clone(),
            entry_path,
            entry_sha256,
            source: install_source,
            source_detail,
            installed_at: Timestamp::now(),
            module_count,
        };
        for (name, result) in [
            (
                MODULES_FILE,
                write_json_atomic(&staging.join(MODULES_FILE), &modules),
            ),
            (
                INSTALL_FILE,
                write_json_atomic(&staging.join(INSTALL_FILE), &record),
            ),
        ] {
            result.map_err(InstallError::io(format!("writing {name}")))?;
        }
        Ok(record)
    }
}

// ---- getting the asset ----

/// Whether the downloader may fetch `url`: HTTPS only. Unit tests may also use a local plain-HTTP
/// server on 127.0.0.1.
fn url_allowed(url: &str) -> bool {
    url.starts_with("https://") || (cfg!(test) && url.starts_with("http://127.0.0.1:"))
}

fn agent() -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        // Also refuses redirects to plain HTTP. Tests serve plain HTTP from 127.0.0.1.
        .https_only(!cfg!(test))
        .user_agent(concat!("suiteDFIR/", env!("CARGO_PKG_VERSION")))
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(RESPONSE_TIMEOUT));
    #[cfg(test)]
    let config = config.proxy(None);
    config.build().into()
}

/// Downloads the asset to `dest` from the first URL that yields exactly the pinned bytes. Returns
/// that URL. The `downloading` stage is reported again when a URL whose body was already being
/// verified is followed by the next one.
fn download(
    asset: &PlatformAsset,
    dest: &Path,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(InstallEvent),
) -> Result<String, InstallError> {
    let agent = agent();
    let mut last: Option<InstallError> = None;
    for url in &asset.urls {
        // Only a hash mismatch fails after the `verifying` stage was reported.
        if last.is_none() || matches!(last, Some(InstallError::HashMismatch { .. })) {
            on_event(stage_event(InstallStage::Downloading));
        }
        let result = if url_allowed(url) {
            download_from(&agent, url, asset, dest, cancel, on_event)
        } else {
            Err(InstallError::Download(format!(
                "{url} is not an https:// URL"
            )))
        };
        match result {
            Ok(()) => return Ok(url.clone()),
            Err(InstallError::Cancelled) => return Err(InstallError::Cancelled),
            Err(error) => {
                on_event(InstallEvent::Message {
                    text: format!("{url}: {error}"),
                });
                last = Some(error);
            }
        }
    }
    Err(last.unwrap_or_else(|| InstallError::Download("the manifest lists no URL".to_owned())))
}

/// What the body-reading thread hands over.
enum Chunk {
    Data(Vec<u8>),
    End,
    Failed(io::Error),
}

fn download_from(
    agent: &ureq::Agent,
    url: &str,
    asset: &PlatformAsset,
    dest: &Path,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(InstallEvent),
) -> Result<(), InstallError> {
    let size = asset.asset_size;
    let budget = MIN_BODY_BUDGET.max(Duration::from_secs(size / MIN_BODY_RATE));
    let response = agent
        .get(url)
        .config()
        .timeout_recv_body(Some(budget))
        .build()
        .call()
        .map_err(|e| InstallError::Download(format!("GET {url}: {e}")))?;
    if let Some(announced) = response.body().content_length()
        && announced > size
    {
        return Err(InstallError::Download(format!(
            "the server announces {announced} bytes; the pinned asset has {size}"
        )));
    }
    // ureq's limit fails the read after `limit` bytes, even at the end of the body, so allow one
    // byte more and let the count below reject anything beyond `size`.
    let mut body = response
        .into_body()
        .into_with_config()
        .limit(size + 1)
        .reader();
    let mut file =
        File::create(dest).map_err(InstallError::io(format!("creating {}", dest.display())))?;

    // The body is read on its own thread, so a cancel or a stalled connection is noticed while a
    // read blocks. When this function returns early, the thread ends at its next read (at the
    // latest when ureq's body budget runs out).
    let (sender, chunks) = mpsc::sync_channel::<Chunk>(4);
    thread::Builder::new()
        .name("leapp-download".to_owned())
        .spawn(move || {
            let mut buffer = vec![0u8; READ_BUFFER];
            loop {
                let chunk = match body.read(&mut buffer) {
                    Ok(0) => Chunk::End,
                    Ok(read) => Chunk::Data(buffer[..read].to_vec()),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => Chunk::Failed(e),
                };
                let more = matches!(chunk, Chunk::Data(_));
                if sender.send(chunk).is_err() || !more {
                    break;
                }
            }
        })
        .map_err(InstallError::io("starting the download thread"))?;

    let mut hasher = Sha256::new();
    let mut done = 0u64;
    let mut reported = 0u64;
    let mut last_data = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(InstallError::Cancelled);
        }
        let bytes = match chunks.recv_timeout(DOWNLOAD_POLL) {
            Ok(Chunk::Data(bytes)) => bytes,
            Ok(Chunk::End) => break,
            Ok(Chunk::Failed(e)) => {
                return Err(InstallError::Download(format!(
                    "reading {url} after {done} bytes: {e}"
                )));
            }
            Err(RecvTimeoutError::Timeout) if last_data.elapsed() >= STALL_TIMEOUT => {
                return Err(InstallError::Download(format!(
                    "{url} sent nothing for {} s after {done} bytes (stalled download)",
                    STALL_TIMEOUT.as_secs()
                )));
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(InstallError::Download(format!(
                    "reading {url} stopped unexpectedly after {done} bytes"
                )));
            }
        };
        last_data = Instant::now();
        done += bytes.len() as u64;
        if done > size {
            return Err(InstallError::Download(format!(
                "{url} sent more than the pinned {size} bytes"
            )));
        }
        hasher.update(&bytes);
        file.write_all(&bytes)
            .map_err(InstallError::io(format!("writing {}", dest.display())))?;
        if done - reported >= PROGRESS_STEP || done == size {
            reported = done;
            on_event(InstallEvent::DownloadProgress {
                bytes_done: done,
                bytes_total: size,
            });
        }
    }
    if done < size {
        return Err(InstallError::Download(format!(
            "{url} ended after {done} of {size} bytes (truncated download)"
        )));
    }
    file.flush()
        .map_err(InstallError::io(format!("writing {}", dest.display())))?;
    on_event(stage_event(InstallStage::Verifying));
    check_sha256(asset, &hasher.finalize())
}

/// Offline import: copies the file to `dest` (reading it through a read-only handle), capped at
/// the pinned size, and checks its SHA-256.
fn copy_verified(
    src: &Path,
    asset: &PlatformAsset,
    dest: &Path,
    cancel: &AtomicBool,
) -> Result<(), InstallError> {
    let mut file =
        File::open(src).map_err(InstallError::io(format!("opening {}", src.display())))?;
    let metadata = file
        .metadata()
        .map_err(InstallError::io(format!("reading {}", src.display())))?;
    if !metadata.is_file() {
        return Err(InstallError::Io {
            context: format!("importing {}", src.display()),
            source: io::Error::new(io::ErrorKind::InvalidInput, "not a regular file"),
        });
    }
    let mismatch = |actual: String| InstallError::HashMismatch {
        what: format!("{} (imported as {})", src.display(), asset.asset_name),
        expected: asset.asset_sha256.clone(),
        actual,
    };
    if metadata.len() != asset.asset_size {
        return Err(mismatch(format!(
            "unknown ({} bytes; the pinned asset has {})",
            metadata.len(),
            asset.asset_size
        )));
    }
    let mut out =
        File::create(dest).map_err(InstallError::io(format!("creating {}", dest.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; READ_BUFFER];
    let mut copied = 0u64;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(InstallError::Cancelled);
        }
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(InstallError::io(format!("reading {}", src.display()))(e)),
        };
        copied += read as u64;
        if copied > asset.asset_size {
            return Err(mismatch(
                "unknown (the file grew while it was read)".to_owned(),
            ));
        }
        hasher.update(&buffer[..read]);
        out.write_all(&buffer[..read])
            .map_err(InstallError::io(format!("writing {}", dest.display())))?;
    }
    out.flush()
        .map_err(InstallError::io(format!("writing {}", dest.display())))?;
    check_sha256(asset, &hasher.finalize())
}

fn check_sha256(asset: &PlatformAsset, digest: &[u8]) -> Result<(), InstallError> {
    let actual = to_hex(digest);
    if actual == asset.asset_sha256 {
        Ok(())
    } else {
        Err(InstallError::HashMismatch {
            what: asset.asset_name.clone(),
            expected: asset.asset_sha256.clone(),
            actual,
        })
    }
}

// ---- extraction ----

/// Extracts the entry into `staging`; returns its path relative to the version dir (`/`-separated).
fn extract(
    asset: &PlatformAsset,
    asset_path: &Path,
    staging: &Path,
    cancel: &AtomicBool,
) -> Result<String, InstallError> {
    let parts = relative_components(&asset.entry).ok_or_else(|| {
        InstallError::Extract(format!(
            "entry {:?} is not a plain relative path (absolute paths and .. are rejected)",
            asset.entry
        ))
    })?;
    match asset.archive_kind {
        ArchiveKind::Zip => extract_zip_entry(asset_path, &asset.entry, &parts, staging, cancel),
        ArchiveKind::Appimage => extract_appimage(asset_path, &parts, staging),
    }
}

fn join_parts(base: &Path, parts: &[&str]) -> PathBuf {
    parts
        .iter()
        .fold(base.to_path_buf(), |path, part| path.join(part))
}

/// Extracts only `entry` from the zip to `bin/<entry>`.
fn extract_zip_entry(
    zip_path: &Path,
    entry: &str,
    parts: &[&str],
    staging: &Path,
    cancel: &AtomicBool,
) -> Result<String, InstallError> {
    let fail = |what: String| InstallError::Extract(format!("{}: {what}", zip_path.display()));
    let file = File::open(zip_path).map_err(|e| fail(e.to_string()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| fail(e.to_string()))?;
    let mut file = archive
        .by_name(entry)
        .map_err(|_| fail(format!("the archive has no entry {entry:?}")))?;
    // Defense in depth: `entry` is already a plain relative name.
    if file.enclosed_name().is_none() {
        return Err(fail(format!("{entry:?} escapes the extraction dir")));
    }
    if !file.is_file() {
        return Err(fail(format!(
            "{entry:?} is not a regular file (directories and symlinks are rejected)"
        )));
    }
    let bin = staging.join("bin");
    let out_path = join_parts(&bin, parts);
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent).map_err(|e| fail(e.to_string()))?;
    }
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&out_path)
        .map_err(|e| fail(format!("creating {}: {e}", out_path.display())))?;
    let mut buffer = vec![0u8; READ_BUFFER];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(InstallError::Cancelled);
        }
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(fail(format!("extracting {entry:?}: {e}"))),
        };
        out.write_all(&buffer[..read])
            .map_err(|e| fail(format!("writing {}: {e}", out_path.display())))?;
    }
    out.flush().map_err(|e| fail(e.to_string()))?;
    drop(out);
    make_executable(&out_path).map_err(|e| fail(format!("chmod {}: {e}", out_path.display())))?;
    Ok(format!("bin/{}", parts.join("/")))
}

#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Runs `<asset> --appimage-extract` in `staging` (no FUSE needed) and checks that
/// `squashfs-root/<entry>` is a regular file inside `squashfs-root` (following a symlink only if
/// it stays inside).
#[cfg(target_os = "linux")]
fn extract_appimage(
    asset_path: &Path,
    parts: &[&str],
    staging: &Path,
) -> Result<String, InstallError> {
    use std::process::Stdio;

    let fail = |what: String| InstallError::Extract(format!("{}: {what}", asset_path.display()));
    make_executable(asset_path).map_err(|e| fail(format!("chmod: {e}")))?;
    let mut attempts = 0;
    let output = loop {
        let result = appimage_extract_command(asset_path, staging)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output();
        match result {
            // A file written a moment ago can be busy while another thread's fork holds an
            // inherited write handle until its exec (ETXTBSY); retry briefly.
            Err(e) if e.kind() == io::ErrorKind::ExecutableFileBusy && attempts < 50 => {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            other => break other.map_err(|e| fail(format!("running --appimage-extract: {e}")))?,
        }
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(10).collect();
        return Err(fail(format!(
            "--appimage-extract failed ({}): {}",
            output.status,
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        )));
    }
    let root = staging.join("squashfs-root");
    let entry = join_parts(&root, parts);
    let inside = fs::canonicalize(&root)
        .and_then(|root| Ok((root, fs::canonicalize(&entry)?)))
        .map(|(root, target)| target.starts_with(root) && target.is_file());
    match inside {
        Ok(true) => Ok(format!("squashfs-root/{}", parts.join("/"))),
        Ok(false) => Err(fail(format!(
            "squashfs-root/{} is not a regular file inside squashfs-root",
            parts.join("/")
        ))),
        Err(e) => Err(fail(format!(
            "squashfs-root/{} is missing after extraction: {e}",
            parts.join("/")
        ))),
    }
}

/// Variables that make an AppImage runtime work on a different file or tree than the one it
/// runs from: `TARGET_APPIMAGE` makes `--appimage-extract` extract that file instead (type-2
/// runtime), and `APPIMAGE`/`APPDIR` describe a running AppImage. The extraction must only ever
/// see the verified asset, so they are removed.
#[cfg(target_os = "linux")]
const APPIMAGE_VARS: [&str; 3] = ["TARGET_APPIMAGE", "APPIMAGE", "APPDIR"];

/// `<asset> --appimage-extract`, run in `staging`, without [`APPIMAGE_VARS`].
#[cfg(target_os = "linux")]
fn appimage_extract_command(asset_path: &Path, staging: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(asset_path);
    command.arg("--appimage-extract").current_dir(staging);
    for name in APPIMAGE_VARS {
        command.env_remove(name);
    }
    command
}

#[cfg(not(target_os = "linux"))]
fn extract_appimage(
    asset_path: &Path,
    _parts: &[&str],
    _staging: &Path,
) -> Result<String, InstallError> {
    Err(InstallError::Extract(format!(
        "{}: AppImage builds can only be installed on Linux",
        asset_path.display()
    )))
}

// ---- committing ----

/// Renames the staging dir to the version dir. An existing version dir (an earlier install of the
/// same version, e.g. one that failed verification) is moved aside first and removed afterwards;
/// if the final rename fails, it is moved back.
fn commit(staging: &Path, version_dir: &Path) -> Result<(), InstallError> {
    let context = || format!("installing into {}", version_dir.display());
    match fs::symlink_metadata(version_dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            rename_with_retry(staging, version_dir).map_err(InstallError::io(context()))
        }
        Err(e) => Err(InstallError::io(context())(e)),
        Ok(_) => {
            let old = version_dir.with_file_name(format!("{OLD_PREFIX}{}", random_hex()?));
            rename_with_retry(version_dir, &old).map_err(InstallError::io(format!(
                "moving the earlier install {} aside",
                version_dir.display()
            )))?;
            if let Err(e) = rename_with_retry(staging, version_dir) {
                if let Err(restore) = rename_with_retry(&old, version_dir) {
                    log::error!(
                        "cannot restore the earlier install from {}: {restore}",
                        old.display()
                    );
                }
                return Err(InstallError::io(context())(e));
            }
            if let Err(e) = remove_tree(&old) {
                // The next install of this tool removes it (see the module docs).
                log::warn!("cannot remove the earlier install {}: {e}", old.display());
            }
            Ok(())
        }
    }
}

/// A rename, retried on Windows for up to [`TRANSIENT_RETRY`] while a just-run or just-scanned
/// file is still open.
fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    retry_transient(TRANSIENT_RETRY, || fs::rename(from, to))
}

/// Removes a directory tree without following symlinks, retried like [`rename_with_retry`].
fn remove_tree(path: &Path) -> io::Result<()> {
    retry_transient(TRANSIENT_RETRY, || fs::remove_dir_all(path))
}

fn random_hex() -> Result<String, InstallError> {
    let mut bytes = [0u8; 3];
    getrandom::fill(&mut bytes).map_err(|e| InstallError::Io {
        context: "getting random bytes".to_owned(),
        source: io::Error::other(e.to_string()),
    })?;
    Ok(to_hex(&bytes))
}

// ---- checks ----

/// A tool that passed [`verify`]: ready to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedTool {
    pub record: InstallRecord,
    /// The absolute path of the entry.
    pub entry: PathBuf,
    /// What the entry hash was checked against.
    pub verified_against: EntryVerifiedAgainst,
}

/// The result of [`status`] or [`verify`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCheck {
    pub status: ToolStatus,
    /// Set only by [`verify`], when the entry hash matched.
    pub verified: Option<VerifiedTool>,
}

/// The install state without hashing: `unsupported_platform`, `not_installed` or
/// `installed_unverified` (CONTRACTS.md §4 "Installed").
pub fn status(pinned: Pinned<'_>) -> ToolCheck {
    check(pinned, false)
}

/// The install state after re-hashing the entry: `verified` or `verification_failed` for an
/// installed tool. The expected hash is the manifest's `entry_sha256`, or `install.json`'s where
/// the manifest has `null`.
pub fn verify(pinned: Pinned<'_>) -> ToolCheck {
    check(pinned, true)
}

fn check(pinned: Pinned<'_>, hash: bool) -> ToolCheck {
    let mut status = ToolStatus {
        tool: pinned.tool,
        display_name: pinned.manifest.display_name.clone(),
        pinned_version: pinned.manifest.version.clone(),
        state: ToolState::NotInstalled,
        installed_version: None,
        install_source: None,
        module_count: None,
        install_dir: None,
        problem: None,
    };
    let asset = match pinned.asset() {
        Ok((_, asset)) => asset,
        Err(e) => {
            status.state = ToolState::UnsupportedPlatform;
            status.problem = Some(e.to_string());
            return ToolCheck {
                status,
                verified: None,
            };
        }
    };
    let (record, entry) = match installed(pinned, asset) {
        Ok(Some(found)) => found,
        Ok(None) => {
            return ToolCheck {
                status,
                verified: None,
            };
        }
        Err(problem) => {
            status.problem = Some(problem);
            return ToolCheck {
                status,
                verified: None,
            };
        }
    };
    status.installed_version = Some(record.version.clone());
    status.install_source = Some(record.source);
    status.module_count = Some(record.module_count);
    status.install_dir = Some(pinned.version_dir().to_string_lossy().into_owned());
    if !hash {
        status.state = ToolState::InstalledUnverified;
        return ToolCheck {
            status,
            verified: None,
        };
    }
    let (expected, against) = match &asset.entry_sha256 {
        Some(hash) => (hash.as_str(), EntryVerifiedAgainst::Manifest),
        None => (
            record.entry_sha256.as_str(),
            EntryVerifiedAgainst::InstallRecord,
        ),
    };
    let basis = match against {
        EntryVerifiedAgainst::Manifest => "the manifest",
        _ => "install.json",
    };
    match sha256_file(&entry) {
        Ok(actual) if actual == expected => {
            status.state = ToolState::Verified;
            ToolCheck {
                status,
                verified: Some(VerifiedTool {
                    record,
                    entry,
                    verified_against: against,
                }),
            }
        }
        Ok(actual) => {
            status.state = ToolState::VerificationFailed;
            status.problem = Some(format!(
                "{} has SHA-256 {actual}; {basis} pins {expected}",
                entry.display()
            ));
            ToolCheck {
                status,
                verified: None,
            }
        }
        Err(e) => {
            status.state = ToolState::VerificationFailed;
            status.problem = Some(format!("cannot hash {}: {e}", entry.display()));
            ToolCheck {
                status,
                verified: None,
            }
        }
    }
}

/// The install record and absolute entry path if the pinned build is installed (CONTRACTS.md §4:
/// `install.json` exists and matches the pinned asset, the entry exists and `modules.json`
/// exists). `Ok(None)` when nothing is installed; `Err` explains a partial or foreign install.
fn installed(
    pinned: Pinned<'_>,
    asset: &PlatformAsset,
) -> Result<Option<(InstallRecord, PathBuf)>, String> {
    let dir = pinned.version_dir();
    let bytes = match fs::read(dir.join(INSTALL_FILE)) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {INSTALL_FILE}: {e}")),
    };
    let record: InstallRecord =
        parse_versioned(&bytes).map_err(|e| format!("{INSTALL_FILE} is unusable: {e}"))?;
    let matches = record.tool == pinned.tool
        && record.version == pinned.manifest.version
        && Some(record.platform) == pinned.platform
        && record.asset_sha256 == asset.asset_sha256;
    if !matches {
        return Err(format!(
            "the installed build ({} {} for {}, asset {}) is not the pinned one",
            record.tool, record.version, record.platform, record.asset_name
        ));
    }
    let parts = relative_components(&record.entry_path).ok_or_else(|| {
        format!(
            "{INSTALL_FILE} names an invalid entry path {:?}",
            record.entry_path
        )
    })?;
    let entry = join_parts(&dir, &parts);
    if !entry.is_file() {
        return Err(format!(
            "the tool's executable {} is missing",
            entry.display()
        ));
    }
    if !dir.join(MODULES_FILE).is_file() {
        return Err(format!(
            "{MODULES_FILE} is missing (the module list was never recorded)"
        ));
    }
    Ok(Some((record, entry)))
}

#[cfg(test)]
mod tests {
    use std::io::BufRead;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::thread::JoinHandle;

    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::contracts::{ModuleInfo, examples};

    const ENTRY_BYTES: &[u8] = b"#!fake ileapp onefile binary\n";
    static NO_CANCEL: AtomicBool = AtomicBool::new(false);
    /// Slack in timing bounds for a loaded CI runner.
    const SLOW_RUNNER: Duration = Duration::from_secs(3);
    const VERSION: &str = "v2026.4.2";

    fn sha256_of(bytes: &[u8]) -> String {
        to_hex(&Sha256::digest(bytes))
    }

    /// A zip holding `entries` (name, bytes); a name ending in `@` is stored as a symlink.
    fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            if let Some(name) = name.strip_suffix('@') {
                writer
                    .add_symlink(name, "/bin/sh", SimpleFileOptions::default())
                    .unwrap();
            } else {
                writer
                    .start_file(*name, SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(bytes).unwrap();
            }
        }
        writer.finish().unwrap().into_inner()
    }

    /// A tool manifest pinning `asset` (bytes) for macos-aarch64 with the given entry.
    fn tool_manifest(asset: &[u8], entry: &str, entry_sha256: Option<String>) -> ToolManifest {
        let mut manifest = examples::leapp_manifest().tools[&ToolId::Ileapp].clone();
        manifest.version = VERSION.into();
        let pinned = PlatformAsset {
            asset_name: "ileapp-test.zip".into(),
            asset_size: asset.len() as u64,
            asset_sha256: sha256_of(asset),
            archive_kind: ArchiveKind::Zip,
            entry: entry.into(),
            entry_sha256,
            urls: Vec::new(),
        };
        manifest.platforms = [(PlatformKey::MacosAarch64, pinned)].into();
        manifest
    }

    fn asset_mut(manifest: &mut ToolManifest) -> &mut PlatformAsset {
        manifest
            .platforms
            .get_mut(&PlatformKey::MacosAarch64)
            .unwrap()
    }

    fn pinned<'a>(tools_dir: &'a Path, manifest: &'a ToolManifest) -> Pinned<'a> {
        Pinned {
            tools_dir,
            tool: ToolId::Ileapp,
            manifest,
            platform: Some(PlatformKey::MacosAarch64),
        }
    }

    fn modules(count: usize) -> ModulesFile {
        let mut file = examples::modules_file();
        file.version = VERSION.into();
        file.modules = (0..count)
            .map(|i| ModuleInfo {
                name: format!("m{i}"),
                module_name: format!("m{i}"),
                category: "C".into(),
                display_name: format!("M {i}"),
                description: None,
            })
            .collect();
        file
    }

    /// Records events and the entry path the introspection callback saw; `cancel` is the install's
    /// cancel flag.
    #[derive(Default)]
    struct Run {
        events: Vec<InstallEvent>,
        introspected: Option<PathBuf>,
        cancel: Arc<AtomicBool>,
    }

    impl Run {
        fn install(
            &mut self,
            pinned: Pinned<'_>,
            source: Source<'_>,
        ) -> Result<InstallRecord, InstallError> {
            let Run {
                events,
                introspected,
                cancel,
            } = self;
            install(
                pinned,
                source,
                cancel,
                &mut |event| events.push(event),
                &mut |entry| {
                    assert!(entry.is_absolute() || entry.starts_with(pinned.tools_dir));
                    assert_eq!(fs::read(entry).unwrap(), ENTRY_BYTES);
                    *introspected = Some(entry.to_path_buf());
                    Ok(modules(3))
                },
            )
        }

        fn stages(&self) -> Vec<InstallStage> {
            self.events
                .iter()
                .filter_map(|e| match e {
                    InstallEvent::Stage { stage } => Some(*stage),
                    _ => None,
                })
                .collect()
        }
    }

    /// What a local HTTP server sends for one request.
    #[derive(Clone)]
    enum Reply {
        /// 200 with a Content-Length.
        Body(Vec<u8>),
        /// 200 announcing `declared` bytes, then closing after `body`.
        Truncated {
            declared: usize,
            body: Vec<u8>,
        },
        /// 200 without a Content-Length: the body ends when the connection closes.
        CloseDelimited(Vec<u8>),
        Status(u16),
        /// 200 announcing `declared` bytes, then `body`, then silence for `hold` before closing.
        Stall {
            declared: usize,
            body: Vec<u8>,
            hold: Duration,
        },
    }

    /// A plain-HTTP server on 127.0.0.1 answering one request per reply, in order. Returns its
    /// base URL.
    fn serve(replies: Vec<Reply>) -> (String, JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            for reply in replies {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = io::BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap() > 0 && line != "\r\n" {
                    line.clear();
                }
                let head = |status: &str, length: Option<usize>| {
                    let length =
                        length.map_or_else(String::new, |n| format!("Content-Length: {n}\r\n"));
                    format!("HTTP/1.1 {status}\r\n{length}Connection: close\r\n\r\n")
                };
                let mut hold = Duration::ZERO;
                let bytes = match reply {
                    Reply::Body(body) => [head("200 OK", Some(body.len())).into_bytes(), body],
                    Reply::Truncated { declared, body } => {
                        [head("200 OK", Some(declared)).into_bytes(), body]
                    }
                    Reply::CloseDelimited(body) => [head("200 OK", None).into_bytes(), body],
                    Reply::Status(code) => [
                        head(&format!("{code} Nope"), Some(0)).into_bytes(),
                        Vec::new(),
                    ],
                    Reply::Stall {
                        declared,
                        body,
                        hold: silence,
                    } => {
                        hold = silence;
                        [head("200 OK", Some(declared)).into_bytes(), body]
                    }
                };
                let _ = stream.write_all(&bytes.concat());
                let _ = stream.flush();
                thread::sleep(hold);
            }
        });
        (base, handle)
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .map(|entries| {
                entries
                    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// Sets up a tools dir below a fresh temp dir (so "nothing left behind" covers the tools dir
    /// itself) and a manifest serving `asset` from a local server.
    struct Case {
        root: tempfile::TempDir,
        manifest: ToolManifest,
    }

    impl Case {
        fn new(asset: &[u8], entry: &str, entry_sha256: Option<String>, urls: Vec<String>) -> Self {
            let mut manifest = tool_manifest(asset, entry, entry_sha256);
            asset_mut(&mut manifest).urls = urls;
            Self {
                root: tempfile::tempdir().unwrap(),
                manifest,
            }
        }

        fn tools_dir(&self) -> PathBuf {
            self.root.path().join("leapp")
        }

        fn assert_nothing_left(&self) {
            assert_eq!(entries(self.root.path()), Vec::<String>::new());
        }
    }

    fn good_zip() -> Vec<u8> {
        zip_bytes(&[("ileapp", ENTRY_BYTES), ("README.txt", b"not extracted")])
    }

    #[test]
    fn a_successful_download_install() {
        let asset = good_zip();
        let (base, server) = serve(vec![Reply::Body(asset.clone())]);
        let url = format!("{base}/ileapp-test.zip");
        let case = Case::new(
            &asset,
            "ileapp",
            Some(sha256_of(ENTRY_BYTES)),
            vec![url.clone()],
        );
        let tools_dir = case.tools_dir();
        let pinned = pinned(&tools_dir, &case.manifest);
        let mut run = Run::default();
        let record = run.install(pinned, Source::Download).unwrap();
        server.join().unwrap();

        assert_eq!(
            run.stages(),
            [
                InstallStage::Downloading,
                InstallStage::Verifying,
                InstallStage::Extracting,
                InstallStage::Hashing,
                InstallStage::Introspecting,
                InstallStage::Done
            ]
        );
        assert!(run.events.contains(&InstallEvent::DownloadProgress {
            bytes_done: asset.len() as u64,
            bytes_total: asset.len() as u64
        }));
        assert_eq!(record.tool, ToolId::Ileapp);
        assert_eq!(record.version, VERSION);
        assert_eq!(record.platform, PlatformKey::MacosAarch64);
        assert_eq!(record.asset_sha256, sha256_of(&asset));
        assert_eq!(record.entry_path, "bin/ileapp");
        assert_eq!(record.entry_sha256, sha256_of(ENTRY_BYTES));
        assert_eq!(record.source, InstallSource::Download);
        assert_eq!(record.source_detail, url);
        assert_eq!(record.module_count, 3);

        // The layout: install.json, modules.json and only the entry; no staging dir, no asset.
        let dir = pinned.version_dir();
        assert_eq!(entries(&pinned.tool_dir()), [VERSION]);
        assert_eq!(entries(&dir), ["bin", "install.json", "modules.json"]);
        assert_eq!(entries(&dir.join("bin")), ["ileapp"]);
        let saved: InstallRecord =
            parse_versioned(&fs::read(dir.join(INSTALL_FILE)).unwrap()).unwrap();
        assert_eq!(saved, record);
        let saved: ModulesFile =
            parse_versioned(&fs::read(dir.join(MODULES_FILE)).unwrap()).unwrap();
        assert_eq!(saved, modules(3));
        // Introspection ran the staged entry, which then moved into place.
        let introspected = run.introspected.unwrap();
        assert!(introspected.ends_with(Path::new("bin").join("ileapp")));
        assert!(!introspected.starts_with(&dir));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.join("bin/ileapp"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }

        let check = verify(pinned);
        assert_eq!(check.status.state, ToolState::Verified);
        assert_eq!(check.status.installed_version.as_deref(), Some(VERSION));
        assert_eq!(check.status.module_count, Some(3));
        assert_eq!(check.status.install_source, Some(InstallSource::Download));
        let verified = check.verified.unwrap();
        assert_eq!(verified.entry, dir.join("bin").join("ileapp"));
        assert_eq!(verified.verified_against, EntryVerifiedAgainst::Manifest);
        assert_eq!(status(pinned).status.state, ToolState::InstalledUnverified);
    }

    #[test]
    fn falls_back_to_the_next_url() {
        let asset = good_zip();
        let (base, server) = serve(vec![Reply::Status(404), Reply::Body(asset.clone())]);
        let urls = vec![format!("{base}/missing.zip"), format!("{base}/mirror.zip")];
        let case = Case::new(&asset, "ileapp", Some(sha256_of(ENTRY_BYTES)), urls.clone());
        let tools_dir = case.tools_dir();
        let mut run = Run::default();
        let record = run
            .install(pinned(&tools_dir, &case.manifest), Source::Download)
            .unwrap();
        server.join().unwrap();
        assert_eq!(record.source_detail, urls[1]);
        let messages: Vec<_> = run
            .events
            .iter()
            .filter(|e| matches!(e, InstallEvent::Message { .. }))
            .collect();
        assert_eq!(messages.len(), 1, "{messages:?}");
        // The 404 failed while downloading, so the stage never left `downloading`.
        assert_eq!(
            run.stages()[..2],
            [InstallStage::Downloading, InstallStage::Verifying]
        );
    }

    #[test]
    fn a_mirror_after_a_hash_mismatch_is_reported_as_downloading_again() {
        let asset = good_zip();
        let mut tampered = asset.clone();
        tampered[0] ^= 1;
        let (base, server) = serve(vec![Reply::Body(tampered), Reply::Body(asset.clone())]);
        let urls = vec![format!("{base}/bad.zip"), format!("{base}/mirror.zip")];
        let case = Case::new(&asset, "ileapp", Some(sha256_of(ENTRY_BYTES)), urls.clone());
        let tools_dir = case.tools_dir();
        let mut run = Run::default();
        let record = run
            .install(pinned(&tools_dir, &case.manifest), Source::Download)
            .unwrap();
        server.join().unwrap();
        assert_eq!(record.source_detail, urls[1]);
        assert_eq!(
            run.stages(),
            [
                InstallStage::Downloading,
                InstallStage::Verifying,
                InstallStage::Downloading,
                InstallStage::Verifying,
                InstallStage::Extracting,
                InstallStage::Hashing,
                InstallStage::Introspecting,
                InstallStage::Done
            ]
        );
    }

    #[test]
    fn a_stalled_download_fails_in_bounded_time() {
        let asset = good_zip();
        // The server stays silent long enough that only the stall timeout can end the download
        // (its close would be reported as a truncated download instead).
        let hold = STALL_TIMEOUT + SLOW_RUNNER;
        let (base, _server) = serve(vec![Reply::Stall {
            declared: asset.len(),
            body: asset[..asset.len() / 2].to_vec(),
            hold,
        }]);
        let case = Case::new(&asset, "ileapp", Some(sha256_of(ENTRY_BYTES)), vec![base]);
        let tools_dir = case.tools_dir();
        let started = Instant::now();
        let error = Run::default()
            .install(pinned(&tools_dir, &case.manifest), Source::Download)
            .unwrap_err();
        let took = started.elapsed();
        assert_eq!(error.code(), ErrorCode::DownloadFailed, "{error}");
        assert!(error.to_string().contains("stalled download"), "{error}");
        // Bounded: the stall timeout, plus the staging cleanup (which may retry for up to
        // TRANSIENT_RETRY on Windows), plus slack for a loaded machine.
        let bound = STALL_TIMEOUT + TRANSIENT_RETRY + SLOW_RUNNER;
        assert!(took >= STALL_TIMEOUT && took < bound, "{took:?}");
        case.assert_nothing_left();
        // The server thread ends on its own once its silence is over; no need to wait for it.
    }

    #[test]
    fn an_install_can_be_cancelled() {
        // While a download waits for data.
        let asset = good_zip();
        let hold = STALL_TIMEOUT + Duration::from_secs(1);
        let (base, server) = serve(vec![Reply::Stall {
            declared: asset.len(),
            body: asset[..10].to_vec(),
            hold,
        }]);
        let case = Case::new(&asset, "ileapp", Some(sha256_of(ENTRY_BYTES)), vec![base]);
        let tools_dir = case.tools_dir();
        let mut run = Run::default();
        let cancel = Arc::clone(&run.cancel);
        let canceller = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            cancel.store(true, Ordering::Relaxed);
        });
        let started = Instant::now();
        let error = run
            .install(pinned(&tools_dir, &case.manifest), Source::Download)
            .unwrap_err();
        // Cancelled, not stalled: the flag was noticed while a read was blocked.
        assert!(matches!(error, InstallError::Cancelled), "{error}");
        // Bounded: the cancel delay, plus the staging cleanup (up to TRANSIENT_RETRY on Windows),
        // plus slack for a loaded machine.
        let bound = Duration::from_millis(300) + TRANSIENT_RETRY + SLOW_RUNNER;
        assert!(started.elapsed() < bound, "{:?}", started.elapsed());
        let app: AppError = error.into();
        assert_eq!(app.code, ErrorCode::DownloadFailed);
        assert_eq!(app.message, "the installation was cancelled");
        canceller.join().unwrap();
        case.assert_nothing_left();
        server.join().unwrap();

        // Before an offline import starts.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("asset.zip");
        fs::write(&src, &asset).unwrap();
        let run = Run::default();
        run.cancel.store(true, Ordering::Relaxed);
        let mut run = run;
        let error = run
            .install(pinned(&tools_dir, &case.manifest), Source::File(&src))
            .unwrap_err();
        assert!(matches!(error, InstallError::Cancelled), "{error}");
        assert!(run.introspected.is_none());
        case.assert_nothing_left();
    }

    #[test]
    fn the_copy_and_extract_loops_stop_on_cancel() {
        let asset = good_zip();
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("asset.zip");
        fs::write(&src, &asset).unwrap();
        let manifest = tool_manifest(&asset, "ileapp", None);
        let pinned_asset = &manifest.platforms[&PlatformKey::MacosAarch64];
        let cancelled = AtomicBool::new(true);
        let copy = copy_verified(&src, pinned_asset, &dir.path().join("copy.zip"), &cancelled);
        assert!(matches!(copy, Err(InstallError::Cancelled)));
        let staging = dir.path().join("staging");
        fs::create_dir(&staging).unwrap();
        let extracted = extract(pinned_asset, &src, &staging, &cancelled);
        assert!(matches!(extracted, Err(InstallError::Cancelled)));
    }

    #[test]
    fn leftovers_of_an_interrupted_install_are_removed() {
        let asset = good_zip();
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("asset.zip");
        fs::write(&src, &asset).unwrap();
        let case = Case::new(&asset, "ileapp", Some(sha256_of(ENTRY_BYTES)), Vec::new());
        let tools_dir = case.tools_dir();
        let pinned = pinned(&tools_dir, &case.manifest);
        let tool_dir = pinned.tool_dir();
        for leftover in [".staging-0a1b2c", ".old-ffffff"] {
            fs::create_dir_all(tool_dir.join(leftover).join("bin")).unwrap();
            fs::write(
                tool_dir.join(leftover).join("bin").join("ileapp"),
                "partial",
            )
            .unwrap();
        }
        // Not leftovers: other versions and anything not named like one.
        for keep in ["v2025.1.0", ".staging-XYZ123", ".staging-0a1b2c3", "notes"] {
            fs::create_dir_all(tool_dir.join(keep)).unwrap();
        }
        let mut run = Run::default();
        run.install(pinned, Source::File(&src)).unwrap();
        assert_eq!(
            entries(&tool_dir),
            [
                ".staging-0a1b2c3",
                ".staging-XYZ123",
                "notes",
                "v2025.1.0",
                VERSION
            ]
        );
        assert!(run.events.contains(&InstallEvent::Message {
            text: "removed 2 leftover folder(s) of an interrupted install".to_owned()
        }));
        // The live version dir is never a leftover, even if it were named like one.
        assert_eq!(remove_leftovers(&tool_dir, VERSION), 0);
        fs::create_dir(tool_dir.join(".old-123abc")).unwrap();
        assert_eq!(remove_leftovers(&tool_dir, ".old-123abc"), 0);
        assert!(tool_dir.join(".old-123abc").is_dir());
    }

    #[test]
    fn a_relative_tools_dir_is_made_absolute() {
        let manifest = tool_manifest(b"x", "ileapp", None);
        let pinned = pinned(Path::new("relative-tools"), &manifest);
        let dir = pinned.version_dir();
        assert!(dir.is_absolute(), "{}", dir.display());
        assert_eq!(
            dir,
            std::env::current_dir()
                .unwrap()
                .join("relative-tools")
                .join("ileapp")
                .join(VERSION)
        );
    }

    #[test]
    fn an_asset_hash_mismatch_installs_nothing() {
        let asset = good_zip();
        let mut tampered = asset.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xff;
        let (base, server) = serve(vec![Reply::Body(tampered)]);
        let case = Case::new(&asset, "ileapp", Some(sha256_of(ENTRY_BYTES)), vec![base]);
        let tools_dir = case.tools_dir();
        let mut run = Run::default();
        let error = run
            .install(pinned(&tools_dir, &case.manifest), Source::Download)
            .unwrap_err();
        server.join().unwrap();
        assert_eq!(error.code(), ErrorCode::HashMismatch, "{error}");
        // Nothing was extracted or introspected.
        assert!(!run.stages().contains(&InstallStage::Extracting));
        assert!(run.introspected.is_none());
        case.assert_nothing_left();
    }

    #[test]
    fn an_entry_hash_mismatch_installs_nothing() {
        let asset = good_zip();
        let (base, server) = serve(vec![Reply::Body(asset.clone())]);
        let case = Case::new(&asset, "ileapp", Some(sha256_of(b"other")), vec![base]);
        let tools_dir = case.tools_dir();
        let error = Run::default()
            .install(pinned(&tools_dir, &case.manifest), Source::Download)
            .unwrap_err();
        server.join().unwrap();
        assert_eq!(error.code(), ErrorCode::HashMismatch, "{error}");
        assert!(
            error.to_string().contains("ileapp (extracted from"),
            "{error}"
        );
        case.assert_nothing_left();
    }

    #[test]
    fn a_body_beyond_the_pinned_size_is_aborted() {
        let asset = good_zip();
        let mut longer = asset.clone();
        longer.extend_from_slice(&[0u8; 4096]);
        // Announced too large, and too large without an announcement.
        let (base, server) = serve(vec![
            Reply::Body(longer.clone()),
            Reply::CloseDelimited(longer),
        ]);
        let case = Case::new(
            &asset,
            "ileapp",
            Some(sha256_of(ENTRY_BYTES)),
            vec![format!("{base}/a"), format!("{base}/b")],
        );
        let tools_dir = case.tools_dir();
        let mut run = Run::default();
        let error = run
            .install(pinned(&tools_dir, &case.manifest), Source::Download)
            .unwrap_err();
        server.join().unwrap();
        assert_eq!(error.code(), ErrorCode::DownloadFailed, "{error}");
        let texts: Vec<String> = run
            .events
            .iter()
            .filter_map(|e| match e {
                InstallEvent::Message { text } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(texts[0].contains("announces"), "{texts:?}");
        assert!(texts[1].contains("more than the pinned"), "{texts:?}");
        case.assert_nothing_left();
    }

    #[test]
    fn a_truncated_download_is_rejected() {
        let asset = good_zip();
        let half = asset[..asset.len() / 2].to_vec();
        let (base, server) = serve(vec![
            Reply::Truncated {
                declared: asset.len(),
                body: half.clone(),
            },
            Reply::CloseDelimited(half),
        ]);
        let case = Case::new(
            &asset,
            "ileapp",
            Some(sha256_of(ENTRY_BYTES)),
            vec![format!("{base}/a"), format!("{base}/b")],
        );
        let tools_dir = case.tools_dir();
        let mut run = Run::default();
        let error = run
            .install(pinned(&tools_dir, &case.manifest), Source::Download)
            .unwrap_err();
        server.join().unwrap();
        assert_eq!(error.code(), ErrorCode::DownloadFailed, "{error}");
        assert!(error.to_string().contains("truncated"), "{error}");
        case.assert_nothing_left();
    }

    #[test]
    fn only_https_urls_are_fetched() {
        assert!(url_allowed("https://github.com/x"));
        assert!(!url_allowed("http://github.com/x"));
        assert!(!url_allowed("ftp://127.0.0.1/x"));
        let asset = good_zip();
        let case = Case::new(
            &asset,
            "ileapp",
            None,
            vec!["http://example.com/ileapp.zip".into()],
        );
        let tools_dir = case.tools_dir();
        let error = Run::default()
            .install(pinned(&tools_dir, &case.manifest), Source::Download)
            .unwrap_err();
        assert!(error.to_string().contains("not an https:// URL"), "{error}");
        case.assert_nothing_left();
    }

    #[test]
    fn zip_slip_and_symlink_entries_are_rejected() {
        let outside = tempfile::tempdir().unwrap();
        for entry in [
            "../../evil",
            "/evil",
            "bin/../../evil",
            "..\\evil",
            "C:evil",
        ] {
            let asset = zip_bytes(&[(entry, ENTRY_BYTES)]);
            let dir = tempfile::tempdir().unwrap();
            let src = dir.path().join("asset.zip");
            fs::write(&src, &asset).unwrap();
            let case = Case::new(&asset, entry, Some(sha256_of(ENTRY_BYTES)), Vec::new());
            let tools_dir = case.tools_dir();
            let error = Run::default()
                .install(pinned(&tools_dir, &case.manifest), Source::File(&src))
                .unwrap_err();
            assert_eq!(error.code(), ErrorCode::ExtractFailed, "{entry}: {error}");
            case.assert_nothing_left();
            assert!(entries(outside.path()).is_empty());
        }
        // A symlink named like the entry.
        let asset = zip_bytes(&[("ileapp@", b"")]);
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("asset.zip");
        fs::write(&src, &asset).unwrap();
        let case = Case::new(&asset, "ileapp", Some(sha256_of(ENTRY_BYTES)), Vec::new());
        let tools_dir = case.tools_dir();
        let error = Run::default()
            .install(pinned(&tools_dir, &case.manifest), Source::File(&src))
            .unwrap_err();
        assert!(error.to_string().contains("not a regular file"), "{error}");
        case.assert_nothing_left();
        // An archive without the entry.
        let asset = zip_bytes(&[("other", ENTRY_BYTES)]);
        fs::write(&src, &asset).unwrap();
        let case = Case::new(&asset, "ileapp", Some(sha256_of(ENTRY_BYTES)), Vec::new());
        let tools_dir = case.tools_dir();
        let error = Run::default()
            .install(pinned(&tools_dir, &case.manifest), Source::File(&src))
            .unwrap_err();
        assert!(error.to_string().contains("has no entry"), "{error}");
        case.assert_nothing_left();
    }

    #[test]
    fn a_successful_offline_import() {
        let asset = good_zip();
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("downloaded elsewhere.zip");
        fs::write(&src, &asset).unwrap();
        crate::fsutil::set_read_only(&src).unwrap();
        let case = Case::new(&asset, "ileapp", Some(sha256_of(ENTRY_BYTES)), Vec::new());
        let tools_dir = case.tools_dir();
        let pinned = pinned(&tools_dir, &case.manifest);
        let mut run = Run::default();
        let record = run.install(pinned, Source::File(&src)).unwrap();
        assert_eq!(record.source, InstallSource::OfflineImport);
        assert_eq!(record.source_detail, src.to_string_lossy());
        assert_eq!(record.asset_name, "ileapp-test.zip");
        assert_eq!(
            run.stages(),
            [
                InstallStage::Verifying,
                InstallStage::Extracting,
                InstallStage::Hashing,
                InstallStage::Introspecting,
                InstallStage::Done
            ]
        );
        // The imported file is untouched.
        assert_eq!(fs::read(&src).unwrap(), asset);
        assert!(fs::metadata(&src).unwrap().permissions().readonly());
        assert_eq!(verify(pinned).status.state, ToolState::Verified);
        crate::fsutil::test_support::make_writable(&src);
    }

    #[test]
    fn an_offline_import_of_the_wrong_file_installs_nothing() {
        let asset = good_zip();
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("wrong.zip");
        let case = Case::new(&asset, "ileapp", Some(sha256_of(ENTRY_BYTES)), Vec::new());
        let tools_dir = case.tools_dir();
        let pinned = pinned(&tools_dir, &case.manifest);
        // Same size, different bytes; then a different size.
        let mut same_size = asset.clone();
        same_size[0] ^= 1;
        for bytes in [same_size, b"short".to_vec()] {
            fs::write(&src, &bytes).unwrap();
            let error = Run::default()
                .install(pinned, Source::File(&src))
                .unwrap_err();
            assert_eq!(error.code(), ErrorCode::HashMismatch, "{error}");
            case.assert_nothing_left();
        }
        let error = Run::default()
            .install(pinned, Source::File(dir.path()))
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::Io, "{error}");
        case.assert_nothing_left();
    }

    #[test]
    fn a_failed_introspection_installs_nothing() {
        let asset = good_zip();
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("asset.zip");
        fs::write(&src, &asset).unwrap();
        let case = Case::new(&asset, "ileapp", Some(sha256_of(ENTRY_BYTES)), Vec::new());
        let tools_dir = case.tools_dir();
        let error = install(
            pinned(&tools_dir, &case.manifest),
            Source::File(&src),
            &NO_CANCEL,
            &mut |_| {},
            &mut |_| {
                Err(InstallError::Introspection {
                    message: "only 12 modules".into(),
                    detail: Some("stderr tail".into()),
                })
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::IntrospectionFailed);
        let app: AppError = error.into();
        assert_eq!(app.detail.as_deref(), Some("stderr tail"));
        case.assert_nothing_left();

        // Modules of another version are refused too.
        let error = install(
            pinned(&tools_dir, &case.manifest),
            Source::File(&src),
            &NO_CANCEL,
            &mut |_| {},
            &mut |_| {
                let mut other = modules(3);
                other.version = "v1".into();
                Ok(other)
            },
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::IntrospectionFailed);
        case.assert_nothing_left();
    }

    #[test]
    fn existing_versions_survive_a_failed_install_and_a_reinstall_replaces() {
        let asset = good_zip();
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("asset.zip");
        fs::write(&src, &asset).unwrap();
        let case = Case::new(&asset, "ileapp", Some(sha256_of(ENTRY_BYTES)), Vec::new());
        let tools_dir = case.tools_dir();
        let pinned = pinned(&tools_dir, &case.manifest);
        let other = pinned.tool_dir().join("v2025.1.0");
        fs::create_dir_all(&other).unwrap();
        let error = Run::default()
            .install(pinned, Source::File(dir.path().join("missing").as_path()))
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::Io);
        assert_eq!(entries(&pinned.tool_dir()), ["v2025.1.0"]);

        Run::default().install(pinned, Source::File(&src)).unwrap();
        fs::write(pinned.version_dir().join("stray"), "x").unwrap();
        let second = Run::default().install(pinned, Source::File(&src)).unwrap();
        assert_eq!(
            entries(&pinned.tool_dir()),
            ["v2025.1.0", VERSION],
            "no .old or .staging dirs are left"
        );
        assert!(!pinned.version_dir().join("stray").exists());
        let saved: InstallRecord =
            parse_versioned(&fs::read(pinned.version_dir().join(INSTALL_FILE)).unwrap()).unwrap();
        assert_eq!(saved, second);
    }

    #[test]
    fn tampering_fails_verification() {
        let asset = good_zip();
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("asset.zip");
        fs::write(&src, &asset).unwrap();
        let case = Case::new(&asset, "ileapp", Some(sha256_of(ENTRY_BYTES)), Vec::new());
        let tools_dir = case.tools_dir();
        let pinned = pinned(&tools_dir, &case.manifest);
        Run::default().install(pinned, Source::File(&src)).unwrap();
        assert_eq!(verify(pinned).status.state, ToolState::Verified);

        // Flip one byte of the installed entry.
        let entry = pinned.version_dir().join("bin").join("ileapp");
        let mut bytes = fs::read(&entry).unwrap();
        bytes[3] ^= 0x01;
        fs::write(&entry, &bytes).unwrap();
        let check = verify(pinned);
        assert_eq!(check.status.state, ToolState::VerificationFailed);
        assert!(check.verified.is_none());
        let problem = check.status.problem.unwrap();
        assert!(problem.contains("the manifest pins"), "{problem}");
        // The cheap check does not hash.
        assert_eq!(status(pinned).status.state, ToolState::InstalledUnverified);
    }

    #[test]
    fn install_states() {
        let asset = good_zip();
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("asset.zip");
        fs::write(&src, &asset).unwrap();
        let case = Case::new(&asset, "ileapp", Some(sha256_of(ENTRY_BYTES)), Vec::new());
        let tools_dir = case.tools_dir();
        let pinned = pinned(&tools_dir, &case.manifest);

        let check = status(pinned);
        assert_eq!(check.status.state, ToolState::NotInstalled);
        assert_eq!(check.status.problem, None);
        assert_eq!(check.status.pinned_version, VERSION);
        assert_eq!(check.status.display_name, "iLEAPP");
        assert_eq!(verify(pinned).status.state, ToolState::NotInstalled);

        for platform in [None, Some(PlatformKey::WindowsX86_64)] {
            let unsupported = Pinned { platform, ..pinned };
            assert_eq!(
                status(unsupported).status.state,
                ToolState::UnsupportedPlatform
            );
            let error = Run::default()
                .install(unsupported, Source::File(&src))
                .unwrap_err();
            assert_eq!(error.code(), ErrorCode::UnsupportedPlatform);
        }
        case.assert_nothing_left();

        Run::default().install(pinned, Source::File(&src)).unwrap();
        let check = status(pinned);
        assert_eq!(check.status.state, ToolState::InstalledUnverified);
        assert_eq!(
            check.status.install_dir.as_deref(),
            Some(pinned.version_dir().to_string_lossy().as_ref())
        );

        // Without modules.json the tool is not installed (CONTRACTS.md §4).
        let modules_json = pinned.version_dir().join(MODULES_FILE);
        let saved = fs::read(&modules_json).unwrap();
        fs::remove_file(&modules_json).unwrap();
        let check = verify(pinned);
        assert_eq!(check.status.state, ToolState::NotInstalled);
        assert!(check.status.problem.unwrap().contains("modules.json"));
        fs::write(&modules_json, saved).unwrap();

        // A record of another asset (a manifest bump) is not the pinned install.
        let mut bumped = case.manifest.clone();
        asset_mut(&mut bumped).asset_sha256 = "0".repeat(64);
        let check = status(Pinned {
            manifest: &bumped,
            ..pinned
        });
        assert_eq!(check.status.state, ToolState::NotInstalled);
        assert!(check.status.problem.unwrap().contains("not the pinned one"));

        // A missing entry, and an unreadable install.json.
        fs::remove_file(pinned.version_dir().join("bin").join("ileapp")).unwrap();
        assert!(
            status(pinned)
                .status
                .problem
                .unwrap()
                .contains("is missing")
        );
        fs::write(pinned.version_dir().join(INSTALL_FILE), "{").unwrap();
        let check = status(pinned);
        assert_eq!(check.status.state, ToolState::NotInstalled);
        assert!(check.status.problem.unwrap().contains("unusable"));
    }

    #[test]
    fn a_null_manifest_entry_hash_is_recorded_and_verified_against_the_record() {
        let asset = good_zip();
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("asset.zip");
        fs::write(&src, &asset).unwrap();
        // A zip with a null entry hash is invalid in a real manifest, but the null-hash path is the
        // same for AppImages; this exercises it on every OS.
        let case = Case::new(&asset, "ileapp", None, Vec::new());
        let tools_dir = case.tools_dir();
        let pinned = pinned(&tools_dir, &case.manifest);
        let record = Run::default().install(pinned, Source::File(&src)).unwrap();
        assert_eq!(record.entry_sha256, sha256_of(ENTRY_BYTES));
        let check = verify(pinned);
        assert_eq!(check.status.state, ToolState::Verified);
        assert_eq!(
            check.verified.unwrap().verified_against,
            EntryVerifiedAgainst::InstallRecord
        );
        let entry = pinned.version_dir().join("bin").join("ileapp");
        fs::write(&entry, b"tampered").unwrap();
        let check = verify(pinned);
        assert_eq!(check.status.state, ToolState::VerificationFailed);
        assert!(check.status.problem.unwrap().contains("install.json pins"));
    }

    #[test]
    fn error_codes() {
        let cases = [
            (
                InstallError::UnsupportedPlatform {
                    tool: ToolId::Aleapp,
                    platform: "unknown".into(),
                },
                ErrorCode::UnsupportedPlatform,
            ),
            (
                InstallError::Download("x".into()),
                ErrorCode::DownloadFailed,
            ),
            (InstallError::Extract("x".into()), ErrorCode::ExtractFailed),
            (
                InstallError::io("x")(io::Error::from(io::ErrorKind::PermissionDenied)),
                ErrorCode::PermissionDenied,
            ),
            (
                InstallError::io("x")(io::Error::from(io::ErrorKind::NotFound)),
                ErrorCode::Io,
            ),
        ];
        for (error, code) in cases {
            let app: AppError = error.into();
            assert_eq!(app.code, code);
        }
    }

    /// The AppImage path runs only on Linux: a fake AppImage (a shell script) implements
    /// `--appimage-extract` like the real runtime.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_appimage_is_extracted_with_appimage_extract() {
        let script = format!(
            "#!/bin/sh\n\
             [ \"$1\" = \"--appimage-extract\" ] || exit 2\n\
             mkdir -p squashfs-root/usr/bin\n\
             printf '%s' '{}' > squashfs-root/usr/bin/ileapp\n\
             chmod 755 squashfs-root/usr/bin/ileapp\n\
             ln -s usr/bin/ileapp squashfs-root/AppRun\n\
             echo squashfs-root/usr/bin/ileapp\n",
            String::from_utf8_lossy(ENTRY_BYTES).trim_end()
        );
        let entry_bytes = String::from_utf8_lossy(ENTRY_BYTES).trim_end().to_owned();
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("ileapp.AppImage");
        fs::write(&src, &script).unwrap();
        let mut case = Case::new(script.as_bytes(), "usr/bin/ileapp", None, Vec::new());
        asset_mut(&mut case.manifest).archive_kind = ArchiveKind::Appimage;
        case.manifest.platforms = [(
            PlatformKey::LinuxX86_64,
            case.manifest.platforms[&PlatformKey::MacosAarch64].clone(),
        )]
        .into();
        let tools_dir = case.tools_dir();
        let record = install(
            linux_pinned(&tools_dir, &case.manifest),
            Source::File(&src),
            &NO_CANCEL,
            &mut |_| {},
            &mut |entry| {
                assert_eq!(fs::read_to_string(entry).unwrap(), entry_bytes);
                Ok(modules(3))
            },
        )
        .unwrap();
        assert_eq!(record.entry_path, "squashfs-root/usr/bin/ileapp");
        assert_eq!(record.entry_sha256, sha256_of(entry_bytes.as_bytes()));
        // The asset itself is not kept, and the imported file was not made executable.
        let version_dir = linux_pinned(&tools_dir, &case.manifest).version_dir();
        assert_eq!(
            entries(&version_dir),
            ["install.json", "modules.json", "squashfs-root"]
        );
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&src).unwrap().permissions().mode() & 0o111, 0);
        let check = verify(linux_pinned(&tools_dir, &case.manifest));
        assert_eq!(check.status.state, ToolState::Verified);
        assert_eq!(
            check.verified.unwrap().verified_against,
            EntryVerifiedAgainst::InstallRecord
        );

        // A pinned entry hash that does not match, and an entry that is not extracted.
        asset_mut_linux(&mut case.manifest).entry_sha256 = Some(sha256_of(b"other"));
        let error = install(
            linux_pinned(&tools_dir, &case.manifest),
            Source::File(&src),
            &NO_CANCEL,
            &mut |_| {},
            &mut |_| Ok(modules(3)),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::HashMismatch, "{error}");
        asset_mut_linux(&mut case.manifest).entry = "usr/bin/missing".into();
        let error = install(
            linux_pinned(&tools_dir, &case.manifest),
            Source::File(&src),
            &NO_CANCEL,
            &mut |_| {},
            &mut |_| Ok(modules(3)),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ExtractFailed, "{error}");
        assert!(
            error.to_string().contains("missing after extraction"),
            "{error}"
        );
        // The failed installs left the earlier install in place and nothing else.
        let tool_dir = linux_pinned(&tools_dir, &case.manifest).tool_dir();
        assert_eq!(entries(&tool_dir), [VERSION]);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn appimage_extraction_never_inherits_the_appimage_variables() {
        let command = appimage_extract_command(Path::new("/x.AppImage"), Path::new("/staging"));
        let removed: Vec<String> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();
        for name in APPIMAGE_VARS {
            assert!(removed.iter().any(|r| r == name), "{name} not removed");
        }
        assert_eq!(command.get_program(), "/x.AppImage");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--appimage-extract"]
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("/staging")));
    }

    #[cfg(target_os = "linux")]
    fn linux_pinned<'a>(tools_dir: &'a Path, manifest: &'a ToolManifest) -> Pinned<'a> {
        Pinned {
            platform: Some(PlatformKey::LinuxX86_64),
            ..pinned(tools_dir, manifest)
        }
    }

    #[cfg(target_os = "linux")]
    fn asset_mut_linux(manifest: &mut ToolManifest) -> &mut PlatformAsset {
        manifest
            .platforms
            .get_mut(&PlatformKey::LinuxX86_64)
            .unwrap()
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn appimages_are_refused_off_linux() {
        let asset = b"#!/bin/sh\n".to_vec();
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("x.AppImage");
        fs::write(&src, &asset).unwrap();
        let mut case = Case::new(&asset, "usr/bin/ileapp", None, Vec::new());
        asset_mut(&mut case.manifest).archive_kind = ArchiveKind::Appimage;
        let tools_dir = case.tools_dir();
        let error = Run::default()
            .install(pinned(&tools_dir, &case.manifest), Source::File(&src))
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::ExtractFailed);
        assert!(error.to_string().contains("only be installed on Linux"));
        case.assert_nothing_left();
    }
}
