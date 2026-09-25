// @ts-check
// Mock record builders: run.json and acquisition.json records derived from the contract fixtures
// (CONTRACTS.md §7, §13.3), plus the summaries the IPC commands return. Not shipped.
import * as fx from "../fixtures/contracts/index.js";

/** @typedef {import("../../ui/types").AcqStatus} AcqStatus */
/** @typedef {import("../../ui/types").AcqSummary} AcqSummary */
/** @typedef {import("../../ui/types").AcquisitionRecord} AcquisitionRecord */
/** @typedef {import("../../ui/types").CaseFile} CaseFile */
/** @typedef {import("../../ui/types").InputKind} InputKind */
/** @typedef {import("../../ui/types").InputType} InputType */
/** @typedef {import("../../ui/types").ModuleSelection} ModuleSelection */
/** @typedef {import("../../ui/types").Reason} Reason */
/** @typedef {import("../../ui/types").RunRecord} RunRecord */
/** @typedef {import("../../ui/types").RunStatus} RunStatus */
/** @typedef {import("../../ui/types").RunSummary} RunSummary */
/** @typedef {import("../../ui/types").ToolId} ToolId */

/** Per-tool facts the core takes from `leapp-manifest.json` (CONTRACTS.md §3). */
export const TOOLS = {
  ileapp: {
    display_name: "iLEAPP",
    version: "v2026.4.2",
    profile_ext: "ilprofile",
    asset_name: "ileapp-v2026.4.2-macOS_Apple_Silicon.zip",
    /** @type {InputType[]} */
    input_types: ["fs", "tar", "zip", "gz", "itunes", "file", "raw"],
  },
  aleapp: {
    display_name: "aLEAPP",
    version: "v2026.4.1",
    profile_ext: "alprofile",
    asset_name: "aleapp-v2026.4.1-macOS_Apple_Silicon.zip",
    /** @type {InputType[]} */
    input_types: ["fs", "tar", "zip", "gz", "raw"],
  },
};

/**
 * RFC 3339 UTC at second precision.
 * @param {Date} [date]
 * @returns {string}
 */
export function isoNow(date = new Date()) {
  return date.toISOString().replace(/\.\d{3}Z$/, "Z");
}

/**
 * `YYYYMMDD-HHMMSSZ-<kind>-<6 hex>` (CONTRACTS.md §7.1, §13.3).
 * @param {string} kind `ileapp`, `aleapp` or `ios`
 * @param {Date} [date]
 * @returns {string}
 */
export function newJobId(kind, date = new Date()) {
  const stamp = isoNow(date).replace(/[-:]/g, "").replace("T", "-");
  const hex = Math.floor(Math.random() * 0x1000000).toString(16).padStart(6, "0");
  return `${stamp}-${kind}-${hex}`;
}

/**
 * @param {string} code
 * @param {string} message
 * @returns {Reason}
 */
const reason = (code, message) => ({ code, message });

export const REASONS = {
  no_output_dir: reason("no_output_dir", "LEAPP exited before creating output; see stdout"),
  lava_data_missing: reason("lava_data_missing", "report/_lava_data.lava is missing or unparsable"),
  no_modules_ran: reason("no_modules_ran", "No module ran; typical for invalid input or a wrong backup password"),
  index_html_missing: reason("index_html_missing", "report/index.html is missing"),
  /** @param {number} code */
  nonzero_exit: (code) =>
    reason("nonzero_exit", `LEAPP exited with code ${code}${code === 2 ? " (LEAPP rejected its arguments)" : ""}`),
  modules_errored: reason("modules_errored", "2 modules reported Error: safariHistory, notesDatabases"),
  cancelled_by_user: reason("cancelled_by_user", "Cancelled by the examiner"),
  app_interrupted: reason("app_interrupted", "The app closed while this job was running"),
  stderr_traceback: reason("stderr_traceback", "stderr contains a Python traceback (see leapp.stderr.log)"),
};

/**
 * @typedef {object} RunOutcome
 * @property {RunStatus} status
 * @property {Reason[]} reasons
 * @property {Reason[]} warnings
 * @property {number | null} exit_code
 * @property {number | null} signal
 * @property {boolean} report `report/` exists
 * @property {boolean} lava
 * @property {boolean} index
 * @property {{ complete: number, error: number, no_files_found: number, other: number } | null} counts
 * @property {string[]} error_modules
 */

/**
 * Final outcomes by scenario (the input path's last segment without extension, DEVELOPMENT.md
 * §4.6), mirroring the fake-leapp scenarios of CONTRACTS.md §7.4.
 * @type {Record<string, RunOutcome>}
 */
export const RUN_OUTCOMES = {
  success: {
    status: "succeeded", reasons: [], warnings: [], exit_code: 0, signal: null, report: true, lava: true, index: true,
    counts: { complete: 118, error: 0, no_files_found: 1058, other: 0 }, error_modules: [],
  },
  errors: {
    status: "completed_with_errors", reasons: [REASONS.modules_errored], warnings: [REASONS.stderr_traceback],
    exit_code: 0, signal: null, report: true, lava: true, index: true,
    counts: { complete: 120, error: 2, no_files_found: 1054, other: 0 }, error_modules: ["safariHistory", "notesDatabases"],
  },
  "fail-invalid": {
    status: "failed", reasons: [REASONS.no_modules_ran, REASONS.index_html_missing], warnings: [], exit_code: 0,
    signal: null, report: true, lava: true, index: false,
    counts: { complete: 0, error: 0, no_files_found: 0, other: 0 }, error_modules: [],
  },
  "fail-early": {
    status: "failed", reasons: [REASONS.no_output_dir], warnings: [], exit_code: 0, signal: null,
    report: false, lava: false, index: false, counts: null, error_modules: [],
  },
  "fail-argparse": {
    status: "failed", reasons: [REASONS.no_output_dir, REASONS.nonzero_exit(2)], warnings: [], exit_code: 2,
    signal: null, report: false, lava: false, index: false, counts: null, error_modules: [],
  },
  "fail-crash": {
    status: "failed", reasons: [REASONS.lava_data_missing, REASONS.index_html_missing, REASONS.nonzero_exit(1)],
    warnings: [REASONS.stderr_traceback], exit_code: 1, signal: null, report: true, lava: false, index: false,
    counts: null, error_modules: [],
  },
  cancelled: {
    status: "cancelled", reasons: [REASONS.cancelled_by_user], warnings: [], exit_code: null, signal: 15,
    report: true, lava: false, index: false, counts: null, error_modules: [],
  },
};

/**
 * @typedef {object} RunSpec
 * @property {string} casePath
 * @property {CaseFile} caseFile
 * @property {string} runId
 * @property {ToolId} tool
 * @property {string | null} label
 * @property {string} inputPath
 * @property {InputKind} inputKind
 * @property {InputType} inputType
 * @property {InputType | null} typeDetected
 * @property {number | null} sizeBytes
 * @property {boolean | null} itunesEncrypted
 * @property {string} createdAt
 * @property {string | null} timezone
 * @property {boolean} passwordSupplied
 * @property {string | null} keychainPath
 * @property {boolean} hashInput
 * @property {ModuleSelection} modules
 * @property {string[]} resolved
 * @property {string[]} alwaysRun
 * @property {number} availableCount
 */

/**
 * The initial `run.json` (CONTRACTS.md §7.2).
 * @param {RunSpec} s
 * @returns {RunRecord}
 */
export function initialRunRecord(s) {
  const rec = structuredClone(fx.RunRecordInitial);
  const tool = TOOLS[s.tool];
  const runDir = `${s.casePath}/runs/${s.runId}`;
  rec.run_id = s.runId;
  rec.label = s.label;
  rec.created_at = s.createdAt;
  rec.case_snapshot = {
    case_id: s.caseFile.case_id,
    name: s.caseFile.name,
    case_number: s.caseFile.case_number,
    examiner: s.caseFile.examiner,
    agency: s.caseFile.agency,
  };
  rec.tool = { ...rec.tool, id: s.tool, version: tool.version, asset_name: tool.asset_name };
  const hashStatus = s.inputKind === "directory" ? "not_applicable" : s.hashInput ? "pending" : "not_requested";
  rec.input = {
    path: s.inputPath,
    kind: s.inputKind,
    type: s.inputType,
    type_detected: s.typeDetected,
    size_bytes: s.sizeBytes,
    itunes_encrypted: s.itunesEncrypted,
    acquisition_id: acquisitionIdOf(s.casePath, s.inputPath),
    hash: { algorithm: "sha256", status: hashStatus, value: null, started_at: null, completed_at: null },
  };
  rec.options = {
    timezone: s.tool === "ileapp" ? s.timezone : null,
    timezone_supported: s.tool === "ileapp",
    password_supplied: s.passwordSupplied,
    keychain_path: s.tool === "ileapp" ? s.keychainPath : null,
    keychain_sha256: s.tool === "ileapp" && s.keychainPath ? "4b7f0c9d2e61a8b3f5c7d9e1a2b4c6d8e0f1a3b5c7d9e1f2a4b6c8d0e2f4a6b8" : null,
  };
  rec.modules = {
    mode: s.modules.mode,
    profile_name: s.modules.mode === "profile" ? s.modules.profile_name : null,
    requested: s.modules.mode === "custom" ? [...s.modules.modules] : s.modules.mode === "profile" ? [...s.resolved] : [],
    resolved: [...s.resolved],
    unknown: [],
    always_run: [...s.alwaysRun],
    available_count: s.availableCount,
  };
  const entry = rec.command.argv[0].replaceAll("ileapp", s.tool).replace(TOOLS.ileapp.version, tool.version);
  const argv = [entry, "-t", s.inputType, "-i", s.inputPath, "-o", runDir, "--custom_output_folder", "report"];
  argv.push("-d", `${runDir}/case.lcasedata`);
  if (s.modules.mode !== "all") argv.push("-m", `${runDir}/profile.${tool.profile_ext}`);
  if (s.tool === "ileapp") {
    argv.push("-tz", s.timezone ?? "UTC");
    if (s.passwordSupplied) argv.push("--itunes_password", "<redacted>");
    if (s.keychainPath) argv.push("--keychain", s.keychainPath);
  }
  rec.command = { argv, cwd: runDir };
  return rec;
}

/**
 * Applies a final outcome (CONTRACTS.md §7.1, §7.3).
 * @param {RunRecord} rec
 * @param {RunOutcome} outcome
 * @param {{ startedAt: string | null, exitedAt: string, endedAt: string }} times
 * @returns {RunRecord}
 */
export function finalizeRunRecord(rec, outcome, times) {
  rec.status = outcome.status;
  rec.status_reasons = structuredClone(outcome.reasons);
  rec.warnings = structuredClone(outcome.warnings);
  rec.started_at = times.startedAt;
  rec.ended_at = times.endedAt;
  rec.duration_ms = Date.parse(times.endedAt) - Date.parse(rec.created_at);
  rec.process = {
    exit_code: outcome.exit_code,
    signal: outcome.signal,
    exited_at: times.exitedAt,
    cancel_requested: outcome.status === "cancelled",
    escalated_to_kill: false,
  };
  rec.leapp_result = {
    lava_data_found: outcome.lava,
    processing_status: outcome.lava ? "Complete" : null,
    leapp_version_reported: outcome.lava ? rec.tool.version.replace(/^v/, "") : null,
    index_html_found: outcome.index,
    module_counts: outcome.counts ? { ...outcome.counts } : null,
    error_modules: [...outcome.error_modules],
  };
  if (rec.input.hash.status === "pending") {
    rec.input.hash = {
      algorithm: "sha256",
      status: outcome.status === "cancelled" ? "cancelled" : "completed",
      value: outcome.status === "cancelled" ? null : "9f2c4e6a8b0d1f3a5c7e9b1d3f5a7c9e1b3d5f7a9c1e3b5d7f9a1c3e5b7d9f1a",
      started_at: times.startedAt,
      completed_at: outcome.status === "cancelled" ? null : times.exitedAt,
    };
  }
  rec.output.seal = outcome.report
    ? {
        status: "sealed",
        manifest: "report.sha256",
        manifest_sha256: "16c6428a0dd24eeb087bcb072aee9a16d37d29aa661f6b16cddbec25d4f2c9aa",
        file_count: outcome.index ? 5321 : 214,
        total_bytes: outcome.index ? 123456789 : 3456789,
      }
    : { status: "skipped_no_output", manifest: null, manifest_sha256: null, file_count: null, total_bytes: null };
  return rec;
}

/**
 * Recovery on `case_open` (CONTRACTS.md §7.2).
 * @param {RunRecord} rec
 * @param {string} at
 */
export function interruptRunRecord(rec, at) {
  rec.status = "interrupted";
  rec.status_reasons = [REASONS.app_interrupted];
  rec.recovered_at = at;
  if (rec.input.hash.status === "pending") rec.input.hash.status = "interrupted";
  if (rec.output.seal.status === "pending") rec.output.seal.status = "interrupted";
}

/**
 * @param {string} casePath
 * @param {RunRecord} rec
 * @returns {RunSummary}
 */
export function runSummary(casePath, rec) {
  return {
    run_id: rec.run_id,
    run_dir: `${casePath}/runs/${rec.run_id}`,
    label: rec.label,
    status: rec.status,
    tool: rec.tool.id,
    tool_version: rec.tool.version,
    input_path: rec.input.path,
    input_type: rec.input.type,
    created_at: rec.created_at,
    started_at: rec.started_at,
    ended_at: rec.ended_at,
    duration_ms: rec.duration_ms,
    report_available: rec.leapp_result?.index_html_found === true,
  };
}

/**
 * `input.acquisition_id`: the acq id when the input lies in this case's `acquisitions/<acq_id>/`.
 * @param {string} casePath
 * @param {string} inputPath
 * @returns {string | null}
 */
export function acquisitionIdOf(casePath, inputPath) {
  const prefix = `${casePath}/acquisitions/`;
  if (!inputPath.startsWith(prefix)) return null;
  return inputPath.slice(prefix.length).split("/")[0] || null;
}

// ---- Acquisitions ----

/**
 * @typedef {object} AcqSpec
 * @property {string} casePath
 * @property {CaseFile} caseFile
 * @property {string} acqId
 * @property {string | null} label
 * @property {import("../../ui/types").DeviceSummary} device
 * @property {boolean} pairedByApp
 * @property {boolean} enableEncryption
 * @property {boolean} restoreEncryption
 * @property {string} createdAt
 */

/**
 * The initial `acquisition.json` (CONTRACTS.md §13.3).
 * @param {AcqSpec} s
 * @returns {AcquisitionRecord}
 */
export function initialAcqRecord(s) {
  const rec = structuredClone(fx.AcquisitionRecord);
  rec.acq_id = s.acqId;
  rec.label = s.label;
  rec.status = "running";
  rec.status_reasons = [];
  rec.warnings = [];
  rec.created_at = s.createdAt;
  rec.started_at = null;
  rec.ended_at = null;
  rec.recovered_at = null;
  rec.duration_ms = null;
  rec.case_snapshot = {
    case_id: s.caseFile.case_id,
    name: s.caseFile.name,
    case_number: s.caseFile.case_number,
    examiner: s.caseFile.examiner,
    agency: s.caseFile.agency,
  };
  rec.device = {
    ...rec.device,
    udid: s.device.udid,
    serial_number: s.device.serial_number,
    device_name: s.device.device_name,
    product_type: s.device.product_type,
    product_version: s.device.product_version,
    captured_at: s.createdAt,
  };
  rec.pairing = { ...rec.pairing, paired_before: !s.pairedByApp, paired_by_app_at: s.pairedByApp ? s.createdAt : null };
  rec.device_changes = [];
  rec.encryption = {
    will_encrypt_before: s.device.will_encrypt,
    enable_requested: s.enableEncryption,
    enabled_by_examiner: false,
    will_encrypt_after_enable: null,
    restore_requested: s.enableEncryption && s.restoreEncryption,
    restored_after: s.enableEncryption && s.restoreEncryption ? "not_attempted" : "not_requested",
    will_encrypt_after_restore: null,
    password_supplied: s.enableEncryption,
    password_channel: s.enableEncryption ? "env" : null,
  };
  rec.commands = [];
  rec.process = null;
  rec.backup_result = null;
  rec.output.seal = { status: "pending", manifest: null, manifest_sha256: null, file_count: null, total_bytes: null };
  return rec;
}

/**
 * @param {string} casePath
 * @param {AcquisitionRecord} rec
 * @returns {AcqSummary}
 */
export function acqSummary(casePath, rec) {
  const acqDir = `${casePath}/acquisitions/${rec.acq_id}`;
  return {
    acq_id: rec.acq_id,
    acq_dir: acqDir,
    label: rec.label,
    status: rec.status,
    udid: rec.device.udid,
    device_name: rec.device.device_name,
    product_version: rec.device.product_version,
    created_at: rec.created_at,
    started_at: rec.started_at,
    ended_at: rec.ended_at,
    duration_ms: rec.duration_ms,
    backup_path: rec.status === "succeeded" ? `${acqDir}/backup/${rec.device.udid}` : null,
    warnings: rec.warnings.map((w) => w.code),
  };
}

/**
 * Recovery on `case_open` (CONTRACTS.md §13.3).
 * @param {AcquisitionRecord} rec
 * @param {string} at
 */
export function interruptAcqRecord(rec, at) {
  rec.status = "interrupted";
  rec.status_reasons = [REASONS.app_interrupted];
  rec.recovered_at = at;
  if (rec.encryption.enabled_by_examiner && rec.encryption.restored_after !== "restored") {
    rec.warnings.push(reason("encryption_left_enabled", "Backup encryption was enabled by the examiner and not confirmed disabled"));
  }
  if (rec.encryption.enable_requested && rec.encryption.will_encrypt_after_enable === null && rec.commands.length > 0) {
    rec.warnings.push(reason("encryption_state_unknown", "The device's backup-encryption state is unknown"));
  }
  if (rec.output.seal.status === "pending") rec.output.seal.status = "interrupted";
}
