// @ts-check
// Log search: matching, highlighting parts, the incremental matcher and match navigation (ROADMAP S2).
import { test } from "node:test";
import assert from "node:assert/strict";

import { appendLog } from "../../ui/lib/jobstream.js";
import {
  MAX_QUERY_LENGTH,
  compileQuery,
  escapeRegExp,
  lineOfRow,
  logMatcher,
  lowerBound,
  matchPosition,
  rowOfLine,
  searchStatus,
  splitMatches,
  stepMatch,
} from "../../ui/lib/logsearch.js";

/**
 * Whether `query` finds `text`.
 * @param {string} query
 * @param {string} text
 */
function finds(query, text) {
  const re = compileQuery(query);
  return re !== null && re.test(text);
}

/**
 * The matched substrings of `text`.
 * @param {string} text
 * @param {string} query
 */
function marked(text, query) {
  const re = compileQuery(query, true);
  assert.ok(re);
  return splitMatches(text, re)
    .filter((p) => p.match)
    .map((p) => p.text);
}

/**
 * Brute force: the absolute indexes of the kept lines that contain `query`, case-insensitively.
 * @param {{ lines: string[], dropped: number }} log
 * @param {string} query
 */
function oracle(log, query) {
  const re = compileQuery(query);
  /** @type {number[]} */
  const out = [];
  if (!re) return out;
  log.lines.forEach((line, i) => {
    if (re.test(line)) out.push(log.dropped + i);
  });
  return out;
}

// ---- Matching ----

test("the empty query is no search: it matches nothing", () => {
  assert.equal(compileQuery(""), null);
  assert.equal(compileQuery("", true), null);
  const m = logMatcher();
  m.setQuery("");
  m.sync({ lines: ["a", "", "b"], dropped: 0 });
  assert.deepEqual(m.matches(), []);
});

test("a case-insensitive substring search", () => {
  assert.ok(finds("error", "Artifact ERROR in sms"));
  assert.ok(finds("ERROR", "an error occurred"));
  assert.ok(finds("Rror oc", "an error occurred"));
  assert.ok(!finds("errors", "an error occurred"));
  // Spaces are part of the query: it is used as typed.
  assert.ok(!finds(" error", "error at the start"));
  assert.ok(finds(" ", "two words"));
});

test("regular-expression characters are literal", () => {
  for (const query of [".", "*", "+", "?", "^", "$", "|", "\\", "/", "(", ")", "[", "]", "{", "}", "-", ",", "\\d", "a{2}", "(a|b)", "[a-z]+", "^$", "?<name>", "\\u0041"]) {
    const re = compileQuery(query);
    assert.ok(re, query);
    assert.ok(re.test(`before ${query} after`), `${query} finds itself`);
  }
  assert.ok(!finds(".*", "anything at all"));
  assert.ok(finds(".*", "matched: .* here"));
  assert.ok(!finds("a+b", "aab"));
  assert.ok(!finds("\\d", "123"));
  assert.ok(!finds("[a-z]+", "letters"));
  assert.ok(!finds("(a|b)", "a"));
  assert.ok(!finds("\\u0041", "A"));
  assert.equal(escapeRegExp("a.b*c(d)[e]{f}|g\\h^i$j+k?l/m"), "a\\.b\\*c\\(d\\)\\[e\\]\\{f\\}\\|g\\\\h\\^i\\$j\\+k\\?l\\/m");
  // Only syntax characters are escaped: anything else would be an invalid escape with the `u` flag.
  assert.equal(escapeRegExp("a-b,c:d=e!f<g>h'i\"j#k"), "a-b,c:d=e!f<g>h'i\"j#k");
});

test("unicode: case folding beyond ASCII", () => {
  assert.ok(finds("ärger", "GROSSER ÄRGER"));
  assert.ok(finds("ÄRGER", "großer ärger"));
  assert.ok(finds("привет", "ПРИВЕТ, МИР"));
  assert.ok(finds("ΟΔΟΣ", "η οδος"));
  // Greek sigma: capital, medial and final forms find each other.
  assert.ok(finds("Σ", "οδος"));
  assert.ok(finds("ς", "ΟΔΟΣ"));
  assert.ok(finds("σ", "οδος"));
  // Kelvin sign and k fold together.
  assert.ok(finds("k", "K"));
  // Folds that change the length are not applied, so offsets stay offsets of the original line.
  assert.ok(!finds("ß", "STRASSE"));
  assert.ok(!finds("i", "İ"));
});

test("unicode: either normalization form finds the other", () => {
  const nfc = "Café";
  const nfd = "Café";
  assert.ok(finds(nfc, `Library/${nfd}/photo.jpg`));
  assert.ok(finds(nfd, `Library/${nfc}/photo.jpg`));
  assert.ok(finds("CAFÉ", `Library/${nfd}/photo.jpg`));
  assert.deepEqual(marked(`${nfc} and ${nfd}`, "café"), [nfc, nfd]);
});

test("unicode: code points, not code units", () => {
  const text = "chat 😀 log 😀";
  assert.ok(finds("😀", text));
  assert.deepEqual(marked(text, "😀"), ["😀", "😀"]);
  // Half of a surrogate pair never matches inside a pair.
  assert.ok(!finds("\ud83d", text));
  assert.ok(!finds("\ude00", text));
  // A lone surrogate in the line (broken text) is still found by itself.
  assert.ok(finds("\ud83d", "broken \ud83d text"));
  // Astral case folding: Deseret capital and small letters.
  assert.ok(finds("\u{10400}", "x\u{10428}y"));
});

// ---- Highlighting parts ----

test("parts join back to the line, matches in order", () => {
  const re = compileQuery("sms", true);
  assert.ok(re);
  const line = "SMS: parsed sms.db (sms)";
  const parts = splitMatches(line, re);
  assert.equal(parts.map((p) => p.text).join(""), line);
  assert.deepEqual(parts, [
    { text: "SMS", match: true },
    { text: ": parsed ", match: false },
    { text: "sms", match: true },
    { text: ".db (", match: false },
    { text: "sms", match: true },
    { text: ")", match: false },
  ]);
});

test("parts keep the original text around length-changing characters", () => {
  // "İ".toLowerCase() is two code units long; offsets must still be the original's.
  const line = "İstanbul İzmir 😀 izmir";
  const re = compileQuery("IZMIR", true);
  assert.ok(re);
  const parts = splitMatches(line, re);
  assert.equal(parts.map((p) => p.text).join(""), line);
  assert.deepEqual(
    parts.filter((p) => p.match).map((p) => p.text),
    ["izmir"],
  );
  assert.deepEqual(marked(line, "stanbul"), ["stanbul"]);
});

test("adjacent and repeated matches do not overlap", () => {
  assert.deepEqual(marked("aaaaa", "aa"), ["aa", "aa"]);
  const re = compileQuery("aa", true);
  assert.ok(re);
  assert.deepEqual(splitMatches("aaaaa", re), [
    { text: "aa", match: true },
    { text: "aa", match: true },
    { text: "a", match: false },
  ]);
});

test("no match: one plain part; an empty line: no parts", () => {
  const re = compileQuery("zzz", true);
  assert.ok(re);
  assert.deepEqual(splitMatches("nothing here", re), [{ text: "nothing here", match: false }]);
  assert.deepEqual(splitMatches("", re), []);
});

test("after the limit, the rest of the line is one plain part", () => {
  const re = compileQuery("a", true);
  assert.ok(re);
  const line = "a".repeat(10_000);
  const parts = splitMatches(line, re, 3);
  assert.equal(parts.filter((p) => p.match).length, 3);
  assert.deepEqual(parts[3], { text: "a".repeat(9_997), match: false });
  assert.equal(parts.length, 4);
});

test("the RegExp is left reusable (lastIndex reset)", () => {
  const re = compileQuery("x", true);
  assert.ok(re);
  splitMatches("x x x", re, 1);
  assert.equal(re.lastIndex, 0);
  assert.equal(splitMatches("x", re).length, 1);
});

test("markup in a line stays text in the parts", () => {
  const re = compileQuery("b", true);
  assert.ok(re);
  const line = '<img src=x onerror="alert(1)"><b>bold</b>';
  const parts = splitMatches(line, re);
  assert.equal(parts.map((p) => p.text).join(""), line);
  assert.ok(parts.every((p) => typeof p.text === "string"));
  assert.deepEqual(
    parts.filter((p) => p.match).map((p) => p.text),
    ["b", "b", "b"],
  );
});

// ---- Incremental matcher ----

test("the matcher finds the matching lines", () => {
  const log = { lines: ["sms started", "calls started", "SMS completed", "calls completed"], dropped: 0 };
  const m = logMatcher();
  assert.equal(m.setQuery("sms"), true);
  assert.equal(m.setQuery("sms"), false);
  m.sync(log);
  assert.deepEqual(m.matches(), [0, 2]);
  assert.equal(m.query(), "sms");
});

test("new lines are tested once, as they arrive", () => {
  const log = { lines: ["a match", "no"], dropped: 0 };
  const m = logMatcher();
  m.setQuery("match");
  m.sync(log);
  assert.deepEqual(m.matches(), [0]);
  // A line already tested is not tested again (the log view never rewrites lines): proven by
  // changing it behind the matcher's back.
  log.lines[1] = "match now";
  appendLog(log, ["match 3", "x", "match 5"]);
  m.sync(log);
  assert.deepEqual(m.matches(), [0, 2, 4]);
  m.sync(log);
  assert.deepEqual(m.matches(), [0, 2, 4]);
});

test("lines dropped from the buffer are forgotten; indexes stay absolute", () => {
  const log = { lines: /** @type {string[]} */ ([]), dropped: 0 };
  const m = logMatcher();
  m.setQuery("hit");
  for (let batch = 0; batch < 30; batch++) {
    appendLog(
      log,
      Array.from({ length: 7 }, (_, k) => `line ${batch * 7 + k} ${(batch * 7 + k) % 3 === 0 ? "hit" : "miss"}`),
      50,
      10,
    );
    m.sync(log);
    assert.deepEqual(m.matches(), oracle(log, "hit"), `after batch ${batch}`);
  }
  assert.ok(log.dropped > 0);
  assert.ok(m.matches().every((line) => line >= log.dropped));
  assert.equal(m.matches()[0] % 3, 0);
});

test("lines dropped before they were searched are skipped", () => {
  const log = { lines: ["hit 0", "hit 1"], dropped: 0 };
  const m = logMatcher();
  m.setQuery("hit");
  m.sync(log);
  appendLog(log, Array.from({ length: 30 }, (_, k) => `hit ${k + 2}`), 20, 5);
  m.sync(log);
  assert.deepEqual(m.matches(), oracle(log, "hit"));
  assert.equal(m.matches()[0], log.dropped);
});

test("a new query, or a new buffer, searches from scratch", () => {
  const log = { lines: ["alpha", "beta", "gamma"], dropped: 0 };
  const m = logMatcher();
  m.setQuery("a");
  m.sync(log);
  assert.deepEqual(m.matches(), [0, 1, 2]);
  m.setQuery("ph");
  assert.deepEqual(m.matches(), []);
  m.sync(log);
  assert.deepEqual(m.matches(), [0]);
  const other = { lines: ["phase", "x"], dropped: 4 };
  m.sync(other);
  assert.deepEqual(m.matches(), [4]);
  m.setQuery("");
  m.sync(other);
  assert.deepEqual(m.matches(), []);
});

test("a buffer that shrank is searched again", () => {
  const log = { lines: ["hit", "hit", "hit"], dropped: 0 };
  const m = logMatcher();
  m.setQuery("hit");
  m.sync(log);
  log.lines.length = 1;
  m.sync(log);
  assert.deepEqual(m.matches(), [0]);
});

test("100,000 lines are searched quickly", () => {
  const lines = Array.from({ length: 100_000 }, (_, k) => `[${String(k + 1).padStart(6, "0")}] module${k % 1300}: parsed ${k % 97} records from /Volumes/Evidence/private/var/mobile/Library/module${k % 1300}.db`);
  const m = logMatcher();
  m.setQuery("PARSED 42 records");
  const t = performance.now();
  m.sync({ lines, dropped: 0 });
  const ms = performance.now() - t;
  assert.equal(m.matches().length, lines.filter((l) => l.includes("parsed 42 records")).length);
  // Generous for slow CI machines; typical is a few milliseconds.
  assert.ok(ms < 500, `took ${ms} ms`);
});

// ---- Navigation and rows ----

test("lowerBound", () => {
  assert.equal(lowerBound([], 5), 0);
  assert.equal(lowerBound([1, 3, 5], 0), 0);
  assert.equal(lowerBound([1, 3, 5], 3), 1);
  assert.equal(lowerBound([1, 3, 5], 4), 2);
  assert.equal(lowerBound([1, 3, 5], 6), 3);
});

test("next and previous from the current match wrap around", () => {
  const matches = [10, 20, 30];
  const view = { first: 0, last: 5 };
  assert.equal(stepMatch(matches, 10, 1, view), 20);
  assert.equal(stepMatch(matches, 30, 1, view), 10);
  assert.equal(stepMatch(matches, 20, -1, view), 10);
  assert.equal(stepMatch(matches, 10, -1, view), 30);
  // A current line that is no longer a match (dropped) still steps from its position.
  assert.equal(stepMatch(matches, 15, 1, view), 20);
  assert.equal(stepMatch(matches, 15, -1, view), 10);
  assert.equal(stepMatch(matches, 5, -1, view), 30);
});

test("without a current match, navigation starts from the view", () => {
  const matches = [10, 20, 30];
  assert.equal(stepMatch(matches, -1, 1, { first: 15, last: 34 }), 20);
  assert.equal(stepMatch(matches, -1, 1, { first: 20, last: 39 }), 20);
  assert.equal(stepMatch(matches, -1, 1, { first: 31, last: 50 }), 10);
  assert.equal(stepMatch(matches, -1, -1, { first: 0, last: 25 }), 20);
  assert.equal(stepMatch(matches, -1, -1, { first: 0, last: 30 }), 30);
  assert.equal(stepMatch(matches, -1, -1, { first: 0, last: 5 }), 30);
  // Nothing in view (an empty list).
  assert.equal(stepMatch(matches, -1, 1, { first: 0, last: -1 }), 10);
});

test("no matches: nowhere to go", () => {
  assert.equal(stepMatch([], -1, 1, { first: 0, last: 10 }), -1);
  assert.equal(stepMatch([], 4, -1, { first: 0, last: 10 }), -1);
});

test("the position of a match", () => {
  assert.equal(matchPosition([10, 20, 30], 10), 1);
  assert.equal(matchPosition([10, 20, 30], 30), 3);
  assert.equal(matchPosition([10, 20, 30], 25), 0);
  assert.equal(matchPosition([], 0), 0);
});

test("rows and lines, with and without the filter", () => {
  // All lines: 100 dropped, 50 kept.
  assert.equal(rowOfLine(120, 100, 50, null), 20);
  assert.equal(lineOfRow(20, 100, null), 120);
  assert.equal(rowOfLine(10, 100, 50, null), 0);
  assert.equal(rowOfLine(500, 100, 50, null), 49);
  // Only matching lines: the row of the first match at or after the line.
  const matches = [105, 120, 140];
  assert.equal(rowOfLine(120, 100, 50, matches), 1);
  assert.equal(rowOfLine(121, 100, 50, matches), 2);
  assert.equal(rowOfLine(141, 100, 50, matches), 2);
  assert.equal(lineOfRow(2, 100, matches), 140);
  assert.equal(rowOfLine(3, 0, 0, []), 0);
});

// ---- Status ----

test("the status: no query, no match, a count, a current match", () => {
  const whole = { kept: 100_090, partial: false };
  assert.equal(searchStatus({ query: "", matches: 0, position: 0, ...whole }), "");
  assert.equal(searchStatus({ query: "Traceback", matches: 0, position: 0, ...whole }), "No matching lines");
  assert.equal(searchStatus({ query: "sms", matches: 1, position: 0, ...whole }), "1 matching line");
  assert.equal(searchStatus({ query: "sms", matches: 1, position: 1, ...whole }), "1 of 1 matching line");
  assert.equal(searchStatus({ query: "safari", matches: 3003, position: 0, ...whole }), "3,003 matching lines");
  assert.equal(searchStatus({ query: "safari", matches: 3003, position: 3, ...whole }), "3 of 3,003 matching lines");
});

test("the status says which lines were searched when the view holds only part of the log", () => {
  // Past the buffer (lines dropped), or only the backlog after a reload.
  assert.equal(searchStatus({ query: "Traceback", matches: 0, position: 0, kept: 180_000, partial: true }), "No matching lines in the 180,000 lines kept here");
  assert.equal(searchStatus({ query: "sms", matches: 12, position: 0, kept: 2000, partial: true }), "12 matching lines in the 2,000 lines kept here");
  assert.equal(searchStatus({ query: "sms", matches: 12, position: 4, kept: 2000, partial: true }), "4 of 12 matching lines in the 2,000 lines kept here");
  assert.equal(searchStatus({ query: "x", matches: 1, position: 1, kept: 1, partial: true }), "1 of 1 matching line in the 1 line kept here");
  assert.equal(searchStatus({ query: "", matches: 0, position: 0, kept: 2000, partial: true }), "");
});

test("the query length the search box accepts", () => {
  assert.equal(MAX_QUERY_LENGTH, 256);
  // Even a query of that length over long repetitive lines stays a literal, linear-ish search.
  const re = compileQuery("a".repeat(MAX_QUERY_LENGTH - 1) + "b");
  assert.ok(re);
  assert.ok(!re.test("a".repeat(2000)));
});
