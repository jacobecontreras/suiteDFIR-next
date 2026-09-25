// @ts-check
// Mock stand-in for the native open/save dialogs: an in-DOM <dialog> listing sample paths (each
// named after the mock scenario it triggers) plus a free-text path field. Not shipped. Built with
// plain DOM APIs and textContent only; it reuses the app's generic CSS classes.

/** @typedef {import("../../ui/api/ipc.js").OpenDialogOptions} OpenDialogOptions */
/** @typedef {import("../../ui/api/ipc.js").SaveDialogOptions} SaveDialogOptions */

/**
 * @typedef {object} Sample
 * @property {string} path
 * @property {"dir" | "file"} kind
 * @property {string} group
 * @property {string} note
 */

const CASES = "/Users/examiner/Documents/suiteDFIR Cases";
const EV = "/Volumes/Evidence";
const DL = "/Users/examiner/Downloads";

/** @type {Sample[]} */
export const SAMPLES = [
  { kind: "dir", group: "Case folders", path: `${CASES}/Operation Nightjar`, note: "valid case" },
  { kind: "dir", group: "Case folders", path: `${CASES}/Harbor Lights`, note: "valid case" },
  { kind: "dir", group: "Case folders", path: `${CASES}/Cold Case 7`, note: "valid case, not in the recent list" },
  { kind: "dir", group: "Case folders", path: "/Users/examiner/Desktop/Photos", note: "no case.json → invalid_case" },
  { kind: "dir", group: "Parent folders", path: CASES, note: "the cases root" },
  { kind: "dir", group: "Parent folders", path: "/Volumes/Cases", note: "another writable folder" },
  { kind: "dir", group: "Parent folders", path: "/Volumes/ReadOnly", note: "→ permission_denied" },
  { kind: "dir", group: "Evidence folders", path: `${EV}/00008101-000A1B2C3D4E`, note: "iTunes backup, encrypted" },
  { kind: "dir", group: "Evidence folders", path: `${EV}/iPhone-11-backup`, note: "iTunes backup, not encrypted" },
  { kind: "dir", group: "Evidence folders", path: `${EV}/Pixel-7-extraction`, note: "file system folder" },
  { kind: "dir", group: "Evidence folders", path: `${CASES}/Operation Nightjar/runs`, note: "→ input_overlaps_case" },
  { kind: "dir", group: "Evidence folders", path: `${EV}/denied`, note: "→ permission_denied" },
  { kind: "dir", group: "Run outcomes", path: `${EV}/errors`, note: "→ completed_with_errors" },
  { kind: "dir", group: "Run outcomes", path: `${EV}/fail-invalid`, note: "→ failed (no modules ran)" },
  { kind: "dir", group: "Run outcomes", path: `${EV}/fail-early`, note: "→ failed (no output dir)" },
  { kind: "dir", group: "Run outcomes", path: `${EV}/fail-argparse`, note: "→ failed (exit 2)" },
  { kind: "dir", group: "Run outcomes", path: `${EV}/fail-crash`, note: "→ failed (crash, traceback)" },
  { kind: "dir", group: "Run outcomes", path: `${EV}/slow`, note: "→ runs until cancelled" },
  { kind: "dir", group: "Run outcomes", path: `${EV}/interrupt`, note: "→ interrupted (app 'crashes')" },
  { kind: "dir", group: "Run outcomes", path: `${EV}/flood`, note: "→ 100,000 log lines, then succeeded" },
  { kind: "file", group: "Evidence files", path: `${EV}/iPhone-12-FFS.zip`, note: "zip, 12.4 GiB" },
  { kind: "file", group: "Evidence files", path: `${EV}/Pixel-7.tar`, note: "tar" },
  { kind: "file", group: "Evidence files", path: `${EV}/Galaxy-S21.E01`, note: "raw (E01)" },
  { kind: "file", group: "Evidence files", path: `${EV}/errors.zip`, note: "zip → completed_with_errors" },
  { kind: "file", group: "Evidence files", path: `${EV}/keychain-backup.plist`, note: "keychain file" },
  { kind: "file", group: "Profiles", path: `${DL}/Messaging.ilprofile`, note: "exists already → profile_exists" },
  { kind: "file", group: "Profiles", path: `${DL}/Triage.ilprofile`, note: "has 2 unknown modules" },
  { kind: "file", group: "Profiles", path: `${DL}/broken.ilprofile`, note: "→ profile_invalid" },
  { kind: "file", group: "Profiles", path: `${DL}/Android triage.alprofile`, note: "aLEAPP profile" },
  { kind: "file", group: "Tool archives", path: `${DL}/ileapp-v2026.4.2-macOS_Apple_Silicon.zip`, note: "valid asset" },
  { kind: "file", group: "Tool archives", path: `${DL}/corrupt.zip`, note: "→ hash_mismatch" },
];

/**
 * @param {string} path
 * @returns {string} the lowercase extension without the dot, or ""
 */
function extOf(path) {
  const base = path.split(/[\\/]/).pop() ?? "";
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(dot + 1).toLowerCase() : "";
}

/**
 * @param {OpenDialogOptions} options
 * @returns {Sample[]}
 */
function samplesFor(options) {
  const exts = options.filters?.flatMap((f) => f.extensions.map((e) => e.toLowerCase())) ?? [];
  return SAMPLES.filter((s) => {
    if (options.directory) return s.kind === "dir";
    if (s.kind !== "file") return false;
    return exts.length === 0 || exts.includes(extOf(s.path));
  });
}

/**
 * @param {string} tag
 * @param {string} [className]
 * @param {string} [text]
 * @returns {HTMLElement}
 */
function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

let pickerCount = 0;

/**
 * Shows the picker and resolves to the chosen path, or null if cancelled.
 * @param {{ title: string, samples: Sample[], defaultValue: string, action: string }} spec
 * @returns {Promise<string | null>}
 */
function showPicker(spec) {
  return new Promise((resolve) => {
    const id = `mock-picker-${++pickerCount}`;
    const dialog = /** @type {HTMLDialogElement} */ (el("dialog", "modal mock-picker"));
    dialog.setAttribute("aria-labelledby", `${id}-title`);
    const form = el("form", "modal-body");
    const title = el("h2", "modal-title", `${spec.title} (mock)`);
    title.id = `${id}-title`;
    form.append(title, el("p", "muted", "Mock mode has no native dialog. Pick a sample path or type one."));

    /** @param {string | null} value */
    const finish = (value) => {
      dialog.close();
      dialog.remove();
      resolve(value);
    };

    let lastGroup = "";
    const list = el("ul", "list-plain mock-picker-list");
    for (const sample of spec.samples) {
      if (sample.group !== lastGroup) {
        lastGroup = sample.group;
        list.append(el("li", "list-heading", sample.group));
      }
      const item = el("li");
      const button = el("button", "btn btn-block btn-plain");
      button.setAttribute("type", "button");
      button.append(el("span", "mono", sample.path), el("span", "muted small", ` ${sample.note}`));
      button.addEventListener("click", () => finish(sample.path));
      item.append(button);
      list.append(item);
    }
    if (spec.samples.length) form.append(list);

    const label = el("label", "field");
    label.append(el("span", "field-label", "Path"));
    const input = /** @type {HTMLInputElement} */ (el("input", "input mono"));
    input.type = "text";
    input.name = "path";
    input.value = spec.defaultValue;
    input.setAttribute("autocomplete", "off");
    label.append(input);
    form.append(label);

    const actions = el("div", "modal-actions");
    const cancel = el("button", "btn", "Cancel");
    cancel.setAttribute("type", "button");
    cancel.addEventListener("click", () => finish(null));
    const ok = el("button", "btn btn-primary", spec.action);
    ok.setAttribute("type", "submit");
    actions.append(cancel, ok);
    form.append(actions);

    form.addEventListener("submit", (event) => {
      event.preventDefault();
      const value = input.value.trim();
      finish(value === "" ? null : value);
    });
    dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      finish(null);
    });
    dialog.append(form);
    document.body.append(dialog);
    dialog.showModal();
  });
}

/**
 * @param {OpenDialogOptions} options
 * @returns {Promise<string | null>}
 */
export function pickOpen(options) {
  return showPicker({
    title: options.title,
    samples: samplesFor(options),
    defaultValue: options.defaultPath ?? "",
    action: "Select",
  });
}

/**
 * @param {SaveDialogOptions} options
 * @returns {Promise<string | null>}
 */
export function pickSave(options) {
  const name = options.defaultPath ?? "";
  const base = name.includes("/") ? name : `/Users/examiner/Desktop/${name}`;
  return showPicker({
    title: options.title,
    samples: [
      { kind: "file", group: "Destinations", path: base, note: "Desktop" },
      {
        kind: "file",
        group: "Destinations",
        path: `${CASES}/Operation Nightjar/runs/${name.split("/").pop() ?? name}`,
        note: "inside a case's runs/ → path_not_allowed",
      },
    ],
    defaultValue: base,
    action: "Save",
  });
}
