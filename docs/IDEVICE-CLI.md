# libimobiledevice CLI reference (iOS USB acquisition)

Facts about the libimobiledevice command-line tools that suiteDFIR uses for iOS backup acquisition (feature F11). Everything below was read from source at tag **libimobiledevice 1.4.0** (released 2025-10-10), including `tools/*.c`, `src/lockdown.c` and `common/userpref.c`, on 2026-09-24. Line numbers refer to `tools/idevicebackup2.c` unless another file is named.

**Nothing has been verified against a real device yet.** Items in §8 must be confirmed by human QA (H4/G2). Before relying on a message string, the X-track tasks must copy it verbatim from the pinned source.

## 1. Pinned sources and licenses

The tools are built from upstream source tarballs; ROADMAP X1 pins their SHA-256 in `idevice-tools.json`. On 2026-09-25 every hash was checked against the GitHub release asset digest, except libplist's: its asset has no digest, and the computed hash matches the Homebrew formula's.

| Project | Version | Asset |
|---|---|---|
| libplist | 2.7.0 | `libplist-2.7.0.tar.bz2` |
| libimobiledevice-glue | 1.3.2 | `libimobiledevice-glue-1.3.2.tar.bz2` |
| libusbmuxd | 2.1.1 | `libusbmuxd-2.1.1.tar.bz2` |
| libtatsu | 1.0.5 | `libtatsu-1.0.5.tar.bz2` (requires libcurl, `configure.ac:39`) |
| mbedtls | 3.6.7 | `mbedtls-3.6.7.tar.bz2` (the newest 3.6 LTS release; TLS backend via `--with-mbedtls`) |
| libimobiledevice | 1.4.0 | `libimobiledevice-1.4.0.tar.bz2` (`--with-mbedtls` and `--without-cython` are valid, `configure.ac:130/176`) |

**Build** (`scripts/build-idevice-tools.sh <platform-key>`):
- **Platforms:** `macos-aarch64` and `macos-x86_64` on the Mac (minimum macOS 11.0; x86_64 is cross-built with `-arch x86_64`), and `windows-x86_64` in a per-user MSYS2 UCRT64 install.
- **Static:** every library is built with `--enable-static --disable-shared` and `pkg-config --static`, so the tools link only system libraries; the script fails otherwise.
  - macOS: `libSystem`, `CoreFoundation` and `SystemConfiguration`.
  - Windows: system DLLs only, so `files` lists no DLL.
- **Only the four tools are built.** `ideviceimagemounter`, the only tool that links libtatsu, is not. libtatsu is still built because libimobiledevice's `configure` requires it; on macOS it compiles against the system libcurl.
- **Upstream build files are used unchanged** (no `autoreconf`, no source patches). Two static-build quirks are handled on the command line:
  - `3rd_party/libsrp6a-sha512` reads `$(mbedtls_CFLAGS)`, which `configure` never sets, so the script passes it to `make`.
  - libimobiledevice-glue initializes itself in a constructor in `glue.o` (on Windows it calls `WSAStartup`). The tools reference nothing else in that object, so a static link drops it, and on Windows every usbmuxd connection then fails. The tools are linked with `-u libimobiledevice_glue_version` (`_libimobiledevice_glue_version` on macOS) to pull it in, and the script checks that the constructor is present.
- **Publishing:** the bundles are assets of the prerelease `idevice-tools-1.4.0`. Each zip holds the tools, `BUILDINFO.json` (sources, toolchain, flags, date, linked libraries) and the notices listed below. `cargo xtask fetch-idevice-tools` checks `bundle_sha256` and every file hash before it installs the tools as Tauri sidecars.

**Licenses:**
- The source headers and README say LGPL-2.1-or-later. The repo also ships a GPL-2 `COPYING`.
- **Ship both texts** (`COPYING`, `COPYING.LESSER`), plus the `3rd_party/` notices and the mbedtls (Apache-2.0) notice.
- The release tarball omits the `3rd_party/*/LICENSE` files (ed25519: zlib; libsrp6a-sha512: Stanford SRP, BSD-style). The build takes them from the `1.4.0` tag, pinned by SHA-256; they match that tag's blobs.
- libplist compiles in MIT-licensed code (`src/jsmn.c`, `src/time64.c`). The bundles carry those notices in `libplist-embedded-notices.txt`.
- The Windows tools link the MinGW-w64 runtime statically, and its license requires its notices in binary distributions: `mingw-w64/COPYING.MinGW-w64-runtime.txt` in the Windows bundle.
- The source obligation is met by attaching the exact source tarballs and the build script to each app release (ROADMAP F1/F2). A link to upstream is not enough.

**Runtime service (usbmuxd):**
- **macOS:** built into the OS.
- **Windows:** Apple Mobile Device Service (Apple Devices app or iTunes).
- **Linux:** the distro's `usbmuxd` daemon. On Linux suiteDFIR uses the distro's tools rather than bundling its own.

## 2. Tools and invocations

| Purpose | Invocation | Notes |
|---|---|---|
| List USB devices | `idevice_id -l` | One UDID per line; exit 0 even when empty. `-n` (network) is never used. |
| Pre-pairing identity | `ideviceinfo -u <udid> -s -x` | "Simple" connection, no session. Returns only the pre-session subset of values, so name and serial **may be null**. |
| Host pair record present? | `idevicepair -u <udid> hostid` | Reads the host's record via usbmuxd **without opening a device session** (`idevicepair.c:390-404`). A non-UUID or `(null)` result means no record → `not_paired`. |
| System BUID | `idevicepair systembuid` | No side effects; recorded in `pairing`. |
| Validate pairing | `idevicepair -u <udid> validate` | **Only call when a host record exists.** It uses `lockdownd_client_new_with_handshake` (`idevicepair.c:452`), which **pairs** if no record exists (`lockdown.c:728-733`) and triggers the Trust dialog. |
| Pair | `idevicepair -u <udid> pair` | Only via the explicit `device_pair` command. |
| Full identity (paired) | `ideviceinfo -u <udid> -x` | XML plist (`plist::Value::from_reader`). Saved as `device-info.plist`; contains IMEI and phone number, so never log it. |
| Encryption state | `ideviceinfo -u <udid> -q com.apple.mobile.backup -x` | The backup domain dictionary. `WillEncrypt` is a boolean; the backup tool treats an **absent** key as false (1843-1851), and so does suiteDFIR. `-k WillEncrypt` is not used: for an absent key it prints nothing with exit 0, exactly like a failed read (`lockdown.c:403-452`, `ideviceinfo.c:235-259`). Empty output or a tool error for the domain means the state is unknown. |
| Disk usage | `ideviceinfo -u <udid> -q com.apple.disk_usage -x` | `TotalDataCapacity`, `TotalDataAvailable`; used capacity = difference. |
| Backup | `idevicebackup2 -u <udid> backup --full <dir>` | `<dir>` **must exist**: otherwise `ERROR: Backup directory "<dir>" does not exist!` and exit 255 (1730-1733). Creates `<dir>/<udid>/`. |
| Encryption on | `idevicebackup2 -u <udid> encryption on` + env `BACKUP_PASSWORD_NEW=<pw>` | Env variables are read at 1458-1459 and 1758-1768. Never pass the password in argv. |
| Encryption off | `idevicebackup2 -u <udid> encryption off` + env `BACKUP_PASSWORD=<pw>` | |

**`ideviceinfo` failure mode:** when the value read fails, it can exit 0 with **empty stdout** (`ideviceinfo.c:235-259`). Treat empty output as a failure.

**`ideviceinfo` without `-s` pairs:** it connects with `lockdownd_client_new_with_handshake` (`ideviceinfo.c:222-224`), which pairs when no host record exists, like `validate`. So the full identity, `WillEncrypt` and disk usage are read only from a device whose pairing was confirmed.

**`hostid` needs the device connected:** `idevicepair` looks the device up through usbmuxd first and prints `No device found with udid <udid>.` if it is gone (`idevicepair.c:371-379`).

**Prompts:** never pass `-i/--interactive`. Without it, a missing password produces `ERROR: Can't get password input in non-interactive mode…` instead of a terminal prompt.

## 3. Exit codes and signals

- **`idevice_id`:** 0 on success; non-zero on errors (e.g. usbmuxd unavailable).
- **`idevicepair`:** prints to **stdout** and exits 1 on errors. `No device found with udid …` → `device_not_found`.
- **`ideviceinfo`:** non-zero on connection/lockdown errors; see the empty-output caveat in §2.
- **`idevicebackup2`:** `main` returns `result_code`, which is negative on error.
  - On Unix the shell sees `(-N) & 0xFF` (105 → 151, -1 → 255, multiples of 256 → **0**).
  - On Windows the exit code is the negative value.
  - **Never treat the numeric exit code as success on its own**; use the messages in §5.
- **SIGINT/SIGTERM/SIGQUIT** set a quit flag (`clean_exit`, prints `Exiting...` to stderr). The flag is checked per data block and per message, and receives time out, so a running backup aborts reasonably promptly. SIGPIPE is ignored.

## 4. Pairing messages (`idevicepair`, `idevicepair.c:114-143` and following)

| Message | → `PairState` / error |
|---|---|
| `SUCCESS: Paired with device <udid>` / `SUCCESS: Validated pairing with device <udid>` | `paired` |
| `ERROR: Please accept the trust dialog on the screen of device <udid>, then attempt to pair again.` | `awaiting_trust` |
| `ERROR: Could not validate with device <udid> because a passcode is set. Please enter the passcode on the device and retry.` | `locked` |
| `ERROR: Device <udid> is not paired with this host` | `not_paired` (a host record exists but the device rejects it) |
| `ERROR: Device <udid> said that the user denied the trust dialog.` | `trust_denied` |
| `ERROR: Pairing with device <udid> failed.` / other `ERROR:` | `pairing_failed` (show the message) |
| `No device found with udid …` | error `device_not_found` |

## 5. Backup behavior (`idevicebackup2 backup --full`)

**Progress:**
- `\r[====   ]  45% (1.2 MB/2.7 MB)` lines are **per upload batch** (`DLMessageUploadFiles`, 1017-1018, 1115-1118); they cycle 0 → 100% repeatedly. **Ignore their sizes.**
- **Overall progress** is printed as `print_progress_real(overall, 0)` followed by ` Finished` (2524-2525), and is not always flushed. Parse overall percent only from `\]\s+(\d+)%\s+Finished`.
- Output may arrive in bursts, so handle records split by `\r` or `\n` across chunk boundaries.

**Device prompts** (exact strings, copied from the pinned source; `idevice::parse` matches them):
- Before a backup on iOS ≥ 16.1, when the device asks for its passcode (2055-2062), on stdout → `passcode_for_backup`:
  `*** Waiting for passcode to be entered on the device ***`
- For encryption changes on iOS ≥ 13 when a passcode is set (2252-2259), on stdout → `passcode_for_encryption`; the tool then waits **without a time limit** (2236-2266):
  - `encryption on`: `Please confirm enabling the backup encryption by entering the passcode on the device.`
  - `encryption off`: `Please confirm disabling the backup encryption by entering the passcode on the device.`
  - (`changepw`, never run: `Please confirm changing the backup password by entering the passcode on the device.`)
- **Buffering:** these lines are plain `printf` output. With stdout on a pipe, the C runtime buffers it fully, and only `print_progress` calls `fflush` (704). So an encryption prompt may reach suiteDFIR only when the command ends, and the backup prompt with the first file batch. The UI therefore also explains, for the whole encryption phase, that the app waits for the device (ROADMAP D5). To be confirmed on a device (§8 item 2).
- **Results of an encryption change** (2588-2599): `Backup encryption has been enabled successfully.` / `Could not enable backup encryption.`, and `Backup encryption has been disabled successfully.` / `Could not disable backup encryption.` suiteDFIR decides the outcome by re-reading `WillEncrypt`, not from these lines.

**Final messages** (2569-2575):
- `Backup Successful.` only if the device reported ErrorCode 0 **and** `SnapshotState == "finished"`.
- Otherwise: `Backup Failed (Error Code N).` (N may be 0 when the snapshot isn't finished; the exit code can then be 0) or `Backup Aborted.`

**Abort causes.** `Backup Aborted.` follows any of:
- an examiner SIGTERM;
- a cancel on the device (preceded by `User has cancelled the backup process on the device.`, line 115);
- a device disconnect (the quit flag is set at ≈ 2297).

**Other messages:**
- **Sync lock:** the tool takes `/com.apple.itunes.lock_sync` on the device (1949-1977). If Finder or iTunes holds it, the lock attempts time out; any other AFC error ends them at once. Either way the tool prints, on **stderr**, one of these (exact strings, → `sync_lock_failed`), skips the backup without a final message, and exits with `result_code` -1 (255 on Unix):
  - `ERROR: timeout while locking for sync` (1973)
  - `ERROR: could not lock file! error code: <n>` (1967)
- **On-device cancel:** `User has cancelled the backup process on the device.` (115), followed later by `Backup Aborted.`.
- **File errors:** `Received an error message from device: <msg>` (1153, preceded by an empty line) → counted as `device_file_errors`.
- **Unchecked writes:** local write results are not checked (`fwrite`, ≈ 1111), so check free space after the backup.

**Output layout:**
- `<dir>/<udid>/` with `Info.plist`, `Manifest.plist`, `Manifest.db` (or `Manifest.mbdb` on very old iOS), `Status.plist` (`SnapshotState`, read at 613-630), and hashed-name subfolders.
- A fresh folder per acquisition means every backup is full (1938/1945).

**Encryption:**
- If `WillEncrypt` is true, the device encrypts with the owner's backup password. No password is needed to *take* the backup, but it is needed to *parse* it (iLEAPP `--itunes_password`).
- Changing the setting changes device state. suiteDFIR records it, restores it by default, and never resets device settings; an unknown existing password can only be removed by "Reset All Settings" on the device.

**Windows paths:** the tools use ANSI file APIs (`fopen`, `mkdir`, `DeleteFile`, `GetDiskFreeSpaceEx`; ≈ 175, 221, 2315). Non-ASCII or long target paths may fail, so suiteDFIR refuses them (ARCHITECTURE §6b step 4).

## 6. How suiteDFIR decides acquisition success

**Succeeded** requires all of these:
- `Backup Successful.` seen on stdout;
- exit code 0;
- `<dir>/<udid>/Manifest.db` (or `Manifest.mbdb`), `Info.plist` and `Status.plist` all exist;
- `SnapshotState == "finished"`.

**Otherwise:** `cancelled` (examiner) or `failed` with reasons (CONTRACTS.md §13.3).

## 7. Known limitations

- USB only; Wi-Fi devices are not supported in phase 1.
- Windows arm64: no pinned build, so no acquisition.
- One device at a time (one-active-job rule).
- Host pair records (`/var/db/lockdown` on macOS, `%ProgramData%\Apple\Lockdown` on Windows, `/var/lib/lockdown` on Linux) are created by pairing and are not managed by the app.

## 8. UNVERIFIED items (X3a/X3b tests + human QA must resolve)

1. `SnapshotState == "finished"` and `Manifest.db` presence on iOS 17/18/26.
2. Exact on-device prompts (passcode for backup / encryption) per iOS version, and their console lines.
3. Real-device progress output cadence (bursts, `Finished` lines).
4. Windows: the tools reach Apple Mobile Device Service without extra configuration; the exact error when it is missing.
   - **Recorded by X1** on 2026-09-25 (Windows 11 build 26200, the published `windows-x86_64` bundle):
     - With nothing listening on the usbmuxd port, `idevice_id -l` prints `ERROR: Unable to retrieve device list!` to stderr, nothing to stdout, and exits with code -1.
     - The test machine has the service installed, and a non-admin account cannot stop it. The missing service was therefore simulated with `USBMUXD_SOCKET_ADDRESS=127.0.0.1:27016` (a closed port), which takes the same connect path in libusbmuxd as the default `127.0.0.1:27015`.
     - With the service running and no device attached, `idevice_id -l` printed nothing and exited 0, with no extra configuration.
   - **Still open:** talking to a real device through the service.
5. Linux: minimum distro package version that supports current iOS.
6. Graceful abort time after SIGTERM on a large backup (grace = 30 s).
7. `ideviceinfo -s` field subset on current iOS for an unpaired device.
8. The sync-lock failure string, and the behavior when Finder/iTunes is open.
9. The `com.apple.mobile.backup` domain on a device that never had backup encryption set: a dictionary without `WillEncrypt` (read as false), or nothing at all (read as unknown, which blocks enabling encryption).
10. Whether `WillEncrypt` reads `true` right after `encryption on` reports success, or only after a delay. Upstream once waited for a backup-domain-changed notification after enabling (the code commented out at 2260-2266). suiteDFIR treats "reported success, but `WillEncrypt` reads false" as an unknown outcome: it warns `encryption_state_unknown` and attempts the restore.
