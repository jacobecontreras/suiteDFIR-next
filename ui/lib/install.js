// @ts-check
/**
 * Parser installs on the Settings screen (ROADMAP D4b; tests/ui/install.test.js): what each
 * `ToolState` means and allows, and the progress of an install or offline import from its
 * `InstallEvent`s. Progress objects are immutable, because they live in the store (installs keep
 * streaming when the examiner leaves the screen).
 */

/** @typedef {import("../types").AppError} AppError */
/** @typedef {import("../types").InstallEvent} InstallEvent */
/** @typedef {import("../types").InstallStage} InstallStage */
/** @typedef {import("../types").ToolState} ToolState */
/** @typedef {import("../components/progress.js").StepState} StepState */

/**
 * @typedef {object} InstallProgress
 * @property {"download" | "import"} mode
 * @property {boolean} running
 * @property {InstallStage[]} stages Stages seen, in order.
 * @property {{ done: number, total: number } | null} download
 * @property {string[]} messages The latest `message` events (at most `MAX_MESSAGES`).
 * @property {AppError | null} error
 */

export const MAX_MESSAGES = 5;

/** @type {readonly InstallStage[]} */
const DOWNLOAD_STAGES = ["downloading", "verifying", "extracting", "hashing", "introspecting", "done"];
/** @type {readonly InstallStage[]} */
const IMPORT_STAGES = ["verifying", "extracting", "hashing", "introspecting", "done"];

/**
 * @typedef {object} StateInfo
 * @property {string} label
 * @property {"ok" | "info" | "warn" | "danger" | "neutral"} tone
 * @property {"check-circle" | "info" | "alert-triangle" | "x-circle" | "slash" | "circle"} icon
 * @property {string} text What the state means for the examiner.
 */

/** @type {Record<ToolState, StateInfo>} */
export const TOOL_STATES = {
  verified: {
    label: "Verified",
    tone: "ok",
    icon: "check-circle",
    text: "Installed. Its binary matched the pinned SHA-256 when it was last checked in this session; it is checked again before every run.",
  },
  installed_unverified: {
    label: "Installed",
    tone: "info",
    icon: "info",
    text: "Installed, not checked yet in this session. Its binary is checked against the pinned SHA-256 before every run; Verify checks it now.",
  },
  not_installed: {
    label: "Not installed",
    tone: "neutral",
    icon: "circle",
    text: "Install downloads the pinned release and checks its SHA-256. Import installs from a release file you already have, without a network.",
  },
  verification_failed: {
    label: "Verification failed",
    tone: "danger",
    icon: "x-circle",
    text: "The installed binary does not match the pinned SHA-256, so runs are blocked. Install it again, or import a release file.",
  },
  unsupported_platform: {
    label: "Not available",
    tone: "neutral",
    icon: "slash",
    text: "No build of this parser is pinned for this platform, so it cannot be installed here.",
  },
  dev_override: {
    label: "Dev override",
    tone: "danger",
    icon: "alert-triangle",
    text: "A development test double replaces this parser (debug builds only). Nothing is installed or verified, and results are not evidence.",
  },
};

/**
 * The actions a tool card offers in each state.
 * @param {ToolState} state
 * @returns {{ install: boolean, import: boolean, verify: boolean }}
 */
export function toolActions(state) {
  switch (state) {
    case "not_installed":
      return { install: true, import: true, verify: false };
    case "verification_failed":
      return { install: true, import: true, verify: true };
    case "installed_unverified":
    case "verified":
      return { install: false, import: false, verify: true };
    default:
      return { install: false, import: false, verify: false };
  }
}

/** @type {Record<InstallStage, string>} */
const STAGE_LABELS = {
  downloading: "Download",
  verifying: "Check SHA-256",
  extracting: "Extract",
  hashing: "Hash the binary",
  introspecting: "Read the module list",
  done: "Done",
};

/**
 * @param {string} stage an `InstallStage`
 * @returns {string}
 */
export function stageLabel(stage) {
  return STAGE_LABELS[/** @type {InstallStage} */ (stage)] ?? stage;
}

/**
 * @param {"download" | "import"} mode
 * @returns {InstallProgress}
 */
export function startInstall(mode) {
  return { mode, running: true, stages: [], download: null, messages: [], error: null };
}

/**
 * @param {InstallProgress} p
 * @param {InstallEvent} event
 * @returns {InstallProgress}
 */
export function applyInstallEvent(p, event) {
  switch (event.type) {
    case "stage":
      return p.stages[p.stages.length - 1] === event.stage ? p : { ...p, stages: [...p.stages, event.stage] };
    case "download_progress":
      return { ...p, download: { done: event.bytes_done, total: event.bytes_total } };
    case "message":
      return { ...p, messages: [...p.messages, event.text].slice(-MAX_MESSAGES) };
  }
  return p;
}

/**
 * @param {InstallProgress} p
 * @param {AppError | null} error
 * @returns {InstallProgress}
 */
export function finishInstall(p, error) {
  return { ...p, running: false, error };
}

/**
 * The stage list: stages seen are done, except the last one, which is in progress while running
 * and failed after an error. Unreached stages are pending while running and left out afterwards.
 * @param {InstallProgress} p
 * @returns {{ phase: InstallStage, state: StepState }[]}
 */
export function installSteps(p) {
  const order = p.mode === "download" ? DOWNLOAD_STAGES : IMPORT_STAGES;
  const last = p.stages[p.stages.length - 1] ?? null;
  const seen = new Set(p.stages);
  /** @type {{ phase: InstallStage, state: StepState }[]} */
  const out = [];
  for (const stage of order) {
    if (stage === last) {
      out.push({ phase: stage, state: p.running ? "current" : p.error ? "failed" : "done" });
    } else if (seen.has(stage)) {
      out.push({ phase: stage, state: "done" });
    } else if (p.running) {
      out.push({ phase: stage, state: "pending" });
    }
  }
  if (!p.running && p.error && last === null) out.unshift({ phase: order[0], state: "failed" });
  return out;
}
