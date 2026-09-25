// @ts-check
/**
 * Entry point: picks the API (api/index.js), loads the start-up state into the store, renders the
 * shell and runs the hash router.
 */
import { loadApi } from "./api/index.js";
import { appError } from "./components/app-error.js";
import { shell } from "./components/shell.js";
import { h } from "./lib/dom.js";
import { pollActiveJob } from "./lib/jobs.js";
import { parseRoute } from "./lib/router.js";
import { createStore } from "./lib/store.js";
import { caseScreen } from "./screens/case.js";
import { casesScreen } from "./screens/cases.js";
import { newRunScreen } from "./screens/new-run.js";
import { notFoundScreen } from "./screens/not-found.js";

/** @typedef {import("./lib/context").AppState} AppState */
/** @typedef {import("./lib/context").ScreenContext} ScreenContext */
/** @typedef {import("./lib/context").View} View */

/** @type {Record<string, (ctx: ScreenContext) => View>} */
const ROUTES = {
  cases: casesScreen,
  case: caseScreen,
  "new-run": newRunScreen,
};

/** How often `job_active` is polled while a job is active. */
const JOB_POLL_MS = 2000;

async function main() {
  const root = document.getElementById("app");
  if (!root) return;

  const loaded = await loadApi().catch(() => null);
  if (!loaded) {
    root.replaceChildren(noApiScreen());
    return;
  }
  const { api, mode } = loaded;

  const store = createStore(
    /** @type {AppState} */ ({ mode, appInfo: null, settings: null, tools: null, activeJob: null, timezones: null }),
  );
  const chrome = shell({ store });
  root.replaceChildren(chrome.node);

  try {
    const [appInfo, settings, tools, activeJob] = await Promise.all([
      api.app_info(),
      api.settings_get(),
      api.tools_status(),
      api.job_active(),
    ]);
    store.set({ appInfo, settings, tools, activeJob });
  } catch (err) {
    chrome.main.replaceChildren(
      h("section", { class: "screen" }, h("h1", null, "suiteDFIR could not start"), appError(err).node),
    );
    return;
  }

  pollActiveJob(api, store, JOB_POLL_MS);

  /** @type {View | null} */
  let current = null;
  let first = true;
  const render = () => {
    const route = parseRoute(window.location.hash);
    current?.dispose();
    const factory = ROUTES[route.name] ?? notFoundScreen;
    current = factory({ api, store, params: route.params, navigate });
    chrome.main.replaceChildren(current.node);
    chrome.setRoute(route.name);
    // Move focus to the new screen's heading so keyboard and screen-reader users follow the change.
    if (!first) current.node.querySelector("h1")?.focus();
    first = false;
  };
  /** @param {string} href */
  function navigate(href) {
    if (window.location.hash === href) render();
    else window.location.hash = href;
  }
  window.addEventListener("hashchange", render);
  render();
}

/** The error screen when neither the app API nor the mock is available (ARCHITECTURE.md §5.3). */
function noApiScreen() {
  return h(
    "main",
    { class: "content" },
    h(
      "section",
      { class: "screen" },
      h("h1", null, "suiteDFIR"),
      appError({
        code: "internal",
        message: "This page must run inside the suiteDFIR app.",
        detail:
          "The app API (window.__TAURI__) is not available. For UI development, serve the UI with scripts/serve-ui.mjs and open it with ?mock for the browser mock.",
      }).node,
    ),
  );
}

main();
