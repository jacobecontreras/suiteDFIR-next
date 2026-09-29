# Development

> Written by AI (Claude Code) during development and not yet fully reviewed by a person. Where it disagrees with the code, the code is right. See [How this was built](README.md#how-this-was-built).

Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) first. This file covers how to build and test, and the rules every change must follow.

## 1. Toolchain

| Tool | Version policy |
|---|---|
| Rust | Pinned in `rust-toolchain.toml` to a specific stable release (≥ 1.89 for `File::try_lock`) with `components = ["rustfmt", "clippy"]`. On a new machine, first run `rustup toolchain install <pin> -c rustfmt,clippy`. |
| Tauri CLI | Exactly **2.11.5**: `cargo install tauri-cli --version "=2.11.5" --locked`. `release.yml` pins the same version (`TAURI_CLI_VERSION`; the only workflow that installs it); bump both deliberately. |
| Node.js | Dev-only (`tsc`, `node --test`, `scripts/serve-ui.mjs`). Version: `.node-version` and `package.json` `engines`. |
| TypeScript | Exact version in `package.json` `devDependencies`, installed with `npm ci`. |
| cargo-deny | Exactly **0.20.2**. CI uses the prebuilt release binary checked against a pinned SHA-256; locally `cargo install cargo-deny --version "=0.20.2" --locked`. |
| cargo-auditable | Exactly **0.7.6**, release builds only: `cargo install cargo-auditable --version "=0.7.6" --locked`. Release builds run `cargo tauri build --runner <abs>/scripts/cargo-auditable` (`scripts/cargo-auditable.cmd` on Windows), which embeds each binary's dependency list. `release.yml` pins the same version (`CARGO_AUDITABLE_VERSION`). |
| sccache | Recommended locally: `cargo install sccache --locked`, then `RUSTC_WRAPPER=sccache`. It caches compiled dependencies by content hash, so it is safe across worktrees, unlike a shared `CARGO_TARGET_DIR`. |
| Linux build deps | Ubuntu 22.04 packages: `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev build-essential libssl-dev pkg-config curl wget file`. |

## 2. Commands

```bash
npm ci                                   # dev tools only (typescript)
cargo tauri dev                          # run the app (CSP NOT enforced in this mode, see ARCHITECTURE §9)
cargo tauri dev --no-dev-server          # run with embedded assets (CSP enforced)
cargo test --workspace --locked          # all Rust tests, incl. fake-leapp process tests
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all --check
cargo deny check
npm run typecheck                        # tsc --noEmit over ui/, ui-dev/, tests/ui/
npm test                                 # node --test "tests/ui/**/*.test.js"
node scripts/record-invokes.mjs          # rewrite tests/ui/recorded-invokes.json (§4.8 IPC replay; npm test fails when stale)
node scripts/serve-ui.mjs [--root <dir>] [--port 5173]
                                         # browser mock mode at http://127.0.0.1:5173/?mock (see its header)
node tests/ui/e2e/shots.mjs --root <dir> --out <dir> [--screens a,b] [--dom]
                                         # mock-mode screenshots and checks; needs Playwright + Chromium (see its header)
cargo xtask pin-leapp --tool ileapp --tag v2026.4.2 --download-verify   # update leapp-manifest.json
                                         # (~350 MB per tool, downloaded to the OS temp dir or
                                         # --download-dir <dir> and deleted after checking)
cargo xtask contracts                    # regenerate ui-dev/fixtures/contracts/ (*.json + index.js)
cargo xtask notices                      # regenerate THIRD-PARTY-NOTICES.md (needs the network; token as for fetch-idevice-tools; see xtask/src/notices.rs)
scripts/build-idevice-tools.sh <platform-key>  # build a pinned libimobiledevice tool bundle (see its header)
cargo xtask fetch-idevice-tools [--target <triple>]  # fetch + verify the pinned tool bundle into src-tauri/binaries/
                                         # (release builds; token: GH_TOKEN, else `gh auth token [--user $SUITEDFIR_GH_USER]`)
cargo test -p suitedfir-core --test leapp_smoke --locked -- --ignored --test-threads=1   # real LEAPP
```

**Release bundles** (unsigned; no release has been published yet). The macOS and Windows x64 bundles can be built on local machines as below, the Linux (x64 and arm64) and Windows arm64 ones only by `release.yml` (§6); run from the repository root, without `RUSTC_WRAPPER`, and with `CI=true` on a Mac without a desktop session (the dmg script then skips its Finder step):

```bash
# macOS, per architecture: .app + .dmg in target/<triple>/release/bundle/{macos,dmg}/
cargo xtask fetch-idevice-tools --target aarch64-apple-darwin    # and x86_64-apple-darwin
cargo tauri build --runner "$PWD/scripts/cargo-auditable" --target aarch64-apple-darwin \
  --config src-tauri/tauri.release.conf.json

# Windows x64: the online installer, then the offline one (WebView2 embedded); both come out as
# target/release/bundle/nsis/suiteDFIR_<ver>_x64-setup.exe, so rename each one in between
cargo xtask fetch-idevice-tools
cargo tauri build --runner <abs path>\scripts\cargo-auditable.cmd --config src-tauri/tauri.release.conf.json
#   → suiteDFIR_<ver>_x64-online-setup.exe
cargo tauri build --runner <abs path>\scripts\cargo-auditable.cmd --config src-tauri/tauri.release.conf.json \
  --config src-tauri/tauri.offline.conf.json
#   → suiteDFIR_<ver>_x64-offline-setup.exe

# The bundled iOS tools must match idevice-tools.json (unsigned builds), checked with the app's own lookup:
SUITEDFIR_BUNDLED_TOOLS_DIR=<suiteDFIR.app/Contents/MacOS or the Windows install dir> \
  [SUITEDFIR_BUNDLED_TOOLS_PLATFORM=macos-x86_64] \
  cargo test -p suitedfir-core --test idevice -- --ignored --exact release_bundle_tools_verify_against_the_manifest
```

For the Linux and Windows arm64 bundles, see `release.yml`'s header. On a Mac the tool bundle is fetched with `SUITEDFIR_GH_USER` set while `gh` holds several accounts; while the repository is public, an anonymous download works too.

The `xtask` alias lives in `.cargo/config.toml`.

**Debug-build dev override:** `SUITEDFIR_DEV_LEAPP_OVERRIDE=<path to target/debug/fake-leapp>` makes both tools report `ToolState.dev_override` (its effects: `crates/core/src/leapp/dev_override.rs`). The UI shows a "DEV OVERRIDE" banner. The code is compiled out of release builds (`#[cfg(debug_assertions)]`).

Likewise, `SUITEDFIR_DEV_IDEVICE_OVERRIDE=<path to target/debug/fake-idevice>` (debug builds only) makes the core use fake-idevice for all four libimobiledevice tools (`IdeviceToolSource.dev_override`).

The bundled libimobiledevice tools are **not** needed for `cargo tauri dev` or `cargo tauri build --debug --no-bundle`. They are attached only by the release overlay config `src-tauri/tauri.release.conf.json` (`bundle.externalBin`). Debug builds find tools via the dev override (`SUITEDFIR_DEV_IDEVICE_OVERRIDE`), next to the app executable (`target/debug/`, the same place release builds look), or on `PATH`; `src-tauri/binaries/` is not searched.

## 3. Repository layout

```
crates/core/                  suitedfir-core: all logic, no Tauri dependency
  src/                        modules as in ARCHITECTURE §5.1
  src/bin/                    the test doubles fake-leapp and fake-idevice (never bundled)
  tests/{process.rs, introspection.rs, runner.rs, idevice.rs, acquire.rs, leapp_smoke.rs, common/,
         golden_acq.rs, golden_run.rs, golden/ (acquisition and run goldens)}
src-tauri/                    app shell; binaries/ (gitignored) holds the fetched iOS tools
xtask/                        pin-leapp, contracts, notices, fetch-idevice-tools
ui/                           SHIPPED frontend (frontendDist)
ui-dev/                       NOT shipped: the browser mock and fixtures/contracts/ (generated)
tests/ui/                     node --test files for ui/ modules; e2e/shots.mjs (Playwright screenshots)
fixtures/leapp/<tool>/<ver>/  captured real-LEAPP outputs (paths sanitized to <RUN_DIR>, <INPUT>)
scripts/                      serve-ui, record-invokes, the cargo-auditable runner, build-idevice-tools and its patches
.github/workflows/            ci-rust.yml, ci-js.yml, leapp-smoke.yml, release.yml
docs/                         ARCHITECTURE, CONTRACTS, LEAPP-CLI, IDEVICE-CLI, USER-GUIDE, QA-CHECKLIST
```

**Task and decision IDs.** Code comments and docs refer to tasks of the development plan (for example A1, E3, K8, X3b, S1, FX1), to owner checkpoints (H1 to H7) and to design decisions (D1 to D25, [ARCHITECTURE §3](docs/ARCHITECTURE.md#3-decision-log)). The plan, `docs/ROADMAP.md`, is no longer in the repository. Its last version, with each task's scope and acceptance criteria, is [docs/ROADMAP.md at b5ac473](https://github.com/jacobecontreras/suiteDFIR-next/blob/b5ac4736aa07b2d8f843af1a2974a6c4113bf707/docs/ROADMAP.md).

## 4. Rules

### 4.1 Scope

Build only what [ARCHITECTURE.md §2](docs/ARCHITECTURE.md#2-scope) lists. The out-of-scope list is binding: no stubs, flags, routes, menu items or "coming soon" UI for those features.

### 4.2 Dependencies

**Rust: allowed direct dependencies**

| Crate | Spec | Where |
|---|---|---|
| `tauri` 2.11.x, `tauri-build` 2.6.x | minimal features | src-tauri |
| `tauri-plugin-dialog` 2.x | JS open/save dialogs + Rust message dialogs | src-tauri |
| `tauri-plugin-opener` 2.x | **free functions only** (`open_path`, `reveal_item_in_dir`); never `.plugin(...)` | src-tauri |
| `serde` (derive), `serde_json` | | `serde`: core, src-tauri; `serde_json`: all |
| `sha2` | | core |
| `zip` | `default-features = false, features = ["deflate-flate2-zlib-rs"]` | core, xtask |
| `ureq` 3 | `default-features = false, features = ["rustls"]`; HTTPS only (redirects too); every response body read with a size cap | core, xtask |
| `plist` | `Value::from_reader` (XML and binary) | core |
| `time` | `formatting`, `parsing` | core |
| `getrandom` | | core |
| `libc` (unix), `windows-sys` (windows; features: ARCHITECTURE §7) | | core |
| `log` | facade; the file logger is our own | core, src-tauri |
| `thiserror` | | core, src-tauri |
| dev-only: `tempfile`; `tauri` feature `test` (E2) | | tests |

Transitive notes: `tauri-plugin-dialog` pulls in the `tauri-plugin-fs` crate (never registered), and `ureq`'s rustls pulls in `webpki-roots`.

Anything else needs a PR labelled `new-dependency` that explains why std or an allowed crate cannot do it, with its transitive count (`cargo tree -e normal`). The maintainer must approve it, and the table above is updated in the same PR.

`deny.toml` holds the allowed licenses, the allowed sources (crates.io only) and the advisory policy (see its comments).

**JavaScript:**
- **Runtime:** zero packages; no `node_modules` code ever ships.
- **Dev-only:** `typescript` only. Adding a dev package or vendoring JS into `ui/` follows the same approval rule.

**GitHub Actions:** third-party actions are pinned by full commit SHA.

### 4.3 Security (see ARCHITECTURE §9)

- **No dynamic HTML:**
  - Never assign dynamic data to `innerHTML`, `outerHTML` or `insertAdjacentHTML`, and never use `document.write`.
  - Use `ui/lib/dom.js` (`h()`), `textContent` and DOM APIs.
- **CSP:**
  - No inline scripts, `style=""` attributes, `eval` or `new Function`; styles via classes or CSSOM.
  - Never loosen the CSP or capabilities.
  - UI tests served by `serve-ui.mjs` must show zero CSP violations in the console.
- **Commands:** new commands validate every path argument per the path policy (ARCHITECTURE §9) and every enum.
- **Secrets:**
  - Never log, persist or emit backup passwords. `itunes_password` exists only in `RunRequest` and in the spawned LEAPP argv; the acquisition passwords only in their request payloads and the idevice tools' env (ARCHITECTURE §9, D15). Redact them everywhere else.
  - Types holding one implement `Debug` by hand so the secret is never printed (e.g. `<redacted>`).
  - The UI clears password fields after use.
- **Reports:** never load LEAPP report files into the app webview.

### 4.4 Forensic integrity

- **Never touch the input:**
  - Never open an input path for writing, create files in it or change its metadata.
  - Hash and inspect with read-only handles.
  - Enforce the overlap rule (ARCHITECTURE §6 step 1).
- **Complete records:** everything that affects a run's result goes into `run.json`. A new option means a new record field.
- **Keep the output:** never delete or rewrite anything in a run folder after finalize. Never auto-delete partial output.
- **UTC on disk:** timestamps on disk are UTC. The UI shows local time with UTC on hover.

### 4.5 Rust conventions

- **Pure core:** no `unwrap`/`expect` outside tests and provably infallible spots (comment why). Errors are `thiserror` enums mapped to `AppError` codes (CONTRACTS.md §12).
- **I/O placement:** `crates/core` takes directories and callbacks as parameters and never reads Tauri state or guesses OS dirs.
- **Async:** blocking work (hashing, process wait, downloads) runs on dedicated threads or `tauri::async_runtime::spawn_blocking`, never on the command thread.
- **Platform code:** the platform files are `process/{unix,windows}.rs` and `fsutil/{unix,windows}.rs` (plus `inspect` for OS backup locations and `idevice` for tool lookup); elsewhere, OS differences are `cfg` attributes or `cfg!` checks in ordinary modules. The only permitted `unsafe` is FFI, commented: in the platform files, and in the test doubles' signal handling (`bin/fake-leapp.rs`, `bin/fake-idevice.rs`). One exception without FFI or `unsafe`: `src-tauri/src/host.rs` reads the OS version and host name per OS for the records' `host` (on Windows through `%SystemRoot%\System32\cmd.exe`, never a bare `cmd.exe`).
- **Style:** `cargo fmt`; clippy clean with `-D warnings`.

### 4.6 UI conventions

- **Code style:**
  - ES modules, `// @ts-check` in every file, JSDoc types via `@typedef {import("../types").X}`.
  - No `@enum` or Closure-style tags (TypeScript 7 rejects them).
- **Structure:**
  - One module per screen/component, exporting a function that returns a DOM node plus `dispose()`.
  - State in `lib/store.js` (tiny pub/sub).
- **API boundary:**
  - `api/ipc.js` is the only module calling into `window.__TAURI__`. `api/index.js` may only test for its existence.
  - `ui-dev/mock.js` implements the same interface from the contract fixtures (it imports the generated `ui-dev/fixtures/contracts/index.js`, the same data as the `*.json` files) and simulates runs: streaming logs, cancel, every final status, chosen by an input-path suffix such as `…/fail-invalid`.
- **Errors:** every command failure is shown through one `AppError` display component (code, message, expandable detail).
- **Dialogs:** in-DOM `<dialog>` elements; never `window.alert`/`confirm`.
- **Accessibility:** semantic elements, labelled controls, visible focus, full keyboard operation. The latest log line goes to an `aria-live="polite"` region, throttled to at most 1 update/s.
- **Styling:** CSS custom properties, light/dark via `prefers-color-scheme`, system font stack, a few inline SVG icons. No web fonts, no CSS frameworks.
- **Timezone lists:** `tool_modules("ileapp").timezones` when available. Otherwise `Intl.supportedValuesOf?.("timeZone")`, falling back to an embedded short list.
- **Performance:** the log view is virtualized (fixed row height) and stays smooth at 100,000 lines. The module picker stays responsive at 1,300 modules.

### 4.7 Contracts

The Rust types in `crates/core/src/contracts/` are the source of truth. `cargo xtask contracts` regenerates `ui-dev/fixtures/contracts/`: one `*.json` example per type, plus the generated `index.js` (the same data as an ES module, which `mock.js` imports and `npm run typecheck` checks against `ui/types.d.ts`). CI fails if the regenerated files differ from the committed ones.

A contract change updates all of these in one PR:
- the Rust types;
- the examples;
- `ui/types.d.ts`;
- `ui-dev/mock.js`;
- docs/CONTRACTS.md.

### 4.8 Testing

**Unit tests:**
- Every function in `crates/core` with logic has unit tests.
- Status rules have table-driven tests, including every row of CONTRACTS.md §7.4, over synthetic outputs and the captured fixtures in `fixtures/leapp/`.

**fake-leapp** (`crates/core/src/bin/fake-leapp.rs`) mimics a LEAPP onefile binary: a bootloader that re-executes itself as a worker, LEAPP's flags and output layout, and `FAKE_LEAPP_*` variables for the PIDs and the pacing. Its module docs describe it in full, scenarios and probe copies included.

**Process tests (all three OSes):**
- Log lines arrive incrementally.
- Cancel leaves **no** surviving process from the tree (check both PIDs).
- The per-run temp dir is gone.
- `prompt` exits within 5 s.

**fake-idevice** (`crates/core/src/bin/fake-idevice.rs`) emulates `idevice_id`, `ideviceinfo`, `idevicepair` and `idevicebackup2` with the output of docs/IDEVICE-CLI.md, so no test needs a real device. Its module docs describe the tool selection, output, pairing, passwords, signals, pacing variables and every scenario. Tests create **copies** of the binary named after the tools, never symlinks.

**Real LEAPP:** `leapp_smoke` tests (ignored by default) install the pinned tools through the core and run introspection and fixture runs. They run in the `leapp-smoke` workflow. On Linux the pinned builds need glibc ≥ 2.43 (iLEAPP) / ≥ 2.42 (aLEAPP) and do not start on Ubuntu 22.04 (LEAPP-CLI.md §2). `fixtures/leapp/<tool>/<version>/` holds outputs of their fixture runs, captured on macOS arm64 (how: the `leapp_smoke.rs` module docs), and the `run::status` tests check them. Re-capture them when the pinned versions change.

**UI tests:**
- `node --test` over pure modules: store, filters, virtual-list math, selection/profile diff, formatting.
- A parity test: `ipc.js` and `mock.js` export identical function names.
- **IPC replay (E2):** `tests/ui/recorded-invokes.json` holds every invoke `ipc.js` makes in a scripted flow over all commands (`tests/ui/ipc-flow.js`). `src-tauri/src/replay.rs` replays it through the real command handlers on Tauri's mock runtime and checks each answer and event against its contract type. Details are in its module docs and in `src-tauri/src/testing.rs`, which says where the src-tauri tests find fake-leapp and fake-idevice.

**Screenshots:** UI PRs link mock-mode screenshots (light and dark) of every changed screen state, made with `tests/ui/e2e/shots.mjs` (§2).

**Goldens:** the golden and characterization tests (`crates/core/tests/golden_run.rs`, `golden_acq.rs` and `crates/core/tests/golden/**`, the `mod z0` test blocks, `src-tauri/src/z0_tests.rs` and `tests/ui/z0-*`) pin the normalized expected outputs of runs, acquisitions and command errors, and a golden changes only in a PR whose stated purpose is that behavior change.

**Platform test caveats.**
- **Windows symlinks:** creating symlinks requires Developer Mode or admin. Tests that create symlinks must **skip with an explicit message** on `ERROR_PRIVILEGE_NOT_HELD` (1314), never fail silently or pass vacuously.
- **Windows paths:** keep test paths short; long-path support may be disabled on the machine.
- **Linux CI runs cargo unprivileged:** the Linux job runs every cargo step as an unprivileged user, never root, so permission and read-only tests are real there.

## 5. Commits and pull requests

- **Branches and merging:** work on a branch and open a pull request against `main`. `main` changes only through squash-merged pull requests. Never force-push, rewrite pushed history or delete branches.
- **Commits:** Conventional Commits (`feat(core): …`, `fix(ui): …`, `test: …`, `ci: …`, `docs: …`).
- **Before merging:** CI (§6) is green on the pull request's head commit, the branch is up to date with `main`, a review has no open blocking issues, and the docs are updated for any change in behavior or contracts.
- **Pull request description:** what changed, how it was verified (commands and results), dependency changes (normally none), contract changes and, for UI changes, screenshot links (§4.8).

## 6. CI and headless development

The UI can be developed and screenshotted entirely in browser mock mode (`node scripts/serve-ui.mjs`, then open `/?mock`) using any headless browser. `?mock&scenario=<flags>` selects mock states (e.g. `empty`, `no_tools`, `dev_override`, `active_run`); the flags and the input-path and label suffixes that choose simulated outcomes are listed at the top of `ui-dev/mock.js`. Machines that cannot open GUI windows can still run every Rust test, including the fake-leapp and fake-idevice process tests.

Each workflow's triggers, runners, legs and steps are in its file (the header comment and the job comments).

| Workflow | For |
|---|---|
| `ci-rust.yml` | Rust checks: fmt, clippy, cargo-deny, contracts and notices drift, tests and build. |
| `ci-js.yml` | UI typecheck and tests. |
| `leapp-smoke.yml` | The real-LEAPP smoke tests (`leapp_smoke`, §4.8). |
| `release.yml` | Release bundles. A dispatch is a build-only dry run (the `os` input picks the legs; the output is uploaded to the run); a `v*` tag creates a **draft** release, which a maintainer publishes by hand. |

**CI rules:**
- **Tauri CLI:** only `release.yml` installs the Tauri CLI. `ci-rust.yml` compiles the app with `cargo build`, which includes `tauri-build`'s config validation, and never launches it.
- **Concurrency:** `concurrency` cancels superseded runs **for pull requests only**; runs on `main` always finish, because they seed the cache.
- **Windows on Arm runners** (`windows-11-arm`, release and smoke legs): a native `aarch64-pc-windows-msvc` Rust host, and `clang` for `ring` from the image's LLVM; see the commented steps in `release.yml` and `leapp-smoke.yml`.
- **Other builds:** the iOS tools are built on local machines (§2). Dispatch requires a workflow file on the default branch; to test a branch's version of a workflow, dispatch it with `--ref <branch>`.
