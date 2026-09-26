# QA checklist (release candidate)

The human checks that CI and the local gates cannot do: real devices, real evidence, desktop sessions and installers (ROADMAP G2, H4). Run it on the release-candidate builds from the draft release, on **macOS and Windows** (required) and **Linux** (recommended).

Each item says where it applies: **[macOS]**, **[Windows]**, **[Linux]**, or **[all]** for every system. Record for each run of the checklist:

- tester, date, suiteDFIR build (file name and SHA-256 from `SHA256SUMS`);
- per system: OS version and architecture; for devices: model, iOS version, and whether a passcode is set;
- for each item: pass / fail / not run, with a short note (and the run or acquisition id where one exists).

Keep real evidence, passwords and device data out of the repository and out of issue text. Where an item resolves an open question in docs/LEAPP-CLI.md §9 or docs/IDEVICE-CLI.md §8, update that document with the observed behaviour (a docs-only PR).

## 1. Installers and first start

- [ ] **[all]** Each installer on each OS installs and starts: the macOS arm64 and x64 dmg, the Windows online and offline installers, the Linux AppImage and deb.
  - [ ] **[macOS]** The unsigned app: the Gatekeeper refusal and the way past it described in the user guide (Open Anyway, or `xattr -dr com.apple.quarantine`), on macOS 11 (the minimum) and the current release; the x64 dmg on an Intel Mac or under Rosetta.
  - [ ] **[Windows]** SmartScreen on the unsigned installers; the installation needs no administrator rights; uninstalling leaves the case folders untouched.
  - [ ] **[Windows]** The offline installer on a machine without internet access and without the WebView2 runtime (for example a fresh Windows 10 VM): WebView2 is installed from the installer and the app starts.
  - [ ] **[Windows]** The online installer on a machine without WebView2 but with internet access downloads it.
  - [ ] **[Linux]** The AppImage starts (with and without FUSE); the deb installs with `apt` on Ubuntu 22.04 and 24.04; the `WEBKIT_DISABLE_DMABUF_RENDERER=1` fallback on a machine or VM with a blank window.
- [ ] **[all]** The bundled iOS tools are verified: the Acquire screen shows the tools as ready (no verification banner); in `acquisition.json`, `tools.binaries.*.verified_against` is `manifest` (macOS, Windows) or `recorded_only` (Linux).
- [ ] **[all]** Settings → About: versions, paths, the privacy statement, and **Third-party licenses** shows `THIRD-PARTY-NOTICES.md`.
- [ ] **[all]** A second instance shows a message and exits, without a window and without touching the first instance's state.

## 2. Parsers

- [ ] **[all]** Online install of iLEAPP and aLEAPP: download, verification, module lists; the Verify action.
- [ ] **[all]** Offline import on an air-gapped machine: copy the pinned release files, import them, and run a parse without any network.
- [ ] **[Windows]** The tools-folder override on a machine with AppLocker or WDAC (if available): parsers blocked in the default folder run from an approved one.
- [ ] **[Linux]** On a system with glibc older than 2.43 (for example Ubuntu 22.04 or 24.04), installing a parser fails with the glibc message; on Ubuntu 26.04 it installs and runs.
- [ ] **[Linux]** linux-aarch64: the minimum glibc of the pinned LEAPP arm64 builds is not inspected (docs/LEAPP-CLI.md §2). Install and run both parsers on an arm64 Linux machine with glibc 2.43 and, if possible, one with 2.42, and record the result there.

## 3. Parsing evidence

- [ ] **[macOS] [Windows]** Real Finder/iTunes backups, one **unencrypted** and one **encrypted**:
  - [ ] the encrypted one with the **right** password: `succeeded` or `completed_with_errors`, with modules that need decryption producing data;
  - [ ] the encrypted one with a **wrong** password: `failed` with `no_modules_ran` (or the observed result, recorded here);
  - [ ] **grep the whole run folder (report, logs, `run.json`) for the password**: it must appear nowhere, including `Screen_Output.html` and `_lava_data.lava` (LEAPP-CLI §9 item 2).
- [ ] **[macOS]** A backup in `~/Library/Application Support/MobileSync/Backup` without Full Disk Access shows the Full Disk Access guidance; after granting it, the backup can be parsed.
- [ ] **[all]** An Android file-system extraction (folder) and a zip of it with aLEAPP.
- [ ] **[all]** An E01 image (type `raw`) with iLEAPP or aLEAPP.
- [ ] **[all]** A large input (≥ 100 GB) with input hashing on: the hash progresses, the run and the hash finish, and `input.hash` in `run.json` matches an independent `sha256sum` of the input.
- [ ] **[all]** Cancel in each phase (preparing, running, hashing the input, analyzing, sealing the report): the status is `cancelled` or as documented, no parser process is left (Activity Monitor, Task Manager, `ps`), and the per-run temp folder is gone.
- [ ] **[all]** Quit during a run: the confirmation appears; quitting cancels the run and waits for it; the record is final (or `interrupted` on the next open).
- [ ] **[Windows]** While a run is watched on a desktop session, no LEAPP console window appears (`CREATE_NO_WINDOW`; LEAPP-CLI §9 item 1).
- [ ] **[all]** `sha256sum -c report.sha256` passes in a run folder, and fails after a report file is changed on a copy of the folder.
- [ ] **[Windows]** Junctions on a normal desktop session (K2 follow-up FU27): on the gate machine a non-admin's junctions are untraversable (Redirection Guard), so a case whose `runs/` is a junction was refused with `io`. On a normal desktop session, make `runs/` (in a test case) a junction to another folder and choose an input inside the junction target: the run must be refused with `input_overlaps_case`.

## 4. iOS acquisition over USB, with a real iPhone

Run on **[macOS]** and **[Windows]** (Apple Devices app or iTunes installed); **[Linux]** recommended with the distribution's `usbmuxd` and `libimobiledevice-utils`.

- [ ] First-time trust: the device is listed as *Not paired*, and **polling never shows a Trust prompt on the device by itself** (wait at least a minute on the Acquire screen); only **Pair** does. Then the Trust flow to *Paired*.
- [ ] A locked device: *Locked*, then *Paired* after unlocking and retrying.
- [ ] Encryption enable and restore with passcode prompts: the device-prompt banner appears, the app waits without a time limit, and the device's backup encryption is off again at the end; `acquisition.json` records both changes.
- [ ] A device already encrypted with an **unknown password**: the backup is taken with the `backup_encryption_preexisting` warning, encryption is not offered, and nothing on the device changes.
- [ ] Cancel during **enable**, during the **backup** and during the **restore**: behaviour as in the user guide (enable completes, then restore; backup stops, then restore; restore completes); the device ends with encryption off.
- [ ] **Unplug mid-backup**: `failed` with `device_disconnected`; if encryption was enabled, `encryption_left_enabled` and the later-restore action.
- [ ] **Finder, iTunes or Apple Devices open** and syncing the device: the sync-lock failure (`sync_lock_failed`); record the exact stderr lines (IDEVICE-CLI §8 item 8).
- [ ] Later **"Turn backup encryption off"**: a failed attempt (wrong password or device locked) writes `encryption-restore.json` and can be retried; the successful retry writes `encryption-restore-2.json`, and the action disappears.
- [ ] The **"Parse with iLEAPP"** handoff: New run is prefilled (backup path, type `itunes`, the password only if set in this session); the run records `input.acquisition_id`; the password field is cleared after the run starts.
- [ ] `device-info.plist` never appears in the app log (`suitedfir.log`): search the log for the device's serial number, IMEI and phone number.
- [ ] **[Windows]** A cases folder with non-ASCII characters or a long path: the acquisition is refused with `path_not_supported_by_tool` before anything changes on the device.
- [ ] **[Windows]** An encryption password with a non-ASCII character is refused with `invalid_input` before anything changes on the device.
- [ ] **[Windows]** Stop the Apple Mobile Device Service (as an administrator): the Acquire screen shows the service guidance; start it again and the device is listed.

### Open items from docs/IDEVICE-CLI.md §8 (resolve and update the document)

- [ ] **[all]** 1. `SnapshotState == "finished"` and `Manifest.db` present after a successful backup on iOS 17, 18 and 26.
- [ ] **[all]** 2. The exact on-device prompts (passcode for backup and for encryption changes) per iOS version, and the console lines the tools print for them.
- [ ] **[all]** 3. Real-device progress output: bursts, and the `NN% Finished` lines.
- [ ] **[Windows]** 4. The tools reach the Apple Mobile Device Service without extra configuration and talk to a real device through it.
- [ ] **[Linux]** 5. The minimum distribution package versions that support current iOS.
- [ ] **[all]** 6. The graceful abort time after a cancel (SIGTERM on macOS/Linux) on a large backup, against the 30 s grace.
- [ ] **[all]** 7. The `ideviceinfo -s` field subset for an unpaired device on current iOS.
- [ ] **[macOS] [Windows]** 8. The sync-lock failure string, and the behaviour when Finder or iTunes is open (above).
- [ ] **[all]** 9. **WillEncrypt absent** (K7): on a device that never had backup encryption set, whether the `com.apple.mobile.backup` domain is a dictionary without `WillEncrypt` (read as off, so enabling is offered) or nothing at all (read as unknown, so enabling is blocked). Record which, and that the app behaves accordingly.
- [ ] **[all]** 10. **Enable unconfirmed** (K7): whether `WillEncrypt` reads `true` right after `encryption on` reports success, or only after a delay. If it reads `false`, the acquisition warns `encryption_state_unknown` and still attempts the restore; record which happens.

## 5. Accessibility and appearance

- [ ] **[macOS]** A VoiceOver spot check (and **[Windows]** Narrator if available): the navigation, the New run form, the module picker, the run log's live region (at most one announcement per second), dialogs and error messages.
- [ ] **[all]** Full keyboard operation of the main flows with visible focus.
- [ ] **[all]** Dark mode on each screen.

## 6. Results

Summarize the failures as issues (without evidence data), and record in the release notes anything the release ships with knowingly. The owner reviews the draft release and publishes it (H3).
