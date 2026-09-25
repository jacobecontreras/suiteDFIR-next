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

/**
 * A Run screen held in `phase` by the mock's `hold_<phase>` flag.
 * @param {string} phase
 * @param {string} label The phase's label in the step list.
 * @param {string} buttonName
 * @param {string} sample
 * @returns {Screen}
 */
function runPhaseScreen(phase, label, buttonName, sample) {
  return {
    name: `run-phase-${phase.replaceAll("_", "-")}`,
    query: `?mock&scenario=hold_${phase}`,
    hash: newRunHash,
    setup: async (page) => {
      await startRun(page, buttonName, sample);
      await page.locator(".phase-step-current", { hasText: label }).waitFor();
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
    // UTC shown on keyboard focus (Tab from the last column header to the first row's time).
    name: "case-runs-utc-focus",
    query: "?mock",
    hash: caseHash(NIGHTJAR),
    element: ".table-wrap",
    setup: async (page) => {
      await page.locator(".runs-table").waitFor();
      await page.getByRole("button", { name: /^Duration/ }).press("Tab");
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
  runPhaseScreen("sealing_report", "Sealing report", "Choose folder…", "Pixel-7-extraction"),
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
          await page.goto(`${base}?mock${check.hash}`);
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
