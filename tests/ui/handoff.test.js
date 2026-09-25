// @ts-check
// The "Parse with iLEAPP" password handoff is cleared as soon as it cannot be used (D5, CONTRACTS.md §13.5).
import { test } from "node:test";
import assert from "node:assert/strict";

import { createHandoff } from "../../ui/lib/handoff.js";

test("New run takes the password once; nothing else can read it afterwards", () => {
  const h = createHandoff();
  h.hold("a1", "s3cret");
  assert.equal(h.has("a1"), true);
  assert.equal(h.take("other"), null, "another acquisition's New run gets nothing");
  assert.equal(h.take("a1"), "s3cret");
  assert.equal(h.has("a1"), false);
  assert.equal(h.take("a1"), null);
});

test("an acquisition that does not succeed drops the password (there is nothing to parse)", () => {
  for (const status of /** @type {const} */ (["failed", "cancelled", "interrupted"])) {
    const h = createHandoff();
    h.hold("a1", "s3cret");
    const unwatch = h.watch("a1");
    h.finished("a1", status);
    assert.equal(h.has("a1"), false, status);
    unwatch();
  }
});

test("a success keeps it while its result is on screen; leaving the result drops it", () => {
  const h = createHandoff();
  h.hold("a1", "s3cret");
  const unwatch = h.watch("a1");
  h.finished("a1", "succeeded");
  assert.equal(h.has("a1"), true);
  unwatch();
  assert.equal(h.has("a1"), false);
  unwatch();
});

test("leaving the Acquire screen during the backup keeps it; finishing unseen drops it", () => {
  const h = createHandoff();
  h.hold("a1", "s3cret");
  const unwatch = h.watch("a1");
  unwatch();
  assert.equal(h.has("a1"), true, "the backup still runs; the examiner may come back");
  h.finished("a1", "succeeded");
  assert.equal(h.has("a1"), false, "finished while no screen showed it");
});

test("coming back to the result keeps it until that screen closes too", () => {
  const h = createHandoff();
  h.hold("a1", "s3cret");
  h.watch("a1")();
  const again = h.watch("a1");
  h.finished("a1", "succeeded");
  assert.equal(h.take("a1"), "s3cret");
  again();
});

test("a new acquisition replaces it, and dismissing the result drops it", () => {
  const h = createHandoff();
  h.hold("a1", "first");
  h.hold("a2", "second");
  assert.equal(h.has("a1"), false);
  assert.equal(h.take("a1"), null);
  h.finished("a1", "failed");
  assert.equal(h.has("a2"), true, "an old acquisition's end does not touch the new password");
  h.drop();
  assert.equal(h.has("a2"), false);
});
