// @ts-check
/**
 * "Find iOS backups" logic (ROADMAP S1; tests/ui/backups.test.js): what a search result means for
 * the finder panel, how a found backup is described, where the default backup folders are, and the
 * input type of a chosen backup. Pure functions: the core finds the backups (`ios_backups_find`).
 */
import { isAppError } from "./errors.js";
import { formatLocalTime, plural } from "./format.js";
import { initialInputType } from "./newrun.js";
import { TOOL_FEATURES } from "./tools.js";

/** @typedef {import("../types").InputInspection} InputInspection */
/** @typedef {import("../types").InputType} InputType */
/** @typedef {import("../types").IosBackup} IosBackup */
/** @typedef {import("../types").ToolId} ToolId */

/**
 * What a search came back with:
 * - `found`: at least one backup;
 * - `empty`: the default folders hold none (or the OS has none, as on Linux);
 * - `denied`: `permission_denied`, shown with how to allow access (Full Disk Access on macOS);
 * - `failed`: any other error.
 * @typedef {{ kind: "found", backups: IosBackup[] } | { kind: "empty" } | { kind: "denied", error: unknown } | { kind: "failed", error: unknown }} FinderResult
 */

/**
 * @param {IosBackup[]} backups what `ios_backups_find` returned
 * @returns {FinderResult}
 */
export function foundResult(backups) {
  return backups.length > 0 ? { kind: "found", backups } : { kind: "empty" };
}

/**
 * @param {unknown} error what `ios_backups_find` threw
 * @returns {FinderResult}
 */
export function failedResult(error) {
  return isAppError(error) && error.code === "permission_denied" ? { kind: "denied", error } : { kind: "failed", error };
}

/**
 * True when the New run form offers the finder: the tool reads iTunes/Finder backups (iLEAPP) and
 * the OS (`app_info.os`) has default backup folders (not Linux).
 * @param {ToolId | null} tool
 * @param {string | null | undefined} os
 * @returns {boolean}
 */
export function canFindBackups(tool, os) {
  return tool !== null && TOOL_FEATURES[tool].itunes && defaultBackupFolders(os).length > 0;
}

/**
 * The live summary after a search.
 * @param {FinderResult} result
 * @returns {string}
 */
export function summaryText(result) {
  switch (result.kind) {
    case "found":
      return `${plural(result.backups.length, "backup", "backups")} found. Choose one to use it as the input.`;
    case "empty":
      return "No backups found.";
    case "denied":
      return "The backup folder could not be read.";
    default:
      return "The search failed.";
  }
}

/**
 * The device's name, or a placeholder when the backup's `Info.plist` and `Manifest.plist` do not
 * tell it.
 * @param {IosBackup} b
 * @returns {string}
 */
export function deviceName(b) {
  return b.device_name ?? "Unknown device";
}

/**
 * The backup folder's own name (the device's UDID for Finder and iTunes backups).
 * @param {string} path
 * @returns {string}
 */
export function folderName(path) {
  return path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? path;
}

/**
 * @param {IosBackup} b
 * @returns {string}
 */
export function iosVersionText(b) {
  return b.ios_version ?? "—";
}

/**
 * @param {IosBackup} b
 * @returns {"Encrypted" | "Not encrypted" | "Unknown"}
 */
export function encryptionLabel(b) {
  if (b.encrypted === true) return "Encrypted";
  if (b.encrypted === false) return "Not encrypted";
  return "Unknown";
}

/**
 * The accessible name of a row's "Use" button: it starts with the visible text and names the
 * backup, since several backups may come from devices with the same name.
 * @param {IosBackup} b
 * @param {string} [timeZone] for tests; default the local zone
 * @returns {string}
 */
export function useLabel(b, timeZone) {
  const when = b.last_backup ? `, last backup ${formatLocalTime(b.last_backup, timeZone)}` : "";
  return b.device_name ? `Use the backup of ${b.device_name}${when}` : `Use the backup in folder ${folderName(b.path)}${when}`;
}

/**
 * The default backup folders `ios_backups_find` searches on `os` (`app_info.os`), as the examiner
 * knows them (ARCHITECTURE.md §10). Linux has none.
 * @param {string | null | undefined} os
 * @returns {string[]}
 */
export function defaultBackupFolders(os) {
  if (os === "macos") return ["~/Library/Application Support/MobileSync/Backup"];
  if (os === "windows") return ["%APPDATA%\\Apple Computer\\MobileSync\\Backup", "%USERPROFILE%\\Apple\\MobileSync\\Backup"];
  return [];
}

/**
 * How to let suiteDFIR read the backup folder after `permission_denied`: Full Disk Access on macOS
 * (the Finder backup folder is privacy-protected), the folder's permissions elsewhere.
 * @param {string | null | undefined} os
 * @returns {{ title: string, intro: string, steps: string[] }}
 */
export function accessGuidance(os) {
  if (os === "windows" || os === "linux") {
    return {
      title: "Allow access to the backup folder",
      intro: "Your user account may not read a backup folder (the details above name it).",
      steps: ["Check the folder's permissions for your account, or ask an administrator.", "Search again."],
    };
  }
  return {
    title: "Give suiteDFIR Full Disk Access",
    intro: "To list the Finder backups:",
    steps: [
      "Open System Settings > Privacy & Security > Full Disk Access.",
      "Turn on suiteDFIR. If it is not in the list, add it with the + button.",
      "Quit and reopen suiteDFIR, then search again.",
    ],
  };
}

/**
 * The input type for a backup chosen from the list, after its inspection: `itunes` only when the
 * inspection found an iTunes backup and allows the type; otherwise the usual preselection, so the
 * inspection result always decides.
 * @param {InputInspection} inspection
 * @returns {InputType | null}
 */
export function chosenBackupType(inspection) {
  return inspection.is_itunes_backup && inspection.allowed_types.includes("itunes") ? "itunes" : initialInputType(inspection);
}
