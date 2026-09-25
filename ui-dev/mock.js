// @ts-check
// Browser mock of the IPC API, served only by scripts/serve-ui.mjs at /dev/mock.js (never bundled).
// M0.3 stub: each function resolves to a copy of a generated contract fixture. ROADMAP D1 implements
// every command in CONTRACTS.md §10 and §13.5, with simulated runs and acquisitions.
import * as fixtures from "./fixtures/contracts/index.js";

/**
 * @template T
 * @param {T} value
 * @returns {Promise<T>}
 */
function reply(value) {
  return Promise.resolve(structuredClone(value));
}

export const app_info = () => reply(fixtures.AppInfo);
export const settings_get = () => reply(fixtures.Settings);
export const tools_status = () => reply([fixtures.ToolStatus]);
export const cases_list = () => reply([fixtures.CaseSummary]);
/** @param {import("../ui/types").PathRequest} _req */
export const case_open = (_req) => reply(fixtures.CaseDetail);
/** @param {import("../ui/types").RunRef} _req */
export const run_get = (_req) => reply(fixtures.RunRecord);
/** @returns {Promise<import("../ui/types").ActiveJob | null>} */
export const job_active = () => reply(null);
export const devices_list = () => reply(fixtures.DevicesResult);
/** @param {import("../ui/types").AcqRef} _req */
export const acq_get = (_req) => reply(fixtures.AcquisitionRecord);
