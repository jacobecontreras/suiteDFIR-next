//! `cargo xtask pin-leapp --tool <ileapp|aleapp> --tag <tag> [--download-verify] [--download-dir <dir>]`
//! (ROADMAP A1): pins one tool's upstream release in `leapp-manifest.json` (CONTRACTS.md §3).
//!
//! - Reads the release from the public GitHub releases API and refuses drafts and prereleases.
//! - Builds the expected asset name of every platform from [`TEMPLATES`] and fails, listing every
//!   problem, if an expected CLI asset is missing, if a CLI asset matches no platform (extra), or
//!   if several CLI assets name the same platform (ambiguous). CLI assets are those named
//!   `<tool>-…`; other products in the release (the `<tool>GUI-…` builds) are ignored.
//! - Writes the name, size, SHA-256 digest (from the API) and download URL of each asset.
//! - With `--download-verify`, downloads every asset into the download dir (default: a fresh dir
//!   under the OS temp dir; never the repository), checks its size and digest, and for zip assets
//!   extracts the entry and records its SHA-256. Each download is deleted once checked. AppImage
//!   entries are not extracted here, so their `entry_sha256` stays `null` (ROADMAP E3 fills them).
//! - For an asset that is unchanged (same name, size, digest, kind and entry), the existing `urls`
//!   (hand-added mirrors) and `entry_sha256` are kept unless `--download-verify` recomputes the
//!   latter. A zip asset without a known entry hash needs `--download-verify`.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use suitedfir_core::contracts::{
    ArchiveKind, InputType, LeappManifest, PlatformAsset, PlatformKey, ToolId, ToolManifest,
    VersionedFile, parse_versioned,
};
use suitedfir_core::fsutil::write_json_atomic;
use suitedfir_core::hashing::sha256_file;
use suitedfir_core::manifest::validate_tool;

const MANIFEST_FILE: &str = "leapp-manifest.json";
const API_BASE: &str = "https://api.github.com";
const MAX_RELEASE_JSON_BYTES: u64 = 8 << 20;

const USAGE: &str = "usage: cargo xtask pin-leapp --tool <ileapp|aleapp> --tag <tag> \
[--download-verify] [--download-dir <dir>]";

/// The facts of a tool that the release does not carry (CONTRACTS.md §3).
struct ToolFacts {
    tool: ToolId,
    display_name: &'static str,
    upstream_repo: &'static str,
    profile_ext: &'static str,
    input_types: &'static [InputType],
    supports_timezone: bool,
    supports_keychain: bool,
    supports_itunes_password: bool,
}

const TOOLS: [ToolFacts; 2] = [
    ToolFacts {
        tool: ToolId::Ileapp,
        display_name: "iLEAPP",
        upstream_repo: "abrignoni/iLEAPP",
        profile_ext: "ilprofile",
        input_types: InputType::ALL,
        supports_timezone: true,
        supports_keychain: true,
        supports_itunes_password: true,
    },
    ToolFacts {
        tool: ToolId::Aleapp,
        display_name: "aLEAPP",
        upstream_repo: "abrignoni/ALEAPP",
        profile_ext: "alprofile",
        input_types: &[
            InputType::Fs,
            InputType::Tar,
            InputType::Zip,
            InputType::Gz,
            InputType::Raw,
        ],
        supports_timezone: false,
        supports_keychain: false,
        supports_itunes_password: false,
    },
];

/// How one platform's CLI asset is named and packaged (docs/LEAPP-CLI.md §1, §2). The asset is
/// `<tool>-<tag>-<suffix>`; `{tool}` in `entry` is the tool id.
#[derive(Debug)]
struct Template {
    platform: PlatformKey,
    suffix: &'static str,
    kind: ArchiveKind,
    entry: &'static str,
}

const TEMPLATES: [Template; 6] = [
    Template {
        platform: PlatformKey::MacosAarch64,
        suffix: "macOS_Apple_Silicon.zip",
        kind: ArchiveKind::Zip,
        entry: "{tool}",
    },
    Template {
        platform: PlatformKey::MacosX86_64,
        suffix: "macOS_Mac_Intel.zip",
        kind: ArchiveKind::Zip,
        entry: "{tool}",
    },
    Template {
        platform: PlatformKey::WindowsX86_64,
        suffix: "Windows_x86_64.zip",
        kind: ArchiveKind::Zip,
        entry: "{tool}.exe",
    },
    Template {
        platform: PlatformKey::WindowsAarch64,
        suffix: "Windows_arm64.zip",
        kind: ArchiveKind::Zip,
        entry: "{tool}.exe",
    },
    Template {
        platform: PlatformKey::LinuxX86_64,
        suffix: "Linux_x86_64.AppImage",
        kind: ArchiveKind::Appimage,
        entry: "usr/bin/{tool}",
    },
    Template {
        platform: PlatformKey::LinuxAarch64,
        suffix: "Linux_arm64.AppImage",
        kind: ArchiveKind::Appimage,
        entry: "usr/bin/{tool}",
    },
];

impl Template {
    fn asset_name(&self, tool: ToolId, tag: &str) -> String {
        format!("{tool}-{tag}-{}", self.suffix)
    }

    fn entry(&self, tool: ToolId) -> String {
        self.entry.replace("{tool}", tool.as_str())
    }

    /// The platform part of the suffix (without the extension): a second asset containing it is
    /// ambiguous, e.g. `…Windows_x86_64.zip.zip` next to `…Windows_x86_64.zip`.
    fn marker(&self) -> String {
        let marker = self.suffix.split('.').next().unwrap_or(self.suffix);
        marker.to_ascii_lowercase()
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Args {
    tool: ToolId,
    tag: String,
    download_verify: bool,
    download_dir: Option<PathBuf>,
}

/// One asset of a release, as listed by the API.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ReleaseAsset {
    name: String,
    size: u64,
    /// `sha256:<hex>` when GitHub computed it.
    digest: Option<String>,
    url: String,
}

#[derive(Debug)]
struct Release {
    tag: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<ReleaseAsset>,
}

/// Entry point for `cargo xtask pin-leapp`.
pub fn run(repo_root: &Path) -> ExitCode {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let args = match parse_args(&args) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match pin(&args, &repo_root.join(MANIFEST_FILE)) {
        Ok(summary) => {
            println!("{summary}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("xtask pin-leapp: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut tool = None;
    let mut tag = None;
    let mut download_verify = false;
    let mut download_dir = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = || {
            rest.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value"))
        };
        match arg.as_str() {
            "--tool" => {
                let name = value()?;
                let id = ToolId::ALL
                    .iter()
                    .find(|t| t.as_str() == name)
                    .ok_or_else(|| format!("unknown tool {name:?}"))?;
                tool = Some(*id);
            }
            "--tag" => tag = Some(value()?),
            "--download-dir" => download_dir = Some(PathBuf::from(value()?)),
            "--download-verify" => download_verify = true,
            other => return Err(format!("unexpected argument {other:?}")),
        }
    }
    let tag = tag.ok_or("--tag is required")?;
    let safe_tag = !tag.is_empty()
        && tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    if !safe_tag {
        return Err(format!("tag {tag:?} is not a release tag"));
    }
    Ok(Args {
        tool: tool.ok_or("--tool is required")?,
        tag,
        download_verify,
        download_dir,
    })
}

fn facts(tool: ToolId) -> &'static ToolFacts {
    // TOOLS has an entry for every ToolId (checked by a test), so the fallback is never used.
    TOOLS.iter().find(|f| f.tool == tool).unwrap_or(&TOOLS[0])
}

fn pin(args: &Args, manifest_path: &Path) -> Result<String, String> {
    let facts = facts(args.tool);
    let mut manifest = read_manifest(manifest_path)?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .user_agent("suiteDFIR-xtask")
        .build()
        .into();
    let release = fetch_release(&agent, facts.upstream_repo, &args.tag)?;
    check_release(&release, &args.tag)?;
    let selected = select_assets(args.tool, &args.tag, &release.assets)?;

    let (download_dir, created_dir) = match &args.download_dir {
        Some(dir) => (dir.clone(), false),
        None => (
            std::env::temp_dir().join(format!("suitedfir-pin-leapp-{}", std::process::id())),
            true,
        ),
    };
    if args.download_verify {
        fs::create_dir_all(&download_dir)
            .map_err(|e| format!("creating {}: {e}", download_dir.display()))?;
    }
    let existing = manifest.tools.get(&args.tool);
    let mut platforms = Vec::new();
    let mut lines = Vec::new();
    let mut result = Ok(());
    for (template, asset) in selected {
        let downloaded = if args.download_verify {
            download_verify(&agent, asset, template, args.tool, &download_dir)
        } else {
            Ok(None)
        };
        let pinned = downloaded.and_then(|entry_hash| {
            let old = existing.and_then(|tool| tool.platforms.get(&template.platform));
            platform_asset(facts, &args.tag, template, asset, old, entry_hash)
        });
        match pinned {
            Ok(pinned) => {
                lines.push(describe(template.platform, &pinned, args.download_verify));
                platforms.push((template.platform, pinned));
            }
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    if created_dir && args.download_verify {
        let _ = fs::remove_dir_all(&download_dir);
    }
    result?;

    let entry = tool_manifest(facts, &args.tag, platforms.into_iter().collect());
    validate_tool(args.tool, &entry).map_err(|e| e.to_string())?;
    manifest.tools.insert(args.tool, entry);
    write_json_atomic(manifest_path, &manifest)
        .map_err(|e| format!("writing {}: {e}", manifest_path.display()))?;
    Ok(format!(
        "{} {} ({}):\n{}\nwrote {}",
        facts.display_name,
        args.tag,
        facts.upstream_repo,
        lines.join("\n"),
        manifest_path.display()
    ))
}

/// The committed manifest, or an empty one if there is none yet.
fn read_manifest(path: &Path) -> Result<LeappManifest, String> {
    match fs::read(path) {
        Ok(bytes) => parse_versioned(&bytes).map_err(|e| e.to_string()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(LeappManifest {
            schema_version: LeappManifest::SCHEMA_VERSION,
            tools: Default::default(),
        }),
        Err(e) => Err(format!("reading {}: {e}", path.display())),
    }
}

fn fetch_release(agent: &ureq::Agent, repo: &str, tag: &str) -> Result<Release, String> {
    let url = format!("{API_BASE}/repos/{repo}/releases/tags/{tag}");
    let mut response = agent
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(|e| format!("GET {url}: {e}"))?;
    let json: serde_json::Value = serde_json::from_reader(
        response
            .body_mut()
            .with_config()
            .limit(MAX_RELEASE_JSON_BYTES)
            .reader(),
    )
    .map_err(|e| format!("reading {url}: {e}"))?;
    parse_release(&json)
}

fn parse_release(json: &serde_json::Value) -> Result<Release, String> {
    let text = |value: &serde_json::Value, key: &str| {
        value[key]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("the release JSON has no {key}"))
    };
    let flag = |key: &str| {
        json[key]
            .as_bool()
            .ok_or_else(|| format!("the release JSON has no {key}"))
    };
    let mut assets = Vec::new();
    for asset in json["assets"]
        .as_array()
        .ok_or("the release JSON has no assets")?
    {
        assets.push(ReleaseAsset {
            name: text(asset, "name")?,
            size: asset["size"]
                .as_u64()
                .ok_or("an asset in the release JSON has no size")?,
            digest: asset["digest"].as_str().map(str::to_owned),
            url: text(asset, "browser_download_url")?,
        });
    }
    Ok(Release {
        tag: text(json, "tag_name")?,
        draft: flag("draft")?,
        prerelease: flag("prerelease")?,
        assets,
    })
}

fn check_release(release: &Release, tag: &str) -> Result<(), String> {
    if release.tag != tag {
        return Err(format!(
            "the API returned release {} for tag {tag}",
            release.tag
        ));
    }
    if release.draft {
        return Err(format!("release {tag} is a draft; refusing to pin it"));
    }
    if release.prerelease {
        return Err(format!("release {tag} is a prerelease; refusing to pin it"));
    }
    Ok(())
}

/// Matches the release's CLI assets (`<tool>-…`) against [`TEMPLATES`]: one exact match per
/// platform, nothing else. All problems are listed together.
fn select_assets<'a>(
    tool: ToolId,
    tag: &str,
    assets: &'a [ReleaseAsset],
) -> Result<Vec<(&'static Template, &'a ReleaseAsset)>, String> {
    let prefix = format!("{tool}-");
    let cli: Vec<&ReleaseAsset> = assets
        .iter()
        .filter(|a| a.name.to_ascii_lowercase().starts_with(&prefix))
        .collect();
    let mut problems = Vec::new();
    let mut selected = Vec::new();
    let mut accounted: BTreeSet<&str> = BTreeSet::new();
    for template in &TEMPLATES {
        let expected = template.asset_name(tool, tag);
        let marker = template.marker();
        let candidates: Vec<&ReleaseAsset> = cli
            .iter()
            .copied()
            .filter(|a| a.name.to_ascii_lowercase().contains(&marker))
            .collect();
        accounted.extend(candidates.iter().map(|a| a.name.as_str()));
        let names = || {
            candidates
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        match candidates.as_slice() {
            [asset] if asset.name == expected => selected.push((template, *asset)),
            [] => problems.push(format!("missing {expected} ({} asset)", template.platform)),
            [_] => problems.push(format!(
                "missing {expected} ({} asset); found {} instead",
                template.platform,
                names()
            )),
            _ => problems.push(format!(
                "ambiguous {} assets: {}",
                template.platform,
                names()
            )),
        }
    }
    for asset in &cli {
        if !accounted.contains(asset.name.as_str()) {
            problems.push(format!("extra asset {} matches no platform", asset.name));
        }
    }
    if problems.is_empty() {
        Ok(selected)
    } else {
        Err(format!(
            "the {tag} release does not match the asset templates:\n  {}",
            problems.join("\n  ")
        ))
    }
}

/// The hex SHA-256 from an API `digest` (`sha256:<hex>`).
fn digest_sha256(asset: &ReleaseAsset) -> Result<String, String> {
    let hex = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .ok_or_else(|| format!("the release lists no SHA-256 digest for {}", asset.name))?;
    let valid = hex.len() == 64
        && hex
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if !valid {
        return Err(format!("{}: digest {hex:?} is not a SHA-256", asset.name));
    }
    Ok(hex.to_owned())
}

/// The manifest entry of one platform. `entry_hash` is the zip entry's SHA-256 computed by
/// `--download-verify` (`None` otherwise, and always for AppImages).
fn platform_asset(
    facts: &ToolFacts,
    tag: &str,
    template: &Template,
    asset: &ReleaseAsset,
    old: Option<&PlatformAsset>,
    entry_hash: Option<String>,
) -> Result<PlatformAsset, String> {
    let url = format!(
        "https://github.com/{}/releases/download/{tag}/{}",
        facts.upstream_repo, asset.name
    );
    if asset.url != url {
        return Err(format!(
            "{}: the API lists download URL {}, expected {url}",
            asset.name, asset.url
        ));
    }
    let mut pinned = PlatformAsset {
        asset_name: asset.name.clone(),
        asset_size: asset.size,
        asset_sha256: digest_sha256(asset)?,
        archive_kind: template.kind,
        entry: template.entry(facts.tool),
        entry_sha256: None,
        urls: vec![url],
    };
    let unchanged = old.filter(|old| {
        old.asset_name == pinned.asset_name
            && old.asset_size == pinned.asset_size
            && old.asset_sha256 == pinned.asset_sha256
            && old.archive_kind == pinned.archive_kind
            && old.entry == pinned.entry
    });
    if let Some(old) = unchanged {
        pinned.urls.clone_from(&old.urls);
        pinned.entry_sha256.clone_from(&old.entry_sha256);
    }
    if entry_hash.is_some() {
        pinned.entry_sha256 = entry_hash;
    }
    if pinned.archive_kind == ArchiveKind::Zip && pinned.entry_sha256.is_none() {
        return Err(format!(
            "{}: the zip entry's SHA-256 is not known yet; run with --download-verify",
            asset.name
        ));
    }
    Ok(pinned)
}

fn tool_manifest(
    facts: &ToolFacts,
    tag: &str,
    platforms: std::collections::BTreeMap<PlatformKey, PlatformAsset>,
) -> ToolManifest {
    ToolManifest {
        display_name: facts.display_name.to_owned(),
        upstream_repo: facts.upstream_repo.to_owned(),
        version: tag.to_owned(),
        license: "MIT".to_owned(),
        profile_ext: facts.profile_ext.to_owned(),
        profile_leapp_id: facts.tool.as_str().to_owned(),
        input_types: facts.input_types.to_vec(),
        supports_timezone: facts.supports_timezone,
        supports_keychain: facts.supports_keychain,
        supports_itunes_password: facts.supports_itunes_password,
        platforms,
    }
}

fn describe(platform: PlatformKey, asset: &PlatformAsset, downloaded: bool) -> String {
    let entry_hash = asset.entry_sha256.as_deref().unwrap_or("null");
    let how = if downloaded {
        "downloaded and verified"
    } else {
        "from the API"
    };
    format!(
        "  {platform}: {} ({} bytes, sha256 {}; entry {} sha256 {entry_hash}; {how})",
        asset.asset_name, asset.asset_size, asset.asset_sha256, asset.entry
    )
}

/// Downloads `asset` into `dir`, checks its size and digest, and for a zip returns its entry's
/// SHA-256. The download is deleted afterwards.
fn download_verify(
    agent: &ureq::Agent,
    asset: &ReleaseAsset,
    template: &Template,
    tool: ToolId,
    dir: &Path,
) -> Result<Option<String>, String> {
    let path = dir.join(&asset.name);
    let result = download(agent, asset, &path).and_then(|()| {
        let got = sha256_file(&path).map_err(|e| format!("hashing {}: {e}", asset.name))?;
        let want = digest_sha256(asset)?;
        if got != want {
            return Err(format!(
                "{}: downloaded SHA-256 {got}, the release lists {want}",
                asset.name
            ));
        }
        match template.kind {
            ArchiveKind::Zip => zip_entry_sha256(&path, &template.entry(tool), dir).map(Some),
            ArchiveKind::Appimage => Ok(None),
        }
    });
    let _ = fs::remove_file(&path);
    result
}

fn download(agent: &ureq::Agent, asset: &ReleaseAsset, dest: &Path) -> Result<(), String> {
    eprintln!("downloading {} ({} bytes)", asset.name, asset.size);
    let mut response = agent
        .get(&asset.url)
        .call()
        .map_err(|e| format!("GET {}: {e}", asset.url))?;
    // ureq's limit reader fails the read after `limit` bytes even at the end of the body, so allow
    // one more byte; the size check below still rejects anything but exactly `size` bytes.
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(asset.size + 1)
        .reader();
    let mut file = File::create(dest).map_err(|e| format!("creating {}: {e}", dest.display()))?;
    let written =
        io::copy(&mut reader, &mut file).map_err(|e| format!("downloading {}: {e}", asset.name))?;
    if written != asset.size {
        return Err(format!(
            "downloading {}: got {written} bytes, the release lists {}",
            asset.name, asset.size
        ));
    }
    Ok(())
}

/// The SHA-256 of `entry` inside the zip at `zip_path`, extracted to a scratch file in `dir`. The
/// entry must be a regular file.
fn zip_entry_sha256(zip_path: &Path, entry: &str, dir: &Path) -> Result<String, String> {
    let label = zip_path.display();
    let file = File::open(zip_path).map_err(|e| format!("opening {label}: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("reading {label}: {e}"))?;
    let names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    let mut file = archive.by_name(entry).map_err(|_| {
        format!(
            "{label} has no entry {entry:?}; it contains: {}",
            names.join(", ")
        )
    })?;
    if !file.is_file() {
        return Err(format!("{label}: {entry} is not a regular file"));
    }
    let scratch = dir.join(format!(
        "{}.entry",
        zip_path.file_name().unwrap_or_default().to_string_lossy()
    ));
    let result = File::create(&scratch)
        .and_then(|mut out| io::copy(&mut file, &mut out))
        .map_err(|e| format!("extracting {entry} from {label}: {e}"))
        .and_then(|_| {
            sha256_file(&scratch).map_err(|e| format!("hashing {}: {e}", scratch.display()))
        });
    let _ = fs::remove_file(&scratch);
    result
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    use super::*;

    const TAG: &str = "v2026.4.2";

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    /// The v2026.4.2 iLEAPP release assets as listed by the API (docs/LEAPP-CLI.md §1), plus the
    /// GUI builds that the same release carries.
    fn ileapp_assets() -> Vec<ReleaseAsset> {
        let mut assets: Vec<ReleaseAsset> = TEMPLATES
            .iter()
            .enumerate()
            .map(|(i, t)| asset(&t.asset_name(ToolId::Ileapp, TAG), 1000 + i as u64))
            .collect();
        assets.push(asset("ileappGUI-v2026.4.2-macOS_Apple_Silicon.dmg", 7));
        assets.push(asset("ileappGUI-v2026.4.2-Windows_x86_64.zip", 8));
        assets
    }

    fn asset(name: &str, size: u64) -> ReleaseAsset {
        ReleaseAsset {
            name: name.to_owned(),
            size,
            digest: Some(format!("sha256:{}", "ab".repeat(32))),
            url: format!("https://github.com/abrignoni/iLEAPP/releases/download/{TAG}/{name}"),
        }
    }

    #[test]
    fn arguments() {
        let parsed = parse_args(&args(&[
            "--tool",
            "aleapp",
            "--tag",
            "v2026.4.1",
            "--download-verify",
            "--download-dir",
            "/tmp/dl",
        ]))
        .unwrap();
        assert_eq!(
            parsed,
            Args {
                tool: ToolId::Aleapp,
                tag: "v2026.4.1".into(),
                download_verify: true,
                download_dir: Some(PathBuf::from("/tmp/dl")),
            }
        );
        let plain = parse_args(&args(&["--tag", TAG, "--tool", "ileapp"])).unwrap();
        assert!(!plain.download_verify && plain.download_dir.is_none());
        for bad in [
            &["--tool", "ileapp"][..],
            &["--tag", TAG],
            &["--tool", "xleapp", "--tag", TAG],
            &["--tool", "ileapp", "--tag", "../v1"],
            &["--tool", "ileapp", "--tag", ""],
            &["--tool", "ileapp", "--tag", TAG, "--bogus"],
            &["--tool", "ileapp", "--tag"],
        ] {
            assert!(parse_args(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn every_tool_has_facts() {
        for tool in ToolId::ALL {
            assert_eq!(facts(*tool).tool, *tool);
        }
    }

    #[test]
    fn templates_cover_every_platform_once() {
        let platforms: Vec<_> = TEMPLATES.iter().map(|t| t.platform).collect();
        assert_eq!(platforms, PlatformKey::ALL);
        let markers: BTreeSet<_> = TEMPLATES.iter().map(Template::marker).collect();
        assert_eq!(markers.len(), TEMPLATES.len());
        for a in &TEMPLATES {
            for b in &TEMPLATES {
                if a.platform != b.platform {
                    assert!(!b.marker().contains(&a.marker()), "{}", a.suffix);
                }
            }
        }
    }

    #[test]
    fn the_committed_manifest_follows_the_templates() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let manifest = read_manifest(&root.join(MANIFEST_FILE)).unwrap();
        for facts in &TOOLS {
            let entry = &manifest.tools[&facts.tool];
            let expected = tool_manifest(facts, &entry.version, entry.platforms.clone());
            assert_eq!(entry, &expected, "{}", facts.tool);
            for template in &TEMPLATES {
                let asset = &entry.platforms[&template.platform];
                assert_eq!(
                    asset.asset_name,
                    template.asset_name(facts.tool, &entry.version)
                );
                assert_eq!(asset.entry, template.entry(facts.tool));
                assert_eq!(asset.archive_kind, template.kind);
            }
        }
    }

    #[test]
    fn release_checks() {
        let release = |tag: &str, draft, prerelease| Release {
            tag: tag.into(),
            draft,
            prerelease,
            assets: Vec::new(),
        };
        check_release(&release(TAG, false, false), TAG).unwrap();
        assert!(
            check_release(&release(TAG, true, false), TAG)
                .unwrap_err()
                .contains("draft")
        );
        assert!(
            check_release(&release(TAG, false, true), TAG)
                .unwrap_err()
                .contains("prerelease")
        );
        assert!(check_release(&release("v1", false, false), TAG).is_err());
    }

    #[test]
    fn parses_the_api_release_json() {
        let json = serde_json::json!({
            "tag_name": TAG, "draft": false, "prerelease": false,
            "assets": [{
                "name": "ileapp-v2026.4.2-macOS_Apple_Silicon.zip", "size": 55085487,
                "digest": "sha256:d99f2d05dbde20ee997de477c38443d4f019d60326a8c6b9456058c6cf590386",
                "browser_download_url": "https://github.com/abrignoni/iLEAPP/releases/download/v2026.4.2/ileapp-v2026.4.2-macOS_Apple_Silicon.zip"
            }, {
                "name": "old.zip", "size": 1, "digest": null,
                "browser_download_url": "https://github.com/x"
            }]
        });
        let release = parse_release(&json).unwrap();
        assert_eq!(release.tag, TAG);
        assert_eq!(release.assets.len(), 2);
        assert_eq!(
            digest_sha256(&release.assets[0]).unwrap(),
            "d99f2d05dbde20ee997de477c38443d4f019d60326a8c6b9456058c6cf590386"
        );
        assert!(
            digest_sha256(&release.assets[1])
                .unwrap_err()
                .contains("no SHA-256")
        );
        let mut bad = release.assets[0].clone();
        bad.digest = Some("sha1:abc".into());
        assert!(digest_sha256(&bad).is_err());
        bad.digest = Some(format!("sha256:{}", "AB".repeat(32)));
        assert!(digest_sha256(&bad).is_err());
        assert!(parse_release(&serde_json::json!({"tag_name": TAG})).is_err());
    }

    #[test]
    fn selects_one_asset_per_platform_and_ignores_other_products() {
        let assets = ileapp_assets();
        let selected = select_assets(ToolId::Ileapp, TAG, &assets).unwrap();
        let names: Vec<_> = selected.iter().map(|(_, a)| a.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "ileapp-v2026.4.2-macOS_Apple_Silicon.zip",
                "ileapp-v2026.4.2-macOS_Mac_Intel.zip",
                "ileapp-v2026.4.2-Windows_x86_64.zip",
                "ileapp-v2026.4.2-Windows_arm64.zip",
                "ileapp-v2026.4.2-Linux_x86_64.AppImage",
                "ileapp-v2026.4.2-Linux_arm64.AppImage",
            ]
        );
    }

    #[test]
    fn lists_missing_extra_and_ambiguous_assets() {
        let mut assets = ileapp_assets();
        // Missing: no Linux arm64 build. Drift: Windows x64 renamed. Ambiguous: a doubled
        // extension next to the right name. Extra: a platform nobody pinned.
        assets.retain(|a| !a.name.contains("Linux_arm64") && !a.name.contains("Windows_x86_64"));
        assets.push(asset("ileapp-v2026.4.2-Windows_x64.zip", 1));
        assets.push(asset("ileapp-v2026.4.2-macOS_Mac_Intel.zip.zip", 1));
        assets.push(asset("ileapp-v2026.4.2-FreeBSD_x86_64.tar.gz", 1));
        let err = select_assets(ToolId::Ileapp, TAG, &assets).unwrap_err();
        for expected in [
            "missing ileapp-v2026.4.2-Linux_arm64.AppImage (linux-aarch64 asset)",
            "missing ileapp-v2026.4.2-Windows_x86_64.zip (windows-x86_64 asset)",
            "ambiguous macos-x86_64 assets: ileapp-v2026.4.2-macOS_Mac_Intel.zip, \
             ileapp-v2026.4.2-macOS_Mac_Intel.zip.zip",
            "extra asset ileapp-v2026.4.2-Windows_x64.zip matches no platform",
            "extra asset ileapp-v2026.4.2-FreeBSD_x86_64.tar.gz matches no platform",
        ] {
            assert!(err.contains(expected), "{expected}\n{err}");
        }
        assert!(!err.contains("GUI"), "{err}");

        // A name that differs only in case is reported, not silently accepted.
        let mut assets = ileapp_assets();
        assets[0].name = "ileapp-v2026.4.2-macos_apple_silicon.zip".into();
        let err = select_assets(ToolId::Ileapp, TAG, &assets).unwrap_err();
        assert!(err.contains("found ileapp-v2026.4.2-macos_apple_silicon.zip instead"));
    }

    fn pinned(old: Option<&PlatformAsset>, hash: Option<&str>) -> Result<PlatformAsset, String> {
        let assets = ileapp_assets();
        platform_asset(
            facts(ToolId::Ileapp),
            TAG,
            &TEMPLATES[0],
            &assets[0],
            old,
            hash.map(str::to_owned),
        )
    }

    #[test]
    fn a_zip_needs_its_entry_hash() {
        let entry_hash = "cd".repeat(32);
        let asset = pinned(None, Some(&entry_hash)).unwrap();
        assert_eq!(asset.asset_name, "ileapp-v2026.4.2-macOS_Apple_Silicon.zip");
        assert_eq!(asset.asset_size, 1000);
        assert_eq!(asset.asset_sha256, "ab".repeat(32));
        assert_eq!(asset.archive_kind, ArchiveKind::Zip);
        assert_eq!(asset.entry, "ileapp");
        assert_eq!(asset.entry_sha256.as_deref(), Some(entry_hash.as_str()));
        assert_eq!(
            asset.urls,
            [
                "https://github.com/abrignoni/iLEAPP/releases/download/v2026.4.2/ileapp-v2026.4.2-macOS_Apple_Silicon.zip"
            ]
        );
        assert!(
            pinned(None, None)
                .unwrap_err()
                .contains("--download-verify")
        );
    }

    #[test]
    fn hand_edits_survive_for_an_unchanged_asset_only() {
        let mut old = pinned(None, Some(&"cd".repeat(32))).unwrap();
        old.urls.push("https://mirror.example/ileapp.zip".into());
        // Unchanged asset: mirrors and the entry hash are kept without a download.
        let kept = pinned(Some(&old), None).unwrap();
        assert_eq!(kept, old);
        // A recomputed hash wins.
        let recomputed = pinned(Some(&old), Some(&"ef".repeat(32))).unwrap();
        assert_eq!(recomputed.entry_sha256, Some("ef".repeat(32)));
        assert_eq!(recomputed.urls, old.urls);
        // A changed asset starts over.
        let mut changed = old.clone();
        changed.asset_sha256 = "00".repeat(32);
        assert!(pinned(Some(&changed), None).is_err());
        let fresh = pinned(Some(&changed), Some(&"ef".repeat(32))).unwrap();
        assert_eq!(fresh.urls.len(), 1);
    }

    #[test]
    fn appimage_entries_stay_null() {
        let assets = ileapp_assets();
        let linux = &TEMPLATES[4];
        let asset =
            platform_asset(facts(ToolId::Ileapp), TAG, linux, &assets[4], None, None).unwrap();
        assert_eq!(asset.archive_kind, ArchiveKind::Appimage);
        assert_eq!(asset.entry, "usr/bin/ileapp");
        assert_eq!(asset.entry_sha256, None);
    }

    #[test]
    fn an_unexpected_download_url_is_rejected() {
        let mut assets = ileapp_assets();
        assets[0].url = "https://evil.example/ileapp.zip".into();
        let err = platform_asset(
            facts(ToolId::Ileapp),
            TAG,
            &TEMPLATES[0],
            &assets[0],
            None,
            Some("cd".repeat(32)),
        )
        .unwrap_err();
        assert!(err.contains("evil.example"), "{err}");
    }

    #[test]
    fn hashes_the_zip_entry() {
        let dir = tempfile::tempdir().unwrap();
        let zip_path = dir.path().join("a.zip");
        let mut writer = zip::ZipWriter::new(File::create(&zip_path).unwrap());
        writer
            .start_file("ileapp", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"abc").unwrap();
        writer
            .add_directory("dir/", SimpleFileOptions::default())
            .unwrap();
        writer.finish().unwrap();
        assert_eq!(
            zip_entry_sha256(&zip_path, "ileapp", dir.path()).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let missing = zip_entry_sha256(&zip_path, "ileapp.exe", dir.path()).unwrap_err();
        assert!(missing.contains("contains: ileapp, dir/"), "{missing}");
        assert!(zip_entry_sha256(&zip_path, "dir/", dir.path()).is_err());
        // Only the zip is left: the scratch copy of the entry is removed.
        let left: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(left.len(), 1);
    }
}
