// @ts-check
/**
 * Progress displays shared by the Run, Acquire and Settings screens: a labelled progress bar and a
 * step list (phases of a job, stages of an install). Plain nodes; nothing to dispose.
 */
import { h } from "../lib/dom.js";
import { icon, phaseLabel, uid } from "../lib/view.js";

/** @typedef {import("../lib/jobstream.js").StepState | "failed"} StepState "failed": the install stage that failed. */

/**
 * @typedef {{ label: string, done: number | null, total: number | null, detail: string }} ProgressSpec
 * `total: null` (or 0) = indeterminate.
 */

/**
 * @typedef {object} ProgressMeter
 * @property {HTMLElement} node
 * @property {(spec: ProgressSpec) => void} update Changes the texts and the value in place.
 */

/**
 * A `<progress>` with a visible label and detail text, updated in place: the screens call `update`
 * on every progress event (up to 4/s) without replacing any element.
 * @param {ProgressSpec} spec
 * @returns {ProgressMeter}
 */
export function progressMeter(spec) {
  const id = uid("progress");
  const label = h("label", { class: "progress-label", for: id });
  const detail = h("span", { class: "muted" });
  const bar = /** @type {HTMLProgressElement} */ (h("progress", { class: "progress", id }));
  const node = h("div", { class: "progress-block" }, h("div", { class: "progress-head" }, label, detail), bar);
  /** @param {ProgressSpec} next */
  const update = (next) => {
    if (label.textContent !== next.label) label.textContent = next.label;
    if (detail.textContent !== next.detail) detail.textContent = next.detail;
    const determinate = typeof next.total === "number" && next.total > 0 && typeof next.done === "number";
    if (determinate) {
      const total = /** @type {number} */ (next.total);
      if (bar.max !== total) bar.max = total;
      const value = Math.min(/** @type {number} */ (next.done), total);
      if (!bar.hasAttribute("value") || bar.value !== value) bar.value = value;
    } else if (bar.hasAttribute("value")) {
      // No value attribute = indeterminate.
      bar.removeAttribute("value");
    }
  };
  update(spec);
  return { node, update };
}

/**
 * A `<progress>` with a visible label and detail text, built once (see `progressMeter` to update).
 * @param {ProgressSpec} spec
 * @returns {HTMLElement}
 */
export function progressBar(spec) {
  return progressMeter(spec).node;
}

/**
 * Whole percent of `done` in `total`, or null.
 * @param {number | null | undefined} done
 * @param {number | null | undefined} total
 * @returns {number | null}
 */
export function percentOf(done, total) {
  if (typeof done !== "number" || typeof total !== "number" || total <= 0) return null;
  return Math.max(0, Math.min(100, Math.floor((done / total) * 100)));
}

/** @type {Record<StepState, { icon: "check-circle" | "play-circle" | "slash" | "circle" | "x-circle", text: string }>} */
const STEP = {
  done: { icon: "check-circle", text: "done" },
  current: { icon: "play-circle", text: "in progress" },
  skipped: { icon: "slash", text: "skipped" },
  pending: { icon: "circle", text: "not started" },
  failed: { icon: "x-circle", text: "failed" },
};

/**
 * An ordered step list; the current step has `aria-current="step"`. Each state is also text
 * (visible for skipped steps, visually hidden otherwise), never colour alone.
 * @param {string} label The list's accessible name, e.g. "Run phases".
 * @param {{ phase: string, state: StepState }[]} steps
 * @param {(phase: string) => string} [labelOf]
 * @returns {HTMLElement}
 */
export function stepList(label, steps, labelOf = phaseLabel) {
  return h(
    "ol",
    { class: "phase-steps", "aria-label": label },
    steps.map((s) =>
      h(
        "li",
        { class: `phase-step phase-step-${s.state}`, "aria-current": s.state === "current" ? "step" : null },
        icon(STEP[s.state].icon, "icon phase-step-icon"),
        h("span", null, labelOf(s.phase)),
        s.state === "skipped" || s.state === "failed"
          ? h("span", { class: "phase-step-note" }, STEP[s.state].text)
          : h("span", { class: "visually-hidden" }, ` (${STEP[s.state].text})`),
      ),
    ),
  );
}
