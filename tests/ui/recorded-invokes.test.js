// @ts-check
// The E2 invoke recording (tests/ui/ipc-flow.js): it covers every command, sends channels as Tauri
// does, and the committed tests/ui/recorded-invokes.json (which the Rust replay reads) is current.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { contractCommands } from "./helpers.js";
import { recordInvokes } from "./ipc-flow.js";

/** Commands that take an `onEvent` channel (CONTRACTS.md §10, §13.5). */
const WITH_CHANNEL = new Set(["tool_install", "tool_import", "run_start", "job_attach", "acq_start"]);

// The flow is deterministic (its channel and job counters are its own), so one recording serves every test.
const recorded = await recordInvokes();

test("the recorded flow calls every command of CONTRACTS.md §10 and §13.5", async () => {
  const commands = contractCommands();
  const called = new Set(recorded.map((r) => r.cmd));
  assert.deepEqual([...commands].filter((c) => !called.has(c)), []);
  assert.deepEqual([...called].filter((c) => !commands.includes(c)), []);
});

test("arguments are sent as the Rust handlers take them", async () => {
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
  const committed = JSON.parse(readFileSync(new URL("./recorded-invokes.json", import.meta.url), "utf8"));
  assert.deepEqual(committed, recorded);
});
