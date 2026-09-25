# Roadmap: phase 1

Tasks are sized for one PR each. Branches are `task/<id>-<slug>` (DEVELOPMENT.md §5). Every task must also meet the Definition of Done in DEVELOPMENT.md §5. **"Gate green"** means the verification gate in DEVELOPMENT.md §6 on the PR head: Linux CI plus local macOS and Windows runs while the repository is private; the full three-OS CI once it is public.

## Dependency graph

```
M0.1 ─► M0.2 ─► M0.3 ─┬─► A1 ─► A2 ─┐
                      ├─► B1 ─► B2 ─┼─► A3 ─┐
                      │             ├─► B3 ─┤
                      │             └─► X2 ─┼─► X3 ─┐
                      ├─► C1 ─► C2 ─► C3 ───┼───────┼─► E1a ─┬─► E1b ─┐
                      │                     │       │        └─► E3 ──┼─► F1 ─► G1 ─┐
                      ├─► D1 ─► D2 ─► D3 ─► D4a ─► D4b ─► D5 ─────────┴─► E2 ─┘     ├─► RC
                      └─► X1 (tool build, parallel; needed by F1)   E1b ─► F2   E2 ─► G2 ┘
Should: S1, S2, S3 after E2 and E3.     Optional (owner approval): U
```

**Edges:**
- A3 needs A2 **and** B2.
- X2 needs B2. X3 needs X2 and C3.
- E1a needs A3, B3 and C3.
- E1b needs E1a and X3.
- E2 needs E1b, D4a, D4b and D5.
- E3 needs E1a.
- F1 needs E2, E3 and X1.
- F2 needs E1b.
- G1 needs F1.
- G2 needs E2.
- RC needs all Musts, F1, F2, G1, G2 and H4.

Tracks A–D and X run in parallel after M0.3 (contract freeze), at most 4 implementers at once.

---

## M0: Bootstrap (serial)

### M0.1 Workspace scaffold

- **Workspace and repo files:**
  - Cargo workspace: `crates/core` (`suitedfir-core`), `src-tauri` (`suitedfir`), `xtask`.
  - `rust-toolchain.toml`, `deny.toml` (DEVELOPMENT §4.2), `.cargo/config.toml` (xtask alias).
  - `.gitattributes` (`* text=auto eol=lf`, `*.png binary`), `.editorconfig`, `.gitignore`, `.node-version`.
  - `[profile.dev.package.sha2] opt-level = 3`.
- **Tauri app:**
  - `tauri` 2.11.x, `tauri-build` 2.6.x; identifier `com.suitedfir.desktop`, product `suiteDFIR`, version `0.2.0`.
  - `build.frontendDist: "../ui"`, no `devUrl`, `app.withGlobalTauri: true`, CSP exactly as ARCHITECTURE §9.
  - `capabilities/default.json`: `core:default`, `dialog:allow-open`, `dialog:allow-save`.
  - Only the dialog plugin is registered.
  - Icons generated with `cargo tauri icon` from the legacy `electron/build/icon.png` (1024², in the legacy repo).
- **UI and dev tooling:**
  - `ui/index.html`, `ui/app.js` (renders a "suiteDFIR" shell), `ui/styles/app.css`, `ui/lib/dom.js`, `ui/types.d.ts` (empty).
  - `ui-dev/` (empty `mock.js`), `tests/ui/dom.test.js`.
  - `package.json`: private; devDependencies = exact `typescript`; scripts `typecheck` (`tsc -p .`) and `test` (`node --test "tests/ui/**/*.test.js"`, glob **quoted**).
  - `package-lock.json`; `tsconfig.json` (`allowJs`, `checkJs`, `strict`, `noEmit`, include `ui`, `ui-dev`, `tests/ui`).
  - `scripts/serve-ui.mjs`: node:http, zero deps; serves `ui/` at `/` and `ui-dev/` at `/dev/`; sends the `tauri.conf.json` CSP as a `Content-Security-Policy` header; `--port` flag.
- **Legal and docs:**
  - `LICENSE` (Apache-2.0), `NOTICE`, `THIRD-PARTY-NOTICES.md` (placeholder with the LEAPP MIT notice).
  - Record the exact Tauri CLI version in DEVELOPMENT §1.

**Accept:**
- Locally, all of these pass: `cargo build`, `cargo test`, `cargo clippy -D warnings`, `cargo fmt --check`, `cargo deny check`, `npm ci && npm run typecheck && npm test`, and `cargo tauri build --debug --no-bundle`.
- `cargo tree -e normal --depth 1` shows only allowlisted crates.
- The CSP string in `tauri.conf.json` equals ARCHITECTURE §9.

### M0.2 CI

**Build:** `.github/workflows/ci.yml` on `pull_request` and `push` to `main`, with `concurrency` cancelling superseded runs and path filters skipping docs-only changes.
- **`js` job** (`ubuntu-24.04`): `npm ci`, typecheck, tests.
- **`rust` job** (matrix):
  - `ubuntu-24.04` running in an `ubuntu:22.04` container with the DEVELOPMENT §1 apt deps, always.
  - `macos-15` (arm64) and `windows-2025` (x64), only on `workflow_dispatch` or when the repo is public (DEVELOPMENT §6).
  - Steps: fmt, clippy, `cargo test --workspace --locked`, `cargo tauri build --debug --no-bundle`.
- **`deny` job**.
- **`contracts-drift` job**: a no-op until M0.3.
- **Hygiene:** actions pinned by commit SHA; cargo + tauri-cli caching.

**Accept:**
- CI is green on the PR, and local macOS + Windows runs are recorded in the PR body.
- The PR description shows a throwaway commit with a clippy warning turning CI red (reverted before merge; history is not rewritten, so it's a normal revert commit).

### M0.3 Contracts and shared foundations

This task removes cross-track collisions so tracks A–D rarely touch the same files.

- **`crates/core/Cargo.toml`:** declares **every** allowlisted crate the core will use (with the DEVELOPMENT §4.2 specs). `Cargo.lock` committed.
- **Module stubs:** `crates/core/src/lib.rs` declares every module in ARCHITECTURE §5.1, each with a stub file (doc comment only, no placeholder functions).
- **Contracts:** every type in CONTRACTS.md §2–§13 in `crates/core/src/contracts/`, with serde, `schema_version` checks, and hand-written `Debug` for password-holding types.
- **Shared helpers:**
  - `fsutil::write_json_atomic`, `fsutil::set_read_only`, and `fsutil::path_within(a, b)` (canonicalizes for comparison only).
  - `hashing::sha256_file(path) -> String` (streaming, read-only).
  - All unit-tested.
- **Fixtures and types:**
  - `cargo xtask contracts` writes example JSON for every file format and IPC type to `ui-dev/fixtures/contracts/`.
  - `ui/types.d.ts` mirrors all IPC types.
  - `ui-dev/mock.js` stub returns the fixtures.
  - Round-trip tests.
- **CI:** the `contracts-drift` job goes live.

**Accept:**
- CI is green.
- The PR description demonstrates that renaming a Rust field without regenerating the fixtures fails `contracts-drift`.
- The `run.json` example matches CONTRACTS.md §7.1 field-for-field, and the initial-record example matches §7.2.

**After M0.3, contracts are frozen** (DEVELOPMENT §4.7).

---

## Track A: LEAPP manager (`manifest`, `leapp::*`, `xtask`)

### A1 Manifest and pinning

- **Manifest:** `leapp-manifest.json` for both tools and all six platforms, from LEAPP-CLI.md §1. Zip `entry_sha256` values are computed by `pin-leapp --download-verify`; AppImage values stay `null`. Embedded via `include_str!`.
- **Platform detection:** `PlatformKey` from `cfg!(target_os/target_arch)`.
- **`cargo xtask pin-leapp --tool <t> --tag <tag> [--download-verify]`:**
  - Queries the GitHub releases API (`ureq`).
  - Builds the **expected** asset names from a per-platform template table. It fails listing any missing, extra or ambiguous names, and refuses drafts and prereleases.
  - Writes the name, size, digest and URL.
  - With `--download-verify`: downloads each asset, checks the digest, extracts the zip entry and writes `entry_sha256` (downloads ~700 MB; use the scratch dir).

**Accept:**
- Unit tests for parsing, platform selection, and unsupported platforms.
- Re-running `pin-leapp --download-verify` for the pinned tags reproduces the committed manifest byte-for-byte.

### A2 Install / import / verify

- **Download:** manifest URLs in order, `https_only(true)`, a body limit of `asset_size`, progress callback. Verify the asset SHA-256 before extraction.
- **Extract:**
  - **zip:** extract **only** `entry`; reject absolute paths and `..`; set mode 0755 on Unix.
  - **appimage:** chmod +x, run `--appimage-extract` in the version dir (Linux only, via `std::process`), and confirm `squashfs-root/<entry>` exists.
- **Verify the entry** against the manifest `entry_sha256` (mismatch → `hash_mismatch`). Where the manifest has `null`, record it in `install.json`.
- **Install atomically:** into `<tools_dir>/<tool>/.staging-<rand>`, then rename to `<version>`. A failed install leaves nothing behind.
- **Offline import:** same pipeline from a local file. **Verify:** re-hash the entry.
- **Paths:** honour the `tools_dir` override.
- **Introspection:** a callback is left for A3 (install is "complete" only after `modules.json` exists).

**Accept** (tests on CI):
- A local test HTTP server (plain HTTP to 127.0.0.1, permitted only under `cfg(test)`) serves a generated zip.
- Cases: hash mismatch; size overflow; zip-slip entry; truncated download; successful install; offline import; verification after tampering (flip a byte → `verification_failed`).
- AppImage extraction is tested on Linux with a fake AppImage shell script that implements `--appimage-extract`.

### A3 Module introspection (needs A2 + B2)

- **Introspection:** per LEAPP-CLI.md §5, via `process` (session/job, temp dir, 180 s timeout). Produces `modules.json` (CONTRACTS.md §5) with the tool rules for selection filtering, `always_run` and `timezones`.
- **Smoke tests:** `crates/core/tests/leapp_smoke.rs` (all `#[ignore]`) installs the real pinned tools for the host platform and introspects. It asserts count ≥ 500, that known names exist (`callHistory` for iLEAPP; pick a stable one for aLEAPP), that `timezones` contains `America/Chicago` for iLEAPP and is `null` for aLEAPP, and prints `entry_sha256`.
- **`.github/workflows/leapp-smoke.yml`:**
  - Triggers: `workflow_dispatch` only while private. When public, add a weekly schedule and `pull_request` paths (`leapp-manifest.json`, `crates/core/src/leapp/**`, `crates/core/src/process/**`, `crates/core/tests/leapp_smoke.rs`, the workflow file).
  - Runners: `ubuntu-24.04` + `ubuntu:22.04` container while private. When public, also `macos-15`, `macos-15-intel`, `windows-2025`, and `ubuntu-24.04-arm` + container.

**Accept:**
- Smoke tests pass locally on macOS arm64 and on the Windows machine (output in the PR, with local paths redacted).
- The smoke workflow (Linux, via `workflow_dispatch` on the PR branch) is green.
- iLEAPP and aLEAPP counts are identical across the three platforms.

---

## Track B: Process runner (`process`, `tail`, `bin/fake-leapp`)

### B1 fake-leapp

Implement `crates/core/src/bin/fake-leapp.rs` with every behavior and scenario in DEVELOPMENT §4.8, producing the outcomes in CONTRACTS.md §7.4.

**Accept:** unit tests for argument parsing and output-layout generation; each scenario run manually, with the resulting tree listing shown in the PR.

### B2 Spawn / cancel / temp

- **API:** `process::spawn(spec) -> Handle` with `Handle::{wait, cancel}` per ARCHITECTURE §7 (Unix `setsid`; Windows suspended → job → resume, `CREATE_NO_WINDOW`).
- **I/O:** stdin null; stdout/stderr streamed to files by reader threads.
- **Temp dirs:** create the per-run temp dir and set `TMPDIR`/`TEMP`/`TMP`. Remove it after exit, retrying on Windows sharing violations for up to 5 s. `sweep_stale_temp(app_cache)` handles leftovers.

**Accept** (integration tests using `CARGO_BIN_EXE_fake-leapp`, all three OSes):
- `success` → exit 0, files captured.
- `slow` + cancel → both PIDs dead within 12 s, temp dir removed, `escalated_to_kill == false`.
- `ignore_term` + cancel (Unix) → both dead within 12 s, `escalated_to_kill == true`.
- Killing only the parent with SIGKILL (Unix) is still cleaned up by the group kill.
- `prompt` exits within 5 s.
- `argparse_error` → exit 2.

### B3 Tail and streaming

- **`tail::ScreenOutputTail`:**
  - Polls the path and tolerates the file not existing yet.
  - Splits records on `<br>` + `\n` or `\r\n`, keeping partial records.
  - Decodes UTF-8 lossily and strips tags.
  - Truncates lines over 8 KiB with `…`.
- **Streaming:** `process` + `tail` emit `log` batches through a callback. On exit, drain the tail and read the last 200 lines of stdout/stderr.

**Accept:**
- Unit tests for chunk-boundary splits, CRLF, tags, invalid UTF-8 and huge lines.
- Integration test: fake-leapp `success` with 50 lines at 100 ms → at least 40 lines are delivered before the process exits (timestamps compared).

---

## Track C: Cases and records (`settings`, `paths`, `case`, `run::*`, `hashing`, `inspect`)

### C1 Settings, paths, cases

- **Settings and paths:**
  - `settings.json` load/save; defaults on first run (`cases_root = <Documents>/suiteDFIR Cases`, passed in).
  - `paths::AppPaths` bundle (constructed by the shell).
- **Cases:**
  - Create (naming and collision rules, CONTRACTS §6), open (validate `case.json`, mark it known), update (atomic; editable fields only), recent list (dedupe, most recent first, max 50), forget.
  - Run discovery via `runs/*/run.json`, skipping unreadable ones with a warning.

**Accept:** unit tests for naming (reserved characters, trailing dots, Windows reserved names, collisions), atomic write (the target is never truncated when a crash is simulated between write and rename), schema-version rejection, and recent-list rules.

### C2 Run record, status, argv, profiles, case data

- **Run record lifecycle:**
  - `run_id` generation.
  - Initial record (CONTRACTS §7.2), finalize (atomic, then read-only: Unix 0444, Windows readonly attribute), and a guard against finalizing twice.
  - Recovery (CONTRACTS §7.2).
- **Status:** `run::status` implementing CONTRACTS §7.3 exactly, with table tests covering every rule, warning and §7.4 row.
- **argv:** `run::argv` builds the LEAPP argv (LEAPP-CLI §4) and the redacted copy.
- **Profiles and case data:** profiles read/write/validate/import/export; `ModuleSelection` → (`requested`, `resolved`, `unknown`) including mode `all`; `.lcasedata` writer.

**Accept:**
- Every status rule and §7.4 row has a test.
- A test proves the password never appears in the serialized `run.json`, in `Debug` output or in error messages.
- Finalize twice is rejected.

### C3 Hashing and inspection

- **Input hashing:** `hashing::sha256_file_with_progress` (1 MiB buffer, ≤ 10 progress callbacks/s, cancellable).
- **Report seal:** `hashing::seal_report` writes `report.sha256` per CONTRACTS §8 (GNU escaping, sorted, symlinks counted, not listed) and returns the manifest hash, count, bytes and warnings.
- **`inspect`:**
  - Kind and size.
  - Type detection: a directory with `Manifest.db`/`Manifest.plist` → `itunes` (iLEAPP), else `fs`. `.zip` → `zip`, `.tar` → `tar`, `.gz`/`.tgz` → `gz`, `.e01`/`.dd`/`.img`/`.bin`/`.raw`/`.001` → `raw`, any other file → `file` (iLEAPP) or `invalid_input` (aLEAPP).
  - `allowed_types` = the tool's `input_types` (passed in as a parameter, not read from `manifest`) ∩ the kind-compatible types.
  - `IsEncrypted` from `Manifest.plist`.
  - The overlap rule via `fsutil::path_within`.
  - Permission errors → `permission_denied`.

**Accept:**
- The hash of a generated 64 MiB file matches a constant computed in the test setup with an independent method (documented).
- On Linux CI, `sha256sum -c report.sha256` passes for a report tree containing names with spaces, `\` and a newline.
- The detection and overlap tables are fully unit-tested.

---

## Track D: UI (against the mock)

### D1 Shell and mock

- **Shell:** top bar (app name, active-run indicator, nav: Cases, Settings), hash router, `lib/store.js`, `lib/dom.js`, `lib/format.js` (sizes, durations, local/UTC time).
- **API layer:** `api/index.js` with the activation rule from ARCHITECTURE §5.3, and `api/ipc.js`.
- **Mock:** `ui-dev/mock.js` implements **every** command in CONTRACTS §10 and §13.5, with simulated runs and acquisitions reaching every final status.
- **Chrome:** the `AppError` component; MOCK DATA / DEV OVERRIDE banners; light/dark tokens.
- **Playwright script:** kept under `tests/ui/e2e/shots.mjs` and run on a machine with a browser (not in `npm test`). It loads mock mode via `serve-ui.mjs`, captures screenshots, and **fails on any console CSP violation**.

**Accept:**
- Node parity test (identical exports in `ipc.js` and `mock.js`); typecheck clean.
- Screenshots of the empty shell (light and dark) and of an `AppError`, with no CSP violations.

### D2 Cases and Case screens

- **Cases screen:**
  - Recent list with a missing-folder state and a **Forget** action.
  - New case form (validation), Open case folder (dialog).
  - Empty state; a tools-not-installed banner.
- **Case screen:**
  - Metadata with edit (editable fields only).
  - Runs table: local time with UTC on hover; status badges with text, not colour alone.
  - Row actions: open report, reveal folder (`run_dir`), details (key facts from `run.json` plus the raw JSON as text).

**Accept:** node tests for sorting and formatting; screenshots of every listed state.

### D3 New run, module picker, profiles

- **New run form** (sections in order):
  - **Tool:** installed tools only.
  - **Input:** "Choose file…" / "Choose folder…". Shows the inspection result, with a type override limited to `allowed_types`; overlap and permission errors are shown inline.
  - **Options:**
    - Timezone select (DEVELOPMENT §4.6 source order; default case → settings → UTC; iLEAPP only).
    - Password field when encrypted: required, `autocomplete="off"`, cleared after start.
    - Keychain file (iLEAPP).
    - Hash-input checkbox (file inputs; default on).
    - Label.
  - **Modules:** All / Profile / Custom.
- **Module picker:**
  - Search, category groups with tri-state checkboxes, select all/none, selected count.
  - Unknown modules listed, with "remove unknown".
  - Save as profile; import (open dialog); export (save dialog).
- **Start:** disabled until the form is valid, with inline reasons.

**Accept:** node tests for selection logic (tri-state, filter, unknown diff); render + filter of 1,300 fixture modules < 100 ms (measured in the PR); screenshots.

### D4a Run screen

- **Layout:** header (tool, input, label), phase, elapsed time.
- **Log:** virtualized log (auto-scroll toggle, copy all).
- **Cancel:** cancel with an in-DOM confirm `<dialog>`.
- **Progress:** hash/seal progress bars.
- **Result panel:** status, reasons, warnings, module counts; "Parser output" (`stdio_tail`); open report / reveal folder / open stdout / stderr / `run.json`.
- **Reloads:** `job_attach` on reload.

**Accept:** virtual-list math tests; a 100,000-line mock run stays interactive (measurement documented); screenshots of every `RunStatus` and every phase.

### D4b Settings screen

- **Per-tool card:** pinned version; `ToolState` (all six values); install/verify/import with a progress bar and install errors; module count; install dir.
- **Other settings:** defaults form, tools-dir override (reset = `null`), clean temp files.
- **About:** versions; licenses (`licenses_get`, rendered as preformatted text); privacy statement.

**Accept:** screenshots of every `ToolState`, plus installing and install-failed states.

### D5 Acquire screen (against the mock)

- **Entry points:** "Acquire iOS backup" on the Case screen. The Case screen gains an **Acquisitions** table (status, device, iOS version, date; actions: reveal folder, details, open log, "Parse with iLEAPP").
- **Device list:**
  - `devices_list` polled every 2 s while the screen is visible.
  - Tools-state banner with platform guidance (`missing`, `usbmuxd_unavailable`, `verification_failed`).
  - Per-device card: name, model, iOS version, serial, pair state with instructions ("Unlock the device and tap Trust", then Pair/Retry), encryption state.
- **Options:**
  - Label.
  - If not encrypted:
    - "Enable backup encryption (recommended)", with an explanation that this changes a device setting and yields more data.
    - Password entered twice.
    - "Turn encryption off again afterwards" (default on).
  - If already encrypted: a warning that the owner's password is needed to parse.
- **Progress view:**
  - Phase, percent bar, bytes, elapsed time, log.
  - A banner: "Watch the device: it may ask for the passcode".
  - Cancel with an in-DOM confirm.
- **Result panel:**
  - Status, reasons, warnings (especially `encryption_restore_failed`).
  - Open folder, `acquisition.json` and logs.
  - "Parse with iLEAPP" (enabled on `succeeded`). It pre-fills New run and carries the password in UI memory only when the examiner set it in this session; cleared per CONTRACTS §13.5.
- **Password fields** are cleared after use.

**Accept:** node tests for the screen state logic (pair-state transitions, option validation, handoff clearing); screenshots of every `PairState`, every tools state, every `AcqStatus` and the encryption option variants, with no CSP violations.

---

## Track X: iOS acquisition (`idevice`, `acquire`, `bin/fake-idevice`, tool build)

### X1 Pinned libimobiledevice tool build

- **`idevice-tools.json`:** sources with SHA-256 per IDEVICE-CLI.md §1, plus any TLS/curl sources.
- **`scripts/build-idevice-tools.sh <platform-key>`:** downloads the pinned tarballs, verifies their hashes, and builds **static** libraries plus the four tools with `--without-cython`. Then:
  - **macOS arm64 and x64** (x64 cross-built on an arm Mac with `-arch x86_64`, or on `macos-15-intel`):
    - `MACOSX_DEPLOYMENT_TARGET=11.0`.
    - TLS via mbedtls (`--with-mbedtls`) or OpenSSL, built statically from pinned source.
    - libcurl for libtatsu may link the macOS system libcurl.
    - `otool -L` must list only `/usr/lib` and `/System` libraries.
  - **Windows x64:** MSYS2 UCRT64 on the `windows-2025` runner; static where possible. Any DLLs that can't be avoided are listed in `files` and shipped next to the tools.
  - **Every bundle** gets `BUILDINFO.json` (source URLs + hashes, compiler and toolchain versions, configure flags, build date) and the license texts.
- **`.github/workflows/idevice-tools.yml`** (`workflow_dispatch` only):
  - Builds the Windows x64 bundle (and macOS bundles when the owner accepts the minutes; otherwise they are built locally on the Mac).
  - Uploads the bundles to a **prerelease** named `idevice-tools-<version>` in this repo.
  - Agents may create and update these prereleases (ORCHESTRATION carve-out); they are build inputs, not app releases.
- **`cargo xtask fetch-idevice-tools`:**
  - Downloads the host platform's bundle; authenticates via `GH_TOKEN` or `gh auth token` while the repo is private.
  - Verifies `bundle_sha256` and every file hash.
  - Extracts to `src-tauri/binaries/` with Tauri sidecar names (`<tool>-<target-triple>[.exe]`).
- **`src-tauri/tauri.release.conf.json`:** `bundle.externalBin` lists the four tools. Windows DLLs, if any, go in `bundle.resources` placed next to the executable.

**Accept:**
- Bundles exist for `macos-aarch64`, `macos-x86_64` and `windows-x86_64`, and `idevice-tools.json` has all hashes.
- On the Mac, the fetched `idevicebackup2 --version` prints `1.4.0` and `otool -L` shows only system libraries.
- On the Windows machine, `idevicebackup2.exe --version` prints `1.4.0`, and `idevice_id -l` without Apple's service fails with a message (captured in IDEVICE-CLI.md §8, item 4).

### X2 `idevice` module and fake-idevice (needs B2)

- **fake-idevice:** `crates/core/src/bin/fake-idevice.rs` per DEVELOPMENT §4.8, covering every scenario in CONTRACTS §13.4.
- **`idevice` module:**
  - Tool lookup: dev override → bundled dir (verify hashes against `idevice-tools.json`) → `PATH` on Linux. The result carries the source and binary hashes.
  - `list_devices`, `device_info`, `validate`/`pair` → `PairState`, `will_encrypt`, `disk_usage`, `set_encryption(on|off, pw)`.
  - All commands run through `process` with a 20 s timeout.
  - Output parsing per IDEVICE-CLI.md §2–§4.
  - Tool and usbmuxd problems map to `IdeviceToolsState`.

**Accept:**
- Parser unit tests use the exact message strings from IDEVICE-CLI.md.
- Integration tests with fake-idevice cover `not_paired` → `awaiting_trust` → `paired`, `locked`, `trust_denied`, `usbmuxd_missing`, tools missing, and a tampered bundled tool (`verification_failed`).
- The gate is green.

### X3 Acquisition runner (needs X2 + C3)

- **`acquire` module:** ARCHITECTURE §6b end-to-end, including:
  - the `acquisition.json` lifecycle and recovery;
  - the free-space check (`statvfs` on Unix; `GetDiskFreeSpaceExW` on Windows, adding the `Win32_Storage_FileSystem` windows-sys feature);
  - progress parsing (split on `\r`/`\n`, regex `\]\s+(\d+)%`);
  - cancel with a 30 s grace;
  - validation and status per CONTRACTS §13.3;
  - encryption enable and restore;
  - sealing via a generalized `hashing::seal_tree(dir, manifest_name)`, shared with runs.

**Accept:**
- Every CONTRACTS §13.4 row is an integration test with the expected status, reasons and warnings, a finalized read-only `acquisition.json`, and `backup.sha256` when a backup exists.
- The password never appears in the record, in `Debug` output or in errors.
- Recovery marks a `running` acquisition `interrupted`.
- The gate is green.

---

## M2: Integration

### E1a Core runner

`core::runner` implements ARCHITECTURE §6 end-to-end, composing A, B and C. Events go through a callback.

**Accept** (integration tests with fake-leapp):
- Every §7.4 scenario produces a finalized, read-only `run.json` with the expected status, reasons and warnings, plus `report.sha256` when a report exists.
- Concurrent input hashing is exercised.
- A cancel after exit only cancels hashing (`input_hash_cancelled`).
- Overlap and `path_too_long` validation are covered.

### E1b Command layer and app state

- **Commands:** `src-tauri` implements every command in CONTRACTS §10 and §13.5 with channels and the path-policy checks (ARCHITECTURE §9). The single active job covers runs and acquisitions. The shell passes the bundled-tools dir (sidecar location) to the core.
- **`AppState`:** single active run and backlog.
- **Lifecycle:**
  - Instance lock (`another_instance_running` → native message, exit).
  - Quit guard (`CloseRequested` + `ExitRequested`, native dialog, cancel → wait ≤ 30 s → exit).
  - Startup temp sweep.
- **Other:** app file logger (no secrets); debug-only dev override; `licenses_get`.

**Accept:**
- Unit tests for the path policy (every row of ARCHITECTURE §9).
- Second `run_start` or `acq_start` while any job is active → `run_already_active`.
- A second-instance test for the lock helper.
- Logger redaction test.

### E2 UI ↔ real IPC

- **Wiring:** complete `api/ipc.js`.
- **Invoke recording:** a node script imports `ui/api/ipc.js` with a stub `window.__TAURI__` that records `invoke(name, args)` for a scripted flow covering every command, and writes `tests/ui/recorded-invokes.json`.
- **Replay:** a Rust test in `src-tauri` (with both dev overrides: fake-leapp and fake-idevice) uses the `tauri::test` mock runtime to replay each recorded invoke through the real command handlers, with the dev override and a test opener that records instead of opening. It asserts success and the response shape (validated against the contract types).

**Accept:**
- The replay test passes the gate; every command in CONTRACTS §10 and §13.5 appears in the recording.
- A human GUI check is scheduled (H4).

### E3 Real-LEAPP smoke (extends A3)

- **New runs in `leapp_smoke.rs`,** for both tools on every smoke platform:
  - a minimal synthetic `fs` fixture yielding ≥ 1 `Complete` module (document the fixture files; keep them tiny);
  - `-t itunes` on a non-backup folder → `failed` with `no_modules_ran`;
  - cancel mid-run → `cancelled`, no surviving process, temp removed;
  - a profile containing an unknown name → rejected before spawn.
- **Fixtures:** capture the resulting `_lava_data.lava` and `Screen_Output.html` samples into `fixtures/leapp/<tool>/<version>/`, with paths sanitized to `<RUN_DIR>`/`<INPUT>`. Extend the C2 status tests with them.
- **Manifest:** fill the AppImage `entry_sha256` values into the manifest.
- **Docs:** resolve every item in LEAPP-CLI.md §9 (update the doc per platform).

**Accept:** smoke runs are green on macOS arm64 (local), Windows x64 (local) and Linux x64 (workflow); LEAPP-CLI.md §9 is empty, or each remaining item explains why CI can't verify it and is added to the G2 checklist.

---

## M3: Packaging

### F1 Bundles and release workflow

- **Bundle targets:**
  - macOS: `.app` + `.dmg` per arch (`macos-15`, `macos-15-intel`), minimum 11.0.
  - Windows x64: NSIS online installer (default `downloadBootstrapper`) and an offline installer built with a config overlay `{"bundle":{"windows":{"webviewInstallMode":{"type":"offlineInstaller","silent":true}}}}` via `cargo tauri build --config <file>`. Rename the outputs to `suiteDFIR_<ver>_x64-online-setup.exe` and `…_x64-offline-setup.exe`.
  - Linux: AppImage + `.deb`, built in the `ubuntu:22.04` container.
- **iOS tools:** `cargo xtask fetch-idevice-tools` + `--config src-tauri/tauri.release.conf.json` bundle the pinned X1 tools as sidecars (macOS arm64/x64, Windows x64). Linux packages declare no dependency on them; the user guide explains installing them.
- **Builds** use `cargo tauri build --runner <abs>/scripts/cargo-auditable` (a wrapper that execs `cargo auditable "$@"`). The Windows `.cmd` wrapper must be verified; if it can't work, document that Windows builds are not auditable and proceed.
- **`release.yml`:**
  - `workflow_dispatch` = build-only dry run (artifacts uploaded to the run, no release).
  - Tag `v*` = build + `SHA256SUMS` + a **draft** GitHub release (never published by automation).
  - Signing/notarization steps run only when the secrets exist (H1); otherwise the release notes state that the builds are unsigned.

**Accept:** the dry run produces every artifact, with sizes recorded in the PR. The bundled tool hashes in the built app match `idevice-tools.json`. Targets:
- < 20 MB for the dmg, the online installer and the deb (includes the iOS tools);
- AppImage exempt (≈ 70+ MB, bundles WebKitGTK);
- offline installer ≈ 215 MB.

### F2 Third-party notices

- **`cargo xtask notices`** generates `THIRD-PARTY-NOTICES.md` from `cargo metadata`: each crate, its version and license, and the license text from registry sources. It also includes the LEAPP MIT notice, the LGPL notices for LEAPP's bundled libheif/libde265, and the libimobiledevice stack's GPL-2.0+/LGPL-2.1+ texts with the exact source tarball URLs and hashes from `idevice-tools.json` (the source offer).
- **`licenses_get`** embeds this file.
- **CI** checks that the file is up to date.

**Accept:** covers 100% of shipped crates (`cargo tree -e normal` for each release target triple).

---

## M4: Docs and QA

- **G1 `docs/USER-GUIDE.md`**, plus README polish. Covers:
  - install per OS (macOS Full Disk Access; Windows offline installer; the AppLocker tools-dir override; the Linux dmabuf fallback);
  - iOS acquisition prerequisites per OS (Windows: Apple Devices app; Linux: `usbmuxd` + `libimobiledevice-utils`), trust/pairing, backup encryption (why, and what the app changes on the device);
  - installing tools online and offline (TLS-intercepting networks → offline import);
  - running a parse, and what each status means;
  - verifying a report with `sha256sum -c report.sha256`;
  - privacy.
- **G2 `docs/QA-CHECKLIST.md`**, a human checklist:
  - real encrypted and unencrypted Finder backups (right and wrong password); **grep the report for the password**;
  - **USB acquisition** on macOS and Windows with a real iPhone: first-time trust, locked device, encryption enable + restore, a device already encrypted with an unknown password, cancel mid-backup, unplug mid-backup, then "Parse with iLEAPP" handoff; resolve IDEVICE-CLI.md §8;
  - Android fs extraction and zip; an E01;
  - a large input (≥ 100 GB) with hashing;
  - cancel in each phase; quit during a run; a second instance;
  - offline import on an air-gapped machine;
  - each installer on each OS;
  - a screen-reader spot check; dark mode;
  - any LEAPP-CLI §9 leftovers.

**Release candidate (RC):**
- All Must features accepted.
- The QA checklist executed by a human on macOS and Windows (Linux recommended).
- The draft release reviewed and published by the owner (H3).

---

## Should (after E2 and E3)

- **S1 iOS backup finder:**
  - `ios_backups_find` scans the default locations: macOS `~/Library/Application Support/MobileSync/Backup`; Windows `%APPDATA%\Apple Computer\MobileSync\Backup` and `%USERPROFILE%\Apple\MobileSync\Backup`.
  - It parses `Info.plist`/`Manifest.plist`, and gives `permission_denied` guidance for macOS Full Disk Access.
  - The New run screen gets a "Find iOS backups" UI.
- **S2 Log search/filter.**
- **S3 arm64 release builds:** Windows arm64 (online installer only) and Linux arm64 in `release.yml`; `windows-11-arm` joins the smoke matrix.

## Optional U: upstream contributions

Draft issues/PRs for LEAPP-CLI.md §8. **Owner approval before anything is posted upstream (H5).**

## Human checkpoints (owner only)

| # | When | What |
|---|---|---|
| H0 | Before M0 | Done by the planning session with owner approval:<ul><li>create the **private** repo `jacobecontreras/suiteDFIR-next`; the existing `suiteDFIR` repo is untouched;</li><li>`delete_branch_on_merge` off; squash-only merges;</li><li>ruleset on `main` if the account plan supports it for private repos (otherwise the orchestrator enforces the gate);</li><li>push the initial docs commit; clone to `~/suiteDFIR-next`;</li><li>set up the Windows test remote;</li><li>update `~/.claude/CLAUDE.md` and the auto-mode trusted repos.</li></ul> |
| H1 | Before F1 signing | Apple Developer ID certificate + notarization credentials; Windows signing (e.g. Azure Trusted Signing) or accept unsigned beta builds; add them as GitHub secrets. |
| H2 | Any time | Approve mirroring pinned LEAPP assets in this repo's releases (adds mirror URLs to the manifest). |
| H3 | RC | Publish releases (automation only creates drafts). |
| H4 | After E2 and at RC | A hands-on GUI check and the QA checklist on a real desktop session with real evidence. |
| H5 | Any time | Approve posting upstream issues/PRs. |
| H6 | After RC | Decide whether and how `suiteDFIR-next` replaces the public `suiteDFIR` (rename/visibility), and what happens to the legacy repo. |
| H7 | Before RC | Credits wording for prior contributors in the README. |
