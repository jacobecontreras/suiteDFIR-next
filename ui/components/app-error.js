// @ts-check
/**
 * The one AppError display component (DEVELOPMENT.md §4.6): every command failure is shown
 * through it, with the code, the message and an expandable detail. All text is set with text
 * nodes; `detail` may be evidence-derived.
 */
import { h } from "../lib/dom.js";
import { toAppError } from "../lib/errors.js";
import { icon } from "../lib/view.js";

/** @typedef {import("../types").AppError} AppError */

/**
 * @param {unknown} error an `AppError`, or anything thrown (normalized by `toAppError`)
 * @param {{ title?: string, onDismiss?: () => void }} [options]
 * @returns {{ node: HTMLElement, dispose: () => void }}
 */
export function appError(error, options = {}) {
  const err = toAppError(error);
  const node = h(
    "div",
    { class: "app-error", role: "alert" },
    icon("x-circle", "icon app-error-icon"),
    h(
      "div",
      { class: "app-error-body" },
      options.title && h("p", { class: "app-error-title" }, options.title),
      h("p", { class: "app-error-message" }, err.message),
      h("p", { class: "app-error-code" }, "Error code: ", h("code", null, err.code)),
      err.detail &&
        h(
          "details",
          { class: "app-error-detail" },
          h("summary", null, "Details"),
          h("pre", { class: "pre" }, err.detail),
        ),
    ),
    options.onDismiss &&
      h("button", { class: "btn btn-sm btn-plain app-error-dismiss", type: "button", onClick: options.onDismiss }, "Dismiss"),
  );
  return { node, dispose() {} };
}

/**
 * A slot that shows at most one AppError, e.g. the result of the last action in a section.
 * @returns {{ node: HTMLElement, show: (error: unknown, title?: string) => void, clear: () => void, dispose: () => void }}
 */
export function errorSlot() {
  const node = h("div", { class: "error-slot" });
  const clear = () => node.replaceChildren();
  return {
    node,
    show(error, title) {
      node.replaceChildren(appError(error, { title, onDismiss: clear }).node);
    },
    clear,
    dispose: clear,
  };
}
