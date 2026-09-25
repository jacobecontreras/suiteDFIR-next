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
 * The session's timezone list, cached in the store. Uses iLEAPP's list when it is installed.
 * @param {Api} api
 * @param {import("./store.js").Store<AppState>} store
 * @returns {Promise<string[]>}
 */
export async function loadTimezones(api, store) {
  const cached = store.get().timezones;
  if (cached) return cached;
  /** @type {string[] | null} */
  let zones = null;
  if (installedTools(store.get().tools).some((t) => t.tool === "ileapp")) {
    try {
      zones = (await api.tool_modules({ tool: "ileapp" })).timezones;
    } catch {
      zones = null;
    }
  }
  const list = timezoneList(zones);
  store.set({ timezones: list });
  return list;
}
