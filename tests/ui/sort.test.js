// @ts-check
import { test } from "node:test";
import assert from "node:assert/strict";

import { DEFAULT_RUN_SORT, nextSort, sortRuns } from "../../ui/lib/sort.js";

/** @typedef {import("../../ui/types").RunSummary} RunSummary */

/**
 * @param {string} id
 * @param {Partial<RunSummary>} fields
 * @returns {RunSummary}
 */
const run = (id, fields) => ({
  run_id: id,
  run_dir: `/c/runs/${id}`,
  label: null,
  status: "succeeded",
  tool: "ileapp",
  tool_version: "v2026.4.2",
  input_path: "/ev/a",
  input_type: "fs",
  created_at: "2026-09-20T10:00:00Z",
  started_at: null,
  ended_at: null,
  duration_ms: null,
  report_available: true,
  ...fields,
});

const RUNS = [
  run("a", { created_at: "2026-09-21T09:00:00Z", status: "interrupted", label: "beta", duration_ms: null }),
  run("b", { created_at: "2026-09-24T18:30:05Z", status: "completed_with_errors", label: "Alpha", duration_ms: 1356000 }),
  run("c", { created_at: "2026-09-22T10:15:30Z", status: "cancelled", label: null, tool: "aleapp", tool_version: "v2026.4.1", duration_ms: 420000 }),
  run("d", { created_at: "2026-09-23T14:02:11Z", status: "failed", label: "item 10", duration_ms: 180000 }),
  run("e", { created_at: "2026-09-23T13:40:00Z", status: "succeeded", label: "item 9", duration_ms: 2460000 }),
  run("f", { created_at: "2026-09-25T08:00:00Z", status: "running", label: "gamma" }),
];

/** @param {RunSummary[]} runs */
const ids = (runs) => runs.map((r) => r.run_id).join("");

test("the default sort is newest first", () => {
  assert.deepEqual(DEFAULT_RUN_SORT, { key: "created_at", dir: "desc" });
  assert.equal(ids(sortRuns(RUNS, DEFAULT_RUN_SORT)), "fbdeca");
  assert.equal(ids(sortRuns(RUNS, { key: "created_at", dir: "asc" })), "acedbf");
});

test("status sorts active first, then by severity, success last", () => {
  assert.equal(ids(sortRuns(RUNS, { key: "status", dir: "asc" })), "fdbace");
  assert.equal(ids(sortRuns(RUNS, { key: "status", dir: "desc" })), "ecabdf");
});

test("labels sort case-insensitively and numerically, missing labels last in both directions", () => {
  const asc = sortRuns(RUNS, { key: "label", dir: "asc" }).map((r) => r.label);
  assert.deepEqual(asc, ["Alpha", "beta", "gamma", "item 9", "item 10", null]);
  const desc = sortRuns(RUNS, { key: "label", dir: "desc" }).map((r) => r.label);
  assert.deepEqual(desc, ["item 10", "item 9", "gamma", "beta", "Alpha", null]);
});

test("durations sort numerically with missing durations last", () => {
  assert.deepEqual(
    sortRuns(RUNS, { key: "duration", dir: "desc" }).map((r) => r.duration_ms),
    [2460000, 1356000, 420000, 180000, null, null],
  );
  // Ties (both null) fall back to newest first: f (09-25) before a (09-21).
  assert.equal(ids(sortRuns(RUNS, { key: "duration", dir: "asc" })).slice(-2), "fa");
});

test("tool sorts by tool then version; ties are newest first", () => {
  assert.equal(ids(sortRuns(RUNS, { key: "tool", dir: "asc" })), "cfbdea");
});

test("sortRuns does not modify its input", () => {
  const copy = [...RUNS];
  sortRuns(RUNS, { key: "status", dir: "asc" });
  assert.deepEqual(RUNS, copy);
});

test("nextSort flips the same column and starts new columns in their natural direction", () => {
  assert.deepEqual(nextSort(DEFAULT_RUN_SORT, "created_at"), { key: "created_at", dir: "asc" });
  assert.deepEqual(nextSort({ key: "created_at", dir: "asc" }, "created_at"), { key: "created_at", dir: "desc" });
  assert.deepEqual(nextSort(DEFAULT_RUN_SORT, "label"), { key: "label", dir: "asc" });
  assert.deepEqual(nextSort(DEFAULT_RUN_SORT, "duration"), { key: "duration", dir: "desc" });
  assert.deepEqual(nextSort({ key: "duration", dir: "desc" }, "status"), { key: "status", dir: "asc" });
});
