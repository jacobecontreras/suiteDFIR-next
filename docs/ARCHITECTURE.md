# Architecture

This document is the source of truth for **what** suiteDFIR phase 1 is and **why** it is built this way. File formats and the UI↔core API are in [CONTRACTS.md](CONTRACTS.md). Verified upstream parser behavior is in [LEAPP-CLI.md](LEAPP-CLI.md). If code and this document disagree, fix one of them in the same PR.

## 1. Purpose and principles

suiteDFIR is a single-user desktop app that makes running iLEAPP (iOS) and aLEAPP (Android) **reliable and auditable**, and can take an iOS backup over USB (libimobiledevice) straight into a case. The parsers do the forensic work. suiteDFIR adds what their command line lacks:
- case organization;
- verified tool identity;
- correct success/failure detection;
- safe cancellation;
- live progress;
- a complete run record.

Principles, in priority order:

1. **Never alter evidence.** The app never writes inside an input path (for acquisition's unavoidable device writes, see §6b). Inputs are opened read-only for hashing and inspection. The parsers read inputs and write only to the run folder. Runs whose output would land inside the input are refused (§6 step 1).
2. **Record everything needed to defend a result:** which tool binary ran, with which parameters, against which input, when, and with what outcome.
3. **Tell the truth about outcomes.** Never report success because a process exited 0. Derive status from the parser's actual output (LEAPP-CLI.md Q3).
4. **Local and private.** No telemetry. The only network use is downloading pinned parser builds on explicit user action.
5. **Minimal dependencies and footprint.** Every dependency is a supply-chain and maintenance cost (allowlist in DEVELOPMENT.md §4.2).
6. **Boring, stable technology.** Plain HTML/CSS/JS in the UI, and a small Rust core.

## 2. Scope

### Phase 1: Must

| ID | Feature |
|---|---|
| F1 | **Cases:** create, open, edit metadata, list recent, forget (remove from the list only). A case is a folder (§8). |
| F2 | **Tool management:** show pinned iLEAPP/aLEAPP versions; download, SHA-256 verify and install; offline import of the release asset file; binary verification against the manifest before every run; module and timezone lists per installed version. |
| F3 | **New run.** Pick tool and input (file or folder). The input type is auto-detected, with a manual override limited to valid types. Options: <ul><li>iTunes backup encryption detection, with a password prompt;</li><li>optional keychain file (iLEAPP);</li><li>explicit timezone (iLEAPP);</li><li>module selection (all / saved profile / custom);</li><li>optional input hashing (file inputs).</li></ul> |
| F4 | **Run execution:** one active job (run or acquisition) at a time, app-wide; live log; elapsed time; cancel with a guaranteed-clean process tree; a per-run temp directory. |
| F5 | **Run result:** status from output analysis; module summary (complete / error / no files); open the report in the system browser; reveal the run folder; view the parser's stdout/stderr. |
| F6 | **Audit record:** `run.json` per run (CONTRACTS.md §7), made read-only once final, plus the report hash manifest `report.sha256`. |
| F7 | **Profiles:** save, import and export in LEAPP's native `.ilprofile`/`.alprofile` format. Validated against the installed version's module list; unknown modules are shown and block the run. |
| F8 | **Case data to LEAPP:** case metadata is passed via `-d` `.lcasedata`, so the LEAPP report shows the case number, agency and examiner. |
| F9 | **Safety:** <ul><li>single app instance;</li><li>a native confirm before quitting during a run or acquisition;</li><li>runs and acquisitions left `running` by a crash are marked `interrupted` on the next open;</li><li>stale temp directories are swept at startup.</li></ul> |
| F10 | **Settings:** cases root folder; default examiner, agency and timezone; tools-directory override (for locked-down machines); clean temp files; about/licenses. |
| F11 | **iOS backup acquisition over USB** (libimobiledevice), described in §6b and [IDEVICE-CLI.md](IDEVICE-CLI.md). <ul><li>**Devices:** list connected iOS devices with name, model, iOS version and serial; pairing/trust flow with clear on-device instructions.</li><li>**Encryption:** show the device's backup-encryption state. Optionally enable encryption with an examiner-chosen password; encrypted backups contain more data. Optionally restore the setting afterwards (default on).</li><li>**Backup:** full backup into the case, with live progress and cancel. Success is validated from the backup contents.</li><li>**Record:** `acquisition.json` audit record plus a `backup.sha256` manifest.</li><li>**Handoff:** "Parse with iLEAPP" opens New run prefilled with the backup.</li><li>**Platforms:** macOS (bundled tools), Windows x64 (bundled tools; needs Apple Mobile Device Service), Linux (system-installed tools).</li></ul> |

### Phase 1: Should (after E2 and E3 are merged, before the release candidate)

| ID | Feature |
|---|---|
| S1 | "Find iOS backups": list local Finder/iTunes backups from the default OS locations (device name, iOS version, date, encrypted), selectable as input. |
| S2 | Log view search/filter. |
| S3 | Windows arm64 and Linux arm64 release builds. x64 builds and macOS on both architectures are Must. |

### Explicitly out of scope for phase 1 (do not build, do not scaffold)

- **Analysis features:** maps/geolocation, timeline, dashboards/charts, database viewer, photo/semantic search, tasks/notes.
- **Case handling:** case export/zip; migration of legacy suiteDFIR data; deleting runs or cases from the UI.
- **Acquisition:** Android/ADB acquisition, Wi-Fi (network) iOS acquisition, iOS acquisition on Windows arm64, and anything beyond a full iTunes-style backup (no filesystem/AFC dumps, no crash logs, no sysdiagnose).
- **App infrastructure:** app auto-update; telemetry/analytics; multi-user/auth; a local HTTP server.
- **LEAPP integration:** viewing LEAPP reports inside the app window; LEAPP GUI builds; `--custom_artifacts_path` UI (it is used only internally for introspection); `--html_row_limit` UI.
- **Hashing:** MD5/SHA-1.

## 3. Decision log

Each decision is final for phase 1 unless the owner reopens it. Do not relitigate these in PRs.

| # | Decision | Rationale |
|---|---|---|
| D1 | **No Python, no Electron.** Run upstream's prebuilt, standalone LEAPP CLI binaries as subprocesses. | The legacy app shipped ~0.5 GB and a stale vendored LEAPP. Upstream publishes per-platform CLI builds with GitHub SHA-256 digests. |
| D2 | **Tauri 2** (`tauri` 2.11.x, `tauri-build` 2.6.x, `tauri-cli` pinned exactly), Rust core, system webview. | ~4 MB binary, first-class offline installers, signing/notarization tooling. Wails v2 had fewer deps but no turnkey offline Linux/Windows installers. |
| D3 | **Frontend: plain HTML/CSS/ES modules.** No framework, no bundler, zero runtime npm packages. JSDoc types are checked by `tsc --noEmit` (dev-only). | ~5 screens do not justify React/Vite. Tauri serves a static folder. |
| D4 | **All logic in a Tauri-free crate (`crates/core`).** `src-tauri` is a thin command layer. | Fast headless tests; the same core drives CI smoke tests against real LEAPP. |
| D5 | **No Tauri shell/fs/http/opener permissions for the webview.** Processes, files and downloads are implemented in Rust behind our own commands. The `dialog` plugin is registered (JS open/save dialogs). `tauri-plugin-opener` is used only through its Rust free functions and is **not registered**. (`tauri-plugin-dialog` depends on the `tauri-plugin-fs` crate; that plugin is never registered.) | Page JS cannot spawn processes or read arbitrary files. |
| D6 | **Pinned tool manifest** (`leapp-manifest.json`, embedded at build time): exact asset name, size, SHA-256 and extracted-entry SHA-256 per platform. Updating LEAPP is a deliberate PR bumping the manifest. | Reproducibility; tool identity is anchored in the signed app binary, not in files beside the tool (LEAPP-CLI.md Q9). |
| D7 | **The live log comes from tailing `Screen_Output.html`, not stdout.** stdout/stderr are captured to files. | The frozen binaries block-buffer piped stdout and ignore `PYTHONUNBUFFERED` (LEAPP-CLI.md Q1). |
| D8 | **Run status is derived from `_lava_data.lava`, `index.html` and exit info**, never the exit code alone (CONTRACTS.md §7.3). | LEAPP exits 0 on invalid input and on failed artifacts (Q3). |
| D9 | **Kill the whole tree.** Unix: new session via `setsid`; on cancel, SIGTERM the group, wait up to 10 s, then SIGKILL. Windows: Job Object with kill-on-close, assigned race-free, plus `CREATE_NO_WINDOW` (§7). | Onefile binaries run as two processes; killing only the parent orphans the worker (Q2). `setsid` also removes the controlling terminal, so LEAPP can never block on `/dev/tty` (Q5). |
| D10 | **Per-run temp directory** (`TMPDIR`/`TEMP`/`TMP` point to an app-owned folder), deleted after exit. Stale ones are swept at startup (never while another instance is live; see F9). | Each run extracts ~130 MB of runtime; hard kills leak it. |
| D11 | **Module and timezone lists come from the pinned binary itself** (an introspection run, LEAPP-CLI.md §5), cached per installed version. | Static source parsing misses computed artifacts; the binary is the ground truth, including its `pytz` zone list. |
| D12 | **Profiles are validated before every run.** Unknown names block the run until removed. The resolved module list is recorded. | LEAPP silently drops unknown names (Q4). |
| D13 | **Cases and runs are plain JSON files; no database.** `run.json` becomes read-only once finalized. `case.json` is written atomically. | Transparent, portable, diffable, no migrations. |
| D14 | **One active job, app-wide (see D25); one app instance** (lock file). | LEAPP runs are heavy. This removes concurrency hazards in case folders and temp sweeping. |
| D15 | **Passwords are never persisted.** LEAPP gets `--itunes_password` in argv (its only non-interactive channel; visible in `ps`; redacted in `run.json`). `idevicebackup2` gets passwords via env (`BACKUP_PASSWORD_NEW` / `BACKUP_PASSWORD`), never argv. stdin is null and there is no controlling terminal, so no tool can block on a prompt. | Minimize exposure per tool; accepted and documented. |
| D16 | **Partial output of cancelled/failed runs is kept and marked**, never auto-deleted. | Transparency; the examiner decides. |
| D17 | **LEAPP HTML reports open in the system browser, never in the app webview.** | Reports contain evidence-derived HTML/JS; the app webview has IPC access. |
| D18 | **SHA-256 only.** | One algorithm, one small crate. |
| D19 | **The timezone is always explicit for iLEAPP** (`-tz`, default from case → settings → `UTC`), validated against the installed iLEAPP's own zone list. aLEAPP has no timezone option; the record says so. | LEAPP silently defaults to UTC. |
| D20 | **App identifier `com.suitedfir.desktop`, product name `suiteDFIR`, version 0.2.0** for the first phase-1 release. | An identifier ending in `.app` makes macOS show data folders as bundles. |
| D21 | **iOS acquisition uses the libimobiledevice CLI tools** (`idevice_id`, `ideviceinfo`, `idevicepair`, `idevicebackup2`) as subprocesses, via the same `process` module as LEAPP (session/job, cancel escalation with a configurable grace: 30 s for backups; no timeout for encryption changes, which may wait for the device passcode). | The proven, open toolset; the tools abort gracefully on SIGTERM and flush progress (IDEVICE-CLI.md). |
| D22 | **Tool binaries are built from pinned upstream source tarballs** by our own scripted build (X1), for macOS arm64/x64 and Windows x64. The **unsigned** bundle and its files are pinned by SHA-256 in `idevice-tools.json`; `fetch-idevice-tools` enforces them. The tools ship as Tauri sidecars in release builds only. Code signing changes the bytes, so at runtime the app records each tool's observed hash and verifies it (`tools.verified_against`):<ul><li>unsigned builds: against the manifest;</li><li>signed macOS builds: via `codesign --verify --strict`;</li><li>otherwise: records only.</li></ul>**Linux uses the distro's tools** (usbmuxd must be system-installed anyway). No binaries of unknown provenance, including the legacy repo's, are ever shipped. | Forensic defensibility and GPL source obligations. |
| D23 | **Enabling backup encryption is an explicit, recorded examiner action.** The password is never stored. The app turns encryption off again afterwards (default), records the result, and offers a later restore if that fails. It never resets device settings. | Encryption changes device state; the record must show it, including crash and disconnect cases. |
| D24 | **Acquisitions live in the case** (`acquisitions/<acq_id>/`) with their own audit record (`acquisition.json`) and manifest (`backup.sha256`). Runs may use them as input. | Keeps acquisition and analysis provenance together. |
| D25 | **One active *job* app-wide**, where a job is a run or an acquisition. | Same rationale as D14; a backup and a parse must not compete for the same disk and device. |

## 4. System overview

```
┌──────────────────────── suiteDFIR (one process) ────────────────────────┐
│  System webview: ui/ (static HTML/CSS/JS, shipped)                       │
│    screens ─► api/index.js ─► api/ipc.js (window.__TAURI__ invoke)       │
│                          └─► /dev/mock.js (browser mock mode only;       │
│                               lives in ui-dev/, never bundled)           │
│                     ▲ Channel<RunEvent | InstallEvent>                   │
│  ───────────────────┼──────────────────────────────────────────────────  │
│  src-tauri: commands/*.rs (thin) ─► AppState (settings, active run,      │
│             backlog, instance lock, quit guard)                          │
│  crates/core: contracts manifest leapp::{install,modules} inspect case   │
│               settings paths fsutil hashing run::{record,status,argv,    │
│               profile,casedata} process tail runner idevice acquire      │
└───────────────┬──────────────────────────────────────────────────────────┘
                │ spawn: new session / job object, stdin=null, no window
                ▼
        LEAPP CLI binary (PyInstaller onefile: bootloader ─► python worker)
        reads: input (read-only)      writes: <run>/report/**
        libimobiledevice tools (idevice_id/ideviceinfo/idevicepair/idevicebackup2)
        talk to the device via usbmuxd   writes: <acq>/backup/**
```

## 5. Components

### 5.1 `crates/core` (library, no Tauri dependency)

| Module | Responsibility | Task |
|---|---|---|
| `contracts` | All serde types from CONTRACTS.md. | M0.3 |
| `fsutil` | `write_json_atomic`, read-only marking, path-overlap checks (canonicalize for comparison only), `free_space` (`fsutil/unix.rs`, `fsutil/windows.rs`). | M0.3 (+C1) |
| `hashing` | `sha256_file` (M0.3). Progress/cancel variant and `seal_tree(dir, manifest_name, cancel)`, used for `report.sha256` and `backup.sha256` (C3). | M0.3, C3 |
| `manifest` | Parse the embedded `leapp-manifest.json`; `PlatformKey` detection. | A1 |
| `leapp::install` | Download (HTTPS only, size-capped, progress) → verify asset hash → extract (zip entry only; AppImage via `--appimage-extract`) → verify the entry hash against the manifest (or record it where the manifest has `null`) → `install.json`. Offline import; `verify`. | A2 |
| `leapp::modules` | Introspection run → `modules.json` (modules, always-run, timezones). | A3 |
| `process` | Spawn in a new session/job, env, cwd, stdin null, stdout/stderr to files; `cancel()` with escalation; `wait()` → `ExitInfo`; temp dir create/remove/sweep. `unix.rs` / `windows.rs`. | B2 |
| `tail` | Poll-based tail of `Screen_Output.html` → plain-text line batches. | B3 |
| `settings`, `paths`, `case` | `settings.json`; app-dir bundle (passed in from the shell; the core never guesses OS dirs); case create/open/update/list/recent; run discovery. | C1 |
| `run::{record,status,argv,profile,casedata}` | `run.json` lifecycle and recovery; status rules; argv building and redaction; profiles; `.lcasedata`. | C2 |
| `inspect` | Input inspection and type detection; iTunes backup and `IsEncrypted`; (S1) backup discovery. | C3 |
| `runner` | One run end-to-end (§6) via a callback; no Tauri types. | E1a |
| `idevice` | Locate the tools (bundled sidecar dir passed in by the shell, or system PATH on Linux; binary hashes); `list_devices`, `device_info`, `pair`/`validate`, `will_encrypt`, `set_encryption`; output parsing per IDEVICE-CLI.md. | X2 |
| `acquire` | One acquisition end-to-end (§6b) via a callback: `acquisition.json` lifecycle, discovery and recovery (`CaseDetail.acquisitions`), preflight, backup process, progress/prompt parsing, validation, seal; encryption enable/restore and later restore. | X3a, X3b |
| `bin/fake-leapp` | Test double of a LEAPP onefile binary (DEVELOPMENT.md §4.8). Never bundled. | B1 |
| `bin/fake-idevice` | Test double of the four libimobiledevice tools, selected by argv[0] or the first argument (DEVELOPMENT.md §4.8). Never bundled. | X2 |

### 5.2 `src-tauri` (binary)

- **Commands:** registers the commands in CONTRACTS.md §10 and §13.5, validates path arguments per the path policy (§9), and maps core errors to `AppError`.
- **`AppState`:** settings cache, active-job handle (run or acquisition), log backlog (last 2,000 lines), instance lock, bundled-tools dir.
- **Event forwarding:** core callbacks go to `tauri::ipc::Channel`.
- **Lifecycle:**
  - The quit guard handles `WindowEvent::CloseRequested` and `RunEvent::ExitRequested`. During a job it shows a native Rust-side dialog (`tauri_plugin_dialog`) asking "cancel and quit?".
    - Runs: on yes, it cancels, waits up to 30 s for finalize, then exits.
    - Acquisitions: on yes, it cancels, then waits for the cancel semantics in §6b (encryption restore may wait for the device passcode). It shows "finishing safely…" with a "Quit anyway" option. Quitting anyway leaves the record to be marked `interrupted` (with encryption warnings) on the next open.
  - The single-instance lock is `<app_data>/instance.lock` via `std::fs::File::try_lock`. A second instance shows a native message and exits before touching any state.
  - Startup: acquire the lock → temp sweep → app log.

### 5.3 `ui/` and `ui-dev/`

`ui/` is the shipped frontend (`frontendDist`). `api/index.js` chooses the implementation:
- **Real IPC** (`api/ipc.js`, the only module that calls into `window.__TAURI__`) when `window.__TAURI__` exists.
- **The mock** when `window.__TAURI__` is absent **and** the URL has `?mock`. It dynamically imports `/dev/mock.js`, which is served only by `scripts/serve-ui.mjs` from `ui-dev/` and does not exist in bundles. A persistent "MOCK DATA" banner is shown.
- **Otherwise** an error screen.

Screens: Cases, Case, New run, Run, Settings, plus the module-picker component. The UI holds no business logic that the core also implements.

## 6. Run lifecycle

1. **Validate** (`run_start`; any failure returns an `AppError` and creates nothing):
   - no active run;
   - the tool is installed, and the entry hash matches the manifest (or `install.json` where the manifest value is `null`);
   - the input exists and is readable;
   - the type is allowed for this tool and input kind;
   - no path overlap (`input_overlaps_case`):
     - the case folder, the would-be run dir and the temp dir must not equal or lie inside `input_path` or `keychain_path`;
     - the input must not lie inside any case's `runs/` folder or the app dirs;
     - inputs inside a case's `acquisitions/` are allowed (that is how acquired backups are parsed);
   - modules resolve with no unknowns;
   - a password is present if the backup is encrypted;
   - the timezone is in the installed iLEAPP zone list;
   - the run dir path is < 248 characters on Windows (`path_too_long`).
2. **Prepare:**
   - Create `runs/<run_id>/`.
   - Write the initial `run.json` (`status: running`, CONTRACTS.md §7.2).
   - Write `case.lcasedata` and `profile.<ext>` (unless the mode is `all`).
   - Create the per-run temp dir.
   - On failure after the run dir exists, finalize as `failed` with `prepare_failed`.
3. **Hash input** (if requested and the input is a file): on its own thread, **concurrently** with LEAPP, with progress events. Finalize waits for it.
4. **Spawn LEAPP** (argv per LEAPP-CLI.md §4; cwd = run dir; temp env vars; stdin null; stdout → `leapp.stdout.log`, stderr → `leapp.stderr.log`). Record `started_at`. A spawn error → `spawn_failed`.
5. **Stream:** tail `report/_HTML/_Script_Logs/Screen_Output.html` every 250 ms and emit `log` batches.
6. **Exit or cancel:**
   - Exit: record the exit code or signal and `exited_at`.
   - Cancel before exit: SIGTERM the group, then SIGKILL after 10 s (Unix), or terminate the job (Windows).
   - A cancel arriving after exit only stops input hashing.
   - Then drain the tail, emit `stdio_tail`, and remove the per-run temp dir.
7. **Wait for the input hash** (phase `hashing_input` only if still running).
8. **Analyze:** parse `report/_lava_data.lava` if present; check `report/index.html`; apply CONTRACTS.md §7.3.
9. **Seal:** if `report/` exists, hash every file into `report.sha256` (CONTRACTS.md §8), with progress events.
10. **Finalize:**
    - Write the complete `run.json` atomically and mark it read-only.
    - Emit `finished` (status, reasons, warnings, summary).
    - Clear the active run.
    - If the final write fails: emit `finished` with `failed` + `record_write_failed` and log it. The record stays `running` and becomes `interrupted` on the next open.

Phases emitted: `preparing` → `running` → (`hashing_input`) → `analyzing` → `sealing_report` → `finalizing`.

## 6b. Acquisition lifecycle (F11)

Acquisition necessarily writes to the device (pairing record, sync lock during backup, and optionally the backup-encryption setting). Principle 1 is therefore amended for acquisition: **every change the app causes on the device is deliberate and recorded** (`pairing`, `device_changes` and `encryption` in `acquisition.json`, CONTRACTS.md §13.3).

1. **Discover** (`devices_list`). The UI polls every 2 s while the Acquire screen is visible.
   - **Single-flight:** a call made while another is still running returns that call's result.
   - For each UDID from `idevice_id -l`:
     - `ideviceinfo -u <udid> -s -x`: pre-session identity (a subset; fields may be null).
     - `idevicepair -u <udid> hostid`: reads the host's pair record through usbmuxd **without opening a device session**. No record → `not_paired`.
     - `idevicepair -u <udid> validate` **only when a record exists.** `validate` without a record would start pairing and show a Trust prompt on the device.
     - If paired: `WillEncrypt` and disk usage.
   - The active job's UDID is not queried; it is returned with `busy: true`.
   - Poll output is never logged. Tool-missing and usbmuxd-unavailable conditions are reported in `tools.state`, not thrown.
2. **Pair** (`device_pair`). This is the only code path that may pair.
   - Refused for a busy device (`device_busy`) or an already-paired one (`already_paired`).
   - Runs `idevicepair pair`. The UI shows "Unlock the device and tap Trust" and retries on `awaiting_trust` or `locked`.
   - The core remembers `paired_by_app_at` per UDID for this app session.
3. **Preflight** (`acq_preflight`) → `{free_bytes, required_bytes, level}`:
   - `required_bytes` = the device's used data capacity.
   - `level`: `ok` if free ≥ 1.1 × required; `warn` if free ≥ 0.5 × required; `block` otherwise.
4. **Validate** (`acq_start`; failures create nothing):
   - no active job;
   - the device is present, paired and not busy;
   - the tools are available (§9 verification);
   - preflight `level != block`;
   - if `enable_encryption`: the password is given (≥ 4 chars) and `WillEncrypt` is false;
   - on Windows, the acquisition dir path is ASCII-only and ≤ 150 chars (`path_not_supported_by_tool`), because the tools use ANSI file APIs.
5. **Prepare:**
   - Create `acquisitions/<acq_id>/` **and `acquisitions/<acq_id>/backup/`** (the tool refuses a missing target dir).
   - Run `ideviceinfo -u <udid> -x` (full values, now that the device is paired) and save the output as `device-info.plist` in the acquisition folder. It contains IMEI and phone number, so it never goes to the app log.
   - Read `hostid` and `systembuid` for the `pairing` record.
   - Write the initial `acquisition.json` (`status: running`).
   - Create the per-job temp dir.
6. **Enable encryption** (if requested):
   - Command: `idevicebackup2 -u <udid> encryption on` with the password in env `BACKUP_PASSWORD_NEW` (never argv).
   - **No timeout.** On iOS ≥ 13 with a passcode, the tool waits for the passcode to be entered on the device. Parsed prompts become `device_prompt` events.
   - Afterwards, re-read `WillEncrypt`, record the command, and **rewrite `acquisition.json` atomically** before continuing.
   - Outcomes: failure with `WillEncrypt` still false → `failed` (`encryption_enable_failed`). If the outcome is unknown (e.g. `WillEncrypt` unreadable), treat it as enabled for restore purposes and warn `encryption_state_unknown`.
7. **Back up:**
   - Command: `idevicebackup2 -u <udid> backup --full <acq_dir>/backup`, with stdout/stderr to files and a chunk callback for parsing.
   - **Progress:** overall progress only from `\]\s+(\d+)%\s+Finished`. `(x/y)` sizes are per upload batch and are ignored. Events are throttled to ≤ 4/s.
   - **Prompts:** passcode prompt lines become `device_prompt` events.
   - **Cancel:** SIGTERM the session group, then SIGKILL after 30 s (Unix); terminate the job at once (Windows).
8. **Restore encryption** (if encryption was enabled or its state is unknown, and `restore_encryption` is true). This runs whatever the backup outcome, including a cancel.
   - Command: `encryption off` with the password in env `BACKUP_PASSWORD`. **No timeout**; it may wait for the device passcode.
   - Re-read `WillEncrypt`, record it, and rewrite `acquisition.json`.
   - Record `restored_after` (CONTRACTS.md §13.1). If the device is gone or the command fails, add warning `encryption_left_enabled` or `encryption_state_unknown`. The UI then offers `acq_restore_encryption` later.
   - The core zeroizes the password after this step, or after step 6 if no restore is needed.
9. **Validate the backup** per IDEVICE-CLI.md §6 → status and reasons (CONTRACTS.md §13.3). After the backup, check free space; warn `disk_nearly_full` if under 1 GiB (the tool doesn't check its writes).
10. **Seal:** if `backup/` exists, write `backup.sha256` via `hashing::seal_tree`. Cancelling stops the seal (`SealStatus.cancelled`).
11. **Finalize:**
    - Write the complete `acquisition.json` atomically and mark it read-only.
    - Emit `finished`.
    - Clear the active job.

**Cancel semantics by phase:**

| Phase | On cancel |
|---|---|
| `enabling_encryption` | Wait for the command to finish, then go to step 8. |
| `backing_up` | Stop the backup, then step 8. |
| `restoring_encryption` | Ignored; restore always completes. |
| `validating`, `sealing` | Stop sealing and finalize. |

**Handoff:** the core keeps no password after step 8. If the examiner ticked "Parse with iLEAPP now", the UI (which already holds the password it collected) pre-fills New run and clears the password once that run starts or the form closes (CONTRACTS.md §13.5).

**Recovery** on `case_open`: a `running` acquisition that is not this process's active job becomes `interrupted` (discovery and recovery are owned by the `acquire` module). If the record shows encryption was enabled by the examiner and not confirmed restored, add warning `encryption_left_enabled`, or `encryption_state_unknown` if the enable outcome was unknown. The Case screen shows a "Turn backup encryption off" action.

**Later restore** (`acq_restore_encryption {case_path, acq_id, password}`):
- Allowed only when the record has one of those two warnings and the device is connected and paired.
- Runs step 8 on its own.
- Writes a separate, read-only `encryption-restore.json` next to the (already read-only) `acquisition.json`.

## 7. Process model details

- **Unix:**
  - Spawn with `std::process::Command` plus `pre_exec(|| { libc::setsid(); Ok(()) })`, which gives a new session with pgid = pid. This is the only `unsafe` code that runs between fork and exec; comment why. Every other `unsafe` in `process` is a plain FFI call (`killpg`, `kill`, and the Win32 calls below), each commented.
  - Cancel: `killpg(pgid, SIGTERM)`, wait up to the spawn's configured grace (10 s for LEAPP, 30 s for backups), then `killpg(pgid, SIGKILL)`.
  - `process` also offers a stdout/stderr chunk callback (used for acquisition progress and prompt parsing) in addition to writing the log files.
  - Reap the leader with `wait`, then poll `killpg(pgid, 0)` until `ESRCH` (up to 2 s) before reporting the tree gone. On Linux, members that exited but were never reaped (zombies under an init that does not reap, as in CI containers) count as gone.
  - If the leader exits while other members of its group still run (for example after a SIGKILL of the onefile parent), they are stopped the same way (SIGTERM, grace, SIGKILL), so nothing of the tree outlives `wait`.
- **Windows:**
  - Build the process with `std::process::Command` and `creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW)`.
  - `CreateJobObjectW` + `SetInformationJobObject(JobObjectExtendedLimitInformation, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE)`, then `AssignProcessToJobObject(job, child.as_raw_handle())`. If assignment fails, `TerminateProcess` and error.
  - Resume the single thread: `CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD)`, `Thread32First/Next` where `th32OwnerProcessID == child.id()` → `OpenThread(THREAD_SUSPEND_RESUME)` → `ResumeThread`.
  - Cancel = `TerminateJobObject`. There is no graceful signal on Windows. When the leader exits, whatever is left in the job is terminated too.
  - `windows-sys` features: `Win32_Foundation`, `Win32_Security`, `Win32_System_JobObjects`, `Win32_System_Threading`, `Win32_System_Diagnostics_ToolHelp`.
  - Stable std has no main-thread handle or raw attribute API, which is why this sequence is prescribed.
- **Paths passed to LEAPP:** absolute via `std::path::absolute`, recorded verbatim in argv (password excepted). **Never** pass `\\?\`-prefixed paths (do not `canonicalize` for argv on Windows): LEAPP adds the prefix itself and checks `path[1] == ':'`. `Command::current_dir` cannot take verbatim paths, so the run dir must stay < 248 chars.
- **Linux AppImage:** never run the AppImage per run. At install, run `<asset> --appimage-extract` once (no FUSE needed); execute `squashfs-root/<entry>` directly.
- **App crash:** if the app crashes during a run, Windows kills children via the job. On Unix, orphans are possible; the next startup marks the run interrupted and sweeps temp dirs. Documented and accepted for phase 1.

## 8. Storage layout

App directories come from Tauri path APIs (identifier `com.suitedfir.desktop`):

```
<app_config>/settings.json
<app_data>/instance.lock
<app_data>/leapp/<tool>/<version>/{install.json, modules.json, bin/<entry> | squashfs-root/…}
<app_data>/profiles/<tool>/<name>.<ilprofile|alprofile>
<app_cache>/tmp/<run_id>/            per-run TMPDIR, deleted after the run
<app_log>/suitedfir.log              app log, truncated at 5 MB, never contains secrets
```

The tools dir (`<app_data>/leapp` by default) can be overridden in settings for machines where AppLocker/WDAC allows execution only from approved paths. It may not be inside a case folder.

A case folder (default parent `<Documents>/suiteDFIR Cases/`):

```
<Case Name>/
  case.json
  runs/
    20260924-183005Z-ileapp-3f9a1c/
      run.json               read-only once finalized
      case.lcasedata         passed to LEAPP -d
      profile.ilprofile      only when module mode != all
      leapp.stdout.log
      leapp.stderr.log
      report/                LEAPP output (--custom_output_folder report)
      report.sha256          manifest of report/** (CONTRACTS.md §8)
  acquisitions/
    20260924-171200Z-ios-9c01de/
      acquisition.json       read-only once finalized (rewritten atomically during the job after each device change)
      device-info.plist      full ideviceinfo output captured after pairing (never logged)
      encryption-restore.json  only if a later restore was performed
      idevicebackup2.stdout.log
      idevicebackup2.stderr.log
      backup/<udid>/         the iTunes-format backup (input for iLEAPP -t itunes)
      backup.sha256          manifest of backup/** (same format as report.sha256)
```

Release builds bundle the libimobiledevice tools as sidecars next to the app executable (macOS: inside the `.app`). The shell passes their directory to the core; on Linux the core looks them up on `PATH`.

Runs are discovered by scanning `runs/*/run.json`; `case.json` does not list runs. The app never deletes anything in a case folder.

A **known case folder** is a path in `settings.recent_cases` whose `case.json` parses.

## 9. Security model

- **CSP (webview, embedded assets):**
  - The CSP is exactly: `default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; frame-src 'none'; form-action 'none'`.
  - IPC uses `ipc://localhost` on macOS/Linux and `http://ipc.localhost` on Windows.
  - Tauri adds hashes for bundled `<script>`/`<style>` and injects its own scripts as init scripts, so they are unaffected.
  - Set styles only via CSSOM (`el.style.x = …`) or classes, never `style=""` attributes.
  - **The CSP is not enforced under `cargo tauri dev`** (the dev server is loaded directly). Verify CSP with `cargo tauri build --debug`, and with `scripts/serve-ui.mjs`, which sends the same CSP header (UI tests assert no CSP violations).
- **Capabilities:**
  - `core:default` + `dialog:allow-open` + `dialog:allow-save`, nothing else.
  - App commands need no capability entries (there is no app ACL manifest).
  - Window close is handled Rust-side, so no JS window permissions are needed.
- **Untrusted text:** log lines, file names, module names and anything from LEAPP output may be evidence-derived. Render it with `textContent` or DOM APIs only; **never** `innerHTML`/`outerHTML`/`insertAdjacentHTML` with dynamic data.
- **Path policy (per command):**

| Command | Path rule |
|---|---|
| `case_create.parent_dir`, `settings_update.cases_root` | Existing writable dir, not inside the tools dir or app dirs. |
| `settings_update.tools_dir` | Existing writable dir, not inside any known case folder. |
| `case_open.path` | Any dir containing a valid `case.json` (it becomes known). |
| `case_update`, `case_forget`, `run_get`, `open_report`, `open_text_file` | `case_path` must be a known case folder; `run_id` must match the `run_id` format and exist. |
| `input_inspect.path`, `run_start.input_path`, `run_start.keychain_path` | Any readable path, subject to the overlap rule in §6 step 1. |
| `tool_import.archive_path`, `profile_import.path` | Any readable regular file (read-only). |
| `profile_export.dest_path` | A path returned by the save dialog; refuse if inside a known case folder's `runs/`. |
| `reveal_path.path` | Inside a known case folder or app dirs only. |
| `acq_preflight`, `acq_start`, `acq_get`, `acq_cancel`, `open_acq_file`, `acq_restore_encryption` | `case_path` must be a known case folder; `acq_id` must match its format and exist. |
| `devices_list`, `device_pair`, and any `udid` argument | No path. `udid` must match `^(?:[0-9a-fA-F]{40}\|[0-9A-Fa-f]{8}-[0-9A-Fa-f]{16})$`. |

- **Downloads:**
  - HTTPS manifest URLs only (`https_only`, which also covers redirects).
  - Abort beyond the manifest size.
  - Verify the asset SHA-256 before extraction; extract only the expected entry; verify the entry SHA-256.
  - Networks with TLS interception may fail certificate checks; the documented remedy is offline import.
- **Secrets:**
  - Backup passwords live only in memory:
    - For parsing: the duration of spawn.
    - For acquisition: from `acq_start` until the restore step finishes (§6b), then they are zeroized.
    - The acquisition→parse handoff is held by the UI, not the core.
  - They are never logged, and never appear in `run.json`/`acquisition.json`, error messages or IPC events. The only IPC traffic is the request from the UI.
  - The UI clears password fields after use.
  - LEAPP's argv password is visible in the OS process list while LEAPP runs; this is accepted and documented. The libimobiledevice tools get passwords via env.

## 10. Platform notes

- **macOS:**
  - Finder backups live in `~/Library/Application Support/MobileSync/Backup`, which is TCC-protected. Reading it requires granting suiteDFIR **Full Disk Access**; detect `EPERM`/`EACCES` there and show guidance.
  - Upstream macOS LEAPP builds are Developer ID signed and notarized.
  - Minimum macOS 11.
- **Windows:**
  - The upstream LEAPP exe is unsigned; AppLocker/WDAC may block it (use the tools-dir override).
  - The WebView2 runtime may be absent on offline Windows 10. The **x64 offline installer variant** bundles it (≈ 215 MB). arm64 gets only the online installer, because Tauri embeds the x86 WebView2 installer for non-x64 targets.
  - Rust std handles long paths for file operations, but the process cwd must be < 248 chars.
- **iOS acquisition prerequisites:**
  - macOS: none (usbmuxd is built in).
  - Windows: the Apple Mobile Device Service (Apple Devices app or iTunes). The app detects its absence and links to installation guidance. The tools use ANSI file APIs, so acquisition folders must be ASCII and short (§6b step 4).
  - Host pair records are created by pairing and live in `/var/db/lockdown` (macOS; not readable by the app), `%ProgramData%\Apple\Lockdown` (Windows), or `/var/lib/lockdown` (Linux). The app records `host_id`/`system_buid` but never edits these stores.
  - Linux: the distro packages `usbmuxd` and `libimobiledevice-utils` (or equivalent), with the daemon running. The app detects missing tools and shows the install command for common distros.
- **Linux:**
  - webkit2gtk-4.1 is required (bundled in the AppImage).
  - Builds use an `ubuntu:22.04` container (glibc baseline).
  - Document the `WEBKIT_DISABLE_DMABUF_RENDERER=1` fallback in the user guide.

## 11. Phase-2 hooks (informational, do not build)

LEAPP writes `_lava_artifacts.db` (SQLite, one table per artifact) and `_lava_data.lava` (JSON) into every report. Phase-2 visualization reads those, never the HTML/TSV/KML. Keep the run folder layout stable.
