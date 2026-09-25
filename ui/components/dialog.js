// @ts-check
/**
 * In-DOM modal dialogs (`<dialog>` + `showModal()`; never `window.alert`/`confirm`,
 * DEVELOPMENT.md §4.6). The browser traps focus, closes on Escape and restores focus on close.
 */
import { h } from "../lib/dom.js";
import { uid } from "../lib/view.js";

/**
 * @typedef {object} Modal
 * @property {HTMLDialogElement} node
 * @property {() => void} open Attaches the dialog and shows it modally.
 * @property {() => void} close Closes and detaches it (calls `onClose` once).
 * @property {() => void} dispose Same as `close`.
 */

/**
 * @param {object} spec
 * @param {string} spec.title
 * @param {(titleId: string) => Node} spec.content Builds the body; the title element is given its id.
 * @param {boolean} [spec.wide]
 * @param {() => void} [spec.onClose]
 * @returns {Modal}
 */
export function modal(spec) {
  const titleId = uid("dialog-title");
  const dialog = /** @type {HTMLDialogElement} */ (
    h(
      "dialog",
      { class: spec.wide ? "modal modal-wide" : "modal", "aria-labelledby": titleId },
      h("h2", { class: "modal-title", id: titleId }, spec.title),
      spec.content(titleId),
    )
  );
  let closed = false;
  const finish = () => {
    if (closed) return;
    closed = true;
    dialog.remove();
    spec.onClose?.();
  };
  dialog.addEventListener("close", finish);
  const close = () => {
    if (dialog.open) dialog.close();
    finish();
  };
  return {
    node: dialog,
    open() {
      document.body.append(dialog);
      dialog.showModal();
    },
    close,
    dispose: close,
  };
}

/**
 * A confirm dialog. Resolves true if confirmed, false if cancelled or dismissed (Escape). Both
 * buttons are plain buttons: Enter activates only the focused one, which is the dismissing button
 * when the dialog opens.
 * @param {{ title: string, message: string | Node, confirmLabel: string, cancelLabel?: string, danger?: boolean }} spec
 * @returns {Promise<boolean>}
 */
export function confirmDialog(spec) {
  return new Promise((resolve) => {
    let result = false;
    const dismiss = /** @type {HTMLButtonElement} */ (h("button", { class: "btn", type: "button", onClick: () => m.close() }, spec.cancelLabel ?? "Cancel"));
    const m = modal({
      title: spec.title,
      onClose: () => resolve(result),
      content: () =>
        h(
          "div",
          { class: "modal-body" },
          typeof spec.message === "string" ? h("p", null, spec.message) : spec.message,
          h(
            "div",
            { class: "modal-actions" },
            dismiss,
            h(
              "button",
              {
                class: spec.danger ? "btn btn-danger" : "btn btn-primary",
                type: "button",
                onClick: () => {
                  result = true;
                  m.close();
                },
              },
              spec.confirmLabel,
            ),
          ),
        ),
    });
    m.open();
    dismiss.focus();
  });
}
