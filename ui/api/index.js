// @ts-check
/**
 * Chooses the API implementation (ARCHITECTURE.md §5.3):
 * - the real IPC (`./ipc.js`) when `window.__TAURI__` exists;
 * - the browser mock (`/dev/mock.js`, served only by scripts/serve-ui.mjs from ui-dev/, never
 *   bundled) when it does not **and** the URL has `?mock`;
 * - otherwise nothing, and the app shows an error screen.
 *
 * This module only tests for `window.__TAURI__`; ipc.js is the only one that calls into it.
 */

/** @typedef {typeof import("./ipc.js")} Api */
/** @typedef {"ipc" | "mock"} ApiMode */

const MOCK_URL = "/dev/mock.js";

/** @returns {Promise<{ mode: ApiMode, api: Api } | null>} */
export async function loadApi() {
  if (window.__TAURI__) {
    return { mode: "ipc", api: await import("./ipc.js") };
  }
  if (new URLSearchParams(window.location.search).has("mock")) {
    // A variable specifier: the mock is not part of ui/ and must not be resolved at build time.
    const url = MOCK_URL;
    /** @type {Api} */
    const api = await import(url);
    return { mode: "mock", api };
  }
  return null;
}
