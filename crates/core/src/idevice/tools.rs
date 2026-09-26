//! Locating and verifying the four libimobiledevice tools (ARCHITECTURE.md D22, CONTRACTS.md
//! §13.1-13.2).
//!
//! **Lookup order:** the dev override (debug builds only) → the bundled sidecar directory → `PATH`.
//! `PATH` is searched on the Linux `system_platforms` always, and on macOS and Windows only in debug
//! builds. All four tools must come from the same place.
//!
//! **Verification** (`ToolVerification`): bundled tools must match the pinned unsigned hashes in
//! `idevice-tools.json` (`manifest`). A code-signed build's tools no longer match them: on macOS they
//! must pass `codesign --verify --strict` with a requirement for a Developer ID signature of the
//! app's own team (`code_signature`), so an ad-hoc or foreign-signed replacement fails; on Windows
//! their hashes are recorded only (`recorded_only`), as for Linux system tools. The dev override is
//! not verified (`none`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::contracts::{
    AcqTools, ContractError, IdeviceBinaries, IdeviceToolSource, IdeviceToolsManifest,
    IdeviceToolsState, IdeviceToolsStatus, PlatformKey, ToolBinary, ToolBundle, ToolVerification,
    parse_versioned,
};
use crate::hashing;

/// The four tools, in the order of [`IdeviceBinaries`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolName {
    IdeviceId,
    Ideviceinfo,
    Idevicepair,
    Idevicebackup2,
}

impl ToolName {
    pub const ALL: [ToolName; 4] = [
        ToolName::IdeviceId,
        ToolName::Ideviceinfo,
        ToolName::Idevicepair,
        ToolName::Idevicebackup2,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            ToolName::IdeviceId => "idevice_id",
            ToolName::Ideviceinfo => "ideviceinfo",
            ToolName::Idevicepair => "idevicepair",
            ToolName::Idevicebackup2 => "idevicebackup2",
        }
    }

    const fn index(self) -> usize {
        match self {
            ToolName::IdeviceId => 0,
            ToolName::Ideviceinfo => 1,
            ToolName::Idevicepair => 2,
            ToolName::Idevicebackup2 => 3,
        }
    }
}

/// `idevice-tools.json` as embedded at build time.
pub fn embedded_manifest() -> Result<IdeviceToolsManifest, ContractError> {
    parse_versioned(include_bytes!("../../../../idevice-tools.json"))
}

/// Where to look for the tools and how to verify them. The shell fills it in; tests inject the
/// manifest, the directories and `PATH`.
#[derive(Clone, Debug)]
pub struct ToolLookup {
    /// This machine's platform; `None` when it has no [`PlatformKey`].
    pub platform: Option<PlatformKey>,
    /// `idevice-tools.json`: [`embedded_manifest`] in the app.
    pub manifest: IdeviceToolsManifest,
    /// The directory of the bundled sidecar tools (next to the app executable in release builds).
    /// Both `<tool>[.exe]` and the sidecar source name `<tool>-<target triple>[.exe]` (as in
    /// `src-tauri/binaries/`) are recognized.
    pub bundled_dir: Option<PathBuf>,
    /// `SUITEDFIR_DEV_IDEVICE_OVERRIDE`: fake-idevice for all four tools. Ignored in release builds.
    pub dev_override: Option<PathBuf>,
    /// The `PATH` to search.
    pub path_var: Option<OsString>,
    /// The app is a code-signed build, so bundled tools are verified by signature (macOS) or
    /// recorded only (Windows) instead of against the unsigned pins.
    pub signed_build: bool,
    /// The Apple Developer ID team (10 characters) that signed a signed macOS build. Bundled tools
    /// that no longer match the pins must carry a Developer ID signature of this team; a signed
    /// macOS build without one cannot verify them.
    pub signing_team_id: Option<String>,
}

/// One located tool: the program, the arguments that select it (the dev override's fake-idevice
/// takes the tool name first), and what was observed about the binary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tool {
    pub program: PathBuf,
    pub prefix: Vec<String>,
    pub binary: ToolBinary,
}

/// The four located and verified tools.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdeviceTools {
    pub source: IdeviceToolSource,
    /// The pinned version for bundled tools; `None` for system tools and the dev override.
    pub version: Option<String>,
    tools: [Tool; 4],
    /// Variables added to every tool's environment (tests point fake-idevice at its scenario).
    pub env: Vec<(OsString, OsString)>,
}

impl IdeviceTools {
    pub fn tool(&self, name: ToolName) -> &Tool {
        &self.tools[name.index()]
    }

    /// The `tools` block of `acquisition.json` and `encryption-restore.json`.
    pub fn record(&self) -> AcqTools {
        let binary = |name| self.tool(name).binary.clone();
        AcqTools {
            version: self.version.clone(),
            source: self.source,
            binaries: IdeviceBinaries {
                idevice_id: binary(ToolName::IdeviceId),
                ideviceinfo: binary(ToolName::Ideviceinfo),
                idevicepair: binary(ToolName::Idevicepair),
                idevicebackup2: binary(ToolName::Idevicebackup2),
            },
        }
    }

    /// The full argv of `name` with `args`, as recorded (never holds a password: passwords travel
    /// only via env).
    pub fn argv(&self, name: ToolName, args: &[String]) -> Vec<String> {
        let tool = self.tool(name);
        std::iter::once(tool.program.to_string_lossy().into_owned())
            .chain(tool.prefix.iter().cloned())
            .chain(args.iter().cloned())
            .collect()
    }

    /// The arguments to spawn `name` with (`argv` without the program).
    pub fn args(&self, name: ToolName, args: &[String]) -> Vec<OsString> {
        self.tool(name)
            .prefix
            .iter()
            .chain(args)
            .map(OsString::from)
            .collect()
    }
}

/// Why the tools cannot be used (reported in `devices_list`'s `tools`, never thrown there).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolsProblem {
    /// `missing`, `verification_failed` or `unsupported_platform`.
    pub state: IdeviceToolsState,
    pub source: Option<IdeviceToolSource>,
    pub version: Option<String>,
    /// Technical detail (paths, hashes); never secrets.
    pub detail: String,
    /// What the examiner can do about it.
    pub guidance: String,
}

impl ToolsProblem {
    /// The `tools` block of a `devices_list` result.
    pub fn status(&self) -> IdeviceToolsStatus {
        IdeviceToolsStatus {
            source: self.source,
            version: self.version.clone(),
            state: self.state,
            guidance: Some(self.guidance.clone()),
        }
    }
}

impl std::fmt::Display for ToolsProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

/// Guidance when the tools are missing.
fn missing_guidance(platform: PlatformKey) -> String {
    match platform {
        PlatformKey::LinuxX86_64 | PlatformKey::LinuxAarch64 => {
            "Install the libimobiledevice tools and the usbmuxd service, then start usbmuxd: \
             `sudo apt install libimobiledevice-utils usbmuxd` (Debian, Ubuntu), \
             `sudo dnf install libimobiledevice-utils usbmuxd` (Fedora), \
             `sudo pacman -S libimobiledevice usbmuxd` (Arch)."
                .to_owned()
        }
        _ => "The iOS tools that ship with suiteDFIR are missing from this installation. \
              Reinstall suiteDFIR."
            .to_owned(),
    }
}

const VERIFICATION_GUIDANCE: &str = "The iOS tools that ship with suiteDFIR do not match the \
     pinned builds, so they may have been modified. Reinstall suiteDFIR from a trusted download.";

const UNSUPPORTED_GUIDANCE: &str = "iOS acquisition is not available on this platform: there is \
     no pinned build of the libimobiledevice tools for it.";

/// Locates the four tools and verifies them (see the module docs).
pub fn locate(lookup: &ToolLookup) -> Result<IdeviceTools, ToolsProblem> {
    #[cfg(debug_assertions)]
    if let Some(fake) = &lookup.dev_override {
        return dev_override(fake);
    }
    let unsupported = |detail: String| ToolsProblem {
        state: IdeviceToolsState::UnsupportedPlatform,
        source: None,
        version: None,
        detail,
        guidance: UNSUPPORTED_GUIDANCE.to_owned(),
    };
    let Some(platform) = lookup.platform else {
        return Err(unsupported(
            "this OS and CPU have no platform key".to_owned(),
        ));
    };
    let manifest = &lookup.manifest;
    let system = manifest.system_platforms.contains(&platform);
    let bundle = manifest.platforms.get(&platform);
    if bundle.is_none() && !system {
        return Err(unsupported(format!(
            "idevice-tools.json has no tool bundle for {platform}"
        )));
    }
    if let (Some(bundle), Some(dir)) = (bundle, &lookup.bundled_dir)
        && let Some(paths) = find_all(|name| sidecar(dir, name, platform))
    {
        let signing = lookup
            .signed_build
            .then_some(lookup.signing_team_id.as_deref());
        return verify_bundled(paths, bundle, platform, signing, &manifest.version);
    }
    if system || cfg!(debug_assertions) {
        let path_var = lookup.path_var.as_deref();
        if let Some(paths) = find_all(|name| on_path(path_var, name, platform)) {
            return system_tools(paths);
        }
    }
    let looked = match (&lookup.bundled_dir, system || cfg!(debug_assertions)) {
        (Some(dir), true) => format!("{} and PATH", dir.display()),
        (Some(dir), false) => dir.display().to_string(),
        (None, true) => "PATH".to_owned(),
        (None, false) => "nowhere (no bundled tools directory)".to_owned(),
    };
    Err(ToolsProblem {
        state: IdeviceToolsState::Missing,
        source: None,
        version: None,
        detail: format!("the libimobiledevice tools were not found in {looked}"),
        guidance: missing_guidance(platform),
    })
}

/// Each tool's path, if all four are found.
fn find_all(mut find: impl FnMut(ToolName) -> Option<PathBuf>) -> Option<[PathBuf; 4]> {
    let [a, b, c, d] = ToolName::ALL.map(&mut find);
    Some([a?, b?, c?, d?])
}

fn exe_suffix(platform: PlatformKey) -> &'static str {
    match platform {
        PlatformKey::WindowsX86_64 | PlatformKey::WindowsAarch64 => ".exe",
        _ => "",
    }
}

/// The Tauri target triple of a platform (the sidecar source names in `src-tauri/binaries/`).
fn target_triple(platform: PlatformKey) -> &'static str {
    match platform {
        PlatformKey::MacosAarch64 => "aarch64-apple-darwin",
        PlatformKey::MacosX86_64 => "x86_64-apple-darwin",
        PlatformKey::WindowsX86_64 => "x86_64-pc-windows-msvc",
        PlatformKey::WindowsAarch64 => "aarch64-pc-windows-msvc",
        PlatformKey::LinuxX86_64 => "x86_64-unknown-linux-gnu",
        PlatformKey::LinuxAarch64 => "aarch64-unknown-linux-gnu",
    }
}

fn sidecar(dir: &Path, name: ToolName, platform: PlatformKey) -> Option<PathBuf> {
    let exe = exe_suffix(platform);
    [
        format!("{}{exe}", name.as_str()),
        format!("{}-{}{exe}", name.as_str(), target_triple(platform)),
    ]
    .into_iter()
    .map(|file| dir.join(file))
    .find(|path| path.is_file())
}

fn on_path(
    path_var: Option<&std::ffi::OsStr>,
    name: ToolName,
    platform: PlatformKey,
) -> Option<PathBuf> {
    let file = format!("{}{}", name.as_str(), exe_suffix(platform));
    std::env::split_paths(path_var?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(&file))
        .find(|path| path.is_file())
}

fn hash(path: &Path) -> Result<String, ToolsProblem> {
    hashing::sha256_file(path).map_err(|e| ToolsProblem {
        state: IdeviceToolsState::VerificationFailed,
        source: None,
        version: None,
        detail: format!("cannot read {}: {e}", path.display()),
        guidance: VERIFICATION_GUIDANCE.to_owned(),
    })
}

fn plain_tools(
    paths: [PathBuf; 4],
    prefix: impl Fn(ToolName) -> Vec<String>,
    mut verify: impl FnMut(ToolName, &Path, &str) -> Result<ToolVerification, ToolsProblem>,
    source: IdeviceToolSource,
    version: Option<String>,
) -> Result<IdeviceTools, ToolsProblem> {
    let mut tools = Vec::with_capacity(4);
    for (name, program) in ToolName::ALL.into_iter().zip(paths) {
        let sha256 = hash(&program).map_err(|mut problem| {
            problem.source = Some(source);
            problem.version.clone_from(&version);
            problem
        })?;
        let verified_against = verify(name, &program, &sha256)?;
        tools.push(Tool {
            binary: ToolBinary {
                path: program.to_string_lossy().into_owned(),
                sha256,
                verified_against,
            },
            prefix: prefix(name),
            program,
        });
    }
    let tools: [Tool; 4] = tools.try_into().map_err(|_| ToolsProblem {
        state: IdeviceToolsState::Missing,
        source: Some(source),
        version: version.clone(),
        detail: "internal error: not four tools".to_owned(),
        guidance: String::new(),
    })?;
    Ok(IdeviceTools {
        source,
        version,
        tools,
        env: Vec::new(),
    })
}

#[cfg(debug_assertions)]
fn dev_override(fake: &Path) -> Result<IdeviceTools, ToolsProblem> {
    if !fake.is_file() {
        return Err(ToolsProblem {
            state: IdeviceToolsState::Missing,
            source: Some(IdeviceToolSource::DevOverride),
            version: None,
            detail: format!("the dev override {} does not exist", fake.display()),
            guidance: "Build fake-idevice (`cargo build -p suitedfir-core`) or unset \
                       SUITEDFIR_DEV_IDEVICE_OVERRIDE."
                .to_owned(),
        });
    }
    plain_tools(
        [(); 4].map(|()| fake.to_path_buf()),
        |name| vec![name.as_str().to_owned()],
        |_, _, _| Ok(ToolVerification::None),
        IdeviceToolSource::DevOverride,
        None,
    )
}

fn system_tools(paths: [PathBuf; 4]) -> Result<IdeviceTools, ToolsProblem> {
    plain_tools(
        paths,
        |_| Vec::new(),
        |_, _, _| Ok(ToolVerification::RecordedOnly),
        IdeviceToolSource::System,
        None,
    )
}

/// `signing`: `None` for an unsigned build, else `Some(the Developer ID team)` (macOS).
fn verify_bundled(
    paths: [PathBuf; 4],
    bundle: &ToolBundle,
    platform: PlatformKey,
    signing: Option<Option<&str>>,
    version: &str,
) -> Result<IdeviceTools, ToolsProblem> {
    let failed = |detail: String| ToolsProblem {
        state: IdeviceToolsState::VerificationFailed,
        source: Some(IdeviceToolSource::Bundled),
        version: Some(version.to_owned()),
        detail,
        guidance: VERIFICATION_GUIDANCE.to_owned(),
    };
    let macos = matches!(
        platform,
        PlatformKey::MacosAarch64 | PlatformKey::MacosX86_64
    );
    plain_tools(
        paths,
        |_| Vec::new(),
        |name, path, sha256| {
            let file = format!("{}{}", name.as_str(), exe_suffix(platform));
            if bundle.files.get(&file).map(String::as_str) == Some(sha256) {
                Ok(ToolVerification::Manifest)
            } else if let Some(team) = signing
                && macos
            {
                let Some(team) = team.filter(|team| is_team_id(team)) else {
                    return Err(failed(format!(
                        "{} does not match the pinned hash, and this signed build names no \
                         valid Developer ID team to check its signature against",
                        path.display()
                    )));
                };
                if codesign_verify(path, team) {
                    Ok(ToolVerification::CodeSignature)
                } else {
                    Err(failed(format!(
                        "{} fails `codesign --verify --strict` with a Developer ID requirement \
                         for team {team}",
                        path.display()
                    )))
                }
            } else if signing.is_some() {
                Ok(ToolVerification::RecordedOnly)
            } else {
                Err(failed(format!(
                    "{} has SHA-256 {sha256}, not the pinned {}",
                    path.display(),
                    bundle.files.get(&file).map_or("(none)", String::as_str)
                )))
            }
        },
        IdeviceToolSource::Bundled,
        Some(version.to_owned()),
    )
}

/// An Apple team identifier: 10 ASCII uppercase letters and digits.
fn is_team_id(team: &str) -> bool {
    team.len() == 10
        && team
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

/// The code requirement for a Developer ID Application signature of `team`: an Apple-anchored
/// chain whose intermediate is the Developer ID CA and whose leaf is a Developer ID Application
/// certificate issued to that team (the designated requirement Xcode generates for such apps).
fn developer_id_requirement(team: &str) -> String {
    format!(
        "anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] and \
         certificate leaf[field.1.2.840.113635.100.6.1.13] and certificate leaf[subject.OU] = \
         \"{team}\""
    )
}

/// `codesign --verify --strict -R=<Developer ID requirement for team> <path>` (macOS signed
/// builds). A plain `--verify` would also accept an ad-hoc or any other valid signature.
fn codesign_verify(path: &Path, team: &str) -> bool {
    codesign_verify_command(path, team)
        .status()
        .is_ok_and(|status| status.success())
}

/// The `codesign` command of [`codesign_verify`]. It exits 0 when the signature is valid and
/// satisfies the requirement, 3 when it is valid but does not satisfy it.
fn codesign_verify_command(path: &Path, team: &str) -> Command {
    let mut command = Command::new("/usr/bin/codesign");
    command
        .args(["--verify", "--strict"])
        .arg(format!("-R={}", developer_id_requirement(team)))
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;
    use std::fs;

    const HOST: PlatformKey = PlatformKey::MacosAarch64;

    /// A tools dir with four small "tools" and a manifest pinning their hashes.
    struct Setup {
        dir: tempfile::TempDir,
        manifest: IdeviceToolsManifest,
    }

    fn setup(platform: PlatformKey) -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let mut files = BTreeMap::new();
        for name in ToolName::ALL {
            let file = format!("{}{}", name.as_str(), exe_suffix(platform));
            let path = dir.path().join(&file);
            fs::write(&path, format!("binary of {}", name.as_str())).unwrap();
            files.insert(file, hashing::sha256_file(&path).unwrap());
        }
        let mut manifest = embedded_manifest().unwrap();
        manifest.platforms.insert(
            platform,
            ToolBundle {
                bundle: "test.zip".to_owned(),
                bundle_sha256: "0".repeat(64),
                files,
            },
        );
        Setup { dir, manifest }
    }

    fn lookup(setup: &Setup, platform: PlatformKey) -> ToolLookup {
        ToolLookup {
            platform: Some(platform),
            manifest: setup.manifest.clone(),
            bundled_dir: Some(setup.dir.path().to_path_buf()),
            dev_override: None,
            path_var: None,
            signed_build: false,
            signing_team_id: None,
        }
    }

    #[test]
    fn the_embedded_manifest_parses() {
        let manifest = embedded_manifest().unwrap();
        assert_eq!(manifest.version, "1.4.0");
        // The build release (FX1) names the bundles; the tools still report the version.
        assert_eq!(manifest.release, "1.4.0-p2");
        for (platform, bundle) in &manifest.platforms {
            assert_eq!(
                bundle.bundle,
                format!("idevice-tools-{}-{platform}.zip", manifest.release)
            );
        }
        assert!(manifest.platforms.contains_key(&PlatformKey::MacosAarch64));
        assert!(
            manifest
                .system_platforms
                .contains(&PlatformKey::LinuxX86_64)
        );
    }

    #[test]
    fn bundled_tools_verify_against_the_manifest() {
        let setup = setup(HOST);
        let tools = locate(&lookup(&setup, HOST)).unwrap();
        assert_eq!(tools.source, IdeviceToolSource::Bundled);
        assert_eq!(tools.version.as_deref(), Some("1.4.0"));
        let record = tools.record();
        assert_eq!(
            record.binaries.idevicebackup2.verified_against,
            ToolVerification::Manifest
        );
        assert_eq!(
            record.binaries.idevice_id.path,
            setup.dir.path().join("idevice_id").to_string_lossy()
        );
        assert_eq!(
            tools.argv(ToolName::Idevicepair, &["hostid".to_owned()]),
            [record.binaries.idevicepair.path.as_str(), "hostid"]
        );
    }

    #[test]
    fn a_tampered_bundled_tool_fails_verification() {
        let setup = setup(HOST);
        fs::write(setup.dir.path().join("ideviceinfo"), "tampered").unwrap();
        let problem = locate(&lookup(&setup, HOST)).unwrap_err();
        assert_eq!(problem.state, IdeviceToolsState::VerificationFailed);
        assert_eq!(problem.source, Some(IdeviceToolSource::Bundled));
        assert!(problem.detail.contains("ideviceinfo"), "{}", problem.detail);
        assert_eq!(
            problem.status().state,
            IdeviceToolsState::VerificationFailed
        );
        assert!(problem.status().guidance.is_some());
    }

    #[test]
    fn signed_windows_builds_record_hashes_only() {
        let platform = PlatformKey::WindowsX86_64;
        let setup = setup(platform);
        fs::write(setup.dir.path().join("idevicepair.exe"), "signed bytes").unwrap();
        let mut lookup = lookup(&setup, platform);
        lookup.signed_build = true;
        let tools = locate(&lookup).unwrap();
        let record = tools.record();
        assert_eq!(
            record.binaries.idevicepair.verified_against,
            ToolVerification::RecordedOnly
        );
        assert_eq!(
            record.binaries.idevice_id.verified_against,
            ToolVerification::Manifest,
            "unchanged files still match"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn signed_macos_builds_need_a_valid_signature() {
        let setup = setup(HOST);
        fs::write(setup.dir.path().join("idevice_id"), "not signed").unwrap();
        let mut lookup = lookup(&setup, HOST);
        lookup.signed_build = true;
        lookup.signing_team_id = Some("ABCDE12345".to_owned());
        let problem = locate(&lookup).unwrap_err();
        assert_eq!(problem.state, IdeviceToolsState::VerificationFailed);
        assert!(problem.detail.contains("codesign"), "{}", problem.detail);
        assert!(problem.detail.contains("ABCDE12345"), "{}", problem.detail);
    }

    /// K7 review N1: a valid ad-hoc signature (which plain `codesign --verify --strict` accepts)
    /// is not a Developer ID signature of the app's team, so it fails.
    #[cfg(target_os = "macos")]
    #[test]
    fn an_ad_hoc_signed_replacement_fails_the_team_requirement() {
        let setup = setup(HOST);
        let tool = setup.dir.path().join("idevicepair");
        fs::copy(std::env::current_exe().unwrap(), &tool).unwrap();
        let codesign = |args: &[&str]| {
            Command::new("/usr/bin/codesign")
                .args(args)
                .arg(&tool)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        };
        assert!(codesign(&["--force", "--sign", "-"]), "ad-hoc signing");
        assert!(
            codesign(&["--verify", "--strict"]),
            "the plain check accepts an ad-hoc signature"
        );
        assert!(!codesign_verify(&tool, "ABCDE12345"));
        // Exit 3: the signature is valid and the requirement well-formed, but not satisfied (a
        // malformed requirement would exit 1).
        let status = codesign_verify_command(&tool, "ABCDE12345")
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(3), "{status:?}");

        let mut lookup = lookup(&setup, HOST);
        lookup.signed_build = true;
        lookup.signing_team_id = Some("ABCDE12345".to_owned());
        let problem = locate(&lookup).unwrap_err();
        assert_eq!(problem.state, IdeviceToolsState::VerificationFailed);
        assert!(problem.detail.contains("idevicepair"), "{}", problem.detail);
    }

    /// A signed macOS build must name a valid team; otherwise changed tools fail without running
    /// codesign at all. Files that still match the pins verify against the manifest.
    #[test]
    fn a_signed_macos_build_without_a_team_fails_closed() {
        let setup = setup(HOST);
        fs::write(setup.dir.path().join("ideviceinfo"), "signed bytes").unwrap();
        for team in [
            None,
            Some("abcde12345"),
            Some("ABCDE1234"),
            Some("ABCDE12345\""),
        ] {
            let mut lookup = lookup(&setup, HOST);
            lookup.signed_build = true;
            lookup.signing_team_id = team.map(str::to_owned);
            let problem = locate(&lookup).unwrap_err();
            assert_eq!(problem.state, IdeviceToolsState::VerificationFailed);
            assert!(
                problem.detail.contains("names no valid Developer ID team"),
                "{team:?}: {}",
                problem.detail
            );
        }
        fs::write(
            setup.dir.path().join("ideviceinfo"),
            "binary of ideviceinfo",
        )
        .unwrap();
        let mut lookup = lookup(&setup, HOST);
        lookup.signed_build = true;
        let tools = locate(&lookup).unwrap();
        assert_eq!(
            tools.record().binaries.ideviceinfo.verified_against,
            ToolVerification::Manifest
        );
    }

    #[test]
    fn the_requirement_names_developer_id_and_the_team() {
        assert!(is_team_id("N2G83326TZ"));
        assert!(!is_team_id("n2g83326tz") && !is_team_id("N2G83326T") && !is_team_id(""));
        let requirement = developer_id_requirement("N2G83326TZ");
        assert!(
            requirement.starts_with("anchor apple generic and "),
            "{requirement}"
        );
        // The Developer ID CA intermediate and the Developer ID Application leaf.
        assert!(requirement.contains("certificate 1[field.1.2.840.113635.100.6.2.6]"));
        assert!(requirement.contains("certificate leaf[field.1.2.840.113635.100.6.1.13]"));
        assert!(requirement.ends_with("certificate leaf[subject.OU] = \"N2G83326TZ\""));
    }

    /// The requirement is valid code-requirement language: `csreq` compiles it (and rejects a
    /// broken one, so the check is not vacuous).
    #[cfg(target_os = "macos")]
    #[test]
    fn the_requirement_compiles() {
        let compiles = |text: &str| {
            Command::new("/usr/bin/csreq")
                .arg(format!("-r={text}"))
                .arg("-t")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        };
        assert!(compiles(&developer_id_requirement("N2G83326TZ")));
        assert!(!compiles(
            "anchor apple generic and certificate leaf[subject.OU"
        ));
    }

    #[test]
    fn sidecar_source_names_are_found() {
        let platform = PlatformKey::WindowsX86_64;
        let setup = setup(platform);
        let from = setup.dir.path().join("idevicebackup2.exe");
        let to = setup
            .dir
            .path()
            .join("idevicebackup2-x86_64-pc-windows-msvc.exe");
        fs::rename(from, &to).unwrap();
        let tools = locate(&lookup(&setup, platform)).unwrap();
        assert_eq!(tools.tool(ToolName::Idevicebackup2).program, to);
    }

    #[test]
    fn missing_and_unsupported() {
        let setup = setup(HOST);
        fs::remove_file(setup.dir.path().join("idevicepair")).unwrap();
        let mut lookup = lookup(&setup, HOST);
        let problem = locate(&lookup).unwrap_err();
        assert_eq!(problem.state, IdeviceToolsState::Missing);
        assert!(
            problem.guidance.contains("Reinstall"),
            "{}",
            problem.guidance
        );

        lookup.platform = Some(PlatformKey::WindowsAarch64);
        let problem = locate(&lookup).unwrap_err();
        assert_eq!(problem.state, IdeviceToolsState::UnsupportedPlatform);
        lookup.platform = None;
        assert_eq!(
            locate(&lookup).unwrap_err().state,
            IdeviceToolsState::UnsupportedPlatform
        );
    }

    #[test]
    fn linux_uses_path_and_records_hashes() {
        let platform = PlatformKey::LinuxX86_64;
        let setup = setup(platform);
        let lookup = ToolLookup {
            platform: Some(platform),
            manifest: embedded_manifest().unwrap(),
            bundled_dir: None,
            dev_override: None,
            path_var: Some(
                std::env::join_paths([Path::new("/nonexistent-dir"), setup.dir.path()]).unwrap(),
            ),
            signed_build: false,
            signing_team_id: None,
        };
        let tools = locate(&lookup).unwrap();
        assert_eq!(tools.source, IdeviceToolSource::System);
        assert_eq!(tools.version, None);
        assert_eq!(
            tools.record().binaries.ideviceinfo.verified_against,
            ToolVerification::RecordedOnly
        );
        let empty = ToolLookup {
            path_var: None,
            ..lookup
        };
        let problem = locate(&empty).unwrap_err();
        assert_eq!(problem.state, IdeviceToolsState::Missing);
        assert!(
            problem.guidance.contains("apt install"),
            "{}",
            problem.guidance
        );
    }

    #[test]
    fn path_is_searched_on_macos_and_windows_only_in_debug_builds() {
        let setup = setup(HOST);
        let lookup = ToolLookup {
            platform: Some(HOST),
            manifest: setup.manifest.clone(),
            bundled_dir: None,
            dev_override: None,
            path_var: Some(setup.dir.path().as_os_str().to_owned()),
            signed_build: false,
            signing_team_id: None,
        };
        let result = locate(&lookup);
        if cfg!(debug_assertions) {
            let tools = result.unwrap();
            assert_eq!(tools.source, IdeviceToolSource::System);
        } else {
            assert_eq!(result.unwrap_err().state, IdeviceToolsState::Missing);
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn the_dev_override_runs_one_binary_for_all_four() {
        let setup = setup(HOST);
        let fake = setup.dir.path().join("idevice_id");
        let mut lookup = lookup(&setup, HOST);
        lookup.dev_override = Some(fake.clone());
        let tools = locate(&lookup).unwrap();
        assert_eq!(tools.source, IdeviceToolSource::DevOverride);
        assert_eq!(tools.version, None);
        let backup = tools.tool(ToolName::Idevicebackup2);
        assert_eq!(backup.program, fake);
        assert_eq!(backup.prefix, ["idevicebackup2"]);
        assert_eq!(backup.binary.verified_against, ToolVerification::None);
        assert_eq!(
            tools.args(ToolName::Idevicebackup2, &["-u".to_owned()]),
            [OsString::from("idevicebackup2"), OsString::from("-u")]
        );
        lookup.dev_override = Some(setup.dir.path().join("missing"));
        assert_eq!(
            locate(&lookup).unwrap_err().state,
            IdeviceToolsState::Missing
        );
    }
}
