// @ts-check
/**
 * Cases screen (ROADMAP D2): the recent cases (with the missing-folder state and Forget), New case,
 * Open case folder, the empty state and the tools-not-installed banner.
 */
import { appError, errorSlot } from "../components/app-error.js";
import { caseForm } from "../components/case-form.js";
import { modal } from "../components/dialog.js";
import { folderLabel } from "../lib/cases.js";
import { h } from "../lib/dom.js";
import { formatCount } from "../lib/format.js";
import { routeHref } from "../lib/router.js";
import { loadTimezones } from "../lib/timezones.js";
import { installedTools } from "../lib/tools.js";
import { icon, timeText } from "../lib/view.js";

/** @typedef {import("../types").CaseSummary} CaseSummary */
/** @typedef {import("../types").ToolStatus} ToolStatus */
/** @typedef {import("../lib/context").ScreenContext} ScreenContext */
/** @typedef {import("../lib/context").View} View */

/**
 * @param {ScreenContext} ctx
 * @returns {View}
 */
export function casesScreen(ctx) {
  const { api, store, navigate } = ctx;
  let disposed = false;
  /** @type {(() => void)[]} */
  const cleanups = [];

  const banner = h("div");
  const errors = errorSlot();
  const list = h("div", { class: "stack", "aria-busy": "true" }, h("p", { class: "muted" }, "Loading recent cases…"));

  const node = h(
    "section",
    { class: "screen" },
    h(
      "div",
      { class: "screen-head" },
      h("h1", { tabindex: "-1" }, "Cases"),
      h(
        "div",
        { class: "actions" },
        h("button", { class: "btn", type: "button", onClick: openCaseFolder }, "Open case folder…"),
        h("button", { class: "btn btn-primary", type: "button", onClick: openNewCase }, "New case"),
      ),
    ),
    banner,
    errors.node,
    list,
  );

  async function load() {
    try {
      const [cases, tools] = await Promise.all([api.cases_list(), api.tools_status()]);
      if (disposed) return;
      store.set({ tools });
      banner.replaceChildren(toolsBanner(tools) ?? "");
      list.replaceChildren(cases.length ? recentList(cases) : emptyState());
    } catch (err) {
      if (disposed) return;
      list.replaceChildren(appError(err, { title: "The recent cases could not be loaded." }).node);
    } finally {
      list.removeAttribute("aria-busy");
    }
  }

  /**
   * @param {readonly ToolStatus[]} tools
   * @returns {HTMLElement | null}
   */
  function toolsBanner(tools) {
    if (installedTools(tools).length > 0) return null;
    return h(
      "div",
      { class: "banner banner-warn", role: "status" },
      icon("alert-triangle"),
      h(
        "div",
        null,
        h("strong", null, "No parser is installed. "),
        "Install iLEAPP or aLEAPP in ",
        h("a", { href: routeHref("settings") }, "Settings"),
        " before starting a run.",
      ),
    );
  }

  /** @param {readonly CaseSummary[]} cases */
  function recentList(cases) {
    return h(
      "section",
      { class: "stack", "aria-labelledby": "recent-heading" },
      h("h2", { class: "section-title", id: "recent-heading" }, "Recent cases"),
      h("ul", { class: "case-list" }, cases.map(caseItem)),
    );
  }

  /** @param {CaseSummary} summary */
  function caseItem(summary) {
    const forget = h(
      "button",
      {
        class: "btn btn-sm btn-plain",
        type: "button",
        title: "Remove from this list. The folder is not changed.",
        onClick: () => forgetCase(summary.path),
      },
      "Forget",
    );
    const file = summary.case;
    if (!summary.exists || !file) {
      return h(
        "li",
        { class: "case-item case-item-missing" },
        h(
          "div",
          { class: "case-main" },
          h(
            "h3",
            { class: "case-name" },
            file?.name ?? folderLabel(summary.path),
            h("span", { class: "badge badge-missing" }, icon("alert-triangle"), summary.exists ? "case.json unreadable" : "Folder not found"),
          ),
          h("p", { class: "case-path mono" }, summary.path),
          h(
            "p",
            { class: "muted small" },
            summary.exists
              ? "The folder exists but its case.json could not be read."
              : "The folder was moved or renamed, or its drive is not connected. Reconnect it, or forget this entry.",
          ),
        ),
        h("div", { class: "case-actions" }, forget),
      );
    }
    const href = routeHref("case", { path: summary.path });
    const meta = [file.case_number && `Case ${file.case_number}`, file.examiner, file.agency].filter(Boolean).join(" · ");
    return h(
      "li",
      { class: "case-item" },
      h(
        "div",
        { class: "case-main" },
        h("h3", { class: "case-name" }, h("a", { href }, file.name)),
        meta && h("p", { class: "case-meta" }, meta),
        h("p", { class: "case-path mono" }, summary.path),
      ),
      h(
        "div",
        { class: "case-stats" },
        h("span", null, summary.run_count === 1 ? "1 run" : `${formatCount(summary.run_count)} runs`),
        summary.last_run_at && h("span", { class: "muted" }, "Last run ", timeText(summary.last_run_at)),
      ),
      h("div", { class: "case-actions" }, h("a", { class: "btn btn-sm", href }, "Open"), forget),
    );
  }

  function emptyState() {
    return h(
      "div",
      { class: "empty-state" },
      icon("folder", "icon icon-xl"),
      h("h2", null, "No cases yet"),
      h("p", null, "A case is a folder that holds your runs and acquisitions. Create one, or open an existing case folder."),
      h(
        "div",
        { class: "actions" },
        h("button", { class: "btn", type: "button", onClick: openCaseFolder }, "Open case folder…"),
        h("button", { class: "btn btn-primary", type: "button", onClick: openNewCase }, "New case"),
      ),
    );
  }

  /** @param {string} path */
  async function forgetCase(path) {
    errors.clear();
    try {
      await api.case_forget({ path });
      await load();
    } catch (err) {
      errors.show(err, "The case could not be removed from the list.");
    }
  }

  async function openCaseFolder() {
    errors.clear();
    try {
      const path = await api.dialog_open({ title: "Open case folder", directory: true });
      if (path) navigate(routeHref("case", { path }));
    } catch (err) {
      errors.show(err);
    }
  }

  async function openNewCase() {
    errors.clear();
    const settings = store.get().settings;
    if (!settings) return;
    const zones = await loadTimezones(api, store);
    if (disposed) return;
    /** @type {string | null} */
    let parentDir = null;
    const location = h("span", { class: "mono" }, settings.cases_root);
    const resetLocation = /** @type {HTMLButtonElement} */ (
      h(
        "button",
        {
          class: "btn btn-sm btn-plain",
          type: "button",
          hidden: true,
          onClick: () => {
            parentDir = null;
            location.textContent = settings.cases_root;
            resetLocation.hidden = true;
          },
        },
        "Use default",
      )
    );
    const locationBlock = h(
      "div",
      { class: "field" },
      h("span", { class: "field-label", id: "new-case-location" }, "Location"),
      h(
        "div",
        { class: "inline-row", role: "group", "aria-labelledby": "new-case-location" },
        location,
        h(
          "button",
          {
            class: "btn btn-sm",
            type: "button",
            onClick: async () => {
              try {
                const picked = await api.dialog_open({
                  title: "Choose where to create the case",
                  directory: true,
                  defaultPath: parentDir ?? settings.cases_root,
                });
                if (picked) {
                  parentDir = picked;
                  location.textContent = picked;
                  resetLocation.hidden = false;
                }
              } catch (err) {
                form.showError(err);
              }
            },
          },
          "Change…",
        ),
        resetLocation,
      ),
      h("p", { class: "field-hint" }, "The case folder is created here."),
    );
    const form = caseForm({
      initial: {
        name: "",
        case_number: "",
        examiner: settings.defaults.examiner,
        agency: settings.defaults.agency,
        description: "",
        default_timezone: null,
      },
      timezones: zones,
      appDefaultTimezone: settings.defaults.timezone,
      submitLabel: "Create case",
      extra: locationBlock,
      onCancel: () => dialog.close(),
      onSubmit: async (fields) => {
        const created = await api.case_create({ ...fields, parent_dir: parentDir });
        dialog.close();
        navigate(routeHref("case", { path: created.path }));
      },
    });
    const dialog = modal({ title: "New case", wide: true, content: () => h("div", { class: "modal-body" }, form.node), onClose: () => form.dispose() });
    cleanups.push(() => dialog.close());
    dialog.open();
    form.focus();
  }

  load();
  return {
    node,
    dispose() {
      disposed = true;
      errors.dispose();
      for (const fn of cleanups) fn();
    },
  };
}
