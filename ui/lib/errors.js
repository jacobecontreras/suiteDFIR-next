// @ts-check
/** @typedef {import("../types").AppError} AppError */

/**
 * True for an `AppError`-shaped value (CONTRACTS.md §9).
 * @param {unknown} value
 * @returns {value is AppError}
 */
export function isAppError(value) {
  if (typeof value !== "object" || value === null) return false;
  const v = /** @type {Record<string, unknown>} */ (value);
  return typeof v.code === "string" && typeof v.message === "string" && (v.detail === null || typeof v.detail === "string");
}

/**
 * Normalizes anything a command or UI code threw into an `AppError`, so every failure is shown by
 * the one AppError component. Non-AppError values become `internal`.
 * @param {unknown} err
 * @returns {AppError}
 */
export function toAppError(err) {
  if (isAppError(err)) return err;
  if (err instanceof Error) {
    return { code: "internal", message: err.message || "Unexpected error.", detail: err.stack ?? null };
  }
  return { code: "internal", message: "Unexpected error.", detail: typeof err === "string" ? err : safeJson(err) };
}

/** @param {unknown} value */
function safeJson(value) {
  try {
    return JSON.stringify(value) ?? null;
  } catch {
    return null;
  }
}
