// @ts-check
// Mock-mode UI screenshots, light and dark (DEVELOPMENT.md §4.8, §5). Needs Playwright with
// Chromium, so it runs on a machine with a browser and is not part of `npm test`:
//
//   node tests/ui/e2e/shots.mjs --root <dir> --out <dir> [--screens <name,name,...>]
//
// It starts `<root>/scripts/serve-ui.mjs --root <root> --port 0` itself, reads the port from the
// server's first stdout line, and stops the server in `finally` (a background server would not
// survive a separate ssh session). Each screen is loaded fresh from /?mock (the mock's state is per
// page load), set up, and saved as <out>/<screen>-<light|dark>.png.
//
// It also runs behavior checks (`check-*`, no screenshot), e.g. that Enter in a New run field never
// starts a run and that cancelling needs a confirmation. It fails (exit 1) on a failed check, on any
// console CSP violation, console error or uncaught page error (screens, checks and measurements
// alike), and when a measurement misses its budget:
// - `perf`: render + filter of 1,300 fixture modules in the module picker, under 100 ms
//   (<out>/perf.json);
// - `perf-log`: the Run screen with a 100,000-line mock run; no long task, scroll jump or scroll
//   frame reaches 100 ms, and the log stays virtualized (<out>/perf-log.json). Then the log search
//   (S2) on it: no query, next-match step, "Only matching lines" toggle, jump, scroll frame or long
//   task reaches 100 ms, and the match counts and shown rows are right (`search` in perf-log.json).
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { chromium } from "playwright";

/** @typedef {import("playwright").Page} Page */

const USAGE = "usage: node tests/ui/e2e/shots.mjs --root <dir> --out <dir> [--screens <name,name,...>]";
const NIGHTJAR = "/Users/examiner/Documents/suiteDFIR Cases/Operation Nightjar";
const MISSING = "/Volumes/Archive/suiteDFIR Cases/Riverside 2025";
const HARBOR = "/Users/examiner/Documents/suiteDFIR Cases/Harbor Lights";
const PERF_BUDGET_MS = 100;
/** "Stays interactive" at 100,000 log lines: no main-thread task, jump or frame reaches 100 ms. */
const LOG_BUDGET_MS = 100;

/**
 * @typedef {object} Screen
 * @property {string} name
 * @property {string} query The URL query, e.g. "?mock&scenario=empty".
 * @property {string} hash
 * @property {(page: Page) => Promise<void>} setup Waits for the state to be on screen.
 * @property {string} [element] Screenshot only this element instead of the full page.
 * @property {boolean} [viewport] Screenshot the viewport (for modal dialogs) instead of the full page.
 */

/** @param {string} path */
const caseHash = (path) => `#/case?path=${encodeURIComponent(path)}`;
const newRunHash = `#/new-run?case=${encodeURIComponent(NIGHTJAR)}`;

/**
 * @param {Page} page
 * @param {string} buttonName "Choose file…" or "Choose folder…"
 * @param {string} sample Text of the sample path to pick in the mock picker.
 */
async function pickInput(page, buttonName, sample) {
  await page.getByRole("button", { name: buttonName }).click();
  await page.locator("dialog.mock-picker button", { hasText: sample }).first().click();
  await page.locator("dialog.mock-picker").waitFor({ state: "detached" });
}

/** @param {Page} page */
async function newRunReady(page) {
  await page.locator(".start-reasons").waitFor();
  await page.getByRole("radio", { name: /Custom selection/ }).waitFor();
}

/** @param {Page} page */
async function customMode(page) {
  await newRunReady(page);
  await page.getByRole("radio", { name: /Custom selection/ }).check();
  await page.locator(".picker").waitFor();
}

/**
 * Starts a run from New run on a sample input and waits for the Run screen.
 * @param {Page} page
 * @param {string} buttonName "Choose file…" or "Choose folder…"
 * @param {string} sample
 */
async function startRun(page, buttonName, sample) {
  await newRunReady(page);
  await pickInput(page, buttonName, sample);
  await page.locator(".ready-text").waitFor();
  await page.getByRole("button", { name: "Start run" }).click();
  await page.locator(".run-screen .phase-steps").waitFor();
}

/**
 * Waits until the log view holds at least `n` lines.
 * @param {Page} page
 * @param {number} n
 * @param {number} [timeoutMs]
 */
async function waitForLines(page, n, timeoutMs = 60000) {
  const until = Date.now() + timeoutMs;
  for (;;) {
    const count = await page.evaluate(() => Number(document.querySelector(".log-viewport")?.getAttribute("data-count") ?? 0));
    if (count >= n) return;
    if (Date.now() > until) throw new Error(`the log has ${count} lines, expected at least ${n}`);
    await page.waitForTimeout(100);
  }
}

/**
 * Waits for the Run screen's result panel with the given status label.
 * @param {Page} page
 * @param {string} status e.g. "Succeeded"
 */
async function runResult(page, status) {
  await page.locator(".result-card:not([hidden]) .card-head .badge", { hasText: status }).waitFor({ timeout: 30000 });
}

/** @param {string} path */
const acquireHash = (path) => `#/acquire?case=${encodeURIComponent(path)}`;

/**
 * Waits for the Acquire screen's device section (devices, a tools problem, or none connected).
 * @param {Page} page
 */
async function acquireReady(page) {
  await page.locator(".device-list, .tools-banner, .empty-state-sm").first().waitFor();
}

/**
 * Ticks "Enable backup encryption" and types the password twice.
 * @param {Page} page
 * @param {string} [password]
 * @param {string} [again]
 */
async function enableEncryption(page, password = "examiner-pw", again = password) {
  await page.getByLabel("Enable backup encryption (recommended)").check();
  await page.getByLabel(/^Backup password/).fill(password);
  await page.getByLabel("Password again").fill(again);
}

/**
 * On the Acquire screen with the iPhone selected: optional label, optional encryption (with "Parse
 * with iLEAPP now"), then Start; waits for the progress view.
 * @param {Page} page
 * @param {{ label?: string, encrypt?: boolean, keep?: boolean }} [o]
 */
async function startAcquisition(page, o = {}) {
  await acquireReady(page);
  await page.getByRole("radio", { name: "Alex's iPhone" }).waitFor();
  if (o.label) await page.getByLabel("Label").fill(o.label);
  if (o.encrypt) {
    await enableEncryption(page);
    if (o.keep) await page.getByLabel("Parse with iLEAPP now").check();
  }
  await page.locator(".ready-text").waitFor();
  await page.getByRole("button", { name: "Start acquisition" }).click();
  await page.locator(".acquire-screen .phase-steps").waitFor();
}

/**
 * @param {Page} page
 * @param {string} status e.g. "Succeeded"
 */
async function acqResult(page, status) {
  await page.locator(".result-card:not([hidden]) .card-head .badge", { hasText: status }).waitFor({ timeout: 30000 });
}

/**
 * The Acquire screen of Operation Nightjar with mock flags.
 * @param {string} name
 * @param {string} flags
 * @param {string} [element]
 * @returns {Screen}
 */
function acquireToolsScreen(name, flags, element) {
  return {
    name,
    query: `?mock&scenario=${flags}`,
    hash: acquireHash(NIGHTJAR),
    element,
    setup: acquireReady,
  };
}

/**
 * A Run screen held in `phase` by the mock's `hold_<phase>` flag.
 * @param {string} phase
 * @param {string} label The phase's label in the step list.
 * @param {string} buttonName
 * @param {string} sample
 * @param {string} [progress] The label of a progress bar to wait for (held part-way).
 * @returns {Screen}
 */
function runPhaseScreen(phase, label, buttonName, sample, progress) {
  return {
    name: `run-phase-${phase.replaceAll("_", "-")}`,
    query: `?mock&scenario=hold_${phase}`,
    hash: newRunHash,
    setup: async (page) => {
      await startRun(page, buttonName, sample);
      await page.locator(".phase-step-current", { hasText: label }).waitFor();
      if (progress) await page.locator(".progress-label", { hasText: progress }).waitFor();
    },
  };
}

/**
 * After a reload in the middle of a phase: the mock's start-up job (`active_run` / `active_acq`)
 * is held in `phase`, the top bar shows it (from job_active), and only then is the job's screen
 * opened, so `job_attach` returns just the backlog and no phase event follows. The screen must
 * show the phase from its first render; `assert` checks the controls that depend on it.
 * @param {string} name
 * @param {string} flags
 * @param {string} label The phase's label.
 * @param {(page: Page) => Promise<void>} [assert]
 * @returns {Screen}
 */
function attachedMidPhaseScreen(name, flags, label, assert) {
  return {
    name,
    query: `?mock&scenario=${flags}`,
    hash: "#/cases",
    setup: async (page) => {
      await page.locator(".job-indicator .job-phase", { hasText: label }).waitFor({ timeout: 20000 });
      await page.locator(".job-indicator").click();
      await page.locator(".phase-steps").waitFor();
      // The first render: no later phase event can have changed it (the job is held).
      const current = await page.locator(".phase-step-current").textContent({ timeout: 1000 });
      if (!current?.includes(label)) throw new Error(`attached mid-phase, the current step is ${JSON.stringify(current)}, expected ${label}`);
      if (assert) await assert(page);
    },
  };
}

/**
 * Waits until the log search reports `text` ("" = no search).
 * @param {Page} page
 * @param {string} text
 * @param {number} [timeoutMs]
 */
async function searchStatus(page, text, timeoutMs = 10000) {
  const until = Date.now() + timeoutMs;
  for (;;) {
    const now = await page.evaluate(() => document.querySelector(".log-search-status")?.textContent ?? null);
    if (now === text) return;
    if (Date.now() > until) throw new Error(`the log search says ${JSON.stringify(now)}, expected ${JSON.stringify(text)}`);
    await page.waitForTimeout(50);
  }
}

/**
 * The Run screen's log after the 100,000-line flood run (finished, so the log no longer moves),
 * searched for `query` (S2).
 * @param {string} name
 * @param {string} query
 * @param {string} status The search status to wait for.
 * @param {(page: Page) => Promise<void>} [then]
 * @returns {Screen}
 */
function logSearchScreen(name, query, status, then) {
  return {
    name,
    query: "?mock",
    hash: newRunHash,
    element: ".log-view",
    setup: async (page) => {
      await startRun(page, "Choose folder…", "Evidence/flood");
      await waitForLines(page, 100_000);
      await runResult(page, "Succeeded");
      await page.getByRole("searchbox", { name: "Search the log" }).fill(query);
      await searchStatus(page, status);
      if (then) await then(page);
    },
  };
}

/** @type {Screen[]} */
const SCREENS = [
  // ---- D1: shell and chrome ----
  {
    name: "shell-empty",
    query: "?mock&scenario=empty,no_tools",
    hash: "#/cases",
    setup: async (page) => {
      await page.locator(".empty-state").waitFor();
    },
  },
  {
    name: "shell-banners-active-job",
    query: "?mock&scenario=dev_override,active_run",
    hash: "#/cases",
    setup: async (page) => {
      await page.locator(".job-indicator").waitFor();
      await page.locator(".banner-dev").waitFor();
      await page.locator(".case-list").waitFor();
    },
  },
  {
    name: "app-error",
    query: "?mock",
    hash: caseHash(MISSING),
    setup: async (page) => {
      await page.locator(".app-error").waitFor();
      await page.locator(".app-error-detail summary").click();
    },
  },
  {
    name: "no-api",
    query: "",
    hash: "",
    setup: async (page) => {
      await page.locator(".app-error").waitFor();
    },
  },
  // ---- D2: Cases and Case ----
  {
    name: "cases-recent",
    query: "?mock",
    hash: "#/cases",
    setup: async (page) => {
      await page.locator(".case-list").waitFor();
    },
  },
  {
    name: "cases-missing-folder",
    query: "?mock",
    hash: "#/cases",
    element: ".case-item-missing",
    setup: async (page) => {
      await page.locator(".case-item-missing").waitFor();
    },
  },
  {
    name: "cases-empty",
    query: "?mock&scenario=empty",
    hash: "#/cases",
    setup: async (page) => {
      await page.locator(".empty-state").waitFor();
    },
  },
  {
    name: "cases-no-tools",
    query: "?mock&scenario=no_tools",
    hash: "#/cases",
    setup: async (page) => {
      await page.locator(".banner-warn").waitFor();
      await page.locator(".case-list").waitFor();
    },
  },
  {
    name: "case-new-validation",
    query: "?mock",
    hash: "#/cases",
    viewport: true,
    setup: async (page) => {
      await page.locator(".case-list").waitFor();
      await page.locator(".screen-head").getByRole("button", { name: "New case" }).click();
      await page.getByRole("button", { name: "Create case" }).click();
      await page.locator(".field-error:not([hidden])").waitFor();
    },
  },
  {
    name: "case-edit",
    query: "?mock",
    hash: caseHash(NIGHTJAR),
    setup: async (page) => {
      await page.getByRole("button", { name: "Edit" }).click();
      await page.getByRole("button", { name: "Save changes" }).waitFor();
    },
  },
  {
    name: "case-runs",
    query: "?mock",
    hash: caseHash(NIGHTJAR),
    setup: async (page) => {
      await page.locator(".runs-table").waitFor();
    },
  },
  {
    // UTC shown on keyboard focus (Tab from the first row's label link to its time).
    name: "case-runs-utc-focus",
    query: "?mock",
    hash: caseHash(NIGHTJAR),
    element: ".table-wrap",
    setup: async (page) => {
      await page.locator(".runs-table").waitFor();
      await page.locator(".runs-table tbody tr").first().locator(".cell-label a").press("Tab");
      await page.locator(".runs-table time:focus").waitFor();
    },
  },
  {
    name: "case-empty-runs",
    query: "?mock",
    hash: caseHash(HARBOR),
    setup: async (page) => {
      await page.getByText("No runs yet.").waitFor();
    },
  },
  {
    // A run whose app "crashes" (…/interrupt) is marked interrupted when the case is opened again.
    name: "case-recovered-notice",
    query: "?mock",
    hash: newRunHash,
    setup: async (page) => {
      await startRun(page, "Choose folder…", "Evidence/interrupt");
      await page.locator(".job-indicator").waitFor({ state: "detached", timeout: 15000 });
      await page.getByRole("link", { name: "Cases", exact: true }).first().click();
      await page.locator(".case-list").waitFor();
      await page.getByRole("link", { name: "Operation Nightjar" }).click();
      await page.locator(".banner-info").waitFor();
    },
  },
  {
    name: "run-details",
    query: "?mock",
    hash: caseHash(NIGHTJAR),
    viewport: true,
    setup: async (page) => {
      await page.locator(".runs-table").waitFor();
      await page.getByRole("button", { name: "Details" }).first().click();
      await page.locator(".detail-groups").waitFor();
    },
  },
  {
    name: "run-details-raw-json",
    query: "?mock",
    hash: caseHash(NIGHTJAR),
    viewport: true,
    setup: async (page) => {
      await page.locator(".runs-table").waitFor();
      await page.getByRole("button", { name: "Details" }).first().click();
      await page.locator(".raw-json summary").click();
      await page.locator(".raw-json pre").waitFor();
      await page.evaluate(() => document.querySelector(".raw-json summary")?.scrollIntoView());
    },
  },
  // ---- D3: New run ----
  {
    name: "newrun-sections",
    query: "?mock",
    hash: newRunHash,
    setup: newRunReady,
  },
  {
    name: "newrun-inspection",
    query: "?mock",
    hash: newRunHash,
    setup: async (page) => {
      await newRunReady(page);
      await pickInput(page, "Choose folder…", "iPhone-11-backup");
      await page.getByLabel("Input type").waitFor();
    },
  },
  {
    name: "newrun-overlap-error",
    query: "?mock",
    hash: newRunHash,
    setup: async (page) => {
      await newRunReady(page);
      await pickInput(page, "Choose folder…", "Operation Nightjar/runs");
      await page.locator(".form-section .app-error").waitFor();
    },
  },
  {
    name: "newrun-permission-error",
    query: "?mock",
    hash: newRunHash,
    element: ".form-section:nth-of-type(2)",
    setup: async (page) => {
      await newRunReady(page);
      await pickInput(page, "Choose folder…", "Evidence/denied");
      await page.locator(".form-section .app-error").waitFor();
    },
  },
  {
    name: "newrun-encrypted-password",
    query: "?mock",
    hash: newRunHash,
    setup: async (page) => {
      await newRunReady(page);
      await pickInput(page, "Choose folder…", "00008101-000A1B2C3D4E");
      await page.getByLabel(/Backup password/).waitFor();
    },
  },
  {
    // A backup whose encryption cannot be read needs a password as if encrypted (K8 owner decision).
    name: "newrun-encryption-unknown",
    query: "?mock",
    hash: newRunHash,
    setup: async (page) => {
      await newRunReady(page);
      await pickInput(page, "Choose folder…", "iPad-backup-encryption-unknown");
      await page.getByLabel(/Backup password/).waitFor();
      await page.getByText("The backup's encryption state couldn't be read, so a password is needed.", { exact: false }).first().waitFor();
    },
  },
  {
    name: "newrun-picker-search",
    query: "?mock",
    hash: newRunHash,
    setup: async (page) => {
      await customMode(page);
      await page.getByRole("searchbox", { name: "Search modules" }).fill("call history");
      await page.locator(".picker-row label", { hasText: "Call History" }).first().click();
    },
  },
  {
    name: "newrun-picker-tristate",
    query: "?mock",
    hash: newRunHash,
    element: ".picker",
    setup: async (page) => {
      await customMode(page);
      const groups = page.locator(".picker-group");
      // Group 1: partially selected (two modules).
      await groups.nth(0).locator(".picker-toggle").click();
      await groups.nth(0).locator(".picker-row input").nth(0).check();
      await groups.nth(0).locator(".picker-row input").nth(2).check();
      // Group 2: fully selected with the group checkbox (left collapsed); the others stay empty.
      await groups.nth(1).locator(".picker-group-label input").check();
    },
  },
  {
    name: "newrun-profile-unknown",
    query: "?mock",
    hash: newRunHash,
    setup: async (page) => {
      await newRunReady(page);
      await page.getByRole("radio", { name: /Saved profile/ }).check();
      await page.getByRole("button", { name: "Edit as custom selection" }).waitFor();
    },
  },
  {
    name: "newrun-unknown-modules",
    query: "?mock",
    hash: newRunHash,
    setup: async (page) => {
      await newRunReady(page);
      await page.getByRole("radio", { name: /Saved profile/ }).check();
      await page.getByRole("button", { name: "Edit as custom selection" }).click();
      await page.getByRole("button", { name: "Remove unknown" }).waitFor();
    },
  },
  {
    name: "newrun-start-disabled",
    query: "?mock",
    hash: newRunHash,
    element: ".start-card",
    setup: async (page) => {
      await newRunReady(page);
      await pickInput(page, "Choose folder…", "00008101-000A1B2C3D4E");
      await page.getByLabel(/Backup password/).waitFor();
      await page.getByRole("radio", { name: /Custom selection/ }).check();
      await page.locator(".picker").waitFor();
    },
  },
  {
    name: "newrun-aleapp-file",
    query: "?mock",
    hash: newRunHash,
    setup: async (page) => {
      await newRunReady(page);
      await page.getByRole("radio", { name: /aLEAPP/ }).check();
      await page.getByRole("radio", { name: /All modules \(1,288\)/ }).waitFor();
      await pickInput(page, "Choose file…", "Pixel-7.tar");
      await page.getByText("Hash the input file (SHA-256)").waitFor();
    },
  },
  {
    name: "newrun-save-profile",
    query: "?mock",
    hash: newRunHash,
    viewport: true,
    setup: async (page) => {
      await customMode(page);
      await page.locator(".picker-group").nth(1).locator(".picker-group-label input").check();
      await page.getByRole("button", { name: "Save as profile…" }).click();
      await page.getByLabel(/Profile name/).fill("AirDrop only");
    },
  },
  {
    name: "newrun-profile-saved",
    query: "?mock",
    hash: newRunHash,
    element: ".form-section:nth-of-type(4)",
    setup: async (page) => {
      await customMode(page);
      await page.locator(".picker-group").nth(1).locator(".picker-group-label input").check();
      await page.getByRole("button", { name: "Save as profile…" }).click();
      await page.getByLabel(/Profile name/).fill("AirDrop only");
      await page.getByRole("button", { name: "Save profile" }).click();
      await page.getByText("Saved profile “AirDrop only”").waitFor();
    },
  },
  {
    name: "newrun-profile-imported",
    query: "?mock",
    hash: newRunHash,
    element: ".form-section:nth-of-type(4)",
    setup: async (page) => {
      await newRunReady(page);
      await page.getByRole("radio", { name: /Saved profile/ }).check();
      await page.getByRole("button", { name: "Import…" }).click();
      await page.locator("dialog.mock-picker button", { hasText: "Triage.ilprofile" }).click();
      await page.locator("dialog.mock-picker").waitFor({ state: "detached" });
      await page.getByText("Imported profile “Triage”").waitFor();
    },
  },
  {
    name: "newrun-import-replace",
    query: "?mock",
    hash: newRunHash,
    viewport: true,
    setup: async (page) => {
      await newRunReady(page);
      await page.getByRole("radio", { name: /Saved profile/ }).check();
      await page.getByRole("button", { name: "Import…" }).click();
      await page.locator("dialog.mock-picker button", { hasText: "Messaging.ilprofile" }).click();
      await page.getByRole("button", { name: "Replace" }).waitFor();
    },
  },
  {
    name: "newrun-profile-exported",
    query: "?mock",
    hash: newRunHash,
    element: ".form-section:nth-of-type(4)",
    setup: async (page) => {
      await newRunReady(page);
      await page.getByRole("radio", { name: /Saved profile/ }).check();
      await page.getByRole("button", { name: "Export…" }).click();
      await page.locator("dialog.mock-picker button", { hasText: "Desktop" }).click();
      await page.getByText("Exported “Messaging”").waitFor();
    },
  },
  {
    name: "newrun-started",
    query: "?mock",
    hash: newRunHash,
    setup: async (page) => {
      await newRunReady(page);
      await pickInput(page, "Choose folder…", "00008101-000A1B2C3D4E");
      await page.getByLabel(/Backup password/).fill("examiner-secret");
      await page.getByLabel("Label").fill("Encrypted backup, second pass");
      await page.getByRole("button", { name: "Start run" }).click();
      await page.locator(".run-screen .phase-steps").waitFor();
      await page.locator(".job-indicator").waitFor();
    },
  },
  {
    name: "newrun-ready",
    query: "?mock",
    hash: newRunHash,
    setup: async (page) => {
      await newRunReady(page);
      await pickInput(page, "Choose file…", "iPhone-12-FFS.zip");
      await page.locator(".ready-text").waitFor();
    },
  },
  // ---- D4a: Run screen, every phase and every RunStatus ----
  runPhaseScreen("preparing", "Preparing", "Choose folder…", "Pixel-7-extraction"),
  {
    name: "run-phase-running",
    query: "?mock",
    hash: newRunHash,
    setup: async (page) => {
      await startRun(page, "Choose folder…", "Evidence/slow");
      await waitForLines(page, 40);
    },
  },
  runPhaseScreen("hashing_input", "Hashing input", "Choose file…", "iPhone-12-FFS.zip"),
  runPhaseScreen("analyzing", "Analyzing", "Choose folder…", "Pixel-7-extraction"),
  runPhaseScreen("sealing_report", "Sealing report", "Choose folder…", "Pixel-7-extraction", "Sealing the report"),
  runPhaseScreen("finalizing", "Finalizing", "Choose folder…", "Pixel-7-extraction"),
  {
    name: "run-cancel-confirm",
    query: "?mock",
    hash: newRunHash,
    viewport: true,
    setup: async (page) => {
      await startRun(page, "Choose folder…", "Evidence/slow");
      await waitForLines(page, 10);
      await page.locator(".screen-head").getByRole("button", { name: "Cancel run" }).click();
      await page.locator("dialog").getByRole("button", { name: "Keep running" }).waitFor();
    },
  },
  {
    name: "run-succeeded",
    query: "?mock",
    hash: newRunHash,
    setup: async (page) => {
      await startRun(page, "Choose file…", "iPhone-12-FFS.zip");
      await runResult(page, "Succeeded");
      await page.getByText(/complete, 0 error/).waitFor();
    },
  },
  {
    name: "run-completed-with-errors",
    query: "?mock",
    hash: newRunHash,
    setup: async (page) => {
      await startRun(page, "Choose folder…", "Evidence/errors");
      await runResult(page, "Completed with errors");
      await page.getByText("Modules with errors").waitFor();
    },
  },
  {
    name: "run-failed",
    query: "?mock",
    hash: newRunHash,
    setup: async (page) => {
      await startRun(page, "Choose folder…", "Evidence/fail-crash");
      await runResult(page, "Failed");
    },
  },
  {
    name: "run-cancelled",
    query: "?mock",
    hash: newRunHash,
    setup: async (page) => {
      await startRun(page, "Choose folder…", "Evidence/slow");
      await waitForLines(page, 20);
      await page.locator(".screen-head").getByRole("button", { name: "Cancel run" }).click();
      await page.locator("dialog").getByRole("button", { name: "Cancel run" }).click();
      await runResult(page, "Cancelled");
    },
  },
  {
    name: "run-interrupted",
    query: "?mock",
    hash: caseHash(NIGHTJAR),
    setup: async (page) => {
      await page.getByRole("link", { name: "Before the power cut" }).click();
      await runResult(page, "Interrupted");
    },
  },
  // ---- D4b: Settings, every ToolState plus installing and install failed ----
  {
    name: "settings",
    query: "?mock",
    hash: "#/settings",
    setup: async (page) => {
      await page.locator(".tool-card").first().waitFor();
      await page.getByRole("button", { name: "Save defaults" }).waitFor();
    },
  },
  {
    // verified (iLEAPP) and installed_unverified (aLEAPP), then Verify on aLEAPP.
    name: "settings-tools-verify",
    query: "?mock",
    hash: "#/settings",
    element: ".tool-grid",
    setup: async (page) => {
      await page.locator(".tool-card", { hasText: "aLEAPP" }).getByRole("button", { name: "Verify" }).click();
      await page.getByText("Verified: the binary matches the pinned SHA-256.").waitFor();
    },
  },
  {
    name: "settings-tools-not-installed",
    query: "?mock&scenario=no_tools",
    hash: "#/settings",
    element: ".tool-grid",
    setup: async (page) => {
      await page.locator(".tool-card", { hasText: "Not installed" }).first().waitFor();
    },
  },
  {
    name: "settings-tools-failed-unsupported",
    query: "?mock&scenario=tool_verification_failed,tool_unsupported",
    hash: "#/settings",
    element: ".tool-grid",
    setup: async (page) => {
      await page.locator(".tool-card", { hasText: "Verification failed" }).waitFor();
      await page.locator(".tool-card", { hasText: "Not available" }).waitFor();
    },
  },
  {
    name: "settings-tools-dev-override",
    query: "?mock&scenario=dev_override",
    hash: "#/settings",
    element: ".tool-grid",
    setup: async (page) => {
      await page.locator(".tool-card", { hasText: "Dev override" }).first().waitFor();
    },
  },
  {
    name: "settings-installing",
    query: "?mock&scenario=no_tools",
    hash: "#/settings",
    element: ".tool-grid",
    setup: async (page) => {
      await page.locator(".tool-card", { hasText: "iLEAPP" }).getByRole("button", { name: /^Install/ }).click();
      await page.getByText(/^Downloading iLEAPP/).waitFor();
    },
  },
  {
    // While a parser installs (its introspection uses a temporary folder): no temp cleanup.
    name: "settings-storage-installing",
    query: "?mock&scenario=no_tools",
    hash: "#/settings",
    element: "section[aria-labelledby=settings-storage]",
    setup: async (page) => {
      await page.locator(".tool-card", { hasText: "iLEAPP" }).getByRole("button", { name: /^Install/ }).click();
      await page.getByText("Not while a parser is being installed.").waitFor();
      if (!(await page.getByRole("button", { name: "Clean temporary files" }).isDisabled())) throw new Error("Clean temporary files is enabled during an install");
    },
  },
  {
    name: "settings-install-failed",
    query: "?mock&scenario=no_tools,install_fail",
    hash: "#/settings",
    element: ".tool-grid",
    setup: async (page) => {
      await page.locator(".tool-card", { hasText: "iLEAPP" }).getByRole("button", { name: /^Install/ }).click();
      await page.locator(".install-progress .app-error").waitFor();
    },
  },
  {
    name: "settings-installed",
    query: "?mock&scenario=no_tools",
    hash: "#/settings",
    element: ".tool-grid",
    setup: async (page) => {
      await page.locator(".tool-card", { hasText: "iLEAPP" }).getByRole("button", { name: /^Import/ }).click();
      await page.locator("dialog.mock-picker button", { hasText: "ileapp-v2026.4.2-macOS_Apple_Silicon.zip" }).click();
      await page.getByText("Imported and verified.").waitFor({ timeout: 15000 });
    },
  },
  {
    // A tools-folder override (with "Use the default"), and a finished temp cleanup.
    name: "settings-storage",
    query: "?mock",
    hash: "#/settings",
    element: "section[aria-labelledby=settings-storage]",
    setup: async (page) => {
      await page.getByRole("group", { name: "Tools folder" }).getByRole("button", { name: "Change…" }).click();
      await page.locator("dialog.mock-picker button", { hasText: "/Applications/Approved Tools" }).click();
      await page.getByRole("button", { name: "Use the default" }).waitFor();
      await page.getByRole("button", { name: "Clean temporary files" }).click();
      await page.getByText(/^Removed .* of temporary files\.$/).waitFor();
    },
  },
  {
    name: "settings-about-licenses",
    query: "?mock",
    hash: "#/settings",
    element: "section[aria-labelledby=settings-about]",
    setup: async (page) => {
      await page.getByText("Third-party licenses").click();
      await page.locator(".licenses-text").waitFor();
    },
  },
  {
    // After a reload (here: a run already active when the page loads), job_attach returns the
    // backlog and the Run screen follows the run again.
    name: "run-attached",
    query: "?mock&scenario=active_run",
    hash: "#/cases",
    setup: async (page) => {
      await page.locator(".job-indicator").click();
      await page.locator(".run-screen .phase-steps").waitFor();
      await waitForLines(page, 10);
    },
  },
  // After a reload mid-phase: the phase comes from the active job, not from a phase event.
  attachedMidPhaseScreen("run-attached-mid-phase", "active_run,hold_analyzing", "Analyzing"),
  // ---- D5: Acquire ----
  {
    name: "acquire-devices",
    query: "?mock",
    hash: acquireHash(NIGHTJAR),
    setup: async (page) => {
      await acquireReady(page);
      await page.locator(".preflight-ok").waitFor();
    },
  },
  {
    name: "acquire-pair-states",
    query: "?mock&scenario=pair_states",
    hash: acquireHash(NIGHTJAR),
    element: ".device-list",
    setup: async (page) => {
      await acquireReady(page);
      await page.getByText("Trust denied").waitFor();
    },
  },
  {
    // Pair on the unpaired iPad: the device now shows the Trust dialog. As with the core, the next
    // polls report `not_paired` (no host pair record yet); the card keeps the Pair answer.
    name: "acquire-awaiting-trust",
    query: "?mock",
    hash: acquireHash(NIGHTJAR),
    element: ".device-list",
    setup: async (page) => {
      await acquireReady(page);
      const card = page.locator(".device-card", { hasText: "iPad13,4" });
      await card.getByRole("button", { name: "Pair" }).click();
      await card.getByText("Waiting for Trust").waitFor();
      await page.waitForTimeout(4500);
      await card.getByText("Waiting for Trust").waitFor({ timeout: 1000 });
      await card.getByRole("button", { name: "Retry pairing" }).waitFor({ timeout: 1000 });
    },
  },
  {
    // Retry after Trust: paired. The iPad's owner had turned backup encryption on.
    name: "acquire-encryption-already-on",
    query: "?mock",
    hash: acquireHash(NIGHTJAR),
    setup: async (page) => {
      await acquireReady(page);
      const card = page.locator(".device-card", { hasText: "iPad13,4" });
      await card.getByRole("button", { name: "Pair" }).click();
      await card.getByRole("button", { name: "Retry pairing" }).click();
      await page.getByRole("radio", { name: "Evidence iPad" }).check();
      await page.getByText("Backup encryption is already on for this device.").waitFor();
      await page.locator(".ready-text").waitFor();
    },
  },
  {
    name: "acquire-encryption-enable",
    query: "?mock",
    hash: acquireHash(NIGHTJAR),
    setup: async (page) => {
      await acquireReady(page);
      await page.getByLabel("Label").fill("Seized iPhone, item 7");
      await enableEncryption(page);
      await page.getByLabel("Parse with iLEAPP now").check();
      await page.locator(".ready-text").waitFor();
    },
  },
  {
    name: "acquire-password-mismatch",
    query: "?mock",
    hash: acquireHash(NIGHTJAR),
    element: ".acquire-screen",
    setup: async (page) => {
      await acquireReady(page);
      await enableEncryption(page, "examiner-pw", "examiner-pq");
      await page.getByLabel("Label").click();
      await page.locator(".field-error:not([hidden])").waitFor();
    },
  },
  {
    // On Windows the iOS tools take only printable ASCII passwords: the form says so at once.
    name: "acquire-password-not-ascii",
    query: "?mock&scenario=windows",
    hash: acquireHash(NIGHTJAR),
    element: ".acquire-screen",
    setup: async (page) => {
      await acquireReady(page);
      await enableEncryption(page, "Prüfer-2026", "Prüfer-2026");
      await page.getByText("On Windows the iOS tools can only use a password of plain ASCII").first().waitFor();
      if (!(await page.getByRole("button", { name: "Start acquisition" }).isDisabled())) throw new Error("Start is enabled with a non-ASCII password on Windows");
    },
  },
  {
    name: "acquire-encryption-unknown",
    query: "?mock&scenario=pair_states",
    hash: acquireHash(NIGHTJAR),
    element: "section[aria-labelledby=acq-options-heading]",
    setup: async (page) => {
      await acquireReady(page);
      await page.getByRole("radio", { name: "Loaner iPhone" }).check();
      await page.getByText("setting could not be read").waitFor();
    },
  },
  acquireToolsScreen("acquire-tools-missing", "idevice_missing"),
  acquireToolsScreen("acquire-tools-usbmuxd-unavailable", "usbmuxd_unavailable"),
  acquireToolsScreen("acquire-tools-verification-failed", "idevice_verification_failed"),
  acquireToolsScreen("acquire-tools-unsupported-platform", "idevice_unsupported"),
  acquireToolsScreen("acquire-no-devices", "no_devices"),
  // The tools are fine but listing the devices failed: state ok with guidance.
  acquireToolsScreen("acquire-tools-list-failed", "idevice_session_error"),
  {
    name: "acquire-preflight-warn",
    query: "?mock&scenario=preflight_warn",
    hash: acquireHash(NIGHTJAR),
    element: "section[aria-labelledby=acq-options-heading]",
    setup: async (page) => {
      await acquireReady(page);
      await page.locator(".preflight-warn").waitFor();
    },
  },
  {
    name: "acquire-preflight-block",
    query: "?mock&scenario=preflight_block",
    hash: acquireHash(NIGHTJAR),
    setup: async (page) => {
      await acquireReady(page);
      await page.locator(".preflight-block").waitFor();
    },
  },
  {
    // Another case's acquisition is running: its device is busy, and Start waits for the job.
    name: "acquire-busy-device",
    query: "?mock&scenario=active_acq",
    hash: acquireHash(HARBOR),
    setup: async (page) => {
      await acquireReady(page);
      await page.getByText("In use by the current acquisition.").waitFor();
    },
  },
  {
    // After a reload: job_attach and the backlog.
    name: "acquire-attached",
    query: "?mock&scenario=active_acq",
    hash: acquireHash(NIGHTJAR),
    setup: async (page) => {
      await page.locator(".acquire-screen .phase-steps").waitFor();
      await page.locator(".progress-block").waitFor();
    },
  },
  attachedMidPhaseScreen("acquire-attached-backing-up", "active_acq,hold_backing_up", "Backing up", async (page) => {
    const cancel = page.locator(".screen-head").getByRole("button", { name: "Cancel acquisition" });
    if (await cancel.isDisabled()) throw new Error("Cancel is disabled while backing up");
  }),
  attachedMidPhaseScreen("acquire-attached-restoring", "active_acq,hold_restoring_encryption", "Restoring encryption", async (page) => {
    // §6b: the encryption step, its no-time-limit note, and Cancel disabled with the reason.
    await page.getByText("Turning backup encryption off.").waitFor({ timeout: 1000 });
    await page.getByText("Cancel is not available in this step").waitFor({ timeout: 1000 });
    const cancel = page.locator(".screen-head").getByRole("button", { name: "Cancel acquisition" });
    if (!(await cancel.isDisabled())) throw new Error("Cancel is enabled while encryption is turned off again");
  }),
  {
    name: "acquire-prompt-encryption",
    query: "?mock&scenario=hold_enabling_encryption",
    hash: acquireHash(NIGHTJAR),
    setup: async (page) => {
      await startAcquisition(page, { encrypt: true, label: "Seized iPhone, item 7" });
      await page.locator(".banner-prompt").waitFor();
    },
  },
  {
    name: "acquire-prompt-backup",
    query: "?mock&scenario=hold_backing_up",
    hash: acquireHash(NIGHTJAR),
    setup: async (page) => {
      await startAcquisition(page, { label: "Seized iPhone, item 7" });
      await page.locator(".banner-prompt").waitFor();
    },
  },
  {
    name: "acquire-progress",
    query: "?mock",
    hash: acquireHash(NIGHTJAR),
    setup: async (page) => {
      await startAcquisition(page, { label: "Seized iPhone, item 7/slow" });
      await page.getByText(/^[4-9]%$/).waitFor({ timeout: 15000 });
    },
  },
  {
    name: "acquire-restoring-encryption",
    query: "?mock&scenario=hold_restoring_encryption",
    hash: acquireHash(NIGHTJAR),
    setup: async (page) => {
      await startAcquisition(page, { encrypt: true, label: "Seized iPhone, item 7" });
      await page.locator(".phase-step-current", { hasText: "Restoring encryption" }).waitFor({ timeout: 30000 });
    },
  },
  {
    name: "acquire-cancel-confirm",
    query: "?mock",
    hash: acquireHash(NIGHTJAR),
    viewport: true,
    setup: async (page) => {
      await startAcquisition(page, { label: "Seized iPhone, item 7/slow" });
      await page.locator(".progress-block").waitFor();
      await page.locator(".screen-head").getByRole("button", { name: "Cancel acquisition" }).click();
      await page.locator("dialog").getByRole("button", { name: "Keep going" }).waitFor();
    },
  },
  {
    name: "acquire-succeeded",
    query: "?mock",
    hash: acquireHash(NIGHTJAR),
    setup: async (page) => {
      await startAcquisition(page, { encrypt: true, keep: true, label: "Seized iPhone, item 7" });
      await acqResult(page, "Succeeded");
      await page.getByText("Turned on for the backup, then off again").waitFor();
    },
  },
  {
    name: "acquire-failed",
    query: "?mock",
    hash: acquireHash(NIGHTJAR),
    setup: async (page) => {
      await startAcquisition(page, { label: "Seized iPhone, item 7/backup_fail" });
      await acqResult(page, "Failed");
    },
  },
  {
    name: "acquire-cancelled",
    query: "?mock",
    hash: acquireHash(NIGHTJAR),
    setup: async (page) => {
      await startAcquisition(page, { label: "Seized iPhone, item 7/slow" });
      await page.locator(".progress-block").waitFor();
      await page.locator(".screen-head").getByRole("button", { name: "Cancel acquisition" }).click();
      await page.locator("dialog").getByRole("button", { name: "Cancel acquisition" }).click();
      await acqResult(page, "Cancelled");
    },
  },
  {
    // Turning encryption off after the backup failed: the warnings and the action.
    name: "acquire-restore-failed",
    query: "?mock",
    hash: acquireHash(NIGHTJAR),
    setup: async (page) => {
      await startAcquisition(page, { encrypt: true, label: "Seized iPhone, item 7/restore_fail" });
      await acqResult(page, "Succeeded");
      await page.getByText("Backup encryption may still be on for this device.").waitFor();
    },
  },
  {
    // The later restore, with a wrong password: encryption stays on.
    name: "acquire-restore-dialog-still-on",
    query: "?mock",
    hash: acquireHash(NIGHTJAR),
    viewport: true,
    setup: async (page) => {
      await startAcquisition(page, { encrypt: true, label: "Seized iPhone, item 7/restore_fail" });
      await acqResult(page, "Succeeded");
      await page.locator(".result-card").getByRole("button", { name: "Turn backup encryption off" }).click();
      await page.locator("dialog").getByLabel("Backup password").fill("wrong");
      await page.locator("dialog").getByRole("button", { name: "Turn encryption off" }).click();
      await page.getByText("Backup encryption is still on.").waitFor();
    },
  },
  {
    // A retry with the right password succeeds; the re-read AcqSummary no longer has the warning, so
    // the result stops offering the action (acquisition.json keeps its warnings).
    name: "acquire-restore-done",
    query: "?mock",
    hash: acquireHash(NIGHTJAR),
    setup: async (page) => {
      await startAcquisition(page, { encrypt: true, label: "Seized iPhone, item 7/restore_fail" });
      await acqResult(page, "Succeeded");
      await page.locator(".result-card").getByRole("button", { name: "Turn backup encryption off" }).click();
      const dialog = page.locator("dialog");
      await dialog.getByLabel("Backup password").fill("wrong");
      await dialog.getByRole("button", { name: "Turn encryption off" }).click();
      await page.getByText("Backup encryption is still on.").waitFor();
      await dialog.getByLabel("Backup password").fill("examiner-pw");
      await dialog.getByRole("button", { name: "Turn encryption off" }).click();
      await page.getByText("Backup encryption is off.").waitFor();
      await dialog.getByRole("button", { name: "Close" }).click();
      await page.getByText("Backup encryption was turned off after this acquisition.").waitFor();
      if (await page.locator(".result-card").getByRole("button", { name: "Turn backup encryption off" }).count()) {
        throw new Error("the result still offers the action after a successful restore");
      }
    },
  },
  {
    // "Parse with iLEAPP" after a success with "Parse with iLEAPP now": New run with the backup and
    // the password filled in.
    name: "newrun-handoff",
    query: "?mock",
    hash: acquireHash(NIGHTJAR),
    setup: async (page) => {
      await startAcquisition(page, { encrypt: true, keep: true, label: "Seized iPhone, item 7" });
      await acqResult(page, "Succeeded");
      await page.getByRole("button", { name: "Parse with iLEAPP" }).click();
      await page.getByText("The backup password set during the acquisition is filled in.").waitFor();
      await page.locator(".ready-text").waitFor();
    },
  },
  {
    name: "case-acquisitions",
    query: "?mock",
    hash: caseHash(NIGHTJAR),
    element: "section[aria-labelledby=acquisitions-heading]",
    setup: async (page) => {
      await page.locator(".acq-table").waitFor();
    },
  },
  {
    name: "case-acquisition-details",
    query: "?mock",
    hash: caseHash(NIGHTJAR),
    viewport: true,
    setup: async (page) => {
      await page.locator(".acq-table").getByRole("button", { name: "Details" }).first().click();
      await page.locator(".detail-groups").waitFor();
    },
  },
  {
    // The app "crashed" during an acquisition that had turned encryption on: interrupted on the
    // next case open, with "Turn backup encryption off".
    name: "case-acquisition-interrupted",
    query: "?mock",
    hash: acquireHash(NIGHTJAR),
    element: "section[aria-labelledby=acquisitions-heading]",
    setup: async (page) => {
      await startAcquisition(page, { encrypt: true, label: "Seized iPhone, item 7/interrupt" });
      await page.locator(".job-indicator").waitFor({ state: "detached", timeout: 20000 });
      await page.getByRole("link", { name: "Cases", exact: true }).first().click();
      await page.getByRole("link", { name: "Operation Nightjar" }).click();
      await page.locator(".acq-table .badge-interrupted").waitFor();
    },
  },
  {
    name: "case-restore-dialog",
    query: "?mock",
    hash: caseHash(NIGHTJAR),
    viewport: true,
    setup: async (page) => {
      await page.locator(".acq-table").getByRole("button", { name: "Turn backup encryption off" }).click();
      await page.locator("dialog").getByLabel("Backup password").fill("examiner-pw");
    },
  },
  {
    name: "case-restore-done",
    query: "?mock",
    hash: caseHash(NIGHTJAR),
    viewport: true,
    setup: async (page) => {
      await page.locator(".acq-table").getByRole("button", { name: "Turn backup encryption off" }).click();
      await page.locator("dialog").getByLabel("Backup password").fill("examiner-pw");
      await page.locator("dialog").getByRole("button", { name: "Turn encryption off" }).click();
      await page.getByText("Backup encryption is off.").waitFor();
    },
  },
  {
    // After the successful later restore the table is re-read (case_open): the row's derived
    // warnings no longer ask for "Turn backup encryption off".
    name: "case-acquisitions-after-restore",
    query: "?mock",
    hash: caseHash(NIGHTJAR),
    element: "section[aria-labelledby=acquisitions-heading]",
    setup: async (page) => {
      const turnOff = page.locator(".acq-table").getByRole("button", { name: "Turn backup encryption off" });
      await turnOff.click();
      await page.locator("dialog").getByLabel("Backup password").fill("examiner-pw");
      await page.locator("dialog").getByRole("button", { name: "Turn encryption off" }).click();
      await page.getByText("Backup encryption is off.").waitFor();
      await page.locator("dialog").getByRole("button", { name: "Close" }).click();
      await turnOff.waitFor({ state: "detached" });
      if (await page.locator(".acq-table").getByText("Encryption may still be on").count()) throw new Error("the row still says encryption may be on");
    },
  },
  {
    // 100,000 lines (the flood scenario), scrolled to the middle: auto-scroll turns itself off.
    name: "run-log-100k",
    query: "?mock",
    hash: newRunHash,
    element: ".log-view",
    setup: async (page) => {
      await startRun(page, "Choose folder…", "Evidence/flood");
      await waitForLines(page, 100_000);
      await page.evaluate(() => {
        const vp = document.querySelector(".log-viewport");
        if (vp) vp.scrollTop = 50_000 * 20 - 100;
      });
      await page.locator(".log-view input[type=checkbox]:not(:checked)").waitFor();
    },
  },
  // ---- S2: log search ----
  // Matches highlighted (the query's case differs from the lines'); Enter three times goes to the
  // third matching line, which is outlined, and auto-scroll turns itself off.
  // The counts are those of the flood log (100,090 lines), counted independently.
  logSearchScreen("run-log-search-matches", "SAFARI", "3,003 matching lines", async (page) => {
    const search = page.getByRole("searchbox", { name: "Search the log" });
    for (let i = 0; i < 3; i++) await search.press("Enter");
    await searchStatus(page, "3 of 3,003 matching lines");
    // The first matching line is line 947, so the third is line 949.
    await page.locator(".log-row-current:not([hidden]) .log-no", { hasText: "949" }).waitFor();
    await page.locator(".log-view input[type=checkbox]:not(:checked)").first().waitFor();
  }),
  logSearchScreen("run-log-search-none", "Traceback", "No matching lines"),
  // Only the matching lines (every 97th flood line), with their own line numbers.
  logSearchScreen("run-log-search-filter", "parsed 42 records", "1,031 matching lines", async (page) => {
    await page.getByLabel("Only matching lines").check();
    await page.waitForTimeout(100);
    const texts = await page.evaluate(() => [...document.querySelectorAll(".log-row:not([hidden]) .log-text")].map((e) => e.textContent ?? ""));
    if (texts.length === 0 || texts.some((t) => !t.includes("parsed 42 records"))) throw new Error(`rows that do not match are shown: ${JSON.stringify(texts.slice(0, 3))}`);
  }),
];

/**
 * @typedef {object} Check
 * @property {string} name
 * @property {string} [query] The URL query (default "?mock").
 * @property {string} hash
 * @property {(page: Page) => Promise<void>} run Throws when the check fails.
 */

/** Behavior checks (no screenshot), run once in the light theme. */
/** @type {Check[]} */
const CHECKS = [
  {
    // Regression check for implicit form submission: Enter in any New run field must never start a
    // run (a run folder and run.json are permanent); only activating Start run does.
    name: "check-enter-does-not-start",
    hash: newRunHash,
    run: async (page) => {
      await newRunReady(page);
      await pickInput(page, "Choose folder…", "00008101-000A1B2C3D4E");
      const password = page.getByLabel(/Backup password/);
      await password.fill("examiner-secret");
      await page.getByRole("radio", { name: /Custom selection/ }).check();
      await page.locator(".picker").waitFor();
      await page.locator(".picker-group").nth(0).locator(".picker-group-label input").check();
      await page.locator(".ready-text").waitFor();
      /** @param {string} field */
      const assertNoRun = async (field) => {
        await page.waitForTimeout(800);
        const state = await page.evaluate(() => ({
          hash: location.hash,
          indicator: document.querySelector(".job-indicator") !== null,
          ready: document.querySelector(".ready-text") !== null,
        }));
        if (!state.hash.startsWith("#/new-run") || state.indicator) {
          throw new Error(`Enter in the ${field} field started a run: ${JSON.stringify(state)}`);
        }
        if (!state.ready) throw new Error(`the form was not ready, so the check proves nothing: ${JSON.stringify(state)}`);
      };
      const search = page.getByRole("searchbox", { name: "Search modules" });
      await search.fill("call");
      await search.press("Enter");
      await assertNoRun("module search");
      const label = page.getByLabel("Label");
      await label.fill("Enter must not start a run");
      await label.press("Enter");
      await assertNoRun("label");
      await password.press("Enter");
      await assertNoRun("password");
      // Explicit keyboard activation of Start run does start the run.
      await page.getByRole("button", { name: "Start run" }).press("Enter");
      await page.locator(".run-screen .phase-steps").waitFor();
    },
  },
  {
    // Cancelling a run needs an explicit confirmation: Escape, "Keep running" and Enter on the
    // initially focused button all leave it running; only the dialog's "Cancel run" cancels.
    name: "check-run-cancel-confirm",
    hash: newRunHash,
    run: async (page) => {
      await startRun(page, "Choose folder…", "Evidence/slow");
      await waitForLines(page, 10);
      const open = async () => {
        await page.locator(".screen-head").getByRole("button", { name: "Cancel run" }).click();
        await page.locator("dialog").getByRole("button", { name: "Keep running" }).waitFor();
      };
      /** @param {string} how */
      const assertRunning = async (how) => {
        await page.locator("dialog").waitFor({ state: "detached" });
        await page.waitForTimeout(1200);
        const state = await page.evaluate(() => ({
          badge: document.querySelector(".run-status .badge")?.textContent ?? "",
          cancel: document.querySelector(".screen-head .btn-danger:not([hidden])")?.textContent ?? "",
        }));
        if (state.badge !== "Running" || state.cancel !== "Cancel run") throw new Error(`${how} cancelled the run: ${JSON.stringify(state)}`);
      };
      await open();
      await page.keyboard.press("Escape");
      await assertRunning("Escape");
      await open();
      await page.locator("dialog").getByRole("button", { name: "Keep running" }).click();
      await assertRunning("Keep running");
      await open();
      await page.keyboard.press("Enter");
      await assertRunning("Enter on the focused button");
      await open();
      await page.locator("dialog").getByRole("button", { name: "Cancel run" }).click();
      await runResult(page, "Cancelled");
    },
  },
  {
    // An acquisition changes the device: Enter in the label or either password field of a ready
    // form never starts one; only activating Start acquisition does.
    name: "check-acq-enter-does-not-start",
    hash: acquireHash(NIGHTJAR),
    run: async (page) => {
      await acquireReady(page);
      await page.getByLabel("Label").fill("Enter must not start");
      await enableEncryption(page);
      await page.locator(".ready-text").waitFor();
      /** @param {string} field */
      const assertNotStarted = async (field) => {
        await page.waitForTimeout(800);
        const state = await page.evaluate(() => ({
          progress: document.querySelector(".acquire-screen .phase-steps") !== null,
          indicator: document.querySelector(".job-indicator") !== null,
          ready: document.querySelector(".ready-text") !== null,
        }));
        if (state.progress || state.indicator) throw new Error(`Enter in the ${field} field started an acquisition: ${JSON.stringify(state)}`);
        if (!state.ready) throw new Error(`the form was not ready, so the check proves nothing: ${JSON.stringify(state)}`);
      };
      await page.getByLabel("Label").press("Enter");
      await assertNotStarted("label");
      await page.getByLabel(/^Backup password/).press("Enter");
      await assertNotStarted("password");
      await page.getByLabel("Password again").press("Enter");
      await assertNotStarted("password again");
      await page.getByRole("button", { name: "Start acquisition" }).press("Enter");
      await page.locator(".acquire-screen .phase-steps").waitFor();
    },
  },
  {
    name: "check-acq-cancel-confirm",
    hash: acquireHash(NIGHTJAR),
    run: async (page) => {
      await startAcquisition(page, { label: "Check/slow" });
      await page.locator(".progress-block").waitFor();
      const open = async () => {
        await page.locator(".screen-head").getByRole("button", { name: "Cancel acquisition" }).click();
        await page.locator("dialog").getByRole("button", { name: "Keep going" }).waitFor();
      };
      /** @param {string} how */
      const assertRunning = async (how) => {
        await page.locator("dialog").waitFor({ state: "detached" });
        await page.waitForTimeout(1200);
        const badge = await page.evaluate(() => document.querySelector(".acquire-screen .run-status .badge")?.textContent ?? "");
        if (badge !== "Running") throw new Error(`${how} cancelled the acquisition (${badge})`);
      };
      await open();
      await page.keyboard.press("Escape");
      await assertRunning("Escape");
      await open();
      await page.locator("dialog").getByRole("button", { name: "Keep going" }).click();
      await assertRunning("Keep going");
      await open();
      await page.keyboard.press("Enter");
      await assertRunning("Enter on the focused button");
      await open();
      await page.locator("dialog").getByRole("button", { name: "Cancel acquisition" }).click();
      await acqResult(page, "Cancelled");
    },
  },
  {
    // Turning backup encryption off changes the device: Enter in the password field and Escape do
    // nothing to it; only the button does, and the field is cleared once it is used.
    name: "check-encryption-off-confirm",
    hash: caseHash(NIGHTJAR),
    run: async (page) => {
      const turnOff = page.locator(".acq-table").getByRole("button", { name: "Turn backup encryption off" });
      await turnOff.click();
      const dialog = page.locator("dialog");
      const password = dialog.getByLabel("Backup password");
      await password.fill("examiner-pw");
      await password.press("Enter");
      await page.waitForTimeout(1000);
      const afterEnter = await page.evaluate(() => ({
        open: document.querySelector("dialog") !== null,
        result: document.querySelector("dialog .restore-result")?.textContent ?? "",
      }));
      if (!afterEnter.open || afterEnter.result !== "") throw new Error(`Enter in the password field acted: ${JSON.stringify(afterEnter)}`);
      await page.keyboard.press("Escape");
      await dialog.waitFor({ state: "detached" });
      await turnOff.click();
      await dialog.getByLabel("Backup password").fill("examiner-pw");
      await dialog.getByRole("button", { name: "Turn encryption off" }).click();
      // While the command runs, Escape (even twice: Chromium's close-watcher rule) keeps the dialog.
      await page.keyboard.press("Escape");
      await page.keyboard.press("Escape");
      await page.getByText("Backup encryption is off.").waitFor();
      const value = await page.evaluate(() => /** @type {HTMLInputElement | null} */ (document.querySelector("dialog input[type=password]"))?.value ?? null);
      if (value !== "") throw new Error(`the password field was not cleared after use: ${JSON.stringify(value)}`);
      await dialog.getByRole("button", { name: "Close" }).click();
      // The rows are re-read: once a restore succeeded, the core's AcqSummary no longer asks for it.
      await turnOff.waitFor({ state: "detached" });
    },
  },
  {
    // The encryption alert of a result is inserted once (not again when the final record
    // arrives), and a result shown again after a later restore from the Case screen is re-read:
    // it no longer offers "Turn backup encryption off" (K5 review SF1, N-b).
    name: "check-acquire-result-reread",
    hash: acquireHash(NIGHTJAR),
    run: async (page) => {
      await acquireReady(page);
      await page.evaluate(() => {
        const w = /** @type {any} */ (window);
        w.__alerts = 0;
        new MutationObserver((records) => {
          for (const r of records) {
            for (const n of r.addedNodes) {
              if (n instanceof Element && (n.matches(".banner-danger[role=alert]") || n.querySelector(".banner-danger[role=alert]"))) w.__alerts += 1;
            }
          }
        }).observe(document.body, { childList: true, subtree: true });
      });
      await startAcquisition(page, { encrypt: true, label: "Seized iPhone, item 7/restore_fail" });
      await acqResult(page, "Succeeded");
      await page.getByText("Backup encryption may still be on for this device.").waitFor();
      // The final acquisition.json is read after `finished`: its seal row appears.
      await page.locator(".result-card dt", { hasText: "Seal" }).waitFor();
      await page.waitForTimeout(1000);
      const alerts = await page.evaluate(() => /** @type {any} */ (window).__alerts);
      if (alerts !== 1) throw new Error(`the encryption alert was inserted ${alerts} times`);
      // Turn it off from the Case screen, then open Acquire again.
      await page.locator(".acquire-screen").getByRole("link", { name: "Operation Nightjar" }).first().click();
      const turnOff = page.locator(".acq-table").getByRole("button", { name: "Turn backup encryption off" }).first();
      await turnOff.click();
      const dialog = page.locator("dialog");
      await dialog.getByLabel("Backup password").fill("examiner-pw");
      await dialog.getByRole("button", { name: "Turn encryption off" }).click();
      await page.getByText("Backup encryption is off.").waitFor();
      await dialog.getByRole("button", { name: "Close" }).click();
      await page.evaluate((hash) => {
        window.location.hash = hash;
      }, acquireHash(NIGHTJAR));
      await acqResult(page, "Succeeded");
      await page.getByText("Backup encryption was turned off after this acquisition.").waitFor();
      if (await page.locator(".result-card").getByRole("button", { name: "Turn backup encryption off" }).count()) {
        throw new Error("the reopened result still offers Turn backup encryption off");
      }
    },
  },
  {
    // "New acquisition" starts from the defaults, whatever the previous acquisition used.
    name: "check-new-acquisition-resets",
    hash: acquireHash(NIGHTJAR),
    run: async (page) => {
      await acquireReady(page);
      await page.getByRole("radio", { name: "Alex's iPhone" }).waitFor();
      await page.getByLabel("Label").fill("Item 7");
      await enableEncryption(page);
      await page.getByLabel("Turn encryption off again afterwards").uncheck();
      await page.getByLabel("Parse with iLEAPP now").check();
      await page.locator(".ready-text").waitFor();
      await page.getByRole("button", { name: "Start acquisition" }).click();
      await acqResult(page, "Succeeded");
      // Left on as asked: turn it off (a later restore), so the next form offers encryption again.
      await page.locator(".result-card").getByRole("button", { name: "Turn backup encryption off" }).click();
      await page.locator("dialog").getByLabel("Backup password").fill("examiner-pw");
      await page.locator("dialog").getByRole("button", { name: "Turn encryption off" }).click();
      await page.getByText("Backup encryption is off.").waitFor();
      await page.locator("dialog").getByRole("button", { name: "Close" }).click();
      await page.getByText("Backup encryption was turned off after this acquisition.").waitFor();
      await page.getByRole("button", { name: "New acquisition" }).click();
      await page.getByRole("radio", { name: "Alex's iPhone" }).waitFor();
      const enable = page.getByLabel("Enable backup encryption (recommended)");
      await enable.waitFor();
      const first = { label: await page.getByLabel("Label").inputValue(), enable: await enable.isChecked() };
      if (first.label !== "" || first.enable) throw new Error(`the form kept the previous options: ${JSON.stringify(first)}`);
      await enable.check();
      const next = {
        restore: await page.getByLabel("Turn encryption off again afterwards").isChecked(),
        parse: await page.getByLabel("Parse with iLEAPP now").isChecked(),
        password: await page.getByLabel(/^Backup password/).inputValue(),
        again: await page.getByLabel("Password again").inputValue(),
      };
      if (!next.restore || next.parse || next.password !== "" || next.again !== "") throw new Error(`the encryption options were not reset: ${JSON.stringify(next)}`);
    },
  },
  {
    // Progress events (several per second) must not move keyboard focus: the facts' time element
    // (focusable for its UTC tooltip) keeps it during a backup.
    name: "check-focus-kept-during-backup",
    hash: acquireHash(NIGHTJAR),
    run: async (page) => {
      await startAcquisition(page, { label: "Focus check/slow" });
      await page.locator(".progress-block").waitFor();
      await page.locator(".run-facts time").focus();
      await page.evaluate(() => {
        /** @type {any} */ (window).__focused = document.activeElement;
      });
      await page.waitForTimeout(2500);
      const state = await page.evaluate(() => ({
        same: document.activeElement === /** @type {any} */ (window).__focused,
        tag: document.activeElement?.tagName ?? "",
        percent: document.querySelector(".progress-block .muted")?.textContent ?? "",
      }));
      if (!state.same) throw new Error(`focus moved during the backup: ${JSON.stringify(state)}`);
      await page.locator(".screen-head").getByRole("button", { name: "Cancel acquisition" }).click();
      await page.locator("dialog").getByRole("button", { name: "Cancel acquisition" }).click();
      await acqResult(page, "Cancelled");
    },
  },
  {
    // Install events must not move keyboard focus: the other parser's Install button keeps it.
    name: "check-focus-kept-during-install",
    query: "?mock&scenario=no_tools",
    hash: "#/settings",
    run: async (page) => {
      await page.locator(".tool-card", { hasText: "iLEAPP" }).getByRole("button", { name: /^Install/ }).click();
      await page.getByText(/^Downloading iLEAPP/).waitFor();
      const other = page.locator(".tool-card", { hasText: "aLEAPP" }).getByRole("button", { name: /^Install/ });
      await other.focus();
      await page.evaluate(() => {
        /** @type {any} */ (window).__focused = document.activeElement;
      });
      await page.getByText("Installed and verified.").waitFor({ timeout: 15000 });
      const same = await page.evaluate(() => document.activeElement === /** @type {any} */ (window).__focused);
      if (!same) throw new Error("focus moved while the other parser installed");
    },
  },
  {
    // Polling never pairs: an unpaired device stays unpaired across several polls.
    name: "check-no-implicit-pairing",
    hash: acquireHash(NIGHTJAR),
    run: async (page) => {
      await acquireReady(page);
      const ipad = page.locator(".device-card", { hasText: "iPad13,4" });
      await ipad.getByText("Not paired").waitFor();
      await page.waitForTimeout(5000);
      await ipad.getByText("Not paired").waitFor({ timeout: 1000 });
      await ipad.getByRole("button", { name: "Pair" }).waitFor({ timeout: 1000 });
    },
  },
  {
    // The handed-over password lives only in that New run form: closing the form drops it, and a
    // second "Parse with iLEAPP" for the same backup does not bring it back.
    name: "check-handoff-cleared",
    hash: acquireHash(NIGHTJAR),
    run: async (page) => {
      await startAcquisition(page, { encrypt: true, keep: true, label: "Handoff check" });
      await acqResult(page, "Succeeded");
      await page.getByRole("button", { name: "Parse with iLEAPP" }).click();
      const password = page.getByLabel(/Backup password/);
      await password.waitFor();
      const handed = await page.evaluate(() => /** @type {HTMLInputElement | null} */ (document.querySelector("input[name=itunes_password]"))?.value ?? "");
      if (handed !== "examiner-pw") throw new Error("the password was not handed over");
      await page.locator(".start-card").getByRole("link", { name: "Cancel" }).click();
      await page.locator(".acq-table").waitFor();
      await page.locator("tr", { hasText: "Handoff check" }).getByRole("button", { name: "Parse with iLEAPP" }).click();
      await password.waitFor();
      await page.locator(".start-reasons").getByText("Enter the backup password").waitFor();
      const again = await page.evaluate(() => /** @type {HTMLInputElement | null} */ (document.querySelector("input[name=itunes_password]"))?.value ?? null);
      if (again !== "") throw new Error(`the password came back after the form closed: ${JSON.stringify(again)}`);
    },
  },
  {
    // Log search (S2) by keyboard, on a live run whose log is complete (held in Analyzing: 90
    // lines, 42 of them "<module> artifact completed", at the even line numbers 6 to 88):
    // case-insensitive matches in <mark>; a 256-character cap; an input-method Enter does nothing;
    // Enter / Shift+Enter and the buttons go round the matches; "Only matching lines";
    // regular-expression characters are literal; Escape clears the query and the filter; a match
    // beyond the right edge is scrolled into view. No key in the box does anything else (the run
    // stays live, no dialog opens), and the live region changes at most once per second, with the
    // last match reached among its updates.
    name: "check-log-search",
    query: "?mock&scenario=hold_analyzing",
    hash: newRunHash,
    run: async (page) => {
      await startRun(page, "Choose folder…", "Pixel-7-extraction");
      await page.locator(".phase-step-current", { hasText: "Analyzing" }).waitFor();
      await waitForLines(page, 90);
      await page.evaluate(() => {
        const w = /** @type {any} */ (window);
        w.__live = [];
        const region = /** @type {HTMLElement} */ (document.querySelector(".log-view [aria-live]"));
        new MutationObserver(() => w.__live.push({ t: performance.now(), text: region.textContent ?? "" })).observe(region, { childList: true, characterData: true, subtree: true });
      });
      const search = page.getByRole("searchbox", { name: "Search the log" });
      /** @param {string} step */
      const rows = async (step) => {
        const r = await page.evaluate(() =>
          [...document.querySelectorAll(".log-row:not([hidden])")].map((row) => ({
            no: Number(row.querySelector(".log-no")?.textContent),
            text: row.querySelector(".log-text")?.textContent ?? "",
            marks: [...row.querySelectorAll(".log-text mark")].map((m) => m.textContent ?? ""),
            current: row.classList.contains("log-row-current"),
          })),
        );
        if (r.length === 0) throw new Error(`${step}: no rows are shown`);
        return r;
      };
      /** @param {string} step @param {number} line */
      const currentIs = async (step, line) => {
        const current = (await rows(step)).filter((row) => row.current);
        if (current.length !== 1 || current[0].no !== line || current[0].marks.length !== 1) {
          throw new Error(`${step}: the current row is ${JSON.stringify(current)}, expected line ${line} with one mark`);
        }
      };

      await search.fill("ARTIFACT Completed");
      await searchStatus(page, "42 matching lines");
      const marked = (await rows("typed")).flatMap((row) => row.marks);
      if (marked.length === 0 || marked.some((m) => m !== "artifact completed")) throw new Error(`marks: ${JSON.stringify(marked)}`);
      if ((await page.evaluate(() => /** @type {HTMLInputElement} */ (document.querySelector(".log-search-input")).maxLength)) !== 256) {
        throw new Error("the search box does not cap the query at 256 characters");
      }
      // The Enter that commits an input-method composition (WebKit: keyCode 229) does nothing.
      await page.evaluate(() => {
        const input = /** @type {HTMLInputElement} */ (document.querySelector(".log-search-input"));
        input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", keyCode: 229, bubbles: true, cancelable: true }));
      });
      await page.waitForTimeout(150);
      await searchStatus(page, "42 matching lines", 1);
      if (!(await page.getByLabel("Auto-scroll").isChecked())) throw new Error("an input-method Enter went to a match");
      // Nothing is current yet, so Enter goes to the first match in view: the view is at the end
      // (lines 72 to 90), and line 72 is match 34.
      await search.press("Enter");
      await searchStatus(page, "34 of 42 matching lines");
      await currentIs("Enter", 72);
      if (await page.getByLabel("Auto-scroll").isChecked()) throw new Error("going to a match left auto-scroll on");
      await search.press("Enter");
      await searchStatus(page, "35 of 42 matching lines");
      await currentIs("Enter again", 74);
      await search.press("Shift+Enter");
      await search.press("Shift+Enter");
      await searchStatus(page, "33 of 42 matching lines");
      await currentIs("Shift+Enter twice", 70);
      // The buttons, by keyboard; both ends wrap around.
      const nextButton = page.getByRole("button", { name: "Next match" });
      for (let i = 0; i < 9; i++) await nextButton.press("Enter");
      await searchStatus(page, "42 of 42 matching lines");
      await currentIs("Next nine times", 88);
      await search.press("Enter");
      await searchStatus(page, "1 of 42 matching lines");
      await currentIs("Enter at the last match", 6);
      await page.getByRole("button", { name: "Previous match" }).press("Space");
      await searchStatus(page, "42 of 42 matching lines");
      await currentIs("Previous at the first match", 88);
      // Rapid presses: the live region still changes at most once per second.
      await search.focus();
      for (let i = 0; i < 10; i++) await search.press("Enter");
      await searchStatus(page, "10 of 42 matching lines");
      await currentIs("ten Enters", 24);
      // Let the throttle deliver the last of them before the next announcement replaces it.
      await page.waitForTimeout(1100);

      // The rows change on the next frame; the status does not change.
      await page.getByLabel("Only matching lines").check();
      for (let tries = 0; ; tries++) {
        const only = await rows("only matching");
        if (only.every((row, i) => row.text.includes("artifact completed") && (i === 0 || row.no === only[i - 1].no + 2))) break;
        if (tries > 50) throw new Error(`"Only matching lines" shows ${JSON.stringify(only.map((row) => row.no))}`);
        await page.waitForTimeout(50);
      }
      await currentIs("only matching", 24);

      // Regular-expression characters match themselves.
      for (const query of [".*", "(", "[a-z]+", "\\"]) {
        await search.fill(query);
        await searchStatus(page, "No matching lines");
      }
      const empty = await page.evaluate(() => document.querySelector(".log-empty:not([hidden])")?.textContent ?? null);
      if (empty !== "No lines match the search.") throw new Error(`the filtered log with no matches says ${JSON.stringify(empty)}`);
      await search.fill("Pixel-7-extraction");
      await searchStatus(page, "1 matching line");
      // Escape clears the query and unticks "Only matching lines".
      await search.press("Escape");
      await searchStatus(page, "");
      if ((await search.inputValue()) !== "") throw new Error("Escape did not clear the search");
      if (await page.getByLabel("Only matching lines").isChecked()) throw new Error("Escape left \"Only matching lines\" ticked");
      const all = await rows("cleared");
      if (all.some((row) => row.marks.length > 0 || row.current) || all.some((row, i) => i > 0 && row.no !== all[i - 1].no + 1)) {
        throw new Error("after Escape, the log still shows a search");
      }

      // Sideways: in a narrow view the match lies beyond the right edge; going to it scrolls there.
      await page.evaluate(() => {
        const vp = /** @type {HTMLElement} */ (document.querySelector(".log-viewport"));
        vp.style.width = "240px";
        vp.scrollLeft = 0;
      });
      await search.fill("artifact COMPLETED");
      await searchStatus(page, "42 matching lines");
      await search.press("Enter");
      await page.locator(".log-row-current:not([hidden]) mark").waitFor();
      await page.waitForTimeout(100);
      const side = await page.evaluate(() => {
        const vp = /** @type {HTMLElement} */ (document.querySelector(".log-viewport"));
        const mark = /** @type {HTMLElement} */ (vp.querySelector(".log-row-current:not([hidden]) mark"));
        const v = vp.getBoundingClientRect();
        const m = mark.getBoundingClientRect();
        const left = v.left + vp.clientLeft;
        return { scrollLeft: vp.scrollLeft, visible: m.left >= left && m.right <= left + vp.clientWidth };
      });
      if (side.scrollLeft <= 0 || !side.visible) throw new Error(`the match beyond the right edge was not scrolled into view: ${JSON.stringify(side)}`);
      await page.evaluate(() => {
        const vp = /** @type {HTMLElement} */ (document.querySelector(".log-viewport"));
        vp.style.width = "";
      });
      await search.press("Escape");
      await searchStatus(page, "");

      const state = await page.evaluate(() => ({
        hash: location.hash,
        dialog: document.querySelector("dialog") !== null,
        badge: document.querySelector(".run-status .badge")?.textContent ?? "",
        cancel: document.querySelector(".screen-head .btn-danger:not([hidden])")?.textContent ?? "",
      }));
      if (!state.hash.startsWith("#/run") || state.dialog || state.badge !== "Running" || state.cancel !== "Cancel run") {
        throw new Error(`a key in the log search did something else: ${JSON.stringify(state)}`);
      }

      await page.waitForTimeout(1200);
      const live = await page.evaluate(() => /** @type {{ t: number, text: string }[]} */ (/** @type {any} */ (window).__live));
      const gaps = live.slice(1).map((x, i) => x.t - live[i].t);
      // The throttle waits 1,000 ms by Date.now(); the observer stamps performance.now().
      if (live.length < 2 || gaps.some((g) => g < 990)) throw new Error(`live region updates ${JSON.stringify(live.map((x) => Math.round(x.t)))}`);
      if (!live.some((x) => x.text.startsWith("Match 10 of 42, line 24: "))) throw new Error(`the rapid presses did not end with match 10: ${JSON.stringify(live.map((x) => x.text))}`);
    },
  },
  {
    // With a query left in the box on a streaming run, the live region still gets the latest line
    // (DEVELOPMENT.md §4.6): the search report goes out first, then the latest line resumes, and
    // updates stay at least a second apart.
    name: "check-log-search-live",
    hash: newRunHash,
    run: async (page) => {
      await startRun(page, "Choose folder…", "Evidence/slow");
      await waitForLines(page, 10);
      await page.evaluate(() => {
        const w = /** @type {any} */ (window);
        w.__live = [];
        const region = /** @type {HTMLElement} */ (document.querySelector(".log-view [aria-live]"));
        new MutationObserver(() => w.__live.push({ t: performance.now(), text: region.textContent ?? "" })).observe(region, { childList: true, characterData: true, subtree: true });
      });
      await page.getByRole("searchbox", { name: "Search the log" }).fill("artifact completed");
      await page.waitForTimeout(3500);
      const live = await page.evaluate(() => /** @type {{ t: number, text: string }[]} */ (/** @type {any} */ (window).__live));
      const texts = live.map((x) => x.text);
      const report = texts.findIndex((t) => /^[\d,]+ matching lines?$/.test(t));
      if (report < 0) throw new Error(`no search report: ${JSON.stringify(texts)}`);
      if (texts.slice(report + 1).filter((t) => / artifact (started|completed)$/.test(t)).length < 2) {
        throw new Error(`the latest line did not resume after the search report: ${JSON.stringify(texts)}`);
      }
      const gaps = live.slice(1).map((x, i) => x.t - live[i].t);
      if (gaps.some((g) => g < 990)) throw new Error(`live region updates ${JSON.stringify(live.map((x) => Math.round(x.t)))}`);
    },
  },
];

/**
 * Records CSP violations, console errors and uncaught page errors of `page` in `problems`.
 * @param {Page} page
 * @param {string} label
 * @param {string[]} problems
 */
function watchPage(page, label, problems) {
  page.on("console", (message) => {
    const text = message.text();
    if (/content security policy|csp violation|refused to/i.test(text)) problems.push(`${label}: CSP: ${text}`);
    else if (message.type() === "error") problems.push(`${label}: console error: ${text}`);
  });
  page.on("pageerror", (error) => problems.push(`${label}: page error: ${error.message}`));
}

/**
 * A browser context that also reports CSP violations as console errors (Chromium logs its own
 * message too).
 * @param {import("playwright").Browser} browser
 * @param {"light" | "dark"} scheme
 */
async function newContext(browser, scheme) {
  const context = await browser.newContext({ colorScheme: scheme, viewport: { width: 1200, height: 800 }, deviceScaleFactor: 1 });
  await context.addInitScript(() => {
    document.addEventListener("securitypolicyviolation", (e) => {
      console.error(`CSP violation: ${e.violatedDirective} blocked ${e.blockedURI || "inline"}`);
    });
  });
  return context;
}

/** @param {string[]} argv */
function parseArgs(argv) {
  /** @type {{ root: string | null, out: string | null, screens: string[] | null }} */
  const opts = { root: null, out: null, screens: null };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const value = argv[i + 1];
    if ((arg === "--root" || arg === "--out" || arg === "--screens") && value !== undefined) {
      i += 1;
      if (arg === "--root") opts.root = path.resolve(value);
      else if (arg === "--out") opts.out = path.resolve(value);
      else opts.screens = value.split(",").map((s) => s.trim()).filter(Boolean);
    } else {
      throw new Error(`unknown or incomplete argument: ${arg}\n${USAGE}`);
    }
  }
  if (!opts.root || !opts.out) throw new Error(USAGE);
  return { root: opts.root, out: opts.out, screens: opts.screens };
}

/**
 * Starts serve-ui and resolves with its port (from the first stdout line).
 * @param {string} root
 */
function startServer(root) {
  const child = spawn(process.execPath, [path.join(root, "scripts", "serve-ui.mjs"), "--root", root, "--port", "0"], {
    stdio: ["ignore", "pipe", "pipe"],
  });
  /** @type {Promise<number>} */
  const port = new Promise((resolve, reject) => {
    let buffer = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
    });
    child.stdout.on("data", (chunk) => {
      buffer += chunk.toString();
      const newline = buffer.indexOf("\n");
      if (newline === -1) return;
      const match = /^listening on (\d+)$/.exec(buffer.slice(0, newline).trim());
      if (match) resolve(Number(match[1]));
      else reject(new Error(`unexpected first line from serve-ui: ${buffer.slice(0, newline)}`));
    });
    child.once("exit", (code) => reject(new Error(`serve-ui exited with ${code}: ${stderr}`)));
    child.once("error", reject);
  });
  return { child, port };
}

/** @param {number[]} values */
const median = (values) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)];

/**
 * Render + filter of 1,300 fixture modules in the module picker, measured in the page with
 * performance.now() (DEVELOPMENT.md §4.6, ROADMAP D3). The picker is placed in view and layout is
 * forced before each reading.
 * @param {Page} page
 * @param {string} query
 */
async function measurePicker(page, query) {
  /** @type {number[]} */
  const render = [];
  /** @type {number[]} */
  const filter = [];
  let modules = 0;
  for (let run = 0; run < 7; run++) {
    const r = await page.evaluate(async (q) => {
      const pickerUrl = "/components/module-picker.js";
      const fixtureUrl = "/dev/fixtures/modules.js";
      const { modulePicker } = await import(pickerUrl);
      const { makeModules } = await import(fixtureUrl);
      const list = makeModules("ileapp", 1300);
      // In view at the top of the page, as an examiner sees it (off-screen content is not rendered).
      const host = document.createElement("div");
      document.body.prepend(host);
      window.scrollTo(0, 0);
      const t0 = performance.now();
      const picker = modulePicker({ modules: list, selected: [], toolLabel: "iLEAPP", onChange() {} });
      host.append(picker.node);
      void host.getBoundingClientRect().height;
      const t1 = performance.now();
      picker.setQuery(q);
      void host.getBoundingClientRect().height;
      const t2 = performance.now();
      host.remove();
      return { modules: list.length, render: t1 - t0, filter: t2 - t1 };
    }, query);
    modules = r.modules;
    render.push(r.render);
    filter.push(r.filter);
  }
  const totals = render.map((v, i) => v + filter[i]);
  return {
    query,
    modules,
    runs: totals.length,
    render_ms_median: Number(median(render).toFixed(1)),
    filter_ms_median: Number(median(filter).toFixed(1)),
    total_ms_median: Number(median(totals).toFixed(1)),
    total_ms_max: Number(Math.max(...totals).toFixed(1)),
  };
}

/**
 * The Run screen with a 100,000-line mock run (`…/flood`: 200 `log` events of 500 lines, the
 * core's maximum batch), measured in the page (ROADMAP D4a, DEVELOPMENT.md §4.6):
 * - streaming: wall time until all lines are in the log view, and main-thread long tasks (> 50 ms,
 *   PerformanceObserver "longtask") while they arrive;
 * - jumps: 50 scrollTop changes spread over the whole log, each timed until two animation frames
 *   later (the scroll event, then the render), with a check that the row for the new position is
 *   rendered with the right line number;
 * - continuous scrolling: 120 frames moving 700 px each, the interval between frames;
 * - the number of row elements in the DOM.
 * @param {Page} page
 */
async function measureLog(page) {
  await newRunReady(page);
  await pickInput(page, "Choose folder…", "Evidence/flood");
  await page.locator(".ready-text").waitFor();
  await page.evaluate(() => {
    const w = /** @type {any} */ (window);
    w.__longTasks = [];
    new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) w.__longTasks.push(Math.round(entry.duration));
    }).observe({ type: "longtask" });
  });
  const t0 = Date.now();
  await page.getByRole("button", { name: "Start run" }).click();
  await page.locator(".run-screen .log-viewport").waitFor();
  await waitForLines(page, 100_000);
  const streamMs = Date.now() - t0;
  const streamLongTasks = await page.evaluate(() => /** @type {number[]} */ (/** @type {any} */ (window).__longTasks.splice(0)));
  const scroll = await page.evaluate(async () => {
    const vp = /** @type {HTMLElement} */ (document.querySelector(".log-viewport"));
    const rowHeight = 20;
    const frame = () => new Promise((resolve) => requestAnimationFrame(() => resolve(undefined)));
    /** @type {number[]} */
    const jumps = [];
    let wrongRows = 0;
    const max = vp.scrollHeight - vp.clientHeight;
    for (let i = 0; i < 50; i++) {
      const target = Math.round((max * ((i * 37) % 50)) / 49);
      const t = performance.now();
      vp.scrollTop = target;
      await frame();
      await frame();
      jumps.push(performance.now() - t);
      const expected = Math.floor(vp.scrollTop / rowHeight) + 1;
      const numbers = [...vp.querySelectorAll(".log-row:not([hidden]) .log-no")].map((e) => Number(e.textContent));
      if (!numbers.includes(expected)) wrongRows += 1;
    }
    vp.scrollTop = 0;
    await frame();
    /** @type {number[]} */
    const intervals = [];
    let last = performance.now();
    for (let i = 0; i < 120; i++) {
      vp.scrollTop += 700;
      await frame();
      const now = performance.now();
      intervals.push(now - last);
      last = now;
    }
    return { jumps, intervals, wrongRows, rows: vp.querySelectorAll(".log-row").length, lines: Number(vp.dataset.count) };
  });
  const scrollLongTasks = await page.evaluate(() => /** @type {number[]} */ (/** @type {any} */ (window).__longTasks.splice(0)));
  const round = (/** @type {number} */ v) => Number(v.toFixed(1));
  return {
    lines: scroll.lines,
    row_elements: scroll.rows,
    stream_ms: streamMs,
    stream_long_tasks: { count: streamLongTasks.length, max_ms: Math.max(0, ...streamLongTasks) },
    jump_ms: { runs: scroll.jumps.length, median: round(median(scroll.jumps)), max: round(Math.max(...scroll.jumps)) },
    jump_wrong_rows: scroll.wrongRows,
    scroll_frame_ms: { frames: scroll.intervals.length, median: round(median(scroll.intervals)), max: round(Math.max(...scroll.intervals)) },
    scroll_long_tasks: { count: scrollLongTasks.length, max_ms: Math.max(0, ...scrollLongTasks) },
  };
}

/** The flood run's log: 4 header lines, 100,000 flood lines, 84 artifact lines and 2 closing lines. */
const FLOOD_LOG_LINES = 100_090;
/**
 * Search queries measured on the flood log, with the status each must give (counted independently).
 * @type {[string, string | null][]}
 */
const LOG_SEARCH_QUERIES = [
  // Typed one key at a time: each key searches every line again.
  ["s", null],
  ["sa", null],
  ["saf", null],
  ["safa", null],
  ["safar", null],
  ["safari", "3,003 matching lines"],
  ["SAFARI", "3,003 matching lines"],
  ["parsed 42 records", "1,031 matching lines"],
  ["a", "100,088 matching lines"],
  ["Traceback", "No matching lines"],
];

/**
 * Log search at 100,000 lines (ROADMAP S2), on the page `measureLog` leaves, once the flood run has
 * finished (100,090 lines), measured in the page as `measureLog` does:
 * - query: setting the search box (an `input` event) until two animation frames later (every line
 *   searched, then the rows in view rendered with their highlights), 5 times per query, each from
 *   an empty box: typed prefixes of "safari", then queries matching ~3%, ~1%, all and none of the
 *   lines. `work` is the view's own part of that frame (the search and the row updates);
 * - next: 30 Enter presses in the box through the "SAFARI" matches, each until two frames later,
 *   with a check that the current match's row is rendered;
 * - "Only matching lines" with "a" (every line: the most rows) and with "SAFARI": the time to turn
 *   it on, then 50 jumps and 120 scroll frames as in `measureLog`, with a check that every
 *   rendered row contains the query;
 * - main-thread long tasks throughout.
 * @param {Page} page
 */
async function measureLogSearch(page) {
  await runResult(page, "Succeeded");
  await waitForLines(page, FLOOD_LOG_LINES);
  await page.evaluate(() => void (/** @type {any} */ (window).__longTasks.splice(0)));
  const r = await page.evaluate(async (queries) => {
    const frame = () => new Promise((resolve) => requestAnimationFrame(() => resolve(undefined)));
    const twoFrames = async () => {
      await frame();
      await frame();
    };
    const input = /** @type {HTMLInputElement} */ (document.querySelector(".log-search-input"));
    const only = /** @type {HTMLInputElement} */ (document.querySelector(".log-search input[type=checkbox]"));
    const vp = /** @type {HTMLElement} */ (document.querySelector(".log-viewport"));
    const status = () => document.querySelector(".log-search-status")?.textContent ?? "";
    /**
     * Sets the query; returns the time until two frames later, and the view's own work in the
     * frame (animation-frame callbacks run in order: one before the view's render, one after).
     * @param {string} q
     */
    const setQuery = async (q) => {
      let before = 0;
      requestAnimationFrame(() => {
        before = performance.now();
      });
      const t = performance.now();
      input.value = q;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      const after = await new Promise((resolve) => requestAnimationFrame(() => resolve(performance.now())));
      await frame();
      return { total: performance.now() - t, work: /** @type {number} */ (after) - before };
    };
    /** @param {boolean} on */
    const setOnly = async (on) => {
      const t = performance.now();
      only.checked = on;
      only.dispatchEvent(new Event("change", { bubbles: true }));
      await twoFrames();
      return performance.now() - t;
    };

    const typed = [];
    for (const [q] of queries) {
      /** @type {number[]} */
      const ms = [];
      /** @type {number[]} */
      const work = [];
      for (let run = 0; run < 5; run++) {
        await setQuery("");
        const m = await setQuery(q);
        ms.push(m.total);
        work.push(m.work);
      }
      typed.push({ query: q, ms, work, status: status() });
    }

    await setQuery("SAFARI");
    /** @type {number[]} */
    const next = [];
    let missing = 0;
    for (let i = 0; i < 30; i++) {
      const t = performance.now();
      input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }));
      await twoFrames();
      next.push(performance.now() - t);
      if (!vp.querySelector(".log-row-current:not([hidden])")) missing += 1;
    }
    const nextStatus = status();

    const filtered = [];
    for (const q of ["a", "SAFARI"]) {
      await setQuery(q);
      const toggle = await setOnly(true);
      const needle = q.toLowerCase();
      let wrong = 0;
      const check = () => {
        for (const e of vp.querySelectorAll(".log-row:not([hidden]) .log-text")) {
          if (!(e.textContent ?? "").toLowerCase().includes(needle)) wrong += 1;
        }
      };
      check();
      /** @type {number[]} */
      const jumps = [];
      const max = vp.scrollHeight - vp.clientHeight;
      for (let i = 0; i < 50; i++) {
        const t = performance.now();
        vp.scrollTop = Math.round((max * ((i * 37) % 50)) / 49);
        await twoFrames();
        jumps.push(performance.now() - t);
        check();
      }
      vp.scrollTop = 0;
      await frame();
      /** @type {number[]} */
      const intervals = [];
      let last = performance.now();
      for (let i = 0; i < 120; i++) {
        vp.scrollTop += 700;
        await frame();
        const now = performance.now();
        intervals.push(now - last);
        last = now;
        check();
      }
      filtered.push({ query: q, status: status(), rows: Math.round(vp.scrollHeight / 20), toggle, jumps, intervals, wrong });
      await setOnly(false);
    }
    await setQuery("");
    return { typed, next, missing, nextStatus, filtered, rows: vp.querySelectorAll(".log-row").length, lines: Number(vp.dataset.count) };
  }, LOG_SEARCH_QUERIES);
  const longTasks = await page.evaluate(() => /** @type {number[]} */ (/** @type {any} */ (window).__longTasks.splice(0)));
  const round = (/** @type {number} */ v) => Number(v.toFixed(1));
  const stats = (/** @type {number[]} */ values) => ({ runs: values.length, median: round(median(values)), max: round(Math.max(...values)) });
  return {
    lines: r.lines,
    row_elements: r.rows,
    query_ms: r.typed.map((q) => ({ query: q.query, status: q.status, ...stats(q.ms), work: stats(q.work) })),
    next_ms: { ...stats(r.next), status: r.nextStatus, missing_current_rows: r.missing },
    only_matching: r.filtered.map((f) => ({
      query: f.query,
      status: f.status,
      rows: f.rows,
      toggle_ms: round(f.toggle),
      jump_ms: stats(f.jumps),
      scroll_frame_ms: stats(f.intervals),
      wrong_rows: f.wrong,
    })),
    long_tasks: { count: longTasks.length, max_ms: Math.max(0, ...longTasks) },
  };
}

/**
 * The problems in a `measureLogSearch` report: a missed budget, a wrong count or row.
 * @param {Awaited<ReturnType<typeof measureLogSearch>>} s
 * @returns {string[]}
 */
function logSearchProblems(s) {
  /** @type {string[]} */
  const problems = [];
  if (s.lines !== FLOOD_LOG_LINES) problems.push(`perf-log search: the log has ${s.lines} lines, expected ${FLOOD_LOG_LINES}`);
  if (s.row_elements > 80) problems.push(`perf-log search: ${s.row_elements} row elements in the DOM (not virtualized?)`);
  for (const [query, status] of LOG_SEARCH_QUERIES) {
    const q = s.query_ms.find((x) => x.query === query);
    if (!q) continue;
    if (status !== null && q.status !== status) problems.push(`perf-log search: "${query}" gave "${q.status}", expected "${status}"`);
    if (q.max >= LOG_BUDGET_MS) problems.push(`perf-log search: "${query}" took up to ${q.max} ms (budget ${LOG_BUDGET_MS} ms)`);
  }
  if (s.next_ms.max >= LOG_BUDGET_MS) problems.push(`perf-log search: going to the next match took up to ${s.next_ms.max} ms (budget ${LOG_BUDGET_MS} ms)`);
  if (s.next_ms.missing_current_rows > 0) problems.push(`perf-log search: ${s.next_ms.missing_current_rows} next-match steps did not render the current match`);
  for (const f of s.only_matching) {
    if (f.wrong_rows > 0) problems.push(`perf-log search: "Only matching lines" (${f.query}) rendered ${f.wrong_rows} rows without a match`);
    for (const [what, ms] of /** @type {const} */ ([
      ["turning it on", f.toggle_ms],
      ["the slowest jump", f.jump_ms.max],
      ["the slowest scroll frame", f.scroll_frame_ms.max],
    ])) {
      if (ms >= LOG_BUDGET_MS) problems.push(`perf-log search: "Only matching lines" (${f.query}): ${what} took ${ms} ms (budget ${LOG_BUDGET_MS} ms)`);
    }
  }
  if (s.long_tasks.max_ms >= LOG_BUDGET_MS) problems.push(`perf-log search: the longest task took ${s.long_tasks.max_ms} ms (budget ${LOG_BUDGET_MS} ms)`);
  return problems;
}

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const wanted = opts.screens;
  const screens = wanted ? SCREENS.filter((s) => wanted.includes(s.name)) : SCREENS;
  const checks = wanted ? CHECKS.filter((c) => wanted.includes(c.name)) : CHECKS;
  const runPerf = !wanted || wanted.includes("perf");
  const runLogPerf = !wanted || wanted.includes("perf-log");
  if (wanted) {
    const known = new Set(["perf", "perf-log", ...SCREENS.map((s) => s.name), ...CHECKS.map((c) => c.name)]);
    const unknown = wanted.filter((n) => !known.has(n));
    if (unknown.length) throw new Error(`unknown screens: ${unknown.join(", ")}`);
  }
  await mkdir(opts.out, { recursive: true });

  /** @type {string[]} */
  const problems = [];
  const server = startServer(opts.root);
  /** @type {import("playwright").Browser | null} */
  let browser = null;
  try {
    const port = await server.port;
    const base = `http://127.0.0.1:${port}/`;
    process.stdout.write(`serve-ui on port ${port}\n`);
    browser = await chromium.launch();
    for (const scheme of /** @type {const} */ (["light", "dark"])) {
      const context = await newContext(browser, scheme);
      for (const screen of screens) {
        const page = await context.newPage();
        const label = `${screen.name} (${scheme})`;
        watchPage(page, label, problems);
        const file = path.join(opts.out, `${screen.name}-${scheme}.png`);
        try {
          await page.goto(`${base}${screen.query}${screen.hash}`);
          await screen.setup(page);
          await page.waitForTimeout(200);
          if (screen.element) await page.locator(screen.element).first().screenshot({ path: file });
          else await page.screenshot({ path: file, fullPage: !screen.viewport });
          process.stdout.write(`captured ${file}\n`);
        } catch (err) {
          problems.push(`${label}: ${err instanceof Error ? err.message : String(err)}`);
        } finally {
          await page.close();
        }
      }
      await context.close();
    }

    if (checks.length) {
      const context = await newContext(browser, "light");
      for (const check of checks) {
        const page = await context.newPage();
        watchPage(page, check.name, problems);
        try {
          await page.goto(`${base}${check.query ?? "?mock"}${check.hash}`);
          await check.run(page);
          process.stdout.write(`passed ${check.name}\n`);
        } catch (err) {
          problems.push(`${check.name}: ${err instanceof Error ? err.message : String(err)}`);
        } finally {
          await page.close();
        }
      }
      await context.close();
    }

    if (runPerf) {
      const context = await newContext(browser, "light");
      const page = await context.newPage();
      watchPage(page, "perf", problems);
      await page.goto(`${base}?mock${newRunHash}`);
      await newRunReady(page);
      const results = [];
      // A typical query, and "a", which matches (and so expands and lays out) nearly everything.
      for (const query of ["history", "a"]) results.push(await measurePicker(page, query));
      await context.close();
      const report = { budget_ms: PERF_BUDGET_MS, results };
      await writeFile(path.join(opts.out, "perf.json"), `${JSON.stringify(report, null, 2)}\n`);
      process.stdout.write(`perf ${JSON.stringify(report)}\n`);
      for (const r of results) {
        if (r.total_ms_max >= PERF_BUDGET_MS) problems.push(`perf: render + filter "${r.query}" took up to ${r.total_ms_max} ms (budget ${PERF_BUDGET_MS} ms)`);
      }
    }

    if (runLogPerf) {
      const context = await newContext(browser, "light");
      const page = await context.newPage();
      watchPage(page, "perf-log", problems);
      try {
        await page.goto(`${base}?mock${newRunHash}`);
        const r = await measureLog(page);
        const search = await measureLogSearch(page);
        const report = { budget_ms: LOG_BUDGET_MS, ...r, search };
        await writeFile(path.join(opts.out, "perf-log.json"), `${JSON.stringify(report, null, 2)}\n`);
        process.stdout.write(`perf-log ${JSON.stringify(report)}\n`);
        if (r.lines < 100_000) problems.push(`perf-log: only ${r.lines} lines reached the log view`);
        if (r.row_elements > 80) problems.push(`perf-log: ${r.row_elements} row elements in the DOM (not virtualized?)`);
        if (r.jump_wrong_rows > 0) problems.push(`perf-log: ${r.jump_wrong_rows} jumps rendered the wrong rows`);
        for (const [what, ms] of /** @type {const} */ ([
          ["longest task while streaming", r.stream_long_tasks.max_ms],
          ["longest task while scrolling", r.scroll_long_tasks.max_ms],
          ["slowest jump", r.jump_ms.max],
          ["slowest scroll frame", r.scroll_frame_ms.max],
        ])) {
          if (ms >= LOG_BUDGET_MS) problems.push(`perf-log: ${what} took ${ms} ms (budget ${LOG_BUDGET_MS} ms)`);
        }
        problems.push(...logSearchProblems(search));
      } catch (err) {
        problems.push(`perf-log: ${err instanceof Error ? err.message : String(err)}`);
      } finally {
        await context.close();
      }
    }
  } finally {
    await browser?.close();
    server.child.kill();
  }
  if (problems.length) {
    process.stderr.write(`FAILED:\n${problems.map((p) => `  ${p}`).join("\n")}\n`);
    process.exitCode = 1;
  } else {
    process.stdout.write("OK: no CSP violations, console errors or page errors\n");
  }
}

main().catch((err) => {
  process.stderr.write(`${err instanceof Error ? err.stack ?? err.message : String(err)}\n`);
  process.exitCode = 1;
});
