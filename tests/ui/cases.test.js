// @ts-check
// Case-form validation, timezone lists and tool states.
import { test } from "node:test";
import assert from "node:assert/strict";

import { editableFields, folderLabel, validateCaseFields } from "../../ui/lib/cases.js";
import { createStore } from "../../ui/lib/store.js";
import { FALLBACK_TIMEZONES, loadTimezones, pickTimezone, timezoneList } from "../../ui/lib/timezones.js";
import { installedTools } from "../../ui/lib/tools.js";
import { CaseFile, ToolModules, ToolStatus } from "../../ui-dev/fixtures/contracts/index.js";

/** @typedef {import("../../ui/lib/context").AppState} AppState */
/** @typedef {import("../../ui/types").ToolStatus} Status */

/** @param {Status[]} tools */
const tzStore = (tools) =>
  createStore(/** @type {AppState} */ ({ mode: "mock", appInfo: null, settings: null, tools, activeJob: null, timezones: null }));

/**
 * A fake API whose tool_modules fails while `state.fail` is set; counts its calls.
 * @param {{ fail: boolean, calls: number }} state
 * @returns {any}
 */
const tzApi = (state) => ({
  tool_modules: async () => {
    state.calls += 1;
    if (state.fail) throw { code: "io", message: "temporarily unreadable", detail: null };
    return structuredClone(ToolModules);
  },
});

test("loadTimezones without iLEAPP uses the fallback and does not cache it", async () => {
  const store = tzStore([{ ...ToolStatus, state: "not_installed", installed_version: null }]);
  const state = { fail: false, calls: 0 };
  const list = await loadTimezones(tzApi(state), store);
  assert.ok(list.includes("UTC"));
  assert.equal(state.calls, 0);
  assert.equal(store.get().timezones, null);
});

test("loadTimezones never caches a fallback, so iLEAPP's list is used once it loads", async () => {
  const store = tzStore([{ ...ToolStatus }]);
  const state = { fail: true, calls: 0 };
  const api = tzApi(state);
  const fallback = await loadTimezones(api, store);
  assert.notDeepEqual(fallback, ToolModules.timezones);
  assert.equal(store.get().timezones, null);
  state.fail = false;
  assert.deepEqual(await loadTimezones(api, store), ToolModules.timezones);
  assert.deepEqual(store.get().timezones, { version: ToolModules.version, list: ToolModules.timezones });
  assert.deepEqual(await loadTimezones(api, store), ToolModules.timezones);
  assert.equal(state.calls, 2, "the cached iLEAPP list is reused");
});

test("loadTimezones reloads iLEAPP's list when its installed version changes", async () => {
  const store = tzStore([{ ...ToolStatus }]);
  const state = { fail: false, calls: 0 };
  const api = tzApi(state);
  await loadTimezones(api, store);
  store.set({ tools: [{ ...ToolStatus, installed_version: "v2026.9.9" }] });
  await loadTimezones(api, store);
  assert.equal(state.calls, 2);
});

test("case name validation: required, at most 120 characters, trimmed", () => {
  assert.deepEqual(validateCaseFields({ name: "Operation Nightjar" }), {});
  assert.deepEqual(validateCaseFields({ name: "   " }), { name: "Enter a case name." });
  assert.deepEqual(validateCaseFields({ name: "x".repeat(120) }), {});
  assert.deepEqual(validateCaseFields({ name: "x".repeat(121) }), { name: "The case name can be at most 120 characters." });
  assert.deepEqual(validateCaseFields({ name: ` ${"x".repeat(120)} ` }), {});
});

test("editableFields takes exactly the editable case.json fields", () => {
  assert.deepEqual(Object.keys(editableFields(CaseFile)).sort(), [
    "agency",
    "case_number",
    "default_timezone",
    "description",
    "examiner",
    "name",
  ]);
});

test("folderLabel is the last path segment on both path styles", () => {
  assert.equal(folderLabel("/a/b/Riverside 2025/"), "Riverside 2025");
  assert.equal(folderLabel("C:\\Cases\\Old"), "Old");
});

test("timezone list source order: the tool's list, then Intl (with UTC), then the embedded list", () => {
  assert.deepEqual(timezoneList(["UTC", "Europe/Berlin"], () => ["X/Y"]), ["UTC", "Europe/Berlin"]);
  assert.deepEqual(timezoneList(null, () => ["Europe/Berlin", "Asia/Tokyo"]), ["UTC", "Europe/Berlin", "Asia/Tokyo"]);
  assert.deepEqual(timezoneList([], () => ["UTC", "Asia/Tokyo"]), ["UTC", "Asia/Tokyo"]);
  assert.deepEqual(timezoneList(null, () => []), [...FALLBACK_TIMEZONES]);
  assert.deepEqual(
    timezoneList(null, () => {
      throw new RangeError("unsupported");
    }),
    [...FALLBACK_TIMEZONES],
  );
  assert.ok(FALLBACK_TIMEZONES.includes("UTC"));
});

test("pickTimezone: case, then settings, then UTC, then the first entry", () => {
  const list = ["America/Chicago", "UTC", "Europe/Berlin"];
  assert.equal(pickTimezone(list, ["Europe/Berlin", "America/Chicago", "UTC"]), "Europe/Berlin");
  assert.equal(pickTimezone(list, [null, "America/Chicago", "UTC"]), "America/Chicago");
  assert.equal(pickTimezone(list, ["Mars/Base", "", "UTC"]), "UTC");
  assert.equal(pickTimezone(["Asia/Tokyo"], [null, "", "UTC"]), "Asia/Tokyo");
  assert.equal(pickTimezone([], ["UTC"]), null);
});

test("installedTools keeps verified, unverified and dev-override tools only", () => {
  /** @type {import("../../ui/types").ToolState[]} */
  const states = ["unsupported_platform", "not_installed", "installed_unverified", "verified", "verification_failed", "dev_override"];
  const tools = states.map((state) => ({ ...ToolStatus, state }));
  assert.deepEqual(
    installedTools(tools).map((t) => t.state),
    ["installed_unverified", "verified", "dev_override"],
  );
  assert.deepEqual(installedTools(null), []);
});
