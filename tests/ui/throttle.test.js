// @ts-check
// The aria-live latest-line announcer is throttled to at most one update per second (D4a, §4.6);
// the log search's reports (S2) go through the same limit, ahead of the latest line.
import { test } from "node:test";
import assert from "node:assert/strict";

import { throttleLatest } from "../../ui/lib/throttle.js";

/** A manual clock: time moves only with `advance`, which runs the timers that come due. */
function fakeClock() {
  let now = 0;
  let nextId = 1;
  /** @type {Map<number, { at: number, fn: () => void }>} */
  const timers = new Map();
  return {
    now: () => now,
    /** @param {() => void} fn @param {number} ms */
    setTimeout: (fn, ms) => {
      const id = nextId++;
      timers.set(id, { at: now + ms, fn });
      return id;
    },
    /** @param {any} id */
    clearTimeout: (id) => {
      timers.delete(id);
    },
    /** @param {number} ms */
    advance(ms) {
      const end = now + ms;
      for (;;) {
        const due = [...timers.entries()].filter(([, t]) => t.at <= end).sort((a, b) => a[1].at - b[1].at)[0];
        if (!due) break;
        timers.delete(due[0]);
        now = due[1].at;
        due[1].fn();
      }
      now = end;
    },
    pending: () => timers.size,
  };
}

test("the first value goes out at once; a burst delivers only its latest value, after the interval", () => {
  const clock = fakeClock();
  /** @type {[number, string][]} */
  const calls = [];
  const t = throttleLatest((/** @type {string} */ v) => calls.push([clock.now(), v]), 1000, clock);
  t.push("a");
  clock.advance(100);
  t.push("b");
  clock.advance(100);
  t.push("c");
  assert.deepEqual(calls, [[0, "a"]]);
  clock.advance(799);
  assert.deepEqual(calls, [[0, "a"]]);
  clock.advance(1);
  assert.deepEqual(calls, [
    [0, "a"],
    [1000, "c"],
  ]);
});

test("never more than one delivery per interval, however fast values arrive", () => {
  const clock = fakeClock();
  /** @type {number[]} */
  const times = [];
  const t = throttleLatest(() => times.push(clock.now()), 1000, clock);
  for (let i = 0; i < 500; i++) {
    t.push(i);
    clock.advance(10);
  }
  clock.advance(2000);
  assert.ok(times.length >= 5 && times.length <= 6, `delivered ${times.length} times`);
  for (let i = 1; i < times.length; i++) assert.ok(times[i] - times[i - 1] >= 1000, `gap ${times[i] - times[i - 1]} ms`);
});

test("after a quiet interval the next value goes out at once again", () => {
  const clock = fakeClock();
  /** @type {string[]} */
  const values = [];
  const t = throttleLatest((/** @type {string} */ v) => values.push(v), 1000, clock);
  t.push("a");
  clock.advance(5000);
  t.push("b");
  assert.deepEqual(values, ["a", "b"]);
  assert.equal(clock.pending(), 0);
});

test("cancel drops the pending value and its timer", () => {
  const clock = fakeClock();
  /** @type {string[]} */
  const values = [];
  const t = throttleLatest((/** @type {string} */ v) => values.push(v), 1000, clock);
  t.push("a");
  t.push("b");
  t.cancel();
  clock.advance(3000);
  assert.deepEqual(values, ["a"]);
  assert.equal(clock.pending(), 0);
});

// ---- Urgent values: the log search's reports share the region and the limit (S2) ----

test("an urgent value after a quiet interval goes out at once", () => {
  const clock = fakeClock();
  /** @type {[number, string][]} */
  const calls = [];
  const t = throttleLatest((/** @type {string} */ v) => calls.push([clock.now(), v]), 1000, clock);
  t.pushUrgent("3 matching lines");
  assert.deepEqual(calls, [[0, "3 matching lines"]]);
  assert.equal(clock.pending(), 0);
});

test("a waiting urgent value is not replaced by lines; the latest line follows one interval later", () => {
  const clock = fakeClock();
  /** @type {[number, string][]} */
  const calls = [];
  const t = throttleLatest((/** @type {string} */ v) => calls.push([clock.now(), v]), 1000, clock);
  t.push("line 1");
  clock.advance(200);
  t.pushUrgent("Match 1 of 3");
  clock.advance(100);
  t.push("line 2");
  clock.advance(100);
  t.push("line 3");
  clock.advance(1600);
  assert.deepEqual(calls, [
    [0, "line 1"],
    [1000, "Match 1 of 3"],
    [2000, "line 3"],
  ]);
  assert.equal(clock.pending(), 0);
});

test("a line waiting before the urgent value goes out after it", () => {
  const clock = fakeClock();
  /** @type {string[]} */
  const values = [];
  const t = throttleLatest((/** @type {string} */ v) => values.push(v), 1000, clock);
  t.push("line 1");
  t.push("line 2");
  t.pushUrgent("No matching lines");
  clock.advance(5000);
  assert.deepEqual(values, ["line 1", "No matching lines", "line 2"]);
});

test("a newer urgent value replaces a waiting one", () => {
  const clock = fakeClock();
  /** @type {string[]} */
  const values = [];
  const t = throttleLatest((/** @type {string} */ v) => values.push(v), 1000, clock);
  t.pushUrgent("Match 1 of 42");
  for (let i = 2; i <= 10; i++) {
    clock.advance(20);
    t.pushUrgent(`Match ${i} of 42`);
  }
  clock.advance(3000);
  assert.deepEqual(values, ["Match 1 of 42", "Match 10 of 42"]);
});

test("latest-line announcements resume once no urgent value waits; one delivery per interval in all", () => {
  const clock = fakeClock();
  /** @type {[number, string][]} */
  const calls = [];
  const t = throttleLatest((/** @type {string} */ v) => calls.push([clock.now(), v]), 1000, clock);
  // A streaming log (a line every 50 ms) with a search report now and then.
  for (let i = 0; i < 200; i++) {
    t.push(`line ${i}`);
    if (i === 30 || i === 31 || i === 90) t.pushUrgent(`report ${i}`);
    clock.advance(50);
  }
  clock.advance(3000);
  for (let i = 1; i < calls.length; i++) assert.ok(calls[i][0] - calls[i - 1][0] >= 1000, `gap ${calls[i][0] - calls[i - 1][0]} ms`);
  const values = calls.map(([, v]) => v);
  assert.deepEqual(
    values.filter((v) => v.startsWith("report")),
    ["report 31", "report 90"],
  );
  // Lines are announced before, between and after the reports, and the very last line at the end.
  const first = values.indexOf("report 31");
  const second = values.indexOf("report 90");
  assert.ok(first > 0 && values[first + 1].startsWith("line") && second > first + 1 && values[second + 1].startsWith("line"));
  assert.equal(values[values.length - 1], "line 199");
});

test("cancel also drops a waiting urgent value", () => {
  const clock = fakeClock();
  /** @type {string[]} */
  const values = [];
  const t = throttleLatest((/** @type {string} */ v) => values.push(v), 1000, clock);
  t.push("line");
  t.pushUrgent("report");
  t.push("line 2");
  t.cancel();
  clock.advance(5000);
  assert.deepEqual(values, ["line"]);
  assert.equal(clock.pending(), 0);
});
