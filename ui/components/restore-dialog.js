// @ts-check
/**
 * "Turn backup encryption off" (ROADMAP D5, ARCHITECTURE.md §6b "Later restore"): asks for the
 * password set during the acquisition and runs `acq_restore_encryption`.
 *
 * - There is no form: Enter in the password field does nothing; only the button changes the device.
 * - The password field is cleared as soon as the command starts.
 * - While the command runs (it may wait for the device passcode, without a time limit), the dialog
 *   cannot be dismissed, by its buttons or by Escape.
 * - A failed attempt can be retried. Each attempt is recorded in its own numbered file; once one
 *   turned encryption off, the core refuses further attempts (`restore_not_applicable`), and the
 *   screens hide the action because the re-read `AcqSummary.warnings` no longer ask for it.
 */
import { toolPasswordProblem } from "../lib/acquire.js";
import { h } from "../lib/dom.js";
import { field } from "../lib/form.js";
import { icon } from "../lib/view.js";
import { errorSlot } from "./app-error.js";
import { modal } from "./dialog.js";

/** @typedef {import("../types").AcqRestoreEncryptionResult} AcqRestoreEncryptionResult */
/** @typedef {import("./dialog.js").Modal} Modal */

/**
 * @param {object} spec
 * @param {{ acq_restore_encryption: (req: import("../types").AcqRestoreEncryptionRequest) => Promise<AcqRestoreEncryptionResult> }} spec.api
 * @param {string} spec.casePath
 * @param {{ acq_id: string, label: string | null, device_name: string | null, udid: string }} spec.acq
 * @param {(result: AcqRestoreEncryptionResult | null) => void} [spec.onSettled] After every attempt:
 *   its result, or null when the command failed. The screens re-read the case's `AcqSummary`.
 * @param {boolean} [spec.windows] The app runs on Windows: the iOS tools take only printable ASCII
 *   passwords, so another one is refused inline before anything is sent.
 * @returns {Modal}
 */
export function restoreEncryptionDialog(spec) {
  const { acq } = spec;
  let running = false;
  let done = false;
  const password = /** @type {HTMLInputElement} */ (
    h("input", { class: "input", type: "password", name: "backup_password", autocomplete: "off", spellcheck: "false" })
  );
  const pwField = field({
    label: "Backup password",
    control: password,
    required: true,
    hint: "The password set when this acquisition turned backup encryption on. It is used once and not stored.",
  });
  const errors = errorSlot();
  const result = h("div", { class: "restore-result", role: "status" });
  const confirm = /** @type {HTMLButtonElement} */ (h("button", { class: "btn btn-primary", type: "button", disabled: true, onClick: run }, "Turn encryption off"));
  const close = /** @type {HTMLButtonElement} */ (h("button", { class: "btn", type: "button", onClick: () => dialog.close() }, "Cancel"));
  const device = acq.device_name ?? "the device";

  /** The password cannot be sent: empty, or one the iOS tools cannot take here. */
  const unusable = () => password.value === "" || toolPasswordProblem(password.value, spec.windows === true) !== null;
  password.addEventListener("input", () => {
    pwField.setError(toolPasswordProblem(password.value, spec.windows === true));
    confirm.disabled = running || done || unusable();
  });

  const dialog = modal({
    title: "Turn backup encryption off",
    onClose: () => {
      password.value = "";
      document.removeEventListener("keydown", holdEscape, true);
      errors.dispose();
    },
    content: () =>
      h(
        "div",
        { class: "modal-body" },
        h(
          "p",
          null,
          `This turns backup encryption off on ${device} (`,
          h("span", { class: "mono" }, acq.udid),
          `), which acquisition ${acq.label ? `“${acq.label}”` : acq.acq_id} turned on.`,
        ),
        h(
          "ul",
          { class: "list-compact small" },
          h("li", null, "Connect the device, unlock it, and make sure it is paired with this computer."),
          h("li", null, "If the device asks for its passcode, enter it on the device. The app waits for the device without a time limit."),
          h("li", null, "Each attempt is recorded in its own numbered restore record in the acquisition folder; acquisition.json is not changed."),
        ),
        pwField.node,
        errors.node,
        result,
        h("div", { class: "modal-actions" }, close, confirm),
      ),
  });
  // Escape would leave a device change running unseen: not while the command runs. Chromium (and
  // WebView2) close a dialog on a second Escape even when its `cancel` event was prevented (the
  // close-watcher rule), so the key itself is stopped too, wherever the focus is (the pressed
  // button is disabled while the command runs).
  /** @param {KeyboardEvent} event */
  const holdEscape = (event) => {
    if (running && event.key === "Escape") event.preventDefault();
  };
  document.addEventListener("keydown", holdEscape, true);
  dialog.node.addEventListener("cancel", (event) => {
    if (running) event.preventDefault();
  });

  async function run() {
    if (running || done || unusable()) return;
    const secret = password.value;
    password.value = "";
    running = true;
    errors.clear();
    result.replaceChildren();
    confirm.disabled = true;
    confirm.textContent = "Turning off…";
    close.disabled = true;
    pwField.setError(null);
    result.replaceChildren(h("p", { class: "muted" }, "Waiting for the device. If it asks for its passcode, enter it on the device."));
    /** @type {AcqRestoreEncryptionResult | null} */
    let outcome = null;
    try {
      const r = await spec.api.acq_restore_encryption({ case_path: spec.casePath, acq_id: acq.acq_id, password: secret });
      outcome = r;
      done = r.restored;
      result.replaceChildren(
        r.restored
          ? h("div", { class: "banner banner-ok" }, icon("check-circle"), h("p", null, h("strong", null, "Backup encryption is off. "), "The device no longer encrypts backups."))
          : h(
              "div",
              { class: "banner banner-warn" },
              icon("alert-triangle"),
              h(
                "p",
                null,
                h("strong", null, r.will_encrypt_after === true ? "Backup encryption is still on. " : "The encryption state could not be read. "),
                "Check the password and try again; each attempt is recorded.",
              ),
            ),
      );
    } catch (err) {
      result.replaceChildren();
      errors.show(err, "Backup encryption could not be turned off.");
    } finally {
      running = false;
      close.disabled = false;
      close.textContent = done ? "Close" : "Cancel";
      confirm.hidden = done;
      confirm.textContent = "Turn encryption off";
      confirm.disabled = done || unusable();
    }
    spec.onSettled?.(outcome);
  }

  const open = dialog.open;
  return {
    ...dialog,
    open() {
      open();
      password.focus();
    },
  };
}
