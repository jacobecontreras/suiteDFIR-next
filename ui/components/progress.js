// @ts-check
/**
 * Progress displays shared by the Run, Acquire and Settings screens: a labelled progress bar and a
 * step list (phases of a job, stages of an install). Plain nodes; nothing to dispose.
 */
import { h } from "../lib/dom.js";
import { icon, phaseLabel, uid } from "../lib/view.js";

/** @typedef {import("../lib/jobstream.js").StepState} StepState */

/**
 * A `<progress>` with a visible label and detail text. `total: null` (or 0) = indeterminate.
 * @param {{ label: string, done: number | null, total: number | null, detail: string }} spec
 * @returns {HTMLElement}
 */
export function progressBar(spec) {
  const id = uid("progress");
  const determinate = typeof spec.total === "number" && spec.total > 0 && typeof spec.done === "number";
  const bar = /** @type {HTMLProgressElement} */ (h("progress", { class: "progress", id }));
  if (determinate) {
    bar.max = /** @type {number} */ (spec.total);
    bar.value = Math.min(/** @type {number} */ (spec.done), /** @type {number} */ (spec.total));
  }
  return h(
    "div",
    { class: "progress-block" },
    h("div", { class: "progress-head" }, h("label", { class: "progress-label", for: id }, spec.label), h("span", { class: "muted" }, spec.detail)),
    bar,
  );
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

/** @type {Record<StepState, { icon: "check-circle" | "play-circle" | "slash" | "circle", text: string }>} */
const STEP = {
  done: { icon: "check-circle", text: "done" },
  current: { icon: "play-circle", text: "in progress" },
  skipped: { icon: "slash", text: "skipped" },
  pending: { icon: "circle", text: "not started" },
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
        s.state === "skipped"
          ? h("span", { class: "phase-step-note" }, "skipped")
          : h("span", { class: "visually-hidden" }, ` (${STEP[s.state].text})`),
      ),
    ),
  );
}
