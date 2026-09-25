// @ts-check
import { test } from "node:test";
import assert from "node:assert/strict";

import { buildRunRequest, canHash, initialInputType, needsPassword, passwordHint, passwordReason, startBlockers } from "../../ui/lib/newrun.js";

/** @typedef {import("../../ui/lib/newrun.js").NewRunForm} NewRunForm */
/** @typedef {import("../../ui/types").InputInspection} InputInspection */

/** @type {InputInspection} */
const ENCRYPTED_BACKUP = {
  path: "/ev/backup",
  kind: "directory",
  size_bytes: null,
  detected_type: "itunes",
  allowed_types: ["fs", "itunes"],
  is_itunes_backup: true,
  itunes_encrypted: true,
  hashable: false,
  warnings: [],
};

/** @type {InputInspection} */
const ZIP_FILE = {
  path: "/ev/ffs.zip",
  kind: "file",
  size_bytes: 1000,
  detected_type: "zip",
  allowed_types: ["tar", "zip", "gz", "file", "raw"],
  is_itunes_backup: false,
  itunes_encrypted: null,
  hashable: true,
  warnings: [],
};

/**
 * A complete, valid form (iLEAPP, a zip file, all modules).
 * @param {Partial<NewRunForm>} [fields]
 * @returns {NewRunForm}
 */
const form = (fields = {}) => ({
  tool: "ileapp",
  toolsInstalled: true,
  inputPath: "/ev/ffs.zip",
  inspection: ZIP_FILE,
  inspecting: false,
  inputFailed: false,
  inputType: "zip",
  timezone: "America/Chicago",
  password: "",
  keychainPath: null,
  hashInput: true,
  label: "",
  modulesLoaded: true,
  moduleMode: "all",
  profile: null,
  customModules: [],
  customUnknown: [],
  jobActive: false,
  ...fields,
});

test("a complete form has no blockers", () => {
  assert.deepEqual(startBlockers(form()), []);
});

test("an empty form lists every missing step, in form order", () => {
  assert.deepEqual(
    startBlockers(form({ inputPath: null, inspection: null, inputType: null, timezone: null })),
    ["Choose an input file or folder.", "Choose a timezone."],
  );
  assert.deepEqual(startBlockers(form({ toolsInstalled: false, tool: null, inputPath: null, inspection: null })), [
    "Install iLEAPP or aLEAPP in Settings.",
    "Choose an input file or folder.",
  ]);
});

test("input problems block: checking, failed inspection, no type, a type not allowed", () => {
  assert.deepEqual(startBlockers(form({ inspecting: true, inspection: null })), ["Wait for the input check to finish."]);
  assert.deepEqual(startBlockers(form({ inputFailed: true, inspection: null })), ["Choose a different input (see the problem above)."]);
  assert.deepEqual(startBlockers(form({ inputType: null })), ["Choose the input type."]);
  assert.deepEqual(startBlockers(form({ inputType: "itunes" })), ["Choose an input type allowed for this input."]);
});

test("an encrypted backup needs a password with iLEAPP only", () => {
  const encrypted = form({ inputPath: "/ev/backup", inspection: ENCRYPTED_BACKUP, inputType: "itunes" });
  assert.equal(needsPassword(encrypted), true);
  assert.deepEqual(startBlockers(encrypted), ["Enter the backup password (the backup is encrypted)."]);
  assert.deepEqual(startBlockers({ ...encrypted, password: "secret" }), []);
  assert.equal(needsPassword({ ...encrypted, tool: "aleapp" }), false);
  assert.equal(needsPassword(form()), false);
  // Read as a plain folder, the backup needs none.
  assert.equal(needsPassword({ ...encrypted, inputType: "fs" }), false);
  assert.equal(passwordReason(encrypted), "encrypted");
  assert.match(passwordHint("encrypted"), /^The backup is encrypted\. /);
});

test("a backup whose encryption cannot be read needs a password as if encrypted", () => {
  const unknown = form({
    inputPath: "/ev/backup",
    inspection: { ...ENCRYPTED_BACKUP, itunes_encrypted: null, warnings: ["Backup encryption is unknown: Manifest.plist has no IsEncrypted"] },
    inputType: "itunes",
  });
  assert.equal(passwordReason(unknown), "unknown");
  assert.equal(needsPassword(unknown), true);
  assert.deepEqual(startBlockers(unknown), ["Enter the backup password (the backup's encryption state couldn't be read)."]);
  assert.deepEqual(startBlockers({ ...unknown, password: "secret" }), []);
  assert.match(passwordHint("unknown"), /^The backup's encryption state couldn't be read, so a password is needed\. /);
  assert.equal(buildRunRequest("/cases/A", { ...unknown, password: "secret" }).itunes_password, "secret");
  // Not for aLEAPP, nor as a plain folder.
  assert.equal(needsPassword({ ...unknown, tool: "aleapp" }), false);
  assert.equal(needsPassword({ ...unknown, inputType: "fs" }), false);
});

test("an unencrypted backup, or a folder that is not a backup read as iTunes, needs no password", () => {
  const plain = form({ inputPath: "/ev/backup", inspection: { ...ENCRYPTED_BACKUP, itunes_encrypted: false }, inputType: "itunes" });
  assert.equal(passwordReason(plain), null);
  assert.deepEqual(startBlockers(plain), []);
  const folder = form({
    inputPath: "/ev/folder",
    inspection: { ...ENCRYPTED_BACKUP, detected_type: "fs", is_itunes_backup: false, itunes_encrypted: null },
    inputType: "itunes",
  });
  assert.equal(passwordReason(folder), null);
  assert.deepEqual(startBlockers(folder), []);
  assert.equal(buildRunRequest("/cases/A", { ...folder, password: "left over" }).itunes_password, null);
});

test("aLEAPP needs no timezone", () => {
  assert.deepEqual(startBlockers(form({ tool: "aleapp", timezone: null })), []);
});

test("module selection blockers: loading, profile, unknown modules, empty custom selection", () => {
  assert.deepEqual(startBlockers(form({ modulesLoaded: false })), ["Wait for the module list to load."]);
  assert.deepEqual(startBlockers(form({ moduleMode: "profile" })), ["Choose a profile."]);
  const profile = { tool: /** @type {const} */ ("ileapp"), name: "Messaging", modules: ["sms", "noSuch"], unknown_modules: ["noSuch"] };
  assert.deepEqual(startBlockers(form({ moduleMode: "profile", profile })), [
    "Profile “Messaging” has 1 unknown module: edit it as a custom selection and remove it.",
  ]);
  assert.deepEqual(startBlockers(form({ moduleMode: "profile", profile: { ...profile, unknown_modules: ["x", "y"] } })), [
    "Profile “Messaging” has 2 unknown modules: edit it as a custom selection and remove them.",
  ]);
  assert.deepEqual(startBlockers(form({ moduleMode: "profile", profile: { ...profile, unknown_modules: [] } })), []);
  assert.deepEqual(startBlockers(form({ moduleMode: "custom" })), ["Select at least one module."]);
  assert.deepEqual(startBlockers(form({ moduleMode: "custom", customModules: ["a", "b"], customUnknown: ["b"] })), ["Remove 1 unknown module."]);
  assert.deepEqual(startBlockers(form({ moduleMode: "custom", customModules: ["x", "y"], customUnknown: ["x", "y"] })), [
    "Select at least one module.",
    "Remove 2 unknown modules.",
  ]);
  assert.deepEqual(startBlockers(form({ moduleMode: "custom", customModules: ["a"] })), []);
});

test("an active job blocks", () => {
  assert.deepEqual(startBlockers(form({ jobActive: true })), ["Another job is running. Start this run after it ends."]);
});

test("buildRunRequest: iLEAPP file input with hashing, custom modules, trimmed label", () => {
  const req = buildRunRequest("/cases/A", form({ moduleMode: "custom", customModules: ["sms"], label: "  Item 3 ", keychainPath: "/ev/kc.plist" }));
  assert.deepEqual(req, {
    case_path: "/cases/A",
    tool: "ileapp",
    input_path: "/ev/ffs.zip",
    input_type: "zip",
    modules: { mode: "custom", modules: ["sms"] },
    timezone: "America/Chicago",
    itunes_password: null,
    keychain_path: "/ev/kc.plist",
    hash_input: true,
    label: "Item 3",
  });
});

test("buildRunRequest: the password only for an encrypted backup; folders are never hashed", () => {
  const req = buildRunRequest(
    "/cases/A",
    form({ inputPath: "/ev/backup", inspection: ENCRYPTED_BACKUP, inputType: "itunes", password: "pw", hashInput: true }),
  );
  assert.equal(req.itunes_password, "pw");
  assert.equal(req.hash_input, false);
  assert.equal(req.label, null);
  assert.deepEqual(req.modules, { mode: "all" });
  const plain = buildRunRequest("/cases/A", form({ password: "left over" }));
  assert.equal(plain.itunes_password, null);
});

test("buildRunRequest: aLEAPP gets no timezone, keychain or password; profile mode names the profile", () => {
  const profile = { tool: /** @type {const} */ ("aleapp"), name: "Android triage", modules: ["x"], unknown_modules: [] };
  const req = buildRunRequest(
    "/cases/A",
    form({ tool: "aleapp", keychainPath: "/ev/kc", timezone: "UTC", moduleMode: "profile", profile, hashInput: false }),
  );
  assert.equal(req.timezone, null);
  assert.equal(req.keychain_path, null);
  assert.equal(req.itunes_password, null);
  assert.equal(req.hash_input, false);
  assert.deepEqual(req.modules, { mode: "profile", profile_name: "Android triage" });
});

test("canHash only for hashable files", () => {
  assert.equal(canHash(form()), true);
  assert.equal(canHash(form({ inspection: ENCRYPTED_BACKUP })), false);
  assert.equal(canHash(form({ inspection: { ...ZIP_FILE, hashable: false } })), false);
  assert.equal(canHash(form({ inspection: null })), false);
});

test("initialInputType picks the detected type if allowed, or the only allowed type", () => {
  assert.equal(initialInputType(ZIP_FILE), "zip");
  assert.equal(initialInputType({ ...ZIP_FILE, detected_type: null }), null);
  assert.equal(initialInputType({ ...ZIP_FILE, detected_type: "itunes" }), null);
  assert.equal(initialInputType({ ...ZIP_FILE, detected_type: null, allowed_types: ["fs"] }), "fs");
});
