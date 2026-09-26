//! `cargo xtask fetch-idevice-tools [--target <triple>]` (ROADMAP X1).
//!
//! Downloads the pinned libimobiledevice tool bundle for a Tauri target from the
//! `idevice-tools-<version>` prerelease, verifies `bundle_sha256` and every file hash in
//! `idevice-tools.json`, and installs the files into `src-tauri/binaries/`: the four tools under
//! their Tauri sidecar names (`<tool>-<target-triple>[.exe]`), any DLLs under their own names.
//!
//! The bundle is downloaded through the release asset's API URL with
//! `Accept: application/octet-stream` and, when one is available, a token: `GH_TOKEN` if set,
//! otherwise the output of `gh auth token` (with `--user $SUITEDFIR_GH_USER` when that is set).
//! Without a token the download is tried anonymously (enough while the repository is public). The
//! token is never printed.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use suitedfir_core::contracts::{IdeviceToolsManifest, PlatformKey, ToolBundle, parse_versioned};
use suitedfir_core::hashing::sha256_file;

/// The repository whose prereleases hold the bundles.
const RELEASE_REPO: &str = "jacobecontreras/suiteDFIR-next";
const API_BASE: &str = "https://api.github.com";
/// The tools suiteDFIR runs (docs/IDEVICE-CLI.md §2).
const TOOLS: [&str; 4] = ["idevice_id", "ideviceinfo", "idevicepair", "idevicebackup2"];
/// Upper bounds for the release metadata and for one bundle (a bundle is a few MB).
const MAX_RELEASE_JSON_BYTES: u64 = 8 << 20;
const MAX_BUNDLE_BYTES: u64 = 128 << 20;
/// Network timeouts: connecting, waiting for the response headers, and reading a whole body.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
const BODY_TIMEOUT: Duration = Duration::from_secs(600);
/// The Tauri targets that have a pinned bundle. Linux uses the distro's tools.
const TARGETS: [(&str, PlatformKey); 3] = [
    ("aarch64-apple-darwin", PlatformKey::MacosAarch64),
    ("x86_64-apple-darwin", PlatformKey::MacosX86_64),
    ("x86_64-pc-windows-msvc", PlatformKey::WindowsX86_64),
];

const USAGE: &str = "usage: cargo xtask fetch-idevice-tools [--target <triple>]";

/// Entry point for `cargo xtask fetch-idevice-tools`.
pub fn run(repo_root: &Path) -> ExitCode {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let triple = match parse_args(&args) {
        Ok(triple) => triple,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match fetch(&triple, repo_root) {
        Ok(summary) => {
            println!("{summary}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("xtask fetch-idevice-tools: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The target triple: `--target <triple>`, or the host's.
fn parse_args(args: &[String]) -> Result<String, String> {
    match args {
        [] => host_triple().map(str::to_owned).ok_or_else(|| {
            "this host has no pinned iOS tool bundle (Linux uses the distro's tools); \
             pass --target for another platform"
                .to_owned()
        }),
        [flag, triple] if flag == "--target" => Ok(triple.clone()),
        _ => Err(format!("unexpected arguments: {}", args.join(" "))),
    }
}

/// The Tauri target triple of this host, if it has a pinned bundle.
fn host_triple() -> Option<&'static str> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("aarch64-apple-darwin")
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some("x86_64-apple-darwin")
    } else if cfg!(all(windows, target_arch = "x86_64", target_env = "msvc")) {
        Some("x86_64-pc-windows-msvc")
    } else {
        None
    }
}

fn platform_for_triple(triple: &str) -> Result<PlatformKey, String> {
    TARGETS
        .iter()
        .find(|(t, _)| *t == triple)
        .map(|(_, platform)| *platform)
        .ok_or_else(|| {
            let known: Vec<_> = TARGETS.iter().map(|(t, _)| *t).collect();
            format!(
                "no pinned iOS tool bundle for target {triple} (bundles exist for {}; \
                 Linux uses the distro's tools)",
                known.join(", ")
            )
        })
}

fn fetch(triple: &str, repo_root: &Path) -> Result<String, String> {
    let platform = platform_for_triple(triple)?;
    let manifest = read_manifest(&repo_root.join("idevice-tools.json"))?;
    let bundle = manifest
        .platforms
        .get(&platform)
        .ok_or_else(|| format!("idevice-tools.json has no bundle for {platform}"))?;
    check_bundle_files(bundle, platform)?;

    let dest_dir = repo_root.join("src-tauri").join("binaries");
    fs::create_dir_all(&dest_dir).map_err(|e| format!("creating {}: {e}", dest_dir.display()))?;
    let zip_path = dest_dir.join(format!("{}.partial", bundle.bundle));
    let result = download_bundle(&manifest.version, bundle, &zip_path)
        .and_then(|()| install(&zip_path, bundle, triple, &dest_dir));
    // The download is only an intermediate; a leftover would be harmless (gitignored, never bundled).
    let _ = fs::remove_file(&zip_path);
    let installed = result?;

    let mut summary = format!(
        "{} verified (sha256 {}); installed into {}:",
        bundle.bundle,
        bundle.bundle_sha256,
        dest_dir.display()
    );
    for path in installed {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        summary.push_str(&format!("\n  {}", name.unwrap_or_default()));
    }
    Ok(summary)
}

pub(crate) fn read_manifest(path: &Path) -> Result<IdeviceToolsManifest, String> {
    let bytes = fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    parse_versioned(&bytes).map_err(|e| e.to_string())
}

/// A name that is safe to join to a directory: not empty, not `.`/`..`, and without path
/// separators or a drive prefix.
pub(crate) fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':'])
}

/// The bundle name must be a plain `.zip` file name (it becomes a path under
/// `src-tauri/binaries/`); every file name must be a plain name, the four tools must be present,
/// and anything else must be a Windows DLL.
pub(crate) fn check_bundle_files(bundle: &ToolBundle, platform: PlatformKey) -> Result<(), String> {
    if !is_plain_name(&bundle.bundle) || !bundle.bundle.ends_with(".zip") {
        return Err(format!(
            "bundle name {:?} is not a plain .zip file name",
            bundle.bundle
        ));
    }
    let windows = matches!(
        platform,
        PlatformKey::WindowsX86_64 | PlatformKey::WindowsAarch64
    );
    let exe = if windows { ".exe" } else { "" };
    for tool in TOOLS {
        let name = format!("{tool}{exe}");
        if !bundle.files.contains_key(&name) {
            return Err(format!("{}: files has no {name}", bundle.bundle));
        }
    }
    for (name, hash) in &bundle.files {
        if !is_plain_name(name) {
            return Err(format!(
                "{}: file name {name:?} is not a plain name",
                bundle.bundle
            ));
        }
        let tool = TOOLS.iter().any(|tool| *name == format!("{tool}{exe}"));
        if !tool && !(windows && name.to_ascii_lowercase().ends_with(".dll")) {
            return Err(format!("{}: unexpected file {name}", bundle.bundle));
        }
        if !is_sha256_hex(hash) {
            return Err(format!("{}: {name} has no valid SHA-256", bundle.bundle));
        }
    }
    if !is_sha256_hex(&bundle.bundle_sha256) {
        return Err(format!("{}: bundle_sha256 is not a SHA-256", bundle.bundle));
    }
    Ok(())
}

pub(crate) fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `GH_TOKEN`, else `gh auth token [--user $SUITEDFIR_GH_USER]`; `None` if neither yields one.
fn github_token() -> Option<String> {
    if let Ok(token) = std::env::var("GH_TOKEN")
        && !token.trim().is_empty()
    {
        return Some(token.trim().to_owned());
    }
    let mut gh = Command::new("gh");
    gh.args(["auth", "token"]);
    if let Ok(user) = std::env::var("SUITEDFIR_GH_USER")
        && !user.is_empty()
    {
        gh.args(["--user", &user]);
    }
    match gh.stdin(Stdio::null()).stderr(Stdio::null()).output() {
        Ok(output) if output.status.success() => {
            let token = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            (!token.is_empty()).then_some(token)
        }
        _ => {
            eprintln!("note: no GH_TOKEN and `gh auth token` failed; trying without a token");
            None
        }
    }
}

/// An HTTPS-only agent with timeouts, for the GitHub API and pinned upstream files.
pub(crate) fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .user_agent("suiteDFIR-xtask")
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(RESPONSE_TIMEOUT))
        .timeout_recv_body(Some(BODY_TIMEOUT))
        .build()
        .into()
}

/// Downloads the bundle's release asset to `dest` and checks its SHA-256 against
/// `bundle_sha256`. A failed check removes `dest`.
pub(crate) fn download_bundle(
    version: &str,
    bundle: &ToolBundle,
    dest: &Path,
) -> Result<(), String> {
    download_release_asset(&format!("idevice-tools-{version}"), &bundle.bundle, dest)?;
    let verified = verify_bundle(dest, bundle);
    if verified.is_err() {
        let _ = fs::remove_file(dest);
    }
    verified
}

/// Downloads the asset `name` of this repository's release `tag` to `dest` through its API URL.
fn download_release_asset(tag: &str, name: &str, dest: &Path) -> Result<(), String> {
    let token = github_token();
    let auth_hint = if token.is_some() {
        " (does the token have read access to the repository?)"
    } else {
        " (set GH_TOKEN, or SUITEDFIR_GH_USER for `gh auth token --user`)"
    };
    let agent = agent();
    let get = |url: &str, accept: &str| {
        let mut request = agent
            .get(url)
            .header("Accept", accept)
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(token) = &token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }
        // ureq drops the Authorization header on redirects (to the signed download URL).
        request
            .call()
            .map_err(|e| format!("GET {url}: {e}{auth_hint}"))
    };

    let release_url = format!("{API_BASE}/repos/{RELEASE_REPO}/releases/tags/{tag}");
    let mut response = get(&release_url, "application/vnd.github+json")?;
    let release: serde_json::Value = serde_json::from_reader(
        response
            .body_mut()
            .with_config()
            .limit(MAX_RELEASE_JSON_BYTES)
            .reader(),
    )
    .map_err(|e| format!("reading release {tag}: {e}"))?;
    let (asset_url, size) =
        select_asset(&release, name).map_err(|e| format!("release {tag}: {e}"))?;

    let mut response = get(&asset_url, "application/octet-stream")?;
    // ureq's limit reader fails the read after `limit` bytes even at end of body, so allow one more
    // byte; copy_exact still rejects anything but exactly `size` bytes.
    let mut reader = response.body_mut().with_config().limit(size + 1).reader();
    let mut file = File::create(dest).map_err(|e| format!("creating {}: {e}", dest.display()))?;
    copy_exact(&mut reader, &mut file, size).map_err(|e| format!("downloading {name}: {e}"))
}

/// The API URL and size of the release asset `name`. The URL must be on the API host (the only
/// host the token is sent to), and the size must be known and within [`MAX_BUNDLE_BYTES`].
fn select_asset(release: &serde_json::Value, name: &str) -> Result<(String, u64), String> {
    let asset = release["assets"]
        .as_array()
        .and_then(|assets| {
            assets
                .iter()
                .find(|asset| asset["name"].as_str() == Some(name))
        })
        .ok_or_else(|| format!("no asset {name}"))?;
    let url = asset["url"]
        .as_str()
        .filter(|url| url.starts_with(&format!("{API_BASE}/")))
        .ok_or_else(|| format!("{name} has no API URL"))?;
    let size = asset["size"]
        .as_u64()
        .filter(|size| *size <= MAX_BUNDLE_BYTES)
        .ok_or_else(|| format!("{name} has no size under the limit"))?;
    Ok((url.to_owned(), size))
}

/// Copies `reader` to `writer` and fails unless exactly `size` bytes arrived.
fn copy_exact(reader: &mut impl Read, writer: &mut impl Write, size: u64) -> Result<(), String> {
    let written = io::copy(reader, writer).map_err(|e| e.to_string())?;
    if written != size {
        return Err(format!("got {written} bytes, the release lists {size}"));
    }
    writer.flush().map_err(|e| e.to_string())
}

fn verify_bundle(zip_path: &Path, bundle: &ToolBundle) -> Result<(), String> {
    let got = sha256_file(zip_path).map_err(|e| format!("hashing {}: {e}", bundle.bundle))?;
    if got != bundle.bundle_sha256 {
        return Err(format!(
            "{}: SHA-256 {got}, idevice-tools.json pins {}",
            bundle.bundle, bundle.bundle_sha256
        ));
    }
    Ok(())
}

/// The name under `src-tauri/binaries/`: `<tool>-<triple>[.exe]` for the tools (Tauri sidecar
/// naming), the bundle name for anything else (DLLs, mapped by `bundle.resources`).
fn installed_name(name: &str, triple: &str) -> String {
    for tool in TOOLS {
        if name == tool {
            return format!("{tool}-{triple}");
        }
        if name == format!("{tool}.exe") {
            return format!("{tool}-{triple}.exe");
        }
    }
    name.to_owned()
}

/// Extracts every file listed in `bundle.files` from the zip and checks its SHA-256, then installs
/// them all or none: files are staged as `<name>.partial`, and only once all of them match are
/// they moved into place, each previous file first set aside as `<name>.previous`. If a move
/// fails, the files already moved are put back and the staged files removed, so a failed fetch
/// leaves `dest_dir` as it was.
fn install(
    zip_path: &Path,
    bundle: &ToolBundle,
    triple: &str,
    dest_dir: &Path,
) -> Result<Vec<PathBuf>, String> {
    install_with(zip_path, bundle, triple, dest_dir, &mut |from, to| {
        fs::rename(from, to)
    })
}

/// [`install`] with the rename function injected, so tests can make a rename fail.
fn install_with(
    zip_path: &Path,
    bundle: &ToolBundle,
    triple: &str,
    dest_dir: &Path,
    rename: &mut dyn FnMut(&Path, &Path) -> io::Result<()>,
) -> Result<Vec<PathBuf>, String> {
    let file = File::open(zip_path).map_err(|e| format!("opening {}: {e}", bundle.bundle))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("reading {}: {e}", bundle.bundle))?;
    let mut staged: Vec<(PathBuf, PathBuf)> = Vec::new();
    if let Err(e) = stage_files(&mut archive, bundle, triple, dest_dir, &mut staged) {
        for (partial, _) in &staged {
            let _ = fs::remove_file(partial);
        }
        return Err(e);
    }

    // (target, the previous file set aside, if there was one)
    let mut moved: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let mut failure = None;
    for (partial, target) in &staged {
        let previous = with_suffix(target, ".previous");
        let had_previous = target.exists();
        if had_previous && let Err(e) = rename(target, &previous) {
            failure = Some(format!("setting aside {}: {e}", target.display()));
            break;
        }
        if let Err(e) = rename(partial, target) {
            // Put the previous file back before rolling back the others.
            if had_previous {
                let _ = rename(&previous, target);
            }
            failure = Some(format!("installing {}: {e}", target.display()));
            break;
        }
        moved.push((target.clone(), had_previous.then_some(previous)));
    }
    if let Some(e) = failure {
        for (target, previous) in moved.iter().rev() {
            match previous {
                Some(previous) => {
                    let _ = rename(previous, target);
                }
                None => {
                    let _ = fs::remove_file(target);
                }
            }
        }
        for (partial, _) in &staged {
            let _ = fs::remove_file(partial);
        }
        return Err(e);
    }
    for (_, previous) in &moved {
        if let Some(previous) = previous {
            let _ = fs::remove_file(previous);
        }
    }
    Ok(moved.into_iter().map(|(target, _)| target).collect())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn stage_files(
    archive: &mut zip::ZipArchive<File>,
    bundle: &ToolBundle,
    triple: &str,
    dest_dir: &Path,
    staged: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), String> {
    for (name, want) in &bundle.files {
        let target = dest_dir.join(installed_name(name, triple));
        let partial = with_suffix(&target, ".partial");
        let mut entry = archive
            .by_name(name)
            .map_err(|e| format!("{}: {name}: {e}", bundle.bundle))?;
        if !entry.is_file() {
            return Err(format!("{}: {name} is not a file", bundle.bundle));
        }
        let mut out =
            File::create(&partial).map_err(|e| format!("creating {}: {e}", partial.display()))?;
        staged.push((partial.clone(), target));
        io::copy(&mut entry, &mut out)
            .map_err(|e| format!("extracting {name} from {}: {e}", bundle.bundle))?;
        drop(out);
        let got = sha256_file(&partial).map_err(|e| format!("hashing {name}: {e}"))?;
        if got != *want {
            return Err(format!(
                "{}: {name} has SHA-256 {got}, idevice-tools.json pins {want}",
                bundle.bundle
            ));
        }
        make_executable(&partial)?;
    }
    Ok(())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("chmod {}: {e}", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Cursor;

    use serde_json::json;
    use zip::write::SimpleFileOptions;

    use super::*;

    fn sha256_of(bytes: &[u8]) -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob");
        fs::write(&path, bytes).unwrap();
        sha256_file(&path).unwrap()
    }

    /// A zip with the given entries, and a bundle pinning `pins` (defaults to the entries' hashes).
    fn make_bundle(
        dir: &Path,
        entries: &[(&str, &[u8])],
        pins: &[(&str, &str)],
    ) -> (PathBuf, ToolBundle) {
        let zip_path = dir.join("bundle.zip");
        let mut writer = zip::ZipWriter::new(File::create(&zip_path).unwrap());
        for (name, contents) in entries {
            writer
                .start_file(*name, SimpleFileOptions::default())
                .unwrap();
            writer.write_all(contents).unwrap();
        }
        writer.finish().unwrap();
        let mut files: BTreeMap<String, String> = entries
            .iter()
            .map(|(name, contents)| ((*name).to_owned(), sha256_of(contents)))
            .collect();
        for (name, hash) in pins {
            files.insert((*name).to_owned(), (*hash).to_owned());
        }
        let bundle = ToolBundle {
            bundle: "idevice-tools-1.4.0-test.zip".into(),
            bundle_sha256: sha256_file(&zip_path).unwrap(),
            files,
        };
        (zip_path, bundle)
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    const ENTRIES: [(&str, &[u8]); 4] = [
        ("idevice_id", b"id"),
        ("ideviceinfo", b"info"),
        ("idevicepair", b"pair"),
        ("idevicebackup2", b"backup"),
    ];

    const MAC_NAMES: [&str; 4] = [
        "idevice_id-aarch64-apple-darwin",
        "idevicebackup2-aarch64-apple-darwin",
        "ideviceinfo-aarch64-apple-darwin",
        "idevicepair-aarch64-apple-darwin",
    ];

    #[test]
    fn committed_manifest_pins_every_bundle() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let manifest = read_manifest(&root.join("idevice-tools.json")).unwrap();
        let platforms: Vec<_> = manifest.platforms.keys().copied().collect();
        assert_eq!(
            platforms,
            [
                PlatformKey::MacosAarch64,
                PlatformKey::MacosX86_64,
                PlatformKey::WindowsX86_64
            ]
        );
        for (platform, bundle) in &manifest.platforms {
            assert_eq!(
                bundle.bundle,
                format!("idevice-tools-{}-{platform}.zip", manifest.version)
            );
            check_bundle_files(bundle, *platform).unwrap();
        }
        for source in &manifest.sources {
            assert!(is_sha256_hex(&source.sha256), "{}", source.name);
        }
    }

    #[test]
    fn triples_map_to_platforms() {
        assert_eq!(
            platform_for_triple("aarch64-apple-darwin").unwrap(),
            PlatformKey::MacosAarch64
        );
        assert_eq!(
            platform_for_triple("x86_64-apple-darwin").unwrap(),
            PlatformKey::MacosX86_64
        );
        assert_eq!(
            platform_for_triple("x86_64-pc-windows-msvc").unwrap(),
            PlatformKey::WindowsX86_64
        );
        let err = platform_for_triple("x86_64-unknown-linux-gnu").unwrap_err();
        assert!(err.contains("distro"), "{err}");
    }

    /// The default target agrees with the core's platform, which is derived independently from the
    /// runtime OS and CPU names: a host with a pinned bundle gets its own triple, any other host
    /// (Linux, Windows arm64) must pass `--target`.
    #[test]
    fn the_default_target_is_this_hosts_platform() {
        let host =
            suitedfir_core::manifest::platform_for(std::env::consts::OS, std::env::consts::ARCH);
        let pinned = host.filter(|platform| TARGETS.iter().any(|(_, p)| p == platform));
        match parse_args(&[]) {
            Ok(triple) => {
                assert_eq!(Some(platform_for_triple(&triple).unwrap()), pinned);
                assert!(triple.starts_with(std::env::consts::ARCH), "{triple}");
            }
            Err(e) => {
                assert_eq!(pinned, None, "{e}");
                assert!(e.contains("--target"), "{e}");
            }
        }
    }

    #[test]
    fn arguments() {
        let args = ["--target".to_owned(), "x86_64-apple-darwin".to_owned()];
        assert_eq!(parse_args(&args).unwrap(), "x86_64-apple-darwin");
        assert!(parse_args(&["--target".to_owned()]).is_err());
        assert!(parse_args(&["--bogus".to_owned(), "x".to_owned()]).is_err());
    }

    #[test]
    fn sidecar_names() {
        assert_eq!(
            installed_name("idevicebackup2", "aarch64-apple-darwin"),
            "idevicebackup2-aarch64-apple-darwin"
        );
        assert_eq!(
            installed_name("idevice_id.exe", "x86_64-pc-windows-msvc"),
            "idevice_id-x86_64-pc-windows-msvc.exe"
        );
        assert_eq!(
            installed_name("libfoo-1.dll", "x86_64-pc-windows-msvc"),
            "libfoo-1.dll"
        );
    }

    #[test]
    fn bundle_file_names_are_checked() {
        let hash = "a".repeat(64);
        let bundle = |names: &[&str]| ToolBundle {
            bundle: "b.zip".into(),
            bundle_sha256: hash.clone(),
            files: names
                .iter()
                .map(|n| ((*n).to_owned(), hash.clone()))
                .collect(),
        };
        let mac = ["idevice_id", "ideviceinfo", "idevicepair", "idevicebackup2"];
        let win = [
            "idevice_id.exe",
            "ideviceinfo.exe",
            "idevicepair.exe",
            "idevicebackup2.exe",
        ];
        check_bundle_files(&bundle(&mac), PlatformKey::MacosAarch64).unwrap();
        check_bundle_files(&bundle(&win), PlatformKey::WindowsX86_64).unwrap();
        let with_dll = [&win[..], &["libx-1.dll"]].concat();
        check_bundle_files(&bundle(&with_dll), PlatformKey::WindowsX86_64).unwrap();

        // A missing tool, the wrong platform's names, stray files and path-like names fail.
        assert!(check_bundle_files(&bundle(&mac[..3]), PlatformKey::MacosAarch64).is_err());
        assert!(check_bundle_files(&bundle(&mac), PlatformKey::WindowsX86_64).is_err());
        for extra in [
            "libx.dylib",
            "../x.dll",
            "sub/x.dll",
            "sub\\x.dll",
            "C:x.dll",
            "..",
        ] {
            let names = [&win[..], &[extra]].concat();
            assert!(
                check_bundle_files(&bundle(&names), PlatformKey::WindowsX86_64).is_err(),
                "{extra}"
            );
        }
        let dll_on_mac = [&mac[..], &["libx-1.dll"]].concat();
        assert!(check_bundle_files(&bundle(&dll_on_mac), PlatformKey::MacosAarch64).is_err());

        let mut bad_hash = bundle(&mac);
        bad_hash.files.insert("idevice_id".into(), "A".repeat(64));
        assert!(check_bundle_files(&bad_hash, PlatformKey::MacosAarch64).is_err());
    }

    #[test]
    fn the_bundle_name_must_be_a_plain_zip_name() {
        let hash = "a".repeat(64);
        let mac = ["idevice_id", "ideviceinfo", "idevicepair", "idevicebackup2"];
        for (name, ok) in [
            ("idevice-tools-1.4.0-macos-aarch64.zip", true),
            ("../x.zip", false),
            ("sub/x.zip", false),
            ("sub\\x.zip", false),
            ("C:x.zip", false),
            ("", false),
            ("..", false),
            ("x.tar.gz", false),
        ] {
            let bundle = ToolBundle {
                bundle: name.into(),
                bundle_sha256: hash.clone(),
                files: mac
                    .iter()
                    .map(|n| ((*n).to_owned(), hash.clone()))
                    .collect(),
            };
            assert_eq!(
                check_bundle_files(&bundle, PlatformKey::MacosAarch64).is_ok(),
                ok,
                "{name:?}"
            );
        }
    }

    fn release(assets: serde_json::Value) -> serde_json::Value {
        json!({ "tag_name": "idevice-tools-1.4.0", "assets": assets })
    }

    #[test]
    fn the_asset_is_selected_by_exact_name() {
        let url = format!("{API_BASE}/repos/{RELEASE_REPO}/releases/assets/2");
        let release = release(json!([
            { "name": "idevice-tools-1.4.0-macos-aarch64.zip.sig", "url": format!("{API_BASE}/x/1"), "size": 1 },
            { "name": "idevice-tools-1.4.0-macos-aarch64.zip", "url": url, "size": 1_839_147 },
            { "name": "idevice-tools-1.4.0-macos-x86_64.zip", "url": format!("{API_BASE}/x/3"), "size": 3 },
        ]));
        assert_eq!(
            select_asset(&release, "idevice-tools-1.4.0-macos-aarch64.zip").unwrap(),
            (url, 1_839_147)
        );
        let err = select_asset(&release, "idevice-tools-1.4.0-windows-x86_64.zip").unwrap_err();
        assert!(err.contains("no asset"), "{err}");
        assert!(select_asset(&json!({}), "x.zip").is_err());
    }

    #[test]
    fn only_api_host_urls_are_accepted() {
        let name = "b.zip";
        for url in [
            "https://github.com/jacobecontreras/suiteDFIR-next/releases/download/t/b.zip",
            "https://api.github.com.evil.example/repos/x",
            "http://api.github.com/repos/x",
            "https://objects.githubusercontent.com/x",
        ] {
            let release = release(json!([{ "name": name, "url": url, "size": 10 }]));
            let err = select_asset(&release, name).unwrap_err();
            assert!(err.contains("API URL"), "{url}: {err}");
        }
        let release = release(json!([{ "name": name, "size": 10 }]));
        assert!(select_asset(&release, name).is_err(), "a missing URL");
    }

    #[test]
    fn the_asset_size_must_be_known_and_bounded() {
        let name = "b.zip";
        let url = format!("{API_BASE}/repos/x/releases/assets/1");
        let ok = release(json!([{ "name": name, "url": url, "size": MAX_BUNDLE_BYTES }]));
        assert_eq!(select_asset(&ok, name).unwrap().1, MAX_BUNDLE_BYTES);
        for size in [
            json!(MAX_BUNDLE_BYTES + 1),
            json!(null),
            json!(-1),
            json!("10"),
        ] {
            let release = release(json!([{ "name": name, "url": url, "size": size }]));
            let err = select_asset(&release, name).unwrap_err();
            assert!(err.contains("size"), "{size}: {err}");
        }
    }

    #[test]
    fn a_download_must_have_exactly_the_listed_size() {
        let mut out = Vec::new();
        copy_exact(&mut Cursor::new(b"12345".to_vec()), &mut out, 5).unwrap();
        assert_eq!(out, b"12345");

        let err = copy_exact(&mut Cursor::new(b"1234".to_vec()), &mut Vec::new(), 5).unwrap_err();
        assert!(
            err.contains("got 4 bytes") && err.contains("lists 5"),
            "{err}"
        );
        // The download reader allows one byte beyond the size, which must still fail.
        let err = copy_exact(&mut Cursor::new(b"123456".to_vec()), &mut Vec::new(), 5).unwrap_err();
        assert!(err.contains("got 6 bytes"), "{err}");
    }

    #[test]
    fn install_extracts_and_renames_verified_files() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let entries = [&ENTRIES[..], &[("COPYING", b"license" as &[u8])]].concat();
        let (zip_path, mut bundle) = make_bundle(src.path(), &entries, &[]);
        // Only the files listed in `files` are installed (notices stay in the zip).
        bundle.files.remove("COPYING");
        verify_bundle(&zip_path, &bundle).unwrap();
        // An earlier installation is replaced.
        fs::write(dest.path().join(MAC_NAMES[3]), b"old").unwrap();

        let installed = install(&zip_path, &bundle, "aarch64-apple-darwin", dest.path()).unwrap();
        assert_eq!(installed.len(), 4);
        assert_eq!(file_names(dest.path()), MAC_NAMES);
        assert_eq!(
            fs::read(dest.path().join("idevicebackup2-aarch64-apple-darwin")).unwrap(),
            b"backup"
        );
        assert_eq!(fs::read(dest.path().join(MAC_NAMES[3])).unwrap(), b"pair");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dest.path().join("idevice_id-aarch64-apple-darwin"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[test]
    fn a_hash_mismatch_installs_nothing() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let wrong = "0".repeat(64);
        let (zip_path, bundle) = make_bundle(src.path(), &ENTRIES, &[("idevicepair", &wrong)]);
        let previous = dest.path().join("idevicepair-aarch64-apple-darwin");
        fs::write(&previous, b"previous").unwrap();

        let err = install(&zip_path, &bundle, "aarch64-apple-darwin", dest.path()).unwrap_err();
        assert!(err.contains("idevicepair") && err.contains(&wrong), "{err}");
        // No partial files are left, and the previously installed file is untouched.
        assert_eq!(
            file_names(dest.path()),
            ["idevicepair-aarch64-apple-darwin"]
        );
        assert_eq!(fs::read(&previous).unwrap(), b"previous");
    }

    #[test]
    fn a_missing_entry_installs_nothing() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let hash = "b".repeat(64);
        let (zip_path, bundle) =
            make_bundle(src.path(), &ENTRIES[..3], &[("idevicebackup2", &hash)]);
        let err = install(&zip_path, &bundle, "aarch64-apple-darwin", dest.path()).unwrap_err();
        assert!(err.contains("idevicebackup2"), "{err}");
        assert!(file_names(dest.path()).is_empty());
    }

    /// A rename that fails mid-way (for every position of the failing rename, with and without
    /// earlier files present) leaves the directory exactly as it was: earlier files restored, no
    /// `.partial` or `.previous` leftovers.
    #[test]
    fn a_failed_rename_rolls_back_the_whole_install() {
        let src = tempfile::tempdir().unwrap();
        let (zip_path, bundle) = make_bundle(src.path(), &ENTRIES, &[]);
        for with_previous in [false, true] {
            // Four files: one or two renames each (set aside, move into place).
            for fail_at in 0..8 {
                let dest = tempfile::tempdir().unwrap();
                let mut before = Vec::new();
                if with_previous {
                    for (i, name) in MAC_NAMES.iter().enumerate() {
                        let contents = format!("previous {i}");
                        fs::write(dest.path().join(name), &contents).unwrap();
                        before.push(((*name).to_owned(), contents.into_bytes()));
                    }
                }
                let mut calls = 0;
                let mut rename = |from: &Path, to: &Path| {
                    calls += 1;
                    if calls == fail_at + 1 {
                        Err(io::Error::other("injected rename failure"))
                    } else {
                        fs::rename(from, to)
                    }
                };
                let result = install_with(
                    &zip_path,
                    &bundle,
                    "aarch64-apple-darwin",
                    dest.path(),
                    &mut rename,
                );
                let renames = if with_previous { 8 } else { 4 };
                if fail_at >= renames {
                    assert!(result.is_ok(), "{with_previous} {fail_at}");
                    continue;
                }
                let err = result.unwrap_err();
                assert!(err.contains("injected"), "{err}");
                let after: Vec<(String, Vec<u8>)> = file_names(dest.path())
                    .into_iter()
                    .map(|name| {
                        let contents = fs::read(dest.path().join(&name)).unwrap();
                        (name, contents)
                    })
                    .collect();
                assert_eq!(after, before, "previous={with_previous} fail_at={fail_at}");
            }
        }
    }

    #[test]
    fn a_bundle_hash_mismatch_is_rejected() {
        let src = tempfile::tempdir().unwrap();
        let (zip_path, mut bundle) = make_bundle(src.path(), &ENTRIES, &[]);
        bundle.bundle_sha256 = "c".repeat(64);
        let err = verify_bundle(&zip_path, &bundle).unwrap_err();
        assert!(err.contains(&"c".repeat(64)), "{err}");
    }
}
