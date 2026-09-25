// @ts-check
/**
 * Acquire screen logic (ROADMAP D5; tests/ui/acquire.test.js): what each pair state and tools state
 * tells the examiner and allows, the device selection, the preflight levels, and the options with
 * the reasons Start is disabled. The core validates everything again (ARCHITECTURE.md §6b step 4).
 */
import { formatBytes } from "./format.js";

/** @typedef {import("../types").AcqPreflight} AcqPreflight */
/** @typedef {import("../types").AcqRequest} AcqRequest */
/** @typedef {import("../types").DeviceSummary} DeviceSummary */
/** @typedef {import("../types").IdeviceToolsState} IdeviceToolsState */
/** @typedef {import("../types").PairState} PairState */

/**
 * @typedef {object} PairInfo
 * @property {string} label
 * @property {"ok" | "info" | "warn" | "danger" | "neutral"} tone
 * @property {"check-circle" | "info" | "lock" | "x-circle" | "alert-triangle" | "circle"} icon
 * @property {string} text What to do.
 * @property {"pair" | "retry" | null} action The only way the UI pairs: an explicit button.
 */

/** @type {Record<PairState, PairInfo>} */
export const PAIR_STATES = {
  paired: { label: "Paired", tone: "ok", icon: "check-circle", text: "This computer is trusted by the device.", action: null },
  not_paired: {
    label: "Not paired",
    tone: "neutral",
    icon: "circle",
    text: "This computer is not trusted by the device yet. Unlock the device and press Pair, then tap Trust on the device and enter its passcode.",
    action: "pair",
  },
  awaiting_trust: {
    label: "Waiting for Trust",
    tone: "info",
    icon: "info",
    text: "Unlock the device and tap Trust, then press Retry.",
    action: "retry",
  },
  locked: {
    label: "Locked",
    tone: "warn",
    icon: "lock",
    text: "The device is locked. Unlock it with its passcode, then press Retry.",
    action: "retry",
  },
  trust_denied: {
    label: "Trust denied",
    tone: "danger",
    icon: "x-circle",
    text: "Trust was denied on the device. Unplug it and plug it in again, unlock it, press Retry and tap Trust.",
    action: "retry",
  },
  pairing_failed: {
    label: "Pairing failed",
    tone: "danger",
    icon: "x-circle",
    text: "Pairing failed. Unplug the device and plug it in again, unlock it, then press Retry.",
    action: "retry",
  },
  unknown: {
    label: "Unknown",
    tone: "warn",
    icon: "alert-triangle",
    text: "The pairing state could not be read. Check the cable and unlock the device; the list refreshes every 2 seconds. If the device is not paired yet, press Pair.",
    action: "pair",
  },
};

/** @type {PairInfo} */
const BUSY = {
  label: "In use",
  tone: "info",
  icon: "info",
  text: "In use by the current acquisition.",
  action: null,
};

/**
 * @param {DeviceSummary} d
 * @returns {PairInfo}
 */
export function deviceState(d) {
  return d.busy ? BUSY : PAIR_STATES[d.pair_state] ?? PAIR_STATES.unknown;
}

/**
 * A device can be acquired when it is paired and not in use.
 * @param {DeviceSummary} d
 */
export function canAcquire(d) {
  return d.pair_state === "paired" && !d.busy;
}

/**
 * The device to keep selected after a poll: the current one while it can still be acquired, else
 * the only device that can be (so one connected, paired phone needs no extra click), else none.
 * @param {readonly DeviceSummary[]} devices
 * @param {string | null} current
 * @returns {string | null}
 */
export function pickDevice(devices, current) {
  const ready = devices.filter(canAcquire);
  if (current && ready.some((d) => d.udid === current)) return current;
  return ready.length === 1 ? ready[0].udid : null;
}

/** @typedef {Pick<DeviceSummary, "pair_state" | "message">} PairOutcome */

/**
 * The part of a `device_pair` answer to keep showing, or null when the device paired.
 * @param {DeviceSummary} answer
 * @returns {PairOutcome | null}
 */
export function pairOutcome(answer) {
  return answer.pair_state === "paired" ? null : { pair_state: answer.pair_state, message: answer.message };
}

/**
 * Keeps the last `device_pair` answer that did not pair on screen (ARCHITECTURE.md §6b steps 1-2).
 * After Pair answers `awaiting_trust`, `locked`, `trust_denied` or `pairing_failed`, the host has no
 * pair record yet, so the next `devices_list` reports `not_paired` and the answer's instructions
 * would vanish 2 s later. While a poll reports `not_paired`, the device shows the remembered answer.
 * An answer is forgotten when the device pairs or disappears (and by the screen when Pair is
 * pressed again).
 * @param {readonly DeviceSummary[]} devices As `devices_list` reported them.
 * @param {ReadonlyMap<string, PairOutcome>} outcomes Remembered answers by UDID.
 * @returns {{ devices: DeviceSummary[], outcomes: Map<string, PairOutcome> }} What to show, and
 *   the answers still remembered.
 */
export function withPairOutcomes(devices, outcomes) {
  /** @type {Map<string, PairOutcome>} */
  const kept = new Map();
  const shown = devices.map((d) => {
    const outcome = outcomes.get(d.udid);
    if (!outcome || d.pair_state === "paired") return d;
    kept.set(d.udid, outcome);
    return d.pair_state === "not_paired" && !d.busy ? { ...d, pair_state: outcome.pair_state, message: outcome.message } : d;
  });
  return { devices: shown, outcomes: kept };
}

/**
 * Replaces a device in the list by UDID (e.g. with `device_pair`'s answer), keeping the order.
 * @param {readonly DeviceSummary[]} devices
 * @param {DeviceSummary} updated
 * @returns {DeviceSummary[]}
 */
export function mergeDevice(devices, updated) {
  return devices.some((d) => d.udid === updated.udid) ? devices.map((d) => (d.udid === updated.udid ? updated : d)) : [...devices, updated];
}

/**
 * Guidance for a tools state other than `ok`, per platform (`AppInfo.os`).
 * @param {IdeviceToolsState} state
 * @param {string | null | undefined} os
 * @returns {{ title: string, text: string } | null}
 */
export function toolsGuidance(state, os) {
  switch (state) {
    case "ok":
      return null;
    case "missing":
      return {
        title: "The iOS tools (libimobiledevice) were not found.",
        text:
          os === "linux"
            ? "Install them with the package manager, e.g. sudo apt install usbmuxd libimobiledevice-utils (Debian, Ubuntu) or sudo dnf install usbmuxd libimobiledevice-utils (Fedora), then reopen this screen."
            : "They ship with suiteDFIR, so the installation is incomplete. Reinstall suiteDFIR.",
      };
    case "usbmuxd_unavailable":
      return {
        title: "The Apple device service is not available.",
        text:
          os === "windows"
            ? "Install the Apple Devices app (Microsoft Store) or iTunes, so the Apple Mobile Device Service runs, then reconnect the device."
            : os === "linux"
              ? "Start the usbmuxd service (sudo systemctl start usbmuxd), or reconnect the device so the system starts it."
              : "usbmuxd is part of macOS. Reconnect the device; if this persists, restart the Mac.",
      };
    case "verification_failed":
      return {
        title: "The iOS tools failed verification.",
        text: "A bundled tool does not match its pinned SHA-256. Do not acquire with it: reinstall suiteDFIR.",
      };
    case "unsupported_platform":
      return {
        title: "iOS acquisition is not available on this platform.",
        text: "suiteDFIR has no iOS tools for this platform (for example Windows on Arm). Use a Mac, a Windows x64 PC or Linux.",
      };
  }
  return null;
}

/**
 * The preflight banner (`acq_preflight`: `ok` if free ≥ 1.1 × required, `warn` if ≥ 0.5 ×, else
 * `block`; ARCHITECTURE.md §6b step 3).
 * @param {AcqPreflight} p
 * @returns {{ tone: "ok" | "warn" | "danger", title: string, text: string }}
 */
export function preflightInfo(p) {
  const free = formatBytes(p.free_bytes);
  const needed = p.required_bytes === null ? null : formatBytes(p.required_bytes);
  const facts = needed === null ? `${free} free in the case folder; the device's used space is unknown.` : `${free} free in the case folder; the device holds ${needed} of data.`;
  switch (p.level) {
    case "ok":
      return { tone: "ok", title: "Enough free space.", text: facts };
    case "warn":
      return { tone: "warn", title: "Free space is tight.", text: `${facts} A full backup needs about as much as the device holds, so it may not fit.` };
    default:
      return { tone: "danger", title: "Not enough free space.", text: `${facts} Free up space on the case folder's drive, or use a case on another drive.` };
  }
}

/** The core requires at least 4 characters (ARCHITECTURE.md §6b step 4). */
export const MIN_PASSWORD_LENGTH = 4;

/**
 * What the encryption options offer for a device: turning encryption on (`WillEncrypt` false), a
 * warning that it is already on (the owner's password is needed to parse), or nothing (unknown).
 * @param {DeviceSummary} d
 * @returns {"offer" | "already_on" | "unknown"}
 */
export function encryptionOption(d) {
  if (d.will_encrypt === false) return "offer";
  if (d.will_encrypt === true) return "already_on";
  return "unknown";
}

/**
 * @typedef {object} AcqForm
 * @property {boolean} toolsOk
 * @property {DeviceSummary | null} device The selected device.
 * @property {"loading" | "failed" | AcqPreflight | null} preflight
 * @property {string} label
 * @property {boolean} enableEncryption
 * @property {string} password
 * @property {string} password2
 * @property {boolean} restoreEncryption
 * @property {boolean} jobActive
 * @property {boolean} windows The app runs on Windows (`app_info.os`), where the iOS tools take
 *   only printable ASCII passwords.
 */

/**
 * True when the request will turn encryption on: ticked, and offered for this device.
 * @param {Pick<AcqForm, "device" | "enableEncryption">} f
 */
export function enablesEncryption(f) {
  return f.enableEncryption && f.device !== null && encryptionOption(f.device) === "offer";
}

/**
 * Why the iOS tools cannot take this backup password here, or null. On Windows they read it in
 * the ANSI code page, so the core refuses anything but printable ASCII (`invalid_input`); this is
 * the same rule, checked before anything is sent.
 * @param {string} password
 * @param {boolean} windows
 * @returns {string | null}
 */
export function toolPasswordProblem(password, windows) {
  if (!windows || /^[\x20-\x7e]*$/.test(password)) return null;
  return "On Windows the iOS tools can only use a password of plain ASCII letters, digits, spaces and punctuation.";
}

/**
 * The problem with the two password fields, or null.
 * @param {Pick<AcqForm, "password" | "password2"> & { windows?: boolean }} f
 * @returns {string | null}
 */
export function passwordProblem(f) {
  if (f.password.length < MIN_PASSWORD_LENGTH) return `Enter a backup password of at least ${MIN_PASSWORD_LENGTH} characters.`;
  const tool = toolPasswordProblem(f.password, f.windows === true);
  if (tool) return tool;
  if (f.password !== f.password2) return "Enter the same password in both fields.";
  return null;
}

/**
 * Every reason Start is disabled, in form order. Empty = ready.
 * @param {AcqForm} f
 * @returns {string[]}
 */
export function acqBlockers(f) {
  /** @type {string[]} */
  const out = [];
  if (!f.toolsOk) out.push("Fix the iOS tools problem shown above.");
  if (!f.device) out.push("Connect a device, pair it and select it.");
  else if (f.preflight === null || f.preflight === "loading") out.push("Wait for the free-space check.");
  else if (f.preflight === "failed") out.push("The free-space check failed (see above). Select the device again to retry.");
  else if (f.preflight.level === "block") out.push("Free up space: the backup does not fit in the case folder.");
  if (enablesEncryption(f)) {
    const problem = passwordProblem(f);
    if (problem) out.push(problem);
  }
  if (f.jobActive) out.push("Another job is running. Start the acquisition after it ends.");
  return out;
}

/**
 * The options a new acquisition starts with. "New acquisition" returns to them: the label names
 * one exhibit, encryption is changed only when ticked for this device, and turning it off again
 * afterwards is the default (D5).
 */
export const ACQ_OPTION_DEFAULTS = Object.freeze({
  label: "",
  enableEncryption: false,
  password: "",
  password2: "",
  restoreEncryption: true,
  parseAfter: false,
});

/**
 * The option controls of the Acquire screen (inputs and checkboxes).
 * @typedef {object} AcqControls
 * @property {{ value: string }} label
 * @property {{ checked: boolean }} enableEncryption
 * @property {{ value: string }} password
 * @property {{ value: string }} password2
 * @property {{ checked: boolean }} restoreEncryption
 * @property {{ checked: boolean }} parseAfter "Parse with iLEAPP now"
 */

/**
 * Sets every option control to `ACQ_OPTION_DEFAULTS` (the password fields are emptied).
 * @param {AcqControls} c
 */
export function resetAcqControls(c) {
  const d = ACQ_OPTION_DEFAULTS;
  c.label.value = d.label;
  c.enableEncryption.checked = d.enableEncryption;
  c.password.value = d.password;
  c.password2.value = d.password2;
  c.restoreEncryption.checked = d.restoreEncryption;
  c.parseAfter.checked = d.parseAfter;
}

/**
 * The `acq_start` request. Call only when `acqBlockers` is empty.
 * @param {string} casePath
 * @param {AcqForm} f
 * @returns {AcqRequest}
 */
export function buildAcqRequest(casePath, f) {
  if (!f.device) throw new Error("buildAcqRequest: no device");
  const enable = enablesEncryption(f);
  return {
    case_path: casePath,
    udid: f.device.udid,
    label: f.label.trim() === "" ? null : f.label.trim(),
    enable_encryption: enable,
    encryption_password: enable ? f.password : null,
    restore_encryption: enable && f.restoreEncryption,
  };
}

/** Warning codes after which the device may still encrypt backups with the examiner's password. */
export const ENCRYPTION_LEFT_ON = /** @type {const} */ (["encryption_left_enabled", "encryption_state_unknown"]);

/**
 * True when "Turn backup encryption off" applies. Pass `AcqSummary.warnings`: the core leaves the
 * two codes out once a later restore recorded `restored: true` (CONTRACTS.md §13.5), so this turns
 * false after a successful "Turn backup encryption off", while acquisition.json keeps them.
 * @param {readonly string[]} warningCodes
 */
export function needsEncryptionOff(warningCodes) {
  return warningCodes.some((c) => /** @type {readonly string[]} */ (ENCRYPTION_LEFT_ON).includes(c));
}

/**
 * Why backup encryption may still be on, for the "Turn backup encryption off" banner.
 * @param {readonly string[]} warningCodes
 * @returns {string | null}
 */
export function encryptionLeftOnText(warningCodes) {
  if (warningCodes.includes("encryption_left_enabled")) return "The app turned it on for this acquisition, and it was not turned off again.";
  if (warningCodes.includes("encryption_state_unknown")) return "The app tried to turn it on for this acquisition, and whether that worked is unknown.";
  return null;
}
