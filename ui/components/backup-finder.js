// @ts-check
/**
 * "Find iOS backups" (ROADMAP S1): a panel in the New run form's Input section that lists the
 * Finder/iTunes backups in the OS's default backup folders (`ios_backups_find`) as a table. "Use"
 * hands a backup to the form, which inspects it like any chosen folder; nothing starts.
 *
 * States: searching, found (the table), empty (with the folders searched), denied
 * (`permission_denied`: the AppError plus how to allow access, Full Disk Access on macOS) and any
 * other error (the AppError). The backups' names and versions come from their plists, so every value
 * is set as text.
 */
import { appError } from "./app-error.js";
import { accessGuidance, defaultBackupFolders, deviceName, encryptionLabel, failedResult, foundResult, iosVersionText, summaryText, useLabel } from "../lib/backups.js";
import { h } from "../lib/dom.js";
import { icon, pathText, sizeText, timeText, uid } from "../lib/view.js";

/** @typedef {import("../types").IosBackup} IosBackup */
/** @typedef {import("../lib/backups.js").FinderResult} FinderResult */
/** @typedef {import("../lib/context").Api} Api */

/** A backup path longer than this is shortened in the middle (the full path is on hover). */
const PATH_CHARS = 64;

/**
 * @typedef {object} BackupFinder
 * @property {HTMLElement} node
 * @property {() => Promise<void>} search Shows the panel and searches (again). Single-flight: a call
 *   while a search is still running does nothing (the core's search cannot be cancelled).
 * @property {() => void} close Hides the panel (without `onClose`, which is for the Close button);
 *   the result of a search still running is then dropped.
 * @property {() => void} focus Moves the focus to the panel's heading.
 * @property {() => boolean} isOpen
 * @property {() => void} dispose
 */

/**
 * @param {object} options
 * @param {Api} options.api
 * @param {() => string | null} options.os `app_info.os`, for the folders and the access guidance
 * @param {(backup: IosBackup) => void} options.onChoose a backup was chosen (the panel is hidden
 *   first)
 * @param {() => void} options.onClose the examiner closed the panel
 * @param {(busy: boolean) => void} options.onBusy a search started (true) or its call returned
 *   (false); the button that starts searches is disabled meanwhile
 * @returns {BackupFinder}
 */
export function backupFinder({ api, os, onChoose, onClose, onBusy }) {
  const headingId = uid("backup-finder");
  // Takes the focus when "Search again" or the disabled search button (see onBusy) had it.
  const heading = h("h3", { id: headingId, tabindex: "-1" }, "iOS backups on this computer");
  const status = h("p", { class: "muted small", role: "status" });
  const body = h("div", { class: "stack-sm" });
  const node = h(
    "section",
    { class: "subpanel stack-sm backup-finder", "aria-labelledby": headingId, hidden: true },
    h(
      "div",
      { class: "card-head" },
      heading,
      h(
        "button",
        {
          class: "btn btn-sm btn-plain",
          type: "button",
          onClick: () => {
            hide();
            onClose();
          },
        },
        "Close",
      ),
    ),
    status,
    body,
  );
  let seq = 0;
  let disposed = false;
  let searching = false;

  async function search() {
    if (searching) return;
    searching = true;
    const mine = ++seq;
    node.hidden = false;
    node.setAttribute("aria-busy", "true");
    status.textContent = "Searching the default backup folders…";
    if (body.contains(document.activeElement)) heading.focus();
    body.replaceChildren();
    onBusy(true);
    /** @type {FinderResult} */
    let result;
    try {
      result = foundResult(await api.ios_backups_find());
    } catch (err) {
      result = failedResult(err);
    }
    searching = false;
    if (!disposed) onBusy(false);
    // A stale result (the panel was closed meanwhile) is dropped.
    if (disposed || mine !== seq) return;
    node.removeAttribute("aria-busy");
    status.textContent = summaryText(result);
    body.replaceChildren(...view(result));
  }

  /**
   * @param {FinderResult} result
   * @returns {Node[]}
   */
  function view(result) {
    const again = h("div", null, h("button", { class: "btn btn-sm", type: "button", onClick: () => void search() }, "Search again"));
    switch (result.kind) {
      case "found":
        return [table(result.backups)];
      case "empty":
        return [emptyView(), again];
      case "denied":
        return [appError(result.error, { title: "The iOS backups could not be listed." }).node, guidance(), again];
      default:
        return [appError(result.error, { title: "The iOS backups could not be listed." }).node, again];
    }
  }

  /** @param {IosBackup[]} backups */
  function table(backups) {
    return h(
      "div",
      { class: "table-wrap" },
      h(
        "table",
        { class: "table backups-table" },
        h("caption", { class: "visually-hidden" }, "iOS backups in this computer's default backup folders, newest first"),
        h(
          "thead",
          null,
          h(
            "tr",
            null,
            h("th", { scope: "col" }, "Device"),
            h("th", { scope: "col" }, "iOS"),
            h("th", { scope: "col" }, "Last backup"),
            h("th", { scope: "col" }, "Encryption"),
            h("th", { scope: "col" }, "Size"),
            h("th", { scope: "col" }, h("span", { class: "visually-hidden" }, "Action")),
          ),
        ),
        h("tbody", null, backups.map(row)),
      ),
    );
  }

  /** @param {IosBackup} b */
  function row(b) {
    const known = b.device_name !== null;
    return h(
      "tr",
      null,
      h(
        "th",
        { scope: "row", class: "cell-device" },
        h("span", { class: known ? "choice-title" : "choice-title untitled" }, deviceName(b)),
        b.product_type && h("span", { class: "muted cell-sub" }, b.product_type),
        h("span", { class: "muted cell-sub" }, pathText(b.path, PATH_CHARS)),
      ),
      h("td", { class: "nowrap" }, iosVersionText(b)),
      h("td", { class: "nowrap" }, timeText(b.last_backup)),
      h("td", { class: "nowrap" }, encryptionCell(b)),
      h("td", { class: "nowrap" }, sizeText(b.size_bytes)),
      h(
        "td",
        { class: "cell-actions" },
        h("button", { class: "btn btn-sm", type: "button", "aria-label": useLabel(b), onClick: () => choose(b) }, "Use"),
      ),
    );
  }

  /** @param {IosBackup} b */
  function encryptionCell(b) {
    const label = encryptionLabel(b);
    if (b.encrypted === true) return h("span", { class: "encryption-on" }, icon("lock"), label);
    if (b.encrypted === null) return h("span", { class: "muted" }, label);
    return label;
  }

  function emptyView() {
    const folders = defaultBackupFolders(os());
    return h(
      "div",
      { class: "stack-sm" },
      folders.length > 0
        ? [
            h("p", null, "No Finder or iTunes backups were found in this computer's default backup folders:"),
            h("ul", { class: "list-compact mono" }, folders.map((f) => h("li", null, f))),
          ]
        : h("p", null, "This operating system has no default folder for iOS backups."),
      h("p", { class: "muted small" }, "For a backup stored elsewhere, use Choose folder… and pick the backup's own folder (named after the device's UDID)."),
    );
  }

  function guidance() {
    const g = accessGuidance(os());
    const titleId = uid("access-guidance");
    return h(
      "div",
      { class: "banner banner-info", role: "group", "aria-labelledby": titleId },
      icon("lock"),
      h("div", { class: "stack-sm" }, h("p", null, h("strong", { id: titleId }, g.title)), h("p", null, g.intro), h("ol", { class: "list-compact" }, g.steps.map((s) => h("li", null, s)))),
    );
  }

  /** @param {IosBackup} b */
  function choose(b) {
    hide();
    onChoose(b);
  }

  /** Hides the panel and drops a search still running. */
  function hide() {
    seq += 1;
    node.hidden = true;
    node.removeAttribute("aria-busy");
    status.textContent = "";
    body.replaceChildren();
  }

  return {
    node,
    search,
    close: hide,
    focus: () => heading.focus(),
    isOpen: () => !node.hidden,
    dispose() {
      disposed = true;
      seq += 1;
    },
  };
}
