// @ts-check
// Device polling on the Acquire screen: every 2 s, never two calls in flight (D5).
import { test } from "node:test";
import assert from "node:assert/strict";

import { createPoller } from "../../ui/lib/poll.js";

/** Manual timers: `fire()` runs the pending timeouts in order. */
function fakeTimers() {
  let nextId = 1;
  /** @type {Map<number, { fn: () => void, ms: number }>} */
  const pending = new Map();
  return {
    /** @param {() => void} fn @param {number} ms */
    setTimeout: (fn, ms) => {
      const id = nextId++;
      pending.set(id, { fn, ms });
      return id;
    },
    /** @param {any} id */
    clearTimeout: (id) => {
      pending.delete(id);
    },
    fire() {
      const all = [...pending.entries()];
      pending.clear();
      for (const [, t] of all) t.fn();
      return all.length;
    },
    delays: () => [...pending.values()].map((t) => t.ms),
    size: () => pending.size,
  };
}

/** A fetch whose calls settle only when the test says so; counts concurrency. */
function controlledFetch() {
  /** @type {{ resolve: (v: number) => void, reject: (e: unknown) => void }[]} */
  const calls = [];
  let inFlight = 0;
  let maxInFlight = 0;
  let n = 0;
  return {
    fetch: () =>
      new Promise((resolve, reject) => {
        inFlight += 1;
        maxInFlight = Math.max(maxInFlight, inFlight);
        n += 1;
        calls.push({
          resolve: (v) => {
            inFlight -= 1;
            resolve(v);
          },
          reject: (e) => {
            inFlight -= 1;
            reject(e);
          },
        });
      }),
    /** @param {number} [value] */
    settle(value = n) {
      const call = calls.shift();
      assert.ok(call, "no call in flight");
      call.resolve(value);
    },
    /** @param {unknown} err */
    fail(err) {
      const call = calls.shift();
      assert.ok(call, "no call in flight");
      call.reject(err);
    },
    count: () => n,
    inFlight: () => inFlight,
    maxInFlight: () => maxInFlight,
  };
}

/** Lets settled promises run their continuations. */
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

test("polls at once, then every interval, and only after the previous call settled", async () => {
  const timers = fakeTimers();
  const f = controlledFetch();
  /** @type {number[]} */
  const results = [];
  const p = createPoller({ fetch: f.fetch, intervalMs: 2000, onResult: (r) => results.push(r), timers });
  p.start();
  assert.equal(f.count(), 1);
  assert.equal(timers.size(), 0, "no timer while a call is in flight");
  f.settle(1);
  await flush();
  assert.deepEqual(results, [1]);
  assert.deepEqual(timers.delays(), [2000]);
  timers.fire();
  assert.equal(f.count(), 2);
  f.settle(2);
  await flush();
  assert.deepEqual(results, [1, 2]);
});

test("single flight: however slow the device list is, there is never a second call in flight", async () => {
  const timers = fakeTimers();
  const f = controlledFetch();
  const p = createPoller({ fetch: f.fetch, intervalMs: 2000, onResult: () => {}, timers });
  p.start();
  for (let i = 0; i < 20; i++) {
    // Timers firing, refreshes and restarts while a call hangs start nothing new.
    timers.fire();
    p.refresh();
    p.start();
    assert.equal(f.inFlight(), 1);
    if (i % 3 === 2) {
      f.settle();
      await flush();
    }
  }
  assert.equal(f.maxInFlight(), 1);
});

test("refresh while a call is in flight polls once more right after it; invalidate drops its result", async () => {
  const timers = fakeTimers();
  const f = controlledFetch();
  /** @type {number[]} */
  const results = [];
  const p = createPoller({ fetch: f.fetch, intervalMs: 2000, onResult: (r) => results.push(r), timers });
  p.start();
  p.refresh();
  p.refresh();
  f.settle(1);
  await flush();
  assert.equal(f.count(), 2, "exactly one follow-up call");
  assert.equal(timers.size(), 0);
  // A newer answer (device_pair) arrived while call 2 was in flight: its result is stale.
  p.invalidate();
  f.settle(2);
  await flush();
  assert.deepEqual(results, [1]);
  assert.equal(f.count(), 3, "and the list is read again at once");
  f.settle(3);
  await flush();
  assert.deepEqual(results, [1, 3]);
});

test("stop cancels the timer and drops the result of the call in flight; start resumes", async () => {
  const timers = fakeTimers();
  const f = controlledFetch();
  /** @type {number[]} */
  const results = [];
  const p = createPoller({ fetch: f.fetch, intervalMs: 2000, onResult: (r) => results.push(r), timers });
  p.start();
  f.settle(1);
  await flush();
  assert.equal(timers.size(), 1);
  p.stop();
  assert.equal(timers.size(), 0);
  assert.equal(timers.fire(), 0);
  assert.equal(f.count(), 1, "hidden: no polling");
  p.start();
  assert.equal(f.count(), 2);
  p.stop();
  f.settle(2);
  await flush();
  assert.deepEqual(results, [1], "a result that arrives after stop is dropped");
  assert.equal(timers.size(), 0);
  // Started again while an old call is still in flight: polls right after it, never alongside it.
  p.start();
  const before = f.count();
  p.stop();
  p.start();
  assert.equal(f.count(), before);
});

test("errors are reported and polling goes on", async () => {
  const timers = fakeTimers();
  const f = controlledFetch();
  /** @type {unknown[]} */
  const errors = [];
  const p = createPoller({ fetch: f.fetch, intervalMs: 2000, onResult: () => {}, onError: (e) => errors.push(e), timers });
  p.start();
  f.fail({ code: "io", message: "x", detail: null });
  await flush();
  assert.equal(errors.length, 1);
  assert.deepEqual(timers.delays(), [2000]);
  assert.equal(p.busy(), false);
});
