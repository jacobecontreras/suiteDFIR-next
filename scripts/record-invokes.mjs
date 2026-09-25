// Writes tests/ui/recorded-invokes.json: every invoke that ui/api/ipc.js makes in the scripted
// flow of tests/ui/ipc-flow.js (ROADMAP E2). The Rust replay test in src-tauri replays the file
// through the real command handlers; tests/ui/recorded-invokes.test.js keeps it current.
//
// Usage: node scripts/record-invokes.mjs
import { writeFileSync } from "node:fs";

import { recordInvokes } from "../tests/ui/ipc-flow.js";

const out = new URL("../tests/ui/recorded-invokes.json", import.meta.url);
const recorded = await recordInvokes();
writeFileSync(out, `${JSON.stringify(recorded, null, 2)}\n`);
console.log(`wrote ${recorded.length} invokes to ${out.pathname}`);
