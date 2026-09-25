// @ts-check
/**
 * The one active job, app-wide (ARCHITECTURE.md D25): identity and polling (tests/ui/jobs.test.js).
 */

/** @typedef {import("../types").ActiveJob} ActiveJob */
/** @typedef {import("./context").AppState} AppState */

/**
 * A value that changes only when the job, its case or its phase changes. `job_active` returns a
 * new but equal object on every poll; comparing keys keeps the UI (and its live region) quiet.
 * @param {ActiveJob | null | undefined} job
 * @returns {string | null}
 */
export function jobKey(job) {
  if (!job) return null;
  const id = job.kind === "run" ? job.run_id : job.acq_id;
  return `${job.kind}|${id}|${job.phase}|${job.case_path}`;
}

/**
 * Sets `activeJob` only when the job actually changed (by `jobKey`).
 * @param {import("./store.js").Store<AppState>} store
 * @param {ActiveJob | null} job
 */
export function setActiveJob(store, job) {
  if (jobKey(job) !== jobKey(store.get().activeJob)) store.set({ activeJob: job });
}

/**
 * Keeps `activeJob` current: polls `job_active` every `intervalMs` while a job is active, and stops
 * when it ends. Returns a function that stops polling.
 * @param {{ job_active: () => Promise<ActiveJob | null> }} api
 * @param {import("./store.js").Store<AppState>} store
 * @param {number} intervalMs
 * @returns {() => void}
 */
export function pollActiveJob(api, store, intervalMs) {
  /** @type {ReturnType<typeof setTimeout> | null} */
  let timer = null;
  let stopped = false;
  const schedule = () => {
    if (stopped || timer !== null || !store.get().activeJob) return;
    timer = setTimeout(async () => {
      try {
        const job = await api.job_active();
        if (!stopped) setActiveJob(store, job);
      } catch {
        // Keep the last known state; the next poll retries.
      }
      timer = null;
      schedule();
    }, intervalMs);
  };
  const unsubscribe = store.subscribe(schedule);
  schedule();
  return () => {
    stopped = true;
    unsubscribe();
    if (timer !== null) clearTimeout(timer);
  };
}
