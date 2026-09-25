// @ts-check
// Virtual-list math for the log view (ROADMAP D4a).
import { test } from "node:test";
import assert from "node:assert/strict";

import { bottomScrollTop, isAtBottom, poolSize, visibleRange } from "../../ui/lib/virtual.js";

const ROW = 20;

test("an empty list renders nothing", () => {
  assert.deepEqual(visibleRange({ count: 0, rowHeight: ROW, scrollTop: 0, viewportHeight: 400 }), { start: 0, end: 0, offset: 0, total: 0 });
  assert.deepEqual(visibleRange({ count: 10, rowHeight: 0, scrollTop: 0, viewportHeight: 400 }), { start: 0, end: 0, offset: 0, total: 0 });
});

test("at the top: the rows that fit, plus a partial row", () => {
  assert.deepEqual(visibleRange({ count: 100_000, rowHeight: ROW, scrollTop: 0, viewportHeight: 380 }), {
    start: 0,
    end: 19,
    offset: 0,
    total: 2_000_000,
  });
  // 390 px shows 19.5 rows: row 19 is partly visible and rendered.
  assert.equal(visibleRange({ count: 100_000, rowHeight: ROW, scrollTop: 0, viewportHeight: 390 }).end, 20);
});

test("in the middle, with overscan on both sides", () => {
  const r = visibleRange({ count: 100_000, rowHeight: ROW, scrollTop: 1_000_010, viewportHeight: 380, overscan: 10 });
  // Row 50,000 starts at 1,000,000 px; 1,000,010 is half-way into it.
  assert.deepEqual(r, { start: 49_990, end: 50_030, offset: 49_990 * ROW, total: 2_000_000 });
});

test("at the end: the last page, overscan clipped to the list", () => {
  const top = bottomScrollTop({ count: 100_000, rowHeight: ROW, viewportHeight: 380 });
  assert.equal(top, 2_000_000 - 380);
  const r = visibleRange({ count: 100_000, rowHeight: ROW, scrollTop: top, viewportHeight: 380, overscan: 10 });
  assert.equal(r.end, 100_000);
  assert.equal(r.start, 100_000 - 19 - 10);
});

test("a scroll position past the end (the list shrank) is clamped to the last page", () => {
  const r = visibleRange({ count: 50, rowHeight: ROW, scrollTop: 50_000, viewportHeight: 200 });
  assert.deepEqual(r, { start: 40, end: 50, offset: 800, total: 1000 });
  const negative = visibleRange({ count: 50, rowHeight: ROW, scrollTop: -30, viewportHeight: 200 });
  assert.equal(negative.start, 0);
  const nan = visibleRange({ count: 50, rowHeight: ROW, scrollTop: Number.NaN, viewportHeight: 200 });
  assert.equal(nan.start, 0);
});

test("a list shorter than the viewport renders every row and cannot scroll", () => {
  assert.deepEqual(visibleRange({ count: 3, rowHeight: ROW, scrollTop: 0, viewportHeight: 380, overscan: 10 }), { start: 0, end: 3, offset: 0, total: 60 });
  assert.equal(bottomScrollTop({ count: 3, rowHeight: ROW, viewportHeight: 380 }), 0);
  assert.equal(isAtBottom({ count: 3, rowHeight: ROW, scrollTop: 0, viewportHeight: 380 }), true);
});

test("a zero-height (hidden) viewport still renders one row", () => {
  assert.deepEqual(visibleRange({ count: 10, rowHeight: ROW, scrollTop: 0, viewportHeight: 0 }), { start: 0, end: 1, offset: 0, total: 200 });
});

test("isAtBottom allows half a row of rounding, not more", () => {
  const v = { count: 1000, rowHeight: ROW, viewportHeight: 400 };
  const bottom = bottomScrollTop(v);
  assert.equal(isAtBottom({ ...v, scrollTop: bottom }), true);
  assert.equal(isAtBottom({ ...v, scrollTop: bottom - 9.5 }), true);
  assert.equal(isAtBottom({ ...v, scrollTop: bottom - 11 }), false);
  assert.equal(isAtBottom({ ...v, scrollTop: 0 }), false);
});

test("the number of row elements depends on the viewport only, not on the line count", () => {
  assert.equal(poolSize(380, ROW, 10), 19 + 1 + 20);
  assert.equal(poolSize(0, ROW, 0), 1);
  assert.equal(poolSize(380, 0, 10), 0);
  for (const count of [100, 100_000, 1_000_000]) {
    for (const scrollTop of [0, 12_345, count * ROW]) {
      const r = visibleRange({ count, rowHeight: ROW, scrollTop, viewportHeight: 380, overscan: 10 });
      assert.ok(r.end - r.start <= poolSize(380, ROW, 10), `count ${count}, scrollTop ${scrollTop}`);
      assert.equal(r.offset, r.start * ROW);
    }
  }
});
