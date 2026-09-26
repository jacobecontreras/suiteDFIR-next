// @ts-check
/**
 * Rate limiting for screen-reader announcements (tests/ui/throttle.test.js): the latest log line
 * goes to an `aria-live="polite"` region at most once per second (DEVELOPMENT.md §4.6). The log
 * search (S2) reports through the same region and the same limit, ahead of the latest line.
 */

/**
 * @typedef {object} Clock
 * @property {() => number} now Milliseconds.
 * @property {(fn: () => void, ms: number) => unknown} setTimeout
 * @property {(id: any) => void} clearTimeout
 */

/** @type {Clock} */
const SYSTEM_CLOCK = {
  now: () => Date.now(),
  setTimeout: (fn, ms) => setTimeout(fn, ms),
  clearTimeout: (id) => clearTimeout(id),
};

/**
 * @template T
 * @typedef {object} Throttled
 * @property {(value: T) => void} push Offers a new value; only the latest one is delivered.
 * @property {(value: T) => void} pushUrgent Offers a value the examiner just asked for (a search
 *   report). It replaces an earlier waiting urgent value, goes out ahead of a waiting ordinary
 *   value, and is never replaced by one; the latest ordinary value follows in the next interval.
 * @property {() => void} cancel Drops the pending values and stops the timer.
 */

/**
 * Delivers the latest pushed value to `fn` at most once per `intervalMs`: a value pushed after a
 * quiet interval goes out at once; values pushed sooner wait for the end of the interval, and only
 * the last of them is delivered. An urgent value takes the next delivery; ordinary values resume
 * as soon as no urgent value is waiting. Deliveries of both kinds together stay at most one per
 * interval.
 * @template T
 * @param {(value: T) => void} fn
 * @param {number} intervalMs
 * @param {Clock} [clock]
 * @returns {Throttled<T>}
 */
export function throttleLatest(fn, intervalMs, clock = SYSTEM_CLOCK) {
  let last = Number.NEGATIVE_INFINITY;
  /** @type {unknown} */
  let timer = null;
  let pending = false;
  /** @type {T | undefined} */
  let latest;
  let urgentPending = false;
  /** @type {T | undefined} */
  let urgent;

  const fire = () => {
    timer = null;
    if (urgentPending) {
      urgentPending = false;
      last = clock.now();
      fn(/** @type {T} */ (urgent));
      // The waiting ordinary value takes the next interval.
      if (pending) arm();
      return;
    }
    if (!pending) return;
    pending = false;
    last = clock.now();
    fn(/** @type {T} */ (latest));
  };

  const arm = () => {
    if (timer !== null) return;
    const wait = last + intervalMs - clock.now();
    if (wait <= 0) fire();
    else timer = clock.setTimeout(fire, wait);
  };

  return {
    push(value) {
      latest = value;
      pending = true;
      arm();
    },
    pushUrgent(value) {
      urgent = value;
      urgentPending = true;
      arm();
    },
    cancel() {
      if (timer !== null) clock.clearTimeout(timer);
      timer = null;
      pending = false;
      urgentPending = false;
    },
  };
}
