# libimobiledevice CLI reference (iOS USB acquisition)

Facts about the libimobiledevice command-line tools that suiteDFIR uses for iOS backup acquisition (feature F11). Everything below was read from source at tag **libimobiledevice 1.4.0** (released 2025-10-10) on 2026-09-24. **Nothing has been verified against a real device yet.** Items marked UNVERIFIED must be confirmed in human QA (H4/G2) and by the X-track tests. §8 is the running list.

## 1. Pinned sources

The tools are built from these upstream source tarballs (GitHub `libimobiledevice/*` releases). ROADMAP X1 pins their SHA-256 hashes in `idevice-tools.json`.

| Project | Version | Asset |
|---|---|---|
| libplist | 2.7.0 | `libplist-2.7.0.tar.bz2` |
| libimobiledevice-glue | 1.3.2 | `libimobiledevice-glue-1.3.2.tar.bz2` |
| libusbmuxd | 2.1.1 | `libusbmuxd-2.1.1.tar.bz2` |
| libtatsu | 1.0.5 | `libtatsu-1.0.5.tar.bz2` (requires libcurl) |
| libimobiledevice | 1.4.0 | `libimobiledevice-1.4.0.tar.bz2` (TLS: OpenSSL by default; `--with-mbedtls` supported) |

**Licenses:**
- The libraries are LGPL-2.1-or-later (`COPYING.LESSER`).
- The **tools are GPL-2.0-or-later** (`COPYING`).
- Distributing the tool binaries requires shipping the license texts and offering the corresponding source. X1 records exact tarball URLs and hashes, and the notices point to them.

**Runtime connection service (usbmuxd):**
- **macOS:** built into the OS.
- **Windows:** Apple's "Apple Mobile Device Service", installed with the Apple Devices app or iTunes.
- **Linux:** the `usbmuxd` daemon from the distro (needs udev rules). On Linux suiteDFIR uses the system-installed tools (`libimobiledevice-utils` or the equivalent package) instead of bundling them.

## 2. Tools and flags used

| Tool | Invocation | Output |
|---|---|---|
| `idevice_id` | `idevice_id -l` | One UDID per line (USB devices only; `-n` would list network devices, never used). Exit 0 even when the list is empty. |
| `ideviceinfo` | `ideviceinfo -u <udid> -x` | XML plist of all lockdown values (parse with `plist::Value::from_reader`). Useful keys: `DeviceName`, `ProductType`, `ProductVersion`, `BuildVersion`, `SerialNumber`, `UniqueDeviceID`. |
| | `ideviceinfo -u <udid> -q com.apple.mobile.backup -k WillEncrypt -x` | Boolean: whether backups will be encrypted. |
| | `ideviceinfo -u <udid> -q com.apple.disk_usage -x` | `TotalDataCapacity`, `TotalDataAvailable`, … (used to estimate backup size). |
| | `ideviceinfo -u <udid> -s -x` | "Simple" connection that avoids auto-pairing. Use it to read basic identity before pairing. |
| `idevicepair` | `idevicepair -u <udid> validate` / `pair` | Prints `SUCCESS: …` or `ERROR: …` lines (§4). |
| `idevicebackup2` | `idevicebackup2 -u <udid> backup --full <dir>` | Creates `<dir>/<udid>/` (§5). |
| | `idevicebackup2 -u <udid> encryption on <pw>` / `encryption off <pw>` | Prints `Backup encryption has been enabled successfully.` / `…disabled successfully.` |

Never pass `-i/--interactive`. Without it, missing passwords produce `ERROR: Can't get password input in non-interactive mode…` instead of a prompt.

## 3. Exit codes and signals

- `idevice_id`: 0 = success; negative return values on errors (e.g. usbmuxd unavailable).
- `ideviceinfo`: 0 = success; non-zero on connection/lockdown errors.
- `idevicepair`: 0 = success; non-zero with an `ERROR:` line.
- `idevicebackup2`:
  - The exit status is `result_code`: 0 on success, otherwise the device error code (negated in source, so the shell sees a non-zero byte).
  - **SIGINT/SIGTERM/SIGQUIT set a quit flag, and the tool aborts gracefully** (`clean_exit`, prints `Exiting...` to stderr).
  - SIGPIPE is ignored.

## 4. Pairing messages (`idevicepair`)

| Message | Meaning → suiteDFIR state |
|---|---|
| `SUCCESS: Paired with device <udid>` / `SUCCESS: Validated pairing with device <udid>` | Paired. |
| `ERROR: Please accept the trust dialog on the screen of device <udid>, then attempt to pair again.` | `awaiting_trust`: show instructions and retry. |
| `ERROR: Could not validate with device <udid> because a passcode is set. Please enter the passcode on the device and retry.` | `locked`: ask the user to unlock the device. |
| `ERROR: Device <udid> is not paired with this host` | `not_paired`: offer Pair. |
| `ERROR: Device <udid> said that the user denied the trust dialog.` | `trust_denied`. |
| `ERROR: Pairing with device <udid> failed.` / other `ERROR:` | `pairing_failed`, with the message shown. |

## 5. Backup behavior (`idevicebackup2 backup --full`)

- **Progress** is written to stdout as `\r[====      ]  45% (1.2 GB/2.7 GB)` via `print_progress_real`, **followed by `fflush(stdout)`**, so it streams through a pipe. Other verbose lines (`Receiving files`, `Sending '<path>' (<size>)`, …) are plain `printf` and may be buffered until the next flush. Parse with the regex `\]\s+(\d+)%` on records split by `\r` or `\n`.
- **Final message:** `Backup Successful.` / `Backup Aborted.` / `Backup Failed (Error Code N).`
- **Output layout:** `<dir>/<udid>/` containing `Info.plist`, `Manifest.plist`, `Manifest.db`, `Status.plist`, plus hashed-name subfolders. `Status.plist` has `SnapshotState` (source reads it in `idevicebackup2.c:613-630`). A finished backup is expected to have `SnapshotState == "finished"` (UNVERIFIED on current iOS).
- **Encryption:** if `WillEncrypt` is true, the device encrypts with the owner's backup password. No password is needed to *take* the backup, but it is needed to *parse* it (iLEAPP `--itunes_password`).
- **Enabling encryption changes a device setting.** Record it, and offer to turn it off again afterwards (needs the same password). If the device already has an unknown backup password, it can only be removed by "Reset All Settings" on the device. suiteDFIR never does that; it only warns.
- **User prompts on the device:** newer iOS versions ask for the device passcode on screen before a backup starts or encryption changes (UNVERIFIED which versions). The UI must tell the examiner to watch the device.

## 6. How suiteDFIR decides acquisition success

**Succeeded** requires all of these:
- exit code 0;
- `Backup Successful.` seen on stdout;
- `<dir>/<udid>/Manifest.db` (or `Manifest.mbdb` for very old iOS), `Info.plist` and `Status.plist` all exist;
- `SnapshotState == "finished"`.

**Otherwise:**
- `cancelled` if the examiner cancelled;
- `failed` with reasons (CONTRACTS.md §13.3).

## 7. Known limitations

- Wi-Fi (network) devices are not supported in phase 1: USB only.
- Windows arm64: not supported for acquisition in phase 1 (no pinned build).
- Only one device is backed up at a time. The one-active-job rule covers this.

## 8. UNVERIFIED items (X4 tests + human QA must resolve)

1. `SnapshotState == "finished"` on current iOS; `Manifest.db` presence on iOS 17/18/26.
2. The exact on-device prompts (passcode for backup / encryption change) per iOS version, and their effect on timing.
3. Progress line format on a real device (sizes appear only when known).
4. Windows: the tools connect to the Apple Mobile Device Service without extra configuration; the error text when the service is missing.
5. Linux: minimum distro package version that supports current iOS.
6. Graceful abort time after SIGTERM on a large backup (the grace period is set to 30 s).
