// @ts-check
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import * as ipc from "../../ui/api/ipc.js";
import * as mock from "../../ui-dev/mock.js";

// Type-level parity: tsc checks every mock function against the ipc.js signature.
/** @type {typeof ipc} */
const typedMock = mock;

/** @param {object} mod */
const functionNames = (mod) =>
  Object.entries(mod)
    .filter(([, value]) => typeof value === "function")
    .map(([name]) => name)
    .sort();

test("ipc.js and mock.js export identical function names, and nothing else", () => {
  assert.deepEqual(functionNames(typedMock), functionNames(ipc));
  assert.deepEqual(Object.keys(mock).sort(), functionNames(mock));
  assert.deepEqual(Object.keys(ipc).sort(), functionNames(ipc));
});

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

test("the API has exactly the commands of CONTRACTS.md §10 and §13.5, plus the two dialogs", () => {
  const doc = readFileSync(new URL("../../docs/CONTRACTS.md", import.meta.url), "utf8");
  const commands = [...commandsIn(doc, /\n## 10\. IPC commands/), ...commandsIn(doc, /\n### 13\.5 /)];
  assert.equal(commands.length, 38);
  assert.deepEqual(functionNames(ipc), [...commands, "dialog_open", "dialog_save"].sort());
});
