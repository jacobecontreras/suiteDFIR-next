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
//   frame reaches 100 ms, and the log stays virtualized (<out>/perf-log.json).
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
        const report = { budget_ms: LOG_BUDGET_MS, ...r };
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
