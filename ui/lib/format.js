// @ts-check
/**
 * Display formatting: sizes, counts, durations and times. Pure functions (tests/ui/format.test.js).
 *
 * Times on disk and over IPC are RFC 3339 UTC (`2026-09-24T18:30:05Z`). The UI shows local time,
 * with UTC on hover (DEVELOPMENT.md §4.4): see `formatLocalTime` and `formatUtcTime`.
 */

const DASH = "—";
const BYTE_UNITS = ["KiB", "MiB", "GiB", "TiB", "PiB"];
const COUNT = new Intl.NumberFormat("en-US");

/**
 * Binary (IEC) units: `512 B`, `1.5 KiB`, `57.0 GiB`, `132 MiB`. One decimal below 100.
 * @param {number | null | undefined} bytes
 * @returns {string}
 */
export function formatBytes(bytes) {
  if (typeof bytes !== "number" || !Number.isFinite(bytes) || bytes < 0) return DASH;
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  let value = bytes;
  let unit = -1;
  while (value >= 1024 && unit < BYTE_UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  // 1023.96 KiB would print as "1024.0 KiB"; carry into the next unit instead.
  if (Number(value.toFixed(1)) >= 1024 && unit < BYTE_UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const text = value < 100 ? value.toFixed(1) : String(Math.round(value));
  return `${text} ${BYTE_UNITS[unit]}`;
}

/**
 * `1,300`.
 * @param {number | null | undefined} n
 * @returns {string}
 */
export function formatCount(n) {
  return typeof n === "number" && Number.isFinite(n) ? COUNT.format(n) : DASH;
}

/**
 * `850 ms`, `42 s`, `22 min 36 s`, `1 h 02 min 05 s`. Negative or missing → `—`.
 * @param {number | null | undefined} ms
 * @returns {string}
 */
export function formatDuration(ms) {
  if (typeof ms !== "number" || !Number.isFinite(ms) || ms < 0) return DASH;
  if (ms < 1000) return `${Math.round(ms)} ms`;
  const total = Math.floor(ms / 1000);
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  if (h > 0) return `${h} h ${pad2(m)} min ${pad2(s)} s`;
  if (m > 0) return `${m} min ${s} s`;
  return `${s} s`;
}

/**
 * Parses an RFC 3339 timestamp. Returns null for anything else.
 * @param {string | null | undefined} iso
 * @returns {Date | null}
 */
export function parseTimestamp(iso) {
  if (typeof iso !== "string" || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})$/.test(iso)) {
    return null;
  }
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? null : date;
}

/** @type {Map<string, Intl.DateTimeFormat>} */
const formatters = new Map();

/**
 * @param {string | undefined} timeZone
 * @returns {Intl.DateTimeFormat}
 */
function formatterFor(timeZone) {
  const key = timeZone ?? "";
  let fmt = formatters.get(key);
  if (!fmt) {
    fmt = new Intl.DateTimeFormat("en-US", {
      timeZone,
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      hourCycle: "h23",
    });
    formatters.set(key, fmt);
  }
  return fmt;
}

/**
 * `YYYY-MM-DD HH:MM:SS` in `timeZone` (default: the system's local zone).
 * @param {string | null | undefined} iso
 * @param {string} [timeZone]
 * @returns {string}
 */
export function formatLocalTime(iso, timeZone) {
  const date = parseTimestamp(iso);
  if (!date) return DASH;
  /** @type {Record<string, string>} */
  const parts = {};
  for (const part of formatterFor(timeZone).formatToParts(date)) parts[part.type] = part.value;
  return `${parts.year}-${parts.month}-${parts.day} ${parts.hour}:${parts.minute}:${parts.second}`;
}

/**
 * `YYYY-MM-DD HH:MM:SS UTC`.
 * @param {string | null | undefined} iso
 * @returns {string}
 */
export function formatUtcTime(iso) {
  const date = parseTimestamp(iso);
  return date ? `${formatLocalTime(iso, "UTC")} UTC` : DASH;
}

/** @param {number} n */
function pad2(n) {
  return String(n).padStart(2, "0");
}
