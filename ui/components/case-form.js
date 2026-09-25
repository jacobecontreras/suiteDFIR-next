// @ts-check
/**
 * The case metadata form, shared by "New case" and the Case screen's edit mode. It edits exactly
 * the editable `case.json` fields (CONTRACTS.md §6).
 */
import { CASE_NAME_MAX, validateCaseFields } from "../lib/cases.js";
import { h } from "../lib/dom.js";
import { field, selectInput, textInput } from "../lib/form.js";
import { errorSlot } from "./app-error.js";

/** @typedef {import("../types").CaseFields} CaseFields */
/** @typedef {import("../lib/context").View} View */

/**
 * @param {object} spec
 * @param {CaseFields} spec.initial
 * @param {readonly string[]} spec.timezones
 * @param {string} spec.appDefaultTimezone `settings.defaults.timezone` (shown for "app default")
 * @param {string} spec.submitLabel
 * @param {Node} [spec.extra] Extra content before the buttons (e.g. the location of a new case).
 * @param {(fields: CaseFields) => Promise<void>} spec.onSubmit Throws to show an AppError.
 * @param {() => void} spec.onCancel
 * @returns {View & { focus: () => void, showError: (error: unknown) => void }}
 */
export function caseForm(spec) {
  const init = spec.initial;
  const name = textInput({ name: "name", value: init.name, maxlength: CASE_NAME_MAX, autocomplete: "off" });
  const caseNumber = textInput({ name: "case_number", value: init.case_number, autocomplete: "off" });
  const examiner = textInput({ name: "examiner", value: init.examiner, autocomplete: "off" });
  const agency = textInput({ name: "agency", value: init.agency, autocomplete: "off" });
  const description = /** @type {HTMLTextAreaElement} */ (h("textarea", { class: "input", name: "description", rows: 3 }));
  description.value = init.description;

  const zones = [...spec.timezones];
  if (init.default_timezone && !zones.includes(init.default_timezone)) zones.unshift(init.default_timezone);
  const appDefault = spec.appDefaultTimezone || "UTC";
  const timezone = selectInput(
    [{ value: "", label: `App default (${appDefault})` }, ...zones.map((z) => ({ value: z, label: z }))],
    init.default_timezone ?? "",
    { name: "default_timezone" },
  );

  const nameField = field({ label: "Case name", control: name, required: true, hint: "Also the folder name. Renaming later does not rename the folder." });
  const errors = errorSlot();
  const submit = /** @type {HTMLButtonElement} */ (h("button", { class: "btn btn-primary", type: "submit" }, spec.submitLabel));

  const node = h(
    "form",
    { class: "form", novalidate: true, onSubmit },
    nameField.node,
    h(
      "div",
      { class: "form-row" },
      field({ label: "Case number", control: caseNumber }).node,
      field({ label: "Examiner", control: examiner }).node,
      field({ label: "Agency", control: agency }).node,
    ),
    field({ label: "Description", control: description }).node,
    field({
      label: "Default timezone",
      control: timezone,
      hint: "Used for iLEAPP runs in this case unless changed in the run.",
    }).node,
    spec.extra,
    errors.node,
    h(
      "div",
      { class: "form-actions" },
      h("button", { class: "btn", type: "button", onClick: () => spec.onCancel() }, "Cancel"),
      submit,
    ),
  );

  /** @param {SubmitEvent} event */
  async function onSubmit(event) {
    event.preventDefault();
    errors.clear();
    /** @type {CaseFields} */
    const fields = {
      name: name.value.trim(),
      case_number: caseNumber.value.trim(),
      examiner: examiner.value.trim(),
      agency: agency.value.trim(),
      description: description.value,
      default_timezone: timezone.value === "" ? null : timezone.value,
    };
    const invalid = validateCaseFields(fields);
    nameField.setError(invalid.name ?? null);
    if (invalid.name) {
      name.focus();
      return;
    }
    submit.disabled = true;
    try {
      await spec.onSubmit(fields);
    } catch (err) {
      errors.show(err);
    } finally {
      submit.disabled = false;
    }
  }

  return {
    node,
    focus: () => name.focus(),
    showError: (error) => errors.show(error),
    dispose: () => errors.dispose(),
  };
}
