// @ts-check
// The mock's simulated runs and acquisitions reach every final status (DEVELOPMENT.md §4.6).
import { test } from "node:test";
import assert from "node:assert/strict";

/** @typedef {import("../../ui/types").AcqEvent} AcqEvent */
/** @typedef {import("../../ui/types").RunEvent} RunEvent */

/** @type {any} */ (globalThis).__SUITEDFIR_MOCK_TICK_MS = 1;
const mock = await import("../../ui-dev/mock.js");

const CASE = "/Users/examiner/Documents/suiteDFIR Cases/Operation Nightjar";
const EV = "/Volumes/Evidence";

/**
 * @param {string} input
 * @returns {import("../../ui/types").RunRequest}
 */
const runRequest = (input) => ({
  case_path: CASE,
  tool: "ileapp",
  input_path: input,
  input_type: "fs",
  modules: { mode: "all" },
  timezone: "UTC",
  itunes_password: null,
  keychain_path: null,
  hash_input: false,
  label: null,
});

/**
 * Starts a run and resolves with its events once `finished` arrives (or `onStart` returns).
 * @param {string} input
 * @param {(runId: string, events: RunEvent[]) => Promise<void>} [during]
 */
async function runToEnd(input, during) {
  /** @type {RunEvent[]} */
  const events = [];
  /** @type {(e: RunEvent) => void} */
  let done = () => {};
  const finished = new Promise((resolve) => {
    done = resolve;
  });
  const started = await mock.run_start(runRequest(input), (event) => {
    events.push(event);
    if (event.type === "finished") done(event);
  });
  if (during) await during(started.run_id, events);
  const fin = /** @type {Extract<RunEvent, { type: "finished" }>} */ (await finished);
  return { started, fin, events };
}

/** Resolves once no job is active. */
async function idle() {
  while (await mock.job_active()) await new Promise((resolve) => setTimeout(resolve, 2));
}

for (const [suffix, status, reasons] of /** @type {const} */ ([
  ["Pixel-7-extraction", "succeeded", []],
  ["errors", "completed_with_errors", ["modules_errored"]],
  ["fail-invalid", "failed", ["no_modules_ran", "index_html_missing"]],
  ["fail-early", "failed", ["no_output_dir"]],
  ["fail-argparse", "failed", ["no_output_dir", "nonzero_exit"]],
  ["fail-crash", "failed", ["lava_data_missing", "index_html_missing", "nonzero_exit"]],
])) {
  test(`a run on …/${suffix} finishes ${status}`, async () => {
    const { started, fin, events } = await runToEnd(`${EV}/${suffix}`);
    assert.equal(fin.status, status);
    assert.deepEqual(
      fin.reasons.map((r) => r.code),
      [...reasons],
    );
    assert.equal(fin.summary.run_id, started.run_id);
    assert.deepEqual(
      events.filter((e) => e.type === "phase").map((e) => (e.type === "phase" ? e.phase : "")).slice(0, 2),
      ["preparing", "running"],
    );
    assert.ok(events.some((e) => e.type === "log"));
    assert.ok(events.some((e) => e.type === "stdio_tail"));
    await idle();
    const rec = await mock.run_get({ case_path: CASE, run_id: started.run_id });
    assert.equal(rec.status, status);
  });
}

test("a flood run streams 100,000 log lines in batches of at most 500, then succeeds", async () => {
  const { fin, events } = await runToEnd(`${EV}/flood`);
  const batches = events.filter((e) => e.type === "log").map((e) => (e.type === "log" ? e.lines.length : 0));
  assert.ok(batches.every((n) => n <= 500));
  assert.ok(batches.reduce((a, b) => a + b, 0) >= 100_000);
  assert.equal(fin.status, "succeeded");
});

test("a cancelled run finishes cancelled", async () => {
  const { fin } = await runToEnd(`${EV}/slow`, async (runId, events) => {
    while (!events.some((e) => e.type === "log")) await new Promise((resolve) => setTimeout(resolve, 2));
    await mock.run_cancel({ run_id: runId });
    await mock.run_cancel({ run_id: runId }); // idempotent
  });
  assert.equal(fin.status, "cancelled");
  assert.deepEqual(
    fin.reasons.map((r) => r.code),
    ["cancelled_by_user"],
  );
});

test("an interrupted run is marked interrupted by the next case_open", async () => {
  const started = await mock.run_start(runRequest(`${EV}/interrupt`), () => {});
  const active = await mock.job_active();
  assert.equal(active?.kind, "run");
  await idle();
  const detail = await mock.case_open({ path: CASE });
  assert.ok(detail.recovered.includes(started.run_id));
  assert.equal(detail.runs.find((r) => r.run_id === started.run_id)?.status, "interrupted");
});

test("one active job at a time; job_attach replaces the subscriber and returns the backlog", async () => {
  /** @type {RunEvent[]} */
  const first = [];
  const started = await mock.run_start(runRequest(`${EV}/slow`), (e) => first.push(e));
  await assert.rejects(mock.run_start(runRequest(`${EV}/slow`), () => {}), { code: "run_already_active" });
  while (!first.some((e) => e.type === "log")) await new Promise((resolve) => setTimeout(resolve, 2));
  /** @type {(RunEvent | AcqEvent)[]} */
  const second = [];
  const { backlog } = await mock.job_attach({ kind: "run", id: started.run_id }, (e) => second.push(e));
  assert.ok(backlog.length > 0);
  await mock.run_cancel({ run_id: started.run_id });
  await idle();
  assert.ok(second.some((e) => e.type === "finished"));
  assert.ok(!first.some((e) => e.type === "finished"));
});

test("start refuses unknown modules, missing passwords and overlapping inputs", async () => {
  await assert.rejects(
    mock.run_start({ ...runRequest(`${EV}/x`), modules: { mode: "profile", profile_name: "Messaging" } }, () => {}),
    { code: "unknown_modules" },
  );
  await assert.rejects(
    mock.run_start({ ...runRequest(`${EV}/00008101-000A1B2C3D4E`), input_type: "itunes" }, () => {}),
    { code: "password_required" },
  );
  await assert.rejects(mock.input_inspect({ tool: "ileapp", path: `${CASE}/runs`, case_path: CASE }), { code: "input_overlaps_case" });
  await assert.rejects(mock.input_inspect({ tool: "ileapp", path: `${EV}/denied`, case_path: CASE }), { code: "permission_denied" });
});

/**
 * @param {string} label
 * @param {Partial<import("../../ui/types").AcqRequest>} [extra]
 * @param {(acqId: string, events: AcqEvent[]) => Promise<void>} [during]
 */
async function acquire(label, extra = {}, during) {
  /** @type {AcqEvent[]} */
  const events = [];
  /** @type {(e: AcqEvent) => void} */
  let done = () => {};
  const finished = new Promise((resolve) => {
    done = resolve;
  });
  const started = await mock.acq_start(
    {
      case_path: CASE,
      udid: "00008101-000A1B2C3D4E001E",
      label,
      enable_encryption: true,
      encryption_password: "1234",
      restore_encryption: true,
      ...extra,
    },
    (event) => {
      events.push(event);
      if (event.type === "finished") done(event);
    },
  );
  if (during) await during(started.acq_id, events);
  const fin = /** @type {Extract<AcqEvent, { type: "finished" }>} */ (await finished);
  await idle();
  return { started, fin, events };
}

for (const [scenario, status, warning] of /** @type {const} */ ([
  ["", "succeeded", null],
  ["/restore_fail", "succeeded", "encryption_left_enabled"],
  ["/backup_fail", "failed", null],
  ["/enable_fail", "failed", null],
  ["/incomplete", "failed", null],
  ["/cancel_on_device", "failed", null],
  ["/disconnect", "failed", "encryption_left_enabled"],
  ["/sync_lock", "failed", null],
])) {
  test(`an acquisition labelled "…${scenario}" finishes ${status}`, async () => {
    const { fin, events, started } = await acquire(`Handset${scenario}`);
    assert.equal(fin.status, status);
    if (warning) assert.ok(fin.warnings.some((w) => w.code === warning));
    assert.ok(events.some((e) => e.type === "device_prompt"));
    if (warning === "encryption_left_enabled") {
      // The device still encrypts backups, so turning encryption on again is refused …
      await assert.rejects(acquire("Handset"), { code: "encryption_already_on" });
      // … until "Turn backup encryption off" (a later acq_restore_encryption) succeeds.
      const wrong = await mock.acq_restore_encryption({ case_path: CASE, acq_id: started.acq_id, password: "wrong" });
      assert.deepEqual(wrong, { restored: false, will_encrypt_after: true });
      const later = await mock.acq_restore_encryption({ case_path: CASE, acq_id: started.acq_id, password: "1234" });
      assert.deepEqual(later, { restored: true, will_encrypt_after: false });
    }
  });
}

test("a later restore is refused for an acquisition that did not leave encryption on", async () => {
  const { started } = await acquire("Handset");
  await assert.rejects(mock.acq_restore_encryption({ case_path: CASE, acq_id: started.acq_id, password: "1234" }), { code: "restore_not_applicable" });
});

test("a cancelled acquisition finishes cancelled and still restores encryption", async () => {
  const { fin, started } = await acquire("Handset/slow", {}, async (acqId, events) => {
    while (!events.some((e) => e.type === "progress")) await new Promise((resolve) => setTimeout(resolve, 2));
    await mock.acq_cancel({ acq_id: acqId });
  });
  assert.equal(fin.status, "cancelled");
  const rec = await mock.acq_get({ case_path: CASE, acq_id: started.acq_id });
  assert.equal(rec.encryption.restored_after, "restored");
});

test("an interrupted acquisition is marked interrupted by the next case_open", async () => {
  const started = await mock.acq_start(
    {
      case_path: CASE,
      udid: "00008101-000A1B2C3D4E001E",
      label: "Handset/interrupt",
      enable_encryption: true,
      encryption_password: "1234",
      restore_encryption: true,
    },
    () => {},
  );
  await idle();
  const detail = await mock.case_open({ path: CASE });
  const summary = detail.acquisitions.find((a) => a.acq_id === started.acq_id);
  assert.equal(summary?.status, "interrupted");
  assert.ok(summary?.warnings.includes("encryption_left_enabled"));
});
