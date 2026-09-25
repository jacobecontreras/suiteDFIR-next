// @ts-check
/**
 * The acquisition → parse handoff (CONTRACTS.md §13.5, ARCHITECTURE.md §6b; tests/ui/handoff.test.js).
 *
 * When the examiner turns backup encryption on and ticks "Parse with iLEAPP now", the UI keeps the
 * password it collected, in memory only (never stored, never in a route), so "Parse with iLEAPP"
 * can pre-fill New run. It holds one password at a time and lets go of it as soon as it cannot be
 * used any more:
 * - New run takes it (and clears it when the run starts or the form closes);
 * - the acquisition finishes without succeeding (there is nothing to parse);
 * - it finishes while no Acquire screen shows it, or the examiner leaves the result;
 * - another acquisition starts, or the examiner dismisses the result.
 */

/**
 * @typedef {object} Handoff
 * @property {(acqId: string, password: string) => void} hold Replaces any held password.
 * @property {(acqId: string) => boolean} has
 * @property {(acqId: string) => string | null} take Returns the password once and forgets it.
 * @property {(acqId: string, status: import("../types").AcqStatus) => void} finished
 * @property {(acqId: string) => () => void} watch An Acquire screen shows this acquisition until
 *   the returned function is called.
 * @property {() => void} drop
 */

/** @returns {Handoff} */
export function createHandoff() {
  /** @type {{ acqId: string, password: string, finished: boolean, watchers: number } | null} */
  let held = null;
  return {
    hold(acqId, password) {
      held = { acqId, password, finished: false, watchers: 0 };
    },
    has(acqId) {
      return held?.acqId === acqId;
    },
    take(acqId) {
      if (!held || held.acqId !== acqId) return null;
      const { password } = held;
      held = null;
      return password;
    },
    finished(acqId, status) {
      if (!held || held.acqId !== acqId) return;
      if (status !== "succeeded" || held.watchers === 0) held = null;
      else held.finished = true;
    },
    watch(acqId) {
      if (held?.acqId === acqId) held.watchers += 1;
      let done = false;
      return () => {
        if (done) return;
        done = true;
        if (!held || held.acqId !== acqId) return;
        held.watchers = Math.max(0, held.watchers - 1);
        if (held.finished && held.watchers === 0) held = null;
      };
    },
    drop() {
      held = null;
    },
  };
}

/** The app's one handoff. */
export const handoff = createHandoff();
