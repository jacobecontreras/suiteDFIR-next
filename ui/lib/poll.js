// @ts-check
/**
 * Single-flight polling (ROADMAP D5; tests/ui/poll.test.js): `devices_list` every 2 s while the
 * Acquire screen shows the device list. At most one call is ever in flight: the next one is
 * scheduled only after the previous one settled.
 */

/**
 * @typedef {object} Timers
 * @property {(fn: () => void, ms: number) => unknown} setTimeout
 * @property {(id: any) => void} clearTimeout
 */

/** @type {Timers} */
const SYSTEM_TIMERS = {
  setTimeout: (fn, ms) => setTimeout(fn, ms),
  clearTimeout: (id) => clearTimeout(id),
};

/**
 * @template T
 * @typedef {object} Poller
 * @property {() => void} start Polls now, then every interval. Does nothing while started.
 * @property {() => void} stop Stops; the result of a call still in flight is dropped.
 * @property {() => void} refresh Polls now, or right after the call in flight settles.
 * @property {() => void} invalidate Drops the result of the call in flight (a newer answer is
 *   known, e.g. from `device_pair`), and polls again after it.
 * @property {() => boolean} busy A call is in flight.
 */

/**
 * @template T
 * @param {object} spec
 * @param {() => Promise<T>} spec.fetch
 * @param {number} spec.intervalMs
 * @param {(result: T) => void} spec.onResult
 * @param {(error: unknown) => void} [spec.onError]
 * @param {Timers} [spec.timers]
 * @returns {Poller<T>}
 */
export function createPoller(spec) {
  const timers = spec.timers ?? SYSTEM_TIMERS;
  let running = false;
  let inFlight = false;
  /** Poll again as soon as the call in flight settles. */
  let again = false;
  /** Results of calls started in an older epoch are dropped. */
  let epoch = 0;
  /** @type {unknown} */
  let timer = null;

  const clearTimer = () => {
    if (timer !== null) timers.clearTimeout(timer);
    timer = null;
  };

  const schedule = () => {
    if (running && !inFlight && timer === null) timer = timers.setTimeout(poll, spec.intervalMs);
  };

  async function poll() {
    timer = null;
    if (!running || inFlight) return;
    inFlight = true;
    const mine = epoch;
    try {
      const result = await spec.fetch();
      if (running && mine === epoch) spec.onResult(result);
    } catch (err) {
      if (running && mine === epoch) spec.onError?.(err);
    } finally {
      inFlight = false;
      if (running && again) {
        again = false;
        void poll();
      } else {
        schedule();
      }
    }
  }

  return {
    start() {
      if (running) return;
      running = true;
      again = false;
      if (inFlight) again = true;
      else void poll();
    },
    stop() {
      running = false;
      again = false;
      epoch += 1;
      clearTimer();
    },
    refresh() {
      if (!running) return;
      if (inFlight) {
        again = true;
        return;
      }
      clearTimer();
      void poll();
    },
    invalidate() {
      epoch += 1;
      if (inFlight) again = true;
    },
    busy: () => inFlight,
  };
}
