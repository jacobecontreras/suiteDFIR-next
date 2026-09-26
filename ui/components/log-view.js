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
 *
 * With `search` (the Run screen, ROADMAP S2), a search box finds the lines that contain a literal,
 * case-insensitive query (lib/logsearch.js):
 * - Matches are highlighted in the rows in view with `<mark>` elements built from text nodes, never
 *   from markup. Only the rows in view are highlighted, so a search costs one pass over the lines
 *   and new lines are searched once, as they arrive.
 * - "Only matching lines" shows just those lines, with their own line numbers.
 * - Previous/Next (and Shift+Enter/Enter in the box) go to the matching lines in turn, wrapping
 *   around; the first one starts from the view. Going to a match stops auto-scroll and scrolls
 *   sideways too when the match is beyond the edge of a long line. Escape clears the box and
 *   unticks "Only matching lines". The box is not in a form, its keys do nothing else, and the
 *   Enter that commits an input-method composition is ignored.
 * - The query is at most `MAX_QUERY_LENGTH` (256) characters, which bounds the cost of a search.
 * - The status counts the lines the view holds; when that is not the whole log (lines dropped past
 *   the buffer, or only the backlog after a reload), it says so.
 * - The one live region reports the search too (the number of matching lines when the query
 *   changes, the line reached by Previous/Next), through the same once-per-second throttle: a
 *   search report goes out first, and the latest line resumes at the next interval.
 */
import { h, setText } from "../lib/dom.js";
import { formatCount } from "../lib/format.js";
import {
  MAX_QUERY_LENGTH,
  compileQuery,
  lineOfRow,
  logMatcher,
  matchPosition,
  rowOfLine,
  searchStatus,
  splitMatches,
  stepMatch,
} from "../lib/logsearch.js";
import { throttleLatest } from "../lib/throttle.js";
import { bottomScrollTop, isAtBottom, visibleRange } from "../lib/virtual.js";
import { icon, uid } from "../lib/view.js";

/** @typedef {import("../lib/jobstream.js").LogBuffer} LogBuffer */
/** @typedef {import("../lib/logsearch.js").ViewLines} ViewLines */
/** @typedef {import("../lib/context").View} View */

/** Row height in px; the CSS reads it from `--log-row` on the viewport. */
export const LOG_ROW_HEIGHT = 20;
const OVERSCAN = 10;
const ANNOUNCE_INTERVAL_MS = 1000;
/** Highlighted matches per row at most; the rest of a (huge) line stays plain text. */
const MAX_MARKS_PER_ROW = 200;

/**
 * @typedef {View & { update: (log: LogBuffer, partial?: boolean) => void }} LogView
 * `partial`: the log starts mid-way (attached after a reload, `JobStream.partial`).
 */

/**
 * @param {{ label: string, emptyText: string, search?: boolean }} spec
 * @returns {LogView}
 */
export function logView(spec) {
  /** @type {LogBuffer} */
  let log = { lines: [], dropped: 0 };
  /** The log starts mid-way (only the backlog after a reload). */
  let partial = false;
  let follow = true;
  let frame = 0;
  /** The scrollTop the view set itself; any other position comes from the examiner. */
  let ownTop = -1;
  let announced = 0;
  /** @type {HTMLElement[]} */
  const pool = [];
  /** @type {number[]} The absolute line number each pooled row shows (-1 = none). */
  const shown = [];

  // Search state (only changed when `spec.search`).
  const matcher = logMatcher();
  /** @type {RegExp | null} For highlighting; `null` without a query. */
  let marker = null;
  let onlyMatching = false;
  /** The current match (absolute line index), or -1. */
  let current = -1;
  /** Scroll the current match into view (both ways) on the next render. */
  let reveal = false;
  /** A line to show at the top on the next render (the rows changed under the examiner), or -1. */
  let anchor = -1;
  /** @type {"count" | "match" | null} What the live region should report after the next render. */
  let report = null;

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
  const announce = throttleLatest((/** @type {string} */ text) => {
    live.textContent = text;
  }, ANNOUNCE_INTERVAL_MS);

  const searchBar = spec.search ? buildSearchBar() : null;

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
    searchBar?.node,
    h("div", { class: "log-frame" }, viewport, empty),
    live,
  );

  viewport.addEventListener(
    "scroll",
    () => {
      const top = viewport.scrollTop;
      if (Math.abs(top - ownTop) >= 1) {
        const atEnd = isAtBottom({ count: rowCount(), rowHeight: LOG_ROW_HEIGHT, scrollTop: top, viewportHeight: viewport.clientHeight });
        if (atEnd !== follow) setFollow(atEnd);
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

  /** @param {boolean} value */
  function setFollow(value) {
    follow = value;
    followBox.checked = value;
  }

  /** The matching lines when only those are shown, else `null` (all kept lines are rows). */
  function filteredMatches() {
    return onlyMatching && matcher.query() !== "" ? matcher.matches() : null;
  }

  function rowCount() {
    return filteredMatches()?.length ?? log.lines.length;
  }

  /**
   * Sets the scroll position; the scroll event it causes is not taken as the examiner's.
   * @param {number} top
   */
  function scrollToTop(top) {
    viewport.scrollTop = top;
    ownTop = viewport.scrollTop;
  }

  /**
   * The lines in view (absolute indexes), as the rows are currently mapped.
   * @returns {ViewLines}
   */
  function viewLines() {
    const matches = filteredMatches();
    const r = visibleRange({ count: rowCount(), rowHeight: LOG_ROW_HEIGHT, scrollTop: viewport.scrollTop, viewportHeight: viewport.clientHeight });
    if (r.end <= r.start) return { first: 0, last: -1 };
    return { first: lineOfRow(r.start, log.dropped, matches), last: lineOfRow(r.end - 1, log.dropped, matches) };
  }

  function render() {
    frame = 0;
    if (spec.search) matcher.sync(log);
    const matches = filteredMatches();
    const lines = log.lines;
    const count = matches?.length ?? lines.length;
    const height = viewport.clientHeight;
    spacer.style.height = `${count * LOG_ROW_HEIGHT}px`;
    const revealing = reveal && current >= 0;
    if (revealing) {
      // Going to a match: into view (centred) unless it is fully in view already.
      const top = rowOfLine(current, log.dropped, lines.length, matches) * LOG_ROW_HEIGHT;
      if (top < viewport.scrollTop || top + LOG_ROW_HEIGHT > viewport.scrollTop + height) {
        scrollToTop(Math.max(0, top - Math.floor((height - LOG_ROW_HEIGHT) / 2)));
      }
    } else if (follow) {
      const target = bottomScrollTop({ count, rowHeight: LOG_ROW_HEIGHT, viewportHeight: height });
      if (Math.abs(viewport.scrollTop - target) >= 1) viewport.scrollTop = target;
      ownTop = viewport.scrollTop;
    } else if (anchor >= 0) {
      // The rows changed (filter or query): keep the line that was at the top in view.
      scrollToTop(rowOfLine(anchor, log.dropped, lines.length, matches) * LOG_ROW_HEIGHT);
    }
    reveal = false;
    anchor = -1;
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
      const line = lineOfRow(i, log.dropped, matches);
      const number = line + 1;
      if (shown[k] !== number) {
        shown[k] = number;
        /** @type {HTMLElement} */ (row.firstChild).textContent = String(number);
        paintText(/** @type {HTMLElement} */ (row.lastChild), lines[line - log.dropped]);
      }
      if (spec.search) row.classList.toggle("log-row-current", line === current);
      if (row.hidden) row.hidden = false;
    }
    if (revealing) revealSideways();
    empty.hidden = count > 0;
    setText(empty, matches && lines.length > 0 ? "No lines match the search." : spec.emptyText);
    const total = log.dropped + lines.length;
    viewport.dataset.count = String(total);
    countText.textContent =
      log.dropped > 0
        ? `${formatCount(total)} lines (the first ${formatCount(log.dropped)} are not kept here; the full log is in the job folder)`
        : `${formatCount(total)} ${total === 1 ? "line" : "lines"}`;
    if (searchBar) searchBar.render();
  }

  /**
   * After going to a match: scrolls sideways when the current row's first highlighted match lies
   * beyond either edge of the view (a long line), leaving a margin around it.
   */
  function revealSideways() {
    const mark = rowsEl.querySelector(".log-row-current:not([hidden]) mark");
    if (!(mark instanceof HTMLElement)) return;
    // Offsets are relative to `.log-rows`, which starts at the left of the scrolled content.
    const left = mark.offsetLeft;
    const right = left + mark.offsetWidth;
    const width = viewport.clientWidth;
    const margin = Math.min(48, Math.floor(width / 4));
    if (left < viewport.scrollLeft || right > viewport.scrollLeft + width) {
      viewport.scrollLeft = right - left + 2 * margin > width ? Math.max(0, left - margin) : Math.max(0, right + margin - width);
    }
  }

  /**
   * Sets a row's text: plain, or split around the matches with each match in a `<mark>`. Both are
   * built as text nodes (the strings passed to `replaceChildren` and `h()` children), never parsed.
   * @param {HTMLElement} el
   * @param {string} text
   */
  function paintText(el, text) {
    const parts = marker && text !== "" ? splitMatches(text, marker, MAX_MARKS_PER_ROW) : null;
    if (!parts || (parts.length === 1 && !parts[0].match)) {
      el.textContent = text;
      return;
    }
    el.replaceChildren(...parts.map((p) => (p.match ? h("mark", null, p.text) : p.text)));
  }

  /** The search box, Previous/Next, "Only matching lines" and the match count. */
  function buildSearchBar() {
    const searchId = uid("log-search");
    const statusId = `${searchId}-status`;
    const onlyId = `${searchId}-only`;
    const input = /** @type {HTMLInputElement} */ (
      h("input", {
        class: "input log-search-input",
        type: "search",
        id: searchId,
        placeholder: "Search the log",
        autocomplete: "off",
        spellcheck: "false",
        maxlength: MAX_QUERY_LENGTH,
        "aria-describedby": statusId,
      })
    );
    const prev = /** @type {HTMLButtonElement} */ (
      h("button", { class: "btn btn-sm", type: "button", title: "Previous matching line (Shift+Enter)", onClick: () => go(-1) }, "Previous", h("span", { class: "visually-hidden" }, " match"))
    );
    const next = /** @type {HTMLButtonElement} */ (
      h("button", { class: "btn btn-sm", type: "button", title: "Next matching line (Enter)", onClick: () => go(1) }, "Next", h("span", { class: "visually-hidden" }, " match"))
    );
    const only = /** @type {HTMLInputElement} */ (h("input", { type: "checkbox", id: onlyId }));
    const status = h("span", { class: "log-search-status muted", id: statusId });

    input.addEventListener("input", () => setQuery(input.value));
    input.addEventListener("keydown", (e) => {
      // An input method's keys, including the Enter that commits a composition (WebKit reports it
      // with isComposing false but keyCode 229), belong to the input method.
      if (e.isComposing || e.keyCode === 229) return;
      if (e.key === "Enter") {
        e.preventDefault();
        go(e.shiftKey ? -1 : 1);
      } else if (e.key === "Escape" && (input.value !== "" || only.checked)) {
        // Clears the search: the query and "Only matching lines", which would otherwise stay
        // ticked while every line is shown.
        e.preventDefault();
        input.value = "";
        setQuery("");
        if (only.checked) {
          only.checked = false;
          onlyMatching = false;
          schedule();
        }
      }
    });
    only.addEventListener("change", () => {
      const top = follow ? -1 : viewLines().first;
      onlyMatching = only.checked;
      if (follow) {
        // Stays at the end of the (other) rows.
      } else if (current >= 0) {
        reveal = true;
      } else {
        anchor = top;
      }
      schedule();
    });

    /** @param {string} query */
    function setQuery(query) {
      const top = follow ? -1 : viewLines().first;
      if (!matcher.setQuery(query)) return;
      marker = compileQuery(query, true);
      current = -1;
      shown.fill(-1);
      if (onlyMatching) anchor = top;
      report = query === "" ? null : "count";
      schedule();
    }

    /** @param {1 | -1} dir */
    function go(dir) {
      if (matcher.query() === "") return;
      matcher.sync(log);
      const target = stepMatch(matcher.matches(), current, dir, viewLines());
      if (target < 0) {
        report = "count";
      } else {
        current = target;
        reveal = true;
        setFollow(false);
        report = "match";
      }
      schedule();
    }

    const node = h(
      "div",
      { class: "log-search", role: "search", "aria-label": `${spec.label} search` },
      h("div", { class: "log-search-field" }, icon("search"), h("label", { class: "visually-hidden", for: searchId }, "Search the log"), input),
      prev,
      next,
      h("label", { class: "check", for: onlyId }, only, h("span", null, "Only matching lines")),
      status,
    );

    return {
      node,
      render() {
        const query = matcher.query();
        const matches = matcher.matches();
        const n = matches.length;
        const position = current >= 0 ? matchPosition(matches, current) : 0;
        const scope = { query, matches: n, kept: log.lines.length, partial: partial || log.dropped > 0 };
        setText(status, searchStatus({ ...scope, position }));
        prev.disabled = n === 0;
        next.disabled = n === 0;
        // Search reports go out ahead of the latest line (which resumes at the next interval).
        if (report === "match" && position > 0) {
          announce.pushUrgent(`Match ${formatCount(position)} of ${formatCount(n)}, line ${formatCount(current + 1)}: ${log.lines[current - log.dropped] ?? ""}`);
        } else if (report !== null && query !== "") {
          announce.pushUrgent(searchStatus({ ...scope, position: 0 }));
        }
        report = null;
      },
    };
  }

  async function copyAll() {
    const lines = log.lines;
    const dropped = log.dropped;
    const ok = await copyText(lines.join("\n"));
    const copied = `${formatCount(lines.length)} ${lines.length === 1 ? "line" : "lines"}`;
    copyStatus.textContent = !ok
      ? "Could not copy: the clipboard is not available."
      : dropped > 0
        ? `Copied the last ${copied}. The first ${formatCount(dropped)} are not kept here and were not copied; the full log is in the job folder.`
        : `Copied ${copied}.`;
  }

  return {
    node,
    update(next, startsMidway = false) {
      if (next !== log) {
        log = next;
        announced = 0;
        current = -1;
        shown.fill(-1);
      }
      partial = startsMidway;
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
