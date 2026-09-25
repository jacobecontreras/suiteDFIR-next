// @ts-check
// Acquire screen logic: pair states and transitions, tools states, preflight levels, options (D5).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  PAIR_STATES,
  acqBlockers,
  buildAcqRequest,
  canAcquire,
  deviceState,
  encryptionOption,
  mergeDevice,
  needsEncryptionOff,
  passwordProblem,
  pickDevice,
  preflightInfo,
  toolsGuidance,
} from "../../ui/lib/acquire.js";
import { DeviceSummary } from "../../ui-dev/fixtures/contracts/index.js";

/** @typedef {import("../../ui/types").DeviceSummary} Device */
/** @typedef {import("../../ui/types").PairState} PairState */
/** @typedef {import("../../ui/lib/acquire.js").AcqForm} AcqForm */

/**
 * @param {Partial<Device>} [fields]
 * @returns {Device}
 */
const device = (fields = {}) => ({ ...structuredClone(DeviceSummary), ...fields });

/**
 * The values of an enumeration row in CONTRACTS.md.
 * @param {string} name
 */
function contractValues(name) {
  const doc = readFileSync(new URL("../../docs/CONTRACTS.md", import.meta.url), "utf8");
  const row = new RegExp(`^\\| \`${name}\` \\| (.+) \\|$`, "m").exec(doc);
  assert.ok(row, `${name} row not found`);
  return [...row[1].matchAll(/`([a-z_]+)`/g)].map((m) => m[1]);
}

test("every PairState of CONTRACTS.md §13.1 has instructions; pairing is offered only as an explicit action", () => {
  const states = contractValues("PairState");
  assert.equal(states.length, 7);
  assert.deepEqual(Object.keys(PAIR_STATES).sort(), [...states].sort());
  /** @type {Record<PairState, string | null>} */
  const actions = {
    paired: null,
    not_paired: "pair",
    awaiting_trust: "retry",
    locked: "retry",
    trust_denied: "retry",
    pairing_failed: "retry",
    unknown: "pair",
  };
  for (const state of states) {
    const info = deviceState(device({ pair_state: /** @type {PairState} */ (state) }));
    assert.equal(info.action, actions[/** @type {PairState} */ (state)], state);
    assert.ok(info.text.length > 20, state);
  }
  assert.match(PAIR_STATES.awaiting_trust.text, /Unlock the device and tap Trust/);
});

test("a busy device is in use by the current acquisition, whatever its pair state", () => {
  const busy = device({ busy: true });
  assert.equal(deviceState(busy).text, "In use by the current acquisition.");
  assert.equal(deviceState(busy).action, null);
  assert.equal(canAcquire(busy), false);
  assert.equal(canAcquire(device()), true);
  assert.equal(canAcquire(device({ pair_state: "locked" })), false);
});

test("pair-state transitions through Pair and Retry: not_paired → awaiting_trust → paired (with the mock)", async () => {
  /** @type {any} */ (globalThis).__SUITEDFIR_MOCK_TICK_MS = 1;
  const mock = await import("../../ui-dev/mock.js");
  const before = await mock.devices_list();
  const ipad = before.devices.find((d) => d.pair_state === "not_paired");
  assert.ok(ipad);
  assert.equal(deviceState(ipad).action, "pair");
  // Polling never pairs: many polls later the device is still not paired.
  for (let i = 0; i < 5; i++) {
    const again = await mock.devices_list();
    assert.equal(again.devices.find((d) => d.udid === ipad.udid)?.pair_state, "not_paired");
  }
  const first = await mock.device_pair({ udid: ipad.udid });
  assert.equal(first.pair_state, "awaiting_trust");
  assert.equal(deviceState(first).action, "retry");
  const second = await mock.device_pair({ udid: ipad.udid });
  assert.equal(second.pair_state, "paired");
  assert.equal(deviceState(second).action, null);
  const list = mergeDevice(before.devices, second);
  assert.deepEqual(
    list.map((d) => d.udid),
    before.devices.map((d) => d.udid),
    "the list keeps its order",
  );
  assert.equal(list.find((d) => d.udid === ipad.udid)?.pair_state, "paired");
  await assert.rejects(mock.device_pair({ udid: ipad.udid }), { code: "already_paired" });
});

test("device selection: keep the choice while it can be acquired, else the only ready device, else none", () => {
  const a = device({ udid: "a" });
  const b = device({ udid: "b" });
  const locked = device({ udid: "c", pair_state: "locked" });
  assert.equal(pickDevice([a, locked], null), "a");
  assert.equal(pickDevice([a, b], null), null, "two ready devices: the examiner chooses");
  assert.equal(pickDevice([a, b], "b"), "b");
  assert.equal(pickDevice([a, { ...b, busy: true }], "b"), "a", "the chosen device became busy");
  assert.equal(pickDevice([locked], "c"), null);
  assert.equal(pickDevice([], "a"), null, "the device was unplugged");
});

test("every tools state other than ok has guidance, per platform", () => {
  const states = contractValues("IdeviceToolsState");
  assert.equal(states.length, 5);
  assert.equal(toolsGuidance("ok", "macos"), null);
  for (const state of states.filter((s) => s !== "ok")) {
    for (const os of ["macos", "windows", "linux"]) {
      const g = toolsGuidance(/** @type {import("../../ui/types").IdeviceToolsState} */ (state), os);
      assert.ok(g && g.title && g.text, `${state} on ${os}`);
    }
  }
  assert.match(toolsGuidance("usbmuxd_unavailable", "windows")?.text ?? "", /Apple Devices/);
  assert.match(toolsGuidance("missing", "linux")?.text ?? "", /libimobiledevice-utils/);
  assert.match(toolsGuidance("usbmuxd_unavailable", "linux")?.text ?? "", /usbmuxd/);
});

test("preflight levels: ok, warn and block banners with free and required space", () => {
  const GiB = 1024 ** 3;
  const ok = preflightInfo({ free_bytes: 400 * GiB, required_bytes: 57 * GiB, level: "ok" });
  assert.equal(ok.tone, "ok");
  assert.match(ok.text, /400 GiB free.*57\.0 GiB/);
  const warn = preflightInfo({ free_bytes: 60 * GiB, required_bytes: 57 * GiB, level: "warn" });
  assert.equal(warn.tone, "warn");
  assert.match(warn.text, /may not fit/);
  const block = preflightInfo({ free_bytes: 20 * GiB, required_bytes: 57 * GiB, level: "block" });
  assert.equal(block.tone, "danger");
  assert.match(block.title, /Not enough/);
  assert.match(preflightInfo({ free_bytes: GiB, required_bytes: null, level: "ok" }).text, /unknown/);
});

/**
 * A complete form: a paired, unencrypted device, preflight ok, no encryption change.
 * @param {Partial<AcqForm>} [fields]
 * @returns {AcqForm}
 */
const form = (fields = {}) => ({
  toolsOk: true,
  device: device(),
  preflight: { free_bytes: 4e11, required_bytes: 6e10, level: "ok" },
  label: "",
  enableEncryption: false,
  password: "",
  password2: "",
  restoreEncryption: true,
  jobActive: false,
  ...fields,
});

test("option validation: tools, device, preflight, passwords and the one-job rule block Start", () => {
  assert.deepEqual(acqBlockers(form()), []);
  assert.deepEqual(acqBlockers(form({ toolsOk: false, device: null })), ["Fix the iOS tools problem shown above.", "Connect a device, pair it and select it."]);
  assert.deepEqual(acqBlockers(form({ preflight: "loading" })), ["Wait for the free-space check."]);
  assert.deepEqual(acqBlockers(form({ preflight: "failed" })), ["The free-space check failed (see above). Select the device again to retry."]);
  assert.deepEqual(acqBlockers(form({ preflight: { free_bytes: 1, required_bytes: 6e10, level: "block" } })), ["Free up space: the backup does not fit in the case folder."]);
  assert.deepEqual(acqBlockers(form({ preflight: { free_bytes: 4e10, required_bytes: 6e10, level: "warn" } })), [], "warn does not block");
  assert.deepEqual(acqBlockers(form({ enableEncryption: true, password: "abc", password2: "abc" })), ["Enter a backup password of at least 4 characters."]);
  assert.deepEqual(acqBlockers(form({ enableEncryption: true, password: "abcd", password2: "abce" })), ["Enter the same password in both fields."]);
  assert.deepEqual(acqBlockers(form({ enableEncryption: true, password: "abcd", password2: "abcd" })), []);
  assert.deepEqual(acqBlockers(form({ jobActive: true })), ["Another job is running. Start the acquisition after it ends."]);
  assert.equal(passwordProblem({ password: "", password2: "" }), "Enter a backup password of at least 4 characters.");
});

test("encryption option variants: offered when off, a warning when already on, nothing when unknown", () => {
  assert.equal(encryptionOption(device({ will_encrypt: false })), "offer");
  assert.equal(encryptionOption(device({ will_encrypt: true })), "already_on");
  assert.equal(encryptionOption(device({ will_encrypt: null })), "unknown");
  // A ticked box with stale passwords does not block or send anything when encryption is not offered.
  const on = form({ device: device({ will_encrypt: true }), enableEncryption: true, password: "x" });
  assert.deepEqual(acqBlockers(on), []);
  assert.deepEqual(buildAcqRequest("/c", on), {
    case_path: "/c",
    udid: DeviceSummary.udid,
    label: null,
    enable_encryption: false,
    encryption_password: null,
    restore_encryption: false,
  });
});

test("buildAcqRequest: encryption with restore by default, the password only when enabling, a trimmed label", () => {
  const req = buildAcqRequest("/c", form({ enableEncryption: true, password: "s3cret", password2: "s3cret", label: "  Item 7 " }));
  assert.deepEqual(req, {
    case_path: "/c",
    udid: DeviceSummary.udid,
    label: "Item 7",
    enable_encryption: true,
    encryption_password: "s3cret",
    restore_encryption: true,
  });
  assert.equal(buildAcqRequest("/c", form({ enableEncryption: true, password: "s3cret", password2: "s3cret", restoreEncryption: false })).restore_encryption, false);
  const plain = buildAcqRequest("/c", form({ password: "left over", password2: "left over" }));
  assert.equal(plain.encryption_password, null);
  assert.equal(plain.enable_encryption, false);
  assert.throws(() => buildAcqRequest("/c", form({ device: null })));
});

test("'Turn backup encryption off' applies to encryption_left_enabled and encryption_state_unknown only", () => {
  assert.equal(needsEncryptionOff(["encryption_restore_failed", "encryption_left_enabled"]), true);
  assert.equal(needsEncryptionOff(["encryption_state_unknown"]), true);
  assert.equal(needsEncryptionOff(["encryption_restore_failed"]), false);
  assert.equal(needsEncryptionOff(["backup_encryption_preexisting", "seal_cancelled"]), false);
  assert.equal(needsEncryptionOff([]), false);
});
