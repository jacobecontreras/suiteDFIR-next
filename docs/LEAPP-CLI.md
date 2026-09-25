# LEAPP CLI reference (verified behavior)

Facts about the upstream iLEAPP/aLEAPP command-line builds that suiteDFIR depends on. They were verified on **2026-09-24** in two ways:
- by running `ileapp v2026.4.2` (macOS arm64 build);
- by reading source at tags `iLEAPP v2026.4.2` and `ALEAPP v2026.4.1`.

Items marked **UNVERIFIED** must be confirmed by ROADMAP task E3 before anything relies on them. §9 is the running list. When you bump the pinned version, re-run the smoke suite and update this file.

## 1. Pinned releases

**Upstream:** `abrignoni/iLEAPP` and `abrignoni/ALEAPP`. Releases use calendar versioning and come out almost weekly.

**Asset hygiene:**
- Assets are uploaded by hand: there is no release workflow and no build attestations.
- Asset naming has drifted between releases (`…Windows_x86_64.zip.zip`, `…Windows_x64.zip`, `…-Ubuntu.zip` in older releases).
- So always pin **exact asset names**.

iLEAPP **v2026.4.2**:

| Platform | Asset | Bytes | SHA-256 |
|---|---|---|---|
| macos-aarch64 | `ileapp-v2026.4.2-macOS_Apple_Silicon.zip` | 55085487 | `d99f2d05dbde20ee997de477c38443d4f019d60326a8c6b9456058c6cf590386` |
| macos-x86_64 | `ileapp-v2026.4.2-macOS_Mac_Intel.zip` | 59380709 | `32889a7b849d54a973b90e9b1fb25e6e9e43a67416ce76486029e71a4115c94a` |
| windows-x86_64 | `ileapp-v2026.4.2-Windows_x86_64.zip` | 62104758 | `805fa3e4324f7422589bdb5704a42ebd6822b17ddd180efbd124cb01af86af53` |
| windows-aarch64 | `ileapp-v2026.4.2-Windows_arm64.zip` | 55163746 | `4f5d45e46643d8247cc3020569bdb269643ff3321052270d9be57627cf878b8a` |
| linux-x86_64 | `ileapp-v2026.4.2-Linux_x86_64.AppImage` | 71145976 | `3cfbb92709166524572848df4064e4c702d33a999e0405c549cfab0794769c95` |
| linux-aarch64 | `ileapp-v2026.4.2-Linux_arm64.AppImage` | 73271816 | `80e03b52bdf8f0455cd305615ddcf1700d4b57b28b544e6da7752b57da265a5c` |

aLEAPP **v2026.4.1**:

| Platform | Asset | Bytes | SHA-256 |
|---|---|---|---|
| macos-aarch64 | `aleapp-v2026.4.1-macOS_Apple_Silicon.zip` | 41209358 | `2829b69c0f9035035d95936ee0a5168674fa6f4ffde01706ee3c6c1d2ecaf7f0` |
| macos-x86_64 | `aleapp-v2026.4.1-macOS_Mac_Intel.zip` | 43367347 | `dde43304acce737032b803b2a92ad0b225c64c6f37c743aab2c6a4a61eecb754` |
| windows-x86_64 | `aleapp-v2026.4.1-Windows_x86_64.zip` | 48827767 | `8eb1ad25f9fdc89af345277ad6ff748636e3552490e39a4fd379debde0af70fa` |
| windows-aarch64 | `aleapp-v2026.4.1-Windows_arm64.zip` | 41775481 | `fa52fdcc81fdfa1617c5aa87e02dea2d241d5ea9b891da36076ad27082c16325` |
| linux-x86_64 | `aleapp-v2026.4.1-Linux_x86_64.AppImage` | 63834616 | `0344db7fce169772b807a88fde3ee2eb7ff06c12c9c360b29928d50e7fc854be` |
| linux-aarch64 | `aleapp-v2026.4.1-Linux_arm64.AppImage` | 63101448 | `6075153896a30d35e96aeefa47b73936a41aa78e6a0566cc03ba57e697563ebb` |

**Digests:** `gh api repos/abrignoni/iLEAPP/releases/tags/<tag> --jq '.assets[]|{name,size,digest}'`. Downloaded assets matched these API digests (VERIFIED for 3 assets).

**Licensing:**
- LEAPP is MIT-licensed. The zips contain no LICENSE file, so ship the notice ourselves.
- The binaries bundle libheif/libde265 (LGPL-3). If we ever redistribute the binaries (mirror or offline bundle), include those notices too.

## 2. Packaging per platform

- **macOS:**
  - The zip holds one PyInstaller **onefile** binary (arm64 ≈ 55.8 MB).
  - Developer ID signed (Johann POLEWCZYK, team N2G83326TZ), hardened runtime; `spctl` reports "Notarized Developer ID".
  - Runs headless: `--help` ≈ 1 s, exit 0. VERIFIED.
- **Windows:** one `ileapp.exe` (≈ 62.7 MB), onefile, **console** subsystem, **unsigned** (empty PE security directory). VERIFIED by inspection; not executed.
- **Linux:**
  - A type-2 AppImage (≈ 71 MB) with the static runtime. Mounting needs FUSE; `--appimage-extract` does not.
  - It wraps `usr/bin/ileapp`, itself a onefile binary.
  - **glibc ≥ 2.38 is required.** VERIFIED by the A3 smoke run in `ubuntu:22.04` (glibc 2.35) for both tools (v2026.4.2 / v2026.4.1, linux-x86_64): `--appimage-extract` works there as an unprivileged user, but the inner binary exits 255 with `[PYI-…:ERROR] Failed to load Python shared library '…/_MEI…/libpython3.14.so.1.0': /lib/x86_64-linux-gnu/libm.so.6: version 'GLIBC_2.38' not found`. Some bundled libraries reference even newer glibc versions (`libtinfo.so.6` 2.42; iLEAPP's `libmvec.so.1` 2.43), which may matter for the modules that load them.
- **Every run:** extracts ≈ 132 MB into `$TMPDIR/_MEI*` and adds ≈ 2 s of startup.

## 3. Command-line flags used by suiteDFIR

| Flag | iLEAPP | aLEAPP | Notes |
|---|---|---|---|
| `-t {…}` | fs, tar, zip, gz, itunes, file, raw | fs, tar, zip, gz, raw | `raw` is a disk image (.img/.dd/.bin/.001) or E01, read in place. |
| `-i PATH` | ✓ | ✓ | Pass an absolute path; it is stored verbatim in `_lava_data.lava.param_input`. |
| `-o PATH` | ✓ | ✓ | Must already exist. |
| `--custom_output_folder NAME` | ✓ | ✓ | Output goes to `os.path.join(o, NAME)`. If that already exists or NAME is illegal: argparse error, **exit 2**, nothing created. VERIFIED (source). |
| `-m PROFILE` | ✓ | ✓ | Missing file → exit 2. Invalid *content* → prints an error, **exit 0**, no output. Unknown names are silently dropped (Q4). |
| `-d CASEDATA` | ✓ | ✓ | `.lcasedata` JSON (CONTRACTS.md §8). Missing file → exit 2. Invalid content → exit 0, no output. |
| `-tz NAME` | ✓ | ✗ | Validated with `pytz`; an invalid zone → exit 2. Default UTC. aLEAPP has no timezone option. |
| `--itunes_password PW` | ✓ | ✗ | The only non-interactive channel; visible in the process list. |
| `--keychain PATH` | ✓ | ✗ | Keychain file captured separately from the extraction. |
| `--custom_artifacts_path DIR` | ✓ | ✓ | Adds DIR to the same `PluginLoader`. Used only for introspection (§5). |
| `-p`, `-c` | | | Never use. `-c` is interactive; `-p` writes `path_list.txt` to the cwd. |

**Other exit-2 cases** (argparse errors): a missing `-o` or input, and an empty `fs` input dir. suiteDFIR pre-validates these so exit 2 should not occur, but status rules handle it (CONTRACTS.md §7.3).

## 4. Invocation used by suiteDFIR

```
<entry> -t <type> -i <abs input> -o <abs run_dir> --custom_output_folder report \
        -d <abs run_dir>/case.lcasedata [-m <abs run_dir>/profile.<ext>] \
        [-tz <zone>]                                      # iLEAPP only, always passed
        [--itunes_password <pw>] [--keychain <abs path>]  # iLEAPP only
```

**Process setup:**
- cwd = run dir; stdin = null; no controlling terminal (`setsid`).
- Env: inherit, plus `TMPDIR`, `TEMP` and `TMP` = the per-run temp dir. The bootloader honours `TMPDIR` (VERIFIED on macOS); `TEMP`/`TMP` on Windows is UNVERIFIED.
- `PYTHON*` variables have **no effect** on the frozen binary (VERIFIED).

## 5. Introspection: modules, always-run and timezones (D11)

The CLI cannot list modules; the binary's own loader can. VERIFIED: 1,176 iLEAPP and 1,288 aLEAPP entries, matching the runtime. After the tool rules below, 1,138 iLEAPP and 1,287 aLEAPP modules are selectable, with identical lists on macOS arm64 and Windows x64 (A3 smoke).

**Loader facts** (from source at the pinned tags):
- `--custom_artifacts_path` feeds the same `PluginLoader` as the built-in artifacts (iLEAPP `ileapp.py:231-236`, aLEAPP `aleapp.py:197-200`).
- A profile filters by name with no validation, so a profile containing only the probe's name selects only the probe (plus always-run artifacts).
- A v2 entry registers only if its function is decorated **or** the dict has a `"function"` key. The function is called only when files matching `paths` are found.
- iLEAPP passes 5 positional arguments to artifact functions; aLEAPP passes 4.

**Procedure:**

1. Create a temp dir containing:
   - `probe_artifacts/suitedfir_probe.py` (`leapp::modules::PROBE_SOURCE`). The dict keys follow the current upstream artifacts at the pinned tags (iLEAPP `scripts/artifacts/lastBuild.py`, aLEAPP `scripts/artifacts/usagestatsVersion.py`), plus `function`, which registers the undecorated function:
     ```python
     __artifacts_v2__ = {
         "suitedfir_probe": {
             "name": "suiteDFIR probe",
             "description": "Lists the artifacts of this build for suiteDFIR",
             "author": "suiteDFIR",
             "creation_date": "2026-09-25",
             "last_update_date": "2026-09-25",
             "requirements": "none",
             "category": "suiteDFIR",
             "notes": "",
             "paths": ("*/suitedfir_probe.marker",),
             "output_types": [],
             "artifact_icon": "list",
             "function": "suitedfir_probe",
         }
     }


     def suitedfir_probe(files_found, report_folder, seeker, wrap_text, *args):
         import json, os
         from scripts.plugin_loader import PluginLoader
         out = {"plugins": [
             {"name": p.name, "module_name": p.module_name, "category": p.category,
              "display_name": (p.artifact_info or {}).get("name"),
              "description": (p.artifact_info or {}).get("description")}
             for p in PluginLoader().plugins]}
         try:
             import pytz
             out["timezones"] = list(pytz.all_timezones)
         except Exception:
             out["timezones"] = None
         path = os.environ["SUITEDFIR_PROBE_OUT"]
         with open(path + ".partial", "w", encoding="utf-8") as f:
             json.dump(out, f)
         os.replace(path + ".partial", path)
     ```
     `PluginLoader()` without arguments loads only the built-in artifacts, so the probe does not list itself.
   - `input/suitedfir_probe.marker` (non-empty dir, avoiding the exit-2 case).
   - An empty `out/`.
   - `probe.<ext>` = `{"leapp": "<tool>", "format_version": 1, "plugins": ["suitedfir_probe"]}`.
2. Run `<entry> -t fs -i <tmp>/input -o <tmp>/out --custom_output_folder probe --custom_artifacts_path <tmp>/probe_artifacts -m <tmp>/probe.<ext>` through `process` (same session/job and temp rules) with `SUITEDFIR_PROBE_OUT` set. The temp dir is `<app_cache>/tmp/<id>` with a run-id-shaped `id` (`YYYYMMDD-HHMMSSZ-<ileapp|aleapp>-<6 lowercase hex>`), which the `process` temp-dir functions require; it is only a directory name under `<app_cache>/tmp`, so it never collides with a real run's folder. Timeout 180 s, then cancel and fail with `introspection_failed`. The exit code is not used; the probe's output decides.
3. Read the JSON, drop the probe itself, and apply the tool's rules:
   - **iLEAPP v2026.4.2:**
     - Selection excludes `module_name == "iTunesBackupInfo"`, `name == "last_build"`, and `module_name == "logarchive" and name != "logarchive"`.
     - `always_run`: `default: ["last_build"]`. With `-t itunes`, `last_build` is replaced by `itunes_backup_info` and `itunes_backup_installed_applications` (from source; that they run this way is UNVERIFIED until E3).
     - The always-run plugins' module names (which the status rules also match, CONTRACTS.md §7.3) are `lastBuild` and `iTunesBackupInfo`.
     - `timezones` = the probe's `pytz.all_timezones`; a missing or empty list fails (runs validate `-tz` against it, D19).
   - **aLEAPP v2026.4.1:** nothing is excluded from the plugin list except that plugins with `module_name == "usagestatsVersion"` are removed from the selectable list and always run first (`aleapp.py:206-212, 329`). `always_run.default` = those plugins' names (`["usagestatsVersion"]`). `timezones` is `null`.
   - Introspection checks the binary against these rules: each always-run artifact must exist in its encoded module, and no selectable plugin may be in an always-run module. Otherwise it fails with `introspection_failed` (the rules are stale for that binary).
4. Fewer than 500 selectable modules → `introspection_failed`.

## 6. Output layout (`<o>/<custom_output_folder>/`)

The output folder contains (VERIFIED):
- `index.html`
- `_HTML/`, including `_HTML/_Script_Logs/Screen_Output.html`
- `_lava_artifacts.db`
- `_lava_data.lava`
- `_TSV Exports/`
- `data/` (copies of matched evidence files)
- `media/`

`_lava_data.lava` (JSON), relevant fields:

```json
{
  "lava_schema_version": "…",
  "parser_info": {"leapp_name": "iLEAPP", "leapp_version": "2026.4.2", "package": "Binary", "start_timestamp": 1727202605},
  "param_input": "…", "param_output": "…", "param_type": "itunes", "param_profile": "…|null",
  "processing_status": "Complete",
  "modules": [{"module_name": "callHistory", "module_status": "Complete|Error|No files found", "artifact_name": "…", "file_count": 3}]
}
```

- `processing_status` is set to `"Complete"` only in `lava_finalize_output`.
- The file is written only at finalize. A crashed or cancelled run therefore leaves **no** `_lava_data.lava` and may leave `_lava_artifacts.db-journal`.

## 7. Quirks and how suiteDFIR handles them

| # | Quirk (evidence) | Handling |
|---|---|---|
| Q1 | stdout is block-buffered when piped. All 45 lines of an 8 s run arrived at t = 10.2 s, and a cancelled run captured 0 bytes. `PYTHONUNBUFFERED` is ignored. A pty or tailing `Screen_Output.html` streams line by line. | D7: tail `Screen_Output.html`; save stdout/stderr to files; show their tails after exit. |
| Q2 | Onefile = bootloader + worker. SIGKILL to the parent orphans the worker (re-parented to PID 1, still running) and leaks `_MEI*`. SIGTERM to the parent or the group → both exit in ≈ 0.18 s and `_MEI` is cleaned. | D9 + D10. |
| Q3 | The exit code is meaningless for success. An invalid iTunes folder logged "not a valid iTunes backup", exited 0, and `_lava_data.lava` said `Complete` with **empty** `modules` and no `index.html`. Failed artifacts also exit 0. Invalid profile/case-data content → exit 0, no output. Argparse errors → exit 2. | D8 status rules (CONTRACTS.md §7.3). |
| Q4 | Profiles: unknown plugin names are silently dropped (`["noSuchModule","callHistory"]` ran `last_build` + `callHistory`, exit 0). Upstream's own sample profiles already contain removed names. | D12: validate before the run; record the resolved modules. |
| Q5 | With no password, an encrypted backup triggers a prompt. iLEAPP opens `/dev/tty` before stdin. A child in a background process group of a terminal session gets SIGTTOU/SIGTTIN-stopped. | `setsid` (no controlling terminal) + stdin null + password required when `IsEncrypted`. Behavior after `setsid` is UNVERIFIED (expected: the tty open fails → EOF → error exit). |
| Q6 | `-tz` defaults to UTC silently; aLEAPP has none. | D19. |
| Q7 | `param_input` is stored exactly as passed. | Always pass absolute paths. |
| Q8 | `Screen_Output.html` records are `message + "<br>" + newline`, appended with an open/close per message. Messages are **not** HTML-escaped and may contain markup or evidence-derived text. On Windows the newline is probably `\r\n` (UNVERIFIED). | Split on `<br>` followed by `\n` or `\r\n`; keep partial records; strip tags; render as text. |
| Q9 | Asset naming drift and manual uploads; no attestations. The frozen artifact sources matched tag v2026.4.2 byte-for-byte (VERIFIED). | D6: exact names plus asset **and** entry SHA-256 pinned in the app. Plan a mirror (H2). |
| Q10 | LEAPP copies matched evidence files into `report/data/` and opens SQLite DBs read-only. The input tree was unchanged after runs. Nothing was written to `~/Library/Application Support/LEAPP` (history is opt-in). | Principle 1 holds; seal the report (F6). |
| Q11 | aLEAPP `--help` does not print its version. | The version comes from the manifest and `_lava_data.lava.parser_info`. |

## 8. Candidate upstream improvements (optional track U, owner approval H5)

- non-zero exit codes on invalid input or failures;
- a `--list-artifacts --json` flag;
- flush in `logfunc`;
- password via env var or stdin;
- a warning on unknown profile names;
- a Windows code signature.

## 9. UNVERIFIED items (E3 must resolve each, per platform)

1. Windows: the bootloader honours `TEMP`/`TMP`; `Screen_Output.html` newline style; the console window stays hidden with `CREATE_NO_WINDOW`.
2. Linux: AppImage extraction works on `ubuntu:22.04`, and the inner binary runs there (glibc).
   - **Resolved by A3 (negative):** extraction works, the inner binary does not run (glibc ≥ 2.38 required, §2). Which Linux baseline the smoke runs and the app support is an open decision.
3. iLEAPP `-t itunes` always-run artifact names; aLEAPP always-run names.
   - **A3:** introspection confirms on macOS arm64 and Windows x64 that the binaries contain `last_build` (module `lastBuild`), `itunes_backup_info` and `itunes_backup_installed_applications` (module `iTunesBackupInfo`) and aLEAPP `usagestatsVersion` (module `usagestatsVersion`). That they run as described in §5 still needs E3 runs.
4. Behavior of the password prompt after `setsid` with stdin null (fake `prompt` scenario mirrors the expected result).
5. The iTunes password never appears in `Screen_Output.html`, `_lava_data.lava` or other report files. It can't be tested in CI without an encrypted fixture, so it is also in the G2 QA checklist.
6. AppImage `entry_sha256` values for linux-x86_64 and linux-aarch64 (fill the manifest).
