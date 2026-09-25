// @ts-check
// Settings: tool states, their actions, and install/import progress (ROADMAP D4b).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { MAX_MESSAGES, TOOL_STATES, applyInstallEvent, finishInstall, installSteps, startInstall, toolActions } from "../../ui/lib/install.js";

/** @typedef {import("../../ui/types").ToolState} ToolState */

/**
 * @param {import("../../ui/lib/install.js").InstallProgress} p
 */
const steps = (p) =>
  installSteps(p)
    .map((s) => `${s.phase}:${s.state}`)
    .join(" ");

test("every ToolState of CONTRACTS.md §2 has a label, a tone and an explanation", () => {
  const doc = readFileSync(new URL("../../docs/CONTRACTS.md", import.meta.url), "utf8");
  const row = /^\| `ToolState` \| (.+) \|$/m.exec(doc);
  assert.ok(row, "ToolState row not found");
  const states = [...row[1].matchAll(/`([a-z_]+)`/g)].map((m) => m[1]).sort();
  assert.equal(states.length, 6);
  assert.deepEqual(Object.keys(TOOL_STATES).sort(), states);
  for (const info of Object.values(TOOL_STATES)) {
    assert.ok(info.label && info.text.length > 20);
  }
});

test("actions per state: install/import when missing or failed, verify when installed, nothing otherwise", () => {
  /** @type {Record<ToolState, string>} */
  const expected = {
    not_installed: "install import",
    verification_failed: "install import verify",
    installed_unverified: "verify",
    verified: "verify",
    unsupported_platform: "",
    dev_override: "",
  };
  for (const [state, actions] of Object.entries(expected)) {
    const a = toolActions(/** @type {ToolState} */ (state));
    const got = [a.install && "install", a.import && "import", a.verify && "verify"].filter(Boolean).join(" ");
    assert.equal(got, actions, state);
  }
});

test("install events: stages in order (repeats ignored), download progress, the latest messages", () => {
  let p = startInstall("download");
  const start = p;
  p = applyInstallEvent(p, { type: "stage", stage: "downloading" });
  p = applyInstallEvent(p, { type: "download_progress", bytes_done: 10, bytes_total: 100 });
  p = applyInstallEvent(p, { type: "stage", stage: "downloading" });
  p = applyInstallEvent(p, { type: "download_progress", bytes_done: 55, bytes_total: 100 });
  assert.deepEqual(p.stages, ["downloading"]);
  assert.deepEqual(p.download, { done: 55, total: 100 });
  for (let i = 1; i <= 7; i++) p = applyInstallEvent(p, { type: "message", text: `m${i}` });
  assert.equal(p.messages.length, MAX_MESSAGES);
  assert.equal(p.messages[MAX_MESSAGES - 1], "m7");
  assert.deepEqual(start, startInstall("download"), "progress objects are never mutated (they live in the store)");
});

test("install steps while downloading, after success and after a failed hash check", () => {
  let p = startInstall("download");
  p = applyInstallEvent(p, { type: "stage", stage: "downloading" });
  assert.equal(steps(p), "downloading:current verifying:pending extracting:pending hashing:pending introspecting:pending done:pending");
  p = applyInstallEvent(p, { type: "stage", stage: "verifying" });
  const failed = finishInstall(p, { code: "hash_mismatch", message: "The asset's SHA-256 does not match the pinned value.", detail: null });
  assert.equal(steps(failed), "downloading:done verifying:failed");
  assert.equal(failed.running, false);
  for (const stage of /** @type {const} */ (["extracting", "hashing", "introspecting", "done"])) p = applyInstallEvent(p, { type: "stage", stage });
  const ok = finishInstall(p, null);
  assert.equal(steps(ok), "downloading:done verifying:done extracting:done hashing:done introspecting:done done:done");
  assert.equal(ok.error, null);
});

test("an offline import has no download step; a failure before any stage marks the first one failed", () => {
  const p = applyInstallEvent(startInstall("import"), { type: "stage", stage: "verifying" });
  assert.equal(steps(p), "verifying:current extracting:pending hashing:pending introspecting:pending done:pending");
  const early = finishInstall(startInstall("download"), { code: "download_failed", message: "offline", detail: null });
  assert.equal(steps(early), "downloading:failed");
});
