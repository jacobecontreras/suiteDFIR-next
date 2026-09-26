//! `cargo xtask notices` (ROADMAP F2): regenerates `THIRD-PARTY-NOTICES.md`, which the app embeds
//! (`licenses_get`) and CI checks for drift.
//!
//! - **Rust crates:** every package compiled into the app (`suitedfir`) on every release target,
//!   exactly as `cargo tree -e normal,no-proc-macro --target <triple>` lists it, except the
//!   workspace's own packages. Proc-macro crates (and what only they use) are left out: they run
//!   in the compiler on the build machine and are not part of the app, and `cargo tree` resolves
//!   their dependencies for the machine it runs on, so including them would make the file depend
//!   on the host (a macOS host adds swift-rs, a Windows host windows-sys). Each crate's license and
//!   registry source come from `cargo metadata`, and its license texts from the files in that
//!   source (`LICENSE*`, `LICENCE*`, `COPYING*`, `NOTICE*`, `UNLICENSE*`, `COPYRIGHT*` and
//!   `LICENSES/`). A crate that is not from crates.io, has no SPDX license or ships no license
//!   file fails the generation.
//! - **libimobiledevice tools:** the notice files inside the published tool bundles (each bundle is
//!   downloaded and checked against its `bundle_sha256` in `idevice-tools.json`; the tool fetch
//!   installs only the tools), plus the source tarball URLs and hashes from `idevice-tools.json`
//!   and the source patches the bundles' `BUILDINFO.json` files list (the same in every bundle).
//! - **iLEAPP and aLEAPP:** their MIT license at the pinned tags, and the license files of the
//!   libraries their builds bundle, each fetched from a pinned URL and checked against a pinned
//!   SHA-256.
//!
//! Downloads are cached in `target/xtask-notices/`, named by their SHA-256 and re-checked on use.
//! The output depends only on the pinned inputs and `Cargo.lock`, never on the machine or the time.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use serde_json::Value;
use suitedfir_core::contracts::{LeappManifest, ToolId, parse_versioned};
use suitedfir_core::hashing::sha256_file;

use crate::idevice_tools;

const OUTPUT: &str = "THIRD-PARTY-NOTICES.md";
/// The app package whose dependency tree ships.
const APP_PACKAGE: &str = "suitedfir";
/// The release targets (ROADMAP F1, S3) and how the notices name them.
const RELEASE_TARGETS: [(&str, &str); 6] = [
    ("aarch64-apple-darwin", "macOS arm64"),
    ("x86_64-apple-darwin", "macOS x64"),
    ("x86_64-pc-windows-msvc", "Windows x64"),
    ("aarch64-pc-windows-msvc", "Windows arm64"),
    ("x86_64-unknown-linux-gnu", "Linux x64"),
    ("aarch64-unknown-linux-gnu", "Linux arm64"),
];
const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";
/// Upper bound for one pinned text file.
const MAX_TEXT_BYTES: u64 = 1 << 20;

/// A file fetched from a pinned URL and checked against a pinned SHA-256.
struct Pinned {
    title: &'static str,
    url: &'static str,
    sha256: &'static str,
}

/// The LEAPP license (`LICENSE`, identical in both repositories), at the tag pinned in
/// `leapp-manifest.json`: `{repo}` and `{tag}` are filled in from the manifest.
const LEAPP_LICENSE_URL: &str = "https://raw.githubusercontent.com/{repo}/{tag}/LICENSE";
const LEAPP_LICENSE_SHA256: &str =
    "b06a819817c119603a135d9f8c40d02142987153be580dc72b28293f2dd17765";

/// The license files of the libraries inside the pinned iLEAPP builds (pillow-heif).
const LEAPP_BUNDLED: [Pinned; 2] = [
    Pinned {
        title: "libheif `COPYING` (v1.23.1)",
        url: "https://raw.githubusercontent.com/strukturag/libheif/v1.23.1/COPYING",
        sha256: "fa81ce652315b013359d6e8e4744335f31a50c7c192907176d3632f78a3b4596",
    },
    Pinned {
        title: "libde265 `COPYING` (v1.0.16; unchanged up to v1.1.3)",
        url: "https://raw.githubusercontent.com/strukturag/libde265/v1.0.16/COPYING",
        sha256: "02cc1585a20677992e0ba578fa692635dc193735f2691dc81de924b51c4e8020",
    },
];

/// License files for crates whose crates.io package ships none: the files at the repository
/// root, at the commit the package was published from (its `.cargo_vcs_info.json`, which the
/// generation checks against the URL). Keyed by crate name and version, so a version bump fails the
/// generation until the entry is updated.
struct Upstream {
    name: &'static str,
    version: &'static str,
    /// `(file name, url, sha256)`.
    files: &'static [(&'static str, &'static str, &'static str)],
}

const OBJC2_LICENSE_SHA256: &str =
    "7f976f7e9cb2d87df7230606feb932c3f21ac0e664045a775b600046ff850c54";
const OBJC2_AT_B4167B5: &[(&str, &str, &str)] = &[(
    "LICENSE.md",
    "https://raw.githubusercontent.com/madsmtm/objc2/b4167b582b2f75f9a1be75495c41b765344fd03c/LICENSE.md",
    OBJC2_LICENSE_SHA256,
)];
const OBJC2_AT_8852B42: &[(&str, &str, &str)] = &[(
    "LICENSE.md",
    "https://raw.githubusercontent.com/madsmtm/objc2/8852b424193ca41602281b3d7540d7c8ed51e49a/LICENSE.md",
    OBJC2_LICENSE_SHA256,
)];
const OBJC2_AT_7B1ABFD: &[(&str, &str, &str)] = &[(
    "LICENSE.md",
    "https://raw.githubusercontent.com/madsmtm/objc2/7b1abfd750a2cacaea71d6a56ecfb83cb7de560b/LICENSE.md",
    OBJC2_LICENSE_SHA256,
)];
const OBJC2_AT_8D214F5: &[(&str, &str, &str)] = &[(
    "LICENSE.md",
    "https://raw.githubusercontent.com/madsmtm/objc2/8d214f5477365ffcbcbb7de058c86ed9a518efb7/LICENSE.md",
    OBJC2_LICENSE_SHA256,
)];
const DLOPEN2_AT_CC80E4A: &[(&str, &str, &str)] = &[(
    "LICENSE",
    "https://raw.githubusercontent.com/OpenByteDev/dlopen2/cc80e4a0a90d499b677fdf7743699b4b3a43a989/LICENSE",
    "39fa265207450e77c62e90c5594a06c085b655d8374c7ced4bf7894b6bd95dd2",
)];
const UNIC_AT_5878605: &[(&str, &str, &str)] = &[
    (
        "COPYRIGHT.md",
        "https://raw.githubusercontent.com/open-i18n/rust-unic/5878605364af97a3358368a6eaef02104af2e016/COPYRIGHT.md",
        "f5c342c49f3ac804f3e8e7bb62a8040a44c50d47bb36902b1abd13f66a1adf8b",
    ),
    (
        "LICENSE-APACHE",
        "https://raw.githubusercontent.com/open-i18n/rust-unic/5878605364af97a3358368a6eaef02104af2e016/LICENSE-APACHE",
        "a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2",
    ),
    (
        "LICENSE-MIT",
        "https://raw.githubusercontent.com/open-i18n/rust-unic/5878605364af97a3358368a6eaef02104af2e016/LICENSE-MIT",
        "23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3",
    ),
];
const UNIC_AT_8A6CE83: &[(&str, &str, &str)] = &[
    (
        "COPYRIGHT.md",
        "https://raw.githubusercontent.com/open-i18n/rust-unic/8a6ce83063d90b91ae2ce59eddb803edd393fca9/COPYRIGHT.md",
        "f5c342c49f3ac804f3e8e7bb62a8040a44c50d47bb36902b1abd13f66a1adf8b",
    ),
    (
        "LICENSE-APACHE",
        "https://raw.githubusercontent.com/open-i18n/rust-unic/8a6ce83063d90b91ae2ce59eddb803edd393fca9/LICENSE-APACHE",
        "a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2",
    ),
    (
        "LICENSE-MIT",
        "https://raw.githubusercontent.com/open-i18n/rust-unic/8a6ce83063d90b91ae2ce59eddb803edd393fca9/LICENSE-MIT",
        "23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3",
    ),
];
const WEBVIEW2_LICENSE_SHA256: &str =
    "0dcf41516e608bbcb6cdc5229feb7b86fe4a643b85e7df251133c93408fdac73";
const WEBVIEW2_AT_B74DC5E: &[(&str, &str, &str)] = &[(
    "LICENSE",
    "https://raw.githubusercontent.com/wravery/webview2-rs/b74dc5e2b394044bea5191052868ce7a106c202c/LICENSE",
    WEBVIEW2_LICENSE_SHA256,
)];

const UPSTREAM: &[Upstream] = &[
    Upstream {
        name: "block2",
        version: "0.6.2",
        files: OBJC2_AT_B4167B5,
    },
    Upstream {
        name: "dispatch2",
        version: "0.3.1",
        files: OBJC2_AT_8852B42,
    },
    Upstream {
        name: "dlopen2",
        version: "0.8.2",
        files: DLOPEN2_AT_CC80E4A,
    },
    Upstream {
        name: "objc2",
        version: "0.6.4",
        files: OBJC2_AT_8852B42,
    },
    Upstream {
        name: "objc2-app-kit",
        version: "0.3.2",
        files: OBJC2_AT_7B1ABFD,
    },
    Upstream {
        name: "objc2-core-foundation",
        version: "0.3.2",
        files: OBJC2_AT_7B1ABFD,
    },
    Upstream {
        name: "objc2-encode",
        version: "4.1.0",
        files: OBJC2_AT_8D214F5,
    },
    Upstream {
        name: "objc2-exception-helper",
        version: "0.1.1",
        files: OBJC2_AT_8D214F5,
    },
    Upstream {
        name: "objc2-foundation",
        version: "0.3.2",
        files: OBJC2_AT_7B1ABFD,
    },
    Upstream {
        name: "objc2-web-kit",
        version: "0.3.2",
        files: OBJC2_AT_7B1ABFD,
    },
    Upstream {
        name: "unic-char-property",
        version: "0.9.0",
        files: UNIC_AT_5878605,
    },
    Upstream {
        name: "unic-char-range",
        version: "0.9.0",
        files: UNIC_AT_5878605,
    },
    Upstream {
        name: "unic-common",
        version: "0.9.0",
        files: UNIC_AT_5878605,
    },
    Upstream {
        name: "unic-ucd-ident",
        version: "0.9.0",
        files: UNIC_AT_8A6CE83,
    },
    Upstream {
        name: "unic-ucd-version",
        version: "0.9.0",
        files: UNIC_AT_5878605,
    },
    Upstream {
        name: "webview2-com",
        version: "0.38.2",
        files: WEBVIEW2_AT_B74DC5E,
    },
    Upstream {
        name: "webview2-com-sys",
        version: "0.38.2",
        files: WEBVIEW2_AT_B74DC5E,
    },
];

/// The standard texts (SPDX license list data, pinned tag) of the licenses that crates without a
/// packaged license file declare. They are added for each such crate, because upstream files are
/// often only statements that point to the standard text.
const SPDX_TEXT_URL: &str =
    "https://raw.githubusercontent.com/spdx/license-list-data/v3.29.0/text/{id}.txt";
const SPDX_TEXTS: [(&str, &str); 3] = [
    (
        "Apache-2.0",
        "074e6e32c86a4c0ef8b3ed25b721ca23aca83df277cd88106ef7177c354615ff",
    ),
    (
        "MIT",
        "b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5",
    ),
    (
        "Zlib",
        "bfb1112d49db5b1daecdfef24bd7e2f3ea0bafb33aa67aa0ab51e2bf8407c03d",
    ),
];

/// Entry point for `cargo xtask notices`.
pub fn run(repo_root: &Path) -> ExitCode {
    if std::env::args().nth(2).is_some() {
        eprintln!("usage: cargo xtask notices");
        return ExitCode::from(2);
    }
    match generate(repo_root) {
        Ok(summary) => {
            println!("{summary}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("xtask notices: {e}");
            ExitCode::FAILURE
        }
    }
}

fn generate(repo_root: &Path) -> Result<String, String> {
    let cache = repo_root.join("target").join("xtask-notices");
    fs::create_dir_all(&cache).map_err(|e| format!("creating {}: {e}", cache.display()))?;

    let crates = shipped_crates(repo_root)?;
    // Entries for crates that no longer ship would go stale unnoticed.
    let unused: Vec<String> = UPSTREAM
        .iter()
        .filter(|u| !crates.contains_key(&(u.name.to_owned(), u.version.to_owned())))
        .map(|u| format!("{} {}", u.name, u.version))
        .collect();
    if !unused.is_empty() {
        return Err(format!(
            "UPSTREAM (xtask/src/notices.rs) has entries for crates the app does not ship: {}",
            unused.join(", ")
        ));
    }
    let mut crate_texts = Vec::new();
    let mut problems = Vec::new();
    for krate in crates.values() {
        match license_texts(krate, &cache) {
            Ok(texts) => crate_texts.push(texts),
            Err(e) => problems.push(e),
        }
    }
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    let idevice = idevice_notices(repo_root, &cache)?;
    let leapp = leapp_notices(repo_root, &cache)?;

    let doc = render(&crates, &crate_texts, &idevice, &leapp);
    let path = repo_root.join(OUTPUT);
    let unchanged = fs::read_to_string(&path).ok().as_deref() == Some(doc.as_str());
    if !unchanged {
        fs::write(&path, &doc).map_err(|e| format!("writing {}: {e}", path.display()))?;
    }
    let per_target: Vec<String> = RELEASE_TARGETS
        .iter()
        .map(|(triple, _)| {
            let n = crates
                .values()
                .filter(|c| c.targets.contains(*triple))
                .count();
            format!("{triple}: {n}")
        })
        .collect();
    Ok(format!(
        "{OUTPUT}: {} ({} bytes); {} crates ({}), {} idevice notice files, {} LEAPP texts",
        if unchanged { "unchanged" } else { "written" },
        doc.len(),
        crates.len(),
        per_target.join(", "),
        idevice.files.len(),
        1 + leapp.bundled.len(),
    ))
}

// ---- Rust crates ----

/// One shipped crates.io package.
#[derive(Debug)]
struct Crate {
    name: String,
    version: String,
    license: String,
    dir: PathBuf,
    /// The release targets it is compiled into.
    targets: BTreeSet<String>,
}

/// `(name, version)` → crate, for every crates.io package the app ships on any release target.
fn shipped_crates(repo_root: &Path) -> Result<BTreeMap<(String, String), Crate>, String> {
    let metadata = cargo_json(
        repo_root,
        &["metadata", "--format-version", "1", "--locked"],
    )?;
    let index = PackageIndex::new(&metadata)?;
    let mut crates: BTreeMap<(String, String), Crate> = BTreeMap::new();
    for (triple, _) in RELEASE_TARGETS {
        let tree = cargo_text(
            repo_root,
            &[
                "tree",
                "--locked",
                "-e",
                "normal,no-proc-macro",
                "--target",
                triple,
                "-p",
                APP_PACKAGE,
                "--prefix",
                "none",
                "--format",
                "{p}",
            ],
        )?;
        for key in tree_packages(&tree) {
            if index.members.contains(&key) {
                continue;
            }
            if !crates.contains_key(&key) {
                let krate = index.krate(&key.0, &key.1)?;
                crates.insert(key.clone(), krate);
            }
            if let Some(krate) = crates.get_mut(&key) {
                krate.targets.insert(triple.to_owned());
            }
        }
    }
    Ok(crates)
}

/// The packages of a `cargo metadata` result by `(name, version)`, and the workspace members.
/// Its resolve graph is not used: it also lists optional dependencies that no build enables.
struct PackageIndex<'a> {
    packages: BTreeMap<(String, String), Vec<&'a Value>>,
    members: BTreeSet<(String, String)>,
}

impl<'a> PackageIndex<'a> {
    fn new(metadata: &'a Value) -> Result<Self, String> {
        let member_ids: BTreeSet<&str> = metadata["workspace_members"]
            .as_array()
            .ok_or("cargo metadata: no workspace_members")?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let mut packages: BTreeMap<(String, String), Vec<&Value>> = BTreeMap::new();
        let mut members = BTreeSet::new();
        for package in metadata["packages"]
            .as_array()
            .ok_or("cargo metadata: no packages")?
        {
            let (Some(name), Some(version)) =
                (package["name"].as_str(), package["version"].as_str())
            else {
                return Err("cargo metadata: a package without name or version".to_owned());
            };
            let key = (name.to_owned(), version.to_owned());
            if package["id"]
                .as_str()
                .is_some_and(|id| member_ids.contains(id))
            {
                members.insert(key.clone());
            }
            packages.entry(key).or_default().push(package);
        }
        Ok(PackageIndex { packages, members })
    }

    /// The crates.io package `name` `version`. Any other source (git, path, another registry) is
    /// an error, as deny.toml allows crates.io only.
    fn krate(&self, name: &str, version: &str) -> Result<Crate, String> {
        let found = self
            .packages
            .get(&(name.to_owned(), version.to_owned()))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let [package] = found else {
            return Err(format!(
                "cargo metadata lists {} packages named {name} {version}",
                found.len()
            ));
        };
        if package["source"].as_str() != Some(CRATES_IO) {
            return Err(format!("{name} {version} does not come from crates.io"));
        }
        let license = package["license"].as_str().ok_or_else(|| {
            format!("{name} {version} declares no SPDX license (license-file only)")
        })?;
        let manifest = package["manifest_path"]
            .as_str()
            .ok_or_else(|| format!("{name} {version}: no manifest_path"))?;
        let dir = Path::new(manifest)
            .parent()
            .ok_or_else(|| format!("{name} {version}: the manifest path has no parent"))?;
        Ok(Crate {
            name: name.to_owned(),
            version: version.to_owned(),
            license: license.to_owned(),
            dir: dir.to_path_buf(),
            targets: BTreeSet::new(),
        })
    }
}

/// `(name, version)` of every line of `cargo tree --prefix none --format {p}` (`name vX.Y.Z`,
/// optionally followed by `(proc-macro)`, `(*)` or, for path packages, the path in parentheses).
fn tree_packages(tree: &str) -> BTreeSet<(String, String)> {
    tree.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let name = words.next()?;
            let version = words.next()?.strip_prefix('v')?;
            Some((name.to_owned(), version.to_owned()))
        })
        .collect()
}

fn cargo() -> Command {
    let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    command.stdin(Stdio::null());
    command
}

fn cargo_text(repo_root: &Path, args: &[&str]) -> Result<String, String> {
    let output = cargo()
        .args(args)
        .current_dir(repo_root)
        .output()
        .map_err(|e| format!("running cargo {}: {e}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "cargo {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("cargo {}: {e}", args.join(" ")))
}

fn cargo_json(repo_root: &Path, args: &[&str]) -> Result<Value, String> {
    let text = cargo_text(repo_root, args)?;
    serde_json::from_str(&text).map_err(|e| format!("cargo {}: {e}", args.join(" ")))
}

/// Whether a file in a crate's root is a license or notice file.
fn is_license_file(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    [
        "LICENSE",
        "LICENCE",
        "COPYING",
        "NOTICE",
        "UNLICENSE",
        "COPYRIGHT",
    ]
    .iter()
    .any(|prefix| upper.starts_with(prefix))
}

/// The license files a crate ships, `(file name relative to the crate root, text)`, sorted. For a
/// crate that ships none: the pinned [`UPSTREAM`] files and the [`SPDX_TEXTS`] of its licenses.
fn license_texts(krate: &Crate, cache: &Path) -> Result<Vec<(String, String)>, String> {
    let texts = packaged_license_texts(krate)?;
    if !texts.is_empty() {
        return Ok(texts);
    }
    let upstream = UPSTREAM
        .iter()
        .find(|u| u.name == krate.name && u.version == krate.version)
        .ok_or_else(|| {
            format!(
                "{} {} ({}) ships no license file in {} and has no entry in UPSTREAM \
                 (xtask/src/notices.rs)",
                krate.name,
                krate.version,
                krate.license,
                krate.dir.display()
            )
        })?;
    let commit = published_commit(&krate.dir)
        .map_err(|e| format!("{} {}: {e}", krate.name, krate.version))?;
    let mut texts = Vec::new();
    for (file, url, sha256) in upstream.files {
        if !url.contains(&format!("/{commit}/")) {
            return Err(format!(
                "{} {} was published from commit {commit}, but its UPSTREAM file is {url}",
                krate.name, krate.version
            ));
        }
        let repo: Vec<&str> = url
            .strip_prefix("https://raw.githubusercontent.com/")
            .unwrap_or(url)
            .splitn(3, '/')
            .take(2)
            .collect();
        let text = normalize(&String::from_utf8_lossy(&fetch_pinned(cache, url, sha256)?));
        texts.push((
            format!("`{file}` of {} at {}", repo.join("/"), &commit[..7]),
            text,
        ));
    }
    for id in spdx_ids(&krate.license) {
        let (_, sha256) = SPDX_TEXTS
            .iter()
            .find(|(known, _)| *known == id)
            .ok_or_else(|| {
                format!(
                    "{} {} ships no license file, and SPDX_TEXTS has no standard {id} text",
                    krate.name, krate.version
                )
            })?;
        let url = SPDX_TEXT_URL.replace("{id}", id);
        let text = normalize(&String::from_utf8_lossy(&fetch_pinned(
            cache, &url, sha256,
        )?));
        texts.push((format!("standard {id} text"), text));
    }
    Ok(texts)
}

/// The git commit a crate package was published from (`.cargo_vcs_info.json`).
fn published_commit(dir: &Path) -> Result<String, String> {
    let path = dir.join(".cargo_vcs_info.json");
    let bytes = fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let info: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    info["git"]["sha1"]
        .as_str()
        .filter(|sha| sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_owned)
        .ok_or_else(|| format!("{} has no git.sha1", path.display()))
}

/// The license identifiers of an SPDX expression (also the old `MIT/Apache-2.0` form), without
/// operators and `WITH` exceptions.
fn spdx_ids(expression: &str) -> Vec<&str> {
    let mut ids = Vec::new();
    let mut after_with = false;
    for token in expression
        .split(|c: char| c.is_whitespace() || c == '/' || c == '(' || c == ')')
        .filter(|t| !t.is_empty())
    {
        match token {
            "OR" | "AND" => {}
            "WITH" => after_with = true,
            _ if after_with => after_with = false,
            id => {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
    }
    ids
}

/// The license files in a crate's package, `(file name relative to the crate root, text)`, sorted.
fn packaged_license_texts(krate: &Crate) -> Result<Vec<(String, String)>, String> {
    let mut texts = Vec::new();
    let read_dir = |dir: &Path| {
        fs::read_dir(dir).map_err(|e| {
            format!(
                "{} {}: reading {}: {e}",
                krate.name,
                krate.version,
                dir.display()
            )
        })
    };
    let mut candidates: Vec<(String, PathBuf)> = Vec::new();
    for entry in read_dir(&krate.dir)? {
        let entry = entry.map_err(|e| format!("{}: {e}", krate.dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if name == "LICENSES" && path.is_dir() {
            for inner in read_dir(&path)? {
                let inner = inner.map_err(|e| format!("{}: {e}", path.display()))?;
                if inner.path().is_file() {
                    let inner_name = inner.file_name().to_string_lossy().into_owned();
                    candidates.push((format!("LICENSES/{inner_name}"), inner.path()));
                }
            }
        } else if is_license_file(&name) && path.is_file() {
            candidates.push((name, path));
        }
    }
    candidates.sort();
    for (name, path) in candidates {
        let bytes = fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        texts.push((
            format!("`{name}`"),
            normalize(&String::from_utf8_lossy(&bytes)),
        ));
    }
    Ok(texts)
}

// ---- pinned downloads ----

/// A cached, hash-checked download: `<cache>/<sha256>`.
fn fetch_pinned(cache: &Path, url: &str, sha256: &str) -> Result<Vec<u8>, String> {
    let path = cache.join(sha256);
    if path.is_file() && sha256_file(&path).ok().as_deref() == Some(sha256) {
        return fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()));
    }
    let agent = idevice_tools::agent();
    let mut response = agent
        .get(url)
        .call()
        .map_err(|e| format!("GET {url}: {e}"))?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .with_config()
        .limit(MAX_TEXT_BYTES)
        .reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("GET {url}: {e}"))?;
    let partial = cache.join(format!("{sha256}.partial"));
    fs::write(&partial, &bytes).map_err(|e| format!("writing {}: {e}", partial.display()))?;
    let got = sha256_file(&partial).map_err(|e| format!("hashing {url}: {e}"))?;
    if got != sha256 {
        let _ = fs::remove_file(&partial);
        return Err(format!("{url}: SHA-256 {got}, pinned {sha256}"));
    }
    fs::rename(&partial, &path).map_err(|e| format!("caching {url}: {e}"))?;
    Ok(bytes)
}

// ---- libimobiledevice ----

/// One notice file of the tool bundles, and the platforms whose bundle has it.
struct BundleFile {
    path: String,
    text: String,
    platforms: Vec<String>,
}

/// A source patch the tool bundles were built with (from their `BUILDINFO.json`).
#[derive(Debug, PartialEq, Eq)]
struct SourcePatch {
    file: String,
    /// The source tarball it applies to.
    applies_to: String,
    sha256: String,
}

struct IdeviceNotices {
    version: String,
    /// The tool build (`idevice-tools-<release>-<platform>.zip`).
    release: String,
    /// `(name, version, url, sha256)` of each source tarball.
    sources: Vec<(String, String, String, String)>,
    /// The same in every bundle (checked).
    patches: Vec<SourcePatch>,
    files: Vec<BundleFile>,
}

fn idevice_notices(repo_root: &Path, cache: &Path) -> Result<IdeviceNotices, String> {
    let manifest = idevice_tools::read_manifest(&repo_root.join("idevice-tools.json"))?;
    let mut files: BTreeMap<String, BundleFile> = BTreeMap::new();
    let mut patches: Option<Vec<SourcePatch>> = None;
    for (platform, bundle) in &manifest.platforms {
        idevice_tools::check_manifest_bundle(&manifest, *platform, bundle)?;
        let zip_path = cache.join(&bundle.bundle_sha256);
        let cached = zip_path.is_file()
            && sha256_file(&zip_path).ok().as_deref() == Some(bundle.bundle_sha256.as_str());
        if !cached {
            let partial = cache.join(format!("{}.partial", bundle.bundle_sha256));
            idevice_tools::download_bundle(&manifest.release, bundle, &partial)?;
            fs::rename(&partial, &zip_path)
                .map_err(|e| format!("caching {}: {e}", bundle.bundle))?;
        }
        let file = fs::File::open(&zip_path)
            .map_err(|e| format!("opening {}: {e}", zip_path.display()))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("reading {}: {e}", bundle.bundle))?;
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|e| format!("{}: {e}", bundle.bundle))?;
            let name = entry.name().to_owned();
            // The tools (installed by fetch-idevice-tools) are not notices.
            if !entry.is_file() || bundle.files.contains_key(&name) {
                continue;
            }
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| format!("{}: {name}: {e}", bundle.bundle))?;
            // Nor is the build record; it names the source patches the bundle was built with.
            if name == "BUILDINFO.json" {
                let found = buildinfo_patches(&bytes)
                    .map_err(|e| format!("{}: BUILDINFO.json: {e}", bundle.bundle))?;
                match &patches {
                    Some(seen) if *seen != found => {
                        return Err(format!(
                            "the source patches in BUILDINFO.json differ between the tool \
                             bundles ({platform})"
                        ));
                    }
                    Some(_) => {}
                    None => patches = Some(found),
                }
                continue;
            }
            let text = normalize(&String::from_utf8_lossy(&bytes));
            match files.get_mut(&name) {
                Some(existing) if existing.text != text => {
                    return Err(format!(
                        "{name} differs between the tool bundles ({} and {platform})",
                        existing.platforms.join(", ")
                    ));
                }
                Some(existing) => existing.platforms.push(platform.to_string()),
                None => {
                    files.insert(
                        name.clone(),
                        BundleFile {
                            path: name,
                            text,
                            platforms: vec![platform.to_string()],
                        },
                    );
                }
            }
        }
    }
    let sources = manifest
        .sources
        .iter()
        .map(|s| {
            (
                s.name.clone(),
                s.version.clone(),
                s.url.clone(),
                s.sha256.clone(),
            )
        })
        .collect();
    let mut files: Vec<BundleFile> = files.into_values().collect();
    files.sort_by_key(|f| notice_order(&f.path));
    Ok(IdeviceNotices {
        version: manifest.version,
        release: manifest.release,
        sources,
        patches: patches.unwrap_or_default(),
        files,
    })
}

/// The `patches` a bundle's `BUILDINFO.json` lists (none if it has no such key).
fn buildinfo_patches(bytes: &[u8]) -> Result<Vec<SourcePatch>, String> {
    let info: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let Some(list) = info.get("patches") else {
        return Ok(Vec::new());
    };
    let list = list.as_array().ok_or("patches is not an array")?;
    list.iter()
        .map(|patch| {
            let field = |key: &str| {
                patch[key]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("a patch without {key}"))
            };
            let file = field("file")?;
            let sha256 = field("sha256")?;
            if !idevice_tools::is_plain_name(&file) || !idevice_tools::is_sha256_hex(&sha256) {
                return Err(format!("patch {file:?}: not a plain name with a SHA-256"));
            }
            Ok(SourcePatch {
                file,
                applies_to: field("applies_to")?,
                sha256,
            })
        })
        .collect()
}

/// The order of the bundle notice files: the license texts first, then the rest by path.
fn notice_order(path: &str) -> (usize, String) {
    let rank = match path {
        "COPYING.LESSER" => 0,
        "COPYING" => 1,
        _ => 2,
    };
    (rank, path.to_owned())
}

// ---- iLEAPP and aLEAPP ----

struct LeappNotices {
    /// `(display name, upstream repo, pinned tag)` per tool.
    tools: Vec<(String, String, String)>,
    license: String,
    /// `(title, url, text)` of each bundled library's license file.
    bundled: Vec<(String, String, String)>,
}

fn leapp_notices(repo_root: &Path, cache: &Path) -> Result<LeappNotices, String> {
    let path = repo_root.join("leapp-manifest.json");
    let bytes = fs::read(&path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let manifest: LeappManifest = parse_versioned(&bytes).map_err(|e| e.to_string())?;
    let mut tools = Vec::new();
    let mut license: Option<String> = None;
    for tool in ToolId::ALL {
        let entry = manifest
            .tools
            .get(tool)
            .ok_or_else(|| format!("leapp-manifest.json has no {tool}"))?;
        let url = LEAPP_LICENSE_URL
            .replace("{repo}", &entry.upstream_repo)
            .replace("{tag}", &entry.version);
        let text = normalize(&String::from_utf8_lossy(&fetch_pinned(
            cache,
            &url,
            LEAPP_LICENSE_SHA256,
        )?));
        license = Some(text);
        tools.push((
            entry.display_name.clone(),
            entry.upstream_repo.clone(),
            entry.version.clone(),
        ));
    }
    let license = license.ok_or("no LEAPP tools")?;
    let mut bundled = Vec::new();
    for pinned in &LEAPP_BUNDLED {
        let text = normalize(&String::from_utf8_lossy(&fetch_pinned(
            cache,
            pinned.url,
            pinned.sha256,
        )?));
        bundled.push((pinned.title.to_owned(), pinned.url.to_owned(), text));
    }
    Ok(LeappNotices {
        tools,
        license,
        bundled,
    })
}

// ---- rendering ----

/// Line ends as LF, whitespace at line ends and blank lines at both ends trimmed, a BOM dropped,
/// one final newline.
fn normalize(text: &str) -> String {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
    let start = lines.iter().position(|l| !l.is_empty()).unwrap_or(0);
    let end = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(start, |i| i + 1);
    let mut out = lines[start..end].join("\n");
    out.push('\n');
    out
}

/// The crates.io page of a crate version, where its source package can be downloaded.
fn crates_io_url(name: &str, version: &str) -> String {
    format!("https://crates.io/crates/{name}/{version}")
}

/// The grouping key of a license text: its words, so texts that differ only in line breaks and
/// indentation are shown once.
fn same_words(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` in a fenced block whose fence is longer than any backtick run inside it.
fn fenced(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}text\n{text}{fence}\n")
}

fn platform_label(platform: &str) -> &str {
    match platform {
        "macos-aarch64" => "macOS arm64",
        "macos-x86_64" => "macOS x64",
        "windows-x86_64" => "Windows x64",
        other => other,
    }
}

fn render(
    crates: &BTreeMap<(String, String), Crate>,
    crate_texts: &[Vec<(String, String)>],
    idevice: &IdeviceNotices,
    leapp: &LeappNotices,
) -> String {
    let mut out = String::new();
    let mut push = |s: &str| {
        out.push_str(s);
        out.push('\n');
    };

    push("# Third-party notices");
    push("");
    push(
        "Generated by `cargo xtask notices` from `Cargo.lock`, `idevice-tools.json`, \
         `leapp-manifest.json` and the pinned upstream files named below. Do not edit it by hand; \
         CI checks that it is current.",
    );
    push("");
    push(
        "suiteDFIR itself is licensed under the Apache License 2.0 (`LICENSE`, `NOTICE`). This \
         file covers the third-party software that the suiteDFIR release builds contain or run:",
    );
    push("");
    push(
        "1. [libimobiledevice tools](#libimobiledevice-tools): bundled with the macOS (arm64 \
         and x64) and Windows x64 builds.",
    );
    push(
        "2. [iLEAPP and aLEAPP](#ileapp-and-aleapp): downloaded on request, run unmodified, not \
         bundled.",
    );
    push("3. [Rust crates](#rust-crates): compiled into the app.");
    push("");
    push("License texts are reproduced as published; whitespace at line ends is trimmed.");
    push("");

    // ---- libimobiledevice ----
    push("## libimobiledevice tools");
    push("");
    let (inputs, attached) = if idevice.patches.is_empty() {
        ("the source tarballs below", "these exact tarballs")
    } else {
        (
            "the source tarballs and patches below",
            "these exact tarballs and patches",
        )
    };
    push(&format!(
        "The macOS (arm64 and x64) and Windows x64 builds include `idevice_id`, `ideviceinfo`, \
         `idevicepair` and `idevicebackup2` from libimobiledevice {}, built by \
         `scripts/build-idevice-tools.sh` from {inputs} and linked statically. Every release \
         attaches {attached}, the build script and each tool bundle's `BUILDINFO.json`. The \
         Windows arm64 build includes none of this code (there is no pinned build for it, so it \
         has no iOS acquisition), and the Linux builds use the distribution's tools and include \
         none of it either.",
        idevice.version
    ));
    push("");
    push("| Source tarball | Version | SHA-256 |");
    push("|---|---|---|");
    for (name, version, url, sha256) in &idevice.sources {
        let file = url.rsplit('/').next().unwrap_or(url);
        push(&format!(
            "| [{file}]({url}) ({name}) | {version} | `{sha256}` |"
        ));
    }
    push("");
    if !idevice.patches.is_empty() {
        push(
            "The build modifies two files, each marked with a \"Modified for suiteDFIR\" comment \
             at the change, so that the tools can open lockdown SSL sessions: Mbed TLS \
             (`library/x509_crt.c`) accepts a certificate with an empty issuer name, as \
             libimobiledevice's pairing certificates have, and libimobiledevice (`src/idevice.c`) \
             sets no TLS host name, which Mbed TLS 3.6.3 and later require before they verify \
             the device's certificate. The patches are in `scripts/idevice-tools-patches/` and \
             are applied with `patch -p1` to the extracted tarballs; each bundle's \
             `BUILDINFO.json` lists them:",
        );
        push("");
        push("| Patch | Applies to | SHA-256 |");
        push("|---|---|---|");
        for patch in &idevice.patches {
            push(&format!(
                "| `{}` | `{}` | `{}` |",
                patch.file, patch.applies_to, patch.sha256
            ));
        }
        push("");
    }
    push("Licenses:");
    push("");
    push(
        "- libimobiledevice, libimobiledevice-glue, libusbmuxd, libplist and libtatsu: GNU \
         Lesser General Public License 2.1 or later (`COPYING.LESSER`). libimobiledevice also \
         ships the GNU General Public License 2.0 (`COPYING`). Both texts follow.",
    );
    push("- Mbed TLS: Apache-2.0 OR GPL-2.0-or-later (`mbedtls/LICENSE`).");
    push(
        "- ed25519 (zlib license) and libsrp6a-sha512 (Stanford SRP, BSD-style), bundled in \
         libimobiledevice's `3rd_party/` directory.",
    );
    push("- jsmn and time64, compiled into libplist: MIT (`libplist-embedded-notices.txt`).");
    push(
        "- Windows builds only: the MinGW-w64 runtime, linked statically \
         (`mingw-w64/COPYING.MinGW-w64-runtime.txt`).",
    );
    push("");
    push(&format!(
        "The texts below are the notice files of the published tool bundles \
         (`idevice-tools-{}-<platform>.zip`, each checked against its `bundle_sha256` in \
         `idevice-tools.json`).",
        idevice.release
    ));
    push("");
    for file in &idevice.files {
        let platforms: Vec<&str> = file.platforms.iter().map(|p| platform_label(p)).collect();
        push(&format!("### `{}`", file.path));
        push("");
        push(&format!("In the {} bundles.", platforms.join(", ")));
        push("");
        push(&fenced(&file.text));
    }

    // ---- LEAPP ----
    push("## iLEAPP and aLEAPP");
    push("");
    let pins: Vec<String> = leapp
        .tools
        .iter()
        .map(|(display, _, tag)| format!("{display} {tag}"))
        .collect();
    push(&format!(
        "suiteDFIR runs the official command-line builds pinned in `leapp-manifest.json` ({}). \
         They are not bundled: the app downloads them (or imports them offline) only when the \
         user asks, verifies their SHA-256 and runs them unmodified.",
        pins.join(", ")
    ));
    push("");
    for (display, repo, _) in &leapp.tools {
        push(&format!("- {display}: <https://github.com/{repo}> (MIT)"));
    }
    push("");
    push(&format!(
        "### `LICENSE` ({}; the same text in both repositories)",
        pins.join(", ")
    ));
    push("");
    push(&fenced(&leapp.license));
    push("### Libraries inside the iLEAPP builds");
    push("");
    push(
        "The pinned iLEAPP builds contain the pillow-heif package, which bundles libheif and \
         libde265 (GNU Lesser General Public License 3.0) and libx265 (GNU General Public \
         License 2.0 or later; the GPL 2.0 text is `COPYING` above). This was checked in the \
         macOS arm64 build, which contains `libheif.1.23.1.dylib`, `libde265.0.2.1.dylib` and \
         `libx265.216.dylib`; the aLEAPP macOS arm64 build contains none of them. The license \
         files of libheif and libde265 follow.",
    );
    push("");
    for (title, url, text) in &leapp.bundled {
        push(&format!("#### {title}"));
        push("");
        push(&format!("From <{url}>."));
        push("");
        push(&fenced(text));
    }

    // ---- crates ----
    push("## Rust crates");
    push("");
    let triples: Vec<String> = RELEASE_TARGETS
        .iter()
        .map(|(t, label)| format!("`{t}` ({label})"))
        .collect();
    push(&format!(
        "The app is compiled from these {} crates.io crates: the normal dependencies of the \
         `{APP_PACKAGE}` package on the release targets {}, as `cargo tree -e \
         normal,no-proc-macro --target <triple> -p {APP_PACKAGE}` lists them. Development, build \
         and proc-macro dependencies run only on the build machine and are not part of the app. \
         \"Targets\" names the builds that include a crate when not all of them do.",
        crates.len(),
        triples.join(", ")
    ));
    push("");
    push("| Crate | Version | License | Targets |");
    push("|---|---|---|---|");
    for krate in crates.values() {
        let targets = if krate.targets.len() == RELEASE_TARGETS.len() {
            String::new()
        } else {
            RELEASE_TARGETS
                .iter()
                .filter(|(t, _)| krate.targets.contains(*t))
                .map(|(_, label)| *label)
                .collect::<Vec<_>>()
                .join(", ")
        };
        push(&format!(
            "| {} | {} | {} | {targets} |",
            krate.name, krate.version, krate.license
        ));
    }
    push("");
    push(
        "The source code of each crate is its crates.io package, available at \
         `https://crates.io/crates/<crate>/<version>`.",
    );
    push("");
    let mpl: Vec<&Crate> = crates
        .values()
        .filter(|krate| spdx_ids(&krate.license).contains(&"MPL-2.0"))
        .collect();
    if !mpl.is_empty() {
        push("### Source code of the MPL-2.0 crates");
        push("");
        push(
            "These crates are distributed in executable form under the Mozilla Public License \
             2.0. Their Source Code Form is available, at no charge, from their crates.io \
             packages (MPL-2.0 §3.2(a)):",
        );
        push("");
        for krate in mpl {
            push(&format!(
                "- {} {}: <{}>",
                krate.name,
                krate.version,
                crates_io_url(&krate.name, &krate.version)
            ));
        }
        push("");
    }
    push("### Crate license texts");
    push("");
    push(
        "Each crate's license and notice files, from its crates.io package. For the few crates \
         whose package has none, the license files at the root of their repository at the commit \
         the package was published from (per its `.cargo_vcs_info.json`) are shown, together with \
         the standard texts (SPDX license list v3.29.0) of the licenses they declare. A text that \
         several crates ship with the same wording (line breaks and indentation aside) is shown \
         once, under the first of them, with the others listed.",
    );
    push("");
    // text → [(crate, version, file)], in crate order.
    type Users = Vec<(String, String, String)>;
    let mut groups: Vec<(String, Users)> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for (krate, texts) in crates.values().zip(crate_texts) {
        for (file, text) in texts {
            let user = (krate.name.clone(), krate.version.clone(), file.clone());
            let key = same_words(text);
            match index.get(&key) {
                Some(&i) => groups[i].1.push(user),
                None => {
                    index.insert(key, groups.len());
                    groups.push((text.clone(), vec![user]));
                }
            }
        }
    }
    for (text, users) in &groups {
        let (name, version, file) = &users[0];
        push(&format!("#### {name} {version}: {file}"));
        push("");
        if users.len() > 1 {
            let others: Vec<String> = users[1..]
                .iter()
                .map(|(n, v, f)| format!("{n} {v} ({f})"))
                .collect();
            push(&format!("Also: {}.", others.join(", ")));
            push("");
        }
        push(&fenced(text));
    }
    // One trailing newline.
    while out.ends_with("\n\n") {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_trims_line_ends_and_blank_edges() {
        assert_eq!(
            normalize("\u{feff}\r\n\r\nA  \r\nB\t\r\n\r\n\r\n"),
            "A\nB\n"
        );
        assert_eq!(normalize("A\rB"), "A\nB\n");
        assert_eq!(normalize("  x\n\n  y  \n"), "  x\n\n  y\n");
        assert_eq!(normalize(""), "\n");
    }

    #[test]
    fn fences_outgrow_backticks_in_the_text() {
        assert_eq!(fenced("a\n"), "```text\na\n```\n");
        assert_eq!(fenced("a ``` b\n"), "````text\na ``` b\n````\n");
        assert_eq!(fenced("`````\n"), "``````text\n`````\n``````\n");
    }

    #[test]
    fn license_file_names() {
        for name in [
            "LICENSE",
            "LICENSE-MIT",
            "license-apache-2.0",
            "LICENCE.md",
            "COPYING",
            "NOTICE",
            "UNLICENSE",
            "COPYRIGHT",
        ] {
            assert!(is_license_file(name), "{name}");
        }
        for name in ["README.md", "Cargo.toml", "src", "LIC"] {
            assert!(!is_license_file(name), "{name}");
        }
    }

    #[test]
    fn tree_lines_parse() {
        let tree = "suitedfir v0.2.0 (/repo/src-tauri)\n\
                    serde v1.0.229\n\
                    serde_derive v1.0.229 (proc-macro)\n\
                    serde v1.0.229 (*)\n\
                    \n";
        let want: BTreeSet<(String, String)> = [
            ("serde", "1.0.229"),
            ("serde_derive", "1.0.229"),
            ("suitedfir", "0.2.0"),
        ]
        .iter()
        .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
        .collect();
        assert_eq!(tree_packages(tree), want);
    }

    fn metadata() -> Value {
        let pkg = |id: &str, name: &str, version: &str, source: Option<&str>| {
            serde_json::json!({
                "id": id, "name": name, "version": version, "license": "MIT OR Apache-2.0",
                "manifest_path": format!("/registry/{name}-{version}/Cargo.toml"),
                "source": source,
            })
        };
        serde_json::json!({
            "packages": [
                pkg("app", "suitedfir", "0.2.0", None),
                pkg("a1", "a", "1.0.0", Some(CRATES_IO)),
                pkg("a2", "a", "2.0.0", Some(CRATES_IO)),
                pkg("git", "g", "1.0.0", Some("git+https://example.com/g")),
                pkg("d1", "d", "1.0.0", Some(CRATES_IO)),
                pkg("d2", "d", "1.0.0", Some("registry+https://example.com/index")),
            ],
            "workspace_members": ["app"],
        })
    }

    #[test]
    fn crate_details_come_from_metadata() {
        let metadata = metadata();
        let index = PackageIndex::new(&metadata).unwrap();
        let app = ("suitedfir".to_owned(), "0.2.0".to_owned());
        assert!(index.members.contains(&app));
        let a = index.krate("a", "2.0.0").unwrap();
        assert_eq!((a.name.as_str(), a.version.as_str()), ("a", "2.0.0"));
        assert_eq!(a.license, "MIT OR Apache-2.0");
        assert_eq!(a.dir, Path::new("/registry/a-2.0.0"));
    }

    #[test]
    fn crates_must_come_from_crates_io_unambiguously() {
        let metadata = metadata();
        let index = PackageIndex::new(&metadata).unwrap();
        let err = index.krate("g", "1.0.0").unwrap_err();
        assert!(err.contains("crates.io"), "{err}");
        let err = index.krate("d", "1.0.0").unwrap_err();
        assert!(err.contains("2 packages"), "{err}");
        let err = index.krate("a", "3.0.0").unwrap_err();
        assert!(err.contains("0 packages"), "{err}");
    }

    #[test]
    fn license_texts_come_from_license_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("LICENSE-MIT"), "MIT text\r\n").unwrap();
        fs::write(dir.path().join("NOTICE"), "notice").unwrap();
        fs::write(dir.path().join("README.md"), "readme").unwrap();
        fs::create_dir(dir.path().join("LICENSES")).unwrap();
        fs::write(dir.path().join("LICENSES").join("Apache-2.0.txt"), "apache").unwrap();
        let krate = Crate {
            name: "x".into(),
            version: "1.0.0".into(),
            license: "MIT".into(),
            dir: dir.path().to_path_buf(),
            targets: BTreeSet::new(),
        };
        let cache = tempfile::tempdir().unwrap();
        assert_eq!(
            license_texts(&krate, cache.path()).unwrap(),
            [
                ("`LICENSE-MIT`".to_owned(), "MIT text\n".to_owned()),
                (
                    "`LICENSES/Apache-2.0.txt`".to_owned(),
                    "apache\n".to_owned()
                ),
                ("`NOTICE`".to_owned(), "notice\n".to_owned()),
            ]
        );
        // A crate without license files needs an UPSTREAM entry (no download is attempted).
        let empty = tempfile::tempdir().unwrap();
        let krate = Crate {
            dir: empty.path().to_path_buf(),
            ..krate
        };
        let err = license_texts(&krate, cache.path()).unwrap_err();
        assert!(
            err.contains("no license file") && err.contains("UPSTREAM"),
            "{err}"
        );
    }

    #[test]
    fn an_upstream_file_must_be_from_the_published_commit() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(".cargo_vcs_info.json"),
            r#"{"git":{"sha1":"0000000000000000000000000000000000000000"},"path_in_vcs":"x"}"#,
        )
        .unwrap();
        assert_eq!(
            published_commit(dir.path()).unwrap(),
            "0000000000000000000000000000000000000000"
        );
        // block2 0.6.2's entry names another commit; the check fails before any download.
        let krate = Crate {
            name: "block2".into(),
            version: "0.6.2".into(),
            license: "MIT".into(),
            dir: dir.path().to_path_buf(),
            targets: BTreeSet::new(),
        };
        let cache = tempfile::tempdir().unwrap();
        let err = license_texts(&krate, cache.path()).unwrap_err();
        assert!(err.contains("published from commit 0000000"), "{err}");

        fs::write(dir.path().join(".cargo_vcs_info.json"), r#"{"git":{}}"#).unwrap();
        assert!(published_commit(dir.path()).is_err());
    }

    #[test]
    fn spdx_expressions_split_into_ids() {
        assert_eq!(spdx_ids("MIT"), ["MIT"]);
        assert_eq!(
            spdx_ids("Zlib OR Apache-2.0 OR MIT"),
            ["Zlib", "Apache-2.0", "MIT"]
        );
        assert_eq!(spdx_ids("MIT/Apache-2.0"), ["MIT", "Apache-2.0"]);
        assert_eq!(
            spdx_ids("(MIT OR Apache-2.0) AND Unicode-3.0"),
            ["MIT", "Apache-2.0", "Unicode-3.0"]
        );
        assert_eq!(
            spdx_ids("Apache-2.0 WITH LLVM-exception OR MIT"),
            ["Apache-2.0", "MIT"]
        );
    }

    /// Every crate without packaged license files has standard texts for all its licenses.
    #[test]
    fn upstream_entries_are_consistent() {
        for upstream in UPSTREAM {
            for (_, url, sha256) in upstream.files {
                assert!(
                    url.starts_with("https://raw.githubusercontent.com/"),
                    "{url}"
                );
                assert!(idevice_tools::is_sha256_hex(sha256), "{url}");
            }
        }
        for (_, sha256) in SPDX_TEXTS {
            assert!(idevice_tools::is_sha256_hex(sha256));
        }
    }

    /// The committed file is what the generator writes (CI regenerates it, which needs the
    /// network; this only checks the parts that do not).
    #[test]
    fn the_committed_notices_cover_the_pinned_inputs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let notices = fs::read_to_string(root.join(OUTPUT)).unwrap();
        let manifest = idevice_tools::read_manifest(&root.join("idevice-tools.json")).unwrap();
        for source in &manifest.sources {
            assert!(notices.contains(&source.url), "{}", source.url);
            assert!(notices.contains(&source.sha256), "{}", source.name);
        }
        // FX1: the bundles are named after the release, and every source patch is listed.
        assert!(
            notices.contains(&format!(
                "`idevice-tools-{}-<platform>.zip`",
                manifest.release
            )),
            "{}",
            manifest.release
        );
        let patch_dir = root.join("scripts").join("idevice-tools-patches");
        let mut patches = 0;
        for entry in fs::read_dir(&patch_dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let sha256 = sha256_file(&path).unwrap();
            assert!(
                notices.contains(&format!("| `{name}` |")) && notices.contains(&sha256),
                "{name} {sha256}"
            );
            patches += 1;
        }
        assert_eq!(patches, 2);
        for heading in [
            "### `COPYING`",
            "### `COPYING.LESSER`",
            "### `mbedtls/LICENSE`",
            "### `libplist-embedded-notices.txt`",
            "### `mingw-w64/COPYING.MinGW-w64-runtime.txt`",
            "### `3rd_party/ed25519/LICENSE`",
            "### `3rd_party/libsrp6a-sha512/LICENSE`",
            "#### libheif `COPYING`",
            "#### libde265 `COPYING`",
            "## Rust crates",
        ] {
            assert!(notices.contains(heading), "{heading}");
        }
        // MPL-2.0 §3.2(a): every MPL-2.0 crate in the table names where its source is.
        let mut mpl = 0;
        for row in notices.lines().filter(|l| l.starts_with("| ")) {
            let cells: Vec<&str> = row.split(" | ").collect();
            if cells.len() >= 3 && spdx_ids(cells[2]).contains(&"MPL-2.0") {
                let name = cells[0].trim_start_matches("| ");
                let url = crates_io_url(name, cells[1]);
                assert!(notices.contains(&format!("<{url}>")), "{url}");
                mpl += 1;
            }
        }
        assert!(mpl > 0, "the app ships MPL-2.0 crates (option-ext)");
    }

    #[test]
    fn buildinfo_patches_are_read() {
        let sha = "a".repeat(64);
        let info = format!(
            r#"{{ "name": "idevice-tools", "patches": [
                {{ "file": "x.patch", "sha256": "{sha}", "applies_to": "x-1.0.tar.bz2" }} ] }}"#
        );
        assert_eq!(
            buildinfo_patches(info.as_bytes()).unwrap(),
            [SourcePatch {
                file: "x.patch".into(),
                applies_to: "x-1.0.tar.bz2".into(),
                sha256: sha.clone(),
            }]
        );
        // A bundle built before FX1 lists none.
        assert!(buildinfo_patches(b"{}").unwrap().is_empty());
        for bad in [
            r#"{ "patches": {} }"#.to_owned(),
            format!(r#"{{ "patches": [{{ "file": "x.patch", "sha256": "{sha}" }}] }}"#),
            format!(
                r#"{{ "patches": [{{ "file": "../x.patch", "sha256": "{sha}", "applies_to": "x" }}] }}"#
            ),
            r#"{ "patches": [{ "file": "x.patch", "sha256": "ABC", "applies_to": "x" }] }"#
                .to_owned(),
            "not json".to_owned(),
        ] {
            assert!(buildinfo_patches(bad.as_bytes()).is_err(), "{bad}");
        }
    }

    #[test]
    fn crates_io_urls() {
        assert_eq!(
            crates_io_url("option-ext", "0.2.0"),
            "https://crates.io/crates/option-ext/0.2.0"
        );
    }
}
