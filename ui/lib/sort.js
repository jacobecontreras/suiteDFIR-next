// @ts-check
/**
 * Sorting for the Case screen's runs table (tests/ui/sort.test.js).
 */

/** @typedef {import("../types").RunSummary} RunSummary */
/** @typedef {import("../types").RunStatus} RunStatus */
/** @typedef {"created_at" | "status" | "tool" | "label" | "input" | "duration"} RunSortKey */
/** @typedef {"asc" | "desc"} SortDir */
/** @typedef {{ key: RunSortKey, dir: SortDir }} RunSort */

/** Status sort order: active first, then by severity, success last. */
/** @type {Record<RunStatus, number>} */
export const STATUS_ORDER = {
  running: 0,
  failed: 1,
  completed_with_errors: 2,
  interrupted: 3,
  cancelled: 4,
  succeeded: 5,
};

/** The initial sort: newest first. */
/** @type {RunSort} */
export const DEFAULT_RUN_SORT = { key: "created_at", dir: "desc" };

const collator = new Intl.Collator("en", { sensitivity: "base", numeric: true });

/**
 * @param {RunSummary} run
 * @param {RunSortKey} key
 * @returns {string | number | null} null = missing (always sorted last)
 */
function valueOf(run, key) {
  switch (key) {
    case "created_at":
      return run.created_at;
    case "status":
      return STATUS_ORDER[run.status] ?? 99;
    case "tool":
      return `${run.tool} ${run.tool_version}`;
    case "label":
      return run.label?.trim() || null;
    case "input":
      return run.input_path;
    case "duration":
      return run.duration_ms;
  }
}

/**
 * @param {string | number} a
 * @param {string | number} b
 * @param {RunSortKey} key
 */
function compareValues(a, b, key) {
  if (typeof a === "number" && typeof b === "number") return a - b;
  // RFC 3339 UTC at second precision sorts chronologically as plain text.
  if (key === "created_at") return a < b ? -1 : a > b ? 1 : 0;
  return collator.compare(String(a), String(b));
}

/**
 * Returns a sorted copy. Missing values (no label, no duration) go last in both directions; ties
 * are broken newest first, then by run id, so the order is total and stable.
 * @param {readonly RunSummary[]} runs
 * @param {RunSort} sort
 * @returns {RunSummary[]}
 */
export function sortRuns(runs, sort) {
  const sign = sort.dir === "asc" ? 1 : -1;
  return [...runs].sort((a, b) => {
    const va = valueOf(a, sort.key);
    const vb = valueOf(b, sort.key);
    if (va === null || vb === null) {
      if (va !== vb) return va === null ? 1 : -1;
    } else {
      const c = compareValues(va, vb, sort.key);
      if (c !== 0) return sign * c;
    }
    return compareValues(b.created_at, a.created_at, "created_at") || (a.run_id < b.run_id ? -1 : a.run_id > b.run_id ? 1 : 0);
  });
}

/**
 * The sort after clicking a column header: the same column flips direction; a new column starts
 * descending for dates and durations, ascending otherwise.
 * @param {RunSort} current
 * @param {RunSortKey} key
 * @returns {RunSort}
 */
export function nextSort(current, key) {
  if (current.key === key) return { key, dir: current.dir === "asc" ? "desc" : "asc" };
  return { key, dir: key === "created_at" || key === "duration" ? "desc" : "asc" };
}
