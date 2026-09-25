// @ts-check
/**
 * Timezone lists (DEVELOPMENT.md §4.6): the installed iLEAPP's own list when available, otherwise
 * `Intl.supportedValuesOf("timeZone")`, otherwise a short embedded list.
 */
import { installedTools } from "./tools.js";

/** @typedef {import("./context").Api} Api */
/** @typedef {import("./context").AppState} AppState */

/** Used only when neither iLEAPP nor `Intl` provides a list. */
export const FALLBACK_TIMEZONES = Object.freeze([
  "UTC",
  "Africa/Johannesburg",
  "America/Anchorage",
  "America/Chicago",
  "America/Denver",
  "America/Los_Angeles",
  "America/New_York",
  "America/Phoenix",
  "America/Sao_Paulo",
  "Asia/Dubai",
  "Asia/Kolkata",
  "Asia/Shanghai",
  "Asia/Singapore",
  "Asia/Tokyo",
  "Australia/Sydney",
  "Europe/Berlin",
  "Europe/London",
  "Europe/Madrid",
  "Europe/Paris",
  "Pacific/Auckland",
  "Pacific/Honolulu",
]);

/**
 * @param {readonly string[] | null | undefined} toolZones `tool_modules("ileapp").timezones`
 * @param {((key: "timeZone") => string[]) | undefined} [intlValues] `Intl.supportedValuesOf`
 * @returns {string[]}
 */
export function timezoneList(toolZones, intlValues = typeof Intl.supportedValuesOf === "function" ? Intl.supportedValuesOf : undefined) {
  if (toolZones && toolZones.length > 0) return [...toolZones];
  /** @type {string[]} */
  let fromIntl = [];
  try {
    fromIntl = intlValues ? intlValues("timeZone") : [];
  } catch {
    fromIntl = [];
  }
  // Browsers leave "UTC" out of their list; it is the app's default, so always offer it.
  if (fromIntl.length > 0) return fromIntl.includes("UTC") ? [...fromIntl] : ["UTC", ...fromIntl];
  return [...FALLBACK_TIMEZONES];
}

/**
 * The first candidate that is in `list` (e.g. case → settings → "UTC", D19), else the first entry.
 * @param {readonly string[]} list
 * @param {readonly (string | null | undefined)[]} candidates
 * @returns {string | null}
 */
export function pickTimezone(list, candidates) {
  for (const zone of candidates) {
    if (zone && list.includes(zone)) return zone;
  }
  return list[0] ?? null;
}

/**
 * Caches iLEAPP's own zone list (from `tool_modules("ileapp")`) for its installed version.
 * Only a tool list is cached; Intl and the embedded list are never cached, so a later call can
 * still get iLEAPP's list.
 * @param {import("./store.js").Store<AppState>} store
 * @param {string} version the `ToolModules.version` the list came from
 * @param {readonly string[] | null} zones
 */
export function rememberToolTimezones(store, version, zones) {
  if (zones && zones.length > 0) store.set({ timezones: { version, list: [...zones] } });
}

/**
 * The timezone list for forms: iLEAPP's own list when iLEAPP is installed (cached per installed
 * version), otherwise `Intl`, otherwise the embedded list (both uncached).
 * @param {Api} api
 * @param {import("./store.js").Store<AppState>} store
 * @returns {Promise<string[]>}
 */
export async function loadTimezones(api, store) {
  const ileapp = installedTools(store.get().tools).find((t) => t.tool === "ileapp");
  const cached = store.get().timezones;
  if (ileapp && cached && cached.version === ileapp.installed_version) return [...cached.list];
  if (ileapp) {
    try {
      const mods = await api.tool_modules({ tool: "ileapp" });
      rememberToolTimezones(store, mods.version, mods.timezones);
      if (mods.timezones && mods.timezones.length > 0) return [...mods.timezones];
    } catch {
      // Fall through to the uncached fallback; the next call tries iLEAPP again.
    }
  }
  return timezoneList(null);
}
