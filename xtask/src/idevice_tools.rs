//! `cargo xtask fetch-idevice-tools [--target <triple>]` (ROADMAP X1).
//!
//! Downloads the pinned libimobiledevice tool bundle for a Tauri target from the
//! `idevice-tools-<version>` prerelease, verifies `bundle_sha256` and every file hash in
//! `idevice-tools.json`, and installs the files into `src-tauri/binaries/`: the four tools under
//! their Tauri sidecar names (`<tool>-<target-triple>[.exe]`), any DLLs under their own names.
//!
//! While the repository is private, the bundle is downloaded through the release asset's API URL
//! with `Accept: application/octet-stream` and a token: `GH_TOKEN` if set, otherwise the output of
//! `gh auth token` (with `--user $SUITEDFIR_GH_USER` when that is set). Without a token the
//! download is tried anonymously. The token is never printed.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

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
        .and_then(|()| verify_bundle(&zip_path, bundle))
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

fn read_manifest(path: &Path) -> Result<IdeviceToolsManifest, String> {
    let bytes = fs::read(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    parse_versioned(&bytes).map_err(|e| e.to_string())
}

/// Every file name must be a plain name (it becomes a path under `src-tauri/binaries/`), the four
/// tools must be present, and anything else must be a Windows DLL.
fn check_bundle_files(bundle: &ToolBundle, platform: PlatformKey) -> Result<(), String> {
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
        let plain =
            !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':']);
        if !plain {
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

fn is_sha256_hex(s: &str) -> bool {
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

/// Downloads the bundle's release asset to `dest` through its API URL.
fn download_bundle(version: &str, bundle: &ToolBundle, dest: &Path) -> Result<(), String> {
    let token = github_token();
    let auth_hint = if token.is_some() {
        " (does the token have read access to the repository?)"
    } else {
        " (set GH_TOKEN, or SUITEDFIR_GH_USER for `gh auth token --user`)"
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .user_agent("suiteDFIR-xtask")
        .build()
        .into();
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

    let tag = format!("idevice-tools-{version}");
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
    let asset = release["assets"]
        .as_array()
        .and_then(|assets| {
            assets
                .iter()
                .find(|asset| asset["name"].as_str() == Some(bundle.bundle.as_str()))
        })
        .ok_or_else(|| format!("release {tag} has no asset {}", bundle.bundle))?;
    let asset_url = asset["url"]
        .as_str()
        .filter(|url| url.starts_with(&format!("{API_BASE}/")))
        .ok_or_else(|| format!("release {tag}: {} has no API URL", bundle.bundle))?;
    let size = asset["size"]
        .as_u64()
        .filter(|size| *size <= MAX_BUNDLE_BYTES)
        .ok_or_else(|| {
            format!(
                "release {tag}: {} has no size under the limit",
                bundle.bundle
            )
        })?;

    let mut response = get(asset_url, "application/octet-stream")?;
    // ureq's limit reader fails the read after `limit` bytes even at end of body, so allow one more
    // byte; the size check below still rejects anything but exactly `size` bytes.
    let mut reader = response.body_mut().with_config().limit(size + 1).reader();
    let mut file = File::create(dest).map_err(|e| format!("creating {}: {e}", dest.display()))?;
    let written = io::copy(&mut reader, &mut file)
        .map_err(|e| format!("downloading {}: {e}", bundle.bundle))?;
    if written != size {
        return Err(format!(
            "downloading {}: got {written} bytes, the release lists {size}",
            bundle.bundle
        ));
    }
    Ok(())
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

/// Extracts every file listed in `bundle.files` from the zip and checks its SHA-256. Files are
/// written as `<name>.partial` first and renamed only once all of them match, so a failed fetch
/// leaves the previously installed files as they were.
fn install(
    zip_path: &Path,
    bundle: &ToolBundle,
    triple: &str,
    dest_dir: &Path,
) -> Result<Vec<PathBuf>, String> {
    let file = File::open(zip_path).map_err(|e| format!("opening {}: {e}", bundle.bundle))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| format!("reading {}: {e}", bundle.bundle))?;
    let mut staged: Vec<(PathBuf, PathBuf)> = Vec::new();
    let result = stage_files(&mut archive, bundle, triple, dest_dir, &mut staged);
    if let Err(e) = result {
        for (partial, _) in &staged {
            let _ = fs::remove_file(partial);
        }
        return Err(e);
    }
    let mut installed = Vec::new();
    for (partial, target) in staged {
        fs::rename(&partial, &target)
            .map_err(|e| format!("installing {}: {e}", target.display()))?;
        installed.push(target);
    }
    Ok(installed)
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
        let partial = dest_dir.join(format!("{}.partial", installed_name(name, triple)));
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
    use std::io::Write;

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

    #[test]
    fn host_triple_matches_this_build() {
        let expected = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Some("aarch64-apple-darwin")
        } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            Some("x86_64-apple-darwin")
        } else if cfg!(all(windows, target_arch = "x86_64", target_env = "msvc")) {
            Some("x86_64-pc-windows-msvc")
        } else {
            None
        };
        assert_eq!(host_triple(), expected);
        assert_eq!(parse_args(&[]).ok().as_deref(), expected);
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
    fn install_extracts_and_renames_verified_files() {
        let src = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let entries = [&ENTRIES[..], &[("COPYING", b"license" as &[u8])]].concat();
        let (zip_path, mut bundle) = make_bundle(src.path(), &entries, &[]);
        // Only the files listed in `files` are installed (notices stay in the zip).
        bundle.files.remove("COPYING");
        verify_bundle(&zip_path, &bundle).unwrap();

        let installed = install(&zip_path, &bundle, "aarch64-apple-darwin", dest.path()).unwrap();
        assert_eq!(installed.len(), 4);
        assert_eq!(
            file_names(dest.path()),
            [
                "idevice_id-aarch64-apple-darwin",
                "idevicebackup2-aarch64-apple-darwin",
                "ideviceinfo-aarch64-apple-darwin",
                "idevicepair-aarch64-apple-darwin"
            ]
        );
        assert_eq!(
            fs::read(dest.path().join("idevicebackup2-aarch64-apple-darwin")).unwrap(),
            b"backup"
        );
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

    #[test]
    fn a_bundle_hash_mismatch_is_rejected() {
        let src = tempfile::tempdir().unwrap();
        let (zip_path, mut bundle) = make_bundle(src.path(), &ENTRIES, &[]);
        bundle.bundle_sha256 = "c".repeat(64);
        let err = verify_bundle(&zip_path, &bundle).unwrap_err();
        assert!(err.contains(&"c".repeat(64)), "{err}");
    }
}
