// @ts-check
/**
 * Acquire screen (ROADMAP D5, ARCHITECTURE.md §6b): an iOS backup over USB into the case.
 *
 * - **Devices:** `devices_list` every 2 s while the list is shown and the window is visible, with at
 *   most one call in flight (lib/poll.js). The tools-state banner gives platform guidance. Each
 *   device card shows its pair state with instructions; the UI pairs only when Pair/Retry is pressed.
 * - **Options:** the preflight banner (ok / warn / block), a label, and the encryption options:
 *   turn encryption on (password twice, turn it off afterwards by default) when it is off, or a
 *   warning when the owner already turned it on.
 * - **Progress:** phase steps, overall percent, elapsed time, the `device_prompt` banner, the
 *   no-time-limit note while encryption changes, the log, and Cancel with a confirm.
 * - **Result:** status, reasons, warnings (with "Turn backup encryption off" when it may still be
 *   on), open folder / acquisition.json / logs, and "Parse with iLEAPP".
 *
 * Start, Pair and the encryption change are plain buttons: Enter in a field never triggers them.
 */
import { appError, errorSlot } from "../components/app-error.js";
import { confirmDialog } from "../components/dialog.js";
import { logView } from "../components/log-view.js";
import { percentOf, progressMeter, stepList } from "../components/progress.js";
import { restoreEncryptionDialog } from "../components/restore-dialog.js";
import {
  acqBlockers,
  buildAcqRequest,
  canAcquire,
  deviceState,
  enablesEncryption,
  encryptionLeftOnText,
  encryptionOption,
  mergeDevice,
  needsEncryptionOff,
  pairOutcome,
  passwordProblem,
  pickDevice,
  preflightInfo,
  resetAcqControls,
  toolPasswordProblem,
  toolsGuidance,
  withPairOutcomes,
} from "../lib/acquire.js";
import { folderLabel } from "../lib/cases.js";
import { fill, h, keepFocus, keyedSlot, setText } from "../lib/dom.js";
import { toAppError } from "../lib/errors.js";
import { field, textInput } from "../lib/form.js";
import { elapsedSince, formatBytes, formatCount, formatElapsed, plural } from "../lib/format.js";
import { handoff } from "../lib/handoff.js";
import { ACQ_PHASES, stepStates } from "../lib/jobstream.js";
import { jobKey, setActiveJob } from "../lib/jobs.js";
import { createPoller } from "../lib/poll.js";
import { routeHref } from "../lib/router.js";
import { watch } from "../lib/store.js";
import { icon, statusBadge, timeText, uid } from "../lib/view.js";

/** @typedef {import("../types").AcqFile} AcqFile */
/** @typedef {import("../types").AcqPreflight} AcqPreflight */
/** @typedef {import("../types").AcqStatus} AcqStatus */
/** @typedef {import("../types").AcqSummary} AcqSummary */
/** @typedef {import("../types").AcquisitionRecord} AcquisitionRecord */
/** @typedef {import("../types").DeviceSummary} DeviceSummary */
/** @typedef {import("../types").DevicesResult} DevicesResult */
/** @typedef {import("../types").Reason} Reason */
/** @typedef {import("../lib/acquire.js").AcqForm} AcqForm */
/** @typedef {import("../lib/jobstream.js").AcqFinished} AcqFinished */
/** @typedef {import("../lib/jobstream.js").JobStream} JobStream */
/** @typedef {import("../lib/context").ScreenContext} ScreenContext */
/** @typedef {import("../lib/context").View} View */

const POLL_MS = 2000;
const ENCRYPTION_PHASES = new Set(["enabling_encryption", "restoring_encryption"]);

/** @type {Record<Exclude<AcqStatus, "running">, string>} */
const OUTCOME = {
  succeeded: "The backup is complete and validated, and backup.sha256 seals it.",
  failed: "The backup failed. The reasons come from the tool's messages and the backup's contents, not only its exit code.",
  cancelled: "The acquisition was cancelled. Any partial backup is kept and recorded as cancelled.",
  interrupted: "The app closed while this acquisition was running.",
};

/**
 * @param {ScreenContext} ctx
 * @returns {View}
 */
export function acquireScreen(ctx) {
  const { api, store, jobs, navigate } = ctx;
  const casePath = ctx.params.case ?? "";
  const caseHref = routeHref("case", { path: casePath });
  let disposed = false;
  /** @type {(() => void)[]} */
  const cleanups = [];
  /** @type {"loading" | "setup" | "progress" | "result"} */
  let view = "loading";

  // ---- Setup state ----
  /** @type {DevicesResult | null} */
  let devices = null;
  /** @type {unknown} */
  let pollError = null;
  /** @type {string | null} */
  let selected = null;
  /** @type {Set<string>} */
  const pairing = new Set();
  /** @type {Map<string, unknown>} */
  const pairErrors = new Map();
  /** The last Pair answer per UDID that did not pair (lib/acquire.js `withPairOutcomes`). */
  /** @type {Map<string, import("../lib/acquire.js").PairOutcome>} */
  let pairOutcomes = new Map();
  /** @type {AcqForm["preflight"]} */
  let preflight = null;
  /** @type {unknown} */
  let preflightError = null;
  let preflightSeq = 0;
  let starting = false;

  // ---- Progress / result state ----
  /** @type {JobStream | null} */
  let stream = null;
  /** @type {AcquisitionRecord | null} */
  let record = null;
  let frame = 0;
  let cancelling = false;
  let finalRecordAsked = false;
  /**
   * The case's current row for this acquisition, re-read (`case_open`) after "Turn backup
   * encryption off": its derived `warnings` say whether the action still applies.
   * @type {AcqSummary | null}
   */
  let summaryNow = null;
  /** @type {() => void} */
  let unwatchHandoff = () => {};

  // ---- Persistent controls ----
  const labelInput = textInput({ name: "label", autocomplete: "off", maxlength: 200 });
  const enableBox = /** @type {HTMLInputElement} */ (h("input", { type: "checkbox", name: "enable_encryption" }));
  const password = /** @type {HTMLInputElement} */ (h("input", { class: "input", type: "password", name: "backup_password", autocomplete: "off", spellcheck: "false" }));
  const password2 = /** @type {HTMLInputElement} */ (h("input", { class: "input", type: "password", name: "backup_password_again", autocomplete: "off", spellcheck: "false" }));
  const restoreBox = /** @type {HTMLInputElement} */ (h("input", { type: "checkbox", name: "restore_encryption" }));
  const parseBox = /** @type {HTMLInputElement} */ (h("input", { type: "checkbox", name: "parse_after" }));
  /** @type {import("../lib/acquire.js").AcqControls} */
  const controls = { label: labelInput, enableEncryption: enableBox, password, password2, restoreEncryption: restoreBox, parseAfter: parseBox };
  resetAcqControls(controls);
  const passwordField = field({ label: "Backup password", control: password, required: true, hint: "At least 4 characters. You need it to parse the backup; it is never stored." });
  const password2Field = field({ label: "Password again", control: password2, required: true });
  for (const el of [labelInput, password, password2]) el.addEventListener("input", () => renderStart());
  for (const el of [enableBox, restoreBox, parseBox]) el.addEventListener("change", () => renderOptions());
  password2.addEventListener("blur", () => renderPasswordError(true));
  password.addEventListener("input", () => renderPasswordError(false));

  // ---- Layout ----
  const caseLink = h("a", { href: caseHref }, folderLabel(casePath));
  const title = h("h1", { tabindex: "-1" }, "Acquire iOS backup");
  const statusSlot = h("div", { class: "run-status" });
  const headActions = h("div", { class: "actions" });
  const body = h("div", { class: "stack", "aria-busy": "true" }, h("p", { class: "muted" }, "Loading…"));
  const node = h(
    "section",
    { class: "screen acquire-screen" },
    h(
      "nav",
      { class: "breadcrumb", "aria-label": "Breadcrumb" },
      h("a", { href: routeHref("cases") }, "Cases"),
      h("span", { "aria-hidden": "true" }, " / "),
      caseLink,
      h("span", { "aria-hidden": "true" }, " / "),
    ),
    h("div", { class: "screen-head" }, h("div", { class: "title-row" }, title, statusSlot), headActions),
    body,
  );

  // Setup view parts
  const jobBanner = h("div");
  const toolsBanner = h("div");
  const devicesStatus = h("span", { class: "muted small", role: "status" });
  const devicesBody = h("div", { class: "stack" });
  const devicesCard = h(
    "section",
    { class: "card", "aria-labelledby": "acq-devices-heading" },
    h("div", { class: "card-head" }, h("h2", { id: "acq-devices-heading" }, "Devices"), devicesStatus),
    devicesBody,
  );
  const optionsBody = h("div", { class: "stack" });
  const optionsCard = h("section", { class: "card", "aria-labelledby": "acq-options-heading", hidden: true }, h("h2", { id: "acq-options-heading" }, "Options"), optionsBody);
  const reasonsId = uid("acq-reasons");
  const reasons = h("div", { class: "start-reasons", id: reasonsId });
  const startErrors = errorSlot();
  const startButton = /** @type {HTMLButtonElement} */ (
    h("button", { class: "btn btn-primary btn-lg", type: "button", "aria-describedby": reasonsId, disabled: true, onClick: start }, "Start acquisition")
  );
  const startCard = h(
    "section",
    { class: "card start-card", "aria-label": "Start" },
    reasons,
    startErrors.node,
    h("div", { class: "form-actions" }, h("a", { class: "btn", href: caseHref }, "Cancel"), startButton),
  );

  // Progress / result parts. The view renders on every event (up to 4/s during a backup); each part
  // is rebuilt only when what it shows changes, so focus stays put and banners are announced once.
  const facts = keyedSlot(h("dl", { class: "facts run-facts" }));
  const elapsed = h("span", { class: "elapsed" });
  const elapsedLabel = h("span", null, "Elapsed");
  const statusBadgeSlot = keyedSlot(statusSlot);
  const promptSlot = keyedSlot(h("div"));
  const phaseSlot = keyedSlot(h("div"));
  const progressSlot = keyedSlot(h("div", { class: "stack-sm progress-slot" }));
  const backupMeter = progressMeter({ label: "Backup", done: null, total: null, detail: "" });
  const sealMeter = progressMeter({ label: "Sealing the backup (backup.sha256)", done: null, total: null, detail: "" });
  const progressNote = h("p", { class: "muted small" }, "The backup's overall progress shows here once the device sends data.");
  const progressErrors = errorSlot();
  const progressCard = h(
    "section",
    { class: "card", "aria-labelledby": "acq-progress-heading" },
    h("div", { class: "card-head" }, h("h2", { id: "acq-progress-heading" }, "Progress"), h("span", { class: "muted" }, elapsedLabel, " ", elapsed)),
    phaseSlot.node,
    progressSlot.node,
    progressErrors.node,
  );
  const resultCard = h("section", { class: "card result-card", "aria-labelledby": "acq-result-heading", hidden: true });
  // Three parts that stay in place (`display: contents`, so the card's own spacing applies), each
  // rebuilt only when what it shows changes: the encryption alert is announced once, not again
  // when the final record arrives.
  const resultHeadSlot = keyedSlot(h("div", { class: "slot-contents" }));
  const encryptionSlot = keyedSlot(h("div", { class: "slot-contents" }));
  const resultBodySlot = keyedSlot(h("div", { class: "slot-contents" }));
  const log = logView({ label: "Backup log", emptyText: "No log lines yet." });
  const logCard = h("section", { class: "card", "aria-labelledby": "acq-log-heading" }, h("div", { class: "card-head" }, h("h2", { id: "acq-log-heading" }, "Log")), log.node);
  const cancelButton = /** @type {HTMLButtonElement} */ (h("button", { class: "btn btn-danger", type: "button", onClick: askCancel }, "Cancel acquisition"));

  // ---- Polling ----
  const poller = createPoller({
    fetch: () => api.devices_list(),
    intervalMs: POLL_MS,
    onResult: (result) => {
      // A Pair answer stays on screen while polls report `not_paired` (no host pair record yet).
      const shown = withPairOutcomes(result.devices, pairOutcomes);
      pairOutcomes = shown.outcomes;
      devices = { ...result, devices: shown.devices };
      pollError = null;
      onDevices();
    },
    onError: (err) => {
      pollError = err;
      renderDevices();
    },
  });
  const syncPolling = () => {
    if (!disposed && view === "setup" && document.visibilityState === "visible") poller.start();
    else poller.stop();
  };
  document.addEventListener("visibilitychange", syncPolling);
  cleanups.push(() => document.removeEventListener("visibilitychange", syncPolling), () => poller.stop());

  // ---- Views ----

  async function init() {
    try {
      const cases = await api.cases_list();
      if (disposed) return;
      const summary = cases.find((c) => c.path === casePath);
      if (!summary?.case) throw { code: "case_not_found", message: "This case is not in the recent list, or its folder is missing.", detail: casePath };
      caseLink.textContent = summary.case.name;
      const job = store.get().activeJob;
      const last = jobs.current();
      if (job?.kind === "acquisition" && job.case_path === casePath) {
        let s = jobs.find("acquisition", job.acq_id);
        if (!s) {
          try {
            s = await jobs.attach(api, "acquisition", job.acq_id, casePath);
          } catch {
            s = null;
          }
        }
        if (disposed) return;
        if (s) {
          showJob(s);
          return;
        }
      }
      if (last?.kind === "acquisition" && last.casePath === casePath && last.id && (last.live || (last.finished && !last.acknowledged))) {
        showJob(last);
        return;
      }
      showSetup();
    } catch (err) {
      if (disposed) return;
      body.replaceChildren(appError(err, { title: "The Acquire screen could not be loaded." }).node, h("p", null, h("a", { href: caseHref }, "Back to the case")));
    } finally {
      body.removeAttribute("aria-busy");
    }
  }

  function showSetup() {
    view = "setup";
    stream = null;
    record = null;
    // Pair answers of an earlier acquisition no longer describe the devices.
    pairOutcomes = new Map();
    title.textContent = "Acquire iOS backup";
    statusBadgeSlot.reset();
    statusSlot.replaceChildren();
    headActions.replaceChildren();
    body.replaceChildren(jobBanner, toolsBanner, devicesCard, optionsCard, startCard);
    renderJobBanner();
    devicesKey = "";
    renderDevices();
    renderOptions();
    syncPolling();
  }

  /** @param {JobStream} s */
  function showJob(s) {
    view = s.finished ? "result" : "progress";
    stream = s;
    record = null;
    summaryNow = null;
    cancelling = false;
    finalRecordAsked = false;
    for (const slot of [facts, statusBadgeSlot, promptSlot, phaseSlot, progressSlot, resultHeadSlot, encryptionSlot, resultBodySlot]) slot.reset();
    resultCard.hidden = true;
    resultCard.replaceChildren(resultHeadSlot.node, encryptionSlot.node, resultBodySlot.node);
    progressErrors.clear();
    poller.stop();
    unwatchHandoff();
    unwatchHandoff = s.id ? handoff.watch(s.id) : () => {};
    const own = s;
    cleanups.push(jobs.subscribe((x) => x === own && schedule()));
    headActions.replaceChildren(cancelButton);
    body.replaceChildren(facts.node, promptSlot.node, progressCard, resultCard, logCard);
    void loadRecord();
    // A result shown again (the screen was left and reopened) may be out of date: a later restore
    // may have turned encryption off since its `finished` event. Re-read the case's row.
    if (s.finished && s.id) void reloadSummary(s.id);
    render();
  }

  async function loadRecord() {
    const s = stream;
    if (!s?.id) return;
    try {
      const r = await api.acq_get({ case_path: casePath, acq_id: s.id });
      if (disposed || stream !== s) return;
      record = r;
      render();
    } catch {
      // The header and the result show what the events said.
    }
  }

  // ---- Setup: devices ----

  function renderJobBanner() {
    const job = store.get().activeJob;
    jobBanner.replaceChildren(
      job
        ? h(
            "div",
            { class: "banner banner-info", role: "status" },
            icon("info"),
            h(
              "p",
              null,
              job.kind === "run" ? "A run is in progress. " : "An acquisition is in progress in another case. ",
              "Only one job runs at a time; you can prepare the acquisition now and start it when that job ends.",
            ),
          )
        : "",
    );
  }

  function onDevices() {
    const list = devices?.devices ?? [];
    const next = devices?.tools.state === "ok" ? pickDevice(list, selected) : null;
    if (next !== selected) {
      selected = next;
      void loadPreflight();
    }
    renderDevices();
    renderOptions();
  }

  /** What the device section showed last; a poll that changes nothing does not re-render it. */
  let devicesKey = "";

  function renderDevices() {
    if (view !== "setup") return;
    const key = JSON.stringify([devices, pollError ? toAppErrorKey(pollError) : null, selected, [...pairing], [...pairErrors.keys()]]);
    if (key === devicesKey) return;
    devicesKey = key;
    const os = store.get().appInfo?.os;
    const tools = devices?.tools ?? null;
    const guide = tools ? toolsGuidance(tools.state, os) : null;
    // The tools work but the device service failed for this poll: the core reports state `ok`
    // with guidance (e.g. "Listing the devices failed unexpectedly.").
    const listProblem = tools?.state === "ok" && tools.guidance ? tools.guidance : null;
    toolsBanner.replaceChildren(
      tools && guide
        ? h(
            "div",
            { class: "banner banner-danger tools-banner", role: "alert" },
            icon("alert-triangle"),
            h(
              "div",
              { class: "stack-sm" },
              h("strong", null, guide.title),
              h("p", null, guide.text),
              tools.guidance && tools.guidance !== guide.text && h("p", { class: "small" }, "Details: ", tools.guidance),
            ),
          )
        : listProblem
          ? h(
              "div",
              { class: "banner banner-warn tools-banner", role: "status" },
              icon("alert-triangle"),
              h(
                "div",
                { class: "stack-sm" },
                h("strong", null, "The connected devices could not be listed."),
                h("p", null, listProblem),
                h("p", { class: "small" }, "The list is read again every 2 seconds. If this persists, reconnect the device."),
              ),
            )
          : "",
    );
    devicesStatus.textContent = pollError ? "" : devices ? `Checked every ${POLL_MS / 1000} s` : "Looking for devices…";
    /** @type {Node[]} */
    const parts = [];
    if (pollError) parts.push(appError(pollError, { title: "The device list could not be read. It is tried again every 2 seconds." }).node);
    if (!devices) {
      parts.push(h("p", { class: "muted", role: "status" }, "Looking for connected devices…"));
    } else if (devices.tools.state !== "ok") {
      parts.push(h("p", { class: "muted" }, "Devices are listed once the iOS tools work."));
    } else if (devices.devices.length === 0 && listProblem) {
      parts.push(h("p", { class: "muted" }, "Devices are listed once the device service answers."));
    } else if (devices.devices.length === 0) {
      parts.push(
        h(
          "div",
          { class: "empty-state empty-state-sm" },
          icon("smartphone", "icon icon-xl"),
          h("h3", null, "No iOS device connected"),
          h("p", null, "Connect the iPhone or iPad with a USB cable and unlock it. It appears here within a few seconds."),
        ),
      );
    } else {
      parts.push(
        fill(
          h("fieldset", { class: "device-list" }),
          h("legend", { class: "visually-hidden" }, "Device to back up"),
          devices.devices.map(deviceCard),
        ),
      );
    }
    keepFocus(devicesBody, () => devicesBody.replaceChildren(...parts));
  }

  /** @param {DeviceSummary} d */
  function deviceCard(d) {
    const info = deviceState(d);
    const ready = canAcquire(d);
    const name = d.device_name ?? (d.pair_state === "paired" ? "Unnamed device" : "Name shown after pairing");
    const headingId = uid("device");
    /** @type {Node} */
    let heading;
    if (ready) {
      const radio = /** @type {HTMLInputElement} */ (h("input", { type: "radio", name: "device", value: d.udid, "data-focus-key": `device:${d.udid}` }));
      radio.checked = d.udid === selected;
      radio.addEventListener("change", () => {
        if (!radio.checked) return;
        selected = d.udid;
        void loadPreflight();
        renderDevices();
        renderOptions();
      });
      heading = h("label", { class: "device-title", id: headingId }, radio, h("span", null, name));
    } else {
      heading = h("h3", { class: "device-title", id: headingId }, icon("smartphone"), h("span", null, name));
    }
    const busy = pairing.has(d.udid);
    const err = pairErrors.get(d.udid);
    /** @type {[string, Node | string][]} */
    const rows = [
      ["Model", d.product_type ?? dash()],
      ["iOS", d.product_version ?? dash()],
      ["Serial", d.serial_number ?? h("span", { class: "muted" }, d.pair_state === "paired" ? "—" : "Shown after pairing")],
      ["UDID", h("span", { class: "mono break" }, d.udid)],
      ["Backup encryption", encryptionText(d)],
    ];
    if (d.data_used_bytes !== null) {
      rows.push(["Data", d.data_capacity_bytes !== null ? `${formatBytes(d.data_used_bytes)} of ${formatBytes(d.data_capacity_bytes)}` : formatBytes(d.data_used_bytes)]);
    }
    return h(
      "article",
      { class: `device-card${d.udid === selected ? " is-selected" : ""}`, "aria-labelledby": headingId },
      h("div", { class: "device-card-head" }, heading, h("span", { class: `badge badge-tone-${info.tone}` }, icon(info.icon), info.label)),
      h("dl", { class: "facts facts-compact" }, rows.map(([k, v]) => [h("dt", null, k), h("dd", null, v)])),
      !ready && h("p", { class: "device-guidance" }, info.text),
      d.message && !d.busy && d.pair_state !== "paired" && h("p", { class: "muted small" }, "The tool said: ", h("span", { class: "break" }, d.message)),
      info.action &&
        h(
          "div",
          { class: "actions" },
          h(
            "button",
            { class: "btn btn-sm", type: "button", disabled: busy, "data-focus-key": `pair:${d.udid}`, onClick: () => pair(d.udid) },
            busy ? "Pairing…" : info.action === "pair" ? "Pair" : "Retry pairing",
          ),
          busy && h("span", { class: "muted small", role: "status" }, "Unlock the device and tap Trust if it asks."),
        ),
      err !== undefined && appError(err, { title: "Pairing did not complete." }).node,
    );
  }

  /** @param {DeviceSummary} d */
  function encryptionText(d) {
    if (d.will_encrypt === true) return h("span", null, icon("lock"), " On (set by the owner or an earlier acquisition)");
    if (d.will_encrypt === false) return "Off";
    return h("span", { class: "muted" }, d.pair_state === "paired" ? "Unknown" : "Shown after pairing");
  }

  /**
   * The only code path that pairs: the examiner pressed Pair or Retry.
   * @param {string} udid
   */
  async function pair(udid) {
    pairing.add(udid);
    pairErrors.delete(udid);
    // Pressing Pair again forgets the previous answer.
    pairOutcomes.delete(udid);
    renderDevices();
    try {
      const updated = await api.device_pair({ udid });
      if (disposed) return;
      // A poll that started before this answer would overwrite it with the old state.
      poller.invalidate();
      const outcome = pairOutcome(updated);
      if (outcome) pairOutcomes.set(udid, outcome);
      if (devices) devices = { ...devices, devices: mergeDevice(devices.devices, updated) };
    } catch (err) {
      pairErrors.set(udid, err);
    } finally {
      pairing.delete(udid);
    }
    if (!disposed) onDevices();
  }

  async function loadPreflight() {
    const udid = selected;
    const seq = ++preflightSeq;
    preflightError = null;
    preflight = udid ? "loading" : null;
    renderOptions();
    if (!udid) return;
    try {
      const p = await api.acq_preflight({ case_path: casePath, udid });
      if (disposed || seq !== preflightSeq) return;
      preflight = p;
    } catch (err) {
      if (disposed || seq !== preflightSeq) return;
      preflight = "failed";
      preflightError = err;
    }
    renderOptions();
  }

  // ---- Setup: options and start ----

  /** @returns {DeviceSummary | null} */
  function selectedDevice() {
    return devices?.devices.find((d) => d.udid === selected) ?? null;
  }

  /** @returns {AcqForm} */
  function form() {
    return {
      toolsOk: devices?.tools.state === "ok",
      device: selectedDevice(),
      preflight,
      label: labelInput.value,
      enableEncryption: enableBox.checked,
      password: password.value,
      password2: password2.value,
      restoreEncryption: restoreBox.checked,
      jobActive: store.get().activeJob !== null,
      windows: store.get().appInfo?.os === "windows",
    };
  }

  function renderOptions() {
    if (view !== "setup") return;
    const d = selectedDevice();
    optionsCard.hidden = d === null;
    if (d) keepFocus(optionsBody, () => fill(optionsBody, optionParts(d)));
    renderStart();
  }

  /** @param {DeviceSummary} d */
  function optionParts(d) {
    /** @type {(Node | null)[]} */
    const parts = [preflightBlock()];
    parts.push(field({ label: "Label", control: labelInput, hint: "Optional, e.g. the exhibit number. Shown in the case and recorded in acquisition.json." }).node);
    const mode = encryptionOption(d);
    const encId = uid("enc");
    /** @type {(Node | null | false)[]} */
    const enc = [h("h3", { id: encId }, "Backup encryption")];
    if (mode === "offer") {
      enc.push(
        h(
          "div",
          { class: "field" },
          h("label", { class: "check" }, enableBox, h("span", null, "Enable backup encryption (recommended)")),
          h(
            "p",
            { class: "field-hint" },
            "Encrypted backups contain more data (for example saved passwords, Health and Wi-Fi settings). This changes a setting on the device: it turns backup encryption on with a password you choose. The change is recorded in acquisition.json.",
          ),
        ),
      );
      if (enableBox.checked) {
        enc.push(
          h("div", { class: "form-row" }, passwordField.node, password2Field.node),
          h(
            "div",
            { class: "field" },
            h("label", { class: "check" }, restoreBox, h("span", null, "Turn encryption off again afterwards")),
            h(
              "p",
              { class: "field-hint" },
              "Restores the device's setting after the backup, even if it fails or is cancelled. If that does not work, the case offers “Turn backup encryption off” later.",
            ),
          ),
          h(
            "div",
            { class: "field" },
            h("label", { class: "check" }, parseBox, h("span", null, "Parse with iLEAPP now")),
            h(
              "p",
              { class: "field-hint" },
              "Keeps this password in memory (never stored) so “Parse with iLEAPP” can fill it in after a successful backup. It is cleared when that run starts, when its form closes, or when you leave this acquisition's result.",
            ),
          ),
        );
      }
    } else if (mode === "already_on") {
      enc.push(
        h(
          "div",
          { class: "banner banner-warn", role: "note" },
          icon("lock"),
          h(
            "div",
            { class: "stack-sm" },
            h("strong", null, "Backup encryption is already on for this device."),
            h(
              "p",
              null,
              "The backup is encrypted with the owner's password, which is needed to parse it. suiteDFIR cannot change or remove that password (only “Reset All Settings” on the device can), and does not try.",
            ),
          ),
        ),
      );
    } else {
      enc.push(
        h(
          "div",
          { class: "banner banner-info", role: "note" },
          icon("info"),
          h("p", null, "The device's backup-encryption setting could not be read, so it cannot be changed from here. If the backup is encrypted, parsing needs its password."),
        ),
      );
    }
    parts.push(fill(h("div", { class: "stack-sm encryption-options", role: "group", "aria-labelledby": encId }), enc));
    renderPasswordError(false);
    return parts;
  }

  function preflightBlock() {
    if (preflight === null) return null;
    if (preflight === "loading") return h("p", { class: "muted", role: "status" }, "Checking the free space in the case folder…");
    if (preflight === "failed") return appError(preflightError, { title: "The free-space check failed." }).node;
    const info = preflightInfo(preflight);
    return h(
      "div",
      { class: `banner banner-${info.tone === "ok" ? "ok" : info.tone === "warn" ? "warn" : "danger"} preflight preflight-${preflight.level}`, role: "status" },
      icon(info.tone === "ok" ? "check-circle" : info.tone === "warn" ? "alert-triangle" : "x-circle"),
      h("div", { class: "stack-sm" }, h("strong", null, info.title), h("p", null, info.text)),
    );
  }

  /** @param {boolean} touched Show the mismatch only after the second field was left. */
  function renderPasswordError(touched) {
    const f = form();
    const on = enablesEncryption(f);
    // A password the iOS tools cannot take (Windows: printable ASCII only) shows at once, inline.
    const toolProblem = on ? toolPasswordProblem(f.password, f.windows) : null;
    passwordField.setError(toolProblem);
    if (!on || (password2.value === "" && !touched)) {
      password2Field.setError(null);
      return;
    }
    const problem = passwordProblem(f);
    const second = problem === toolProblem ? null : problem;
    password2Field.setError(second && (touched || password2.value.length >= password.value.length) ? second : null);
  }

  function renderStart() {
    if (view !== "setup") return;
    const blockers = acqBlockers(form());
    startButton.disabled = starting || blockers.length > 0;
    startButton.textContent = starting ? "Starting…" : "Start acquisition";
    reasons.replaceChildren(
      ...(blockers.length > 0
        ? [h("p", { class: "start-reasons-title" }, "Start becomes available when you:"), h("ul", { class: "list-compact" }, blockers.map((b) => h("li", null, b)))]
        : [h("p", { class: "ready-text" }, icon("check-circle"), enablesEncryption(form()) ? "Ready to turn encryption on and back up." : "Ready to back up.")]),
    );
  }

  function clearPasswords() {
    password.value = "";
    password2.value = "";
  }

  async function start() {
    const f = form();
    if (starting || acqBlockers(f).length > 0) return;
    const req = buildAcqRequest(casePath, f);
    const keep = req.enable_encryption && parseBox.checked;
    starting = true;
    startErrors.clear();
    renderStart();
    const beginning = jobs.begin("acquisition", casePath);
    try {
      const started = await api.acq_start(req, beginning.onEvent);
      beginning.bind(started.acq_id);
      if (keep && req.encryption_password) handoff.hold(started.acq_id, req.encryption_password);
      else handoff.drop();
      clearPasswords();
      api
        .job_active()
        .then((job) => setActiveJob(store, job))
        .catch(() => {});
      if (disposed) return;
      const s = jobs.find("acquisition", started.acq_id);
      if (s) showJob(s);
    } catch (err) {
      beginning.abandon();
      clearPasswords();
      if (!disposed) startErrors.show(err, "The acquisition could not be started.");
    } finally {
      starting = false;
      renderPasswordError(false);
      if (!disposed) renderStart();
    }
  }

  // ---- Progress and result ----

  function schedule() {
    if (frame === 0 && !disposed) frame = requestAnimationFrame(render);
  }

  function render() {
    frame = 0;
    const s = stream;
    if (!s || disposed) return;
    const finished = /** @type {AcqFinished | null} */ (s.finished);
    if (finished && view === "progress") view = "result";
    // The record read at the start is still "running": read the final one (seal, encryption) once.
    if (finished && record?.status === "running" && !finalRecordAsked) {
      finalRecordAsked = true;
      void loadRecord();
    }
    setText(title, record?.label ?? s.finished?.summary.label ?? "Acquisition");
    const status = finished ? finished.status : "running";
    statusBadgeSlot.update(status, () => statusBadge(status));
    // §6b: a cancel is ignored while encryption is turned off again (the restore always completes).
    const restoring = s.phase === "restoring_encryption";
    cancelButton.hidden = !s.live;
    cancelButton.disabled = cancelling || restoring;
    setText(cancelButton, cancelling ? "Cancelling…" : "Cancel acquisition");
    cancelButton.title = restoring ? "Turning encryption off always completes; it cannot be cancelled." : "";
    renderFacts();
    renderElapsed();
    renderPrompt(s);
    // Attached after a reload: turning encryption off again implies it was turned on (§6b step 8).
    const phases =
      s.partial && s.phases.includes("restoring_encryption") && !s.phases.includes("enabling_encryption") ? ["enabling_encryption", ...s.phases] : s.phases;
    const steps = stepStates(ACQ_PHASES, { phases, phase: s.phase, finished: finished !== null, partial: s.partial }, ENCRYPTION_PHASES);
    phaseSlot.update(JSON.stringify(steps), () => stepList("Acquisition phases", steps));
    renderProgress(s);
    log.update(s.log);
    if (finished) renderResult(finished);
  }

  function renderFacts() {
    const s = stream;
    const r = record;
    const summary = /** @type {AcqFinished | null} */ (s?.finished ?? null)?.summary ?? null;
    const data = {
      device: r?.device.device_name ?? summary?.device_name ?? null,
      model: r?.device.product_type ?? null,
      ios: r?.device.product_version ?? summary?.product_version ?? null,
      udid: r?.device.udid ?? summary?.udid ?? null,
      label: r?.label ?? summary?.label ?? null,
      created: r?.created_at ?? summary?.created_at ?? null,
      id: s?.id ?? null,
    };
    // Rebuilt only when a value changes: the time element is focusable (its UTC tooltip).
    facts.update(JSON.stringify(data), () => {
      /** @type {[string, Node | string][]} */
      const rows = [
        ["Device", data.device ?? dash()],
        ["Model", data.model ?? dash()],
        ["iOS", data.ios ?? dash()],
        ["UDID", h("span", { class: "mono break" }, data.udid ?? "—")],
        ["Label", data.label ?? dash()],
        ["Created", timeText(data.created)],
        ["Acquisition ID", h("span", { class: "mono" }, data.id ?? "—")],
      ];
      return rows.flatMap(([k, v]) => [h("dt", null, k), h("dd", null, v)]);
    });
  }

  function renderElapsed() {
    const finished = /** @type {AcqFinished | null} */ (stream?.finished ?? null);
    const created = record?.created_at ?? store.get().activeJob?.created_at ?? null;
    const ms = finished ? finished.summary.duration_ms : elapsedSince(created, Date.now());
    elapsedLabel.textContent = finished ? "Duration" : "Elapsed";
    elapsed.textContent = formatElapsed(ms);
  }

  /**
   * The device-prompt banner (`role="alert"`) or the encryption-phase note (`role="status"`),
   * inserted only when it changes, so each is announced once.
   * @param {JobStream} s
   */
  function renderPrompt(s) {
    const key = !s.live ? "" : s.prompt ? `prompt|${s.phase}|${s.prompt.kind}|${s.prompt.text}` : s.phase && ENCRYPTION_PHASES.has(s.phase) ? `phase|${s.phase}` : "";
    promptSlot.update(key, () => promptBanner(s));
  }

  /** @param {JobStream} s */
  function promptBanner(s) {
    if (!s.live) return null;
    const noCancel = s.phase === "restoring_encryption" ? "Cancel is not available in this step: turning encryption off always completes." : null;
    if (s.prompt) {
      return h(
        "div",
        { class: "banner banner-prompt", role: "alert" },
        icon("smartphone", "icon icon-prompt"),
        h(
          "div",
          { class: "stack-sm" },
          h("strong", { class: "prompt-title" }, "Enter the passcode on the device"),
          h("p", null, s.prompt.kind === "passcode_for_encryption" ? "Changing the backup-encryption setting needs the device's passcode." : "The backup starts after the passcode is entered on the device."),
          h("p", { class: "mono small" }, s.prompt.text),
          h("p", { class: "small" }, "The app waits for the device without a time limit.", noCancel && ` ${noCancel}`),
        ),
      );
    }
    if (s.phase && ENCRYPTION_PHASES.has(s.phase)) {
      return h(
        "div",
        { class: "banner banner-info", role: "status" },
        icon("lock"),
        h(
          "p",
          null,
          s.phase === "enabling_encryption" ? "Turning backup encryption on. " : "Turning backup encryption off. ",
          "The app waits for the device without a time limit; if the device asks for its passcode, enter it on the device.",
          noCancel && ` ${noCancel}`,
        ),
      );
    }
    return null;
  }

  /**
   * The overall percent and the seal progress, updated in place.
   * @param {JobStream} s
   */
  function renderProgress(s) {
    if (s.percent !== null) backupMeter.update({ label: "Backup", done: s.percent, total: 100, detail: `${s.percent}%` });
    if (s.seal) {
      const pct = percentOf(s.seal.done, s.seal.total);
      sealMeter.update({
        label: "Sealing the backup (backup.sha256)",
        done: s.seal.done,
        total: s.seal.total,
        detail: s.seal.total === null ? plural(s.seal.done, "file", "files") : `${formatCount(s.seal.done)} of ${plural(s.seal.total, "file", "files")}${pct === null ? "" : ` (${pct}%)`}`,
      });
    }
    // Until the backup reports progress (after a reload, only its next report shows it).
    const beforeBackupEnd = s.phase === null || s.phase === "preparing" || s.phase === "enabling_encryption" || s.phase === "backing_up";
    const note = s.percent === null && s.seal === null && s.live && beforeBackupEnd;
    progressSlot.update(`${s.percent !== null}|${s.seal !== null}|${note}`, () => [s.percent !== null && backupMeter.node, s.seal !== null && sealMeter.node, note && progressNote]);
  }

  /** @param {AcqFinished} fin */
  function renderResult(fin) {
    const s = stream;
    if (!s?.id) return;
    const acqId = s.id;
    const r = record;
    const summary = fin.summary;
    // "Turn backup encryption off" follows the case's AcqSummary.warnings: the finished event's
    // summary, or the row re-read after a later restore. The core leaves the two codes out once a
    // restore attempt recorded `restored: true`; acquisition.json (the warnings below) keeps them.
    const current = summaryNow ?? summary;
    const offerOff = needsEncryptionOff(current.warnings);
    const turnedOffLater = needsEncryptionOff(summary.warnings) && !offerOff;
    const backupPath = summary.backup_path;
    const canParse = fin.status === "succeeded" && backupPath !== null;
    const kept = handoff.has(acqId);
    resultHeadSlot.update(fin.status, () => [
      h("div", { class: "card-head" }, h("h2", { id: "acq-result-heading" }, "Result"), statusBadge(fin.status)),
      h("p", null, OUTCOME[/** @type {Exclude<AcqStatus, "running">} */ (fin.status)] ?? ""),
    ]);
    encryptionSlot.update(JSON.stringify([offerOff, turnedOffLater, current.warnings]), () => [
      offerOff &&
        h(
          "div",
          { class: "banner banner-danger", role: "alert" },
          icon("lock"),
          h(
            "div",
            { class: "stack-sm" },
            h("strong", null, "Backup encryption may still be on for this device."),
            h("p", null, encryptionLeftOnText(current.warnings) ?? "", " Turn it off now with the password you set."),
            h(
              "div",
              null,
              h("button", { class: "btn btn-sm", type: "button", "data-focus-key": "restore", onClick: () => openRestore(acqId, summary) }, "Turn backup encryption off"),
            ),
          ),
        ),
      turnedOffLater &&
        h(
          "div",
          { class: "banner banner-ok", role: "status" },
          icon("check-circle"),
          h("p", null, "Backup encryption was turned off after this acquisition. That restore is recorded in its own file in the acquisition folder; acquisition.json still lists the warnings below."),
        ),
    ]);
    const key = JSON.stringify([fin.status, r ? [r.status, r.output.seal, r.encryption, r.backup_result?.final_message ?? null] : null, kept]);
    resultBodySlot.update(key, () => {
      /** @type {[string, Node | string][]} */
      const rows = [];
      if (backupPath) rows.push(["Backup", h("span", { class: "mono break" }, backupPath)]);
      if (r) {
        rows.push(["Seal", sealText(r)]);
        rows.push(["Encryption", encryptionSummary(r)]);
        if (r.backup_result?.final_message) rows.push(["Tool said", h("span", { class: "mono" }, r.backup_result.final_message)]);
      }
      return [
        reasonList("Reasons", fin.reasons, "reasons"),
        reasonList("Warnings", fin.warnings, "warnings"),
        rows.length > 0 && h("dl", { class: "facts facts-compact" }, rows.map(([k, v]) => [h("dt", null, k), h("dd", null, v)])),
        h(
          "div",
          { class: "actions" },
          h(
            "button",
            {
              class: "btn btn-primary",
              type: "button",
              disabled: !canParse,
              title: canParse ? "Open New run with this backup as the input" : "Only a succeeded backup can be parsed",
              "data-focus-key": "parse",
              onClick: () => parse(acqId, /** @type {string} */ (backupPath)),
            },
            "Parse with iLEAPP",
          ),
          h("button", { class: "btn", type: "button", "data-focus-key": "reveal", onClick: () => reveal(summary.acq_dir) }, icon("folder"), "Reveal folder"),
          h("button", { class: "btn", type: "button", "data-focus-key": "json", onClick: () => openFile(acqId, "acquisition_json") }, "Open acquisition.json"),
          h("button", { class: "btn", type: "button", "data-focus-key": "stdout", onClick: () => openFile(acqId, "stdout") }, "Open log"),
          h("button", { class: "btn", type: "button", "data-focus-key": "stderr", onClick: () => openFile(acqId, "stderr") }, "Open error log"),
          h("button", { class: "btn btn-plain", type: "button", "data-focus-key": "new", onClick: newAcquisition }, "New acquisition"),
        ),
        canParse && kept && h("p", { class: "muted small" }, "The backup password you set is kept in memory for “Parse with iLEAPP” until you leave this result."),
      ];
    });
    resultCard.hidden = false;
  }

  /**
   * @param {string} acqId
   * @param {AcqSummary} summary
   */
  function openRestore(acqId, summary) {
    const dialog = restoreEncryptionDialog({
      api,
      casePath,
      acq: { acq_id: acqId, label: summary.label, device_name: summary.device_name, udid: summary.udid },
      windows: store.get().appInfo?.os === "windows",
      // Whatever the outcome, re-read the case's row: its warnings say whether the action still applies.
      onSettled: () => {
        if (!disposed) void reloadSummary(acqId);
      },
    });
    cleanups.push(() => dialog.close());
    dialog.open();
  }

  /**
   * Re-reads this acquisition's `AcqSummary` (`case_open`), e.g. after a later restore.
   * @param {string} acqId
   */
  async function reloadSummary(acqId) {
    try {
      const detail = await api.case_open({ path: casePath });
      if (disposed || stream?.id !== acqId) return;
      summaryNow = detail.acquisitions.find((a) => a.acq_id === acqId) ?? summaryNow;
      render();
    } catch {
      // Keep the result as it is; the Case screen shows the current state.
    }
  }

  /**
   * @param {string} acqId
   * @param {string} backupPath
   */
  function parse(acqId, backupPath) {
    // The password (if kept) moves to New run, which takes it as it opens and clears it when the
    // run starts or the form closes. Leaving this screen must not drop it first.
    unwatchHandoff = () => {};
    navigate(routeHref("new-run", { case: casePath, input: backupPath, acq: acqId }));
  }

  function newAcquisition() {
    if (stream) stream.acknowledged = true;
    unwatchHandoff();
    unwatchHandoff = () => {};
    handoff.drop();
    // The next acquisition starts from the defaults: its own label, no encryption change unless
    // ticked again, turning it off afterwards on, no "Parse with iLEAPP now", empty passwords.
    resetAcqControls(controls);
    password2Field.setError(null);
    startErrors.clear();
    showSetup();
    // The backup used disk space: check the free space again.
    if (selected) void loadPreflight();
    title.focus();
  }

  /** @param {string} path */
  async function reveal(path) {
    try {
      await api.reveal_path({ path });
    } catch (err) {
      showResultError(err, "The folder could not be revealed.");
    }
  }

  /**
   * @param {string} acqId
   * @param {AcqFile} which
   */
  async function openFile(acqId, which) {
    try {
      await api.open_acq_file({ case_path: casePath, acq_id: acqId, which });
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

  async function askCancel() {
    const phase = stream?.phase ?? null;
    const message =
      phase === "backing_up"
        ? "The backup stops. The partial backup is kept and recorded as cancelled. If the app turned encryption on, it turns it off again when you asked for that."
        : phase === "validating" || phase === "sealing" || phase === "finalizing"
          ? "The backup has already finished: cancelling stops sealing it (backup.sha256) and finalizes the record."
          : "No backup is taken. If encryption is being turned on, the app first waits for that to finish, then turns it off again when you asked for that.";
    const ok = await confirmDialog({ title: "Cancel this acquisition?", message, confirmLabel: "Cancel acquisition", cancelLabel: "Keep going", danger: true });
    if (!ok || disposed || !stream?.live || !stream.id) return;
    cancelling = true;
    progressErrors.clear();
    render();
    try {
      await api.acq_cancel({ acq_id: stream.id });
    } catch (err) {
      cancelling = false;
      if (disposed) return;
      progressErrors.show(err, "The acquisition could not be cancelled.");
      render();
    }
  }

  // ---- Store updates, elapsed time ----

  cleanups.push(
    watch(
      store,
      (s) => jobKey(s.activeJob),
      () => {
        if (view !== "setup") return;
        renderJobBanner();
        renderStart();
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
      clearPasswords();
      unwatchHandoff();
      log.dispose();
      startErrors.dispose();
      progressErrors.dispose();
      for (const fn of cleanups) fn();
    },
  };
}

function dash() {
  return h("span", { class: "muted" }, "—");
}

/**
 * A stable key for a polling error (a new but equal error must not re-render the list).
 * @param {unknown} err
 */
function toAppErrorKey(err) {
  const e = toAppError(err);
  return `${e.code}|${e.message}|${e.detail ?? ""}`;
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

/** @param {AcquisitionRecord} r */
function sealText(r) {
  const seal = r.output.seal;
  switch (seal.status) {
    case "sealed":
      return `Sealed in backup.sha256: ${plural(seal.file_count ?? 0, "file", "files")}, ${formatBytes(seal.total_bytes)}`;
    case "skipped_no_output":
      return "No backup folder, so nothing was sealed";
    default:
      return `Seal ${seal.status}`;
  }
}

/** @param {AcquisitionRecord} r */
function encryptionSummary(r) {
  const e = r.encryption;
  if (!e.enable_requested) return e.will_encrypt_before === true ? "Already on (the owner's password is needed to parse)" : "Not changed";
  if (!e.enabled_by_examiner) return "Turning it on failed; the setting is unchanged";
  switch (e.restored_after) {
    case "restored":
      return "Turned on for the backup, then off again";
    case "not_requested":
      return "Turned on for the backup and left on (as requested)";
    case "failed":
      return "Turned on for the backup; turning it off failed";
    default:
      return "Turned on for the backup; turning it off was not confirmed";
  }
}
