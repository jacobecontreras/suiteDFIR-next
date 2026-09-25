// @ts-check
// Browser mock of the IPC API (ARCHITECTURE.md §5.3, DEVELOPMENT.md §4.6). Served only by
// scripts/serve-ui.mjs at /dev/mock.js; never bundled.
//
// It exports exactly the functions of ui/api/ipc.js (tests/ui/parity.test.js): every command in
// CONTRACTS.md §10 and §13.5 plus the two dialogs. Data comes from the generated contract fixtures
// (./fixtures/contracts/index.js, the same data as the *.json files), varied per scenario.
//
// Scenarios:
// - URL flags, `?mock&scenario=a,b`: `empty` (no recent cases), `no_tools` (no parser installed),
//   `dev_override`, `active_run` (a slow run is already active at load), `install_fail`,
//   `no_devices`, `idevice_missing`, `usbmuxd_unavailable`, `idevice_verification_failed`,
//   `idevice_unsupported`, `preflight_warn`, `preflight_block`, `tool_verification_failed` (iLEAPP
//   fails verification), `tool_unsupported` (no aLEAPP build for the platform), `windows`
//   (`app_info` reports Windows x64: backup passwords must be printable ASCII).
//   `hold_<phase>` (e.g. `hold_analyzing`, `hold_sealing_report`, `hold_enabling_encryption`,
//   `hold_backing_up`): a run or acquisition stops in that phase (after its device prompt, if any)
//   until it is cancelled, so every phase can be screenshotted. With `active_run` / `active_acq`, a
//   `hold_<phase>` flag makes the start-up job a normal one (an acquisition then turns encryption
//   on and off), so it reaches that phase before the UI attaches, as after a reload mid-phase.
//   `pair_states`: one device in every PairState, as `devices_list` reports it (the real core
//   reports `not_paired` while there is no host pair record, e.g. after a Pair that returned
//   `awaiting_trust`; this rendering fixture shows every state at once). `active_acq`: a slow
//   acquisition of the first device is already active at load. `idevice_session_error`: the tools
//   are ok but listing the devices failed (state `ok` with `guidance`, as the core reports it).
// - Pairing: Pair on a device that was never asked answers `awaiting_trust` (the Trust dialog) and
//   creates no host record, so `devices_list` still reports `not_paired`; the next Pair pairs.
// - Later restores (`acq_restore_encryption`): each attempt is numbered (encryption-restore.json,
//   encryption-restore-2.json, …). The password `wrong` fails an attempt, which can be retried;
//   after an attempt that restored, `AcqSummary.warnings` leaves out `encryption_left_enabled` and
//   `encryption_state_unknown`, and further attempts are refused (`restore_not_applicable`).
// - Runs: the final status is chosen by the input path's last segment without extension:
//   `errors`, `fail-invalid`, `fail-early`, `fail-argparse`, `fail-crash`, `slow` (runs until
//   cancelled), `interrupt` (the "app crashes": the next case_open marks it interrupted), `flood`
//   (100,000 log lines in batches of 500, then success); anything else succeeds. `run_cancel`
//   gives `cancelled`.
// - Inputs: `…/denied` → permission_denied, `…/missing…` → invalid_input, anything overlapping a
//   case (ARCHITECTURE.md §6 step 1) → input_overlaps_case. A folder named like a backup
//   (`…backup…`, a UDID) is an iTunes backup: encrypted when its name has `encrypted`, of unknown
//   encryption (null, which needs a password as if encrypted) when it ends in `encryption-unknown`.
// - Acquisitions: chosen by a label suffix `/<scenario>` (fake-idevice names, CONTRACTS.md §13.4):
//   `/backup_fail`, `/enable_fail`, `/restore_fail`, `/incomplete`, `/cancel_on_device`,
//   `/disconnect`, `/sync_lock`, `/slow`, `/interrupt`; anything else succeeds. `acq_cancel`
//   follows the per-phase semantics of ARCHITECTURE.md §6b.
// Simulated steps are `globalThis.__SUITEDFIR_MOCK_TICK_MS` apart (default 300; tests set 1).
import * as fx from "./fixtures/contracts/index.js";
import { makeModules } from "./fixtures/modules.js";
import { pickOpen, pickSave, SAMPLES } from "./mock/picker.js";
import {
  REASONS,
  RUN_OUTCOMES,
  TOOLS,
  acquisitionIdOf,
  acqSummary,
  finalizeRunRecord,
  initialAcqRecord,
  initialRunRecord,
  interruptAcqRecord,
  interruptRunRecord,
  isoNow,
  newJobId,
  runSummary,
} from "./mock/records.js";

/** @typedef {import("../ui/types").AcqEvent} AcqEvent */
/** @typedef {import("../ui/types").AcqPhase} AcqPhase */
/** @typedef {import("../ui/types").AcqRequest} AcqRequest */
/** @typedef {import("../ui/types").AcqStatus} AcqStatus */
/** @typedef {import("../ui/types").AcquisitionRecord} AcquisitionRecord */
/** @typedef {import("../ui/types").ActiveJob} ActiveJob */
/** @typedef {import("../ui/types").AppError} AppError */
/** @typedef {import("../ui/types").CaseDetail} CaseDetail */
/** @typedef {import("../ui/types").CaseFields} CaseFields */
/** @typedef {import("../ui/types").CaseFile} CaseFile */
/** @typedef {import("../ui/types").DeviceSummary} DeviceSummary */
/** @typedef {import("../ui/types").ErrorCode} ErrorCode */
/** @typedef {import("../ui/types").InputInspection} InputInspection */
/** @typedef {import("../ui/types").InputType} InputType */
/** @typedef {import("../ui/types").InstallEvent} InstallEvent */
/** @typedef {import("../ui/types").ModuleSelection} ModuleSelection */
/** @typedef {import("../ui/types").ProfileInfo} ProfileInfo */
/** @typedef {import("../ui/types").Reason} Reason */
/** @typedef {import("../ui/types").RunEvent} RunEvent */
/** @typedef {import("../ui/types").RunPhase} RunPhase */
/** @typedef {import("../ui/types").RunRecord} RunRecord */
/** @typedef {import("../ui/types").ToolId} ToolId */
/** @typedef {import("../ui/types").ToolStatus} ToolStatus */
/** @typedef {typeof import("../ui/api/ipc.js")} Api */

// ---- Scenario flags, timing, helpers ----

const FLAGS = new Set(
  (typeof location === "undefined" ? "" : (new URLSearchParams(location.search).get("scenario") ?? ""))
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean),
);
/** @param {string} flag */
const has = (flag) => FLAGS.has(flag);
/** A `hold_<phase>` flag is set: a job started at load must reach that phase. */
const HOLDING = [...FLAGS].some((flag) => flag.startsWith("hold_"));

function tick() {
  const t = /** @type {any} */ (globalThis).__SUITEDFIR_MOCK_TICK_MS;
  return typeof t === "number" ? t : 300;
}

/** @param {number} ms */
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * With the `hold_<phase>` flag, keeps the job in `phase` until it is cancelled.
 * @param {{ cancelRequested: boolean }} j
 * @param {string} phase
 */
async function holdIn(j, phase) {
  if (!has(`hold_${phase}`)) return;
  while (!j.cancelRequested) await sleep(Math.max(tick(), 50));
}

/** `flood` runs: lines and batch size (the core sends at most 500 lines per `log` event). */
const FLOOD_LINES = 100_000;
const FLOOD_BATCH = 500;

/**
 * @param {ErrorCode} code
 * @param {string} message
 * @param {string | null} [detail]
 * @returns {AppError}
 */
const appError = (code, message, detail = null) => ({ code, message, detail });

/**
 * @template T
 * @param {T} value
 * @returns {T}
 */
const clone = (value) => structuredClone(value);

/** @param {string} path */
const baseName = (path) => path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? "";
/** @param {string} path */
const stemOf = (path) => baseName(path).replace(/\.[^.]*$/, "");
/** @param {string} path */
const extOf = (path) => (/\.([^./\\]+)$/.exec(baseName(path))?.[1] ?? "").toLowerCase();
/**
 * True if `child` equals `parent` or lies inside it.
 * @param {string} child
 * @param {string} parent
 */
const within = (child, parent) => child === parent || child.startsWith(`${parent.replace(/[\\/]+$/, "")}/`);

/** @param {number} n */
const randomHex = (n) => Array.from({ length: n }, () => Math.floor(Math.random() * 16).toString(16)).join("");

// ---- Seed data ----

const CASES_ROOT = fx.Settings.cases_root;
const NIGHTJAR = fx.CaseDetail.path;
const HARBOR = `${CASES_ROOT}/Harbor Lights`;
const COLD = `${CASES_ROOT}/Cold Case 7`;
const MISSING = "/Volumes/Archive/suiteDFIR Cases/Riverside 2025";
const APP_DATA = fx.AppInfo.paths.app_data;
const LEAPP_DIR = `${APP_DATA}/leapp`;

/** @type {Record<ToolId, import("../ui/types").ModuleInfo[]>} */
const MODULES = { ileapp: makeModules("ileapp", 1300), aleapp: makeModules("aleapp", 1288) };
/** @type {Record<ToolId, Record<string, string[]>>} */
const ALWAYS_RUN = { ileapp: clone(fx.ToolModules.always_run), aleapp: { default: ["usagestatsVersion"] } };

/** @returns {string[]} iLEAPP's zone list: the fixture's plus the browser's (a real list has ~600). */
function ileappTimezones() {
  const intl = typeof Intl.supportedValuesOf === "function" ? Intl.supportedValuesOf("timeZone") : [];
  return [...new Set([...(fx.ToolModules.timezones ?? []), ...intl, "UTC"])].sort();
}
const TIMEZONES = ileappTimezones();

/**
 * @param {ToolId} tool
 * @returns {ToolStatus}
 */
function installedStatus(tool) {
  const t = TOOLS[tool];
  return {
    ...clone(fx.ToolStatus),
    tool,
    display_name: t.display_name,
    pinned_version: t.version,
    state: tool === "ileapp" ? "verified" : "installed_unverified",
    installed_version: t.version,
    install_source: "download",
    module_count: MODULES[tool].length,
    install_dir: `${LEAPP_DIR}/${tool}/${t.version}`,
  };
}

/**
 * @param {ToolId} tool
 * @returns {ToolStatus}
 */
function notInstalledStatus(tool) {
  return {
    ...installedStatus(tool),
    state: "not_installed",
    installed_version: null,
    install_source: null,
    module_count: null,
    install_dir: null,
  };
}

/** @type {Record<ToolId, ToolStatus>} */
const tools = {
  ileapp: has("no_tools") ? notInstalledStatus("ileapp") : installedStatus("ileapp"),
  aleapp: has("no_tools") ? notInstalledStatus("aleapp") : installedStatus("aleapp"),
};
if (has("dev_override")) {
  for (const tool of /** @type {ToolId[]} */ (["ileapp", "aleapp"])) {
    tools[tool] = { ...installedStatus(tool), state: "dev_override", installed_version: "dev-override", install_source: "dev_override" };
  }
}
if (has("tool_verification_failed")) {
  tools.ileapp = {
    ...installedStatus("ileapp"),
    state: "verification_failed",
    problem: "The entry's SHA-256 does not match the pinned value: expected 5c1e0b3a…9d42, found 77ab04e1…c3f0 (bin/ileapp).",
  };
}
if (has("tool_unsupported")) {
  tools.aleapp = { ...notInstalledStatus("aleapp"), state: "unsupported_platform", problem: null };
}

const settings = clone(fx.Settings);
settings.recent_cases = has("empty") ? [] : [NIGHTJAR, HARBOR, MISSING];

/**
 * @typedef {object} MockCase
 * @property {CaseFile} file
 * @property {RunRecord[]} runs
 * @property {AcquisitionRecord[]} acqs
 */

/**
 * @param {string} name
 * @param {Partial<CaseFile>} fields
 * @returns {CaseFile}
 */
function caseFile(name, fields) {
  return { ...clone(fx.CaseFile), case_id: randomHex(32), name, ...fields };
}

/**
 * @param {string} casePath
 * @param {CaseFile} file
 * @param {object} o
 * @param {ToolId} o.tool
 * @param {string | null} o.label
 * @param {string} o.input
 * @param {"file" | "directory"} o.kind
 * @param {InputType} o.type
 * @param {string} o.created
 * @param {number} o.minutes
 * @param {keyof typeof RUN_OUTCOMES | "interrupted"} o.outcome
 * @param {ModuleSelection} [o.modules]
 * @returns {RunRecord}
 */
function seedRun(casePath, file, o) {
  const created = new Date(o.created);
  const modules = o.modules ?? { mode: "all" };
  const rec = initialRunRecord({
    casePath,
    caseFile: file,
    runId: newJobId(o.tool, created),
    tool: o.tool,
    label: o.label,
    inputPath: o.input,
    inputKind: o.kind,
    inputType: o.type,
    typeDetected: o.type,
    sizeBytes: o.kind === "file" ? 13314398617 : null,
    itunesEncrypted: o.type === "itunes" ? true : null,
    createdAt: o.created,
    timezone: o.tool === "ileapp" ? file.default_timezone ?? "UTC" : null,
    passwordSupplied: o.type === "itunes",
    keychainPath: null,
    hashInput: o.kind === "file",
    modules,
    resolved: modules.mode === "custom" ? modules.modules : MODULES[o.tool].map((m) => m.name).sort(),
    alwaysRun: ALWAYS_RUN[o.tool][o.type] ?? ALWAYS_RUN[o.tool].default,
    availableCount: MODULES[o.tool].length,
  });
  const started = isoNow(new Date(created.getTime() + 1000));
  if (o.outcome === "interrupted") {
    rec.started_at = started;
    interruptRunRecord(rec, isoNow(new Date(created.getTime() + 86400000)));
    return rec;
  }
  const end = new Date(created.getTime() + o.minutes * 60000);
  return finalizeRunRecord(rec, RUN_OUTCOMES[o.outcome], {
    startedAt: o.outcome === "fail-argparse" ? null : started,
    exitedAt: isoNow(new Date(end.getTime() - 20000)),
    endedAt: isoNow(end),
  });
}

/** @type {Map<string, MockCase>} */
const cases = new Map();
{
  const file = clone(fx.CaseFile);
  const ev = "/Volumes/Evidence";
  cases.set(NIGHTJAR, {
    file,
    runs: [
      clone(fx.RunRecord),
      seedRun(NIGHTJAR, file, { tool: "ileapp", label: "Full file system", input: `${ev}/iPhone-12-FFS.zip`, kind: "file", type: "zip", created: "2026-09-23T14:02:11Z", minutes: 41, outcome: "success" }),
      seedRun(NIGHTJAR, file, { tool: "ileapp", label: "Wrong backup password", input: `${ev}/00008101-000A1B2C3D4E`, kind: "directory", type: "itunes", created: "2026-09-23T13:40:00Z", minutes: 3, outcome: "fail-invalid" }),
      seedRun(NIGHTJAR, file, { tool: "aleapp", label: null, input: `${ev}/Pixel-7.tar`, kind: "file", type: "tar", created: "2026-09-22T10:15:30Z", minutes: 7, outcome: "cancelled" }),
      seedRun(NIGHTJAR, file, { tool: "ileapp", label: "Before the power cut", input: `${ev}/iPhone-11-backup`, kind: "directory", type: "itunes", created: "2026-09-21T09:00:00Z", minutes: 0, outcome: "interrupted" }),
      seedRun(NIGHTJAR, file, { tool: "aleapp", label: "Pixel 7 logical", input: `${ev}/Pixel-7-extraction`, kind: "directory", type: "fs", created: "2026-09-20T16:45:10Z", minutes: 18, outcome: "success" }),
      seedRun(NIGHTJAR, file, { tool: "ileapp", label: "Argument check", input: `${ev}/Galaxy-S21.E01`, kind: "file", type: "raw", created: "2026-09-19T11:05:00Z", minutes: 1, outcome: "fail-argparse" }),
    ],
    acqs: [clone(fx.AcquisitionRecord), seedAcqLeftEncrypted(file)],
  });
  cases.set(HARBOR, {
    file: caseFile("Harbor Lights", {
      case_number: "2026-0198",
      description: "Marina burglary series",
      default_timezone: null,
      created_at: "2026-09-18T08:02:44Z",
      updated_at: "2026-09-18T08:02:44Z",
    }),
    runs: [],
    acqs: [],
  });
  cases.set(COLD, {
    file: caseFile("Cold Case 7", {
      case_number: "2019-0007",
      examiner: "R. Alvarez",
      description: "Re-examination of archived handsets",
      default_timezone: "America/New_York",
      created_at: "2026-08-02T15:30:00Z",
      updated_at: "2026-08-02T15:30:00Z",
    }),
    runs: [],
    acqs: [],
  });
}

/**
 * An acquisition whose encryption restore failed (for "Turn backup encryption off", D5).
 * @param {CaseFile} file
 * @returns {AcquisitionRecord}
 */
function seedAcqLeftEncrypted(file) {
  const rec = clone(fx.AcquisitionRecord);
  rec.acq_id = "20260923-101500Z-ios-51be07";
  rec.label = "Second handset";
  rec.created_at = "2026-09-23T10:15:00Z";
  rec.started_at = "2026-09-23T10:15:03Z";
  rec.ended_at = "2026-09-23T10:52:40Z";
  rec.duration_ms = 2260000;
  rec.case_snapshot = { case_id: file.case_id, name: file.name, case_number: file.case_number, examiner: file.examiner, agency: file.agency };
  rec.encryption.restored_after = "failed";
  rec.encryption.will_encrypt_after_restore = true;
  rec.warnings = [
    { code: "encryption_restore_failed", message: "Turning backup encryption off failed" },
    { code: "encryption_left_enabled", message: "Backup encryption was enabled by the examiner and not confirmed disabled" },
  ];
  return rec;
}

/** @type {Record<ToolId, Map<string, string[]>>} */
const profiles = {
  ileapp: new Map([
    [fx.ProfileInfo.name, [...fx.ProfileInfo.modules]],
    ["Quick triage", MODULES.ileapp.filter((_, i) => i % 97 === 0).map((m) => m.name)],
  ]),
  aleapp: new Map([["Android triage", MODULES.aleapp.filter((_, i) => i % 83 === 0).map((m) => m.name)]]),
};

/**
 * A device as seen before pairing (`ideviceinfo -s`: no name or serial; no encryption or disk data).
 * @param {string} udid
 * @param {string} productType
 * @param {string} version
 * @param {import("../ui/types").PairState} state
 * @param {string | null} message
 * @returns {DeviceSummary}
 */
function unpairedDevice(udid, productType, version, state, message) {
  return {
    udid,
    device_name: null,
    product_type: productType,
    product_version: version,
    serial_number: null,
    pair_state: state,
    busy: false,
    will_encrypt: null,
    data_used_bytes: null,
    data_capacity_bytes: null,
    message,
  };
}

const IPAD = unpairedDevice("4e1c2b3a5d6f708192a3b4c5d6e7f8091a2b3c4d", "iPad13,4", "17.5.1", "not_paired", null);

/** @type {DeviceSummary[]} */
const devices = has("no_devices")
  ? []
  : has("pair_states")
    ? [
        clone(fx.DeviceSummary),
        IPAD,
        unpairedDevice("00008110-001A2B3C4D5E6F70", "iPhone14,5", "18.5", "awaiting_trust", "ERROR: Please accept the trust dialog on the screen of device 00008110-001A2B3C4D5E6F70, then attempt to pair again."),
        unpairedDevice("00008120-000C1D2E3F405162", "iPhone15,2", "18.6", "locked", "ERROR: Could not validate with device 00008120-000C1D2E3F405162 because a passcode is set. Please enter the passcode on the device and retry."),
        unpairedDevice("00008030-0019283746AB5C6D", "iPhone12,1", "17.7", "trust_denied", "ERROR: Device 00008030-0019283746AB5C6D said that the user denied the trust dialog."),
        unpairedDevice("00008101-0005A4B3C2D1E0F9", "iPhone13,4", "18.6", "pairing_failed", "ERROR: Pairing with device 00008101-0005A4B3C2D1E0F9 failed."),
        unpairedDevice("00008027-001122334455AABB", "iPad8,9", "16.7.10", "unknown", "idevicepair hostid did not answer within 20 s"),
        {
          ...clone(fx.DeviceSummary),
          udid: "00008140-00AB12CD34EF5601",
          device_name: "Loaner iPhone",
          product_type: "iPhone16,1",
          serial_number: "G7KXXXXXXX",
          will_encrypt: null,
          message: "WillEncrypt could not be read",
        },
      ]
    : [clone(fx.DeviceSummary), IPAD];
/** @type {Map<string, number>} */
const pairAttempts = new Map();

// ---- Jobs (one active, app-wide) ----

/**
 * @typedef {object} RunJob
 * @property {"run"} kind
 * @property {string} casePath
 * @property {RunRecord} record
 * @property {RunPhase} phase
 * @property {((event: RunEvent) => void) | null} subscriber
 * @property {string[]} lines
 * @property {boolean} cancelRequested
 */

/**
 * @typedef {object} AcqJob
 * @property {"acquisition"} kind
 * @property {string} casePath
 * @property {AcquisitionRecord} record
 * @property {AcqPhase} phase
 * @property {((event: AcqEvent) => void) | null} subscriber
 * @property {string[]} lines
 * @property {boolean} cancelRequested
 */

/** @type {RunJob | AcqJob | null} */
let job = null;

/**
 * @param {RunJob | AcqJob} j
 * @param {RunEvent | AcqEvent} event
 */
function emit(j, event) {
  const fn = /** @type {((e: RunEvent | AcqEvent) => void) | null} */ (j.subscriber);
  if (!fn) return;
  try {
    fn(clone(event));
  } catch (err) {
    console.error("mock: event subscriber threw", err);
  }
}

/**
 * @param {RunJob | AcqJob} j
 * @param {string[]} lines
 */
function emitLog(j, lines) {
  j.lines.push(...lines);
  if (j.lines.length > 2000) j.lines.splice(0, j.lines.length - 2000);
  emit(j, { type: "log", lines });
}

/**
 * @param {RunJob} j
 * @param {RunPhase} phase
 */
function runPhase(j, phase) {
  j.phase = phase;
  emit(j, { type: "phase", phase });
}

/**
 * @param {AcqJob} j
 * @param {AcqPhase} phase
 */
function acqPhase(j, phase) {
  j.phase = phase;
  emit(j, { type: "phase", phase });
}

/** @returns {ActiveJob | null} */
function activeJob() {
  if (!job) return null;
  if (job.kind === "run") {
    const r = job.record;
    return { kind: "run", case_path: job.casePath, run_id: r.run_id, tool: r.tool.id, created_at: r.created_at, phase: job.phase };
  }
  const a = job.record;
  return { kind: "acquisition", case_path: job.casePath, acq_id: a.acq_id, udid: a.device.udid, created_at: a.created_at, phase: job.phase };
}

// ---- Lookups and validation shared by commands ----

/**
 * A known case folder: in `recent_cases` and with a readable case.json (ARCHITECTURE.md §8).
 * @param {string} path
 * @returns {MockCase}
 */
function knownCase(path) {
  const c = cases.get(path);
  if (!c || !settings.recent_cases.includes(path)) {
    throw appError("case_not_found", "This case folder is not a known case.", path);
  }
  return c;
}

/**
 * @param {ToolId} tool
 * @returns {ToolStatus}
 */
function usableTool(tool) {
  const status = tools[tool];
  if (!status) throw appError("internal", `Unknown tool: ${tool}`);
  if (status.state === "unsupported_platform") {
    throw appError("unsupported_platform", `${status.display_name} has no build for this platform.`);
  }
  if (status.state === "not_installed") {
    throw appError("tool_not_installed", `${status.display_name} ${status.pinned_version} is not installed.`, `${LEAPP_DIR}/${tool}/${status.pinned_version}/install.json does not exist`);
  }
  if (status.state === "verification_failed") {
    throw appError("tool_verification_failed", `${status.display_name} failed verification.`, status.problem);
  }
  return status;
}

/** @param {string} name */
function checkName(name) {
  const n = name.trim().length;
  return n >= 1 && n <= 120;
}

/**
 * @param {string} name
 * @returns {string} the folder name derived from a case name (CONTRACTS.md §6)
 */
function folderName(name) {
  let out = name.replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").replace(/[. ]+$/, "");
  if (/^(con|prn|aux|nul|com\d|lpt\d)$/i.test(out)) out += "_";
  return out || "_";
}

/**
 * @param {string} path
 * @param {MockCase} c
 * @param {string[]} [recovered]
 * @returns {CaseDetail}
 */
function detail(path, c, recovered = []) {
  return clone({
    path,
    case: c.file,
    runs: c.runs.map((r) => runSummary(path, r)).sort((a, b) => b.created_at.localeCompare(a.created_at)),
    acquisitions: c.acqs.map((a) => acqSummary(path, a, restoredLater(a.acq_id))).sort((a, b) => b.created_at.localeCompare(a.created_at)),
    recovered,
  });
}

/** @param {string} path */
function touchRecent(path) {
  settings.recent_cases = [path, ...settings.recent_cases.filter((p) => p !== path)].slice(0, 50);
}

/** @param {CaseFields} fields */
function validateFields(fields) {
  if (!checkName(fields.name)) {
    throw appError("invalid_case", "The case name must be 1–120 characters.");
  }
  if (fields.default_timezone !== null && !TIMEZONES.includes(fields.default_timezone)) {
    throw appError("invalid_timezone", `Unknown timezone: ${fields.default_timezone}`);
  }
}

const SIZES = /** @type {Record<string, number>} */ ({
  "iPhone-12-FFS.zip": 13314398617,
  "Pixel-7.tar": 8804682752,
  "Galaxy-S21.E01": 68719476736,
});

/**
 * Input inspection (ARCHITECTURE.md §6 step 1 overlap rule; type detection by name).
 * @param {ToolId} tool
 * @param {string} path
 * @param {string} casePath
 * @returns {InputInspection}
 */
function inspect(tool, path, casePath) {
  if (!/^(\/|[A-Za-z]:[\\/])/.test(path)) throw appError("invalid_input", "The input path must be absolute.", path);
  const stem = stemOf(path);
  if (stem === "denied") {
    throw appError("permission_denied", "The input cannot be read: permission denied.", `open ${path}: Operation not permitted (os error 1)`);
  }
  if (stem.startsWith("missing")) throw appError("invalid_input", "The input does not exist.", path);
  const inRuns = [...cases.keys()].some((c) => within(path, `${c}/runs`));
  if (within(casePath, path) || inRuns || within(path, APP_DATA)) {
    throw appError(
      "input_overlaps_case",
      "This input overlaps a case: the run's output would land inside it, or it lies inside a case's runs/ folder.",
      `input: ${path}\ncase: ${casePath}`,
    );
  }
  const sample = SAMPLES.find((s) => s.path === path);
  const isFile = sample ? sample.kind === "file" : extOf(path) !== "";
  const toolTypes = TOOLS[tool].input_types;
  if (!isFile) {
    const itunes = /backup/i.test(stem) || /^[0-9a-f]{8}-[0-9a-f]{12,16}$/i.test(stem) || /^[0-9a-f]{40}$/i.test(stem);
    // An acquired backup is encrypted when the acquisition turned encryption on, or it was on before.
    const acqId = acquisitionIdOf(casePath, path);
    const acq = acqId ? cases.get(casePath)?.acqs.find((a) => a.acq_id === acqId) : undefined;
    const acqEncrypted = acq ? acq.encryption.enabled_by_examiner || acq.encryption.will_encrypt_before === true : false;
    const encrypted = itunes && (path === fx.InputInspection.path || /encrypted/i.test(stem) || acqEncrypted);
    // `…encryption-unknown`: Manifest.plist has no IsEncrypted (the core reports null and a warning).
    const unknown = itunes && /encryption-unknown$/i.test(stem);
    const canItunes = itunes && toolTypes.includes("itunes");
    /** @type {string[]} */
    const warnings = [];
    if (unknown) warnings.push("Backup encryption is unknown: Manifest.plist has no IsEncrypted");
    if (itunes && !canItunes) warnings.push(`This looks like an iTunes backup; ${TOOLS[tool].display_name} parses it as a plain folder.`);
    return {
      path,
      kind: "directory",
      size_bytes: null,
      detected_type: canItunes ? "itunes" : "fs",
      allowed_types: canItunes ? ["fs", "itunes"] : ["fs"],
      is_itunes_backup: itunes,
      itunes_encrypted: itunes && !unknown ? encrypted : null,
      hashable: false,
      warnings,
    };
  }
  /** @type {Record<string, InputType>} */
  const byExt = { zip: "zip", tar: "tar", gz: "gz", tgz: "gz", e01: "raw", img: "raw", dd: "raw", bin: "raw", "001": "raw" };
  const detected = byExt[extOf(path)] ?? null;
  return {
    path,
    kind: "file",
    size_bytes: SIZES[baseName(path)] ?? 1048576,
    detected_type: detected && toolTypes.includes(detected) ? detected : null,
    allowed_types: toolTypes.filter((t) => t !== "fs" && t !== "itunes"),
    is_itunes_backup: false,
    itunes_encrypted: null,
    hashable: true,
    warnings: [],
  };
}

/**
 * @param {ToolId} tool
 * @param {ModuleSelection} sel
 * @returns {string[]} the resolved module names
 */
function resolveModules(tool, sel) {
  const known = new Set(MODULES[tool].map((m) => m.name));
  if (sel.mode === "all") return [...known].sort();
  /** @type {string[]} */
  let names;
  if (sel.mode === "profile") {
    const stored = profiles[tool].get(sel.profile_name);
    if (!stored) throw appError("profile_not_found", `No profile named "${sel.profile_name}".`);
    names = stored;
  } else {
    names = sel.modules;
  }
  const unknown = names.filter((n) => !known.has(n));
  if (unknown.length) {
    throw appError("unknown_modules", `${unknown.length} unknown module(s); remove them before starting.`, unknown.join("\n"));
  }
  return [...names];
}

/**
 * @param {ToolId} tool
 * @param {string} name
 * @returns {ProfileInfo}
 */
function profileInfo(tool, name) {
  const modules = profiles[tool].get(name) ?? [];
  const known = new Set(MODULES[tool].map((m) => m.name));
  return { tool, name, modules: [...modules], unknown_modules: modules.filter((m) => !known.has(m)) };
}

// ---- §10: app, settings, tools ----

/** @type {Api["app_info"]} */
export const app_info = async () => {
  const info = { ...clone(fx.AppInfo), dev_override: has("dev_override") };
  if (has("windows")) Object.assign(info, { platform: "windows-x86_64", os: "windows", arch: "x86_64" });
  // `paths.tools_dir` is the folder in effect: the override, else the default.
  info.paths.tools_dir = settings.tools_dir ?? LEAPP_DIR;
  return info;
};

/** @type {Api["licenses_get"]} */
export const licenses_get = async () =>
  [
    "# Third-party notices (mock)",
    "",
    "The real app returns the embedded THIRD-PARTY-NOTICES.md here.",
    "",
    "## iLEAPP and aLEAPP",
    "",
    "MIT License",
    "",
    "Copyright (c) Alexis Brignoni and contributors",
    "",
    "Permission is hereby granted, free of charge, to any person obtaining a copy",
    "of this software and associated documentation files (the \"Software\"), to deal",
    "in the Software without restriction, including without limitation the rights",
    "to use, copy, modify, merge, publish, distribute, sublicense, and/or sell",
    "copies of the Software, and to permit persons to whom the Software is",
    "furnished to do so, subject to the following conditions: …",
    "",
    "## Rust crates",
    "",
    "| Crate      | Version | License           |",
    "|------------|---------|-------------------|",
    "| serde      | 1.0.228 | MIT OR Apache-2.0 |",
    "| sha2       | 0.10.9  | MIT OR Apache-2.0 |",
    "| tauri      | 2.11.0  | Apache-2.0 OR MIT |",
  ].join("\n");

/** @type {Api["settings_get"]} */
export const settings_get = async () => clone(settings);

/** @type {Api["settings_update"]} */
export const settings_update = async (req) => {
  if ("cases_root" in req) {
    if (typeof req.cases_root !== "string") throw appError("invalid_input", "cases_root may not be null.");
    settings.cases_root = req.cases_root;
  }
  if ("defaults" in req) {
    if (!req.defaults) throw appError("invalid_input", "defaults may not be null.");
    settings.defaults = clone(req.defaults);
  }
  if ("tools_dir" in req) {
    const dir = req.tools_dir ?? null;
    if (dir !== null && settings.recent_cases.some((c) => within(dir, c))) {
      throw appError("path_not_allowed", "The tools folder cannot be inside a case folder.", dir);
    }
    settings.tools_dir = dir;
  }
  return clone(settings);
};

/** @type {Api["tools_status"]} */
export const tools_status = async () => clone([tools.ileapp, tools.aleapp]);

/** @type {Api["tool_verify"]} */
export const tool_verify = async (req) => {
  const status = tools[req.tool];
  if (status.state === "not_installed" || status.state === "unsupported_platform") {
    throw appError("tool_not_installed", `${status.display_name} is not installed.`);
  }
  if (status.state === "installed_unverified") status.state = "verified";
  return clone(status);
};

/**
 * The install pipeline (CONTRACTS.md §10 `tool_install`), with events.
 * @param {ToolId} tool
 * @param {(event: InstallEvent) => void} onEvent
 * @param {{ download: boolean, fail: ErrorCode | null }} how
 */
async function installPipeline(tool, onEvent, how) {
  const status = tools[tool];
  if (status.state === "unsupported_platform") throw appError("unsupported_platform", `${status.display_name} has no build for this platform.`);
  const t = tick();
  /** @param {InstallEvent} e */
  const send = (e) => onEvent(clone(e));
  const total = 55085487;
  if (how.download) {
    send({ type: "stage", stage: "downloading" });
    for (let i = 1; i <= 5; i++) {
      await sleep(t);
      send({ type: "download_progress", bytes_done: Math.round((total * i) / 5), bytes_total: total });
    }
  }
  send({ type: "stage", stage: "verifying" });
  await sleep(t);
  if (how.fail) {
    throw appError(how.fail, "The asset's SHA-256 does not match the pinned value.", `expected d99f2d05…0386, got ${randomHex(8)}…`);
  }
  for (const stage of /** @type {const} */ (["extracting", "hashing", "introspecting"])) {
    send({ type: "stage", stage });
    await sleep(t);
  }
  send({ type: "message", text: `Found ${MODULES[tool].length} modules` });
  send({ type: "stage", stage: "done" });
  tools[tool] = { ...installedStatus(tool), state: "verified", install_source: how.download ? "download" : "offline_import" };
  return clone(tools[tool]);
}

/** @type {Api["tool_install"]} */
export const tool_install = (req, onEvent) =>
  installPipeline(req.tool, onEvent, { download: true, fail: has("install_fail") ? "hash_mismatch" : null });

/** @type {Api["tool_import"]} */
export const tool_import = (req, onEvent) =>
  installPipeline(req.tool, onEvent, { download: false, fail: stemOf(req.archive_path) === "corrupt" ? "hash_mismatch" : null });

/** @type {Api["tool_modules"]} */
export const tool_modules = async (req) => {
  const status = usableTool(req.tool);
  return clone({
    tool: req.tool,
    version: status.installed_version ?? status.pinned_version,
    always_run: ALWAYS_RUN[req.tool],
    timezones: req.tool === "ileapp" ? TIMEZONES : null,
    modules: MODULES[req.tool],
  });
};

// ---- §10: cases and runs ----

/** @type {Api["cases_list"]} */
export const cases_list = async () =>
  clone(
    settings.recent_cases.map((path) => {
      const c = cases.get(path);
      const runs = c?.runs ?? [];
      const last = runs.map((r) => r.created_at).sort().pop() ?? null;
      return { path, exists: c !== undefined, case: c ? c.file : null, run_count: runs.length, last_run_at: last };
    }),
  );

/** @type {Api["case_create"]} */
export const case_create = async (req) => {
  validateFields(req);
  const parent = req.parent_dir ?? settings.cases_root;
  if (parent === "/Volumes/ReadOnly") {
    throw appError("permission_denied", "The parent folder is not writable.", `mkdir ${parent}/…: Permission denied (os error 13)`);
  }
  if (within(parent, LEAPP_DIR) || within(parent, APP_DATA)) {
    throw appError("path_not_allowed", "Cases cannot be created inside the app or tools folders.", parent);
  }
  const base = `${parent.replace(/[\\/]+$/, "")}/${folderName(req.name.trim())}`;
  let path = base;
  for (let n = 2; cases.has(path); n++) path = `${base} (${n})`;
  const now = isoNow();
  /** @type {MockCase} */
  const c = {
    file: {
      schema_version: 1,
      case_id: randomHex(32),
      name: req.name.trim(),
      case_number: req.case_number,
      examiner: req.examiner,
      agency: req.agency,
      description: req.description,
      default_timezone: req.default_timezone,
      created_at: now,
      updated_at: now,
      created_by_app_version: fx.AppInfo.app_version,
    },
    runs: [],
    acqs: [],
  };
  cases.set(path, c);
  touchRecent(path);
  return detail(path, c);
};

/** @type {Api["case_open"]} */
export const case_open = async (req) => {
  const c = cases.get(req.path);
  if (!c) {
    if (req.path === MISSING || /missing/i.test(req.path)) {
      throw appError("case_not_found", "The case folder does not exist.", `${req.path}: No such file or directory (os error 2)`);
    }
    throw appError("invalid_case", "This folder has no valid case.json.", `${req.path}/case.json: No such file or directory (os error 2)`);
  }
  touchRecent(req.path);
  const now = isoNow();
  /** @type {string[]} */
  const recovered = [];
  for (const run of c.runs) {
    const isActive = job?.kind === "run" && job.record === run;
    if (run.status === "running" && !isActive) {
      interruptRunRecord(run, now);
      recovered.push(run.run_id);
    }
  }
  for (const acq of c.acqs) {
    const isActive = job?.kind === "acquisition" && job.record === acq;
    if (acq.status === "running" && !isActive) {
      interruptAcqRecord(acq, now);
      recovered.push(acq.acq_id);
    }
  }
  return detail(req.path, c, recovered);
};

/** @type {Api["case_update"]} */
export const case_update = async (req) => {
  const c = knownCase(req.path);
  validateFields(req.fields);
  c.file = { ...c.file, ...clone(req.fields), name: req.fields.name.trim(), updated_at: isoNow() };
  return detail(req.path, c);
};

/** @type {Api["case_forget"]} */
export const case_forget = async (req) => {
  if (!settings.recent_cases.includes(req.path)) throw appError("case_not_found", "This case is not in the recent list.", req.path);
  settings.recent_cases = settings.recent_cases.filter((p) => p !== req.path);
};

/**
 * @param {string} casePath
 * @param {string} runId
 * @returns {RunRecord}
 */
function findRun(casePath, runId) {
  const run = knownCase(casePath).runs.find((r) => r.run_id === runId);
  if (!run) throw appError("run_not_found", `No run ${runId} in this case.`);
  return run;
}

/** @type {Api["run_get"]} */
export const run_get = async (req) => clone(findRun(req.case_path, req.run_id));

/** @type {Api["input_inspect"]} */
export const input_inspect = async (req) => {
  knownCase(req.case_path);
  await sleep(Math.min(tick(), 150));
  return inspect(req.tool, req.path, req.case_path);
};

/** @type {Api["ios_backups_find"]} */
export const ios_backups_find = async () => [clone(fx.IosBackup)];

// ---- §10: profiles ----

/** @type {Api["profiles_list"]} */
export const profiles_list = async (req) =>
  [...profiles[req.tool].keys()].sort((a, b) => a.localeCompare(b)).map((name) => profileInfo(req.tool, name));

/** @param {string} name */
function checkProfileName(name) {
  if (name.trim().length < 1 || name.trim().length > 80) throw appError("invalid_input", "A profile name must be 1–80 characters.");
}

/** @type {Api["profile_save"]} */
export const profile_save = async (req) => {
  checkProfileName(req.name);
  const known = new Set(MODULES[req.tool].map((m) => m.name));
  const unknown = req.modules.filter((m) => !known.has(m));
  if (unknown.length) throw appError("unknown_modules", "A profile cannot be saved with unknown modules.", unknown.join("\n"));
  profiles[req.tool].set(req.name.trim(), [...req.modules]);
  return profileInfo(req.tool, req.name.trim());
};

/** @type {Api["profile_delete"]} */
export const profile_delete = async (req) => {
  if (!profiles[req.tool].delete(req.name)) throw appError("profile_not_found", `No profile named "${req.name}".`);
};

/** @type {Api["profile_import"]} */
export const profile_import = async (req) => {
  const ext = extOf(req.path);
  const stem = stemOf(req.path);
  if (ext !== TOOLS[req.tool].profile_ext || stem === "broken") {
    throw appError(
      "profile_invalid",
      `This is not a valid ${TOOLS[req.tool].display_name} profile.`,
      ext !== TOOLS[req.tool].profile_ext ? `expected "leapp": "${req.tool}"` : "format_version must be 1",
    );
  }
  const name = (req.name ?? stem).trim();
  checkProfileName(name);
  if (profiles[req.tool].has(name) && !req.overwrite) {
    throw appError("profile_exists", `A profile named "${name}" already exists.`);
  }
  const modules = MODULES[req.tool].filter((_, i) => i % 131 === 0).map((m) => m.name);
  if (stem === "Triage") modules.push("legacyNotesParser", "removedArtifact");
  profiles[req.tool].set(name, modules);
  return profileInfo(req.tool, name);
};

/** @type {Api["profile_export"]} */
export const profile_export = async (req) => {
  if (!profiles[req.tool].has(req.name)) throw appError("profile_not_found", `No profile named "${req.name}".`);
  if ([...cases.keys()].some((c) => within(req.dest_path, `${c}/runs`))) {
    throw appError("path_not_allowed", "Profiles cannot be exported into a case's runs/ folder.", req.dest_path);
  }
};

// ---- §10: runs ----

/** @type {Api["run_start"]} */
export const run_start = async (req, onEvent) => {
  if (job) throw appError("run_already_active", "Another job is running. Wait for it to finish or cancel it.");
  const c = knownCase(req.case_path);
  usableTool(req.tool);
  const insp = inspect(req.tool, req.input_path, req.case_path);
  if (!insp.allowed_types.includes(req.input_type)) {
    throw appError("input_type_not_allowed", `Type "${req.input_type}" is not allowed for this input.`, `allowed: ${insp.allowed_types.join(", ")}`);
  }
  const resolved = resolveModules(req.tool, req.modules);
  if (req.tool === "ileapp") {
    // As the core: an iTunes read needs the password when the backup is encrypted or its
    // encryption could not be read.
    if (req.input_type === "itunes" && !req.itunes_password) {
      if (insp.itunes_encrypted === true) throw appError("password_required", "This iTunes backup is encrypted: enter its backup password");
      if (insp.is_itunes_backup && insp.itunes_encrypted === null) {
        throw appError(
          "password_required",
          "This iTunes backup's encryption state could not be read, so a password is needed: enter its backup password",
        );
      }
    }
    if (!req.timezone || !TIMEZONES.includes(req.timezone)) throw appError("invalid_timezone", `Unknown timezone: ${req.timezone}`);
  }
  const createdAt = isoNow();
  const runId = newJobId(req.tool);
  const record = initialRunRecord({
    casePath: req.case_path,
    caseFile: c.file,
    runId,
    tool: req.tool,
    label: req.label,
    inputPath: req.input_path,
    inputKind: insp.kind,
    inputType: req.input_type,
    typeDetected: insp.detected_type,
    sizeBytes: insp.size_bytes,
    itunesEncrypted: insp.itunes_encrypted,
    createdAt,
    timezone: req.timezone,
    passwordSupplied: req.tool === "ileapp" && !!req.itunes_password,
    keychainPath: req.keychain_path,
    hashInput: req.hash_input && insp.kind === "file",
    modules: req.modules,
    resolved,
    alwaysRun: ALWAYS_RUN[req.tool][req.input_type] ?? ALWAYS_RUN[req.tool].default,
    availableCount: MODULES[req.tool].length,
  });
  c.runs.push(record);
  /** @type {RunJob} */
  const j = { kind: "run", casePath: req.case_path, record, phase: "preparing", subscriber: onEvent, lines: [], cancelRequested: false };
  job = j;
  setTimeout(() => void simulateRun(j, stemOf(req.input_path), resolved), 0);
  return { run_id: runId, run_dir: `${req.case_path}/runs/${runId}` };
};

/**
 * Streams a run: phases, log batches, hash/seal progress, stdio tails, `finished`.
 * @param {RunJob} j
 * @param {string} scenario
 * @param {string[]} resolved
 */
async function simulateRun(j, scenario, resolved) {
  const t = tick();
  const rec = j.record;
  const tool = TOOLS[rec.tool.id];
  runPhase(j, "preparing");
  await sleep(t);
  await holdIn(j, "preparing");
  rec.started_at = isoNow();
  runPhase(j, "running");
  emitLog(j, [
    `${tool.display_name}: ${rec.tool.version} started`,
    `Processing started. Please wait. This may take a few minutes...`,
    `File/Directory selected: ${rec.input.path}`,
    `Artifact categories to parse: ${resolved.length}`,
  ]);
  if (scenario === "flood") {
    for (let n = 0; n < FLOOD_LINES && !j.cancelRequested; n += FLOOD_BATCH) {
      const lines = [];
      for (let k = n; k < n + FLOOD_BATCH; k++) {
        const name = resolved[k % Math.max(resolved.length, 1)] ?? "last_build";
        lines.push(`[${String(k + 1).padStart(6, "0")}] ${name}: parsed ${k % 97} records from ${rec.input.path}/private/var/mobile/Library/${name}.db`);
      }
      emitLog(j, lines);
      await sleep(5);
    }
  }
  const hashing = rec.input.hash.status === "pending";
  const bytesTotal = rec.input.size_bytes ?? 1;
  const batches = scenario === "slow" ? Number.POSITIVE_INFINITY : scenario === "interrupt" ? 5 : 14;
  for (let i = 0; i < batches; i++) {
    await sleep(t);
    if (j.cancelRequested) break;
    const lines = [];
    for (let k = 0; k < 3; k++) {
      const name = resolved[(i * 3 + k) % Math.max(resolved.length, 1)] ?? "last_build";
      lines.push(`${name} artifact started`, `${name} artifact completed`);
    }
    emitLog(j, lines);
    if (hashing) {
      emit(j, { type: "hash_progress", bytes_done: Math.min(bytesTotal, Math.round((bytesTotal * (i + 1)) / 16)), bytes_total: bytesTotal });
    }
  }
  if (scenario === "interrupt" && !j.cancelRequested) {
    // The "app crashes": no finished event; the record stays `running` until the next case_open.
    job = null;
    return;
  }
  const cancelled = j.cancelRequested;
  const outcome = cancelled ? RUN_OUTCOMES.cancelled : RUN_OUTCOMES[scenario] ?? RUN_OUTCOMES.success;
  const exitedAt = isoNow();
  if (outcome.report && !cancelled) emitLog(j, ["Processes completed.", "Report generation started."]);
  emit(j, {
    type: "stdio_tail",
    stream: "stdout",
    lines:
      outcome.exit_code === 2
        ? [`usage: ${rec.tool.id}.py [-h] [-t {${tool.input_types.join(",")}}] …`]
        : [`${tool.display_name}: iOS/Android Logs, Events, and Plists Parser`, `Objects will be written to ${rec.command.cwd}/report`, "Processes completed."],
  });
  emit(j, {
    type: "stdio_tail",
    stream: "stderr",
    lines: outcome.warnings.some((w) => w.code === "stderr_traceback")
      ? ["Traceback (most recent call last):", '  File "scripts/artifacts/safariHistory.py", line 88, in get_safari', "sqlite3.DatabaseError: database disk image is malformed"]
      : outcome.exit_code === 2
        ? [`${rec.tool.id}.py: error: argument -t: invalid choice`]
        : [],
  });
  if (hashing && !cancelled) {
    runPhase(j, "hashing_input");
    await sleep(t);
    if (has("hold_hashing_input")) {
      emit(j, { type: "hash_progress", bytes_done: Math.round(bytesTotal * 0.93), bytes_total: bytesTotal });
      await holdIn(j, "hashing_input");
    }
    emit(j, { type: "hash_progress", bytes_done: bytesTotal, bytes_total: bytesTotal });
  }
  runPhase(j, "analyzing");
  await sleep(t);
  await holdIn(j, "analyzing");
  if (outcome.report) {
    runPhase(j, "sealing_report");
    const files = outcome.index ? 5321 : 214;
    for (let i = 1; i <= 3; i++) {
      await sleep(t);
      emit(j, { type: "seal_progress", files_done: Math.round((files * i) / 3), files_total: files });
      if (i === 1) await holdIn(j, "sealing_report");
    }
  }
  runPhase(j, "finalizing");
  await sleep(t);
  await holdIn(j, "finalizing");
  finalizeRunRecord(rec, outcome, { startedAt: rec.started_at, exitedAt, endedAt: isoNow() });
  job = null;
  emit(j, { type: "finished", status: rec.status, reasons: rec.status_reasons, warnings: rec.warnings, summary: runSummary(j.casePath, rec) });
}

/** @type {Api["run_cancel"]} */
export const run_cancel = async (req) => {
  if (job?.kind !== "run" || job.record.run_id !== req.run_id) throw appError("run_not_found", `Run ${req.run_id} is not active.`);
  job.cancelRequested = true;
};

/** @type {Api["job_active"]} */
export const job_active = async () => clone(activeJob());

/** @type {Api["job_attach"]} */
export const job_attach = async (req, onEvent) => {
  if (req.kind === "run") {
    if (job?.kind !== "run" || job.record.run_id !== req.id) throw appError("run_not_found", `Run ${req.id} is not active.`);
    job.subscriber = onEvent;
  } else {
    if (job?.kind !== "acquisition" || job.record.acq_id !== req.id) throw appError("acq_not_found", `Acquisition ${req.id} is not active.`);
    job.subscriber = onEvent;
  }
  return { backlog: [...job.lines] };
};

/** @type {Api["open_report"]} */
export const open_report = async (req) => {
  const run = findRun(req.case_path, req.run_id);
  if (run.leapp_result?.index_html_found !== true) throw appError("report_missing", "This run has no report/index.html.");
  console.info("mock: open_report", req.run_id);
};

/** @type {Api["reveal_path"]} */
export const reveal_path = async (req) => {
  const allowed = settings.recent_cases.some((c) => within(req.path, c)) || within(req.path, APP_DATA);
  if (!allowed) throw appError("path_not_allowed", "Only paths inside a known case folder or the app folders can be revealed.", req.path);
  console.info("mock: reveal_path", req.path);
};

/** @type {Api["open_text_file"]} */
export const open_text_file = async (req) => {
  const run = findRun(req.case_path, req.run_id);
  if (req.which === "report_manifest" && run.output.seal.status !== "sealed") {
    throw appError("report_missing", "This run has no report.sha256.");
  }
  console.info("mock: open_text_file", req.run_id, req.which);
};

/** @type {Api["temp_cleanup"]} */
export const temp_cleanup = async () => {
  if (job) throw appError("run_already_active", "Temp files cannot be cleaned while a job is running.");
  return clone(fx.TempCleanupResult);
};

// ---- §13.5: devices and acquisitions ----

/** @returns {import("../ui/types").DevicesResult["tools"]} */
function ideviceTools() {
  if (has("idevice_missing")) {
    return { source: null, version: null, state: "missing", guidance: "idevice_id was not found next to the app executable." };
  }
  if (has("usbmuxd_unavailable")) {
    return { source: "bundled", version: "1.4.0", state: "usbmuxd_unavailable", guidance: "idevice_id -l: ERROR: Unable to retrieve device list!" };
  }
  if (has("idevice_verification_failed")) {
    return { source: "bundled", version: "1.4.0", state: "verification_failed", guidance: "idevicebackup2: SHA-256 7c1e9a04…b25d does not match the pinned 945993e3…a2d2." };
  }
  if (has("idevice_unsupported")) {
    return { source: null, version: null, state: "unsupported_platform", guidance: "No iOS tools are pinned for windows-aarch64." };
  }
  return clone(fx.DevicesResult.tools);
}

/** @type {Api["devices_list"]} */
export const devices_list = async () => {
  const toolsState = ideviceTools();
  if (toolsState.state !== "ok") return { tools: toolsState, devices: [] };
  if (has("idevice_session_error")) return { tools: { ...toolsState, guidance: "Listing the devices failed unexpectedly." }, devices: [] };
  const busyUdid = job?.kind === "acquisition" ? job.record.device.udid : null;
  return {
    tools: toolsState,
    devices: devices.map((d) =>
      d.udid === busyUdid ? { ...clone(d), busy: true } : clone(d),
    ),
  };
};

/**
 * @param {string} udid
 * @returns {DeviceSummary}
 */
function findDevice(udid) {
  const device = devices.find((d) => d.udid === udid);
  if (!device) throw appError("device_not_found", "The device is not connected.", udid);
  return device;
}

/** @type {Api["device_pair"]} */
export const device_pair = async (req) => {
  const device = findDevice(req.udid);
  if (job?.kind === "acquisition" && job.record.device.udid === req.udid) throw appError("device_busy", "The device is in use by the current acquisition.");
  if (device.pair_state === "paired") throw appError("already_paired", "The device is already paired.");
  await sleep(tick());
  const attempt = (pairAttempts.get(req.udid) ?? 0) + 1;
  pairAttempts.set(req.udid, attempt);
  // A device that was never asked shows the Trust dialog first; a retry (the examiner tapped Trust,
  // unlocked it or reconnected it) pairs. Until then the host has no pair record, so the device
  // stays `not_paired` in `devices_list` (ARCHITECTURE.md §6b step 1); only this answer says more.
  if (attempt === 1 && device.pair_state === "not_paired") {
    return {
      ...clone(device),
      pair_state: "awaiting_trust",
      message: `ERROR: Please accept the trust dialog on the screen of device ${req.udid}, then attempt to pair again.`,
    };
  }
  const ipad = device.product_type?.startsWith("iPad") ?? false;
  Object.assign(device, {
    pair_state: "paired",
    message: null,
    device_name: ipad ? "Evidence iPad" : "Evidence iPhone",
    serial_number: ipad ? "DMPXXXXXXXXX" : "FFMXXXXXXXXX",
    // The iPad's owner turned backup encryption on (the "already encrypted" variant).
    will_encrypt: ipad,
    data_used_bytes: 42949672960,
    data_capacity_bytes: 128849018880,
  });
  return clone(device);
};

/** @type {Api["acq_preflight"]} */
export const acq_preflight = async (req) => {
  knownCase(req.case_path);
  const device = findDevice(req.udid);
  if (device.pair_state !== "paired") throw appError("device_not_paired", "Pair the device first.");
  const required = device.data_used_bytes;
  if (has("preflight_block")) return { free_bytes: 21474836480, required_bytes: required, level: "block" };
  if (has("preflight_warn")) return { free_bytes: 64424509440, required_bytes: required, level: "warn" };
  return { ...clone(fx.AcqPreflight), required_bytes: required };
};

/** @type {Api["acq_start"]} */
export const acq_start = async (req, onEvent) => {
  if (job) throw appError("run_already_active", "Another job is running. Wait for it to finish or cancel it.");
  const c = knownCase(req.case_path);
  const toolsState = ideviceTools();
  if (toolsState.state === "missing" || toolsState.state === "unsupported_platform") throw appError("idevice_tools_missing", "The libimobiledevice tools are not available.", toolsState.guidance);
  if (toolsState.state === "usbmuxd_unavailable") throw appError("usbmuxd_unavailable", "The device service (usbmuxd) is not available.", toolsState.guidance);
  if (toolsState.state === "verification_failed") throw appError("idevice_tools_verification_failed", "A bundled tool failed verification.", toolsState.guidance);
  const device = findDevice(req.udid);
  if (device.pair_state !== "paired") throw appError("device_not_paired", "Pair the device first.");
  if (has("preflight_block")) throw appError("insufficient_space", "Not enough free space in the case folder for this backup.");
  if (req.enable_encryption) {
    if (device.will_encrypt) throw appError("encryption_already_on", "Backup encryption is already on for this device.");
    if (!req.encryption_password || req.encryption_password.length < 4) {
      throw appError("encryption_password_required", "Enter a backup password of at least 4 characters.");
    }
  }
  const createdAt = isoNow();
  const acqId = newJobId("ios");
  const record = initialAcqRecord({
    casePath: req.case_path,
    caseFile: c.file,
    acqId,
    label: req.label,
    device,
    pairedByApp: (pairAttempts.get(req.udid) ?? 0) > 0,
    enableEncryption: req.enable_encryption,
    restoreEncryption: req.restore_encryption,
    createdAt,
  });
  c.acqs.push(record);
  /** @type {AcqJob} */
  const j = { kind: "acquisition", casePath: req.case_path, record, phase: "preparing", subscriber: onEvent, lines: [], cancelRequested: false };
  job = j;
  const scenario = /\/([a-z_]+)$/.exec(req.label ?? "")?.[1] ?? "success";
  setTimeout(() => void simulateAcq(j, scenario, req), 0);
  return { acq_id: acqId, acq_dir: `${req.case_path}/acquisitions/${acqId}` };
};

/**
 * @param {string} code
 * @param {string} message
 * @returns {Reason}
 */
const r = (code, message) => ({ code, message });

/** @type {Record<string, Reason[]>} */
const ACQ_FAILURES = {
  backup_fail: [r("nonzero_exit", "idevicebackup2 exited with code 151"), r("success_message_missing", "“Backup Successful.” was not printed"), r("snapshot_not_finished", "SnapshotState is not “finished”")],
  incomplete: [r("success_message_missing", "“Backup Successful.” was not printed"), r("snapshot_not_finished", "SnapshotState is “new”")],
  cancel_on_device: [r("cancelled_on_device", "The backup was cancelled on the device"), r("success_message_missing", "“Backup Successful.” was not printed"), r("snapshot_not_finished", "SnapshotState is not “finished”")],
  disconnect: [r("device_disconnected", "The device was disconnected during the backup"), r("success_message_missing", "“Backup Successful.” was not printed"), r("snapshot_not_finished", "SnapshotState is not “finished”")],
  sync_lock: [r("sync_lock_failed", "Could not lock the device's sync (is Finder or iTunes syncing it?)"), r("nonzero_exit", "idevicebackup2 exited with code 255"), r("success_message_missing", "“Backup Successful.” was not printed"), r("backup_dir_missing", "backup/<udid>/ is missing")],
};

/**
 * Streams an acquisition through the phases of ARCHITECTURE.md §6b.
 * @param {AcqJob} j
 * @param {string} scenario
 * @param {AcqRequest} req
 */
async function simulateAcq(j, scenario, req) {
  const t = tick();
  const rec = j.record;
  const tool = rec.tools.binaries.idevicebackup2.path;
  const udid = rec.device.udid;
  const device = devices.find((d) => d.udid === udid);
  acqPhase(j, "preparing");
  await sleep(t);
  await holdIn(j, "preparing");
  rec.started_at = isoNow();
  /** @type {Reason[]} */
  let reasons = [];
  /** @type {Reason[]} */
  const warnings = [];
  let enabled = false;
  let backupRan = false;
  let backupExited = false;
  let cancelledBeforeExit = false;

  if (req.enable_encryption) {
    acqPhase(j, "enabling_encryption");
    emit(j, { type: "device_prompt", kind: "passcode_for_encryption", text: "Please confirm enabling the backup encryption by entering the passcode on the device." });
    await holdIn(j, "enabling_encryption");
    await sleep(3 * t);
    const ok = scenario !== "enable_fail";
    rec.commands.push({ purpose: "enable_encryption", argv: [tool, "-u", udid, "encryption", "on"], exit_code: ok ? 0 : 1, started_at: rec.started_at, exited_at: isoNow() });
    rec.encryption.will_encrypt_after_enable = ok;
    if (ok) {
      enabled = true;
      if (device) device.will_encrypt = true;
      rec.encryption.enabled_by_examiner = true;
      rec.device_changes.push({ at: isoNow(), change: "backup_encryption_enabled", detail: "WillEncrypt false → true" });
    } else {
      reasons = [r("encryption_enable_failed", "Turning backup encryption on failed")];
    }
  }

  if (reasons.length === 0 && !j.cancelRequested) {
    acqPhase(j, "backing_up");
    backupRan = true;
    const started = isoNow();
    rec.device_changes.push({ at: started, change: "sync_lock_taken", detail: "idevicebackup2 holds /com.apple.itunes.lock_sync during backup" });
    rec.commands.push({ purpose: "backup", argv: [tool, "-u", udid, "backup", "--full", `${j.casePath}/acquisitions/${rec.acq_id}/backup`], exit_code: null, started_at: started, exited_at: null });
    emit(j, { type: "device_prompt", kind: "passcode_for_backup", text: "*** Waiting for passcode to be entered on the device ***" });
    emitLog(j, ['Started "com.apple.mobilebackup2" service on port 49324.', "Negotiated Protocol Version 2.1", "Starting backup..."]);
    await holdIn(j, "backing_up");
    const stopAt = ACQ_FAILURES[scenario] ? 40 : 100;
    const step = scenario === "slow" ? 1 : 10;
    for (let pct = 0; pct <= stopAt; pct += step) {
      await sleep(t);
      if (j.cancelRequested) break;
      if (scenario === "interrupt" && pct >= 30) {
        job = null;
        return;
      }
      emit(j, { type: "progress", percent: scenario === "slow" ? Math.min(pct, 99) : pct });
      emitLog(j, [`[${"=".repeat(Math.round(pct / 5)).padEnd(20)}] ${pct}% Finished`]);
      if (scenario === "slow") pct = Math.min(pct, 98);
    }
    cancelledBeforeExit = j.cancelRequested;
    backupExited = true;
    const cmd = rec.commands[rec.commands.length - 1];
    cmd.exited_at = isoNow();
    const failure = ACQ_FAILURES[scenario];
    cmd.exit_code = cancelledBeforeExit ? null : failure ? (scenario === "incomplete" ? 0 : 151) : 0;
    emitLog(j, [cancelledBeforeExit ? "Backup Aborted." : failure ? (scenario === "incomplete" ? "Backup Failed (Error Code 0)." : "Backup Failed (Error Code 105).") : "Backup Successful."]);
    rec.process = { exit_code: cmd.exit_code, signal: cancelledBeforeExit ? 15 : null, cancel_requested: cancelledBeforeExit, escalated_to_kill: false };
    if (cancelledBeforeExit) reasons = [REASONS.cancelled_by_user];
    else if (failure) reasons = clone(failure);
  } else if (j.cancelRequested) {
    cancelledBeforeExit = true;
    reasons = [REASONS.cancelled_by_user];
  }

  if (enabled && req.restore_encryption && scenario !== "disconnect") {
    acqPhase(j, "restoring_encryption");
    emit(j, { type: "device_prompt", kind: "passcode_for_encryption", text: "Please confirm disabling the backup encryption by entering the passcode on the device." });
    await holdIn(j, "restoring_encryption");
    await sleep(2 * t);
    const ok = scenario !== "restore_fail";
    rec.commands.push({ purpose: "restore_encryption", argv: [tool, "-u", udid, "encryption", "off"], exit_code: ok ? 0 : 1, started_at: isoNow(), exited_at: isoNow() });
    rec.encryption.restored_after = ok ? "restored" : "failed";
    rec.encryption.will_encrypt_after_restore = !ok;
    if (ok) {
      if (device) device.will_encrypt = false;
      rec.device_changes.push({ at: isoNow(), change: "backup_encryption_disabled", detail: "WillEncrypt true → false" });
    } else {
      warnings.push(r("encryption_restore_failed", "Turning backup encryption off failed"));
    }
  }
  if (enabled && rec.encryption.restored_after !== "restored") {
    warnings.push(r("encryption_left_enabled", "Backup encryption was enabled by the examiner and not confirmed disabled"));
  }
  if (rec.encryption.will_encrypt_before === true) {
    warnings.push(r("backup_encryption_preexisting", "Backup encryption was already on: parsing needs the owner's password"));
  }

  acqPhase(j, "validating");
  await sleep(t);
  await holdIn(j, "validating");
  if (backupRan) {
    rec.backup_result = {
      final_message: cancelledBeforeExit ? "Backup Aborted." : ACQ_FAILURES[scenario] ? null : "Backup Successful.",
      udid_dir: `backup/${udid}`,
      manifest_found: scenario === "sync_lock" ? null : "Manifest.db",
      info_plist_found: scenario !== "sync_lock",
      status_plist_found: scenario !== "sync_lock",
      snapshot_state: scenario === "sync_lock" ? null : reasons.length ? "new" : "finished",
      last_progress_percent: reasons.length ? 40 : 100,
      device_file_errors: 0,
      free_bytes_after: 81234567890,
    };
  }
  const sealable = backupRan && scenario !== "sync_lock";
  if (sealable) {
    acqPhase(j, "sealing");
    const files = 48210;
    let done = 0;
    for (let i = 1; i <= 3; i++) {
      await sleep(t);
      if (j.cancelRequested && backupExited && !cancelledBeforeExit) break;
      done = Math.round((files * i) / 3);
      emit(j, { type: "seal_progress", files_done: done, files_total: files });
      if (i === 1) await holdIn(j, "sealing");
    }
    const sealCancelled = done < files;
    if (sealCancelled) warnings.push(r("seal_cancelled", "Sealing was cancelled; backup.sha256 is incomplete"));
    rec.output.seal = sealCancelled
      ? { status: "cancelled", manifest: null, manifest_sha256: null, file_count: null, total_bytes: null }
      : { status: "sealed", manifest: "backup.sha256", manifest_sha256: fx.AcquisitionRecord.output.seal.manifest_sha256, file_count: files, total_bytes: 61203455110 };
  } else {
    rec.output.seal = { status: "skipped_no_output", manifest: null, manifest_sha256: null, file_count: null, total_bytes: null };
  }
  acqPhase(j, "finalizing");
  await sleep(t);
  await holdIn(j, "finalizing");
  /** @type {AcqStatus} */
  const status = cancelledBeforeExit ? "cancelled" : reasons.length ? "failed" : "succeeded";
  rec.status = status;
  rec.status_reasons = reasons;
  rec.warnings = warnings;
  rec.ended_at = isoNow();
  rec.duration_ms = Date.parse(rec.ended_at) - Date.parse(rec.created_at);
  job = null;
  emit(j, { type: "finished", status, reasons: rec.status_reasons, warnings: rec.warnings, summary: acqSummary(j.casePath, rec) });
}

/** @type {Api["acq_cancel"]} */
export const acq_cancel = async (req) => {
  if (job?.kind !== "acquisition" || job.record.acq_id !== req.acq_id) throw appError("acq_not_found", `Acquisition ${req.acq_id} is not active.`);
  // Per ARCHITECTURE.md §6b: ignored while restoring encryption (the restore always completes).
  if (job.phase !== "restoring_encryption") job.cancelRequested = true;
};

/**
 * @param {string} casePath
 * @param {string} acqId
 * @returns {AcquisitionRecord}
 */
function findAcq(casePath, acqId) {
  const acq = knownCase(casePath).acqs.find((a) => a.acq_id === acqId);
  if (!acq) throw appError("acq_not_found", `No acquisition ${acqId} in this case.`);
  return acq;
}

/** @type {Api["acq_get"]} */
export const acq_get = async (req) => clone(findAcq(req.case_path, req.acq_id));

/**
 * The later-restore attempt files of each acquisition, in order (CONTRACTS.md §13.3):
 * `encryption-restore.json`, then `encryption-restore-2.json`, … Each records whether it restored.
 * @type {Map<string, { file: string, restored: boolean }[]>}
 */
const restoreAttempts = new Map();

/**
 * True once a later-restore attempt of this acquisition recorded `restored: true`.
 * @param {string} acqId
 */
const restoredLater = (acqId) => (restoreAttempts.get(acqId) ?? []).some((a) => a.restored);

/** @type {Api["acq_restore_encryption"]} */
export const acq_restore_encryption = async (req) => {
  const acq = findAcq(req.case_path, req.acq_id);
  if (!acq.warnings.some((w) => w.code === "encryption_left_enabled" || w.code === "encryption_state_unknown")) {
    throw appError("restore_not_applicable", "This acquisition did not leave backup encryption on.");
  }
  const attempts = restoreAttempts.get(acq.acq_id) ?? [];
  const earlier = attempts.find((a) => a.restored);
  if (earlier) throw appError("restore_not_applicable", "An earlier attempt already turned backup encryption off for this acquisition.", earlier.file);
  if (job) throw appError("run_already_active", "Another job is running.");
  const device = findDevice(acq.device.udid);
  if (device.pair_state !== "paired") throw appError("device_not_paired", "Pair the device first.");
  if (!req.password) throw appError("encryption_password_required", "Enter the backup password that was set during the acquisition.");
  await sleep(2 * tick());
  // The password "wrong" stands for a wrong password: the tool fails, encryption stays on, and
  // the attempt can be retried.
  const restored = req.password !== "wrong";
  const n = attempts.length + 1;
  attempts.push({ file: n === 1 ? "encryption-restore.json" : `encryption-restore-${n}.json`, restored });
  restoreAttempts.set(acq.acq_id, attempts);
  if (!restored) return { restored: false, will_encrypt_after: true };
  device.will_encrypt = false;
  return { restored: true, will_encrypt_after: false };
};

/** @type {Api["open_acq_file"]} */
export const open_acq_file = async (req) => {
  findAcq(req.case_path, req.acq_id);
  console.info("mock: open_acq_file", req.acq_id, req.which);
};

// ---- Dialogs ----

/** @type {Api["dialog_open"]} */
export const dialog_open = (options) => pickOpen(options);

/** @type {Api["dialog_save"]} */
export const dialog_save = (options) => pickSave(options);

// ---- Start-up scenario ----

// With a `hold_<phase>` flag the start-up job is a normal one that stops in that phase, so the UI
// attaches mid-phase (as after a reload); otherwise it is a slow one.
if (has("active_run")) {
  void run_start(
    {
      case_path: NIGHTJAR,
      tool: "ileapp",
      input_path: HOLDING ? "/Volumes/Evidence/Pixel-7-extraction" : "/Volumes/Evidence/slow",
      input_type: "fs",
      modules: { mode: "all" },
      timezone: "UTC",
      itunes_password: null,
      keychain_path: null,
      hash_input: false,
      label: "Long-running triage",
    },
    () => {},
  );
}
if (has("active_acq")) {
  void acq_start(
    {
      case_path: NIGHTJAR,
      udid: fx.DeviceSummary.udid,
      label: HOLDING ? "Seized iPhone, item 7" : "Seized iPhone, item 7/slow",
      enable_encryption: HOLDING,
      encryption_password: HOLDING ? "examiner-pw" : null,
      restore_encryption: HOLDING,
    },
    () => {},
  );
}
