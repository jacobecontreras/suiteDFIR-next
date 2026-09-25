# Contracts

File formats on disk and the UI↔core API. During M0 this document is the spec. After M0.3, **the Rust types in `crates/core/src/contracts/` are the source of truth**; this document, `ui/types.d.ts` and the generated examples in `ui-dev/fixtures/contracts/` must match them (CI enforces the examples, see DEVELOPMENT.md §4.7).

## 1. Conventions

- **Encoding and keys:** JSON everywhere, UTF-8, `snake_case` keys on disk and inside IPC payloads. (Only top-level Tauri command *argument names* are camelCase on the JS side: `req`, `onEvent`.)
- **Schema versions:** every top-level file has `schema_version` (integer, starts at 1). Readers reject unknown versions with a clear error. Unknown extra fields are ignored on read.
- **Timestamps:** RFC 3339 UTC with `Z` at second precision (`2026-09-24T18:30:05Z`). With the `time` crate, use `Rfc3339` after `.replace_nanosecond(0)`. Durations are integer milliseconds.
- **Paths:** absolute, OS-native strings, never `\\?\`-prefixed. Paths *inside* a run record are relative to the run folder where noted.
- **Hashes:** lowercase hex SHA-256 without a prefix.
- **Optional values:** `null`, never omitted, in files the app writes.
- **Atomic write** (`fsutil::write_json_atomic`): write `<name>.tmp-<rand>` in the same dir → flush + `sync_all` → rename over the target.

## 2. Enumerations

| Type | Values |
|---|---|
| `ToolId` | `ileapp`, `aleapp` |
| `PlatformKey` | `macos-aarch64`, `macos-x86_64`, `windows-x86_64`, `windows-aarch64`, `linux-x86_64`, `linux-aarch64` |
| `InputKind` | `file`, `directory` |
| `InputType` | `fs`, `tar`, `zip`, `gz`, `itunes`, `file`, `raw` (LEAPP `-t` values) |
| `ModuleMode` | `all`, `profile`, `custom` |
| `RunStatus` | `running`, `succeeded`, `completed_with_errors`, `failed`, `cancelled`, `interrupted` |
| `HashStatus` | `not_requested`, `not_applicable`, `pending`, `completed`, `cancelled`, `failed`, `interrupted` |
| `SealStatus` | `pending`, `sealed`, `skipped_no_output`, `failed`, `interrupted` |
| `InstallSource` | `download`, `offline_import`, `dev_override` (debug builds only) |
| `EntryVerifiedAgainst` | `manifest`, `install_record`, `none` (dev override only) |
| `ToolState` | `unsupported_platform`, `not_installed`, `installed_unverified`, `verified`, `verification_failed`, `dev_override` |
| `RunPhase` | `preparing`, `running`, `hashing_input`, `analyzing`, `sealing_report`, `finalizing` |

## 3. `leapp-manifest.json` (repo root, embedded at build time)

Maintained by `cargo xtask pin-leapp` (ROADMAP A1). Hand edits only for `urls` mirrors and for filling AppImage `entry_sha256` values reported by the smoke workflow (E3).

```json
{
  "schema_version": 1,
  "tools": {
    "ileapp": {
      "display_name": "iLEAPP",
      "upstream_repo": "abrignoni/iLEAPP",
      "version": "v2026.4.2",
      "license": "MIT",
      "profile_ext": "ilprofile",
      "profile_leapp_id": "ileapp",
      "input_types": ["fs", "tar", "zip", "gz", "itunes", "file", "raw"],
      "supports_timezone": true,
      "supports_keychain": true,
      "supports_itunes_password": true,
      "platforms": {
        "macos-aarch64": {
          "asset_name": "ileapp-v2026.4.2-macOS_Apple_Silicon.zip",
          "asset_size": 55085487,
          "asset_sha256": "d99f2d05dbde20ee997de477c38443d4f019d60326a8c6b9456058c6cf590386",
          "archive_kind": "zip",
          "entry": "ileapp",
          "entry_sha256": "<computed by pin-leapp --download-verify>",
          "urls": ["https://github.com/abrignoni/iLEAPP/releases/download/v2026.4.2/ileapp-v2026.4.2-macOS_Apple_Silicon.zip"]
        }
      }
    },
    "aleapp": { "...": "same shape; profile_leapp_id \"aleapp\", profile_ext \"alprofile\", input_types [fs,tar,zip,gz,raw], supports_* false" }
  }
}
```

**Field rules:**
- `archive_kind`: `zip` (the entry is a path inside the zip) or `appimage` (the entry is a path inside `squashfs-root/`, e.g. `usr/bin/ileapp`).
- `entry_sha256`: required for `zip`. It may be `null` for `appimage` until E3 reports the value. While `null`, the install-time hash is recorded in `install.json` and runs record `entry_verified_against: "install_record"`.
- `urls`: tried in order; each must be `https://`.
- Unsupported platform: a platform missing from `platforms` is unsupported (`ToolState.unsupported_platform`).

## 4. `install.json` (`<tools_dir>/<tool>/<version>/install.json`)

```json
{
  "schema_version": 1,
  "tool": "ileapp",
  "version": "v2026.4.2",
  "platform": "macos-aarch64",
  "asset_name": "ileapp-v2026.4.2-macOS_Apple_Silicon.zip",
  "asset_sha256": "d99f…0386",
  "entry_path": "bin/ileapp",
  "entry_sha256": "…",
  "source": "download",
  "source_detail": "https://github.com/abrignoni/iLEAPP/releases/download/…",
  "installed_at": "2026-09-24T18:00:00Z",
  "module_count": 1176
}
```

**Installed:** `install.json` exists, its `asset_sha256` equals the manifest's, `entry_path` exists, and `modules.json` exists.

**Verified:** in addition, the entry's hash equals the manifest `entry_sha256` (or `install.json.entry_sha256` when the manifest value is `null`). Verification runs before every run and on the Settings "Verify" action.

## 5. `modules.json` (next to `install.json`)

```json
{
  "schema_version": 1,
  "tool": "ileapp",
  "version": "v2026.4.2",
  "generated_at": "2026-09-24T18:00:05Z",
  "always_run": { "default": ["last_build"], "itunes": ["itunes_backup_info", "itunes_backup_installed_applications"] },
  "timezones": ["Africa/Abidjan", "…", "UTC"],
  "modules": [
    { "name": "callHistory", "module_name": "callHistory", "category": "Call History",
      "display_name": "Call History", "description": null }
  ]
}
```

- **`name`:** the artifact key. It is what profiles contain and what LEAPP matches.
- **`always_run`:** maps an `InputType` (or `default`) to artifact names that run regardless of selection. It is derived per tool by the core's encoded rules, which are verified by E3 (LEAPP-CLI.md §5):
  - For aLEAPP, the names are those of plugins whose `module_name` is `usagestatsVersion`.
  - These names are **not** in `modules`.
- **`modules`:** excludes everything the tool's own `main()` excludes from selection. Sorted by `category`, then `display_name` (case-insensitive).
- **`timezones`:** `pytz.all_timezones` from the binary (iLEAPP). `null` for aLEAPP.

## 6. `settings.json` and `case.json`

`<app_config>/settings.json`:

```json
{
  "schema_version": 1,
  "cases_root": "/Users/examiner/Documents/suiteDFIR Cases",
  "recent_cases": ["/Users/examiner/Documents/suiteDFIR Cases/Operation Nightjar"],
  "defaults": { "examiner": "J. Doe", "agency": "County Forensics Lab", "timezone": "UTC" },
  "tools_dir": null
}
```

`<case>/case.json`:

```json
{
  "schema_version": 1,
  "case_id": "5b0c2f4e9a7d4b1f8c3e6a2d1f0b9e7c",
  "name": "Operation Nightjar",
  "case_number": "2026-0142",
  "examiner": "J. Doe",
  "agency": "County Forensics Lab",
  "description": "",
  "default_timezone": "America/Chicago",
  "created_at": "2026-09-24T18:10:00Z",
  "updated_at": "2026-09-24T18:10:00Z",
  "created_by_app_version": "0.2.0"
}
```

**`case.json` fields:**
- `name` is required, 1–120 chars.
- Editable via `case_update`: `name`, `case_number`, `examiner`, `agency`, `description`, `default_timezone`. The folder is **not** renamed when `name` changes.
- `case_id` is 32 lowercase hex chars from the OS RNG (`getrandom`).
- `default_timezone` may be `null` (fall back to settings).

**Folder name:**
- Derived from `name`: `<>:"/\|?*` and control characters are replaced by `_`, and trailing dots and spaces are trimmed.
- Windows reserved names (`CON`, `NUL`, …) get a `_` suffix.
- On collision, append ` (2)`, ` (3)`, and so on.

**`recent_cases`:** deduplicated, most recent first, max 50.

## 7. `run.json` (the audit record)

### 7.1 Final record

```json
{
  "schema_version": 1,
  "run_id": "20260924-183005Z-ileapp-3f9a1c",
  "label": "iPhone 12 Finder backup",
  "status": "completed_with_errors",
  "status_reasons": [
    { "code": "modules_errored", "message": "2 modules reported Error: foo, bar" }
  ],
  "warnings": [
    { "code": "stderr_traceback", "message": "stderr contains a Python traceback (see leapp.stderr.log)" }
  ],
  "created_at": "2026-09-24T18:30:05Z",
  "started_at": "2026-09-24T18:30:06Z",
  "ended_at": "2026-09-24T18:52:41Z",
  "recovered_at": null,
  "duration_ms": 1356000,
  "app": { "name": "suiteDFIR", "version": "0.2.0" },
  "host": { "os": "macos", "os_version": "15.6", "arch": "aarch64", "hostname": "LAB-MAC-01" },
  "case_snapshot": {
    "case_id": "5b0c…9e7c", "name": "Operation Nightjar", "case_number": "2026-0142",
    "examiner": "J. Doe", "agency": "County Forensics Lab"
  },
  "tool": {
    "id": "ileapp", "version": "v2026.4.2", "platform": "macos-aarch64",
    "asset_name": "ileapp-v2026.4.2-macOS_Apple_Silicon.zip", "asset_sha256": "d99f…0386",
    "entry_sha256": "…", "entry_verified_against": "manifest", "install_source": "download"
  },
  "input": {
    "path": "/Volumes/Evidence/00008101-000A1B2C3D4E",
    "kind": "directory",
    "type": "itunes",
    "type_detected": "itunes",
    "size_bytes": null,
    "itunes_encrypted": true,
    "hash": { "algorithm": "sha256", "status": "not_applicable", "value": null, "started_at": null, "completed_at": null }
  },
  "options": {
    "timezone": "America/Chicago",
    "timezone_supported": true,
    "password_supplied": true,
    "keychain_path": null,
    "keychain_sha256": null
  },
  "modules": {
    "mode": "custom",
    "profile_name": null,
    "requested": ["callHistory", "sms"],
    "resolved": ["callHistory", "sms"],
    "unknown": [],
    "always_run": ["itunes_backup_info", "itunes_backup_installed_applications"],
    "available_count": 1176
  },
  "command": {
    "argv": ["/Users/examiner/Library/Application Support/com.suitedfir.desktop/leapp/ileapp/v2026.4.2/bin/ileapp",
             "-t", "itunes", "-i", "/Volumes/Evidence/00008101-000A1B2C3D4E",
             "-o", "/Users/examiner/Documents/suiteDFIR Cases/Operation Nightjar/runs/20260924-183005Z-ileapp-3f9a1c",
             "--custom_output_folder", "report",
             "-d", "/Users/examiner/Documents/suiteDFIR Cases/Operation Nightjar/runs/20260924-183005Z-ileapp-3f9a1c/case.lcasedata",
             "-m", "/Users/examiner/Documents/suiteDFIR Cases/Operation Nightjar/runs/20260924-183005Z-ileapp-3f9a1c/profile.ilprofile",
             "-tz", "America/Chicago", "--itunes_password", "<redacted>"],
    "cwd": "/Users/examiner/Documents/suiteDFIR Cases/Operation Nightjar/runs/20260924-183005Z-ileapp-3f9a1c"
  },
  "process": {
    "exit_code": 0, "signal": null, "exited_at": "2026-09-24T18:51:10Z",
    "cancel_requested": false, "escalated_to_kill": false
  },
  "leapp_result": {
    "lava_data_found": true,
    "processing_status": "Complete",
    "leapp_version_reported": "2026.4.2",
    "index_html_found": true,
    "module_counts": { "complete": 120, "error": 2, "no_files_found": 1054, "other": 0 },
    "error_modules": ["foo", "bar"]
  },
  "output": {
    "report_dir": "report",
    "seal": { "status": "sealed", "manifest": "report.sha256", "manifest_sha256": "…", "file_count": 5321, "total_bytes": 123456789 }
  },
  "logs": {
    "stdout": "leapp.stdout.log",
    "stderr": "leapp.stderr.log",
    "screen_output": "report/_HTML/_Script_Logs/Screen_Output.html"
  }
}
```

**Identifiers and times:**
- `run_id` = `YYYYMMDD-HHMMSSZ-<tool>-<6 lowercase hex>` (UTC).
- `created_at`: `run_start` accepted.
- `started_at`: LEAPP spawned, or `null` if it never was.
- `ended_at`: finalize time (`null` for `interrupted`, which sets `recovered_at` instead).
- `duration_ms` = `ended_at − created_at` (`null` if `ended_at` is `null`).

**`command.argv`:** recorded verbatim, including absolute paths. The only exception is the value after `--itunes_password`, which becomes `<redacted>`.

**`modules`:**
- For mode `all`: `requested = []`, `resolved` = every selectable name (sorted), `unknown = []`.
- `always_run` = the entry for this input type (or `default`).

**Hashes:**
- `input.hash.status` is `not_applicable` for directories and `not_requested` when unticked.
- The keychain file, if given, is always hashed.

**`tool` for dev override** (debug builds only): `asset_name` and `asset_sha256` are `null`, `entry_verified_against` is `none`, and `install_source` is `dev_override`.

### 7.2 Initial record (written at lifecycle step 2) and recovery

The initial record carries every known field. The unknown ones are:
- `status: "running"`, `status_reasons: []`, `warnings: []`;
- `started_at`, `ended_at`, `recovered_at`, `duration_ms`, `process` and `leapp_result`: `null`;
- `input.hash.status`: `pending` | `not_requested` | `not_applicable`;
- `output.seal`: `{ "status": "pending", "manifest": null, "manifest_sha256": null, "file_count": null, "total_bytes": null }`.

**Recovery** (on `case_open`, for any `running` record that is not this process's active run):
- set `status: interrupted`, `status_reasons: [{code: "app_interrupted"}]`, `recovered_at: now`;
- change any `pending` hash or seal status to `interrupted`;
- then finalize: atomic write and read-only.

### 7.3 Status rules (`run::status`)

1. **Short-circuits** (only this reason is recorded):
   - `prepare_failed` → `failed`.
   - `spawn_failed` → `failed`.
   - Cancel requested **before** the process exit was observed → `cancelled` (`cancelled_by_user`).
2. **Otherwise, evaluate every check below** and add a reason for each that matches. Skip a check whose input is unavailable (e.g. checks 5–6 when the lava file was not parsed).

   | # | Check | Reason code |
   |---|---|---|
   | 3 | `report/` does not exist | `no_output_dir` (message: "LEAPP exited before creating output; see stdout") |
   | 4 | `report/` exists but `_lava_data.lava` is missing or unparsable | `lava_data_missing` |
   | 5 | `processing_status` ≠ `Complete` | `processing_incomplete` |
   | 6 | No lava module entry other than always-run entries | `no_modules_ran` (typical for invalid input or a wrong backup password) |
   | 7 | `report/` exists but `index.html` is missing | `index_html_missing` |
   | 8 | Exit code ≠ 0 | `nonzero_exit` (message includes the code; exit 2 = LEAPP rejected its arguments) |
   | 8b | Killed by a signal | `killed_by_signal` |

   A lava module entry is **always-run** if its `artifact_name` (or `module_name` when `artifact_name` is absent) is in the always-run set, or its `module_name` equals the `module_name` of an always-run plugin.
3. **Status:**
   - If any check 3–8b matched → `failed`.
   - Otherwise, if any lava module has `module_status == "Error"` → `completed_with_errors` (`modules_errored`, listing names).
   - Otherwise → `succeeded`.
4. **Warnings** (never change the status):

   | Code | When |
   |---|---|
   | `stderr_traceback` | stderr contains `Traceback (most recent call last)` |
   | `input_hash_failed`, `input_hash_cancelled` | a cancel arrived after exit |
   | `seal_failed` | |
   | `symlinks_in_report` | count; symlinks are not followed or listed |
   | `unencodable_filename` | Windows names that are not valid Unicode |
   | `modules_other_status` | module statuses other than Complete / Error / No files found |

### 7.4 Expected outcomes for fake-leapp scenarios (normative for tests)

| Scenario | Exit | Status | Reasons | Warnings |
|---|---|---|---|---|
| `success` | 0 | `succeeded` | none | none |
| `artifact_error` | 0 | `completed_with_errors` | `modules_errored` | `stderr_traceback` |
| `invalid_input` | 0 | `failed` | `no_modules_ran`, `index_html_missing` | none |
| `early_exit` | 0 | `failed` | `no_output_dir` | none |
| `argparse_error` | 2 | `failed` | `no_output_dir`, `nonzero_exit` | none |
| `crash` | 1 | `failed` | `lava_data_missing`, `index_html_missing`, `nonzero_exit` | `stderr_traceback` |
| `prompt` | 1 | `failed` | `lava_data_missing`, `index_html_missing`, `nonzero_exit` | `stderr_traceback` |
| `slow` + cancel | n/a | `cancelled` | `cancelled_by_user` | none; `escalated_to_kill: false` |
| `ignore_term` + cancel (Unix) | n/a | `cancelled` | `cancelled_by_user` | none; `escalated_to_kill: true` |

## 8. Other files

**Profile** (`.ilprofile` / `.alprofile`, LEAPP native; also the import/export format):

```json
{"leapp": "ileapp", "format_version": 1, "plugins": ["callHistory", "sms"]}
```

- **Storage:** stored profiles live at `<app_data>/profiles/<tool>/<name>.<ext>`. `name` is 1–80 chars, sanitized like case folder names.
- **Import:**
  - Reject a wrong `leapp` value or a `format_version` other than 1 (`profile_invalid`).
  - Default name = the file stem. An existing name → `profile_exists` unless `overwrite: true`.
  - Unknown names are kept and reported.
- **Save:** rejects unknown names.

**Case data** (`case.lcasedata`, LEAPP native):

```json
{"leapp": "case_data", "case_data_values": {"Case Number": "2026-0142", "Agency": "County Forensics Lab", "Examiner": "J. Doe"}}
```

**Report manifest** (`report.sha256`), GNU coreutils `sha256sum` format so `sha256sum -c report.sha256` works from the run folder:
- **Lines:** one line per regular file under `report/`, sorted by byte order of the relative path, LF endings.
- **Line format:** `<64 hex><space><space><path>`, where the path is relative to the run folder with forward slashes (e.g. `report/index.html`).
- **Escaping:** if the path contains `\`, LF or CR, the line starts with `\` and those characters are written as `\\`, `\n` and `\r`.
- **Name bytes:**
  - Unix: raw bytes.
  - Windows: UTF-8. Names that are not valid Unicode are written lossily and produce the warning `unencodable_filename`.
- **Symlinks:** neither followed nor listed; they are counted in the warning `symlinks_in_report`.

## 9. Shared IPC types

```ts
type AppError = { code: ErrorCode; message: string; detail: string | null };

type ToolStatus = {
  tool: ToolId; display_name: string; pinned_version: string; state: ToolState;
  installed_version: string | null; install_source: InstallSource | null;
  module_count: number | null; install_dir: string | null; problem: string | null;
};
type ToolModules = {
  tool: ToolId; version: string;
  always_run: Record<string, string[]>;
  timezones: string[] | null;
  modules: ModuleInfo[];
};
type ModuleInfo = { name: string; module_name: string; category: string; display_name: string; description: string | null };
type CaseSummary = { path: string; exists: boolean; case: CaseFile | null; run_count: number; last_run_at: string | null };
type CaseDetail = { path: string; case: CaseFile; runs: RunSummary[]; acquisitions: AcqSummary[]; recovered: string[] };  // recovered = run_ids and acq_ids marked interrupted by this open
type RunSummary = {
  run_id: string; run_dir: string; label: string | null; status: RunStatus; tool: ToolId; tool_version: string;
  input_path: string; input_type: InputType; created_at: string; started_at: string | null;
  ended_at: string | null; duration_ms: number | null; report_available: boolean;
};
type InputInspection = {
  path: string; kind: InputKind; size_bytes: number | null; detected_type: InputType | null;
  allowed_types: InputType[]; is_itunes_backup: boolean; itunes_encrypted: boolean | null;
  hashable: boolean; warnings: string[];
};
type ModuleSelection =
  | { mode: "all" }
  | { mode: "profile"; profile_name: string }
  | { mode: "custom"; modules: string[] };
type RunRequest = {
  case_path: string; tool: ToolId; input_path: string; input_type: InputType;
  modules: ModuleSelection; timezone: string | null; itunes_password: string | null;
  keychain_path: string | null; hash_input: boolean; label: string | null;
};
type ActiveJob =
  | { kind: "run"; case_path: string; run_id: string; tool: ToolId; created_at: string; phase: RunPhase }
  | { kind: "acquisition"; case_path: string; acq_id: string; udid: string; created_at: string; phase: AcqPhase };
type ProfileInfo = { tool: ToolId; name: string; modules: string[]; unknown_modules: string[] };
type Reason = { code: string; message: string };
type IosBackup = {                                   // S1
  path: string; device_name: string | null; product_type: string | null; ios_version: string | null;
  last_backup: string | null; encrypted: boolean | null; size_bytes: number | null;
};
```

`CaseFile` is the §6 `case.json` object. `RunRecord` is the §7 object. `Settings` is the §6 `settings.json` object.

## 10. IPC commands

All commands are `async`. Each takes at most one argument named `req` (an object) plus, where noted, a `Channel` named `on_event` (`onEvent` in JS), and returns `Result<T, AppError>`. JS calls `invoke("<name>", { req, onEvent })`. Path arguments follow the path policy in ARCHITECTURE.md §9.

| Command | `req` | Returns | Notes |
|---|---|---|---|
| `app_info` | none | `{app_version, platform: PlatformKey\|null, os, arch, dev_override: boolean, paths:{app_data, app_config, app_cache, app_log, tools_dir}}` | |
| `licenses_get` | none | `string` | Embedded `THIRD-PARTY-NOTICES.md`. |
| `settings_get` | none | `Settings` | |
| `settings_update` | `{cases_root?, defaults?, tools_dir?}` | `Settings` | Omitted = unchanged. `tools_dir: null` = reset to default. |
| `tools_status` | none | `ToolStatus[]` | Cheap: no entry hashing (the state is `installed_unverified` until verified this session). |
| `tool_verify` | `{tool}` | `ToolStatus` | Re-hashes the entry. |
| `tool_install` | `{tool}` + `on_event: InstallEvent` | `ToolStatus` | Download → verify → extract → verify entry → introspect. |
| `tool_import` | `{tool, archive_path}` + `on_event` | `ToolStatus` | Same pipeline from a local file. |
| `tool_modules` | `{tool}` | `ToolModules` | |
| `cases_list` | none | `CaseSummary[]` | From `recent_cases`. |
| `case_create` | `{name, case_number, examiner, agency, description, default_timezone, parent_dir: string\|null}` | `CaseDetail` | `parent_dir: null` → `cases_root`. Becomes known and most recent. |
| `case_open` | `{path}` | `CaseDetail` | Becomes known and most recent; runs recovery. |
| `case_update` | `{path, fields}` | `CaseDetail` | `fields` = editable fields from §6. |
| `case_forget` | `{path}` | none | Removes from `recent_cases` only. |
| `run_get` | `{case_path, run_id}` | `RunRecord` | |
| `input_inspect` | `{tool, path, case_path}` | `InputInspection` | Read-only. Overlap violations are returned as `input_overlaps_case`. |
| `ios_backups_find` | none | `IosBackup[]` | S1. |
| `profiles_list` | `{tool}` | `ProfileInfo[]` | `unknown_modules` is computed against the installed module list. |
| `profile_save` | `{tool, name, modules}` | `ProfileInfo` | Overwrites the same name. |
| `profile_delete` | `{tool, name}` | none | |
| `profile_import` | `{tool, path, name: string\|null, overwrite: boolean}` | `ProfileInfo` | |
| `profile_export` | `{tool, name, dest_path}` | none | `dest_path` comes from the save dialog. |
| `run_start` | `RunRequest` + `on_event: RunEvent` | `{run_id, run_dir}` | ARCHITECTURE.md §6 step 1. `run_already_active` if any job is active. |
| `run_cancel` | `{run_id}` | none | Idempotent; `run_not_found` if the id isn't active. |
| `job_active` | none | `ActiveJob \| null` | Run or acquisition. |
| `job_attach` | `{kind, id}` + `on_event` (`RunEvent` or `AcqEvent`) | `{backlog: string[]}` | For UI reloads; replaces the previous subscriber. |
| `open_report` | `{case_path, run_id}` | none | Opens `report/index.html` in the default browser (`report_missing` if absent). |
| `reveal_path` | `{path}` | none | |
| `open_text_file` | `{case_path, run_id, which: "stdout"\|"stderr"\|"run_json"\|"report_manifest"}` | none | Opens in the OS default app. |
| `temp_cleanup` | none | `{freed_bytes}` | Refuses while a run is active. |

## 11. Events

`RunEvent` (tagged by `type`):

```ts
| { type: "phase"; phase: RunPhase }
| { type: "log"; lines: string[] }            // batches, ≤ 500 lines per event
| { type: "stdio_tail"; stream: "stdout" | "stderr"; lines: string[] }  // once each after exit; last 200 lines
| { type: "hash_progress"; bytes_done: number; bytes_total: number }
| { type: "seal_progress"; files_done: number; files_total: number | null }
| { type: "finished"; status: RunStatus; reasons: Reason[]; warnings: Reason[]; summary: RunSummary }
```

`InstallEvent`:

```ts
| { type: "stage"; stage: "downloading" | "verifying" | "extracting" | "hashing" | "introspecting" | "done" }
| { type: "download_progress"; bytes_done: number; bytes_total: number }
| { type: "message"; text: string }
```

Log lines are plain text; the core strips HTML tags from `Screen_Output.html` records. The UI renders them with `textContent` only. The Run screen shows `stdio_tail` in the result panel ("Parser output").

## 12. Error codes

`another_instance_running`, `run_already_active` (any active job), `acq_not_found`, `device_not_found`, `device_not_paired`, `device_locked`, `trust_pending`, `trust_denied`, `pairing_failed`, `usbmuxd_unavailable`, `idevice_tools_missing`, `idevice_tools_verification_failed`, `encryption_already_on`, `encryption_password_required`, `insufficient_space`, `run_not_found`, `tool_not_installed`, `tool_verification_failed`, `unsupported_platform`, `download_failed`, `hash_mismatch`, `extract_failed`, `introspection_failed`, `case_not_found`, `case_exists`, `invalid_case`, `invalid_input`, `input_type_not_allowed`, `input_overlaps_case`, `password_required`, `invalid_timezone`, `profile_not_found`, `profile_invalid`, `profile_exists`, `unknown_modules`, `report_missing`, `path_not_allowed`, `path_too_long`, `permission_denied`, `io`, `internal`.

`message` is human-readable and safe to show. `detail` may hold technical context (never secrets).

## 13. Acquisition (F11)

### 13.1 Enumerations

| Type | Values |
|---|---|
| `AcqStatus` | `running`, `succeeded`, `failed`, `cancelled`, `interrupted` |
| `AcqPhase` | `preparing`, `enabling_encryption`, `backing_up`, `restoring_encryption`, `validating`, `sealing`, `finalizing` |
| `PairState` | `paired`, `not_paired`, `awaiting_trust`, `locked`, `trust_denied`, `unknown` |
| `IdeviceToolSource` | `bundled`, `system`, `dev_override` |
| `IdeviceToolsState` | `ok`, `missing`, `verification_failed`, `usbmuxd_unavailable` |

### 13.2 `idevice-tools.json` (repo root, embedded at build time; maintained by ROADMAP X1)

```json
{
  "schema_version": 1,
  "version": "1.4.0",
  "sources": [
    { "name": "libplist", "version": "2.7.0", "url": "https://github.com/libimobiledevice/libplist/releases/download/2.7.0/libplist-2.7.0.tar.bz2", "sha256": "…" },
    { "name": "libimobiledevice", "version": "1.4.0", "url": "…/libimobiledevice-1.4.0.tar.bz2", "sha256": "…" }
  ],
  "platforms": {
    "macos-aarch64":  { "bundle": "idevice-tools-1.4.0-macos-aarch64.tar.gz", "bundle_sha256": "…",
                        "files": { "idevice_id": "…", "ideviceinfo": "…", "idevicepair": "…", "idevicebackup2": "…" } },
    "macos-x86_64":   { "…": "same shape" },
    "windows-x86_64": { "…": "same shape; files include .exe names and any required .dll" }
  },
  "system_platforms": ["linux-x86_64", "linux-aarch64"]
}
```

- `sources` lists every tarball the build consumes, including TLS/curl dependencies if they are built from source.
- `files` maps each bundled file to its SHA-256; the app verifies these before every use.
- `system_platforms` use tools found on `PATH` (hashes recorded, not pinned).

### 13.3 `acquisition.json` (the acquisition audit record)

```json
{
  "schema_version": 1,
  "acq_id": "20260924-171200Z-ios-9c01de",
  "label": "Suspect iPhone 12",
  "status": "succeeded",
  "status_reasons": [],
  "warnings": [],
  "created_at": "2026-09-24T17:12:00Z",
  "started_at": "2026-09-24T17:12:04Z",
  "ended_at": "2026-09-24T17:48:51Z",
  "recovered_at": null,
  "duration_ms": 2211000,
  "app": { "name": "suiteDFIR", "version": "0.2.0" },
  "host": { "os": "macos", "os_version": "15.6", "arch": "aarch64", "hostname": "LAB-MAC-01" },
  "case_snapshot": { "case_id": "5b0c…9e7c", "name": "Operation Nightjar", "case_number": "2026-0142", "examiner": "J. Doe", "agency": "County Forensics Lab" },
  "device": {
    "udid": "00008101-000A1B2C3D4E001E", "serial_number": "F2LXXXXXXX", "device_name": "Alex's iPhone",
    "product_type": "iPhone13,2", "product_version": "18.6", "build_version": "22G86", "captured_at": "2026-09-24T17:12:01Z"
  },
  "tools": {
    "version": "1.4.0", "source": "bundled", "verified_against": "manifest",
    "binaries": { "idevicebackup2": { "path": "/Applications/suiteDFIR.app/Contents/MacOS/idevicebackup2", "sha256": "…" } }
  },
  "encryption": {
    "will_encrypt_before": false, "enable_requested": true, "enabled_by_examiner": true,
    "restore_requested": true, "restored_after": true, "password_supplied": true
  },
  "commands": [
    { "purpose": "enable_encryption", "argv": ["…/idevicebackup2", "-u", "00008101-…", "encryption", "on", "<redacted>"], "exit_code": 0, "started_at": "…", "exited_at": "…" },
    { "purpose": "backup", "argv": ["…/idevicebackup2", "-u", "00008101-…", "backup", "--full", "/…/acquisitions/20260924-171200Z-ios-9c01de/backup"], "exit_code": 0, "started_at": "…", "exited_at": "…" },
    { "purpose": "restore_encryption", "argv": ["…/idevicebackup2", "-u", "00008101-…", "encryption", "off", "<redacted>"], "exit_code": 0, "started_at": "…", "exited_at": "…" }
  ],
  "process": { "exit_code": 0, "signal": null, "cancel_requested": false, "escalated_to_kill": false },
  "backup_result": {
    "success_message_seen": true, "udid_dir": "backup/00008101-000A1B2C3D4E001E",
    "manifest_found": "Manifest.db", "info_plist_found": true, "status_plist_found": true,
    "snapshot_state": "finished", "last_progress_percent": 100
  },
  "output": {
    "backup_dir": "backup",
    "seal": { "status": "sealed", "manifest": "backup.sha256", "manifest_sha256": "…", "file_count": 48210, "total_bytes": 61203455110 }
  },
  "logs": { "stdout": "idevicebackup2.stdout.log", "stderr": "idevicebackup2.stderr.log" }
}
```

**Record rules:**
- `acq_id` = `YYYYMMDD-HHMMSSZ-ios-<6 hex>`.
- `process` describes the backup command.
- Times follow the §7.1 rules.
- Passwords are always `<redacted>` in argv.
- The initial record and recovery follow §7.2: `commands: []`, `process`/`backup_result` `null`, seal `pending`.

**Status rules** (same structure as §7.3):

1. **Short-circuits** (only this reason is recorded):
   - `prepare_failed`.
   - `encryption_enable_failed`.
   - `spawn_failed`.
   - Cancel requested before the backup's exit was observed → `cancelled` (`cancelled_by_user`).
2. **Otherwise, evaluate each check** and add a reason for each match:

   | Check | Reason code |
   |---|---|
   | Exit code ≠ 0 | `nonzero_exit` |
   | Killed by a signal | `killed_by_signal` |
   | `Backup Successful.` not seen | `success_message_missing` |
   | `backup/<udid>/` missing | `backup_dir_missing` |
   | Neither `Manifest.db` nor `Manifest.mbdb` | `manifest_missing` |
   | `Info.plist` missing | `info_plist_missing` |
   | `Status.plist` missing | `status_plist_missing` |
   | `SnapshotState` ≠ `finished` | `snapshot_not_finished` |

3. **Status:** any match → `failed`; otherwise `succeeded`.
4. **Warnings:**
   - `encryption_restore_failed`;
   - `seal_failed`;
   - `symlinks_in_backup`;
   - `unencodable_filename`;
   - `already_encrypted_unknown_password` (`WillEncrypt` was true before the acquisition, so parsing needs the owner's password).

### 13.4 Expected outcomes for fake-idevice scenarios (normative for tests)

| `FAKE_IDEVICE_SCENARIO` | Behavior | Status | Reasons | Warnings |
|---|---|---|---|---|
| `success` | 1 paired device; backup streams progress and writes a valid layout | `succeeded` | none | none |
| `success_encrypt` | as `success`; `encryption on`/`off` succeed | `succeeded` | none | none; `enabled_by_examiner` and `restored_after` true |
| `already_encrypted` | `WillEncrypt` true | `succeeded` | none | `already_encrypted_unknown_password` |
| `restore_fail` | `encryption off` fails | `succeeded` | none | `encryption_restore_failed` |
| `enable_fail` | `encryption on` fails | `failed` | `encryption_enable_failed` | none |
| `backup_fail` | prints `Backup Failed (Error Code 105).`, exit 1 | `failed` | `nonzero_exit`, `success_message_missing`, then whichever layout checks fail | none |
| `incomplete` | exit 0 + success message, but `SnapshotState` = `new` | `failed` | `snapshot_not_finished` | none |
| `slow` + cancel | on SIGTERM prints `Backup Aborted.` and exits within 2 s | `cancelled` | `cancelled_by_user` | none |
| `ignore_term` + cancel (Unix) | ignores SIGTERM | `cancelled` | `cancelled_by_user` | none; `escalated_to_kill` true |
| `not_paired` | `validate` → not paired; the first `pair` → trust dialog; the second → success | none (tests `device_pair` states) | | |
| `locked`, `trust_denied` | respective `idevicepair` errors | none (tests states) | | |
| `usbmuxd_missing` | `idevice_id` fails to connect | none (`devices_list` → tools state `usbmuxd_unavailable`) | | |

### 13.5 IPC types, commands and events

```ts
type DeviceSummary = {
  udid: string; device_name: string | null; product_type: string | null; product_version: string | null;
  serial_number: string | null; pair_state: PairState; will_encrypt: boolean | null;
  data_used_bytes: number | null; data_capacity_bytes: number | null; message: string | null;
};
type DevicesResult = {
  tools: { source: IdeviceToolSource | null; version: string | null; state: IdeviceToolsState; guidance: string | null };
  devices: DeviceSummary[];
};
type AcqRequest = {
  case_path: string; udid: string; label: string | null;
  enable_encryption: boolean; encryption_password: string | null; restore_encryption: boolean;
};
type AcqSummary = {
  acq_id: string; acq_dir: string; label: string | null; status: AcqStatus; udid: string;
  device_name: string | null; product_version: string | null; created_at: string; started_at: string | null;
  ended_at: string | null; duration_ms: number | null; backup_path: string | null; // abs path of backup/<udid> when succeeded
};
```

| Command | `req` | Returns | Notes |
|---|---|---|---|
| `devices_list` | none | `DevicesResult` | Never throws for tool or usbmuxd problems; they are reported in `tools.state`. |
| `device_pair` | `{udid}` | `DeviceSummary` | Runs `idevicepair pair`. Trust and lock outcomes are states, not errors. |
| `acq_start` | `AcqRequest` + `on_event: AcqEvent` | `{acq_id, acq_dir}` | Validation per ARCHITECTURE.md §6b step 3. |
| `acq_cancel` | `{acq_id}` | none | Idempotent. |
| `acq_get` | `{case_path, acq_id}` | `AcquisitionRecord` | |
| `open_acq_file` | `{case_path, acq_id, which: "stdout"\|"stderr"\|"acquisition_json"\|"backup_manifest"}` | none | |

```ts
type AcqEvent =
  | { type: "phase"; phase: AcqPhase }
  | { type: "log"; lines: string[] }
  | { type: "progress"; percent: number; bytes_done: number | null; bytes_total: number | null }
  | { type: "seal_progress"; files_done: number; files_total: number | null }
  | { type: "finished"; status: AcqStatus; reasons: Reason[]; warnings: Reason[]; summary: AcqSummary };
```

**Parse handoff:** the core never retains backup passwords. If the examiner ticks "Parse with iLEAPP now", the **UI** keeps the password it already has in memory, pre-fills the New run form (input = `backup_path`, type `itunes`), and clears it when that run starts or the form closes.
