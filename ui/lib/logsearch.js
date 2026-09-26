// @ts-check
/**
 * Log search (ROADMAP S2): which lines of the log view contain the examiner's query, where in a line
 * they do, and the next or previous match. Pure functions and a small incremental matcher
 * (tests/ui/logsearch.test.js); the log view (components/log-view.js) renders the result.
 *
 * Matching rules:
 * - A literal, case-insensitive substring search. Regular-expression characters in the query match
 *   themselves (`.*` finds the two characters ".*", not everything).
 * - Case-insensitive by Unicode simple case folding (RegExp flags `iu`): "Ä" finds "ä", and "Σ",
 *   "σ" and "ς" find each other. Folds that change the length are not applied ("ß" does not find
 *   "SS", "İ" does not find "i"), so match offsets are always offsets in the original line.
 * - A query also finds its other Unicode normalization form: "é" typed as one character finds "e"
 *   followed by a combining accent (as in macOS file names), and the other way round.
 * - Matching works on code points: a query never matches half of a surrogate pair.
 * - The empty query matches nothing (there is no search).
 *
 * Line indexes are absolute and 0-based: line `n` of the log view (1-based, as shown) is index
 * `n - 1`, counting the lines the log buffer dropped from its start, so an index stays valid while
 * the buffer grows and drops (lib/jobstream.js `appendLog`).
 */

/**
 * @typedef {object} LogLines The part of `LogBuffer` (lib/jobstream.js) the search reads.
 * @property {readonly string[]} lines
 * @property {number} dropped Lines dropped from the start.
 */

/**
 * @typedef {object} Part One piece of a line: plain text, or a match to highlight.
 * @property {string} text
 * @property {boolean} match
 */

/** RegExp syntax characters: the only ones that may (and must) be escaped with the `u` flag. */
const SYNTAX_CHARS = /[\\^$.*+?()[\]{}|/]/g;

/**
 * Escapes RegExp syntax characters so `text` matches itself.
 * @param {string} text
 * @returns {string}
 */
export function escapeRegExp(text) {
  return text.replace(SYNTAX_CHARS, "\\$&");
}

/**
 * The RegExp for a query: literal, case-insensitive, either normalization form. `global` gives one
 * for `splitMatches` (which resets its `lastIndex`); the non-global one suits `test` on each line.
 * @param {string} query
 * @param {boolean} [global]
 * @returns {RegExp | null} `null` for the empty query.
 */
export function compileQuery(query, global = false) {
  if (query === "") return null;
  const forms = [...new Set([query, query.normalize("NFC"), query.normalize("NFD")])];
  return new RegExp(forms.map(escapeRegExp).join("|"), global ? "giu" : "iu");
}

/**
 * Splits `text` into plain parts and matches of `re` (from `compileQuery(query, true)`), in order;
 * joined, the parts give `text` back. After `limit` matches the rest is one plain part, so a huge
 * line full of matches stays cheap to render.
 * @param {string} text
 * @param {RegExp} re A global RegExp.
 * @param {number} [limit]
 * @returns {Part[]}
 */
export function splitMatches(text, re, limit = Number.POSITIVE_INFINITY) {
  /** @type {Part[]} */
  const parts = [];
  let at = 0;
  re.lastIndex = 0;
  let found = 0;
  while (found < limit) {
    const m = re.exec(text);
    // A match is never empty (the query is not), but an empty one would never advance.
    if (m === null || m[0] === "") break;
    if (m.index > at) parts.push({ text: text.slice(at, m.index), match: false });
    parts.push({ text: m[0], match: true });
    at = m.index + m[0].length;
    found += 1;
  }
  re.lastIndex = 0;
  if (at < text.length) parts.push({ text: text.slice(at), match: false });
  return parts;
}

/**
 * The first position in ascending `sorted` whose value is >= `value` (`sorted.length` if none).
 * @param {readonly number[]} sorted
 * @param {number} value
 * @returns {number}
 */
export function lowerBound(sorted, value) {
  let lo = 0;
  let hi = sorted.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (sorted[mid] < value) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/**
 * @typedef {object} LogMatcher
 * @property {() => string} query
 * @property {() => readonly number[]} matches The matching lines' absolute indexes, ascending, as of
 *   the last `sync`.
 * @property {(query: string) => boolean} setQuery Returns whether the query changed; a change
 *   forgets the matches (the next `sync` searches the whole buffer).
 * @property {(log: LogLines) => void} sync Brings the matches up to date with `log`.
 */

/**
 * The lines of a growing log that match the query, kept up to date incrementally: each line is
 * tested once, when it first reaches `sync`; lines the buffer dropped from its start are forgotten.
 * A different buffer object, or one that shrank, is searched from scratch.
 * @returns {LogMatcher}
 */
export function logMatcher() {
  let query = "";
  /** @type {RegExp | null} */
  let re = null;
  /** @type {number[]} */
  let matches = [];
  /** Absolute index of the first line not tested yet. */
  let scanned = 0;
  /** @type {LogLines | null} */
  let source = null;

  return {
    query: () => query,
    matches: () => matches,
    setQuery(next) {
      if (next === query) return false;
      query = next;
      re = compileQuery(next);
      matches = [];
      scanned = 0;
      return true;
    },
    sync(log) {
      const total = log.dropped + log.lines.length;
      if (log !== source || total < scanned) {
        source = log;
        matches = [];
        scanned = 0;
      }
      if (re === null) {
        scanned = total;
        return;
      }
      if (matches.length > 0 && matches[0] < log.dropped) matches.splice(0, lowerBound(matches, log.dropped));
      for (let line = Math.max(scanned, log.dropped); line < total; line++) {
        if (re.test(log.lines[line - log.dropped])) matches.push(line);
      }
      scanned = total;
    },
  };
}

/**
 * @typedef {object} ViewLines The lines in view (absolute indexes, inclusive); `last < first` when
 *   nothing is shown.
 * @property {number} first
 * @property {number} last
 */

/**
 * The match to go to, as a line index (-1 when there are no matches), wrapping around at either
 * end. From a current match, "next" (`dir` 1) is the following match and "previous" (-1) the one
 * before. Without one (`current` -1), "next" is the first match at or after the top of the view and
 * "previous" the last match at or before its bottom.
 * @param {readonly number[]} matches Ascending line indexes.
 * @param {number} current A line index, or -1.
 * @param {1 | -1} dir
 * @param {ViewLines} view
 * @returns {number}
 */
export function stepMatch(matches, current, dir, view) {
  const n = matches.length;
  if (n === 0) return -1;
  if (dir > 0) {
    const i = current >= 0 ? lowerBound(matches, current + 1) : lowerBound(matches, view.first);
    return matches[i < n ? i : 0];
  }
  const i = (current >= 0 ? lowerBound(matches, current) : lowerBound(matches, view.last + 1)) - 1;
  return matches[i >= 0 ? i : n - 1];
}

/**
 * The 1-based position of `line` among the matches ("3" of "3 of 345"), or 0 if it is not one.
 * @param {readonly number[]} matches
 * @param {number} line
 * @returns {number}
 */
export function matchPosition(matches, line) {
  const i = lowerBound(matches, line);
  return i < matches.length && matches[i] === line ? i + 1 : 0;
}

/**
 * The row that shows line `line` in the log view: its index among the kept lines, or, when only
 * matching lines are shown (`matches` given), the first matching line at or after it. Clamped to
 * the rows that exist.
 * @param {number} line
 * @param {number} dropped
 * @param {number} kept Number of lines kept (`LogBuffer.lines.length`).
 * @param {readonly number[] | null} matches
 * @returns {number}
 */
export function rowOfLine(line, dropped, kept, matches) {
  const rows = matches ? matches.length : kept;
  const row = matches ? lowerBound(matches, line) : line - dropped;
  return Math.max(0, Math.min(rows - 1, row));
}

/**
 * The line a row shows: `dropped + row`, or, when only matching lines are shown, the row-th match.
 * @param {number} row
 * @param {number} dropped
 * @param {readonly number[] | null} matches
 * @returns {number}
 */
export function lineOfRow(row, dropped, matches) {
  return matches ? matches[row] : dropped + row;
}
