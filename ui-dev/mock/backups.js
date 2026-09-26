// @ts-check
// Mock data for `ios_backups_find` (ROADMAP S1): the Finder backups the mock "finds" in the macOS
// default backup folder, and the scenario results. Not shipped.
//
// - Default: four backups, newest first (as the core sorts them): the contract fixture (encrypted),
//   an unencrypted iPad, one whose Manifest.plist does not tell its encryption, and one whose folder
//   cannot be read (every detail unknown; inspecting it gives permission_denied, as the core would).
// - `backups_empty`: none. `backups_denied`: permission_denied, as the core reports the protected
//   Finder backup folder without Full Disk Access (or, with `windows`, an unreadable folder).
import * as fx from "../fixtures/contracts/index.js";

/** @typedef {import("../../ui/types").AppError} AppError */
/** @typedef {import("../../ui/types").IosBackup} IosBackup */

export const BACKUP_DIR = "/Users/examiner/Library/Application Support/MobileSync/Backup";

/** The found backup whose folder cannot be read. */
export const UNREADABLE_BACKUP = `${BACKUP_DIR}/9c01de2b7a4f4e1d8b3a6c5e0f1d2a3b4c5d6e7f`;

/** @type {IosBackup[]} */
export const FOUND_BACKUPS = [
  fx.IosBackup,
  {
    path: `${BACKUP_DIR}/00008030-001229C01146402E`,
    device_name: "Evidence iPad",
    product_type: "iPad8,1",
    ios_version: "17.5.1",
    last_backup: "2025-03-01T08:00:00Z",
    encrypted: false,
    size_bytes: 21_474_836_480,
  },
  {
    path: `${BACKUP_DIR}/00008020-000A4D1E0C21002E`,
    device_name: "Old iPhone",
    product_type: "iPhone11,2",
    ios_version: "16.7.10",
    last_backup: "2024-11-15T17:42:08Z",
    encrypted: null,
    size_bytes: 12_884_901_888,
  },
  {
    path: UNREADABLE_BACKUP,
    device_name: null,
    product_type: null,
    ios_version: null,
    last_backup: null,
    encrypted: null,
    size_bytes: null,
  },
];

/**
 * The found backup at `path`, if any.
 * @param {string} path
 * @returns {IosBackup | null}
 */
export function foundBackup(path) {
  return FOUND_BACKUPS.find((b) => b.path === path) ?? null;
}

/**
 * The core's `permission_denied` for an unreadable backup folder.
 * @param {boolean} windows
 * @returns {AppError}
 */
export function backupsDenied(windows) {
  return windows
    ? {
        code: "permission_denied",
        message: "suiteDFIR may not read the backup folder. Check that your user account can read it, then search again.",
        detail: "C:\\Users\\examiner\\Apple\\MobileSync\\Backup: access denied: Access is denied. (os error 5)",
      }
    : {
        code: "permission_denied",
        message:
          "suiteDFIR may not read the Finder backup folder. Give suiteDFIR Full Disk Access (System Settings > Privacy & Security > Full Disk Access), then quit and reopen suiteDFIR and search again.",
        detail: `${BACKUP_DIR}: access denied: Operation not permitted (os error 1)`,
      };
}
