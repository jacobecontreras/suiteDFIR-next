// @ts-check
// "Find iOS backups" (ROADMAP S1): the finder's logic (ui/lib/backups.js) and the mock's found,
// empty and permission_denied results.
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  accessGuidance,
  canFindBackups,
  chosenBackupType,
  defaultBackupFolders,
  deviceName,
  encryptionLabel,
  failedResult,
  folderName,
  foundResult,
  iosVersionText,
  summaryText,
  useLabel,
} from "../../ui/lib/backups.js";

/** @typedef {import("../../ui/types").InputInspection} InputInspection */
/** @typedef {import("../../ui/types").IosBackup} IosBackup */
/** @typedef {typeof import("../../ui/api/ipc.js")} Api */

/** @type {IosBackup} */
const ALEX = {
  path: "/Users/examiner/Library/Application Support/MobileSync/Backup/00008101-000A1B2C3D4E001E",
  device_name: "Alex's iPhone",
  product_type: "iPhone13,2",
  ios_version: "18.6",
  last_backup: "2026-09-20T21:04:33Z",
  encrypted: true,
  size_bytes: 61203455110,
};

/** @type {IosBackup} */
const UNKNOWN = {
  path: "C:\\Users\\examiner\\Apple\\MobileSync\\Backup\\9c01de2b7a4f4e1d8b3a6c5e0f1d2a3b4c5d6e7f\\",
  device_name: null,
  product_type: null,
  ios_version: null,
  last_backup: null,
  encrypted: null,
  size_bytes: null,
};

/** @type {InputInspection} */
const BACKUP_FOLDER = {
  path: ALEX.path,
  kind: "directory",
  size_bytes: null,
  detected_type: "itunes",
  allowed_types: ["fs", "itunes"],
  is_itunes_backup: true,
  itunes_encrypted: true,
  hashable: false,
  warnings: [],
};

test("a search comes back found, empty, denied or failed", () => {
  assert.deepEqual(foundResult([ALEX]), { kind: "found", backups: [ALEX] });
  assert.deepEqual(foundResult([]), { kind: "empty" });
  const denied = { code: "permission_denied", message: "Full Disk Access", detail: "…/Backup: Operation not permitted" };
  assert.deepEqual(failedResult(denied), { kind: "denied", error: denied });
  const io = { code: "io", message: "The backup folder could not be read", detail: null };
  assert.deepEqual(failedResult(io), { kind: "failed", error: io });
  // Anything that is not an AppError is a plain failure (shown as `internal`).
  const thrown = new Error("boom");
  assert.deepEqual(failedResult(thrown), { kind: "failed", error: thrown });
  assert.equal(summaryText(foundResult([ALEX])), "1 backup found. Choose one to use it as the input.");
  assert.equal(summaryText(foundResult([ALEX, UNKNOWN])), "2 backups found. Choose one to use it as the input.");
  assert.equal(summaryText(foundResult([])), "No backups found.");
  assert.equal(summaryText(failedResult(denied)), "The backup folder could not be read.");
  assert.equal(summaryText(failedResult(io)), "The search failed.");
});

test("a row describes the backup; details the plists do not give stay visibly unknown", () => {
  assert.equal(deviceName(ALEX), "Alex's iPhone");
  assert.equal(iosVersionText(ALEX), "18.6");
  assert.equal(encryptionLabel(ALEX), "Encrypted");
  assert.equal(encryptionLabel({ ...ALEX, encrypted: false }), "Not encrypted");
  assert.equal(deviceName(UNKNOWN), "Unknown device");
  assert.equal(iosVersionText(UNKNOWN), "—");
  assert.equal(encryptionLabel(UNKNOWN), "Unknown");
  assert.equal(folderName(ALEX.path), "00008101-000A1B2C3D4E001E");
  assert.equal(folderName(UNKNOWN.path), "9c01de2b7a4f4e1d8b3a6c5e0f1d2a3b4c5d6e7f");
});

test("each Use button's accessible name starts with its text and tells the backups apart", () => {
  assert.equal(useLabel(ALEX, "UTC"), "Use the backup of Alex's iPhone, last backup 2026-09-20 21:04:33");
  assert.equal(useLabel(ALEX, "America/Chicago"), "Use the backup of Alex's iPhone, last backup 2026-09-20 16:04:33");
  assert.equal(useLabel(UNKNOWN, "UTC"), "Use the backup in folder 9c01de2b7a4f4e1d8b3a6c5e0f1d2a3b4c5d6e7f");
  // Device names are evidence-derived text: used as they are, never interpreted.
  const odd = { ...UNKNOWN, device_name: "<img src=x onerror=alert(1)>" };
  assert.equal(useLabel(odd, "UTC"), "Use the backup of <img src=x onerror=alert(1)>");
  for (const b of [ALEX, UNKNOWN, odd]) assert.ok(useLabel(b).startsWith("Use"));
});

test("the default folders and the access guidance follow the OS", () => {
  assert.deepEqual(defaultBackupFolders("macos"), ["~/Library/Application Support/MobileSync/Backup"]);
  assert.deepEqual(defaultBackupFolders("windows"), [
    "%APPDATA%\\Apple Computer\\MobileSync\\Backup",
    "%USERPROFILE%\\Apple\\MobileSync\\Backup",
  ]);
  assert.deepEqual(defaultBackupFolders("linux"), []);
  assert.deepEqual(defaultBackupFolders(null), []);
  const mac = accessGuidance("macos");
  assert.match(mac.title, /Full Disk Access/);
  assert.ok(mac.steps.some((s) => s.includes("System Settings > Privacy & Security > Full Disk Access")));
  assert.ok(mac.steps.at(-1)?.includes("search again"));
  // The steps are shown once: the intro does not repeat them.
  assert.ok(!mac.intro.includes("System Settings"), mac.intro);
  // The macOS guidance is the default while the OS is not known yet.
  assert.deepEqual(accessGuidance(null), mac);
  for (const os of ["windows", "linux"]) {
    const g = accessGuidance(os);
    assert.ok(!`${g.title} ${g.intro} ${g.steps.join(" ")}`.includes("Full Disk Access"), os);
  }
});

test("only a tool that reads iTunes backups offers the finder, and not on Linux", () => {
  for (const os of ["macos", "windows"]) {
    assert.equal(canFindBackups("ileapp", os), true, os);
    assert.equal(canFindBackups("aleapp", os), false, os);
    assert.equal(canFindBackups(null, os), false, os);
  }
  // Linux has no default backup folder; an unknown OS offers nothing either.
  assert.equal(canFindBackups("ileapp", "linux"), false);
  assert.equal(canFindBackups("ileapp", null), false);
});

test("a chosen backup is read as itunes only when its inspection says so", () => {
  assert.equal(chosenBackupType(BACKUP_FOLDER), "itunes");
  // The inspection decides: no iTunes backup detected → the usual preselection (fs), never itunes.
  assert.equal(chosenBackupType({ ...BACKUP_FOLDER, detected_type: "fs", is_itunes_backup: false }), "fs");
  // A tool without iTunes input keeps the usual preselection.
  assert.equal(chosenBackupType({ ...BACKUP_FOLDER, detected_type: "fs", allowed_types: ["fs"] }), "fs");
});

// ---- The mock (ui-dev/mock.js) ----

/**
 * A fresh mock instance with the given scenario flags (its state and flags are per module load).
 * @param {string} flags
 * @returns {Promise<Api>}
 */
async function mockWith(flags) {
  const g = /** @type {any} */ (globalThis);
  g.__SUITEDFIR_MOCK_TICK_MS = 1;
  g.location = { search: `?mock&scenario=${flags}` };
  try {
    return await import(new URL(`../../ui-dev/mock.js?flags=${flags}`, import.meta.url).href);
  } finally {
    delete g.location;
  }
}

const CASE = "/Users/examiner/Documents/suiteDFIR Cases/Operation Nightjar";

test("the mock finds its fixture backups, newest first, and they inspect as the list says", async () => {
  const mock = await mockWith("");
  const found = await mock.ios_backups_find();
  assert.equal(found.length, 4);
  assert.equal(found[0].device_name, "Alex's iPhone");
  const dates = found.map((b) => b.last_backup);
  assert.deepEqual(dates, [...dates].sort((a, b) => (b ?? "").localeCompare(a ?? "")));
  assert.equal(found.at(-1)?.device_name, null);
  for (const b of found.slice(0, 3)) {
    const insp = await mock.input_inspect({ tool: "ileapp", path: b.path, case_path: CASE });
    assert.equal(insp.is_itunes_backup, true, b.path);
    assert.equal(chosenBackupType(insp), "itunes", b.path);
    assert.equal(insp.itunes_encrypted, b.encrypted, b.path);
  }
  // The backup listed with unknown details cannot be read.
  await assert.rejects(mock.input_inspect({ tool: "ileapp", path: found[3].path, case_path: CASE }), { code: "permission_denied" });
  // Each call returns a copy.
  found[0].device_name = "changed";
  assert.equal((await mock.ios_backups_find())[0].device_name, "Alex's iPhone");
});

test("the mock's backups_empty and backups_denied flags", async () => {
  const empty = await mockWith("backups_empty");
  assert.deepEqual(await empty.ios_backups_find(), []);
  const denied = await mockWith("backups_denied");
  await assert.rejects(denied.ios_backups_find(), (/** @type {any} */ err) => {
    assert.equal(err.code, "permission_denied");
    assert.match(err.message, /Full Disk Access/);
    assert.match(err.detail, /MobileSync\/Backup: access denied/);
    return true;
  });
  const windows = await mockWith("backups_denied,windows");
  await assert.rejects(windows.ios_backups_find(), (/** @type {any} */ err) => {
    assert.equal(err.code, "permission_denied");
    assert.ok(!err.message.includes("Full Disk Access"), err.message);
    return true;
  });
});
