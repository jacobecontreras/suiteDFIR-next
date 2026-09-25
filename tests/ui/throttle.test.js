// @ts-check
// The aria-live latest-line announcer is throttled to at most one update per second (D4a, §4.6).
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
