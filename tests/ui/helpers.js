// @ts-check
// Helpers shared by the UI test files (not a test file itself, like ipc-flow.js).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { createStore } from "../../ui/lib/store.js";

/** @typedef {import("../../ui/lib/context").AppState} AppState */

const readContracts = () => readFileSync(new URL("../../docs/CONTRACTS.md", import.meta.url), "utf8");

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

/**
 * The commands of CONTRACTS.md §10 and §13.5, in document order; asserts there are 38.
 * @returns {string[]}
 */
export function contractCommands() {
  const doc = readContracts();
  const commands = [...commandsIn(doc, /\n## 10\. IPC commands/), ...commandsIn(doc, /\n### 13\.5 /)];
  assert.equal(commands.length, 38);
  return commands;
}

/**
 * The values of an enumeration row in CONTRACTS.md, in document order.
 * @param {string} name
 * @returns {string[]}
 */
export function contractValues(name) {
  const row = new RegExp(`^\\| \`${name}\` \\| (.+) \\|$`, "m").exec(readContracts());
  assert.ok(row, `${name} row not found`);
  return [...row[1].matchAll(/`([a-z_]+)`/g)].map((m) => m[1]);
}

/**
 * An app store with nothing loaded, changed by `patch`.
 * @param {Partial<AppState>} [patch]
 */
export function appStore(patch = {}) {
  return createStore(/** @type {AppState} */ ({ mode: "mock", appInfo: null, settings: null, tools: null, activeJob: null, timezones: null, ...patch }));
}

/**
 * The mock (ui-dev/mock.js) with 1 ms steps. Without `flags`, the instance every plain import of
 * mock.js in this test file shares; with `flags` (even ""), a fresh instance with those scenario
 * flags (its state and flags are per module load).
 * @param {string} [flags]
 * @returns {Promise<typeof import("../../ui-dev/mock.js")>}
 */
export async function loadMock(flags) {
  const g = /** @type {any} */ (globalThis);
  g.__SUITEDFIR_MOCK_TICK_MS = 1;
  if (flags === undefined) return import("../../ui-dev/mock.js");
  g.location = { search: `?mock&scenario=${flags}` };
  try {
    return await import(new URL(`../../ui-dev/mock.js?flags=${flags}`, import.meta.url).href);
  } finally {
    delete g.location;
  }
}
