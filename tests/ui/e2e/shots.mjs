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
// starts a run. It fails (exit 1) on a failed check, on any console CSP violation, console error or
// uncaught page error (screens, checks and perf alike), and when the module-picker measurement
// (`perf`: render + filter of 1,300 fixture modules) is not under 100 ms. The measurement is written
// to <out>/perf.json.
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
      await newRunReady(page);
      await pickInput(page, "Choose folder…", "Evidence/interrupt");
      await page.locator(".ready-text").waitFor();
      await page.getByRole("button", { name: "Start run" }).click();
      await page.locator(".runs-table .badge-running").waitFor();
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
      await page.locator(".runs-table .badge-running").waitFor();
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
      await page.locator(".runs-table .badge-running").waitFor();
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

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const wanted = opts.screens;
  const screens = wanted ? SCREENS.filter((s) => wanted.includes(s.name)) : SCREENS;
  const checks = wanted ? CHECKS.filter((c) => wanted.includes(c.name)) : CHECKS;
  const runPerf = !wanted || wanted.includes("perf");
  if (wanted) {
    const known = new Set(["perf", ...SCREENS.map((s) => s.name), ...CHECKS.map((c) => c.name)]);
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
