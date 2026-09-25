// @ts-check
/**
 * The virtualized log (ROADMAP D4a, DEVELOPMENT.md §4.6), shared by the Run and Acquire screens.
 *
 * - Rows have a fixed height (`LOG_ROW_HEIGHT`), so only the rows in view (plus a small overscan)
 *   exist in the DOM, whatever the number of lines (lib/virtual.js). A fixed pool of row elements
 *   is reused; a row's text is set only when the line it shows changes.
 * - Rendering is batched to one animation frame, however many `log` events arrive.
 * - Auto-scroll follows new lines. Scrolling up stops it; scrolling back to the end (or ticking the
 *   box) resumes it.
 * - The latest line goes to an `aria-live="polite"` region at most once per second. The scrolling
 *   rows themselves are not a live region.
 * - Lines are evidence-derived text: set with `textContent` only.
 */
import { h } from "../lib/dom.js";
import { formatCount } from "../lib/format.js";
import { throttleLatest } from "../lib/throttle.js";
import { bottomScrollTop, isAtBottom, visibleRange } from "../lib/virtual.js";
import { uid } from "../lib/view.js";

/** @typedef {import("../lib/jobstream.js").LogBuffer} LogBuffer */
/** @typedef {import("../lib/context").View} View */

/** Row height in px; the CSS reads it from `--log-row` on the viewport. */
export const LOG_ROW_HEIGHT = 20;
const OVERSCAN = 10;
const ANNOUNCE_INTERVAL_MS = 1000;

/**
 * @typedef {View & { update: (log: LogBuffer) => void }} LogView
 */

/**
 * @param {{ label: string, emptyText: string }} spec
 * @returns {LogView}
 */
export function logView(spec) {
  /** @type {LogBuffer} */
  let log = { lines: [], dropped: 0 };
  let follow = true;
  let frame = 0;
  /** The scrollTop the view set itself; any other position comes from the examiner. */
  let ownTop = -1;
  let announced = 0;
  /** @type {HTMLElement[]} */
  const pool = [];
  /** @type {number[]} The absolute line number each pooled row shows (-1 = none). */
  const shown = [];

  const viewport = h("div", {
    class: "log-viewport",
    tabindex: "0",
    role: "region",
    "aria-label": `${spec.label} (scrollable)`,
    "data-count": "0",
  });
  viewport.style.setProperty("--log-row", `${LOG_ROW_HEIGHT}px`);
  const rowsEl = h("div", { class: "log-rows" });
  const spacer = h("div", { class: "log-spacer" }, rowsEl);
  viewport.append(spacer);
  const empty = h("p", { class: "log-empty muted" }, spec.emptyText);

  const followId = uid("log-follow");
  const followBox = /** @type {HTMLInputElement} */ (h("input", { type: "checkbox", id: followId }));
  followBox.checked = true;
  followBox.addEventListener("change", () => {
    follow = followBox.checked;
    if (follow) schedule();
  });
  const countText = h("span", { class: "log-count muted" }, "0 lines");
  const copyStatus = h("span", { class: "log-copy-status muted", role: "status" });
  const copyButton = h("button", { class: "btn btn-sm", type: "button", onClick: copyAll }, "Copy all");
  const live = h("p", { class: "visually-hidden", "aria-live": "polite" });
  const announce = throttleLatest((/** @type {string} */ line) => {
    live.textContent = line;
  }, ANNOUNCE_INTERVAL_MS);

  const node = h(
    "div",
    { class: "log-view" },
    h(
      "div",
      { class: "log-toolbar" },
      h("label", { class: "check", for: followId }, followBox, h("span", null, "Auto-scroll")),
      copyButton,
      copyStatus,
      h("span", { class: "log-toolbar-spacer" }),
      countText,
    ),
    h("div", { class: "log-frame" }, viewport, empty),
    live,
  );

  viewport.addEventListener(
    "scroll",
    () => {
      const top = viewport.scrollTop;
      if (Math.abs(top - ownTop) >= 1) {
        const atEnd = isAtBottom({ count: log.lines.length, rowHeight: LOG_ROW_HEIGHT, scrollTop: top, viewportHeight: viewport.clientHeight });
        if (atEnd !== follow) {
          follow = atEnd;
          followBox.checked = atEnd;
        }
      }
      schedule();
    },
    { passive: true },
  );

  const resize = typeof ResizeObserver === "function" ? new ResizeObserver(() => schedule()) : null;
  resize?.observe(viewport);

  function schedule() {
    if (frame === 0) frame = requestAnimationFrame(render);
  }

  function render() {
    frame = 0;
    const lines = log.lines;
    const count = lines.length;
    const height = viewport.clientHeight;
    spacer.style.height = `${count * LOG_ROW_HEIGHT}px`;
    if (follow) {
      const target = bottomScrollTop({ count, rowHeight: LOG_ROW_HEIGHT, viewportHeight: height });
      if (Math.abs(viewport.scrollTop - target) >= 1) viewport.scrollTop = target;
      ownTop = viewport.scrollTop;
    }
    const range = visibleRange({ count, rowHeight: LOG_ROW_HEIGHT, scrollTop: viewport.scrollTop, viewportHeight: height, overscan: OVERSCAN });
    const needed = range.end - range.start;
    while (pool.length < needed) {
      const row = h("div", { class: "log-row" }, h("span", { class: "log-no", "aria-hidden": "true" }), h("span", { class: "log-text" }));
      pool.push(row);
      shown.push(-1);
      rowsEl.append(row);
    }
    rowsEl.style.transform = `translateY(${range.offset}px)`;
    for (let k = 0; k < pool.length; k++) {
      const row = pool[k];
      const i = range.start + k;
      if (i >= range.end) {
        if (!row.hidden) row.hidden = true;
        continue;
      }
      const number = log.dropped + i + 1;
      if (shown[k] !== number) {
        shown[k] = number;
        /** @type {HTMLElement} */ (row.firstChild).textContent = String(number);
        /** @type {HTMLElement} */ (row.lastChild).textContent = lines[i];
      }
      if (row.hidden) row.hidden = false;
    }
    empty.hidden = count > 0;
    const total = log.dropped + count;
    viewport.dataset.count = String(total);
    countText.textContent =
      log.dropped > 0
        ? `${formatCount(total)} lines (the first ${formatCount(log.dropped)} are not kept here; the full log is in the job folder)`
        : `${formatCount(total)} ${total === 1 ? "line" : "lines"}`;
  }

  async function copyAll() {
    const lines = log.lines;
    const ok = await copyText(lines.join("\n"));
    copyStatus.textContent = ok
      ? `Copied ${formatCount(lines.length)} ${lines.length === 1 ? "line" : "lines"}.`
      : "Could not copy: the clipboard is not available.";
  }

  return {
    node,
    update(next) {
      if (next !== log) {
        log = next;
        announced = 0;
        shown.fill(-1);
      }
      const total = log.dropped + log.lines.length;
      if (total > announced && log.lines.length > 0) {
        announced = total;
        announce.push(log.lines[log.lines.length - 1]);
      }
      schedule();
    },
    dispose() {
      if (frame !== 0) cancelAnimationFrame(frame);
      frame = 0;
      resize?.disconnect();
      announce.cancel();
    },
  };
}

/**
 * Copies text to the clipboard: the async Clipboard API, else a hidden textarea and the legacy
 * copy command (still the only way in some webviews).
 * @param {string} text
 * @returns {Promise<boolean>}
 */
async function copyText(text) {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // Fall back below.
  }
  const area = /** @type {HTMLTextAreaElement} */ (h("textarea", { class: "clipboard-helper", readonly: true, "aria-hidden": "true", tabindex: "-1" }));
  area.value = text;
  document.body.append(area);
  area.select();
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  }
  area.remove();
  return ok;
}
