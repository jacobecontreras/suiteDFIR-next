// @ts-check
// The E2 invoke recording (tests/ui/ipc-flow.js): it covers every command, sends channels as Tauri
// does, and the committed tests/ui/recorded-invokes.json (which the Rust replay reads) is current.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { recordInvokes } from "./ipc-flow.js";

/**
 * The command names in the first column of the tables in a CONTRACTS.md section.
 * @param {string} doc
 * @param {RegExp} heading
 * @returns {string[]}
 */
function commandsIn(doc, heading) {
  const start = doc.search(heading);
  assert.ok(start >= 0, `heading ${heading} not found`);
  const rest = doc.slice(start + 1);
  const next = rest.search(/\n#{2,3} /);
  const section = next === -1 ? rest : rest.slice(0, next);
  return [...section.matchAll(/^\| `([a-z_]+)` \|/gm)].map((m) => m[1]);
}

/** Commands that take an `onEvent` channel (CONTRACTS.md §10, §13.5). */
const WITH_CHANNEL = new Set(["tool_install", "tool_import", "run_start", "job_attach", "acq_start"]);

test("the recorded flow calls every command of CONTRACTS.md §10 and §13.5", async () => {
  const recorded = await recordInvokes();
  const doc = readFileSync(new URL("../../docs/CONTRACTS.md", import.meta.url), "utf8");
  const commands = [...commandsIn(doc, /\n## 10\. IPC commands/), ...commandsIn(doc, /\n### 13\.5 /)];
  assert.equal(commands.length, 38);
  const called = new Set(recorded.map((r) => r.cmd));
  assert.deepEqual([...commands].filter((c) => !called.has(c)), []);
  assert.deepEqual([...called].filter((c) => !commands.includes(c)), []);
});

test("arguments are sent as the Rust handlers take them", async () => {
  const recorded = await recordInvokes();
  const channels = new Set();
  for (const { cmd, args } of recorded) {
    const keys = Object.keys(args).sort();
    if (WITH_CHANNEL.has(cmd)) {
      // `onEvent` is the Rust parameter `on_event`; Tauri serializes a Channel as `__CHANNEL__:<id>`.
      assert.deepEqual(keys, ["onEvent", "req"], cmd);
      assert.match(String(args.onEvent), /^__CHANNEL__:\d+$/);
      assert.ok(!channels.has(args.onEvent), `${cmd} reuses a channel`);
      channels.add(args.onEvent);
    } else {
      assert.ok(keys.length === 0 || (keys.length === 1 && keys[0] === "req"), `${cmd}: ${keys}`);
    }
  }
});

test("tests/ui/recorded-invokes.json is current (node scripts/record-invokes.mjs)", async () => {
  const recorded = await recordInvokes();
  const committed = JSON.parse(readFileSync(new URL("./recorded-invokes.json", import.meta.url), "utf8"));
  assert.deepEqual(committed, recorded);
});
