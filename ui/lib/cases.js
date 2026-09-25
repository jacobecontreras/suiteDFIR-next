// @ts-check
/** Case form helpers (tests/ui/cases.test.js). */

/** @typedef {import("../types").CaseFields} CaseFields */
/** @typedef {import("../types").CaseFile} CaseFile */

/** `case.json` `name`: 1–120 characters (CONTRACTS.md §6). */
export const CASE_NAME_MAX = 120;

/**
 * Client-side checks for the editable case fields; the core validates again.
 * @param {Pick<CaseFields, "name">} fields
 * @returns {Partial<Record<keyof CaseFields, string>>} messages by field; empty when valid
 */
export function validateCaseFields(fields) {
  /** @type {Partial<Record<keyof CaseFields, string>>} */
  const errors = {};
  const name = fields.name.trim();
  if (name.length === 0) errors.name = "Enter a case name.";
  else if (name.length > CASE_NAME_MAX) errors.name = `The case name can be at most ${CASE_NAME_MAX} characters.`;
  return errors;
}

/**
 * The editable fields of a case (`case_update` sends all of them).
 * @param {CaseFile} file
 * @returns {CaseFields}
 */
export function editableFields(file) {
  return {
    name: file.name,
    case_number: file.case_number,
    examiner: file.examiner,
    agency: file.agency,
    description: file.description,
    default_timezone: file.default_timezone,
  };
}

/**
 * The last path segment, for naming a case whose case.json is unreadable.
 * @param {string} path
 * @returns {string}
 */
export function folderLabel(path) {
  return path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path;
}
