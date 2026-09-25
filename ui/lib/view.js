// @ts-check
/**
 * Small presentational helpers that return plain nodes (no state, nothing to dispose): inline SVG
 * icons, status badges (icon + text, never colour alone), and time/size text with the exact value
 * on hover.
 */
import { h } from "./dom.js";
import { formatBytes, formatLocalTime, formatUtcTime, parseTimestamp } from "./format.js";

/** @typedef {import("../types").RunStatus | import("../types").AcqStatus} JobStatus */

const SVG_NS = "http://www.w3.org/2000/svg";

/** @type {Record<string, [string, Record<string, string>][]>} */
const ICONS = {
  "check-circle": [["circle", { cx: "12", cy: "12", r: "10" }], ["polyline", { points: "8 12.5 11 15.5 16.5 9" }]],
  "alert-triangle": [
    ["path", { d: "M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z" }],
    ["line", { x1: "12", y1: "9", x2: "12", y2: "13" }],
    ["line", { x1: "12", y1: "17", x2: "12.01", y2: "17" }],
  ],
  "x-circle": [["circle", { cx: "12", cy: "12", r: "10" }], ["line", { x1: "15", y1: "9", x2: "9", y2: "15" }], ["line", { x1: "9", y1: "9", x2: "15", y2: "15" }]],
  slash: [["circle", { cx: "12", cy: "12", r: "10" }], ["line", { x1: "4.9", y1: "4.9", x2: "19.1", y2: "19.1" }]],
  "pause-circle": [["circle", { cx: "12", cy: "12", r: "10" }], ["line", { x1: "10", y1: "15", x2: "10", y2: "9" }], ["line", { x1: "14", y1: "15", x2: "14", y2: "9" }]],
  "play-circle": [["circle", { cx: "12", cy: "12", r: "10" }], ["polygon", { points: "10 8 16 12 10 16 10 8" }]],
  info: [["circle", { cx: "12", cy: "12", r: "10" }], ["line", { x1: "12", y1: "16", x2: "12", y2: "12" }], ["line", { x1: "12", y1: "8", x2: "12.01", y2: "8" }]],
  "chevron-right": [["polyline", { points: "9 18 15 12 9 6" }]],
  "chevron-down": [["polyline", { points: "6 9 12 15 18 9" }]],
  search: [["circle", { cx: "11", cy: "11", r: "7" }], ["line", { x1: "20", y1: "20", x2: "16.2", y2: "16.2" }]],
  folder: [["path", { d: "M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" }]],
};

/**
 * A decorative inline SVG icon (hidden from assistive technology; pair it with text).
 * @param {keyof typeof ICONS} name
 * @param {string} [className]
 * @returns {SVGSVGElement}
 */
export function icon(name, className = "icon") {
  const svg = /** @type {SVGSVGElement} */ (document.createElementNS(SVG_NS, "svg"));
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "2");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  svg.setAttribute("focusable", "false");
  svg.setAttribute("class", className);
  for (const [tag, attrs] of ICONS[name] ?? []) {
    const shape = document.createElementNS(SVG_NS, tag);
    for (const [key, value] of Object.entries(attrs)) shape.setAttribute(key, value);
    svg.appendChild(shape);
  }
  return svg;
}

/** @type {Record<JobStatus, { label: string, icon: keyof typeof ICONS }>} */
const STATUS = {
  running: { label: "Running", icon: "play-circle" },
  succeeded: { label: "Succeeded", icon: "check-circle" },
  completed_with_errors: { label: "Completed with errors", icon: "alert-triangle" },
  failed: { label: "Failed", icon: "x-circle" },
  cancelled: { label: "Cancelled", icon: "slash" },
  interrupted: { label: "Interrupted", icon: "pause-circle" },
};

/**
 * @param {JobStatus} status
 * @returns {string}
 */
export function statusLabel(status) {
  return STATUS[status]?.label ?? status;
}

/**
 * A status badge: icon + text + colour.
 * @param {JobStatus} status
 * @returns {HTMLElement}
 */
export function statusBadge(status) {
  const spec = STATUS[status] ?? { label: status, icon: "info" };
  return h("span", { class: `badge badge-${status}` }, icon(spec.icon), spec.label);
}

/**
 * Local time as text, with UTC on hover (DEVELOPMENT.md §4.4).
 * @param {string | null | undefined} iso
 * @returns {HTMLElement}
 */
export function timeText(iso) {
  if (!parseTimestamp(iso)) return h("span", { class: "muted" }, "—");
  return h("time", { datetime: iso, title: formatUtcTime(iso) }, formatLocalTime(iso));
}

/**
 * A size, with the exact byte count on hover.
 * @param {number | null | undefined} bytes
 * @returns {HTMLElement}
 */
export function sizeText(bytes) {
  if (typeof bytes !== "number") return h("span", { class: "muted" }, "—");
  return h("span", { title: `${bytes.toLocaleString("en-US")} bytes` }, formatBytes(bytes));
}

/** @type {Record<string, string>} */
const TOOL_NAMES = { ileapp: "iLEAPP", aleapp: "aLEAPP" };

/**
 * @param {string} tool
 * @returns {string}
 */
export function toolName(tool) {
  return TOOL_NAMES[tool] ?? tool;
}

/** @type {Record<string, string>} */
const PHASES = {
  preparing: "Preparing",
  running: "Running",
  hashing_input: "Hashing input",
  analyzing: "Analyzing",
  sealing_report: "Sealing report",
  finalizing: "Finalizing",
  enabling_encryption: "Enabling encryption",
  backing_up: "Backing up",
  restoring_encryption: "Restoring encryption",
  validating: "Validating",
  sealing: "Sealing",
};

/**
 * @param {string} phase a `RunPhase` or `AcqPhase`
 * @returns {string}
 */
export function phaseLabel(phase) {
  return PHASES[phase] ?? phase;
}

/** @type {Record<string, string>} */
const INPUT_TYPES = {
  fs: "File system folder (fs)",
  tar: "tar archive (tar)",
  zip: "zip archive (zip)",
  gz: "gzip archive (gz)",
  itunes: "iTunes/Finder backup (itunes)",
  file: "Single file (file)",
  raw: "Disk image (raw)",
};

/**
 * @param {string} type an `InputType`
 * @returns {string}
 */
export function inputTypeLabel(type) {
  return INPUT_TYPES[type] ?? type;
}

let idCounter = 0;

/**
 * A document-unique id for label/aria wiring.
 * @param {string} prefix
 * @returns {string}
 */
export function uid(prefix) {
  idCounter += 1;
  return `${prefix}-${idCounter}`;
}
