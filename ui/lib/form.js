// @ts-check
/**
 * Labelled form fields with a hint and an inline error (wired with `aria-describedby` and
 * `aria-invalid`). Plain node helpers: nothing to dispose.
 */
import { h } from "./dom.js";
import { uid } from "./view.js";

/**
 * @typedef {object} Field
 * @property {HTMLElement} node The wrapper (label, control, hint, error).
 * @property {(message: string | null) => void} setError Shows or clears the inline error.
 */

/**
 * @param {object} spec
 * @param {string} spec.label
 * @param {HTMLElement} spec.control An input, select or textarea (gets an id if it has none).
 * @param {string} [spec.hint]
 * @param {boolean} [spec.required] Marks the label; validation is the caller's.
 * @param {string} [spec.className]
 * @returns {Field}
 */
export function field(spec) {
  const control = spec.control;
  if (!control.id) control.id = uid("field");
  const hintId = spec.hint ? `${control.id}-hint` : null;
  const errorId = `${control.id}-error`;
  const error = h("p", { class: "field-error", id: errorId, hidden: true });
  const describe = (/** @type {boolean} */ withError) => {
    const ids = [hintId, withError ? errorId : null].filter(Boolean).join(" ");
    if (ids) control.setAttribute("aria-describedby", ids);
    else control.removeAttribute("aria-describedby");
  };
  describe(false);
  if (spec.required) control.setAttribute("aria-required", "true");
  const node = h(
    "div",
    { class: spec.className ? `field ${spec.className}` : "field" },
    h("label", { class: "field-label", for: control.id }, spec.label, spec.required && h("span", { class: "req", "aria-hidden": "true" }, " *")),
    control,
    spec.hint && h("p", { class: "field-hint", id: hintId }, spec.hint),
    error,
  );
  return {
    node,
    setError(message) {
      error.textContent = message ?? "";
      error.hidden = !message;
      if (message) control.setAttribute("aria-invalid", "true");
      else control.removeAttribute("aria-invalid");
      describe(Boolean(message));
    },
  };
}

/**
 * @param {Record<string, string | number | boolean | null | undefined>} [attrs]
 * @returns {HTMLInputElement}
 */
export function textInput(attrs = {}) {
  const { value, ...rest } = attrs;
  const input = /** @type {HTMLInputElement} */ (h("input", { class: "input", type: "text", ...rest }));
  if (value !== undefined && value !== null) input.value = String(value);
  return input;
}

/**
 * @param {{ value: string, label: string }[]} options
 * @param {string} selected
 * @param {Record<string, string | boolean | null | undefined>} [attrs]
 * @returns {HTMLSelectElement}
 */
export function selectInput(options, selected, attrs = {}) {
  const select = /** @type {HTMLSelectElement} */ (
    h(
      "select",
      { class: "input", ...attrs },
      options.map((o) => h("option", { value: o.value }, o.label)),
    )
  );
  select.value = selected;
  return select;
}
