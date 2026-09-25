// @ts-check
// Router and AppError normalization.
import { test } from "node:test";
import assert from "node:assert/strict";

import { isAppError, toAppError } from "../../ui/lib/errors.js";
import { parseRoute, routeHref } from "../../ui/lib/router.js";

test("routes round-trip, including paths with slashes, spaces and plus signs", () => {
  const path = "/Users/examiner/Documents/suiteDFIR Cases/A+B (2)";
  const href = routeHref("case", { path });
  assert.ok(href.startsWith("#/case?path="));
  assert.deepEqual(parseRoute(href), { name: "case", params: { path } });
  assert.deepEqual(parseRoute(""), { name: "cases", params: {} });
  assert.deepEqual(parseRoute("#/"), { name: "cases", params: {} });
  assert.deepEqual(parseRoute("#/settings"), { name: "settings", params: {} });
  assert.deepEqual(parseRoute("#/new-run?case=C%3A%5Ccases%5CX"), { name: "new-run", params: { case: "C:\\cases\\X" } });
  assert.equal(routeHref("cases"), "#/cases");
});

test("toAppError passes AppErrors through and wraps anything else as internal", () => {
  const err = { code: "io", message: "disk full", detail: null };
  assert.equal(toAppError(err), err);
  assert.equal(isAppError(err), true);
  assert.equal(isAppError({ code: "io", message: "x" }), false);
  const wrapped = toAppError(new TypeError("boom"));
  assert.equal(wrapped.code, "internal");
  assert.equal(wrapped.message, "boom");
  assert.deepEqual(toAppError("text"), { code: "internal", message: "Unexpected error.", detail: "text" });
  assert.deepEqual(toAppError({ some: 1 }), { code: "internal", message: "Unexpected error.", detail: '{"some":1}' });
});
