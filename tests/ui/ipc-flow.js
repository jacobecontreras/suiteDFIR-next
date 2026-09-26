// @ts-check
/**
 * The E2 invoke recording (ROADMAP E2): `ui/api/ipc.js` runs a scripted flow that calls every
 * command of CONTRACTS.md §10 and §13.5 against a stub `window.__TAURI__`, which records each
 * `invoke(name, args)` exactly as the real Tauri API would receive it (channels become
 * `__CHANNEL__:<id>`, as Tauri's own `Channel.toJSON` writes them).
 *
 * The flow uses placeholders that the Rust replay test (`src-tauri/src/replay.rs`) fills in:
 * `<ROOT>` is its temp dir, `<CASE>` the case `case_create` made, `<RUN_n>` / `<ACQ_n>` the ids
 * `run_start` / `acq_start` returned, `<UDID>` fake-idevice's device. The stub answers those three
 * commands with the placeholders so the flow can pass them on, as the UI passes on real answers.
 * The replay picks fake scenarios the way the mock does: a run whose input folder is named `slow`
 * runs until cancelled, and an acquisition label ending in `/<scenario>` selects that
 * fake-idevice scenario.
 *
 * `scripts/record-invokes.mjs` writes the result to `tests/ui/recorded-invokes.json`;
 * `tests/ui/recorded-invokes.test.js` checks that the committed file is current.
 */

/** @typedef {{ cmd: string, args: Record<string, unknown> }} RecordedInvoke */

const PASSWORD = "e2-Replay-Pw";

/**
 * Runs the flow through ipc.js and returns the recorded invokes, in order.
 * @returns {Promise<RecordedInvoke[]>}
 */
export async function recordInvokes() {
  /** @type {RecordedInvoke[]} */
  const recorded = [];
  let nextChannel = 1;
  let runs = 0;
  let acqs = 0;

  class Channel {
    constructor() {
      this.id = nextChannel++;
      /** @type {(message: unknown) => void} */
      this.onmessage = () => {};
    }

    toJSON() {
      return `__CHANNEL__:${this.id}`;
    }
  }

  /**
   * @param {string} cmd
   * @param {Record<string, unknown>} [args]
   */
  async function invoke(cmd, args) {
    recorded.push({ cmd, args: JSON.parse(JSON.stringify(args ?? {})) });
    switch (cmd) {
      case "case_create":
        return { path: "<CASE>" };
      case "run_start":
        runs += 1;
        return { run_id: `<RUN_${runs}>`, run_dir: `<CASE>/runs/<RUN_${runs}>` };
      case "acq_start":
        acqs += 1;
        return { acq_id: `<ACQ_${acqs}>`, acq_dir: `<CASE>/acquisitions/<ACQ_${acqs}>` };
      default:
        return null;
    }
  }

  const g = /** @type {any} */ (globalThis);
  const previous = g.window;
  g.window = { __TAURI__: { core: { invoke, Channel }, dialog: {} } };
  try {
    const ipc = await import("../../ui/api/ipc.js");
    await flow(ipc);
  } finally {
    g.window = previous;
  }
  return recorded;
}

/**
 * Every command, in an order that works against the real handlers.
 * @param {typeof import("../../ui/api/ipc.js")} ipc
 */
async function flow(ipc) {
  const ignore = () => {};

  // §10: app, settings, tools. iLEAPP is the dev override (fake-leapp); aLEAPP is installed
  // through the real pipeline from a test archive.
  await ipc.app_info();
  await ipc.licenses_get();
  await ipc.settings_get();
  await ipc.settings_update({
    cases_root: "<ROOT>/cases",
    defaults: { examiner: "J. Doe", agency: "County Forensics Lab", timezone: "America/Chicago" },
  });
  await ipc.tools_status();
  await ipc.tool_verify({ tool: "ileapp" });
  await ipc.tool_modules({ tool: "ileapp" });
  await ipc.tool_install({ tool: "aleapp" }, ignore);
  await ipc.tool_import({ tool: "aleapp", archive_path: "<ROOT>/aleapp-replay.zip" }, ignore);
  await ipc.tool_verify({ tool: "aleapp" });
  await ipc.tool_modules({ tool: "aleapp" });
  await ipc.settings_update({ tools_dir: null });

  // §10: cases, inspection, profiles.
  await ipc.cases_list();
  const created = await ipc.case_create({
    name: "Operation Nightjar",
    case_number: "2026-0142",
    examiner: "J. Doe",
    agency: "County Forensics Lab",
    description: "Seized handset, item 3",
    default_timezone: "America/Chicago",
    parent_dir: null,
  });
  const casePath = created.path;
  await ipc.case_update({
    path: casePath,
    fields: {
      name: "Operation Nightjar",
      case_number: "2026-0142",
      examiner: "J. Doe",
      agency: "County Forensics Lab",
      description: "Seized handset, item 3 (updated)",
      default_timezone: null,
    },
  });
  await ipc.case_open({ path: casePath });
  await ipc.input_inspect({ tool: "ileapp", path: "<ROOT>/evidence/fs", case_path: casePath });
  await ipc.ios_backups_find();
  await ipc.profiles_list({ tool: "aleapp" });
  await ipc.profile_save({ tool: "aleapp", name: "Triage", modules: ["callLogs"] });
  await ipc.profile_import({ tool: "aleapp", path: "<ROOT>/Imported.alprofile", name: null, overwrite: false });
  await ipc.profile_export({ tool: "aleapp", name: "Triage", dest_path: "<ROOT>/Triage.alprofile" });
  await ipc.profile_delete({ tool: "aleapp", name: "Imported" });

  // §10: runs. A slow run is attached to and cancelled; a second one finishes.
  const slow = await ipc.run_start(
    {
      case_path: casePath,
      tool: "ileapp",
      input_path: "<ROOT>/evidence/slow",
      input_type: "fs",
      modules: { mode: "all" },
      timezone: "America/Chicago",
      itunes_password: null,
      keychain_path: null,
      hash_input: false,
      label: "Long triage",
    },
    ignore,
  );
  await ipc.job_active();
  await ipc.job_attach({ kind: "run", id: slow.run_id }, ignore);
  await ipc.run_cancel({ run_id: slow.run_id });
  const run = await ipc.run_start(
    {
      case_path: casePath,
      tool: "aleapp",
      input_path: "<ROOT>/evidence/fs",
      input_type: "fs",
      modules: { mode: "profile", profile_name: "Triage" },
      timezone: null,
      itunes_password: null,
      keychain_path: null,
      hash_input: false,
      label: null,
    },
    ignore,
  );
  await ipc.run_get({ case_path: casePath, run_id: run.run_id });
  await ipc.open_report({ case_path: casePath, run_id: run.run_id });
  await ipc.open_text_file({ case_path: casePath, run_id: run.run_id, which: "stdout" });
  await ipc.reveal_path({ path: run.run_dir });
  await ipc.temp_cleanup();

  // §13.5: the device starts unpaired: the first Pair shows the Trust dialog, the second pairs.
  const udid = "<UDID>";
  await ipc.devices_list();
  await ipc.device_pair({ udid });
  await ipc.device_pair({ udid });
  await ipc.acq_preflight({ case_path: casePath, udid });
  /** @param {string} label @param {string | null} password */
  const acqRequest = (label, password) => ({
    case_path: casePath,
    udid,
    label,
    enable_encryption: password !== null,
    encryption_password: password,
    restore_encryption: true,
  });
  const acq = await ipc.acq_start(acqRequest("Seized iPhone, item 7", null), ignore);
  await ipc.acq_get({ case_path: casePath, acq_id: acq.acq_id });
  await ipc.open_acq_file({ case_path: casePath, acq_id: acq.acq_id, which: "acquisition_json" });
  const slowAcq = await ipc.acq_start(acqRequest("Seized iPhone, item 7/slow", null), ignore);
  await ipc.job_attach({ kind: "acquisition", id: slowAcq.acq_id }, ignore);
  await ipc.acq_cancel({ acq_id: slowAcq.acq_id });
  // Encryption turned on and not turned off again: a later restore.
  const leftOn = await ipc.acq_start(acqRequest("Seized iPhone, item 7/restore_fail", PASSWORD), ignore);
  await ipc.acq_restore_encryption({ case_path: casePath, acq_id: leftOn.acq_id, password: PASSWORD });

  await ipc.case_forget({ path: casePath });
}
