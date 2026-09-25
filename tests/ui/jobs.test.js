// @ts-check
import { test } from "node:test";
import assert from "node:assert/strict";

import { jobKey, pollActiveJob, setActiveJob } from "../../ui/lib/jobs.js";
import { createStore, watch } from "../../ui/lib/store.js";
import { ActiveJob } from "../../ui-dev/fixtures/contracts/index.js";

/** @typedef {import("../../ui/lib/context").AppState} AppState */
/** @typedef {import("../../ui/types").ActiveJob} Job */

const RUN = /** @type {Extract<Job, { kind: "run" }>} */ (ActiveJob[0]);
const ACQ = ActiveJob[1];

/** @param {Job | null} activeJob */
const appStore = (activeJob) =>
  createStore(/** @type {AppState} */ ({ mode: "mock", appInfo: null, settings: null, tools: null, activeJob, timezones: null }));

/** @param {number} ms */
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

test("jobKey identifies kind, id, phase and case", () => {
  assert.equal(jobKey(null), null);
  assert.equal(jobKey(RUN), jobKey(structuredClone(RUN)));
  assert.notEqual(jobKey(RUN), jobKey({ ...RUN, phase: "analyzing" }));
  assert.notEqual(jobKey(RUN), jobKey({ ...RUN, run_id: "20260924-183005Z-ileapp-000000" }));
  assert.notEqual(jobKey(RUN), jobKey({ ...RUN, case_path: "/other" }));
  assert.notEqual(jobKey(RUN), jobKey(ACQ));
});

test("setActiveJob ignores an equal job and applies a changed one", () => {
  const store = appStore(RUN);
  let notified = 0;
  store.subscribe(() => notified++);
  setActiveJob(store, structuredClone(RUN));
  assert.equal(notified, 0);
  setActiveJob(store, { ...RUN, phase: "sealing_report" });
  setActiveJob(store, null);
  setActiveJob(store, null);
  assert.equal(notified, 2);
});

test("polling equal jobs does not re-render; a phase change and the job's end do", async () => {
  const store = appStore(RUN);
  /** @type {(Job | null)[]} */
  const answers = [structuredClone(RUN), structuredClone(RUN), structuredClone(RUN), { ...RUN, phase: "analyzing" }, null];
  let calls = 0;
  const api = { job_active: async () => (calls < answers.length ? answers[calls++] : null) };
  /** @type {(string | null)[]} */
  const seen = [];
  const unwatch = watch(store, (s) => jobKey(s.activeJob), (key) => seen.push(key));
  const stop = pollActiveJob(api, store, 1);
  for (let i = 0; i < 200 && store.get().activeJob !== null; i++) await sleep(2);
  stop();
  unwatch();
  assert.equal(calls, 5);
  assert.deepEqual(seen, [jobKey(RUN), jobKey({ ...RUN, phase: "analyzing" }), null]);
});

test("polling stops when no job is active and restarts when one begins", async () => {
  const store = appStore(null);
  let calls = 0;
  const api = { job_active: async () => (calls++, null) };
  const stop = pollActiveJob(api, store, 1);
  await sleep(10);
  assert.equal(calls, 0);
  store.set({ activeJob: RUN });
  for (let i = 0; i < 100 && store.get().activeJob !== null; i++) await sleep(2);
  stop();
  assert.equal(calls, 1);
  assert.equal(store.get().activeJob, null);
});
