//! The pinned LEAPP builds: `leapp-manifest.json` (CONTRACTS.md §3), embedded in the app at build
//! time (ARCHITECTURE.md D6), and this host's [`PlatformKey`].
//!
//! The manifest is maintained by `cargo xtask pin-leapp` (ROADMAP A1). [`embedded`] parses and
//! validates it once; a manifest that fails validation is a build defect, reported as `internal`.

use std::sync::OnceLock;

use crate::contracts::{
    AppError, ArchiveKind, ContractError, ErrorCode, LeappManifest, PlatformAsset, PlatformKey,
    ToolId, ToolManifest, parse_versioned,
};

/// `leapp-manifest.json` from the repository root, as committed.
pub const EMBEDDED_JSON: &str = include_str!("../../../leapp-manifest.json");

/// Why a manifest cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error(transparent)]
    Contract(#[from] ContractError),
    #[error("leapp-manifest.json is invalid: {0}")]
    Invalid(String),
}

impl From<ManifestError> for AppError {
    fn from(error: ManifestError) -> Self {
        AppError {
            code: ErrorCode::Internal,
            message: "The embedded LEAPP tool manifest is invalid.".to_owned(),
            detail: Some(error.to_string()),
        }
    }
}

/// The embedded manifest, parsed and validated on first use.
pub fn embedded() -> Result<&'static LeappManifest, ManifestError> {
    static PARSED: OnceLock<Result<LeappManifest, String>> = OnceLock::new();
    PARSED
        .get_or_init(|| parse(EMBEDDED_JSON.as_bytes()).map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| ManifestError::Invalid(e.clone()))
}

/// Parses a manifest (rejecting an unknown `schema_version` first) and validates it.
pub fn parse(bytes: &[u8]) -> Result<LeappManifest, ManifestError> {
    let manifest: LeappManifest = parse_versioned(bytes)?;
    validate(&manifest)?;
    Ok(manifest)
}

/// Checks the rules of CONTRACTS.md §3 that the types cannot express: every tool is present and
/// each tool entry passes [`validate_tool`].
pub fn validate(manifest: &LeappManifest) -> Result<(), ManifestError> {
    for tool in ToolId::ALL {
        let entry = manifest
            .tools
            .get(tool)
            .ok_or_else(|| ManifestError::Invalid(format!("tool {tool} is missing")))?;
        validate_tool(*tool, entry)?;
    }
    Ok(())
}

/// Checks one tool's entry:
/// - `version` is a plain name (it names the install dir) and `profile_leapp_id` is the tool id
///   (LEAPP matches it in profiles);
/// - `asset_name` is a plain file name, `asset_size` is not zero and hashes are lowercase hex
///   SHA-256;
/// - `entry` is a relative `/`-separated path without `.` or `..` components;
/// - `entry_sha256` is set for `zip` (it may be `null` for `appimage`);
/// - `appimage` assets exist only for Linux platforms (they are extracted with
///   `--appimage-extract`, which only runs on Linux);
/// - there is at least one URL and every URL is `https://`.
pub fn validate_tool(tool: ToolId, entry: &ToolManifest) -> Result<(), ManifestError> {
    let invalid = |what: String| ManifestError::Invalid(format!("{tool}: {what}"));
    if !relative_components(&entry.version).is_some_and(|parts| parts.len() == 1) {
        return Err(invalid(format!(
            "version {:?} is not a plain name",
            entry.version
        )));
    }
    if entry.profile_leapp_id != tool.as_str() {
        return Err(invalid(format!(
            "profile_leapp_id is {:?}, expected {:?}",
            entry.profile_leapp_id,
            tool.as_str()
        )));
    }
    if entry.input_types.is_empty() {
        return Err(invalid("input_types is empty".to_owned()));
    }
    for (platform, asset) in &entry.platforms {
        validate_asset(*platform, asset).map_err(|what| invalid(format!("{platform}: {what}")))?;
    }
    Ok(())
}

fn validate_asset(platform: PlatformKey, asset: &PlatformAsset) -> Result<(), String> {
    let plain_name = relative_components(&asset.asset_name).is_some_and(|parts| parts.len() == 1);
    if !plain_name {
        return Err(format!(
            "asset_name {:?} is not a plain file name",
            asset.asset_name
        ));
    }
    if asset.asset_size == 0 {
        return Err("asset_size is 0".to_owned());
    }
    if !is_sha256_hex(&asset.asset_sha256) {
        return Err("asset_sha256 is not a lowercase hex SHA-256".to_owned());
    }
    if relative_components(&asset.entry).is_none() {
        return Err(format!(
            "entry {:?} is not a relative path inside the archive",
            asset.entry
        ));
    }
    match (&asset.entry_sha256, asset.archive_kind) {
        (Some(hash), _) if !is_sha256_hex(hash) => {
            return Err("entry_sha256 is not a lowercase hex SHA-256".to_owned());
        }
        (None, ArchiveKind::Zip) => return Err("entry_sha256 is required for zip".to_owned()),
        _ => {}
    }
    if asset.archive_kind == ArchiveKind::Appimage && !is_linux(platform) {
        return Err("archive_kind appimage is only valid for Linux platforms".to_owned());
    }
    if asset.urls.is_empty() {
        return Err("urls is empty".to_owned());
    }
    if let Some(url) = asset.urls.iter().find(|url| !url.starts_with("https://")) {
        return Err(format!("url {url:?} is not https://"));
    }
    Ok(())
}

fn is_linux(platform: PlatformKey) -> bool {
    matches!(
        platform,
        PlatformKey::LinuxX86_64 | PlatformKey::LinuxAarch64
    )
}

/// Whether `text` is a lowercase hex SHA-256 (CONTRACTS.md §1).
pub(crate) fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The components of a relative `/`-separated path, or `None` unless every component is a plain
/// name: not empty, `.` or `..`, and without `\`, `:` or NUL (so no absolute, drive-relative or
/// parent-escaping path on any OS).
pub(crate) fn relative_components(path: &str) -> Option<Vec<&str>> {
    let parts: Vec<&str> = path.split('/').collect();
    let plain = |part: &&str| {
        !part.is_empty() && *part != "." && *part != ".." && !part.contains(['\\', ':', '\0'])
    };
    parts.iter().all(plain).then_some(parts)
}

/// The pinned asset of `tool` for `platform`, or `None` if the platform is unsupported: the host
/// has no [`PlatformKey`] (`None`), or the tool has no build for it.
pub fn asset_for(tool: &ToolManifest, platform: Option<PlatformKey>) -> Option<&PlatformAsset> {
    platform.and_then(|platform| tool.platforms.get(&platform))
}

/// This host's platform, from the compile-time target, or `None` for an OS/CPU pair that has no
/// pinned builds.
pub const fn host_platform() -> Option<PlatformKey> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some(PlatformKey::MacosAarch64)
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        Some(PlatformKey::MacosX86_64)
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some(PlatformKey::WindowsX86_64)
    } else if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
        Some(PlatformKey::WindowsAarch64)
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(PlatformKey::LinuxX86_64)
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some(PlatformKey::LinuxAarch64)
    } else {
        None
    }
}

/// The platform for an OS and CPU as spelled by `std::env::consts::{OS, ARCH}`.
pub fn platform_for(os: &str, arch: &str) -> Option<PlatformKey> {
    Some(match (os, arch) {
        ("macos", "aarch64") => PlatformKey::MacosAarch64,
        ("macos", "x86_64") => PlatformKey::MacosX86_64,
        ("windows", "x86_64") => PlatformKey::WindowsX86_64,
        ("windows", "aarch64") => PlatformKey::WindowsAarch64,
        ("linux", "x86_64") => PlatformKey::LinuxX86_64,
        ("linux", "aarch64") => PlatformKey::LinuxAarch64,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{InputType, examples};

    fn valid() -> LeappManifest {
        let manifest = examples::leapp_manifest();
        validate(&manifest).unwrap();
        manifest
    }

    fn ileapp_asset(manifest: &mut LeappManifest) -> &mut PlatformAsset {
        manifest
            .tools
            .get_mut(&ToolId::Ileapp)
            .unwrap()
            .platforms
            .get_mut(&PlatformKey::MacosAarch64)
            .unwrap()
    }

    fn invalid_because(manifest: &LeappManifest) -> String {
        match validate(manifest) {
            Err(ManifestError::Invalid(why)) => why,
            other => panic!("expected an invalid manifest, got {other:?}"),
        }
    }

    #[test]
    fn the_embedded_manifest_pins_both_tools_on_all_six_platforms() {
        let manifest = embedded().unwrap();
        assert_eq!(manifest, &parse(EMBEDDED_JSON.as_bytes()).unwrap());
        for tool in ToolId::ALL {
            let entry = &manifest.tools[tool];
            let platforms: Vec<_> = entry.platforms.keys().copied().collect();
            assert_eq!(platforms, PlatformKey::ALL, "{tool}");
            for (platform, asset) in &entry.platforms {
                let kind = if is_linux(*platform) {
                    ArchiveKind::Appimage
                } else {
                    ArchiveKind::Zip
                };
                assert_eq!(asset.archive_kind, kind, "{tool} {platform}");
                assert!(
                    asset
                        .asset_name
                        .starts_with(&format!("{tool}-{}-", entry.version)),
                    "{}",
                    asset.asset_name
                );
                assert_eq!(
                    asset.urls,
                    [format!(
                        "https://github.com/{}/releases/download/{}/{}",
                        entry.upstream_repo, entry.version, asset.asset_name
                    )]
                );
                // Every entry hash is pinned. The AppImage ones come from the leapp-smoke runs
                // (ROADMAP E3): linux-x86_64 from run 36176500110, linux-aarch64 from run
                // 36193682690, where the builds installed, introspected and ran.
                assert!(asset.entry_sha256.is_some(), "{tool} {platform}");
            }
        }
    }

    #[test]
    fn the_embedded_manifest_matches_the_contract_tool_facts() {
        let manifest = embedded().unwrap();
        let ileapp = &manifest.tools[&ToolId::Ileapp];
        assert_eq!(
            (ileapp.display_name.as_str(), ileapp.version.as_str()),
            ("iLEAPP", "v2026.4.2")
        );
        assert_eq!(ileapp.upstream_repo, "abrignoni/iLEAPP");
        assert_eq!(
            (ileapp.profile_ext.as_str(), ileapp.license.as_str()),
            ("ilprofile", "MIT")
        );
        assert_eq!(ileapp.input_types, InputType::ALL);
        assert!(
            ileapp.supports_timezone && ileapp.supports_keychain && ileapp.supports_itunes_password
        );
        let aleapp = &manifest.tools[&ToolId::Aleapp];
        assert_eq!(
            (aleapp.display_name.as_str(), aleapp.version.as_str()),
            ("aLEAPP", "v2026.4.1")
        );
        assert_eq!(aleapp.upstream_repo, "abrignoni/ALEAPP");
        assert_eq!(aleapp.profile_ext, "alprofile");
        assert_eq!(
            aleapp.input_types,
            [
                InputType::Fs,
                InputType::Tar,
                InputType::Zip,
                InputType::Gz,
                InputType::Raw
            ]
        );
        assert!(
            !aleapp.supports_timezone
                && !aleapp.supports_keychain
                && !aleapp.supports_itunes_password
        );
        // Spot checks against docs/LEAPP-CLI.md §1.
        let asset = &ileapp.platforms[&PlatformKey::MacosAarch64];
        assert_eq!(asset.asset_name, "ileapp-v2026.4.2-macOS_Apple_Silicon.zip");
        assert_eq!(asset.asset_size, 55_085_487);
        assert_eq!(
            asset.asset_sha256,
            "d99f2d05dbde20ee997de477c38443d4f019d60326a8c6b9456058c6cf590386"
        );
        assert_eq!(asset.entry, "ileapp");
        let asset = &aleapp.platforms[&PlatformKey::LinuxX86_64];
        assert_eq!(asset.asset_name, "aleapp-v2026.4.1-Linux_x86_64.AppImage");
        assert_eq!(asset.entry, "usr/bin/aleapp");
        assert_eq!(
            ileapp.platforms[&PlatformKey::WindowsX86_64].entry,
            "ileapp.exe"
        );
    }

    #[test]
    fn parse_rejects_unknown_schema_versions_and_invalid_content() {
        let text = serde_json::to_string(&valid()).unwrap();
        assert!(parse(text.as_bytes()).is_ok());
        let newer = text.replacen("\"schema_version\":1", "\"schema_version\":2", 1);
        assert!(matches!(
            parse(newer.as_bytes()),
            Err(ManifestError::Contract(
                ContractError::UnsupportedSchemaVersion { .. }
            ))
        ));
        let broken = text.replacen("\"https://", "\"http://", 1);
        assert!(matches!(
            parse(broken.as_bytes()),
            Err(ManifestError::Invalid(_))
        ));
        assert!(matches!(
            parse(b"{\"schema_version\":1}"),
            Err(ManifestError::Contract(ContractError::Invalid { .. }))
        ));
    }

    #[test]
    fn validation_rules() {
        let mut manifest = valid();
        manifest.tools.remove(&ToolId::Aleapp);
        assert!(invalid_because(&manifest).contains("aleapp is missing"));

        type Breaker = fn(&mut PlatformAsset);
        let cases: [(&str, Breaker); 10] = [
            ("asset_name", |a| a.asset_name = "dir/ileapp.zip".into()),
            ("asset_name", |a| a.asset_name = "..".into()),
            ("asset_size", |a| a.asset_size = 0),
            ("asset_sha256", |a| a.asset_sha256 = "D".repeat(64)),
            ("entry", |a| a.entry = "../ileapp".into()),
            ("entry", |a| a.entry = "/ileapp".into()),
            ("entry_sha256 is required", |a| a.entry_sha256 = None),
            ("appimage", |a| a.archive_kind = ArchiveKind::Appimage),
            ("urls is empty", |a| a.urls.clear()),
            ("not https", |a| a.urls.push("http://mirror/x.zip".into())),
        ];
        for (expected, break_it) in cases {
            let mut manifest = valid();
            break_it(ileapp_asset(&mut manifest));
            let why = invalid_because(&manifest);
            assert!(why.contains(expected), "{expected}: {why}");
            assert!(why.contains("ileapp: macos-aarch64"), "{why}");
        }

        let mut manifest = valid();
        manifest
            .tools
            .get_mut(&ToolId::Aleapp)
            .unwrap()
            .profile_leapp_id = "ileapp".into();
        assert!(invalid_because(&manifest).contains("profile_leapp_id"));
        for version in ["", "..", "v1/../v2", "v1\\x"] {
            let mut manifest = valid();
            manifest.tools.get_mut(&ToolId::Ileapp).unwrap().version = version.into();
            assert!(invalid_because(&manifest).contains("is not a plain name"));
        }
    }

    #[test]
    fn appimage_entries_may_have_no_hash_yet() {
        let manifest = valid();
        let asset = &manifest.tools[&ToolId::Aleapp].platforms[&PlatformKey::LinuxX86_64];
        assert_eq!(asset.archive_kind, ArchiveKind::Appimage);
        assert_eq!(asset.entry_sha256, None);
    }

    #[test]
    fn relative_components_accept_only_plain_names() {
        assert_eq!(relative_components("ileapp"), Some(vec!["ileapp"]));
        assert_eq!(
            relative_components("usr/bin/ileapp"),
            Some(vec!["usr", "bin", "ileapp"])
        );
        for bad in [
            "",
            "/ileapp",
            "usr//ileapp",
            "usr/bin/",
            "./ileapp",
            "usr/../ileapp",
            "..",
            "C:ileapp",
            "C:/ileapp",
            "usr\\ileapp",
            "\\\\server\\share",
            "nul\0byte",
        ] {
            assert_eq!(relative_components(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn sha256_hex_is_lowercase_and_64_digits() {
        assert!(is_sha256_hex(&"0a".repeat(32)));
        assert!(!is_sha256_hex(&"0A".repeat(32)));
        assert!(!is_sha256_hex(&"0a".repeat(31)));
        assert!(!is_sha256_hex(&"g".repeat(64)));
    }

    #[test]
    fn platform_selection() {
        let expected = [
            ("macos", "aarch64", PlatformKey::MacosAarch64),
            ("macos", "x86_64", PlatformKey::MacosX86_64),
            ("windows", "x86_64", PlatformKey::WindowsX86_64),
            ("windows", "aarch64", PlatformKey::WindowsAarch64),
            ("linux", "x86_64", PlatformKey::LinuxX86_64),
            ("linux", "aarch64", PlatformKey::LinuxAarch64),
        ];
        for (os, arch, platform) in expected {
            assert_eq!(platform_for(os, arch), Some(platform));
        }
        assert_eq!(
            host_platform(),
            platform_for(std::env::consts::OS, std::env::consts::ARCH)
        );
    }

    #[test]
    fn unsupported_platforms() {
        for (os, arch) in [
            ("freebsd", "x86_64"),
            ("linux", "riscv64"),
            ("linux", "x86"),
            ("windows", "x86"),
            ("macos", "powerpc"),
        ] {
            assert_eq!(platform_for(os, arch), None, "{os} {arch}");
        }
        let manifest = valid();
        let ileapp = &manifest.tools[&ToolId::Ileapp];
        // The example pins only macos-aarch64 for iLEAPP.
        assert!(asset_for(ileapp, Some(PlatformKey::MacosAarch64)).is_some());
        assert!(asset_for(ileapp, Some(PlatformKey::WindowsX86_64)).is_none());
        assert!(asset_for(ileapp, None).is_none());
    }

    #[test]
    fn manifest_errors_are_internal_app_errors() {
        let error: AppError = ManifestError::Invalid("x".into()).into();
        assert_eq!(error.code, ErrorCode::Internal);
        assert!(error.detail.unwrap().contains('x'));
    }
}
