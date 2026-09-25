// @ts-check
import { test } from "node:test";
import assert from "node:assert/strict";

import { createStore, watch } from "../../ui/lib/store.js";

test("get returns the state; set merges and notifies with the new and previous state", () => {
  const store = createStore({ a: 1, b: "x" });
  /** @type {[unknown, unknown][]} */
  const calls = [];
  store.subscribe((state, prev) => calls.push([state, prev]));
  store.set({ a: 2 });
  assert.deepEqual(store.get(), { a: 2, b: "x" });
  assert.deepEqual(calls, [[{ a: 2, b: "x" }, { a: 1, b: "x" }]]);
});

test("set without a change does not notify", () => {
  const store = createStore({ a: 1, list: /** @type {number[]} */ ([]) });
  let count = 0;
  store.subscribe(() => count++);
  store.set({ a: 1 });
  store.set({ list: store.get().list });
  assert.equal(count, 0);
  store.set({ list: [] });
  assert.equal(count, 1);
});

test("state is frozen and replaced, never mutated", () => {
  const store = createStore({ a: 1 });
  const before = store.get();
  store.set({ a: 2 });
  assert.equal(before.a, 1);
  assert.ok(Object.isFrozen(store.get()));
});

test("unsubscribe stops notifications, even during a notification", () => {
  const store = createStore({ a: 0 });
  /** @type {string[]} */
  const seen = [];
  const offA = store.subscribe(() => {
    seen.push("a");
    offA();
  });
  store.subscribe(() => seen.push("b"));
  store.set({ a: 1 });
  store.set({ a: 2 });
  assert.deepEqual(seen, ["a", "b", "b"]);
});

test("watch calls now and on changes of the selected value only", () => {
  const store = createStore({ job: /** @type {string | null} */ (null), other: 0 });
  /** @type {(string | null)[]} */
  const values = [];
  const off = watch(store, (s) => s.job, (v) => values.push(v));
  store.set({ other: 1 });
  store.set({ job: "run-1" });
  store.set({ job: "run-1", other: 2 });
  store.set({ job: null });
  off();
  store.set({ job: "run-2" });
  assert.deepEqual(values, [null, "run-1", null]);
});
