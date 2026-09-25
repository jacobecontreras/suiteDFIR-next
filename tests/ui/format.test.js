// @ts-check
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  formatBytes,
  formatCount,
  formatDuration,
  formatLocalTime,
  formatUtcTime,
  parseTimestamp,
} from "../../ui/lib/format.js";

test("formatBytes uses binary units with one decimal below 100", () => {
  assert.equal(formatBytes(0), "0 B");
  assert.equal(formatBytes(1023), "1023 B");
  assert.equal(formatBytes(1024), "1.0 KiB");
  assert.equal(formatBytes(1536), "1.5 KiB");
  assert.equal(formatBytes(132120576), "126 MiB");
  assert.equal(formatBytes(61203455110), "57.0 GiB");
  assert.equal(formatBytes(1024 ** 4 * 3), "3.0 TiB");
});

test("formatBytes carries 1023.96 KiB into the next unit and rejects bad input", () => {
  assert.equal(formatBytes(1024 * 1024 - 1), "1.0 MiB");
  assert.equal(formatBytes(null), "—");
  assert.equal(formatBytes(undefined), "—");
  assert.equal(formatBytes(-1), "—");
  assert.equal(formatBytes(Number.NaN), "—");
});

test("formatCount groups thousands", () => {
  assert.equal(formatCount(1300), "1,300");
  assert.equal(formatCount(0), "0");
  assert.equal(formatCount(null), "—");
});

test("formatDuration picks ms, s, min+s or h+min+s", () => {
  assert.equal(formatDuration(0), "0 ms");
  assert.equal(formatDuration(850), "850 ms");
  assert.equal(formatDuration(1000), "1 s");
  assert.equal(formatDuration(59999), "59 s");
  assert.equal(formatDuration(60000), "1 min 0 s");
  assert.equal(formatDuration(1356000), "22 min 36 s");
  assert.equal(formatDuration(3600000), "1 h 00 min 00 s");
  assert.equal(formatDuration(2211000), "36 min 51 s");
  assert.equal(formatDuration(3725000), "1 h 02 min 05 s");
  assert.equal(formatDuration(null), "—");
  assert.equal(formatDuration(-5), "—");
});

test("parseTimestamp accepts RFC 3339 only", () => {
  assert.equal(parseTimestamp("2026-09-24T18:30:05Z")?.getTime(), Date.UTC(2026, 8, 24, 18, 30, 5));
  assert.equal(parseTimestamp("2026-09-24T18:30:05.123Z")?.getUTCMilliseconds(), 123);
  assert.equal(parseTimestamp("2026-09-24T13:30:05-05:00")?.getTime(), Date.UTC(2026, 8, 24, 18, 30, 5));
  assert.equal(parseTimestamp("2026-09-24"), null);
  assert.equal(parseTimestamp("yesterday"), null);
  assert.equal(parseTimestamp(null), null);
});

test("formatLocalTime shows the given zone, formatUtcTime shows UTC", () => {
  const iso = "2026-09-24T18:30:05Z";
  assert.equal(formatUtcTime(iso), "2026-09-24 18:30:05 UTC");
  assert.equal(formatLocalTime(iso, "UTC"), "2026-09-24 18:30:05");
  assert.equal(formatLocalTime(iso, "America/Chicago"), "2026-09-24 13:30:05");
  assert.equal(formatLocalTime(iso, "Asia/Tokyo"), "2026-09-25 03:30:05");
  assert.equal(formatLocalTime("2026-01-01T00:00:00Z", "Europe/Berlin"), "2026-01-01 01:00:00");
  // Midnight stays 00, never 24.
  assert.equal(formatLocalTime("2026-09-24T05:00:00Z", "America/Chicago"), "2026-09-24 00:00:00");
});

test("time formatting of missing or invalid values is a dash", () => {
  assert.equal(formatLocalTime(null), "—");
  assert.equal(formatUtcTime("not a time"), "—");
});

test("formatLocalTime without a zone uses the system zone", () => {
  const iso = "2026-09-24T18:30:05Z";
  const local = new Date(iso);
  const pad = (/** @type {number} */ n) => String(n).padStart(2, "0");
  const expected = `${local.getFullYear()}-${pad(local.getMonth() + 1)}-${pad(local.getDate())} ${pad(local.getHours())}:${pad(local.getMinutes())}:${pad(local.getSeconds())}`;
  assert.equal(formatLocalTime(iso), expected);
});
