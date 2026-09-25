// @ts-check
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  filterModules,
  groupByCategory,
  orderedSelection,
  queryTokens,
  searchText,
  setMany,
  toggleGroup,
  triState,
  unknownNames,
  withoutUnknown,
} from "../../ui/lib/selection.js";
import { makeModules } from "../../ui-dev/fixtures/modules.js";

/** @typedef {import("../../ui/types").ModuleInfo} ModuleInfo */

/** @type {ModuleInfo[]} */
const MODULES = [
  { name: "callHistory", module_name: "callHistory", category: "Call History", display_name: "Call History", description: null },
  { name: "facetimeCalls", module_name: "callHistory", category: "Call History", display_name: "FaceTime Calls", description: "FaceTime audio and video" },
  { name: "sms", module_name: "sms", category: "SMS & iMessage", display_name: "SMS & iMessage", description: "Messages from sms.db" },
  { name: "smsAttachments", module_name: "sms", category: "SMS & iMessage", display_name: "Message Attachments", description: null },
  { name: "safariHistory", module_name: "safari", category: "Safari", display_name: "Safari History", description: null },
];
const KNOWN = new Set(MODULES.map((m) => m.name));

test("queryTokens splits on whitespace and lowercases", () => {
  assert.deepEqual(queryTokens("  Call   HISTORY "), ["call", "history"]);
  assert.deepEqual(queryTokens(""), []);
});

test("filter matches display name, artifact name, category and description, all tokens required", () => {
  const names = (/** @type {string} */ q) => filterModules(MODULES, q).map((m) => m.name);
  assert.deepEqual(names(""), ["callHistory", "facetimeCalls", "sms", "smsAttachments", "safariHistory"]);
  assert.deepEqual(names("facetime"), ["facetimeCalls"]);
  assert.deepEqual(names("SMSATTACH"), ["smsAttachments"]);
  assert.deepEqual(names("imessage"), ["sms", "smsAttachments"]);
  assert.deepEqual(names("sms.db"), ["sms"]);
  assert.deepEqual(names("history call"), ["callHistory", "facetimeCalls"]);
  assert.deepEqual(names("history safari"), ["safariHistory"]);
  assert.deepEqual(names("nothing-like-this"), []);
  assert.ok(searchText(MODULES[1]).includes("facetime audio"));
});

test("groupByCategory keeps first-seen order", () => {
  assert.deepEqual(
    groupByCategory(MODULES).map((g) => [g.category, g.modules.length]),
    [
      ["Call History", 2],
      ["SMS & iMessage", 2],
      ["Safari", 1],
    ],
  );
});

test("triState is none, some or all over the given names", () => {
  const names = ["callHistory", "facetimeCalls"];
  assert.equal(triState(names, new Set()), "none");
  assert.equal(triState(names, new Set(["facetimeCalls"])), "some");
  assert.equal(triState(names, new Set(["callHistory", "facetimeCalls", "sms"])), "all");
  assert.equal(triState([], new Set(["sms"])), "none");
});

test("toggling a group selects all of a partial or empty group and clears a full one", () => {
  const names = ["callHistory", "facetimeCalls"];
  const partial = new Set(["callHistory", "sms"]);
  const full = toggleGroup(partial, names);
  assert.deepEqual([...full].sort(), ["callHistory", "facetimeCalls", "sms"]);
  assert.deepEqual([...toggleGroup(full, names)], ["sms"]);
  assert.deepEqual([...toggleGroup(new Set(), names)].sort(), names);
  assert.deepEqual([...partial].sort(), ["callHistory", "sms"], "input set unchanged");
});

test("toggling a filtered group only touches the shown members", () => {
  const shown = filterModules(MODULES, "facetime").map((m) => m.name);
  const next = toggleGroup(new Set(["sms"]), shown);
  assert.deepEqual([...next].sort(), ["facetimeCalls", "sms"]);
  assert.equal(triState(["callHistory", "facetimeCalls"], next), "some");
});

test("setMany adds or removes names", () => {
  assert.deepEqual([...setMany(new Set(["a"]), ["b", "c"], true)], ["a", "b", "c"]);
  assert.deepEqual([...setMany(new Set(["a", "b"]), ["b", "x"], false)], ["a"]);
});

test("unknown diff: names the tool does not know, in selection order, and their removal", () => {
  const selection = ["sms", "noSuchModule", "callHistory", "removedArtifact"];
  assert.deepEqual(unknownNames(selection, KNOWN), ["noSuchModule", "removedArtifact"]);
  assert.deepEqual([...withoutUnknown(selection, KNOWN)], ["sms", "callHistory"]);
  assert.deepEqual(unknownNames(["sms"], KNOWN), []);
});

test("orderedSelection lists known names in module order, then unknown names", () => {
  const selected = new Set(["safariHistory", "zzUnknown", "callHistory", "aaUnknown"]);
  assert.deepEqual(orderedSelection(selected, MODULES), ["callHistory", "safariHistory", "zzUnknown", "aaUnknown"]);
});

test("filtering 1,300 fixture modules is fast and correct", () => {
  const modules = makeModules("ileapp", 1300);
  assert.equal(modules.length, 1300);
  assert.equal(new Set(modules.map((m) => m.name)).size, 1300);
  const start = performance.now();
  const hits = filterModules(modules, "history call");
  const elapsed = performance.now() - start;
  assert.ok(hits.some((m) => m.name === "callHistory"));
  assert.ok(hits.every((m) => searchText(m).includes("history") && searchText(m).includes("call")));
  assert.ok(elapsed < 50, `filter took ${elapsed.toFixed(1)} ms`);
});
