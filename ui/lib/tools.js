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
 * Per-tool options from the pinned manifest (CONTRACTS.md §3 `supports_*` and `input_types`): only
 * iLEAPP takes a timezone, a keychain file, an iTunes backup password and iTunes backups (`itunes`).
 * @type {Record<ToolId, { timezone: boolean, keychain: boolean, password: boolean, itunes: boolean, profileExt: string }>}
 */
export const TOOL_FEATURES = {
  ileapp: { timezone: true, keychain: true, password: true, itunes: true, profileExt: "ilprofile" },
  aleapp: { timezone: false, keychain: false, password: false, itunes: false, profileExt: "alprofile" },
};
