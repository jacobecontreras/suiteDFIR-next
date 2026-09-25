// @ts-check
/**
 * Case screen (ROADMAP D2): metadata with edit (editable fields only), and the runs table with
 * local times (UTC on hover), status badges and row actions (open report, reveal folder, details).
 */
import { appError, errorSlot } from "../components/app-error.js";
import { caseForm } from "../components/case-form.js";
import { modal } from "../components/dialog.js";
import { editableFields, folderLabel } from "../lib/cases.js";
import { h } from "../lib/dom.js";
import { formatBytes, formatCount, formatDuration } from "../lib/format.js";
import { routeHref } from "../lib/router.js";
import { DEFAULT_RUN_SORT, nextSort, sortRuns } from "../lib/sort.js";
import { watch } from "../lib/store.js";
import { loadTimezones } from "../lib/timezones.js";
import { icon, inputTypeLabel, sizeText, statusBadge, timeText, toolName } from "../lib/view.js";

/** @typedef {import("../types").CaseDetail} CaseDetail */
/** @typedef {import("../types").RunRecord} RunRecord */
/** @typedef {import("../types").RunSummary} RunSummary */
/** @typedef {import("../lib/sort.js").RunSort} RunSort */
/** @typedef {import("../lib/sort.js").RunSortKey} RunSortKey */
/** @typedef {import("../lib/context").ScreenContext} ScreenContext */
/** @typedef {import("../lib/context").View} View */

/** @type {{ key: RunSortKey | null, label: string }[]} */
const COLUMNS = [
  { key: "status", label: "Status" },
  { key: "label", label: "Label" },
  { key: "tool", label: "Tool" },
  { key: "input", label: "Input" },
  { key: "created_at", label: "Created" },
  { key: "duration", label: "Duration" },
  { key: null, label: "Actions" },
];

/**
 * @param {ScreenContext} ctx
 * @returns {View}
 */
export function caseScreen(ctx) {
  const { api, store } = ctx;
  const path = ctx.params.path ?? "";
  let disposed = false;
  /** @type {CaseDetail | null} */
  let detail = null;
  /** @type {RunSort} */
  let sort = DEFAULT_RUN_SORT;
  /** @type {(() => void)[]} */
  const cleanups = [];

  const title = h("h1", { tabindex: "-1" }, folderLabel(path) || "Case");
  const headActions = h("div", { class: "actions" });
  const body = h("div", { class: "stack", "aria-busy": "true" }, h("p", { class: "muted" }, "Opening the case…"));
  const node = h(
    "section",
    { class: "screen" },
    h("nav", { class: "breadcrumb", "aria-label": "Breadcrumb" }, h("a", { href: routeHref("cases") }, "Cases"), h("span", { "aria-hidden": "true" }, " / ")),
    h("div", { class: "screen-head" }, title, headActions),
    body,
  );

  const metaCard = h("section", { class: "card", "aria-labelledby": "case-details-heading" });
  const runsErrors = errorSlot();
  const runsCard = h("section", { class: "card", "aria-labelledby": "runs-heading" });

  async function open() {
    try {
      const opened = await api.case_open({ path });
      if (disposed) return;
      detail = opened;
      render(opened.recovered);
    } catch (err) {
      if (disposed) return;
      body.replaceChildren(
        appError(err, { title: "The case could not be opened." }).node,
        h("p", null, h("a", { href: routeHref("cases") }, "Back to Cases")),
      );
    } finally {
      body.removeAttribute("aria-busy");
    }
  }

  /** @param {readonly string[]} recovered */
  function render(recovered) {
    if (!detail) return;
    title.textContent = detail.case.name;
    headActions.replaceChildren(
      h("button", { class: "btn", type: "button", onClick: () => reveal(detail?.path ?? path) }, icon("folder"), "Reveal folder"),
      h("a", { class: "btn btn-primary", href: routeHref("new-run", { case: detail.path }) }, "New run"),
    );
    renderMeta();
    renderRuns();
    body.replaceChildren(
      ...(recovered.length ? [recoveredNotice(recovered)] : []),
      metaCard,
      runsCard,
    );
  }

  /** @param {readonly string[]} ids */
  function recoveredNotice(ids) {
    return h(
      "div",
      { class: "banner banner-info", role: "status" },
      icon("info"),
      h(
        "div",
        null,
        h("strong", null, ids.length === 1 ? "1 job was marked interrupted " : `${ids.length} jobs were marked interrupted `),
        "because the app closed while they were running: ",
        h("span", { class: "mono" }, ids.join(", ")),
      ),
    );
  }

  function renderMeta() {
    if (!detail) return;
    const c = detail.case;
    const settings = store.get().settings;
    const tzText = c.default_timezone ?? `App default (${settings?.defaults.timezone || "UTC"})`;
    /** @type {[string, Node | string][]} */
    const facts = [
      ["Case number", c.case_number || dash()],
      ["Examiner", c.examiner || dash()],
      ["Agency", c.agency || dash()],
      ["Description", c.description ? h("span", { class: "prewrap" }, c.description) : dash()],
      ["Default timezone", tzText],
      ["Created", timeText(c.created_at)],
      ["Updated", timeText(c.updated_at)],
      ["Folder", h("span", { class: "mono break" }, detail.path)],
    ];
    metaCard.replaceChildren(
      h(
        "div",
        { class: "card-head" },
        h("h2", { id: "case-details-heading" }, "Case details"),
        h("button", { class: "btn btn-sm", type: "button", onClick: startEdit }, "Edit"),
      ),
      h("dl", { class: "facts" }, facts.map(([k, v]) => [h("dt", null, k), h("dd", null, v)])),
    );
  }

  async function startEdit() {
    if (!detail) return;
    const zones = await loadTimezones(api, store);
    if (disposed || !detail) return;
    const settings = store.get().settings;
    const form = caseForm({
      initial: editableFields(detail.case),
      timezones: zones,
      appDefaultTimezone: settings?.defaults.timezone ?? "UTC",
      submitLabel: "Save changes",
      onCancel: () => {
        form.dispose();
        renderMeta();
      },
      onSubmit: async (fields) => {
        const updated = await api.case_update({ path: detail?.path ?? path, fields });
        if (disposed) return;
        form.dispose();
        detail = updated;
        title.textContent = updated.case.name;
        renderMeta();
      },
    });
    metaCard.replaceChildren(
      h("div", { class: "card-head" }, h("h2", { id: "case-details-heading" }, "Edit case details")),
      form.node,
    );
    form.focus();
  }

  function renderRuns() {
    if (!detail) return;
    const runs = sortRuns(detail.runs, sort);
    const count = detail.runs.length;
    runsCard.replaceChildren(
      h(
        "div",
        { class: "card-head" },
        h("h2", { id: "runs-heading" }, "Runs"),
        h("span", { class: "muted" }, count === 1 ? "1 run" : `${formatCount(count)} runs`),
      ),
      runsErrors.node,
      count === 0
        ? h(
            "p",
            { class: "muted" },
            "No runs yet. ",
            h("a", { href: routeHref("new-run", { case: detail.path }) }, "Start a new run"),
            ".",
          )
        : h(
            "div",
            { class: "table-wrap" },
            h(
              "table",
              { class: "table runs-table" },
              h("caption", { class: "visually-hidden" }, "Runs in this case. Column headers sort the table."),
              h("thead", null, h("tr", null, COLUMNS.map(headerCell))),
              h("tbody", null, runs.map(runRow)),
            ),
          ),
    );
  }

  /** @param {{ key: RunSortKey | null, label: string }} col */
  function headerCell(col) {
    if (!col.key) return h("th", { scope: "col" }, h("span", { class: "visually-hidden" }, col.label));
    const key = col.key;
    const active = sort.key === key;
    return h(
      "th",
      { scope: "col", "aria-sort": active ? (sort.dir === "asc" ? "ascending" : "descending") : null },
      h(
        "button",
        {
          class: "th-sort",
          type: "button",
          onClick: () => {
            sort = nextSort(sort, key);
            renderRuns();
            runsCard.querySelector(`[data-sort-key="${key}"]`)?.closest("button")?.focus();
          },
        },
        col.label,
        h("span", { class: active ? "sort-mark" : "sort-mark sort-mark-idle", "data-sort-key": key, "aria-hidden": "true" }, active ? (sort.dir === "asc" ? "▲" : "▼") : "↕"),
      ),
    );
  }

  /** @param {RunSummary} run */
  function runRow(run) {
    const reportButton = h(
      "button",
      {
        class: "btn btn-sm",
        type: "button",
        disabled: !run.report_available,
        title: run.report_available ? "Open report/index.html in the default browser" : "This run has no report",
        onClick: () => openReport(run),
      },
      "Report",
    );
    return h(
      "tr",
      null,
      h("td", null, statusBadge(run.status)),
      h("td", { class: "cell-label" }, run.label ? run.label : dash()),
      h("td", { class: "nowrap" }, toolName(run.tool), h("span", { class: "muted cell-sub" }, run.tool_version)),
      h(
        "td",
        { class: "cell-input" },
        h("span", { class: "tag", title: inputTypeLabel(run.input_type) }, run.input_type),
        h("span", { class: "mono truncate", title: run.input_path }, run.input_path),
      ),
      h("td", { class: "nowrap" }, timeText(run.created_at)),
      h("td", { class: "nowrap" }, run.duration_ms === null ? dash() : formatDuration(run.duration_ms)),
      h(
        "td",
        { class: "cell-actions" },
        reportButton,
        h("button", { class: "btn btn-sm", type: "button", title: "Reveal the run folder", onClick: () => reveal(run.run_dir) }, "Folder"),
        h("button", { class: "btn btn-sm", type: "button", onClick: () => showDetails(run) }, "Details"),
      ),
    );
  }

  /** @param {RunSummary} run */
  async function openReport(run) {
    runsErrors.clear();
    try {
      await api.open_report({ case_path: detail?.path ?? path, run_id: run.run_id });
    } catch (err) {
      runsErrors.show(err, "The report could not be opened.");
    }
  }

  /** @param {string} target */
  async function reveal(target) {
    runsErrors.clear();
    try {
      await api.reveal_path({ path: target });
    } catch (err) {
      runsErrors.show(err, "The folder could not be revealed.");
    }
  }

  /** @param {RunSummary} run */
  function showDetails(run) {
    const content = h("div", { class: "stack", "aria-busy": "true" }, h("p", { class: "muted" }, "Loading run.json…"));
    const dialog = modal({
      title: `Run ${run.label ?? run.run_id}`,
      wide: true,
      content: () =>
        h(
          "div",
          { class: "modal-main" },
          h("div", { class: "modal-body modal-scroll" }, content),
          h("div", { class: "modal-footer" }, h("button", { class: "btn", type: "button", onClick: () => dialog.close() }, "Close")),
        ),
    });
    cleanups.push(() => dialog.close());
    dialog.open();
    api
      .run_get({ case_path: detail?.path ?? path, run_id: run.run_id })
      .then((record) => content.replaceChildren(...runDetails(record)))
      .catch((err) => content.replaceChildren(appError(err, { title: "run.json could not be read." }).node))
      .finally(() => content.removeAttribute("aria-busy"));
  }

  // Refresh the runs when the active job ends (e.g. a run started from New run finished).
  cleanups.push(
    watch(
      store,
      (s) => s.activeJob,
      (job) => {
        if (!job && detail) void refreshRuns();
      },
    ),
  );

  async function refreshRuns() {
    try {
      const fresh = await api.case_open({ path });
      if (disposed) return;
      detail = fresh;
      renderRuns();
    } catch {
      // Keep the table as it is; opening again shows the error.
    }
  }

  open();
  return {
    node,
    dispose() {
      disposed = true;
      runsErrors.dispose();
      for (const fn of cleanups) fn();
    },
  };
}

function dash() {
  return h("span", { class: "muted" }, "—");
}

/**
 * The key facts of a run record, then the raw JSON as text (D2).
 * @param {RunRecord} r
 * @returns {Node[]}
 */
function runDetails(r) {
  const yesNo = (/** @type {boolean | null} */ v) => (v === null ? dash() : v ? "Yes" : "No");
  /** @param {import("../types").Reason[]} list */
  const reasons = (list) =>
    list.length
      ? h("ul", { class: "list-compact" }, list.map((x) => h("li", null, h("code", null, x.code), " ", x.message)))
      : dash();
  const counts = r.leapp_result?.module_counts;
  const proc = r.process;
  const seal = r.output.seal;
  const modules = r.modules;
  /** @type {[string, [string, Node | string][]][]} */
  const groups = [
    [
      "Outcome",
      [
        ["Status", statusBadge(r.status)],
        ["Reasons", reasons(r.status_reasons)],
        ["Warnings", reasons(r.warnings)],
        ["Run ID", h("span", { class: "mono" }, r.run_id)],
        ["Label", r.label ?? dash()],
      ],
    ],
    [
      "Times",
      [
        ["Created", timeText(r.created_at)],
        ["Started", timeText(r.started_at)],
        ["Ended", timeText(r.ended_at)],
        ...(r.recovered_at ? /** @type {[string, Node][]} */ ([["Recovered", timeText(r.recovered_at)]]) : []),
        ["Duration", r.duration_ms === null ? dash() : formatDuration(r.duration_ms)],
      ],
    ],
    [
      "Tool",
      [
        ["Tool", `${toolName(r.tool.id)} ${r.tool.version} (${r.tool.platform})`],
        ["Entry SHA-256", h("span", { class: "mono break" }, r.tool.entry_sha256)],
        ["Verified against", r.tool.entry_verified_against],
        ["Installed from", r.tool.install_source],
      ],
    ],
    [
      "Input",
      [
        ["Path", h("span", { class: "mono break" }, r.input.path)],
        ["Type", `${inputTypeLabel(r.input.type)}${r.input.type_detected && r.input.type_detected !== r.input.type ? `, detected ${r.input.type_detected}` : ""}`],
        ["Kind", r.input.kind === "file" ? "File" : "Folder"],
        ["Size", sizeText(r.input.size_bytes)],
        ["Encrypted backup", yesNo(r.input.itunes_encrypted)],
        ["Hash (SHA-256)", r.input.hash.value ? h("span", { class: "mono break" }, r.input.hash.value) : r.input.hash.status.replaceAll("_", " ")],
        ...(r.input.acquisition_id ? /** @type {[string, Node][]} */ ([["Acquisition", h("span", { class: "mono" }, r.input.acquisition_id)]]) : []),
      ],
    ],
    [
      "Options",
      [
        ["Timezone", r.options.timezone_supported ? (r.options.timezone ?? dash()) : "Not supported by this tool"],
        ["Backup password", r.options.password_supplied ? "Supplied (not recorded)" : "Not supplied"],
        ["Keychain", r.options.keychain_path ? h("span", { class: "mono break" }, r.options.keychain_path) : dash()],
      ],
    ],
    [
      "Modules",
      [
        ["Selection", modules.mode === "profile" ? `Profile “${modules.profile_name ?? ""}”` : modules.mode === "all" ? "All modules" : "Custom"],
        ["Resolved", `${formatCount(modules.resolved.length)} of ${formatCount(modules.available_count)}`],
        ["Always run", modules.always_run.length ? h("span", { class: "mono" }, modules.always_run.join(", ")) : dash()],
      ],
    ],
    [
      "Result",
      [
        ["Exit", proc ? (proc.exit_code !== null ? `Code ${proc.exit_code}` : `Signal ${proc.signal ?? "?"}`) : dash()],
        ["Cancel requested", proc ? yesNo(proc.cancel_requested) : dash()],
        ["LEAPP status", r.leapp_result?.processing_status ?? dash()],
        [
          "Modules run",
          counts
            ? `${formatCount(counts.complete)} complete, ${formatCount(counts.error)} error, ${formatCount(counts.no_files_found)} no files found, ${formatCount(counts.other)} other`
            : dash(),
        ],
        ["Report", r.leapp_result ? (r.leapp_result.index_html_found ? "index.html present" : "index.html missing") : dash()],
        ["Report seal", seal.status === "sealed" ? `${statusWord(seal.status)}: ${formatCount(seal.file_count)} files, ${formatBytes(seal.total_bytes)}` : statusWord(seal.status)],
      ],
    ],
  ];
  return [
    h(
      "div",
      { class: "detail-groups" },
      groups.map(([heading, rows]) =>
        h(
          "section",
          { class: "detail-group" },
          h("h3", null, heading),
          h("dl", { class: "facts facts-compact" }, rows.map(([k, v]) => [h("dt", null, k), h("dd", null, v)])),
        ),
      ),
    ),
    h(
      "details",
      { class: "raw-json" },
      h("summary", null, "Raw run.json"),
      h("pre", { class: "pre" }, JSON.stringify(r, null, 2)),
    ),
  ];
}

/** @param {string} status */
function statusWord(status) {
  const text = status.replaceAll("_", " ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}
