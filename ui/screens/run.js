// @ts-check
/**
 * Run screen (ROADMAP D4a): the header (tool, input, label), the phase and elapsed time, the
 * virtualized log (auto-scroll, copy all), hash and seal progress, Cancel with an in-DOM confirm,
 * and the result panel (status, reasons, warnings, module counts, the parser's stdout/stderr tails
 * as "Parser output", and open report / reveal folder / open stdout, stderr, run.json).
 *
 * The events come from the job stream hub (lib/jobstream.js): the run started in this window, or
 * `job_attach` after a reload. A run that is not the active job is shown from its `run.json`.
 */
import { appError, errorSlot } from "../components/app-error.js";
import { confirmDialog } from "../components/dialog.js";
import { logView } from "../components/log-view.js";
import { percentOf, progressBar, stepList } from "../components/progress.js";
import { folderLabel } from "../lib/cases.js";
import { fill, h } from "../lib/dom.js";
import { elapsedSince, formatBytes, formatCount, formatElapsed, plural } from "../lib/format.js";
import { RUN_PHASES, stepStates } from "../lib/jobstream.js";
import { jobKey } from "../lib/jobs.js";
import { routeHref } from "../lib/router.js";
import { watch } from "../lib/store.js";
import { icon, inputTypeLabel, pathText, statusBadge, timeText, toolName } from "../lib/view.js";

/** @typedef {import("../types").Reason} Reason */
/** @typedef {import("../types").RunFile} RunFile */
/** @typedef {import("../types").RunRecord} RunRecord */
/** @typedef {import("../types").RunStatus} RunStatus */
/** @typedef {import("../lib/jobstream.js").JobStream} JobStream */
/** @typedef {import("../lib/jobstream.js").RunFinished} RunFinished */
/** @typedef {import("../lib/context").ScreenContext} ScreenContext */
/** @typedef {import("../lib/context").View} View */

/** Phases shown only when they happen (input hashing continues after LEAPP exits only if needed). */
const OPTIONAL_PHASES = new Set(["hashing_input"]);

/** @type {Record<Exclude<RunStatus, "running">, string>} */
const OUTCOME = {
  succeeded: "LEAPP completed and no module reported an error.",
  completed_with_errors: "The report was created, but some modules reported errors.",
  failed: "The run failed. The reasons come from LEAPP's output files, not only its exit code.",
  cancelled: "The run was cancelled. Its partial output is kept and marked cancelled.",
  interrupted: "The app closed while this run was running. Its partial output is kept.",
};

/**
 * @param {ScreenContext} ctx
 * @returns {View}
 */
export function runScreen(ctx) {
  const { api, store, jobs } = ctx;
  const casePath = ctx.params.case ?? "";
  const runId = ctx.params.id ?? "";
  const caseHref = routeHref("case", { path: casePath });
  let disposed = false;
  /** @type {(() => void)[]} */
  const cleanups = [];
  /** @type {RunRecord | null} */
  let record = null;
  /** @type {JobStream | null} */
  let stream = null;
  let frame = 0;
  let cancelling = false;
  /** The stream version the result panel was last built for (-1 = not built). */
  let resultBuiltFor = -1;
  /** A notice when the run stopped reporting without finishing (the app "crashed" in the mock). */
  let stalled = false;

  const caseLink = h("a", { href: caseHref }, folderLabel(casePath));
  const title = h("h1", { tabindex: "-1" }, "Run");
  const statusSlot = h("div", { class: "run-status" });
  const cancelButton = /** @type {HTMLButtonElement} */ (
    h("button", { class: "btn btn-danger", type: "button", hidden: true, onClick: askCancel }, "Cancel run")
  );
  const facts = h("dl", { class: "facts run-facts" });
  const elapsed = h("span", { class: "elapsed" });
  const elapsedLabel = h("span", null, "Elapsed");
  const phaseSlot = h("div", { class: "phase-slot" });
  const progressSlot = h("div", { class: "stack-sm progress-slot" });
  const actionErrors = errorSlot();
  const log = logView({ label: "Run log", emptyText: "No log lines yet." });
  const logBody = h("div", { class: "stack-sm" });
  const logCard = h("section", { class: "card", "aria-labelledby": "run-log-heading" }, h("div", { class: "card-head" }, h("h2", { id: "run-log-heading" }, "Log")), logBody);
  const resultCard = h("section", { class: "card result-card", "aria-labelledby": "run-result-heading", hidden: true });
  const progressCard = h(
    "section",
    { class: "card", "aria-labelledby": "run-progress-heading" },
    h("div", { class: "card-head" }, h("h2", { id: "run-progress-heading" }, "Progress"), h("span", { class: "muted" }, elapsedLabel, " ", elapsed)),
    phaseSlot,
    progressSlot,
    actionErrors.node,
  );
  const body = h("div", { class: "stack", "aria-busy": "true" }, h("p", { class: "muted" }, "Loading the run…"));
  const node = h(
    "section",
    { class: "screen run-screen" },
    h(
      "nav",
      { class: "breadcrumb", "aria-label": "Breadcrumb" },
      h("a", { href: routeHref("cases") }, "Cases"),
      h("span", { "aria-hidden": "true" }, " / "),
      caseLink,
      h("span", { "aria-hidden": "true" }, " / "),
    ),
    h("div", { class: "screen-head" }, h("div", { class: "title-row" }, title, statusSlot), h("div", { class: "actions" }, cancelButton)),
    body,
  );

  // ---- Loading ----

  async function init() {
    try {
      record = await api.run_get({ case_path: casePath, run_id: runId });
      if (disposed) return;
      stream = jobs.find("run", runId);
      const job = store.get().activeJob;
      const isActive = job?.kind === "run" && job.run_id === runId;
      // A stream that never finished, of a run that is no longer active and whose record is final
      // (recovered as interrupted), says less than the record.
      if (stream && !stream.finished && !isActive && record.status !== "running") stream = null;
      if (!stream && isActive) {
        try {
          stream = await jobs.attach(api, "run", runId, casePath);
        } catch {
          // It ended between job_active and job_attach: show its record.
          stream = null;
          record = await api.run_get({ case_path: casePath, run_id: runId });
        }
        if (disposed) return;
      }
      caseLink.textContent = record.case_snapshot.name;
      title.textContent = record.label ?? "Run";
      renderFacts();
      body.replaceChildren(facts, ...(stream ? [progressCard] : []), resultCard, logCard);
      if (stream) {
        const own = stream;
        cleanups.push(jobs.subscribe((s) => s === own && schedule()));
        logBody.replaceChildren(log.node);
      } else {
        logBody.replaceChildren(
          h(
            "p",
            { class: "muted" },
            "The live log is shown while a run is in progress. The complete log is ",
            h("span", { class: "mono" }, "report/_HTML/_Script_Logs/Screen_Output.html"),
            " in the run folder.",
          ),
        );
      }
      render();
    } catch (err) {
      if (disposed) return;
      body.replaceChildren(appError(err, { title: "The run could not be loaded." }).node, h("p", null, h("a", { href: caseHref }, "Back to the case")));
    } finally {
      body.removeAttribute("aria-busy");
    }
  }

  function schedule() {
    if (frame === 0 && !disposed) frame = requestAnimationFrame(render);
  }

  // ---- Rendering ----

  function render() {
    frame = 0;
    if (!record || disposed) return;
    const s = stream;
    const finished = /** @type {RunFinished | null} */ (s?.finished ?? null);
    /** @type {RunStatus} */
    const status = finished ? finished.status : s ? "running" : record.status;
    statusSlot.replaceChildren(statusBadge(status));
    cancelButton.hidden = !s || !s.live || stalled;
    cancelButton.disabled = cancelling;
    cancelButton.textContent = cancelling ? "Cancelling…" : "Cancel run";
    renderElapsed();
    if (s) {
      phaseSlot.replaceChildren(
        stepList("Run phases", stepStates(RUN_PHASES, { phases: s.phases, phase: s.phase, finished: finished !== null, partial: s.partial }, OPTIONAL_PHASES)),
      );
      progressSlot.replaceChildren(...progressBars(s));
      log.update(s.log);
    }
    if (status !== "running" && resultBuiltFor !== (s?.version ?? 0)) {
      resultBuiltFor = s?.version ?? 0;
      renderResult(status);
      // The record now holds the analysis (module counts, seal); re-read it once.
      if (finished && record.status === "running") void refreshRecord();
    } else if (status === "running" && stalled) {
      resultCard.hidden = false;
      resultCard.replaceChildren(
        h(
          "div",
          { class: "banner banner-warn", role: "status" },
          icon("alert-triangle"),
          h("p", null, "This run stopped reporting progress and is no longer the active job. Its record is marked interrupted when the case is opened again."),
        ),
      );
    }
  }

  function renderElapsed() {
    if (!record) return;
    const finished = /** @type {RunFinished | null} */ (stream?.finished ?? null);
    const ms = finished ? finished.summary.duration_ms : stream?.live ? elapsedSince(record.created_at, Date.now()) : record.duration_ms;
    elapsedLabel.textContent = finished ? "Duration" : "Elapsed";
    elapsed.textContent = formatElapsed(ms);
  }

  function renderFacts() {
    if (!record) return;
    const r = record;
    /** @type {[string, Node | string][]} */
    const rows = [
      ["Tool", `${toolName(r.tool.id)} ${r.tool.version}`],
      ["Input", h("span", { class: "cell-input" }, h("span", { class: "tag", title: inputTypeLabel(r.input.type) }, r.input.type), pathText(r.input.path, 90))],
      ["Label", r.label ?? h("span", { class: "muted" }, "—")],
      ["Created", timeText(r.created_at)],
      ["Run ID", h("span", { class: "mono" }, r.run_id)],
    ];
    facts.replaceChildren(...rows.map(([k, v]) => [h("dt", null, k), h("dd", null, v)]).flat());
  }

  /** @param {JobStream} s */
  function progressBars(s) {
    /** @type {HTMLElement[]} */
    const out = [];
    if (s.hash) {
      const pct = percentOf(s.hash.done, s.hash.total);
      out.push(
        progressBar({
          label: "Hashing the input (SHA-256)",
          done: s.hash.done,
          total: s.hash.total,
          detail: `${formatBytes(s.hash.done)} of ${formatBytes(s.hash.total)}${pct === null ? "" : ` (${pct}%)`}`,
        }),
      );
    }
    if (s.seal) {
      const pct = percentOf(s.seal.done, s.seal.total);
      out.push(
        progressBar({
          label: "Sealing the report (report.sha256)",
          done: s.seal.done,
          total: s.seal.total,
          detail:
            s.seal.total === null ? `${plural(s.seal.done, "file", "files")}` : `${formatCount(s.seal.done)} of ${plural(s.seal.total, "file", "files")}${pct === null ? "" : ` (${pct}%)`}`,
        }),
      );
    }
    if (out.length === 0 && s.live) {
      out.push(h("p", { class: "muted small" }, "Input hashing and report sealing show their progress here when they run."));
    }
    return out;
  }

  /** @param {Exclude<RunStatus, "running"> | RunStatus} status */
  function renderResult(status) {
    if (!record || status === "running") return;
    const r = record;
    const finished = /** @type {RunFinished | null} */ (stream?.finished ?? null);
    const reasons = finished ? finished.reasons : r.status_reasons;
    const warnings = finished ? finished.warnings : r.warnings;
    const reportAvailable = finished ? finished.summary.report_available : r.leapp_result?.index_html_found === true;
    const counts = r.leapp_result?.module_counts ?? null;
    const analyzed = r.status !== "running";
    /** @type {[string, Node | string][]} */
    const summary = [];
    if (analyzed && counts) {
      summary.push([
        "Modules",
        `${formatCount(counts.complete)} complete, ${formatCount(counts.error)} error, ${formatCount(counts.no_files_found)} no files found, ${formatCount(counts.other)} other`,
      ]);
    }
    if (analyzed && r.leapp_result && r.leapp_result.error_modules.length > 0) {
      summary.push(["Modules with errors", h("span", { class: "mono" }, r.leapp_result.error_modules.join(", "))]);
    }
    if (analyzed) summary.push(["Report", sealText(r)]);
    if (!analyzed && finished) summary.push(["Module counts", h("span", { class: "muted" }, "Reading run.json…")]);

    resultCard.hidden = false;
    fill(
      resultCard,
      h("div", { class: "card-head" }, h("h2", { id: "run-result-heading" }, "Result"), statusBadge(status)),
      h("p", null, OUTCOME[/** @type {Exclude<RunStatus, "running">} */ (status)]),
      reasonList("Reasons", reasons, "reasons"),
      reasonList("Warnings", warnings, "warnings"),
      summary.length > 0 && h("dl", { class: "facts facts-compact" }, summary.map(([k, v]) => [h("dt", null, k), h("dd", null, v)])),
      parserOutput(),
      h(
        "div",
        { class: "actions" },
        h(
          "button",
          {
            class: "btn btn-primary",
            type: "button",
            disabled: !reportAvailable,
            title: reportAvailable ? "Open report/index.html in the default browser" : "This run has no report/index.html",
            onClick: openReport,
          },
          "Open report",
        ),
        h("button", { class: "btn", type: "button", onClick: revealFolder }, icon("folder"), "Reveal folder"),
        h("button", { class: "btn", type: "button", onClick: () => openFile("stdout") }, "Open stdout"),
        h("button", { class: "btn", type: "button", onClick: () => openFile("stderr") }, "Open stderr"),
        h("button", { class: "btn", type: "button", onClick: () => openFile("run_json") }, "Open run.json"),
      ),
    );
  }

  /** "Parser output": the stdout/stderr tails sent after LEAPP exits (CONTRACTS.md §11). */
  function parserOutput() {
    const tails = stream?.stdio;
    if (!tails || (tails.stdout === null && tails.stderr === null)) {
      return h(
        "div",
        { class: "stack-sm" },
        h("h3", null, "Parser output"),
        h("p", { class: "muted small" }, "LEAPP's stdout and stderr are shown here for runs that finish in this window. Open them with the buttons below."),
      );
    }
    /**
     * @param {string} name
     * @param {string[] | null} lines
     */
    const block = (name, lines) =>
      h(
        "details",
        { class: "parser-output", open: lines !== null && lines.length > 0 },
        h("summary", null, `${name} `, h("span", { class: "muted" }, lines === null ? "(not received)" : lines.length === 0 ? "(empty)" : `(last ${plural(lines.length, "line", "lines")})`)),
        lines && lines.length > 0 ? h("pre", { class: "pre" }, lines.join("\n")) : null,
      );
    return h("div", { class: "stack-sm" }, h("h3", null, "Parser output"), block("stdout", tails.stdout), block("stderr", tails.stderr));
  }

  async function refreshRecord() {
    try {
      const next = await api.run_get({ case_path: casePath, run_id: runId });
      if (disposed) return;
      record = next;
      resultBuiltFor = -1;
      render();
    } catch {
      // Keep the event's result; the case screen shows the record.
    }
  }

  // ---- Actions ----

  async function askCancel() {
    const ok = await confirmDialog({
      title: "Cancel this run?",
      message: h(
        "div",
        { class: "stack-sm" },
        h("p", null, "LEAPP is stopped and its whole process tree ends. The partial output is kept, sealed and recorded as cancelled."),
        h("p", { class: "muted small" }, "If LEAPP has already exited, only input hashing stops."),
      ),
      confirmLabel: "Cancel run",
      cancelLabel: "Keep running",
      danger: true,
    });
    if (!ok || disposed || !stream?.live) return;
    cancelling = true;
    actionErrors.clear();
    render();
    try {
      await api.run_cancel({ run_id: runId });
    } catch (err) {
      cancelling = false;
      if (disposed) return;
      actionErrors.show(err, "The run could not be cancelled.");
      render();
    }
  }

  async function openReport() {
    actionErrors.clear();
    try {
      await api.open_report({ case_path: casePath, run_id: runId });
    } catch (err) {
      showResultError(err, "The report could not be opened.");
    }
  }

  async function revealFolder() {
    if (!record) return;
    try {
      await api.reveal_path({ path: record.command.cwd });
    } catch (err) {
      showResultError(err, "The folder could not be revealed.");
    }
  }

  /** @param {RunFile} which */
  async function openFile(which) {
    try {
      await api.open_text_file({ case_path: casePath, run_id: runId, which });
    } catch (err) {
      showResultError(err, "The file could not be opened.");
    }
  }

  /**
   * @param {unknown} err
   * @param {string} titleText
   */
  function showResultError(err, titleText) {
    const slot = errorSlot();
    slot.show(err, titleText);
    resultCard.querySelector(".error-slot")?.remove();
    resultCard.append(slot.node);
  }

  // ---- Job end without a `finished` event, and the elapsed-time tick ----

  let lastJob = store.get().activeJob;
  cleanups.push(
    watch(
      store,
      (s) => jobKey(s.activeJob),
      () => {
        const job = store.get().activeJob;
        const was = lastJob;
        lastJob = job;
        if (was?.kind !== "run" || was.run_id !== runId || (job?.kind === "run" && job.run_id === runId)) return;
        // Give the finished event a moment; it normally arrives before job_active reports no job.
        setTimeout(() => {
          if (disposed || !stream || stream.finished) return;
          stalled = true;
          render();
        }, 1500);
      },
    ),
  );
  const ticker = setInterval(() => {
    if (stream?.live) renderElapsed();
  }, 1000);
  cleanups.push(() => clearInterval(ticker));

  void init();
  return {
    node,
    dispose() {
      disposed = true;
      if (frame !== 0) cancelAnimationFrame(frame);
      log.dispose();
      actionErrors.dispose();
      for (const fn of cleanups) fn();
    },
  };
}

/**
 * @param {string} heading
 * @param {readonly Reason[]} list
 * @param {"reasons" | "warnings"} kind
 */
function reasonList(heading, list, kind) {
  if (list.length === 0) return null;
  return h(
    "div",
    { class: `stack-sm reason-list reason-list-${kind}` },
    h("h3", null, heading),
    h("ul", { class: "list-compact" }, list.map((x) => h("li", null, h("code", null, x.code), " ", x.message))),
  );
}

/** @param {RunRecord} r */
function sealText(r) {
  const seal = r.output.seal;
  switch (seal.status) {
    case "sealed":
      return `Sealed in report.sha256: ${plural(seal.file_count ?? 0, "file", "files")}, ${formatBytes(seal.total_bytes)}`;
    case "skipped_no_output":
      return "No report folder, so nothing was sealed";
    case "pending":
      return "Not sealed yet";
    default:
      return `Seal ${seal.status}`;
  }
}
