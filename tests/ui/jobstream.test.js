// @ts-check
// The active job's event stream: reducers, the log cap, phase steps and the hub (D4a, D5).
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  ACQ_PHASES,
  RUN_PHASES,
  appendLog,
  applyEvent,
  createJobStreams,
  newStream,
  seedPhase,
  stepStates,
} from "../../ui/lib/jobstream.js";
import { createStore } from "../../ui/lib/store.js";
import { ActiveJob, RunEvent } from "../../ui-dev/fixtures/contracts/index.js";

/** @typedef {import("../../ui/lib/context").AppState} AppState */
/** @typedef {import("../../ui/types").ActiveJob} Job */
/** @typedef {import("../../ui/types").RunEvent} Run */
/** @typedef {import("../../ui/types").AcqEvent} Acq */

const RUN_JOB = /** @type {Extract<Job, { kind: "run" }>} */ (ActiveJob[0]);
const ACQ_JOB = /** @type {Extract<Job, { kind: "acquisition" }>} */ (ActiveJob[1]);
const FINISHED = /** @type {Extract<Run, { type: "finished" }>} */ (RunEvent[5]);

/** @param {Job | null} activeJob */
const appStore = (activeJob) =>
  createStore(/** @type {AppState} */ ({ mode: "mock", appInfo: null, settings: null, tools: null, activeJob, timezones: null }));

test("run events: phases, log lines, stdio tails, hash and seal progress, finished", () => {
  const s = newStream("run", "r1", "/c");
  /** @type {Run[]} */
  const events = [
    { type: "phase", phase: "preparing" },
    { type: "phase", phase: "running" },
    { type: "phase", phase: "running" },
    { type: "log", lines: ["a", "b"] },
    { type: "log", lines: ["c"] },
    { type: "hash_progress", bytes_done: 10, bytes_total: 100 },
    { type: "stdio_tail", stream: "stdout", lines: ["out"] },
    { type: "stdio_tail", stream: "stderr", lines: [] },
    { type: "phase", phase: "sealing_report" },
    { type: "seal_progress", files_done: 3, files_total: null },
  ];
  for (const e of events) applyEvent(s, e);
  assert.deepEqual(s.phases, ["preparing", "running", "sealing_report"]);
  assert.equal(s.phase, "sealing_report");
  assert.deepEqual(s.log.lines, ["a", "b", "c"]);
  assert.deepEqual(s.hash, { done: 10, total: 100 });
  assert.deepEqual(s.seal, { done: 3, total: null });
  assert.deepEqual(s.stdio, { stdout: ["out"], stderr: [] });
  assert.equal(s.live, true);
  assert.equal(s.version, events.length);
  applyEvent(s, FINISHED);
  assert.equal(s.finished, FINISHED);
  assert.equal(s.live, false);
});

test("acquisition events: a device prompt shows until progress resumes or the phase changes", () => {
  const s = newStream("acquisition", "a1", "/c");
  /** @param {Acq} e */
  const apply = (e) => applyEvent(s, e);
  apply({ type: "phase", phase: "enabling_encryption" });
  apply({ type: "device_prompt", kind: "passcode_for_encryption", text: "Please confirm enabling the backup encryption by entering the passcode on the device." });
  assert.equal(s.prompt?.kind, "passcode_for_encryption");
  apply({ type: "log", lines: ["waiting"] });
  assert.ok(s.prompt, "a log line does not clear the prompt");
  apply({ type: "phase", phase: "backing_up" });
  assert.equal(s.prompt, null, "a new phase clears it");
  apply({ type: "device_prompt", kind: "passcode_for_backup", text: "*** Waiting for passcode to be entered on the device ***" });
  apply({ type: "progress", percent: 12 });
  assert.equal(s.prompt, null, "progress means the passcode was entered");
  assert.equal(s.percent, 12);
  apply({ type: "device_prompt", kind: "passcode_for_backup", text: "again" });
  apply({
    type: "finished",
    status: "cancelled",
    reasons: [],
    warnings: [],
    summary: {
      acq_id: "a1",
      acq_dir: "/c/acquisitions/a1",
      label: null,
      status: "cancelled",
      udid: "u",
      device_name: null,
      product_version: null,
      created_at: "2026-09-24T17:12:00Z",
      started_at: null,
      ended_at: null,
      duration_ms: null,
      backup_path: null,
      warnings: [],
    },
  });
  assert.equal(s.prompt, null, "finishing clears it");
  assert.equal(s.live, false);
});

test("the log keeps at most `max` lines, dropping the oldest in chunks and counting them", () => {
  const log = { lines: /** @type {string[]} */ ([]), dropped: 0 };
  appendLog(log, ["1", "2", "3", "4"], 5, 2);
  assert.deepEqual(log, { lines: ["1", "2", "3", "4"], dropped: 0 });
  appendLog(log, ["5", "6"], 5, 2);
  // 6 lines > 5: drop 6 - 5 + 2 = 3.
  assert.deepEqual(log, { lines: ["4", "5", "6"], dropped: 3 });
  appendLog(log, Array.from({ length: 20 }, (_, i) => String(7 + i)), 5, 2);
  assert.equal(log.lines.length, 3);
  assert.equal(log.dropped + log.lines.length, 26);
  assert.equal(log.lines[log.lines.length - 1], "26");
});

test("100,000 lines in batches of 500 are kept in full (under the cap)", () => {
  const s = newStream("run", "r1", "/c");
  for (let n = 0; n < 100_000; n += 500) {
    applyEvent(s, { type: "log", lines: Array.from({ length: 500 }, (_, i) => `line ${n + i + 1}`) });
  }
  assert.equal(s.log.lines.length, 100_000);
  assert.equal(s.log.dropped, 0);
  assert.equal(s.log.lines[99_999], "line 100000");
});

test("phase steps: done, current, pending; skipped steps; optional steps only when they happen", () => {
  const optional = new Set(["hashing_input"]);
  const steps = (/** @type {Partial<{ phases: string[], phase: string | null, finished: boolean, partial: boolean }>} */ s) =>
    stepStates(RUN_PHASES, { phases: [], phase: null, finished: false, partial: false, ...s }, optional)
      .map((x) => `${x.phase}:${x.state}`)
      .join(" ");
  assert.equal(
    steps({ phases: ["preparing", "running"], phase: "running" }),
    "preparing:done running:current analyzing:pending sealing_report:pending finalizing:pending",
  );
  assert.equal(
    steps({ phases: ["preparing", "running", "hashing_input"], phase: "hashing_input" }),
    "preparing:done running:done hashing_input:current analyzing:pending sealing_report:pending finalizing:pending",
  );
  // No report: sealing was skipped.
  assert.equal(
    steps({ phases: ["preparing", "running", "analyzing", "finalizing"], phase: "finalizing" }),
    "preparing:done running:done analyzing:done sealing_report:skipped finalizing:current",
  );
  assert.equal(
    steps({ phases: ["preparing", "running", "analyzing", "sealing_report", "finalizing"], phase: "finalizing", finished: true }),
    "preparing:done running:done analyzing:done sealing_report:done finalizing:done",
  );
  // Attached after a reload during sealing: the earlier steps count as done; hashing is unknown.
  assert.equal(
    steps({ phases: ["sealing_report"], phase: "sealing_report", partial: true }),
    "preparing:done running:done analyzing:done sealing_report:current finalizing:pending",
  );
  assert.equal(steps({}), "preparing:pending running:pending analyzing:pending sealing_report:pending finalizing:pending");
});

test("acquisition steps: the encryption steps appear only when encryption is changed", () => {
  const optional = new Set(["enabling_encryption", "restoring_encryption"]);
  const plain = stepStates(ACQ_PHASES, { phases: ["preparing", "backing_up"], phase: "backing_up", finished: false, partial: false }, optional);
  assert.deepEqual(
    plain.map((x) => x.phase),
    ["preparing", "backing_up", "validating", "sealing", "finalizing"],
  );
  const enc = stepStates(ACQ_PHASES, { phases: ["preparing", "enabling_encryption"], phase: "enabling_encryption", finished: false, partial: false }, optional);
  assert.deepEqual(
    enc.map((x) => `${x.phase}:${x.state}`),
    ["preparing:done", "enabling_encryption:current", "backing_up:pending", "validating:pending", "sealing:pending", "finalizing:pending"],
  );
});

test("hub: a started job streams into its stream, keeps the active job's phase and clears it on finish", () => {
  const store = appStore(null);
  /** @type {string[]} */
  const finishedHook = [];
  const jobs = createJobStreams(store, { onFinished: (s) => finishedHook.push(`${s.kind}:${s.id}`) });
  let notified = 0;
  jobs.subscribe(() => notified++);
  const b = jobs.begin("run", RUN_JOB.case_path);
  // An event before run_start returns (the id is not known yet) is kept.
  b.onEvent({ type: "phase", phase: "running" });
  b.bind(RUN_JOB.run_id);
  assert.equal(jobs.find("run", RUN_JOB.run_id), jobs.current());
  store.set({ activeJob: { ...RUN_JOB, phase: "running" } });
  b.onEvent({ type: "phase", phase: "analyzing" });
  assert.equal(store.get().activeJob?.phase, "analyzing");
  b.onEvent({ type: "log", lines: ["x"] });
  b.onEvent(FINISHED);
  assert.equal(store.get().activeJob, null);
  assert.deepEqual(finishedHook, [`run:${RUN_JOB.run_id}`]);
  assert.ok(notified >= 5);
  const s = jobs.current();
  assert.ok(s);
  assert.deepEqual(s.log.lines, ["x"]);
  assert.equal(s.partial, false);
});

test("hub: a failed start is abandoned, and a newer job's stream ignores the old channel", () => {
  const jobs = createJobStreams(appStore(null));
  const failed = jobs.begin("run", "/c");
  failed.abandon();
  assert.equal(jobs.current(), null);
  const first = jobs.begin("run", "/c");
  first.bind("r1");
  const second = jobs.begin("acquisition", "/c");
  second.bind("a1");
  first.onEvent({ type: "log", lines: ["stale"] });
  assert.equal(jobs.current()?.id, "a1");
  assert.deepEqual(jobs.current()?.log.lines, []);
});

test("hub: attach after a reload seeds the backlog, then applies events that arrived meanwhile, in order", async () => {
  const store = appStore(ACQ_JOB);
  const jobs = createJobStreams(store);
  /** @type {((e: Run | Acq) => void) | null} */
  let channel = null;
  /** @type {import("../../ui/types").JobAttachRequest | null} */
  let asked = null;
  const api = {
    /** @param {import("../../ui/types").JobAttachRequest} req @param {(e: Run | Acq) => void} onEvent */
    job_attach: async (req, onEvent) => {
      asked = req;
      channel = onEvent;
      onEvent({ type: "log", lines: ["after attach"] });
      onEvent({ type: "progress", percent: 51 });
      return { backlog: ["old 1", "old 2"] };
    },
  };
  const s = await jobs.attach(api, "acquisition", ACQ_JOB.acq_id, ACQ_JOB.case_path);
  assert.deepEqual(asked, { kind: "acquisition", id: ACQ_JOB.acq_id });
  assert.deepEqual(s.log.lines, ["old 1", "old 2", "after attach"]);
  assert.equal(s.percent, 51);
  assert.equal(s.partial, true);
  assert.ok(channel);
  /** @type {(e: Run | Acq) => void} */ (channel)({ type: "phase", phase: "sealing" });
  assert.equal(store.get().activeJob?.phase, "sealing");
  // A second attach for the same live job reuses the stream.
  assert.equal(await jobs.attach(api, "acquisition", ACQ_JOB.acq_id, ACQ_JOB.case_path), s);
});

/**
 * @param {readonly string[]} order
 * @param {import("../../ui/lib/jobstream.js").JobStream} s
 * @param {ReadonlySet<string>} optional
 */
const stepText = (order, s, optional) =>
  stepStates(order, { phases: s.phases, phase: s.phase, finished: s.finished !== null, partial: s.partial }, optional)
    .map((x) => `${x.phase}:${x.state}`)
    .join(" ");

test("hub: attach after a reload starts in the active job's phase, before any phase event", async () => {
  // Reloaded while the acquisition turns encryption off again: job_attach returns only the backlog,
  // and no phase event follows (the step may wait for the device passcode for a long time).
  const acqJob = { ...ACQ_JOB, phase: /** @type {const} */ ("restoring_encryption") };
  const store = appStore(acqJob);
  const jobs = createJobStreams(store);
  const quiet = { job_attach: async () => ({ backlog: ["Please confirm disabling the backup encryption…"] }) };
  const s = await jobs.attach(quiet, "acquisition", acqJob.acq_id, acqJob.case_path);
  assert.equal(s.phase, "restoring_encryption");
  assert.equal(
    stepText(ACQ_PHASES, s, new Set(["enabling_encryption", "restoring_encryption"])),
    "preparing:done backing_up:done restoring_encryption:current validating:pending sealing:pending finalizing:pending",
    "the encryption step is shown and current; the earlier steps count as done",
  );

  // A run reloaded during analysis.
  const runJob = { ...RUN_JOB, phase: /** @type {const} */ ("analyzing") };
  const runStore = appStore(runJob);
  const run = await createJobStreams(runStore).attach(quiet, "run", runJob.run_id, runJob.case_path);
  assert.equal(
    stepText(RUN_PHASES, run, new Set(["hashing_input"])),
    "preparing:done running:done analyzing:current sealing_report:pending finalizing:pending",
  );
});

test("hub: attach reads job_active once subscribed; a phase event that already arrived wins", async () => {
  // The stored phase is from the last poll (up to 2 s old); job_active, read after subscribing, is
  // current, and it also updates the top bar.
  const store = appStore({ ...ACQ_JOB, phase: "backing_up" });
  const jobs = createJobStreams(store);
  let calls = 0;
  const api = {
    job_attach: async () => ({ backlog: [] }),
    job_active: async () => {
      calls += 1;
      return { ...ACQ_JOB, phase: /** @type {const} */ ("restoring_encryption") };
    },
  };
  const s = await jobs.attach(api, "acquisition", ACQ_JOB.acq_id, ACQ_JOB.case_path);
  assert.equal(calls, 1);
  assert.equal(s.phase, "restoring_encryption");
  assert.equal(store.get().activeJob?.phase, "restoring_encryption");

  // A phase event delivered through the new subscription is newer than both.
  const store2 = appStore({ ...ACQ_JOB, phase: "backing_up" });
  const jobs2 = createJobStreams(store2);
  const api2 = {
    /** @param {unknown} _req @param {(e: Run | Acq) => void} onEvent */
    job_attach: async (_req, onEvent) => {
      onEvent({ type: "phase", phase: "validating" });
      return { backlog: [] };
    },
    job_active: async () => ({ ...ACQ_JOB, phase: /** @type {const} */ ("restoring_encryption") }),
  };
  const s2 = await jobs2.attach(api2, "acquisition", ACQ_JOB.acq_id, ACQ_JOB.case_path);
  assert.equal(s2.phase, "validating");
  assert.equal(store2.get().activeJob?.phase, "validating");

  // job_active failing or naming another job leaves the stored phase.
  const store3 = appStore({ ...ACQ_JOB, phase: "sealing" });
  const s3 = await createJobStreams(store3).attach(
    {
      job_attach: async () => ({ backlog: [] }),
      job_active: async () => {
        throw { code: "internal", message: "x", detail: null };
      },
    },
    "acquisition",
    ACQ_JOB.acq_id,
    ACQ_JOB.case_path,
  );
  assert.equal(s3.phase, "sealing");
  assert.equal(seedPhase(newStream("run", "other", "/c"), ACQ_JOB), false, "another job's phase is never taken");
});

test("hub: attach rejects when the job already ended and leaves no stream behind", async () => {
  const jobs = createJobStreams(appStore(null));
  const api = {
    job_attach: async () => {
      throw { code: "run_not_found", message: "Run r1 is not active.", detail: null };
    },
  };
  await assert.rejects(jobs.attach(api, "run", "r1", "/c"), { code: "run_not_found" });
  assert.equal(jobs.current(), null);
});
