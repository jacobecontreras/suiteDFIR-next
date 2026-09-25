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
//!    SHA-256 is computed on the way and checked before anything is extracted.
//! 2. Extract: a zip yields **only** `entry` (a regular file with a plain relative name; absolute
//!    paths, `..` and symlinks are rejected), mode 0755 on Unix. An AppImage is made executable and
//!    run once with `--appimage-extract` (Linux only), and `squashfs-root/<entry>` must exist.
//! 3. Hash the entry and check it against the manifest `entry_sha256` (`hash_mismatch`). Where the
//!    manifest has `null` (AppImages until ROADMAP E3), the hash is recorded in `install.json`.
//! 4. Introspect through the caller's callback (ROADMAP A3), then write `modules.json` and
//!    `install.json`.
//! 5. Rename the staging dir to `<version>` (replacing an earlier install of that version).
//!
//! Any failure removes the staging dir, and the tool dirs if the install created them, so a failed
//! install leaves nothing behind.
//!
//! **Checks.** [`status`] is cheap (no hashing): `installed_unverified` when the tool is installed
//! per CONTRACTS.md §4. [`verify`] re-hashes the entry: `verified` or `verification_failed`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::contracts::{
    AppError, ArchiveKind, EntryVerifiedAgainst, ErrorCode, InstallEvent, InstallRecord,
    InstallSource, InstallStage, ModulesFile, PlatformAsset, PlatformKey, Timestamp, ToolId,
    ToolManifest, ToolState, ToolStatus, VersionedFile, parse_versioned,
};
use crate::fsutil::write_json_atomic;
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
/// How long renaming the staging dir is retried (Windows: a just-scanned or just-run file may
/// still be open for a moment).
const RENAME_RETRY: Duration = Duration::from_secs(5);
const RENAME_INTERVAL: Duration = Duration::from_millis(100);

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
}

impl InstallError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::UnsupportedPlatform { .. } => ErrorCode::UnsupportedPlatform,
            Self::Download(_) => ErrorCode::DownloadFailed,
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
    /// `<tools_dir>/<tool>`.
    pub fn tool_dir(&self) -> PathBuf {
        self.tools_dir.join(self.tool.as_str())
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
/// staged entry and returns the tool's module list. Events: `stage` in pipeline order,
/// `download_progress` while downloading, and a `message` for each URL that failed.
pub fn install(
    pinned: Pinned<'_>,
    source: Source<'_>,
    on_event: &mut dyn FnMut(InstallEvent),
    introspect: &mut dyn FnMut(&Path) -> Result<ModulesFile, InstallError>,
) -> Result<InstallRecord, InstallError> {
    let (platform, asset) = pinned.asset()?;
    let tool_dir = pinned.tool_dir();
    let created: Vec<PathBuf> = [pinned.tools_dir, tool_dir.as_path()]
        .into_iter()
        .filter(|dir| !dir.exists())
        .map(Path::to_path_buf)
        .collect();
    fs::create_dir_all(&tool_dir)
        .map_err(InstallError::io(format!("creating {}", tool_dir.display())))?;
    let staging = tool_dir.join(format!(".staging-{}", random_hex()?));
    let result = fs::create_dir(&staging)
        .map_err(InstallError::io(format!("creating {}", staging.display())))
        .and_then(|()| {
            let record = stage(
                pinned, platform, asset, source, &staging, on_event, introspect,
            )?;
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

/// Steps 1–4 of the pipeline, inside `staging`.
fn stage(
    pinned: Pinned<'_>,
    platform: PlatformKey,
    asset: &PlatformAsset,
    source: Source<'_>,
    staging: &Path,
    on_event: &mut dyn FnMut(InstallEvent),
    introspect: &mut dyn FnMut(&Path) -> Result<ModulesFile, InstallError>,
) -> Result<InstallRecord, InstallError> {
    let asset_path = staging.join(&asset.asset_name);
    let (install_source, source_detail) = match source {
        Source::Download => (
            InstallSource::Download,
            download(asset, &asset_path, on_event)?,
        ),
        Source::File(path) => {
            on_event(stage_event(InstallStage::Verifying));
            copy_verified(path, asset, &asset_path)?;
            (
                InstallSource::OfflineImport,
                path.to_string_lossy().into_owned(),
            )
        }
    };

    on_event(stage_event(InstallStage::Extracting));
    let entry_path = extract(asset, &asset_path, staging)?;
    fs::remove_file(&asset_path).map_err(InstallError::io(format!(
        "removing {}",
        asset_path.display()
    )))?;

    on_event(stage_event(InstallStage::Hashing));
    let entry = staging.join(&entry_path);
    let entry_sha256 =
        sha256_file(&entry).map_err(InstallError::io(format!("hashing {}", entry.display())))?;
    if let Some(expected) = &asset.entry_sha256
        && *expected != entry_sha256
    {
        return Err(InstallError::HashMismatch {
            what: format!("{} (extracted from {})", asset.entry, asset.asset_name),
            expected: expected.clone(),
            actual: entry_sha256,
        });
    }

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
        platform,
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
/// that URL.
fn download(
    asset: &PlatformAsset,
    dest: &Path,
    on_event: &mut dyn FnMut(InstallEvent),
) -> Result<String, InstallError> {
    on_event(stage_event(InstallStage::Downloading));
    let agent = agent();
    let mut last = None;
    for url in &asset.urls {
        let result = if url_allowed(url) {
            download_from(&agent, url, asset, dest, on_event)
        } else {
            Err(InstallError::Download(format!(
                "{url} is not an https:// URL"
            )))
        };
        match result {
            Ok(()) => return Ok(url.clone()),
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

fn download_from(
    agent: &ureq::Agent,
    url: &str,
    asset: &PlatformAsset,
    dest: &Path,
    on_event: &mut dyn FnMut(InstallEvent),
) -> Result<(), InstallError> {
    let size = asset.asset_size;
    let mut response = agent
        .get(url)
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
    let mut body = response.body_mut().with_config().limit(size + 1).reader();
    let mut file =
        File::create(dest).map_err(InstallError::io(format!("creating {}", dest.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; READ_BUFFER];
    let mut done = 0u64;
    let mut reported = 0u64;
    loop {
        let read = match body.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(InstallError::Download(format!(
                    "reading {url} after {done} bytes: {e}"
                )));
            }
        };
        done += read as u64;
        if done > size {
            return Err(InstallError::Download(format!(
                "{url} sent more than the pinned {size} bytes"
            )));
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read])
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
fn copy_verified(src: &Path, asset: &PlatformAsset, dest: &Path) -> Result<(), InstallError> {
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
) -> Result<String, InstallError> {
    let parts = relative_components(&asset.entry).ok_or_else(|| {
        InstallError::Extract(format!(
            "entry {:?} is not a plain relative path (absolute paths and .. are rejected)",
            asset.entry
        ))
    })?;
    match asset.archive_kind {
        ArchiveKind::Zip => extract_zip_entry(asset_path, &asset.entry, &parts, staging),
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
    io::copy(&mut file, &mut out).map_err(|e| fail(format!("extracting {entry:?}: {e}")))?;
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
    use std::process::{Command, Stdio};

    let fail = |what: String| InstallError::Extract(format!("{}: {what}", asset_path.display()));
    make_executable(asset_path).map_err(|e| fail(format!("chmod: {e}")))?;
    let mut attempts = 0;
    let output = loop {
        let result = Command::new(asset_path)
            .arg("--appimage-extract")
            .current_dir(staging)
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
            let old = version_dir.with_file_name(format!(".old-{}", random_hex()?));
            rename_with_retry(version_dir, &old).map_err(InstallError::io(format!(
                "moving the earlier install {} aside",
                version_dir.display()
            )))?;
            if let Err(e) = rename_with_retry(staging, version_dir) {
                let _ = fs::rename(&old, version_dir);
                return Err(InstallError::io(context())(e));
            }
            if let Err(e) = remove_tree(&old) {
                log::warn!("cannot remove the earlier install {}: {e}", old.display());
            }
            Ok(())
        }
    }
}

fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    let deadline = std::time::Instant::now().checked_add(RENAME_RETRY);
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e)
                if cfg!(windows)
                    && e.kind() == io::ErrorKind::PermissionDenied
                    && deadline.is_some_and(|d| std::time::Instant::now() < d) =>
            {
                std::thread::sleep(RENAME_INTERVAL);
            }
            Err(e) => return Err(e),
        }
    }
}

/// Removes a directory tree without following symlinks.
fn remove_tree(path: &Path) -> io::Result<()> {
    fs::remove_dir_all(path)
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
    use std::thread::{self, JoinHandle};

    use zip::write::SimpleFileOptions;

    use super::*;
    use crate::contracts::{ModuleInfo, examples};

    const ENTRY_BYTES: &[u8] = b"#!fake ileapp onefile binary\n";
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

    /// Records events and the entry path the introspection callback saw.
    #[derive(Default)]
    struct Run {
        events: Vec<InstallEvent>,
        introspected: Option<PathBuf>,
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
            } = self;
            install(
                pinned,
                source,
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
                };
                let _ = stream.write_all(&bytes.concat());
                let _ = stream.flush();
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
            &mut |_| {},
            &mut |_| Ok(modules(3)),
        )
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::HashMismatch, "{error}");
        asset_mut_linux(&mut case.manifest).entry = "usr/bin/missing".into();
        let error = install(
            linux_pinned(&tools_dir, &case.manifest),
            Source::File(&src),
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
