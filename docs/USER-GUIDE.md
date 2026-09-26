# suiteDFIR user guide

suiteDFIR 0.2.0 runs the iLEAPP (iOS) and aLEAPP (Android) mobile-forensics parsers and takes iOS backups over USB. It keeps the results in case folders, with an audit record and a hash manifest for every run and acquisition. This guide covers installing it, the parsers and the iOS prerequisites, parsing evidence, acquiring an iOS backup, what each result means, verifying a report, and privacy.

Contents:

1. [Install suiteDFIR](#1-install-suitedfir)
2. [First start](#2-first-start)
3. [Install the parsers (online or offline)](#3-install-the-parsers-online-or-offline)
4. [Cases](#4-cases)
5. [Parse evidence](#5-parse-evidence)
6. [Acquire an iOS backup over USB](#6-acquire-an-ios-backup-over-usb)
7. [Verify a report or a backup](#7-verify-a-report-or-a-backup)
8. [Files and folders](#8-files-and-folders)
9. [Privacy](#9-privacy)
10. [Troubleshooting](#10-troubleshooting)

## 1. Install suiteDFIR

Download the files for your system from the release page, together with `SHA256SUMS`, and check them before you install:

- Linux: `sha256sum -c SHA256SUMS --ignore-missing`
- macOS: `shasum -a 256 -c SHA256SUMS --ignore-missing`
- Windows (PowerShell): `Get-FileHash .\suiteDFIR_0.2.0_x64-online-setup.exe -Algorithm SHA256`, then compare the hash with the file's line in `SHA256SUMS`.

**The 0.2.0 builds are not code-signed.** macOS and Windows warn before they run an unsigned app; the steps below say how to proceed. Only do so for a file whose SHA-256 you checked.

| System | File | Notes |
|---|---|---|
| macOS 11 or later, Apple silicon | `suiteDFIR_0.2.0_aarch64.dmg` | iOS tools included |
| macOS 11 or later, Intel | `suiteDFIR_0.2.0_x64.dmg` | iOS tools included |
| Windows 10/11 x64 | `suiteDFIR_0.2.0_x64-online-setup.exe` | Downloads the WebView2 runtime during setup if it is missing |
| Windows 10/11 x64, offline | `suiteDFIR_0.2.0_x64-offline-setup.exe` | Includes the WebView2 runtime installer (for machines without internet access) |
| Windows 10/11 on Arm (arm64) | `suiteDFIR_0.2.0_arm64-online-setup.exe` | Online installer only: downloads the WebView2 runtime during setup if it is missing. **No iOS acquisition** (below) |
| Linux x64 (glibc 2.35 or later) | `suiteDFIR_0.2.0_amd64.AppImage` or `suiteDFIR_0.2.0_amd64.deb` | Uses the distribution's iOS tools; the parsers need glibc 2.43 (below) |
| Linux arm64 (glibc 2.35 or later) | `suiteDFIR_0.2.0_aarch64.AppImage` or `suiteDFIR_0.2.0_arm64.deb` | As Linux x64 |

The iOS acquisition tools (libimobiledevice) are built by the suiteDFIR project from pinned source releases and are part of the macOS and Windows x64 apps. Their source code and build script are attached to every release (see `THIRD-PARTY-NOTICES.md`). There is no build of them for Windows on Arm, so the arm64 Windows app parses evidence but cannot acquire iOS backups: its Acquire screen says "iOS acquisition is not available on this platform." Acquire on a Mac, a Windows x64 PC or Linux, and parse the case there or copy it over.

### macOS

1. Open the `.dmg` and drag **suiteDFIR** to **Applications**.
2. Start it from Applications. Because the build is unsigned, macOS refuses it the first time:
   - If macOS says it cannot check the app for malicious software, open **System Settings → Privacy & Security** and click **Open Anyway** next to the message about suiteDFIR, then confirm.
   - If macOS says the app "is damaged and can't be opened" (common for unsigned apps on Apple silicon), remove the download quarantine in Terminal and start it again:

     ```bash
     xattr -dr com.apple.quarantine /Applications/suiteDFIR.app
     ```
3. **Full Disk Access** (only to parse Finder backups where Finder keeps them): macOS protects `~/Library/Application Support/MobileSync/Backup`. To read backups there, open **System Settings → Privacy & Security → Full Disk Access**, add suiteDFIR, and restart suiteDFIR. Without it, choosing such a backup shows "Access to the input was denied (on macOS, Finder backups need Full Disk Access)", and **Find iOS backups** (section 5) shows "suiteDFIR may not read the Finder backup folder: macOS protects it until suiteDFIR has Full Disk Access." Backups that suiteDFIR acquires itself go into the case folder and need no extra permission.

### Windows

1. Choose the installer:
   - **Online** (`…-online-setup.exe`): small. suiteDFIR needs the Microsoft Edge WebView2 Runtime; Windows 11 and current Windows 10 have it. If it is missing, the installer downloads it, which needs internet access.
   - **Offline** (`…-offline-setup.exe`): includes the WebView2 Runtime installer. Use it on machines without internet access (for example an air-gapped lab machine).
   - **Windows on Arm** (`…_arm64-online-setup.exe`): the online installer for arm64 PCs. There is no arm64 offline installer (the packaging tool would embed the x86 WebView2 installer), so a machine without the WebView2 Runtime needs internet access during setup; Windows 11 on Arm includes the runtime.
2. Run the installer. Because it is unsigned, SmartScreen shows "Windows protected your PC": click **More info**, check that the publisher is shown as unknown and the file name is the one you verified, then **Run anyway**. The installer installs for the current user and needs no administrator rights.
3. Uninstall from **Settings → Apps → Installed apps**. Uninstalling never touches your case folders.

**AppLocker or WDAC.** The parsers are unsigned programs that suiteDFIR downloads into its tools folder (by default inside the app's data folder). If an application control policy only allows programs from approved folders, set **Settings → Storage → Tools folder → Change…** to an approved folder and install the parsers again. The folder may not be inside a case folder. Parsers installed elsewhere are not moved.

### Linux

The app itself runs on Ubuntu 22.04 (glibc 2.35) and newer distributions with WebKitGTK 4.1, on x64 and arm64. The commands below use the x64 file names; on arm64 use `suiteDFIR_0.2.0_aarch64.AppImage` and `suiteDFIR_0.2.0_arm64.deb`.

- **AppImage** (bundles WebKitGTK): `chmod +x suiteDFIR_0.2.0_amd64.AppImage`, then run it. If it does not start because FUSE is missing, install your distribution's FUSE 2 package (`libfuse2`, or `libfuse2t64` on Ubuntu 24.04 and later), or run it as `./suiteDFIR_0.2.0_amd64.AppImage --appimage-extract-and-run`.
- **deb** (Debian, Ubuntu): `sudo apt install ./suiteDFIR_0.2.0_amd64.deb`. It installs the WebKitGTK and GTK packages it needs. It does not depend on the iOS tools; install those only if you acquire iOS backups (section 6).
- **Blank, white or flickering window:** some graphics drivers, virtual machines and remote sessions cannot use WebKitGTK's DMA-BUF renderer. Start suiteDFIR with it turned off:

  ```bash
  WEBKIT_DISABLE_DMABUF_RENDERER=1 ./suiteDFIR_0.2.0_amd64.AppImage
  ```

**The parsers need glibc 2.43 on Linux.** The pinned Linux builds of the parsers were compiled against a newer C library than the app: iLEAPP v2026.4.2 needs glibc 2.43 and aLEAPP v2026.4.1 needs glibc 2.42 (docs/LEAPP-CLI.md §2). Check yours with `ldd --version`. At the time of writing:

| Distribution | glibc | Parsers |
|---|---|---|
| Ubuntu 26.04 | 2.43 | run (tested) |
| Ubuntu 22.04 | 2.35 | do not start (tested) |
| Ubuntu 24.04 | 2.39 | do not start |
| Debian 13 | 2.41 | do not start |
| RHEL / Rocky / Alma 9 and 10 | 2.34, 2.39 | do not start |
| Rolling distributions (Arch, openSUSE Tumbleweed) | current | usually run; check `ldd --version` |

On an older system, installing a parser fails at the module-list step (`introspection_failed`) and a run fails to start (`spawn_failed`), with the message "the pinned Linux iLEAPP build needs glibc X or newer; this system's glibc is too old". X is the highest missing version in the loader's error lines, which can be lower than the full requirement because the loader stops at the first library it cannot load (on Ubuntu 22.04 it reports 2.38). There is no workaround inside suiteDFIR: use a distribution with glibc 2.43, or parse on macOS or Windows. Case folders are portable, so you can acquire on one machine and parse on another.

## 2. First start

suiteDFIR allows one instance at a time; a second start shows a message and exits.

- **Cases folder:** new cases go into `Documents/suiteDFIR Cases` unless you choose another folder (**Settings → Storage → Cases folder**).
- **Case defaults:** **Settings → Case defaults** sets the examiner and agency that new cases start with, and the timezone iLEAPP runs use when their case has none.
- **Tools folder:** **Settings → Storage → Tools folder** shows where the parsers are installed (section 1, AppLocker or WDAC).
- **About:** the version, the pinned parser versions, the app's folders (app data, settings, cache, app log), the privacy statement and **Third-party licenses**.
- **Clean temporary files:** removes leftover per-run temporary folders (normally removed automatically). It is refused while a job runs.

## 3. Install the parsers (online or offline)

suiteDFIR runs the official iLEAPP and aLEAPP command-line builds at pinned versions: iLEAPP **v2026.4.2** and aLEAPP **v2026.4.1**. The app knows the exact file name, size and SHA-256 of each build and of the program inside it, and refuses anything else.

**Online:** **Settings → Parsers → Install v2026.4.2** (or v2026.4.1). The app downloads the release file from GitHub over HTTPS, checks its SHA-256, extracts the program, checks the program's SHA-256, and then runs it once to list its modules and timezones (up to three minutes). It ends with "Installed and verified."

**Offline:** on a machine with internet access, download the exact release file for the target platform from the parser's GitHub release page and copy it over (for example on a USB stick). Then use **Import release file…** in the parser's card. The import runs the same checks. The pinned files are:

| Platform | iLEAPP v2026.4.2 | aLEAPP v2026.4.1 |
|---|---|---|
| macOS Apple silicon | `ileapp-v2026.4.2-macOS_Apple_Silicon.zip` | `aleapp-v2026.4.1-macOS_Apple_Silicon.zip` |
| macOS Intel | `ileapp-v2026.4.2-macOS_Mac_Intel.zip` | `aleapp-v2026.4.1-macOS_Mac_Intel.zip` |
| Windows x64 | `ileapp-v2026.4.2-Windows_x86_64.zip` | `aleapp-v2026.4.1-Windows_x86_64.zip` |
| Windows on Arm | `ileapp-v2026.4.2-Windows_arm64.zip` | `aleapp-v2026.4.1-Windows_arm64.zip` |
| Linux x64 | `ileapp-v2026.4.2-Linux_x86_64.AppImage` | `aleapp-v2026.4.1-Linux_x86_64.AppImage` |
| Linux arm64 | `ileapp-v2026.4.2-Linux_arm64.AppImage` | `aleapp-v2026.4.1-Linux_arm64.AppImage` |

Their SHA-256 values are in `docs/LEAPP-CLI.md` §1 and in the app's `leapp-manifest.json`.

**Networks that intercept TLS** (a proxy that re-signs HTTPS traffic) make the download fail its certificate check. suiteDFIR does not turn the check off; use the offline import instead.

**Parser states** in the card: *Not installed*; *Installed* (not checked yet in this session); *Verified* (the program matched its pinned SHA-256); *Verification failed* (the installed program changed, so runs are blocked: install or import it again); *Not available* (no build of this parser is pinned for this system). **Verify** re-checks the program now. suiteDFIR also checks it before every run.

## 4. Cases

A case is a plain folder with a `case.json` and one subfolder per run and acquisition. suiteDFIR never deletes anything in a case folder.

- **New case:** a name (required), case number, examiner, agency, description and default timezone. The folder name is derived from the name. The case number, agency and examiner appear in the LEAPP reports.
- **Open case folder…:** adds an existing case folder (for example one copied from another machine) to the recent list.
- **Forget:** removes a case from the recent list only; the folder is not changed.

When a case is opened, runs and acquisitions that a crash or power loss left unfinished are marked **Interrupted**.

## 5. Parse evidence

On the case screen, choose **New run**.

1. **Tool:** iLEAPP (iOS) or aLEAPP (Android). Only installed parsers are offered.
2. **Input:** **Choose file…** or **Choose folder…**. suiteDFIR detects the type and lets you change it among the types that fit:

   | Type | Input | iLEAPP | aLEAPP |
   |---|---|---|---|
   | `fs` | a folder: a file-system extraction | ✓ | ✓ |
   | `itunes` | a folder with `Manifest.db` / `Manifest.plist`: an iTunes/Finder backup | ✓ | |
   | `zip`, `tar`, `gz` | an archive of an extraction | ✓ | ✓ |
   | `raw` | a disk image (`.e01`, `.dd`, `.img`, `.bin`, `.raw`, `.001`) | ✓ | ✓ |
   | `file` | any other single file | ✓ | |

   Inputs are only read, never changed. A run is refused if its output would land inside the input, or if the input is inside another run's output.

   **Find iOS backups** (iLEAPP) lists the Finder/iTunes backups in the default folders, with device, iOS version, date, size and encryption; **Use** picks one as the input. The folders are `~/Library/Application Support/MobileSync/Backup` on macOS (needs Full Disk Access, section 1) and `%APPDATA%\Apple Computer\MobileSync\Backup` (iTunes) and `%USERPROFILE%\Apple\MobileSync\Backup` (Apple Devices) on Windows; Linux has none.
3. **Options:**
   - **Timezone** (iLEAPP): the case's default, else the settings default, else UTC. It is always passed explicitly.
   - **Backup password** (iLEAPP, iTunes/Finder backups): required when the backup is encrypted, and also when its encryption cannot be read, because iLEAPP would otherwise wait for a password prompt. It is never stored. iLEAPP only accepts it on its command line, so other programs of the same user can see it in the process list while the run lasts; `run.json` records it as `<redacted>`.
   - **Keychain file (optional)** (iLEAPP).
   - **Hash the input file (SHA-256)** for file inputs (on by default). It runs alongside the parser.
   - **Label:** shown in the runs table and recorded.
4. **Modules:** all, a saved profile, or a custom selection (search, categories, select all/none). Profiles use LEAPP's own format (`.ilprofile`, `.alprofile`) and can be imported and exported. A profile with module names that the installed version does not have cannot be used until they are removed.
5. **Start run.** The run screen shows the phase, the elapsed time and the live log. The log's search box finds text in it (Enter jumps to the next match), **Only matching lines** filters it, and **Copy all** copies it. **Cancel run** asks for confirmation and stops the parser and every process it started.

**Results.** **Open report** opens LEAPP's HTML report in your web browser (never inside suiteDFIR). **Reveal folder**, **Open stdout**, **Open stderr** and **Open run.json** show the rest. "Parser output" shows the last lines the parser printed. Output of cancelled and failed runs is kept and marked; nothing is deleted.

### What a run's status means

suiteDFIR decides the status from what LEAPP actually produced (its `_lava_data.lava`, the report's `index.html` and the exit code), never from the exit code alone, because LEAPP exits 0 even on invalid input.

| Status | Meaning |
|---|---|
| **Succeeded** | LEAPP completed, at least one module ran, and no module reported an error. |
| **Completed with errors** | LEAPP completed, but some modules reported *Error* (listed as `modules_errored`). The report is usable; check those modules. |
| **Failed** | See the reasons below. A partial report may exist. |
| **Cancelled** | You cancelled it (`cancelled_by_user`). Partial output is kept. |
| **Interrupted** | suiteDFIR stopped (crash, power loss, forced quit) during the run; found and marked when the case was next opened (`app_interrupted`). |

| Reason | Usually means |
|---|---|
| `no_modules_ran` | No module found anything to parse: the wrong input type, not the kind of data the tool parses, or a wrong backup password. |
| `no_output_dir` | LEAPP exited before creating its output; see stdout. |
| `lava_data_missing`, `index_html_missing`, `processing_incomplete` | LEAPP stopped before finishing its report (a crash; see stderr). |
| `nonzero_exit`, `killed_by_signal` | LEAPP exited with an error code or was killed (exit code 2 = LEAPP rejected its arguments). |
| `spawn_failed` | The parser could not start (for example glibc too old on Linux, or blocked by AppLocker). |
| `prepare_failed` | The run folder could not be prepared (permissions, disk full). |

Warnings never change the status: `stderr_traceback` (a Python traceback in stderr), `input_hash_failed` / `input_hash_cancelled`, `seal_failed`, `symlinks_in_report` (links in the report are counted, not followed), `unencodable_filename` (Windows file names that are not valid Unicode), `modules_other_status`.

## 6. Acquire an iOS backup over USB

suiteDFIR takes a full iTunes-style backup of an iPhone or iPad over USB into the case (`acquisitions/<id>/backup/`), with its own audit record (`acquisition.json`) and hash manifest (`backup.sha256`). Wi-Fi devices are not supported.

### Prerequisites

- **macOS:** nothing to install.
- **Windows (x64):** install the **Apple Devices** app (Microsoft Store) or iTunes, so that the **Apple Mobile Device Service** runs. Windows on Arm is not supported for acquisition: there is no build of the iOS tools for it, and the Acquire screen says "iOS acquisition is not available on this platform."
  - The acquisition folder must be an ASCII-only path of at most 150 characters (the tools use the old ANSI file functions), so keep the cases folder short and plain, for example `C:\Cases`. Otherwise the app refuses to start (`path_not_supported_by_tool`).
  - Backup-encryption passwords must be printable ASCII on Windows (letters, digits, spaces and the usual punctuation, no accents or other scripts). The tools read the password in the Windows ANSI code page, so other characters would reach the device as different bytes and the backup could not be decrypted with the password you typed. The app refuses such a password (`invalid_input`) before it changes anything on the device.
- **Linux:** install the distribution's packages and start the service:

  ```bash
  sudo apt install usbmuxd libimobiledevice-utils     # Debian, Ubuntu
  sudo dnf install usbmuxd libimobiledevice-utils     # Fedora
  sudo pacman -S usbmuxd libimobiledevice             # Arch
  sudo systemctl start usbmuxd
  ```

  suiteDFIR uses these tools from `PATH` and records their hashes; it cannot verify them against pinned builds as it does on macOS and Windows x64. Older packages may not support the newest iOS versions.

**"The Apple device service is not available."** The tools report every failure to reach the USB device service (usbmuxd) with the same message ("Unable to retrieve device list"), so suiteDFIR cannot tell a missing service from one that is installed but not reachable. Check, in order:

- Windows: the Apple Devices app (or iTunes) is installed; the **Apple Mobile Device Service** is running in `services.msc` (restart it, or restart Windows); the device is connected directly, not through a hub.
- Linux: `systemctl status usbmuxd`; the device is connected (the service is usually started when a device is plugged in); try another cable or port.
- macOS: reconnect the device; if that does not help, restart the Mac.
- On every system: unlock the device, try another cable or port, and close other programs that talk to the device.

### Trust and pairing

Open **Acquire iOS backup** on the case screen. It lists the connected devices every 2 seconds, with name, model, iOS version, serial, pair state and backup-encryption state. Listing never pairs a device and never makes it show the Trust prompt; pairing happens only when you click **Pair**:

| Pair state | What to do |
|---|---|
| Not paired | Unlock the device and click **Pair**, then tap **Trust** on the device and enter its passcode. |
| Waiting for Trust | Unlock the device and tap **Trust**, then click **Retry pairing**. |
| Locked | Unlock the device with its passcode, then retry. |
| Trust denied | Unplug the device and plug it in again, unlock it, retry and tap **Trust**. |
| Pairing failed | Unplug the device and plug it in again, unlock it, then retry. |
| Unknown | Check the cable and unlock the device; if it is not paired yet, click **Pair**. |

Pairing creates a pairing record on the computer and on the device. `acquisition.json` records it (`pair_record_created`).

### Backup encryption

An encrypted backup contains more data than an unencrypted one (for example saved passwords, Wi-Fi networks, website history and Health data), and iLEAPP needs the backup password to parse it.

- **The device's backup encryption is off:** the options offer **Enable backup encryption (recommended)**. You choose a password (at least 4 characters, typed twice). This **changes a setting on the device**: suiteDFIR turns backup encryption on with your password, takes the backup, and, with **Turn encryption off again afterwards** (on by default), turns it off again. Each change is recorded in `acquisition.json` (`device_changes`, `encryption`). On iOS 13 and later the device asks for its passcode for each change; the app shows "Enter the passcode on the device" and waits without a time limit.
- **Backup encryption is already on:** the backup is encrypted with the owner's password, which suiteDFIR does not know and cannot change. The backup is taken as it is (`backup_encryption_preexisting`), but you need the owner's password to parse it. An unknown backup password can only be removed on the device with **Reset All Settings**, which suiteDFIR never does.
- **The state cannot be read:** encryption cannot be turned on from suiteDFIR for this device.

The password is never stored. It reaches the tools through an environment variable (never the command line) and is erased from memory after the last encryption step.

### Taking the backup

The options show the free space against the space the device uses (`ok`, `warn` below 1.1 ×, and a block below 0.5 ×), and a **Label**. **Start acquisition** starts it; the screen shows the phase, the overall progress, the log and any passcode prompt.

**Cancel** depends on the phase: while encryption is being turned on, the app waits for that step to finish; during the backup it stops the backup; in both cases it then turns encryption off again if **Turn encryption off again afterwards** is on. While encryption is being turned off, the cancel is ignored so that step completes. Quitting the app during an acquisition asks first, then shows "Finishing safely…" until the device work has stopped and the record is written, with a **Quit anyway** option (the acquisition is then marked **Interrupted**, with its encryption warnings, when the case is next opened).

| Status | Meaning |
|---|---|
| **Succeeded** | The tool reported success, and the backup has `Manifest.db` (or `Manifest.mbdb` on very old iOS), `Info.plist` and `Status.plist` with a finished snapshot. |
| **Failed** | See the reasons, for example `cancelled_on_device`, `device_disconnected`, `sync_lock_failed` (Finder, iTunes or Apple Devices was syncing the device: close them and retry), `success_message_missing`, `snapshot_not_finished`, `encryption_enable_failed`. |
| **Cancelled** | You cancelled it. |
| **Interrupted** | suiteDFIR stopped during the acquisition. |

Warnings include `encryption_restore_failed`, `encryption_left_enabled` and `encryption_state_unknown` (backup encryption may still be on: see below), `device_file_errors` (the device reported errors for some files), `disk_nearly_full` (under 1 GiB free after the backup) and `seal_failed` / `seal_cancelled`.

### Turn backup encryption off later

If encryption was left on (the device disconnected, the passcode was not entered, the command failed) or its state is unknown, the case's **Acquisitions** table offers **Turn backup encryption off**. Connect and pair the device, enter the password used for that acquisition, and confirm.

- Each attempt writes its own read-only record next to the acquisition: `encryption-restore.json`, then `encryption-restore-2.json`, `encryption-restore-3.json` and so on. Earlier records are never changed.
- A failed attempt can be retried. Once an attempt succeeds, the action is no longer offered. `acquisition.json` keeps its original warnings; the attempt records show what happened afterwards.

### Parse with iLEAPP

**Parse with iLEAPP** (for a succeeded acquisition) opens New run with the backup as the input and the type `itunes`. The password is filled in only if you turned encryption on for this acquisition and ticked **Parse with iLEAPP now** before starting it: the app then keeps that password in memory (never stored) until the run starts or its form closes, and drops it when you leave the acquisition's result without parsing. Otherwise, enter the backup password yourself. The run's `run.json` records the acquisition it came from.

## 7. Verify a report or a backup

Every run folder has `report.sha256`, a manifest of every file in `report/`, and every acquisition folder has `backup.sha256` for `backup/`. Both use the GNU `sha256sum` format, and `run.json` / `acquisition.json` record the SHA-256 of the manifest itself (`output.seal.manifest_sha256`).

Check a run from its folder:

```bash
cd "<case>/runs/<run id>"
sha256sum -c report.sha256              # every file: OK; a changed or missing file: FAILED
sha256sum report.sha256                 # compare with output.seal.manifest_sha256 in run.json
```

For an acquisition, run the same in `<case>/acquisitions/<acq id>` with `backup.sha256`.

- **macOS:** use `shasum -a 256 --strict -c report.sha256`. macOS's `shasum` and `sha256sum` cannot read the escaped lines that the format uses for file names containing a backslash or a line break, and without `--strict` they only warn about such lines and still report success. If `--strict` fails for that reason, check on Linux or with GNU coreutils (`gsha256sum` from Homebrew's `coreutils`).
- **Windows:** use `sha256sum` from Git for Windows (Git Bash) or WSL.

## 8. Files and folders

A case folder:

```
<Case Name>/
  case.json
  runs/<run id>/            run.json (read-only once final), case.lcasedata, profile.<ext>,
                            leapp.stdout.log, leapp.stderr.log, report/, report.sha256
  acquisitions/<acq id>/    acquisition.json (read-only once final), device-info.plist,
                            encryption-restore[-N].json, idevicebackup2.stdout.log,
                            idevicebackup2.stderr.log, backup/<udid>/, backup.sha256
```

`device-info.plist` holds the device's full identity (including IMEI and phone number); it stays in the case folder and never goes to the app log.

The app's own folders (**Settings → About** shows the exact paths): settings, the installed parsers (the tools folder), saved profiles, per-run temporary folders, and the app log (`suitedfir.log`, at most 5 MB, never contains passwords).

## 9. Privacy

- Everything runs on your computer. There is no telemetry, no account, no crash reporting and no update check.
- The only network access is downloading a parser's pinned release from GitHub when you click Install. iOS acquisition talks only to the connected device over USB.
- Backup passwords are never stored or logged. They are held in memory only while a run or an acquisition needs them, and the password fields are cleared after use. For parsing, iLEAPP receives the password on its command line (visible to other programs of the same user while it runs); the libimobiledevice tools receive it through the environment.
- LEAPP reports open in your web browser from the local case folder; suiteDFIR never shows them inside the app.

## 10. Troubleshooting

| Problem | What to do |
|---|---|
| macOS: the app "is damaged" or cannot be checked | Section 1, macOS (unsigned build). |
| macOS: a Finder backup cannot be read | Grant Full Disk Access (section 1). |
| Windows: SmartScreen blocks the installer | **More info → Run anyway** after checking the hash. |
| Windows: the window stays empty on a machine without internet | Install with the offline installer (WebView2). On Windows on Arm, which has no offline installer, install the Microsoft Edge WebView2 Runtime first (its arm64 standalone installer), or run setup with internet access. |
| Windows: parsers blocked by AppLocker/WDAC | Move the tools folder to an approved location (section 1). |
| Linux: blank or flickering window | `WEBKIT_DISABLE_DMABUF_RENDERER=1` (section 1). |
| Linux: parser install fails with a glibc message | The parsers need glibc 2.43 (section 1). |
| A parser download fails with a certificate error | Offline import (section 3). |
| "The Apple device service is not available" | Section 6, prerequisites. |
| Acquisition refused on Windows because of the path | Use a short ASCII cases folder such as `C:\Cases`. |
| Backup encryption was left on | **Turn backup encryption off** on the case screen (section 6). |
