// @ts-check
/**
 * Settings screen (ROADMAP D4b, ARCHITECTURE.md F10):
 * - a card per parser: pinned version, `ToolState`, install (download), offline import and verify,
 *   with install progress and errors, module count and install folder;
 * - case defaults (examiner, agency, timezone);
 * - storage: the cases folder, the tools-folder override (reset = `null`), and clean temp files;
 * - about: versions and paths, the privacy statement, and the third-party licenses as preformatted
 *   text.
 *
 * Installs keep running (and their progress stays in the store) when the examiner leaves the screen.
 */
import { appError, errorSlot } from "../components/app-error.js";
import { progressBar, percentOf, stepList } from "../components/progress.js";
import { fill, h } from "../lib/dom.js";
import { toAppError } from "../lib/errors.js";
import { field, selectInput, textInput } from "../lib/form.js";
import { formatBytes, formatCount } from "../lib/format.js";
import { TOOL_STATES, applyInstallEvent, finishInstall, installSteps, stageLabel, startInstall, toolActions } from "../lib/install.js";
import { jobKey } from "../lib/jobs.js";
import { watch } from "../lib/store.js";
import { loadTimezones } from "../lib/timezones.js";
import { icon, uid } from "../lib/view.js";

/** @typedef {import("../types").InstallEvent} InstallEvent */
/** @typedef {import("../types").ToolId} ToolId */
/** @typedef {import("../types").ToolStatus} ToolStatus */
/** @typedef {import("../lib/context").AppState} AppState */
/** @typedef {import("../lib/context").ScreenContext} ScreenContext */
/** @typedef {import("../lib/context").View} View */
/** @typedef {import("../lib/install.js").InstallProgress} InstallProgress */

/**
 * @param {ScreenContext} ctx
 * @returns {View}
 */
export function settingsScreen(ctx) {
  const { api, store } = ctx;
  let disposed = false;
  /** @type {(() => void)[]} */
  const cleanups = [];
  /** Per-tool results of the last Verify, shown under the card's buttons. */
  /** @type {Partial<Record<ToolId, { ok: boolean, text: string }>>} */
  const verifyNotes = {};
  /** @type {Partial<Record<ToolId, unknown>>} */
  const toolErrors = {};
  /** @type {Set<ToolId>} */
  const verifying = new Set();

  const toolsGrid = h("div", { class: "tool-grid" });
  const defaultsBody = h("div", { class: "stack" }, h("p", { class: "muted" }, "Loading…"));
  const storageBody = h("div", { class: "stack" });
  const aboutBody = h("div", { class: "stack" });
  const body = h("div", { class: "stack", "aria-busy": "true" }, h("p", { class: "muted" }, "Loading the settings…"));

  /**
   * @param {string} id
   * @param {string} title
   * @param {(Node | string | null)[]} children
   */
  const card = (id, title, children) =>
    fill(h("section", { class: "card", "aria-labelledby": id }), h("div", { class: "card-head" }, h("h2", { id }, title)), children);

  const node = h("section", { class: "screen settings-screen" }, h("div", { class: "screen-head" }, h("h1", { tabindex: "-1" }, "Settings")), body);

  // ---- Loading ----

  async function init() {
    try {
      const [settings, tools, appInfo] = await Promise.all([api.settings_get(), api.tools_status(), api.app_info()]);
      if (disposed) return;
      store.set({ settings, tools, appInfo });
      body.replaceChildren(
        card("settings-tools", "Parsers", [
          h("p", { class: "muted small" }, "iLEAPP and aLEAPP are downloaded from their pinned GitHub releases and checked against SHA-256 values built into suiteDFIR."),
          toolsGrid,
        ]),
        card("settings-defaults", "Case defaults", [defaultsBody]),
        card("settings-storage", "Storage", [storageBody]),
        card("settings-about", "About", [aboutBody]),
      );
      renderTools();
      renderStorage();
      renderAbout();
      await renderDefaults();
    } catch (err) {
      if (disposed) return;
      body.replaceChildren(appError(err, { title: "The settings could not be loaded." }).node);
    } finally {
      body.removeAttribute("aria-busy");
    }
  }

  // ---- Parsers ----

  function renderTools() {
    const s = store.get();
    toolsGrid.replaceChildren(...(s.tools ?? []).map((t) => toolCard(t, s.installs[t.tool])));
  }

  /**
   * @param {ToolStatus} t
   * @param {InstallProgress | undefined} p
   */
  function toolCard(t, p) {
    const info = TOOL_STATES[t.state];
    const actions = toolActions(t.state);
    const busy = p?.running === true || verifying.has(t.tool);
    const headingId = uid("tool");
    /** @type {[string, Node | string][]} */
    const facts = [
      ["Pinned version", t.pinned_version],
      ["Installed version", t.installed_version ?? dash()],
      ["Installed from", sourceText(t.install_source)],
      ["Modules", t.module_count === null ? dash() : formatCount(t.module_count)],
      ["Folder", t.install_dir ? h("span", { class: "mono break" }, t.install_dir) : dash()],
    ];
    const note = verifyNotes[t.tool];
    return fill(
      h("article", { class: "tool-card", "aria-labelledby": headingId }),
      h(
        "div",
        { class: "tool-card-head" },
        h("h3", { id: headingId }, t.display_name),
        h("span", { class: `badge badge-tone-${info.tone}` }, icon(info.icon), info.label),
      ),
      h("p", { class: "small" }, info.text),
      t.problem &&
        h("div", { class: "banner banner-danger" }, icon("x-circle"), h("div", null, h("strong", null, "Problem: "), h("span", { class: "break" }, t.problem))),
      h("dl", { class: "facts facts-compact" }, facts.map(([k, v]) => [h("dt", null, k), h("dd", null, v)])),
      p && installView(t, p),
      (actions.install || actions.import || actions.verify) &&
        h(
          "div",
          { class: "actions" },
          actions.install &&
            h(
              "button",
              { class: "btn btn-primary btn-sm", type: "button", disabled: busy, onClick: () => install(t, "download") },
              t.state === "not_installed" ? `Install ${t.pinned_version}` : "Install again",
            ),
          actions.import && h("button", { class: "btn btn-sm", type: "button", disabled: busy, onClick: () => install(t, "import") }, "Import release file…"),
          actions.verify &&
            h("button", { class: "btn btn-sm", type: "button", disabled: busy, onClick: () => verify(t) }, verifying.has(t.tool) ? "Verifying…" : "Verify"),
        ),
      note && h("p", { class: note.ok ? "status-text" : "field-error", role: "status" }, note.text),
      toolErrors[t.tool] !== undefined && appError(toolErrors[t.tool], { title: "The parser could not be verified." }).node,
    );
  }

  /**
   * @param {ToolStatus} t
   * @param {InstallProgress} p
   */
  function installView(t, p) {
    const what = p.mode === "download" ? "Installing" : "Importing";
    const bar =
      p.running &&
      (p.stages[p.stages.length - 1] === "downloading" && p.download
        ? progressBar({
            label: `Downloading ${t.display_name} ${t.pinned_version}`,
            done: p.download.done,
            total: p.download.total,
            detail: `${formatBytes(p.download.done)} of ${formatBytes(p.download.total)}${pct(p.download.done, p.download.total)}`,
          })
        : progressBar({ label: `${what} ${t.display_name}`, done: null, total: null, detail: stageLabel(p.stages[p.stages.length - 1] ?? "") }));
    return h(
      "div",
      { class: "subpanel stack-sm install-progress" },
      stepList(`${what} stages`, installSteps(p), stageLabel),
      bar,
      p.messages.length > 0 && h("ul", { class: "list-compact small muted" }, p.messages.map((m) => h("li", null, m))),
      !p.running && !p.error && h("p", { class: "status-text", role: "status" }, p.mode === "download" ? "Installed and verified." : "Imported and verified."),
      p.error && appError(p.error, { title: p.mode === "download" ? "The install failed. Nothing was installed." : "The import failed. Nothing was installed." }).node,
    );
  }

  /**
   * Installs from the pinned download, or imports a release file (the same pipeline).
   * @param {ToolStatus} t
   * @param {"download" | "import"} mode
   */
  async function install(t, mode) {
    const tool = t.tool;
    delete verifyNotes[tool];
    delete toolErrors[tool];
    /** @type {string | null} */
    let archive = null;
    if (mode === "import") {
      try {
        archive = await api.dialog_open({
          title: `Import a ${t.display_name} ${t.pinned_version} release file`,
          filters: [{ name: `${t.display_name} release`, extensions: ["zip", "AppImage"] }],
        });
      } catch (err) {
        toolErrors[tool] = err;
        renderTools();
        return;
      }
      if (!archive) return;
    }
    setInstall(tool, startInstall(mode));
    /** @param {InstallEvent} event */
    const onEvent = (event) => {
      const current = store.get().installs[tool];
      if (current) setInstall(tool, applyInstallEvent(current, event));
    };
    try {
      const status =
        mode === "download" ? await api.tool_install({ tool }, onEvent) : await api.tool_import({ tool, archive_path: /** @type {string} */ (archive) }, onEvent);
      replaceTool(status);
      finish(tool, null);
    } catch (err) {
      finish(tool, err);
    }
  }

  /**
   * @param {ToolId} tool
   * @param {unknown} error
   */
  function finish(tool, error) {
    const current = store.get().installs[tool];
    if (current) setInstall(tool, finishInstall(current, error === null ? null : toAppError(error)));
  }

  /**
   * @param {ToolId} tool
   * @param {InstallProgress} progress
   */
  function setInstall(tool, progress) {
    store.set({ installs: { ...store.get().installs, [tool]: progress } });
  }

  /** @param {ToolStatus} status */
  function replaceTool(status) {
    const tools = store.get().tools ?? [];
    store.set({ tools: tools.map((t) => (t.tool === status.tool ? status : t)) });
  }

  /** @param {ToolStatus} t */
  async function verify(t) {
    verifying.add(t.tool);
    delete verifyNotes[t.tool];
    delete toolErrors[t.tool];
    renderTools();
    try {
      const status = await api.tool_verify({ tool: t.tool });
      verifyNotes[t.tool] =
        status.state === "verified"
          ? { ok: true, text: "Verified: the binary matches the pinned SHA-256." }
          : { ok: false, text: `Not verified: ${TOOL_STATES[status.state].label.toLowerCase()}.` };
      verifying.delete(t.tool);
      replaceTool(status);
    } catch (err) {
      toolErrors[t.tool] = err;
      verifying.delete(t.tool);
    }
    if (!disposed) renderTools();
  }

  // ---- Case defaults ----

  async function renderDefaults() {
    const settings = store.get().settings;
    if (!settings) return;
    const zones = await loadTimezones(api, store);
    if (disposed) return;
    const d = settings.defaults;
    const list = d.timezone && !zones.includes(d.timezone) ? [d.timezone, ...zones] : zones;
    const examiner = textInput({ name: "examiner", value: d.examiner, autocomplete: "off" });
    const agency = textInput({ name: "agency", value: d.agency, autocomplete: "off" });
    const timezone = selectInput(
      list.map((z) => ({ value: z, label: z })),
      d.timezone || "UTC",
      { name: "timezone" },
    );
    const status = h("p", { class: "status-text", role: "status" });
    const errors = errorSlot();
    const save = /** @type {HTMLButtonElement} */ (h("button", { class: "btn btn-primary", type: "submit" }, "Save defaults"));
    const form = h(
      "form",
      {
        class: "form",
        novalidate: true,
        onSubmit: async (/** @type {SubmitEvent} */ event) => {
          event.preventDefault();
          status.textContent = "";
          errors.clear();
          save.disabled = true;
          try {
            const next = await api.settings_update({ defaults: { examiner: examiner.value.trim(), agency: agency.value.trim(), timezone: timezone.value } });
            if (disposed) return;
            store.set({ settings: next });
            status.textContent = "Saved. New cases and runs use these defaults.";
          } catch (err) {
            errors.show(err, "The defaults could not be saved.");
          } finally {
            save.disabled = false;
          }
        },
      },
      h(
        "div",
        { class: "form-row" },
        field({ label: "Examiner", control: examiner, hint: "Pre-filled in new cases." }).node,
        field({ label: "Agency", control: agency, hint: "Pre-filled in new cases." }).node,
        field({ label: "Timezone", control: timezone, hint: "For iLEAPP runs when the case has none." }).node,
      ),
      errors.node,
      h("div", { class: "form-actions form-actions-start" }, save, status),
    );
    cleanups.push(() => errors.dispose());
    defaultsBody.replaceChildren(form);
  }

  // ---- Storage: cases folder, tools folder, temp files ----

  const storageErrors = errorSlot();
  const storageStatus = h("p", { class: "status-text", role: "status" });
  let storageBusy = false;

  function renderStorage() {
    const s = store.get();
    const settings = s.settings;
    if (!settings) return;
    const override = settings.tools_dir;
    const toolsDir = override ?? s.appInfo?.paths.tools_dir ?? "";
    const installing = Object.values(s.installs).some((p) => p?.running);
    const jobActive = s.activeJob !== null;
    storageBody.replaceChildren(
      folderRow({
        label: "Cases folder",
        path: settings.cases_root,
        tag: null,
        hint: "New cases are created here unless you choose another location.",
        buttons: [h("button", { class: "btn btn-sm", type: "button", disabled: storageBusy, onClick: changeCasesRoot }, "Change…")],
      }),
      folderRow({
        label: "Tools folder",
        path: toolsDir,
        tag: override === null ? "Default" : "Custom",
        hint:
          "Parsers are installed in and run from this folder. On machines where AppLocker or WDAC allow programs only from approved folders, choose an approved one. It may not be inside a case folder. Parsers already installed elsewhere are not moved.",
        buttons: [
          h("button", { class: "btn btn-sm", type: "button", disabled: storageBusy || installing, onClick: changeToolsDir }, "Change…"),
          override !== null &&
            h("button", { class: "btn btn-sm", type: "button", disabled: storageBusy || installing, onClick: () => setToolsDir(null) }, "Use the default"),
        ],
      }),
      h(
        "div",
        { class: "field" },
        h("span", { class: "field-label" }, "Temporary files"),
        h(
          "p",
          { class: "field-hint" },
          "Each run and acquisition uses its own temporary folder (a parser run unpacks about 130 MB), deleted when the job ends. Leftovers from a crash are removed at start-up; clean them now if disk space is short.",
        ),
        h(
          "div",
          { class: "inline-row" },
          h("button", { class: "btn btn-sm", type: "button", disabled: storageBusy || jobActive, onClick: cleanTemp }, "Clean temporary files"),
          jobActive && h("span", { class: "muted small" }, "Not while a job is running."),
        ),
      ),
      storageErrors.node,
      storageStatus,
    );
  }

  /**
   * @param {{ label: string, path: string, tag: string | null, hint: string, buttons: (Node | false | null)[] }} spec
   */
  function folderRow(spec) {
    const labelId = uid("folder");
    return h(
      "div",
      { class: "field" },
      h("span", { class: "field-label", id: labelId }, spec.label),
      h(
        "div",
        { class: "inline-row", role: "group", "aria-labelledby": labelId },
        h("span", { class: "mono break folder-path" }, spec.path),
        spec.tag && h("span", { class: "tag" }, spec.tag),
        spec.buttons,
      ),
      h("p", { class: "field-hint" }, spec.hint),
    );
  }

  /** @param {() => Promise<string>} action */
  async function storageAction(action) {
    storageErrors.clear();
    storageStatus.textContent = "";
    storageBusy = true;
    renderStorage();
    try {
      const text = await action();
      if (!disposed) storageStatus.textContent = text;
    } catch (err) {
      if (!disposed) storageErrors.show(err, "The change could not be made.");
    } finally {
      storageBusy = false;
      if (!disposed) renderStorage();
    }
  }

  async function changeCasesRoot() {
    const current = store.get().settings?.cases_root;
    const picked = await pick("Choose the cases folder", current);
    if (!picked) return;
    await storageAction(async () => {
      const next = await api.settings_update({ cases_root: picked });
      store.set({ settings: next });
      return "The cases folder was changed.";
    });
  }

  async function changeToolsDir() {
    const s = store.get();
    const picked = await pick("Choose the tools folder", s.settings?.tools_dir ?? s.appInfo?.paths.tools_dir);
    if (picked) await setToolsDir(picked);
  }

  /** @param {string | null} dir `null` = the default folder */
  async function setToolsDir(dir) {
    await storageAction(async () => {
      const next = await api.settings_update({ tools_dir: dir });
      // Which parsers are installed depends on the folder.
      const [tools, appInfo] = await Promise.all([api.tools_status(), api.app_info()]);
      store.set({ settings: next, tools, appInfo });
      return dir === null ? "The tools folder is the default again." : "The tools folder was changed. Install or import the parsers there if needed.";
    });
  }

  /**
   * @param {string} title
   * @param {string | undefined} defaultPath
   * @returns {Promise<string | null>}
   */
  async function pick(title, defaultPath) {
    storageErrors.clear();
    try {
      return await api.dialog_open({ title, directory: true, defaultPath });
    } catch (err) {
      storageErrors.show(err);
      return null;
    }
  }

  async function cleanTemp() {
    await storageAction(async () => {
      const { freed_bytes } = await api.temp_cleanup();
      return freed_bytes > 0 ? `Removed ${formatBytes(freed_bytes)} of temporary files.` : "There were no temporary files to remove.";
    });
  }

  // ---- About ----

  function renderAbout() {
    const info = store.get().appInfo;
    const tools = store.get().tools ?? [];
    if (!info) return;
    /** @type {[string, Node | string][]} */
    const facts = [
      ["suiteDFIR", info.app_version],
      ["Platform", info.platform ?? `${info.os} ${info.arch} (no pinned parser builds)`],
      ...tools.map((t) => /** @type {[string, string]} */ ([`${t.display_name} (pinned)`, t.pinned_version])),
      ["App data", h("span", { class: "mono break" }, info.paths.app_data)],
      ["Settings", h("span", { class: "mono break" }, info.paths.app_config)],
      ["Cache", h("span", { class: "mono break" }, info.paths.app_cache)],
      ["App log", h("span", { class: "mono break" }, info.paths.app_log)],
    ];
    const licenses = h("div", { class: "licenses-body" });
    let loaded = false;
    const details = /** @type {HTMLDetailsElement} */ (
      h("details", { class: "licenses" }, h("summary", null, "Third-party licenses"), licenses)
    );
    details.addEventListener("toggle", async () => {
      if (!details.open || loaded) return;
      loaded = true;
      licenses.replaceChildren(h("p", { class: "muted", role: "status" }, "Loading…"));
      try {
        const text = await api.licenses_get();
        if (!disposed) licenses.replaceChildren(h("pre", { class: "pre licenses-text" }, text));
      } catch (err) {
        loaded = false;
        if (!disposed) licenses.replaceChildren(appError(err, { title: "The licenses could not be loaded." }).node);
      }
    });
    aboutBody.replaceChildren(
      h("dl", { class: "facts facts-compact" }, facts.map(([k, v]) => [h("dt", null, k), h("dd", null, v)])),
      h(
        "div",
        { class: "stack-sm" },
        h("h3", null, "Privacy"),
        h(
          "p",
          null,
          "suiteDFIR has no telemetry, no accounts and no update checks. It uses the network only when you install a parser, to download its pinned release from GitHub. Cases, evidence, reports and logs stay on this computer, in the folders you choose.",
        ),
        h(
          "p",
          null,
          "Backup passwords are never stored or logged. They are held in memory only while a run or an acquisition needs them, and the password fields are cleared after use.",
        ),
      ),
      details,
    );
  }

  // ---- Store updates ----

  cleanups.push(
    watch(store, (s) => s.tools, () => {
      if (!disposed && store.get().settings) renderTools();
    }),
    watch(store, (s) => s.installs, () => {
      if (!disposed && store.get().settings) {
        renderTools();
        renderStorage();
      }
    }),
    watch(store, (s) => jobKey(s.activeJob) === null, () => {
      if (!disposed && store.get().settings) renderStorage();
    }),
  );

  void init();
  return {
    node,
    dispose() {
      disposed = true;
      storageErrors.dispose();
      for (const fn of cleanups) fn();
    },
  };
}

function dash() {
  return h("span", { class: "muted" }, "—");
}

/**
 * @param {number} done
 * @param {number} total
 */
function pct(done, total) {
  const p = percentOf(done, total);
  return p === null ? "" : ` (${p}%)`;
}

/** @param {ToolStatus["install_source"]} source */
function sourceText(source) {
  switch (source) {
    case "download":
      return "Download";
    case "offline_import":
      return "Offline import";
    case "dev_override":
      return "Dev override";
    default:
      return dash();
  }
}
