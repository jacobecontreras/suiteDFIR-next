// @ts-check
/**
 * Rate limiting for screen-reader announcements (tests/ui/throttle.test.js): the latest log line
 * goes to an `aria-live="polite"` region at most once per second (DEVELOPMENT.md §4.6).
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
 * @property {() => void} cancel Drops a pending value and stops the timer.
 */

/**
 * Delivers the latest pushed value to `fn` at most once per `intervalMs`: a value pushed after a
 * quiet interval goes out at once; values pushed sooner wait for the end of the interval, and only
 * the last of them is delivered.
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

  const fire = () => {
    timer = null;
    if (!pending) return;
    pending = false;
    last = clock.now();
    fn(/** @type {T} */ (latest));
  };

  return {
    push(value) {
      latest = value;
      pending = true;
      if (timer !== null) return;
      const wait = last + intervalMs - clock.now();
      if (wait <= 0) fire();
      else timer = clock.setTimeout(fire, wait);
    },
    cancel() {
      if (timer !== null) clock.clearTimeout(timer);
      timer = null;
      pending = false;
    },
  };
}
