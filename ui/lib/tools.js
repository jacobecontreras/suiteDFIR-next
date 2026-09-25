// @ts-check
/** Tool-state helpers shared by the screens. */

/** @typedef {import("../types").ToolId} ToolId */
/** @typedef {import("../types").ToolState} ToolState */
/** @typedef {import("../types").ToolStatus} ToolStatus */

/** States in which a tool is installed and may be started (`run_start` verifies it again). */
/** @type {ReadonlySet<ToolState>} */
export const RUNNABLE_STATES = new Set(["verified", "installed_unverified", "dev_override"]);

/**
 * @param {readonly ToolStatus[] | null} tools
 * @returns {ToolStatus[]} the installed tools, in the given order
 */
export function installedTools(tools) {
  return (tools ?? []).filter((t) => RUNNABLE_STATES.has(t.state));
}

/**
 * Per-tool options from the pinned manifest (CONTRACTS.md §3 `supports_*`): only iLEAPP takes a
 * timezone, a keychain file and an iTunes backup password.
 * @type {Record<ToolId, { timezone: boolean, keychain: boolean, password: boolean, profileExt: string }>}
 */
export const TOOL_FEATURES = {
  ileapp: { timezone: true, keychain: true, password: true, profileExt: "ilprofile" },
  aleapp: { timezone: false, keychain: false, password: false, profileExt: "alprofile" },
};
