// @ts-check
/**
 * The real API: the only module that calls into `window.__TAURI__` (DEVELOPMENT.md §4.6).
 *
 * One export per Tauri command in CONTRACTS.md §10 and §13.5, plus the two dialog-plugin calls.
 * ui-dev/mock.js exports exactly the same names (tests/ui/parity.test.js). Every failure rejects
 * with an `AppError`.
 */
import { toAppError } from "../lib/errors.js";

/** @typedef {import("../types").AcqCancelRequest} AcqCancelRequest */
/** @typedef {import("../types").AcqEvent} AcqEvent */
/** @typedef {import("../types").AcqPreflight} AcqPreflight */
/** @typedef {import("../types").AcqPreflightRequest} AcqPreflightRequest */
/** @typedef {import("../types").AcqRef} AcqRef */
/** @typedef {import("../types").AcqRequest} AcqRequest */
/** @typedef {import("../types").AcqRestoreEncryptionRequest} AcqRestoreEncryptionRequest */
/** @typedef {import("../types").AcqRestoreEncryptionResult} AcqRestoreEncryptionResult */
/** @typedef {import("../types").AcqStarted} AcqStarted */
/** @typedef {import("../types").AcquisitionRecord} AcquisitionRecord */
/** @typedef {import("../types").ActiveJob} ActiveJob */
/** @typedef {import("../types").AppInfo} AppInfo */
/** @typedef {import("../types").CaseCreateRequest} CaseCreateRequest */
/** @typedef {import("../types").CaseDetail} CaseDetail */
/** @typedef {import("../types").CaseSummary} CaseSummary */
/** @typedef {import("../types").CaseUpdateRequest} CaseUpdateRequest */
/** @typedef {import("../types").DevicePairRequest} DevicePairRequest */
/** @typedef {import("../types").DeviceSummary} DeviceSummary */
/** @typedef {import("../types").DevicesResult} DevicesResult */
/** @typedef {import("../types").InputInspectRequest} InputInspectRequest */
/** @typedef {import("../types").InputInspection} InputInspection */
/** @typedef {import("../types").InstallEvent} InstallEvent */
/** @typedef {import("../types").IosBackup} IosBackup */
/** @typedef {import("../types").JobAttachRequest} JobAttachRequest */
/** @typedef {import("../types").JobBacklog} JobBacklog */
/** @typedef {import("../types").OpenAcqFileRequest} OpenAcqFileRequest */
/** @typedef {import("../types").OpenTextFileRequest} OpenTextFileRequest */
/** @typedef {import("../types").PathRequest} PathRequest */
/** @typedef {import("../types").ProfileExportRequest} ProfileExportRequest */
/** @typedef {import("../types").ProfileImportRequest} ProfileImportRequest */
/** @typedef {import("../types").ProfileInfo} ProfileInfo */
/** @typedef {import("../types").ProfileRef} ProfileRef */
/** @typedef {import("../types").ProfileSaveRequest} ProfileSaveRequest */
/** @typedef {import("../types").RunCancelRequest} RunCancelRequest */
/** @typedef {import("../types").RunEvent} RunEvent */
/** @typedef {import("../types").RunRecord} RunRecord */
/** @typedef {import("../types").RunRef} RunRef */
/** @typedef {import("../types").RunRequest} RunRequest */
/** @typedef {import("../types").RunStarted} RunStarted */
/** @typedef {import("../types").Settings} Settings */
/** @typedef {import("../types").SettingsUpdateRequest} SettingsUpdateRequest */
/** @typedef {import("../types").TempCleanupResult} TempCleanupResult */
/** @typedef {import("../types").ToolImportRequest} ToolImportRequest */
/** @typedef {import("../types").ToolModules} ToolModules */
/** @typedef {import("../types").ToolRequest} ToolRequest */
/** @typedef {import("../types").ToolStatus} ToolStatus */

/**
 * @typedef {object} DialogFilter
 * @property {string} name
 * @property {string[]} extensions Without the dot.
 */

/**
 * @typedef {object} OpenDialogOptions
 * @property {string} title
 * @property {boolean} [directory] Pick a folder instead of a file.
 * @property {DialogFilter[]} [filters]
 * @property {string} [defaultPath]
 */

/**
 * @typedef {object} SaveDialogOptions
 * @property {string} title
 * @property {DialogFilter[]} [filters]
 * @property {string} [defaultPath]
 */

/** @returns {TauriGlobal} */
function tauri() {
  const t = window.__TAURI__;
  if (!t) {
    throw { code: "internal", message: "The app API is not available.", detail: null };
  }
  return t;
}

/**
 * Invokes a command; a rejection is normalized to an `AppError`.
 * @param {string} cmd
 * @param {Record<string, unknown>} [args]
 * @returns {Promise<any>}
 */
async function invoke(cmd, args) {
  try {
    return await tauri().core.invoke(cmd, args);
  } catch (err) {
    throw toAppError(err);
  }
}

/**
 * A command that streams events over a `Channel` named `onEvent` (CONTRACTS.md §10).
 * @template E
 * @param {string} cmd
 * @param {unknown} req
 * @param {(event: E) => void} onEvent
 * @returns {Promise<any>}
 */
async function invokeWithEvents(cmd, req, onEvent) {
  /** @type {TauriChannel<E>} */
  let channel;
  try {
    channel = new (tauri().core.Channel)();
  } catch (err) {
    throw toAppError(err);
  }
  channel.onmessage = onEvent;
  return invoke(cmd, { req, onEvent: channel });
}

// ---- §10 ----

/** @returns {Promise<AppInfo>} */
export const app_info = () => invoke("app_info");
/** @returns {Promise<string>} */
export const licenses_get = () => invoke("licenses_get");
/** @returns {Promise<Settings>} */
export const settings_get = () => invoke("settings_get");
/** @param {SettingsUpdateRequest} req @returns {Promise<Settings>} */
export const settings_update = (req) => invoke("settings_update", { req });
/** @returns {Promise<ToolStatus[]>} */
export const tools_status = () => invoke("tools_status");
/** @param {ToolRequest} req @returns {Promise<ToolStatus>} */
export const tool_verify = (req) => invoke("tool_verify", { req });
/** @param {ToolRequest} req @param {(event: InstallEvent) => void} onEvent @returns {Promise<ToolStatus>} */
export const tool_install = (req, onEvent) => invokeWithEvents("tool_install", req, onEvent);
/** @param {ToolImportRequest} req @param {(event: InstallEvent) => void} onEvent @returns {Promise<ToolStatus>} */
export const tool_import = (req, onEvent) => invokeWithEvents("tool_import", req, onEvent);
/** @param {ToolRequest} req @returns {Promise<ToolModules>} */
export const tool_modules = (req) => invoke("tool_modules", { req });
/** @returns {Promise<CaseSummary[]>} */
export const cases_list = () => invoke("cases_list");
/** @param {CaseCreateRequest} req @returns {Promise<CaseDetail>} */
export const case_create = (req) => invoke("case_create", { req });
/** @param {PathRequest} req @returns {Promise<CaseDetail>} */
export const case_open = (req) => invoke("case_open", { req });
/** @param {CaseUpdateRequest} req @returns {Promise<CaseDetail>} */
export const case_update = (req) => invoke("case_update", { req });
/** @param {PathRequest} req @returns {Promise<void>} */
export const case_forget = (req) => invoke("case_forget", { req });
/** @param {RunRef} req @returns {Promise<RunRecord>} */
export const run_get = (req) => invoke("run_get", { req });
/** @param {InputInspectRequest} req @returns {Promise<InputInspection>} */
export const input_inspect = (req) => invoke("input_inspect", { req });
/** @returns {Promise<IosBackup[]>} */
export const ios_backups_find = () => invoke("ios_backups_find");
/** @param {ToolRequest} req @returns {Promise<ProfileInfo[]>} */
export const profiles_list = (req) => invoke("profiles_list", { req });
/** @param {ProfileSaveRequest} req @returns {Promise<ProfileInfo>} */
export const profile_save = (req) => invoke("profile_save", { req });
/** @param {ProfileRef} req @returns {Promise<void>} */
export const profile_delete = (req) => invoke("profile_delete", { req });
/** @param {ProfileImportRequest} req @returns {Promise<ProfileInfo>} */
export const profile_import = (req) => invoke("profile_import", { req });
/** @param {ProfileExportRequest} req @returns {Promise<void>} */
export const profile_export = (req) => invoke("profile_export", { req });
/** @param {RunRequest} req @param {(event: RunEvent) => void} onEvent @returns {Promise<RunStarted>} */
export const run_start = (req, onEvent) => invokeWithEvents("run_start", req, onEvent);
/** @param {RunCancelRequest} req @returns {Promise<void>} */
export const run_cancel = (req) => invoke("run_cancel", { req });
/** @returns {Promise<ActiveJob | null>} */
export const job_active = () => invoke("job_active");
/**
 * `onEvent` receives `RunEvent`s for `kind: "run"` and `AcqEvent`s for `kind: "acquisition"`.
 * @param {JobAttachRequest} req @param {(event: RunEvent | AcqEvent) => void} onEvent
 * @returns {Promise<JobBacklog>}
 */
export const job_attach = (req, onEvent) => invokeWithEvents("job_attach", req, onEvent);
/** @param {RunRef} req @returns {Promise<void>} */
export const open_report = (req) => invoke("open_report", { req });
/** @param {PathRequest} req @returns {Promise<void>} */
export const reveal_path = (req) => invoke("reveal_path", { req });
/** @param {OpenTextFileRequest} req @returns {Promise<void>} */
export const open_text_file = (req) => invoke("open_text_file", { req });
/** @returns {Promise<TempCleanupResult>} */
export const temp_cleanup = () => invoke("temp_cleanup");

// ---- §13.5 ----

/** @returns {Promise<DevicesResult>} */
export const devices_list = () => invoke("devices_list");
/** @param {DevicePairRequest} req @returns {Promise<DeviceSummary>} */
export const device_pair = (req) => invoke("device_pair", { req });
/** @param {AcqPreflightRequest} req @returns {Promise<AcqPreflight>} */
export const acq_preflight = (req) => invoke("acq_preflight", { req });
/** @param {AcqRequest} req @param {(event: AcqEvent) => void} onEvent @returns {Promise<AcqStarted>} */
export const acq_start = (req, onEvent) => invokeWithEvents("acq_start", req, onEvent);
/** @param {AcqCancelRequest} req @returns {Promise<void>} */
export const acq_cancel = (req) => invoke("acq_cancel", { req });
/** @param {AcqRef} req @returns {Promise<AcquisitionRecord>} */
export const acq_get = (req) => invoke("acq_get", { req });
/** @param {AcqRestoreEncryptionRequest} req @returns {Promise<AcqRestoreEncryptionResult>} */
export const acq_restore_encryption = (req) => invoke("acq_restore_encryption", { req });
/** @param {OpenAcqFileRequest} req @returns {Promise<void>} */
export const open_acq_file = (req) => invoke("open_acq_file", { req });

// ---- Dialog plugin (capabilities `dialog:allow-open`, `dialog:allow-save`) ----

/**
 * The native open dialog. Resolves to the chosen path, or null if cancelled.
 * @param {OpenDialogOptions} options
 * @returns {Promise<string | null>}
 */
export async function dialog_open(options) {
  try {
    const picked = await tauri().dialog.open({
      title: options.title,
      directory: options.directory ?? false,
      multiple: false,
      filters: options.filters,
      defaultPath: options.defaultPath,
    });
    return typeof picked === "string" ? picked : null;
  } catch (err) {
    throw toAppError(err);
  }
}

/**
 * The native save dialog. Resolves to the chosen path, or null if cancelled.
 * @param {SaveDialogOptions} options
 * @returns {Promise<string | null>}
 */
export async function dialog_save(options) {
  try {
    return await tauri().dialog.save({
      title: options.title,
      filters: options.filters,
      defaultPath: options.defaultPath,
    });
  } catch (err) {
    throw toAppError(err);
  }
}
