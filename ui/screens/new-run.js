// @ts-check
/**
 * New run screen (ROADMAP D3). Sections in order: Tool (installed tools only), Input (choose a file
 * or folder, the inspection result, a type override limited to `allowed_types`, inline overlap and
 * permission errors), Options (timezone, backup password, keychain, input hashing, label), Modules
 * (all / profile / custom with the module picker; save, import and export profiles), then Start,
 * which stays disabled with inline reasons until the form is complete.
 */
import { appError, errorSlot } from "../components/app-error.js";
import { confirmDialog, modal } from "../components/dialog.js";
import { modulePicker } from "../components/module-picker.js";
import { folderLabel } from "../lib/cases.js";
import { h, keepFocus } from "../lib/dom.js";
import { isAppError } from "../lib/errors.js";
import { field, selectInput, textInput } from "../lib/form.js";
import { formatCount, plural } from "../lib/format.js";
import { handoff } from "../lib/handoff.js";
import { buildRunRequest, canHash, initialInputType, needsPassword, passwordHint, passwordReason, startBlockers } from "../lib/newrun.js";
import { routeHref } from "../lib/router.js";
import { jobKey, setActiveJob } from "../lib/jobs.js";
import { unknownNames } from "../lib/selection.js";
import { watch } from "../lib/store.js";
import { pickTimezone, rememberToolTimezones, timezoneList } from "../lib/timezones.js";
import { TOOL_FEATURES, installedTools } from "../lib/tools.js";
import { icon, inputTypeLabel, sizeText, toolName, uid } from "../lib/view.js";

/** @typedef {import("../types").CaseFile} CaseFile */
/** @typedef {import("../types").InputType} InputType */
/** @typedef {import("../types").ModuleMode} ModuleMode */
/** @typedef {import("../types").ProfileInfo} ProfileInfo */
/** @typedef {import("../types").ToolId} ToolId */
/** @typedef {import("../types").ToolModules} ToolModules */
/** @typedef {import("../types").ToolStatus} ToolStatus */
/** @typedef {import("../lib/newrun.js").NewRunForm} NewRunForm */
/** @typedef {import("../components/module-picker.js").ModulePicker} ModulePicker */
/** @typedef {import("../lib/context").ScreenContext} ScreenContext */
/** @typedef {import("../lib/context").View} View */

/**
 * @param {ScreenContext} ctx
 * @returns {View}
 */
export function newRunScreen(ctx) {
  const { api, store, navigate, jobs } = ctx;
  const casePath = ctx.params.case ?? "";
  const caseHref = routeHref("case", { path: casePath });
  // "Parse with iLEAPP" (D5): the acquired backup as the input, and the backup password the examiner
  // set, if the Acquire screen kept it. Taken at once, so it lives only in this form from now on.
  const handedInput = ctx.params.input ?? null;
  const handedAcq = ctx.params.acq ?? null;
  /** @type {string | null} */
  let handedPassword = handedAcq ? handoff.take(handedAcq) : null;
  const passwordHanded = handedPassword !== null;
  let disposed = false;
  /** @type {(() => void)[]} */
  const cleanups = [];

  /** @type {NewRunForm} */
  const f = {
    tool: null,
    toolsInstalled: false,
    inputPath: null,
    inspection: null,
    inspecting: false,
    inputFailed: false,
    inputType: null,
    timezone: null,
    password: "",
    keychainPath: null,
    hashInput: true,
    label: "",
    modulesLoaded: false,
    moduleMode: "all",
    profile: null,
    customModules: [],
    customUnknown: [],
    jobActive: store.get().activeJob !== null,
  };
  /** @type {ToolStatus[]} */
  let installed = [];
  /** @type {ToolStatus[]} */
  let allTools = [];
  /** @type {CaseFile | null} */
  let caseFile = null;
  /** @type {string[]} */
  let timezones = [];
  /** @type {ToolModules | null} */
  let toolModules = null;
  /** @type {unknown} */
  let modulesError = null;
  /** @type {ProfileInfo[]} */
  let profiles = [];
  /** @type {ModulePicker | null} */
  let picker = null;
  /** @type {unknown} */
  let inputError = null;
  let inspectSeq = 0;
  let starting = false;

  // ---- Persistent controls (their values survive re-rendering the sections) ----

  const tzSelect = /** @type {HTMLSelectElement} */ (h("select", { class: "input", name: "timezone" }));
  tzSelect.addEventListener("change", () => {
    f.timezone = tzSelect.value || null;
    refresh();
  });
  const password = /** @type {HTMLInputElement} */ (
    h("input", { class: "input", type: "password", name: "itunes_password", autocomplete: "off", spellcheck: "false" })
  );
  password.addEventListener("input", () => {
    f.password = password.value;
    refresh();
  });
  const passwordField = field({
    label: "Backup password",
    control: password,
    required: true,
    hint: passwordHint("encrypted"),
  });
  // The inline reason follows the inspection (encrypted, or its encryption could not be read).
  const passwordHintNode = passwordField.node.querySelector(".field-hint");
  const hashBox = /** @type {HTMLInputElement} */ (h("input", { type: "checkbox", name: "hash_input" }));
  hashBox.checked = true;
  hashBox.addEventListener("change", () => {
    f.hashInput = hashBox.checked;
  });
  const labelInput = textInput({ name: "label", autocomplete: "off", maxlength: 200 });
  labelInput.addEventListener("input", () => {
    f.label = labelInput.value;
  });

  // ---- Layout ----

  const caseName = h("a", { href: caseHref }, folderLabel(casePath));
  const toolBody = h("div", { class: "stack" });
  const inputBody = h("div", { class: "stack" });
  const optionsBody = h("div", { class: "stack" });
  const modulesBody = h("div", { class: "stack" });
  const modulesErrors = errorSlot();
  const modulesStatus = h("p", { class: "status-text", role: "status" });
  const reasonsId = uid("start-reasons");
  const reasons = h("div", { class: "start-reasons", id: reasonsId });
  const startErrors = errorSlot();
  const saveProfileButton = /** @type {HTMLButtonElement} */ (
    h("button", { class: "btn btn-sm", type: "button", onClick: saveProfile }, "Save as profile…")
  );
  // Start is a plain button, not a submit button: the form then has no default button, so Enter in
  // a field (the module search, the label, the password) can never start a run.
  const startButton = /** @type {HTMLButtonElement} */ (
    h("button", { class: "btn btn-primary btn-lg", type: "button", "aria-describedby": reasonsId, disabled: true, onClick: start }, "Start run")
  );

  /**
   * @param {number} step
   * @param {string} title
   * @param {Node[]} children
   */
  const section = (step, title, children) => {
    const id = uid("section");
    return h(
      "section",
      { class: "card form-section", "aria-labelledby": id },
      h("h2", { class: "card-title", id }, h("span", { class: "step", "aria-hidden": "true" }, String(step)), title),
      children,
    );
  };

  const form = h(
    "form",
    { class: "form new-run-form", novalidate: true, onSubmit: ignoreSubmit },
    section(1, "Tool", [toolBody]),
    section(2, "Input", [inputBody]),
    section(3, "Options", [optionsBody]),
    section(4, "Modules", [modulesErrors.node, modulesStatus, modulesBody]),
    h(
      "section",
      { class: "card start-card", "aria-label": "Start" },
      reasons,
      startErrors.node,
      h("div", { class: "form-actions" }, h("a", { class: "btn", href: caseHref }, "Cancel"), startButton),
    ),
  );
  const body = h("div", { class: "stack", "aria-busy": "true" }, h("p", { class: "muted" }, "Loading…"));
  const node = h(
    "section",
    { class: "screen" },
    h(
      "nav",
      { class: "breadcrumb", "aria-label": "Breadcrumb" },
      h("a", { href: routeHref("cases") }, "Cases"),
      h("span", { "aria-hidden": "true" }, " / "),
      caseName,
      h("span", { "aria-hidden": "true" }, " / "),
    ),
    h("div", { class: "screen-head" }, h("h1", { tabindex: "-1" }, "New run")),
    body,
  );

  // ---- Loading ----

  async function init() {
    try {
      const [cases, tools] = await Promise.all([api.cases_list(), api.tools_status()]);
      if (disposed) return;
      store.set({ tools });
      const summary = cases.find((c) => c.path === casePath);
      if (!summary?.case) {
        throw { code: "case_not_found", message: "This case is not in the recent list, or its folder is missing.", detail: casePath };
      }
      caseFile = summary.case;
      caseName.textContent = caseFile.name;
      allTools = tools;
      installed = installedTools(tools);
      f.toolsInstalled = installed.length > 0;
      f.tool = installed.find((t) => t.tool === "ileapp")?.tool ?? installed[0]?.tool ?? null;
      if (handedInput && f.tool) f.inputPath = handedInput;
      body.replaceChildren(form);
      renderTool();
      renderInput();
      renderOptions();
      renderModules();
      refresh();
      if (f.inputPath) void inspectInput();
      await loadToolData();
    } catch (err) {
      if (disposed) return;
      body.replaceChildren(appError(err, { title: "The New run form could not be loaded." }).node, h("p", null, h("a", { href: caseHref }, "Back to the case")));
    } finally {
      body.removeAttribute("aria-busy");
    }
  }

  async function loadToolData() {
    const tool = f.tool;
    picker?.dispose();
    picker = null;
    toolModules = null;
    modulesError = null;
    profiles = [];
    f.modulesLoaded = false;
    f.profile = null;
    f.customModules = [];
    f.customUnknown = [];
    renderModules();
    refresh();
    if (!tool) return;
    try {
      const [mods, profs] = await Promise.all([api.tool_modules({ tool }), api.profiles_list({ tool })]);
      if (disposed || f.tool !== tool) return;
      toolModules = mods;
      profiles = profs;
      f.modulesLoaded = true;
      if (TOOL_FEATURES[tool].timezone) {
        // The timezone must be one iLEAPP itself accepts (D19), so use its own list.
        rememberToolTimezones(store, mods.version, mods.timezones);
        applyTimezones(timezoneList(mods.timezones));
      }
    } catch (err) {
      if (disposed || f.tool !== tool) return;
      modulesError = err;
    }
    renderOptions();
    renderModules();
    refresh();
  }

  /**
   * Shows `list` in the timezone select. Keeps the examiner's choice if it is still in the list;
   * otherwise picks the case's zone, then the app's, then UTC.
   * @param {string[]} list
   */
  function applyTimezones(list) {
    timezones = list;
    const kept = f.timezone && list.includes(f.timezone) ? f.timezone : null;
    f.timezone = kept ?? pickTimezone(list, [caseFile?.default_timezone, store.get().settings?.defaults.timezone, "UTC"]);
    tzSelect.replaceChildren(...list.map((z) => h("option", { value: z }, z)));
    tzSelect.value = f.timezone ?? "";
  }

  // ---- 1. Tool ----

  function renderTool() {
    if (!f.toolsInstalled) {
      toolBody.replaceChildren(
        h(
          "div",
          { class: "banner banner-warn", role: "status" },
          icon("alert-triangle"),
          h("div", null, h("strong", null, "No parser is installed. "), "Install iLEAPP or aLEAPP in ", h("a", { href: routeHref("settings") }, "Settings"), "."),
        ),
      );
      return;
    }
    const missing = allTools.filter((t) => !installed.includes(t));
    toolBody.replaceChildren(
      h(
        "fieldset",
        { class: "choice-group" },
        h("legend", { class: "visually-hidden" }, "Tool"),
        installed.map((t) => {
          const radio = /** @type {HTMLInputElement} */ (h("input", { type: "radio", name: "tool", value: t.tool }));
          radio.checked = t.tool === f.tool;
          radio.addEventListener("change", () => selectTool(t.tool));
          return h(
            "label",
            { class: "choice" },
            radio,
            h(
              "span",
              { class: "choice-text" },
              h("span", { class: "choice-title" }, t.display_name),
              h(
                "span",
                { class: "muted small" },
                [t.installed_version ?? t.pinned_version, stateText(t), t.module_count !== null ? plural(t.module_count, "module", "modules") : null]
                  .filter(Boolean)
                  .join(" · "),
              ),
            ),
          );
        }),
      ),
      missing.length > 0
        ? h(
            "p",
            { class: "muted small" },
            `Not installed: ${missing.map((t) => t.display_name).join(", ")}. `,
            h("a", { href: routeHref("settings") }, "Install in Settings"),
            ".",
          )
        : "",
    );
  }

  /** @param {ToolId} tool */
  function selectTool(tool) {
    if (f.tool === tool) return;
    f.tool = tool;
    if (f.inputPath) void inspectInput();
    renderOptions();
    void loadToolData();
  }

  // ---- 2. Input ----

  function renderInput() {
    const pickers = h(
      "div",
      { class: "inline-row" },
      h("button", { class: "btn", type: "button", disabled: !f.tool, onClick: () => chooseInput(false) }, "Choose file…"),
      h("button", { class: "btn", type: "button", disabled: !f.tool, onClick: () => chooseInput(true) }, "Choose folder…"),
      h("span", { class: "muted small" }, "Inputs are only read, never changed."),
    );
    inputBody.replaceChildren(pickers, inputResult());
  }

  function inputResult() {
    if (!f.inputPath) return h("p", { class: "muted" }, "No input chosen.");
    const pathLine =
      f.inputPath === handedInput && handedAcq
        ? h(
            "div",
            { class: "stack-sm" },
            h("p", { class: "mono break input-path" }, f.inputPath),
            h(
              "p",
              { class: "muted small" },
              "The backup of acquisition ",
              h("span", { class: "mono" }, handedAcq),
              passwordHanded && f.password !== "" ? ". The backup password set during the acquisition is filled in." : ".",
            ),
          )
        : h("p", { class: "mono break input-path" }, f.inputPath);
    if (f.inspecting) return h("div", { class: "stack-sm" }, pathLine, h("p", { class: "muted", role: "status" }, "Checking the input…"));
    if (f.inputFailed || !f.inspection) {
      return h("div", { class: "stack-sm" }, pathLine, appError(inputError, { title: "This input cannot be used." }).node);
    }
    const insp = f.inspection;
    /** @type {[string, Node | string][]} */
    const facts = [
      ["Kind", insp.kind === "file" ? "File" : "Folder"],
      ...(insp.kind === "file" ? /** @type {[string, Node][]} */ ([["Size", sizeText(insp.size_bytes)]]) : []),
      ["Detected type", insp.detected_type ? inputTypeLabel(insp.detected_type) : "Not detected"],
    ];
    if (insp.is_itunes_backup) {
      facts.push([
        "iTunes/Finder backup",
        insp.itunes_encrypted === true ? "Yes, encrypted" : insp.itunes_encrypted === false ? "Yes, not encrypted" : "Yes, encryption unknown",
      ]);
    }
    const typeSelect = selectInput(
      [
        ...(f.inputType ? [] : [{ value: "", label: "Choose a type…" }]),
        ...insp.allowed_types.map((t) => ({ value: t, label: inputTypeLabel(t) })),
      ],
      f.inputType ?? "",
      { name: "input_type" },
    );
    typeSelect.addEventListener("change", () => {
      f.inputType = /** @type {InputType} */ (typeSelect.value) || null;
      renderInput();
      // The password field depends on the type (only an iTunes read takes one).
      renderOptions();
      refresh();
    });
    return h(
      "div",
      { class: "stack-sm" },
      pathLine,
      h("dl", { class: "facts facts-compact" }, facts.map(([k, v]) => [h("dt", null, k), h("dd", null, v)])),
      insp.warnings.length > 0 &&
        h("div", { class: "banner banner-warn" }, icon("alert-triangle"), h("ul", { class: "list-compact" }, insp.warnings.map((w) => h("li", null, w)))),
      field({
        label: "Input type",
        control: typeSelect,
        hint:
          insp.allowed_types.length > 1
            ? "Override the detected type only if you know it is wrong. Only types valid for this input and tool are offered."
            : "The only type valid for this input and tool.",
      }).node,
    );
  }

  /** @param {boolean} directory */
  async function chooseInput(directory) {
    startErrors.clear();
    try {
      const path = await api.dialog_open({ title: directory ? "Choose the input folder" : "Choose the input file", directory });
      if (!path || disposed) return;
      f.inputPath = path;
      await inspectInput();
    } catch (err) {
      startErrors.show(err);
    }
  }

  async function inspectInput() {
    const tool = f.tool;
    const path = f.inputPath;
    if (!tool || !path) return;
    const seq = ++inspectSeq;
    f.inspecting = true;
    f.inspection = null;
    f.inputFailed = false;
    f.inputType = null;
    inputError = null;
    renderInput();
    renderOptions();
    refresh();
    try {
      const insp = await api.input_inspect({ tool, path, case_path: casePath });
      if (disposed || seq !== inspectSeq) return;
      f.inspection = insp;
      f.inputType = initialInputType(insp);
      f.hashInput = true;
      hashBox.checked = true;
      if (path === handedInput) {
        // An acquired backup is an iTunes-format backup (CONTRACTS.md §13.5).
        if (insp.allowed_types.includes("itunes")) f.inputType = "itunes";
        if (handedPassword !== null && needsPassword(f)) {
          password.value = handedPassword;
          f.password = handedPassword;
        }
        handedPassword = null;
      }
    } catch (err) {
      if (disposed || seq !== inspectSeq) return;
      f.inputFailed = true;
      inputError = err;
      if (path === handedInput) handedPassword = null;
    }
    f.inspecting = false;
    renderInput();
    renderOptions();
    refresh();
  }

  // ---- 3. Options ----

  function renderOptions() {
    // Re-rendering moves the persistent controls, which drops their focus: an examiner typing the
    // password or the label while the tool data loads keeps the focus and the caret.
    keepFocus(optionsBody, renderOptionsParts);
  }

  function renderOptionsParts() {
    const features = f.tool ? TOOL_FEATURES[f.tool] : null;
    /** @type {Node[]} */
    const parts = [];
    if (features?.timezone && timezones.length === 0) {
      parts.push(
        h(
          "p",
          { class: "muted small", role: "status" },
          modulesError ? `${toolName(f.tool ?? "ileapp")}'s timezone list could not be loaded (see Modules).` : `Loading ${toolName(f.tool ?? "ileapp")}'s timezone list…`,
        ),
      );
    } else if (features?.timezone) {
      parts.push(
        field({
          label: "Timezone",
          control: tzSelect,
          hint: "Passed to iLEAPP with -tz. Defaults to the case's timezone, then the app's, then UTC.",
        }).node,
      );
    } else if (f.tool) {
      parts.push(h("p", { class: "muted small" }, `${toolName(f.tool)} has no timezone option; the run record notes this.`));
    }
    const reason = passwordReason(f);
    if (reason) {
      if (passwordHintNode) passwordHintNode.textContent = passwordHint(reason);
      parts.push(passwordField.node);
    } else if (password.value !== "") {
      password.value = "";
      f.password = "";
    }
    if (features?.keychain) parts.push(keychainBlock());
    if (canHash(f)) {
      parts.push(
        h(
          "div",
          { class: "field" },
          h("label", { class: "check" }, hashBox, h("span", null, "Hash the input file (SHA-256)")),
          h("p", { class: "field-hint" }, "Runs alongside the parser; the hash is recorded in run.json."),
        ),
      );
    }
    parts.push(field({ label: "Label", control: labelInput, hint: "Optional. Shown in the runs table and recorded in run.json." }).node);
    optionsBody.replaceChildren(...parts);
  }

  function keychainBlock() {
    const labelId = uid("keychain");
    return h(
      "div",
      { class: "field" },
      h("span", { class: "field-label", id: labelId }, "Keychain file (optional)"),
      h(
        "div",
        { class: "inline-row", role: "group", "aria-labelledby": labelId },
        f.keychainPath ? h("span", { class: "mono break" }, f.keychainPath) : h("span", { class: "muted" }, "None"),
        h("button", { class: "btn btn-sm", type: "button", onClick: chooseKeychain }, "Choose…"),
        f.keychainPath &&
          h(
            "button",
            {
              class: "btn btn-sm btn-plain",
              type: "button",
              onClick: () => {
                f.keychainPath = null;
                renderOptions();
              },
            },
            "Clear",
          ),
      ),
      h("p", { class: "field-hint" }, "A keychain captured separately from the extraction (iLEAPP --keychain). It is hashed and recorded."),
    );
  }

  async function chooseKeychain() {
    try {
      const path = await api.dialog_open({ title: "Choose the keychain file", directory: false });
      if (!path || disposed) return;
      f.keychainPath = path;
      renderOptions();
    } catch (err) {
      startErrors.show(err);
    }
  }

  // ---- 4. Modules ----

  function renderModules() {
    if (!f.tool) {
      modulesBody.replaceChildren(h("p", { class: "muted" }, "Choose a tool first."));
      return;
    }
    if (modulesError) {
      modulesBody.replaceChildren(
        appError(modulesError, { title: "The module list could not be loaded." }).node,
        h("div", null, h("button", { class: "btn btn-sm", type: "button", onClick: () => loadToolData() }, "Try again")),
      );
      return;
    }
    if (!f.modulesLoaded || !toolModules) {
      modulesBody.replaceChildren(h("p", { class: "muted", role: "status" }, "Loading the module list…"));
      return;
    }
    const total = toolModules.modules.length;
    /** @type {[ModuleMode, string, string][]} */
    const modes = [
      ["all", `All modules (${formatCount(total)})`, "Runs every module of the installed version."],
      ["profile", "Saved profile", profiles.length ? `${profiles.length} saved for ${toolName(f.tool)}.` : "None saved yet."],
      ["custom", "Custom selection", "Pick modules by category or search."],
    ];
    const group = h(
      "fieldset",
      { class: "choice-group choice-group-row" },
      h("legend", { class: "visually-hidden" }, "Module selection"),
      modes.map(([mode, title, hint]) => {
        const radio = /** @type {HTMLInputElement} */ (h("input", { type: "radio", name: "module_mode", value: mode }));
        radio.checked = f.moduleMode === mode;
        radio.addEventListener("change", () => setMode(mode));
        return h(
          "label",
          { class: "choice" },
          radio,
          h("span", { class: "choice-text" }, h("span", { class: "choice-title" }, title), h("span", { class: "muted small" }, hint)),
        );
      }),
    );
    modulesBody.replaceChildren(group, f.moduleMode === "profile" ? profilePanel() : f.moduleMode === "custom" ? customPanel() : "");
  }

  /** @param {ModuleMode} mode */
  function setMode(mode) {
    f.moduleMode = mode;
    if (mode === "profile" && !f.profile && profiles.length) f.profile = profiles[0];
    modulesStatus.textContent = "";
    renderModules();
    refresh();
  }

  function profilePanel() {
    const tool = f.tool;
    if (!tool) return "";
    const importButton = h("button", { class: "btn btn-sm", type: "button", onClick: importProfile }, "Import…");
    if (profiles.length === 0) {
      return h(
        "div",
        { class: "subpanel" },
        h("p", { class: "muted" }, `No saved ${toolName(tool)} profiles yet. Import one, or save a custom selection as a profile.`),
        h("div", { class: "inline-row" }, importButton),
      );
    }
    const select = selectInput(
      profiles.map((p) => ({ value: p.name, label: `${p.name} (${plural(p.modules.length, "module", "modules")})` })),
      f.profile?.name ?? profiles[0].name,
      { name: "profile" },
    );
    select.addEventListener("change", () => {
      f.profile = profiles.find((p) => p.name === select.value) ?? null;
      renderModules();
      refresh();
    });
    const profile = f.profile;
    return h(
      "div",
      { class: "subpanel stack-sm" },
      h(
        "div",
        { class: "inline-row inline-row-end" },
        field({ label: "Profile", control: select, className: "field-grow" }).node,
        importButton,
        h("button", { class: "btn btn-sm", type: "button", disabled: !profile, onClick: exportProfile }, "Export…"),
      ),
      profile && profile.unknown_modules.length > 0
        ? h(
            "div",
            { class: "banner banner-warn" },
            icon("alert-triangle"),
            h(
              "div",
              { class: "stack-sm" },
              h(
                "p",
                null,
                h("strong", null, profile.unknown_modules.length === 1 ? "1 of its modules is unknown" : `${profile.unknown_modules.length} of its modules are unknown`),
                ` to ${toolName(tool)} ${toolModules?.version ?? ""}. LEAPP would silently skip ${profile.unknown_modules.length === 1 ? "it" : "them"}, so the run is blocked.`,
              ),
              h("ul", { class: "list-compact mono" }, profile.unknown_modules.map((n) => h("li", null, n))),
              h("div", null, h("button", { class: "btn btn-sm", type: "button", onClick: () => editAsCustom(profile) }, "Edit as custom selection")),
            ),
          )
        : profile && h("p", { class: "muted small" }, `${plural(profile.modules.length, "module", "modules")}, all known to ${toolName(tool)} ${toolModules?.version ?? ""}.`),
    );
  }

  /** @param {ProfileInfo} profile */
  function editAsCustom(profile) {
    f.moduleMode = "custom";
    f.customModules = [...profile.modules];
    f.customUnknown = [...profile.unknown_modules];
    picker?.setSelected(f.customModules);
    modulesStatus.textContent = `Loaded “${profile.name}” as a custom selection.`;
    renderModules();
    refresh();
  }

  function customPanel() {
    if (!toolModules || !f.tool) return "";
    if (!picker) {
      const known = new Set(toolModules.modules.map((m) => m.name));
      picker = modulePicker({
        modules: toolModules.modules,
        selected: f.customModules,
        toolLabel: `${toolName(f.tool)} ${toolModules.version}`,
        onChange: (names) => {
          f.customModules = names;
          f.customUnknown = unknownNames(names, known);
          refresh();
        },
      });
    }
    return h(
      "div",
      { class: "subpanel stack-sm" },
      picker.node,
      h("div", { class: "inline-row" }, saveProfileButton, h("span", { class: "muted small" }, "Profiles cannot contain unknown modules.")),
    );
  }

  function saveProfile() {
    const tool = f.tool;
    if (!tool) return;
    const nameInput = textInput({ maxlength: 80, autocomplete: "off" });
    const nameField = field({ label: "Profile name", control: nameInput, required: true, hint: "1–80 characters. A profile with the same name is replaced." });
    const errors = errorSlot();
    const dialog = modal({
      title: "Save as profile",
      onClose: () => errors.dispose(),
      content: () =>
        h(
          "form",
          {
            class: "modal-body form",
            novalidate: true,
            onSubmit: async (/** @type {SubmitEvent} */ event) => {
              event.preventDefault();
              const name = nameInput.value.trim();
              if (name.length < 1 || name.length > 80) {
                nameField.setError("Enter a name of 1–80 characters.");
                nameInput.focus();
                return;
              }
              nameField.setError(null);
              try {
                const saved = await api.profile_save({ tool, name, modules: [...f.customModules] });
                const list = await api.profiles_list({ tool });
                dialog.close();
                if (disposed || f.tool !== tool) return;
                profiles = list;
                modulesStatus.textContent = `Saved profile “${saved.name}” (${plural(saved.modules.length, "module", "modules")}).`;
                renderModules();
              } catch (err) {
                errors.show(err);
              }
            },
          },
          // Both tool names (iLEAPP, aLEAPP) start with a vowel sound: "an".
          h("p", null, `${plural(f.customModules.length, "module", "modules")} will be saved as an ${toolName(tool)} profile.`),
          nameField.node,
          errors.node,
          h(
            "div",
            { class: "modal-actions" },
            h("button", { class: "btn", type: "button", onClick: () => dialog.close() }, "Cancel"),
            h("button", { class: "btn btn-primary", type: "submit" }, "Save profile"),
          ),
        ),
    });
    cleanups.push(() => dialog.close());
    dialog.open();
    nameInput.focus();
  }

  async function importProfile() {
    const tool = f.tool;
    if (!tool) return;
    modulesErrors.clear();
    modulesStatus.textContent = "";
    const ext = TOOL_FEATURES[tool].profileExt;
    try {
      const path = await api.dialog_open({ title: "Import profile", filters: [{ name: `${toolName(tool)} profile`, extensions: [ext] }] });
      if (!path || disposed) return;
      /** @type {ProfileInfo} */
      let info;
      try {
        info = await api.profile_import({ tool, path, name: null, overwrite: false });
      } catch (err) {
        if (!isAppError(err) || err.code !== "profile_exists") throw err;
        const replace = await confirmDialog({
          title: "Replace profile?",
          message: `${err.message} Replace it with the imported file?`,
          confirmLabel: "Replace",
        });
        if (!replace) return;
        info = await api.profile_import({ tool, path, name: null, overwrite: true });
      }
      const list = await api.profiles_list({ tool });
      // The examiner may have switched tools meanwhile: never show another tool's profiles.
      if (disposed || f.tool !== tool) return;
      profiles = list;
      f.moduleMode = "profile";
      f.profile = profiles.find((p) => p.name === info.name) ?? info;
      modulesStatus.textContent =
        `Imported profile “${info.name}” (${plural(info.modules.length, "module", "modules")}` +
        (info.unknown_modules.length ? `, ${info.unknown_modules.length} unknown).` : ").");
      renderModules();
      refresh();
    } catch (err) {
      modulesErrors.show(err, "The profile could not be imported.");
    }
  }

  async function exportProfile() {
    const tool = f.tool;
    const profile = f.profile;
    if (!tool || !profile) return;
    modulesErrors.clear();
    modulesStatus.textContent = "";
    const ext = TOOL_FEATURES[tool].profileExt;
    try {
      const dest = await api.dialog_save({
        title: "Export profile",
        defaultPath: `${profile.name}.${ext}`,
        filters: [{ name: `${toolName(tool)} profile`, extensions: [ext] }],
      });
      if (!dest || disposed) return;
      await api.profile_export({ tool, name: profile.name, dest_path: dest });
      modulesStatus.textContent = `Exported “${profile.name}” to ${dest}.`;
    } catch (err) {
      modulesErrors.show(err, "The profile could not be exported.");
    }
  }

  // ---- Start ----

  function refresh() {
    f.jobActive = store.get().activeJob !== null;
    saveProfileButton.disabled = f.customModules.length === 0 || f.customUnknown.length > 0;
    const blockers = startBlockers(f);
    startButton.disabled = starting || blockers.length > 0;
    startButton.textContent = starting ? "Starting…" : "Start run";
    reasons.replaceChildren(
      ...(blockers.length > 0
        ? [h("p", { class: "start-reasons-title" }, "Start becomes available when you:"), h("ul", { class: "list-compact" }, blockers.map((b) => h("li", null, b)))]
        : [h("p", { class: "ready-text" }, icon("check-circle"), "Ready to start.")]),
    );
  }

  /**
   * The form never submits: a run starts only from an explicit click (or keyboard activation) of
   * Start run.
   * @param {SubmitEvent} event
   */
  function ignoreSubmit(event) {
    event.preventDefault();
  }

  async function start() {
    if (starting || startBlockers(f).length > 0) return;
    const req = buildRunRequest(casePath, f);
    starting = true;
    startErrors.clear();
    refresh();
    // The run's events stream into the job hub, so the Run screen (and any later visit to it) sees
    // all of them; the hub also keeps the top bar's active-job phase current.
    const stream = jobs.begin("run", casePath);
    try {
      const started = await api.run_start(req, stream.onEvent);
      stream.bind(started.run_id);
      clearPassword();
      api
        .job_active()
        .then((job) => setActiveJob(store, job))
        .catch(() => {});
      navigate(routeHref("run", { case: casePath, id: started.run_id }));
    } catch (err) {
      stream.abandon();
      clearPassword();
      startErrors.show(err, "The run could not be started.");
    } finally {
      starting = false;
      if (!disposed) refresh();
    }
  }

  function clearPassword() {
    password.value = "";
    f.password = "";
  }

  cleanups.push(watch(store, (s) => jobKey(s.activeJob), () => refresh()));

  void init();
  return {
    node,
    dispose() {
      disposed = true;
      handedPassword = null;
      clearPassword();
      picker?.dispose();
      modulesErrors.dispose();
      startErrors.dispose();
      for (const fn of cleanups) fn();
    },
  };
}

/** @param {ToolStatus} t */
function stateText(t) {
  switch (t.state) {
    case "verified":
      return "verified";
    case "installed_unverified":
      return "verified before each run";
    case "dev_override":
      return "dev override";
    default:
      return t.state.replaceAll("_", " ");
  }
}
