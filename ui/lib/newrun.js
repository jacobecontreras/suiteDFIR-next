// @ts-check
/**
 * New run form logic (tests/ui/newrun.test.js): why Start is disabled, and the `run_start`
 * request. The core validates everything again (ARCHITECTURE.md §6 step 1); these checks only
 * explain what is missing before the examiner presses Start.
 */
import { plural } from "./format.js";
import { TOOL_FEATURES } from "./tools.js";

/** @typedef {import("../types").InputInspection} InputInspection */
/** @typedef {import("../types").InputType} InputType */
/** @typedef {import("../types").ModuleMode} ModuleMode */
/** @typedef {import("../types").ProfileInfo} ProfileInfo */
/** @typedef {import("../types").RunRequest} RunRequest */
/** @typedef {import("../types").ToolId} ToolId */

/**
 * @typedef {object} NewRunForm
 * @property {ToolId | null} tool
 * @property {boolean} toolsInstalled At least one tool is installed.
 * @property {string | null} inputPath
 * @property {InputInspection | null} inspection
 * @property {boolean} inspecting
 * @property {boolean} inputFailed `input_inspect` returned an error (shown inline).
 * @property {InputType | null} inputType
 * @property {string | null} timezone
 * @property {string} password
 * @property {string | null} keychainPath
 * @property {boolean} hashInput
 * @property {string} label
 * @property {boolean} modulesLoaded
 * @property {ModuleMode} moduleMode
 * @property {ProfileInfo | null} profile The chosen profile (mode `profile`).
 * @property {readonly string[]} customModules The custom selection, unknown names included.
 * @property {readonly string[]} customUnknown The unknown names in `customModules`.
 * @property {boolean} jobActive
 */

/**
 * Why the run needs a backup password, or null: the tool takes one (iLEAPP), the input is read as
 * an iTunes backup, and the backup is encrypted (`encrypted`) or its encryption could not be read
 * (`unknown`, which the core treats as encrypted: ARCHITECTURE.md §6 step 1).
 * @param {Pick<NewRunForm, "tool" | "inspection" | "inputType">} f
 * @returns {"encrypted" | "unknown" | null}
 */
export function passwordReason(f) {
  if (f.tool === null || !TOOL_FEATURES[f.tool].password || f.inputType !== "itunes") return null;
  if (f.inspection?.itunes_encrypted === true) return "encrypted";
  if (f.inspection?.is_itunes_backup && f.inspection.itunes_encrypted === null) return "unknown";
  return null;
}

/**
 * True when the run needs a backup password (see `passwordReason`).
 * @param {Pick<NewRunForm, "tool" | "inspection" | "inputType">} f
 * @returns {boolean}
 */
export function needsPassword(f) {
  return passwordReason(f) !== null;
}

/**
 * The inline reason under the password field.
 * @param {"encrypted" | "unknown"} reason
 * @returns {string}
 */
export function passwordHint(reason) {
  const why =
    reason === "encrypted"
      ? "The backup is encrypted."
      : "The backup's encryption state couldn't be read, so a password is needed.";
  return `${why} The password goes to iLEAPP only, is never stored, and this field is cleared when the run starts.`;
}

/**
 * True when the input is a file that may be hashed (folders are never hashed).
 * @param {Pick<NewRunForm, "inspection">} f
 * @returns {boolean}
 */
export function canHash(f) {
  return f.inspection?.kind === "file" && f.inspection.hashable;
}

/**
 * Every reason Start is disabled, in form order. Empty = the form is ready.
 * @param {NewRunForm} f
 * @returns {string[]}
 */
export function startBlockers(f) {
  /** @type {string[]} */
  const out = [];
  if (!f.toolsInstalled) out.push("Install iLEAPP or aLEAPP in Settings.");
  else if (!f.tool) out.push("Choose a tool.");

  if (!f.inputPath) out.push("Choose an input file or folder.");
  else if (f.inspecting) out.push("Wait for the input check to finish.");
  else if (f.inputFailed || !f.inspection) out.push("Choose a different input (see the problem above).");
  else if (!f.inputType) out.push("Choose the input type.");
  else if (!f.inspection.allowed_types.includes(f.inputType)) out.push("Choose an input type allowed for this input.");

  const reason = passwordReason(f);
  if (reason && f.password === "") {
    out.push(
      reason === "encrypted"
        ? "Enter the backup password (the backup is encrypted)."
        : "Enter the backup password (the backup's encryption state couldn't be read).",
    );
  }
  if (f.tool && TOOL_FEATURES[f.tool].timezone && !f.timezone) out.push("Choose a timezone.");

  if (f.tool && !f.modulesLoaded) out.push("Wait for the module list to load.");
  else if (f.moduleMode === "profile") {
    if (!f.profile) out.push("Choose a profile.");
    else if (f.profile.unknown_modules.length > 0) {
      const n = f.profile.unknown_modules.length;
      out.push(
        `Profile “${f.profile.name}” has ${plural(n, "unknown module", "unknown modules")}: edit it as a custom selection and remove ${n === 1 ? "it" : "them"}.`,
      );
    }
  } else if (f.moduleMode === "custom") {
    if (f.customModules.length - f.customUnknown.length === 0) out.push("Select at least one module.");
    if (f.customUnknown.length > 0) out.push(`Remove ${plural(f.customUnknown.length, "unknown module", "unknown modules")}.`);
  }

  if (f.jobActive) out.push("Another job is running. Start this run after it ends.");
  return out;
}

/**
 * The `run_start` request. Call only when `startBlockers` is empty.
 * @param {string} casePath
 * @param {NewRunForm} f
 * @returns {RunRequest}
 */
export function buildRunRequest(casePath, f) {
  if (!f.tool || !f.inputPath || !f.inputType) throw new Error("buildRunRequest: the form is incomplete");
  const features = TOOL_FEATURES[f.tool];
  /** @type {RunRequest["modules"]} */
  let modules;
  if (f.moduleMode === "profile") {
    if (!f.profile) throw new Error("buildRunRequest: no profile");
    modules = { mode: "profile", profile_name: f.profile.name };
  } else if (f.moduleMode === "custom") {
    modules = { mode: "custom", modules: [...f.customModules] };
  } else {
    modules = { mode: "all" };
  }
  return {
    case_path: casePath,
    tool: f.tool,
    input_path: f.inputPath,
    input_type: f.inputType,
    modules,
    timezone: features.timezone ? f.timezone : null,
    itunes_password: needsPassword(f) ? f.password : null,
    keychain_path: features.keychain ? f.keychainPath : null,
    hash_input: canHash(f) && f.hashInput,
    label: f.label.trim() === "" ? null : f.label.trim(),
  };
}

/**
 * The type preselected for a fresh inspection: the detected type when it is allowed.
 * @param {InputInspection} inspection
 * @returns {InputType | null}
 */
export function initialInputType(inspection) {
  const detected = inspection.detected_type;
  if (detected && inspection.allowed_types.includes(detected)) return detected;
  return inspection.allowed_types.length === 1 ? inspection.allowed_types[0] : null;
}
