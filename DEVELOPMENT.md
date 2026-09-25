# Development

Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) first. This file covers how to build and test, and the rules every change must follow.

## 1. Toolchain

| Tool | Version policy |
|---|---|
| Rust | Pinned in `rust-toolchain.toml` to a specific stable release (≥ 1.89 for `File::try_lock`; 1.98.x at planning time) with `components = ["rustfmt", "clippy"]`. On a new machine, first run `rustup toolchain install <pin> -c rustfmt,clippy`. |
| Tauri | `tauri` 2.11.x, `tauri-build` 2.6.x, `tauri-plugin-dialog` 2.x. Exact versions come from `Cargo.lock`. |
| Tauri CLI | Exactly **2.11.5**: `cargo install tauri-cli --version "=2.11.5" --locked`. The exact version is recorded in CI; bump deliberately. |
| Node.js | Dev-only (`tsc`, `node --test`, `scripts/serve-ui.mjs`). `.node-version` = 22 (the lowest in use), `engines.node` = `>=22`. |
| TypeScript | Exact version in `package.json` `devDependencies` (7.0.x), installed with `npm ci`. |
| cargo-deny | Exactly **0.20.2**. CI uses the prebuilt release binary checked against a pinned SHA-256; locally `cargo install cargo-deny --version "=0.20.2" --locked`. |
| cargo-auditable | Release builds only (see F1). |
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
node scripts/serve-ui.mjs [--root <dir>] [--port 5173]
                                         # serve <root>/ui + <root>/ui-dev (at /dev/) with the CSP from
                                         # <root>/src-tauri/tauri.conf.json (--root defaults to the repo root);
                                         # open http://127.0.0.1:5173/?mock for browser mock mode
node tests/ui/e2e/shots.mjs --root <dir> --out <dir> [--screens a,b]   # mock-mode screenshots, light and dark
                                         # (needs Playwright + Chromium; starts serve-ui itself; not in npm test)
cargo xtask pin-leapp --tool ileapp --tag v2026.4.2 --download-verify   # update leapp-manifest.json
                                         # (~350 MB per tool, downloaded to the OS temp dir or
                                         # --download-dir <dir> and deleted after checking)
cargo xtask contracts                    # regenerate ui-dev/fixtures/contracts/ (*.json + index.js)
cargo xtask notices                      # regenerate THIRD-PARTY-NOTICES.md
scripts/build-idevice-tools.sh <platform-key>  # build a pinned libimobiledevice tool bundle: macos-aarch64 / macos-x86_64
                                         # on the Mac, windows-x86_64 in a per-user MSYS2 UCRT64 shell (X1)
cargo xtask fetch-idevice-tools [--target <triple>]  # fetch + verify the pinned tool bundle into src-tauri/binaries/
                                         # (release builds; token: GH_TOKEN, else `gh auth token [--user $SUITEDFIR_GH_USER]`)
cargo test -p suitedfir-core --test leapp_smoke --locked -- --ignored --test-threads=1   # real LEAPP
cargo xtask fetch-idevice-tools && cargo tauri build --config src-tauri/tauri.release.conf.json   # release bundle (host)
```

The `xtask` alias lives in `.cargo/config.toml`.

**Debug-build dev override:** `SUITEDFIR_DEV_LEAPP_OVERRIDE=<path to target/debug/fake-leapp>` makes both tools report `ToolState.dev_override`:
- the version is `dev-override`;
- modules and timezones come from `fake-leapp --list-modules-json <tool>`;
- verification is skipped;
- runs record `install_source: dev_override`.

The UI shows a "DEV OVERRIDE" banner. The code is compiled out of release builds (`#[cfg(debug_assertions)]`).

Likewise, `SUITEDFIR_DEV_IDEVICE_OVERRIDE=<path to target/debug/fake-idevice>` (debug builds only) makes the core use fake-idevice for all four libimobiledevice tools (`IdeviceToolSource.dev_override`).

The bundled libimobiledevice tools are **not** needed for `cargo tauri dev` or `cargo tauri build --debug --no-bundle`. They are attached only by the release overlay config `src-tauri/tauri.release.conf.json` (`bundle.externalBin`). Debug builds find tools via the dev override, `PATH`, or `src-tauri/binaries/` if present.

## 3. Repository layout

```
Cargo.toml / Cargo.lock       workspace ([profile.dev.package.sha2] opt-level = 3)
rust-toolchain.toml  deny.toml  leapp-manifest.json  idevice-tools.json  .cargo/config.toml
.gitattributes (* text=auto eol=lf; *.png, *.ico, *.icns binary)  .editorconfig  .gitignore  .node-version
crates/core/                  suitedfir-core: all logic, no Tauri dependency
  src/{lib.rs, contracts/, fsutil/, hashing.rs, manifest.rs, leapp/, process/, tail.rs,
       settings.rs, paths.rs, case.rs, run/, inspect.rs, runner.rs, idevice.rs, acquire.rs}
  src/bin/fake-leapp.rs       test double (never bundled); tests use env!("CARGO_BIN_EXE_fake-leapp")
  src/bin/fake-idevice.rs     test double for the libimobiledevice tools (never bundled)
  tests/{process.rs, introspection.rs, runner.rs, acquire.rs, leapp_smoke.rs}
src-tauri/                    app shell: tauri.conf.json, tauri.release.conf.json (externalBin overlay),
                              capabilities/default.json, icons/, src/, binaries/ (gitignored; fetched tools)
xtask/                        pin-leapp, contracts, notices, fetch-idevice-tools
ui/                           SHIPPED frontend (frontendDist): index.html app.js styles/ lib/ api/ screens/ components/ types.d.ts
ui-dev/                       NOT shipped: mock.js (+ mock/), fixtures/contracts/{*.json, index.js} (generated),
                              fixtures/modules.js (large module lists)
tests/ui/                     node --test files for ui/ modules; e2e/shots.mjs (Playwright screenshots)
fixtures/leapp/<tool>/<ver>/  captured real-LEAPP outputs (paths sanitized to <RUN_DIR>, <INPUT>)
scripts/serve-ui.mjs          zero-dependency static server (ui/ at /, ui-dev/ at /dev/, CSP header)
scripts/cargo-auditable(.cmd) runner wrapper for release builds
scripts/build-idevice-tools.sh  scripted (not bit-reproducible) libimobiledevice build from pinned tarballs (X1)
.github/workflows/            ci-rust.yml, ci-js.yml, leapp-smoke.yml, release.yml
docs/                         ARCHITECTURE, CONTRACTS, LEAPP-CLI, IDEVICE-CLI, ROADMAP, USER-GUIDE, QA-CHECKLIST
```

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
| `serde` (derive), `serde_json` | | all |
| `sha2` | | core |
| `zip` | `default-features = false, features = ["deflate-flate2-zlib-rs"]` | core, xtask |
| `ureq` 3 | `default-features = false, features = ["rustls"]`; `https_only(true)`; size cap via `body_mut().with_config().limit(n)` | core, xtask |
| `plist` | `Value::from_reader` (XML and binary) | core |
| `time` | `formatting`, `parsing` | core |
| `getrandom` | | core |
| `libc` (unix), `windows-sys` (windows, features listed in ARCHITECTURE §7) | | core |
| `log` | facade; the file logger is our own | core, src-tauri |
| `thiserror` | | core, src-tauri |
| dev-only: `tempfile`; `tauri` feature `test` (E2) | | tests |

Transitive notes: `tauri-plugin-dialog` pulls in the `tauri-plugin-fs` crate (never registered), and `ureq`'s rustls pulls in `webpki-roots`.

Anything else needs a PR labelled `new-dependency` that explains why std or an allowed crate cannot do it, with its transitive count (`cargo tree -e normal`). The orchestrator must approve it, and the table above is updated in the same PR.

`deny.toml`:
- **licenses** `MIT, Apache-2.0, Apache-2.0 WITH LLVM-exception, BSD-3-Clause, ISC, Zlib, Unicode-3.0, MPL-2.0, CDLA-Permissive-2.0`;
- **sources**: crates.io only;
- **`[advisories]`**: `unmaintained = "workspace"`. Transitive unmaintained crates in the Tauri/GTK stack are known and accepted; vulnerabilities are always errors.

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
  - Never log, persist or emit them. `itunes_password` exists only in `RunRequest` and in the spawned argv; redact it everywhere else.
  - Implement `Debug` by hand for types holding it.
  - The UI clears the password field after `run_start`.
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
- **Platform code:** lives only in `process/{unix,windows}.rs` and `fsutil/{unix,windows}.rs` (plus `inspect` for OS backup locations and `idevice` for tool lookup). The only permitted `unsafe` is FFI there, commented.
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

After M0.3, contract changes are coordinated by the orchestrator: no new tasks start in affected tracks until the contract PR merges, and in-flight branches then merge `main`.

### 4.8 Testing

**Unit tests:**
- Every function in `crates/core` with logic has unit tests.
- Status rules have table-driven tests, including every row of CONTRACTS.md §7.4, over synthetic outputs and (from E3) the captured fixtures in `fixtures/leapp/`.

**fake-leapp** (`crates/core/src/bin/fake-leapp.rs`) mimics a LEAPP onefile binary:
- **CLI:** accepts LEAPP's flags; writes the LEAPP output layout.
- **Process shape:**
  - The parent re-execs itself as `--fake-worker` and forwards SIGTERM (Unix).
  - Writes its own and the worker's PIDs to `FAKE_LEAPP_PIDFILE` if set (`parent <pid>` and `worker <pid>` lines, written atomically).
- **Output behavior:**
  - The worker appends `Screen_Output.html` records at `FAKE_LEAPP_INTERVAL_MS` (default 100) for `FAKE_LEAPP_LINES` (default 50) lines.
  - stdout is fully buffered until exit.
  - Creates `$TMPDIR/_MEIfake<pid>` and removes it on graceful exit.
  - Writes `_lava_data.lava` at the end.
- **Module list:** `--list-modules-json <tool>` prints a small module list as a `ToolModules` JSON object with version `dev-override` (for the dev override).
- **Scenarios** (`FAKE_LEAPP_SCENARIO`): `success`, `artifact_error`, `invalid_input`, `early_exit`, `argparse_error`, `crash`, `prompt` (opens `/dev/tty` if possible, then reads stdin; EOF → traceback, exit 1), `slow`, `ignore_term` (ignores SIGTERM). Expected outcomes are in CONTRACTS.md §7.4.

**Process tests (all three OSes):**
- Log lines arrive incrementally.
- Cancel leaves **no** surviving process from the tree (check both PIDs).
- The per-run temp dir is gone.
- `prompt` exits within 5 s.

**fake-idevice** (`crates/core/src/bin/fake-idevice.rs`) emulates `idevice_id`, `ideviceinfo`, `idevicepair` and `idevicebackup2`:
- **Selection:** by `argv[0]` file stem (tests create **copies** named after the tools) or by the first argument (`fake-idevice idevicebackup2 …`).
- **Output formats:** follow docs/IDEVICE-CLI.md: XML plists for `-x`, `idevicepair` message lines, and `\r[==  ] NN% (x/y)` progress with explicit flush.
- **Backup layout:** writes a valid tiny layout (`Info.plist`, `Manifest.plist`, `Manifest.db`, `Status.plist` with `SnapshotState`).
- **Signals:** handles SIGTERM like the real tool.
- **Pairing semantics:** mirrors the real tools. `hostid` prints `(null)` without a host record, and `validate` without a record behaves like `pair` (it starts pairing), so tests can prove that polling never pairs.
- **Password handling:** reads passwords only from `BACKUP_PASSWORD_NEW`/`BACKUP_PASSWORD`, and fails if a password appears in argv.
- **Scenarios** (`FAKE_IDEVICE_SCENARIO`): CONTRACTS.md §13.4; state persisted between invocations in `FAKE_IDEVICE_STATE_DIR`.

**Real LEAPP:** `leapp_smoke` tests (ignored by default) install the pinned tools through the core and run introspection and fixture runs. They run in the `leapp-smoke` workflow.

**UI tests:**
- `node --test` over pure modules: store, filters, virtual-list math, selection/profile diff, formatting.
- A parity test: `ipc.js` and `mock.js` export identical function names.

**Screenshots:** UI PRs attach mock-mode screenshots (light and dark) of every changed screen state, delivered as described in §5.

## 5. Git and pull requests

- **Branches:** `task/<id>-<slug>`, with the task ID lowercase and no dots (`task/m01-scaffold`, `task/b1-fake-leapp`, `task/x3a-acquire-core`). One bundle (docs/ROADMAP.md "Execution bundles") = one PR. Bundles may be large; keep commits focused, one per contained task where practical.
- **Commits:** Conventional Commits (`feat(core): …`, `fix(ui): …`, `test: …`, `ci: …`, `docs: …`). Messages describe the change and contain no tool-attribution lines or co-author trailers for non-humans.
- **Draft first:**
  - Open PRs as **drafts**; CI skips drafts.
  - Mark ready for review (`gh pr ready`) only after the local macOS and Windows gates pass on the head commit. That triggers CI once.
- **Keeping current:**
  - **Never rebase or amend a pushed branch; never force-push.**
  - Merge `origin/main` into the branch **only when preparing to merge** (not every time `main` moves), then re-run the gate on the new head.
- **Gate evidence** is recorded as **commit statuses** on the head SHA:
  - `gate/macos` and `gate/windows`, posted by the gate tooling;
  - the CI checks on GitHub.
  
  Anything else (comments, PR text) is informational.
- **Merging:**
  - One merger, one PR at a time.
  - Squash only, pinned to the verified head: `gh pr merge <n> --squash --match-head-commit <sha>`. Never `--delete-branch`; the repo setting `delete_branch_on_merge` stays off; no branch is ever deleted.
  - **Pre-merge checks**, all on the same head SHA:
    - both gate statuses `success`;
    - CI checks passed;
    - `behind_by == 0` against `main` (compare API);
    - an independent review verdict naming that SHA;
    - no `.claude/`, `CLAUDE.md` or `AGENTS.md` paths in the diff.
  - **Post-merge check:** `main`'s tree must equal the head's tree. Otherwise stop merging and revert via a PR.
- **Docs-only PRs** (only `*.md` changes): no gate statuses are required. Review is still required.
- **Screenshots:**
  - Push PNGs to a separate, never-merged branch `shots/<task-id>` under `shots/<task-id>/`.
  - Link them in the PR body.
  - Reviewers fetch that branch and inspect the images.
- **PR description:** the task ID, what changed, how it was verified (commands + results), dependency changes (normally "none"), contract changes, deviations from the task spec, and screenshot links for UI.
- **Definition of done:**
  - acceptance criteria from docs/ROADMAP.md met and demonstrated;
  - the **verification gate** (§6) green on the PR head commit;
  - docs updated for any behavior or contract change;
  - an independent review with no unresolved blocking issues.

## 6. Headless development and the verification gate

The UI can be developed and screenshotted entirely in browser mock mode (`node scripts/serve-ui.mjs`, then open `/?mock`) using any headless browser. `?mock&scenario=<flags>` selects mock states (e.g. `empty`, `no_tools`, `dev_override`, `active_run`); the flags and the input-path and label suffixes that choose simulated outcomes are listed at the top of `ui-dev/mock.js`. Machines that cannot open GUI windows can still run every Rust test, including the fake-leapp and fake-idevice process tests.

### Verification gate while the repository is private (local-first)

GitHub Actions minutes are limited for private repositories (Windows minutes count about 2×, macOS about 10×), so the gate is split:

| Status / check | Where | What |
|---|---|---|
| `gate/macos` | a Mac, clean checkout of the head SHA | `npm ci`, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`, `npm run typecheck`, `npm test`, `cargo tauri build --debug --no-bundle` |
| `gate/windows` | a Windows machine, clean checkout of the head SHA | the same Rust commands (fmt, clippy, test, `cargo tauri build --debug --no-bundle`) |
| CI (`ci-rust.yml`, `ci-js.yml`) | GitHub Actions, Linux, on non-draft PRs and on push to `main` | Rust: fmt, clippy, tests, `cargo build --workspace --locked`, cargo-deny (prebuilt, hash-checked), contracts-drift, all in **one job** in an `ubuntu:22.04` container on `ubuntu-24.04`. JS: typecheck and tests. Each workflow has `paths` filters, so UI-only PRs skip Rust and vice versa. |

**CI rules:**
- **Tauri CLI:** CI does **not** install the Tauri CLI; `cargo build` compiles the app, including `tauri-build` config validation. `cargo tauri build` runs in the local gates.
- **Concurrency:** `concurrency` cancels superseded runs **for pull requests only**; runs on `main` always finish, because they seed the cache.
- **macOS/Windows on Actions:** the Rust workflow also has macOS and Windows jobs. A `workflow_dispatch` runs only the job(s) selected by its `os` input (`linux`, `macos`, `windows` or `all`). On pull requests and pushes to `main`, the Linux job always runs, and the macOS and Windows jobs run only once the repository is public (`!github.event.repository.private`). Draft pull requests run no CI jobs.
- **Dispatch-only workflows:** `leapp-smoke.yml` and `release.yml` are `workflow_dispatch`-only while private (Linux legs only). All other builds, including the iOS tools and the macOS/Windows release bundles, happen on local machines. Skeleton versions exist on `main` from M0.2, because dispatch requires the workflow file on the default branch. Later tasks dispatch their branch's version with `--ref <branch>`.
- **Artifacts:** uploaded with `retention-days: 1`.

### Platform test caveats

- **Windows symlinks:** creating symlinks requires Developer Mode or admin. Tests that create symlinks must **skip with an explicit message** on `ERROR_PRIVILEGE_NOT_HELD` (1314), never fail silently or pass vacuously.
- **fake-idevice on Windows:** tests make fake-idevice tool names by **copying** the binary, never by symlinking.
- **Windows paths:** keep test paths short; long-path support may be disabled on the machine.
- **Linux CI runs cargo unprivileged:** the Linux job runs every cargo step as an unprivileged user, never root, so permission and read-only tests are real there.
