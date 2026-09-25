// @ts-check
/**
 * Virtual-list math for the log view (DEVELOPMENT.md §4.6). Rows have a fixed height, so the rows
 * in view follow from the scroll position alone and only those are in the DOM. Pure functions
 * (tests/ui/virtual.test.js).
 */

/**
 * @typedef {object} Viewport
 * @property {number} count Number of rows.
 * @property {number} rowHeight In px, > 0.
 * @property {number} scrollTop
 * @property {number} viewportHeight
 */

/**
 * @typedef {object} Range
 * @property {number} start First row to render (inclusive).
 * @property {number} end The row after the last one to render (exclusive); `start` when empty.
 * @property {number} offset Top of row `start`, in px.
 * @property {number} total Height of all rows, in px.
 */

/**
 * The rows to render for a scroll position, plus `overscan` rows on each side. A scroll position
 * past the end (the list shrank, or the viewport grew) is clamped to the last full page.
 * @param {Viewport & { overscan?: number }} v
 * @returns {Range}
 */
export function visibleRange({ count, rowHeight, scrollTop, viewportHeight, overscan = 0 }) {
  if (count <= 0 || rowHeight <= 0) return { start: 0, end: 0, offset: 0, total: 0 };
  const total = count * rowHeight;
  const height = Math.max(0, viewportHeight);
  const top = clamp(scrollTop, 0, Math.max(0, total - height));
  const first = Math.min(count - 1, Math.floor(top / rowHeight));
  const last = Math.max(first + 1, Math.ceil((top + height) / rowHeight));
  const start = Math.max(0, first - overscan);
  const end = Math.min(count, last + overscan);
  return { start, end, offset: start * rowHeight, total };
}

/**
 * The scrollTop that shows the last row at the bottom of the viewport (0 if everything fits).
 * @param {Omit<Viewport, "scrollTop">} v
 * @returns {number}
 */
export function bottomScrollTop({ count, rowHeight, viewportHeight }) {
  return Math.max(0, count * rowHeight - Math.max(0, viewportHeight));
}

/**
 * True when the viewport shows the end of the list, within half a row (browsers round scroll
 * positions, and zoom makes them fractional).
 * @param {Viewport} v
 * @returns {boolean}
 */
export function isAtBottom(v) {
  return v.scrollTop >= bottomScrollTop(v) - v.rowHeight / 2;
}

/**
 * How many row elements the view needs at most: a full viewport plus a partial row at each edge,
 * plus the overscan on both sides. Independent of the number of lines.
 * @param {number} viewportHeight
 * @param {number} rowHeight
 * @param {number} overscan
 * @returns {number}
 */
export function poolSize(viewportHeight, rowHeight, overscan) {
  if (rowHeight <= 0) return 0;
  return Math.ceil(Math.max(0, viewportHeight) / rowHeight) + 1 + 2 * overscan;
}

/**
 * @param {number} value
 * @param {number} min
 * @param {number} max
 */
function clamp(value, min, max) {
  return Math.min(max, Math.max(min, Number.isFinite(value) ? value : min));
}
