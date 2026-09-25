// @ts-check
/**
 * Tiny pub/sub store (DEVELOPMENT.md §4.6). State is a frozen object replaced on every change;
 * subscribers are called synchronously with the new and the previous state.
 */

/**
 * @template T
 * @typedef {object} Store
 * @property {() => Readonly<T>} get
 * @property {(patch: Partial<T>) => void} set Shallow-merges `patch`; does nothing if no value changes.
 * @property {(fn: (state: Readonly<T>, prev: Readonly<T>) => void) => () => void} subscribe
 *   Returns the unsubscribe function.
 */

/**
 * @template {object} T
 * @param {T} initial
 * @returns {Store<T>}
 */
export function createStore(initial) {
  /** @type {Readonly<T>} */
  let state = Object.freeze({ ...initial });
  /** @type {Set<(state: Readonly<T>, prev: Readonly<T>) => void>} */
  const subscribers = new Set();
  return {
    get: () => state,
    set(patch) {
      const prev = state;
      const changed = Object.keys(patch).some(
        (key) => !Object.is(/** @type {any} */ (prev)[key], /** @type {any} */ (patch)[key]),
      );
      if (!changed) return;
      state = Object.freeze({ ...prev, ...patch });
      // Copy first: a subscriber may unsubscribe (or subscribe) while we notify.
      for (const fn of [...subscribers]) fn(state, prev);
    },
    subscribe(fn) {
      subscribers.add(fn);
      return () => {
        subscribers.delete(fn);
      };
    },
  };
}

/**
 * Calls `fn` now and whenever `select(state)` changes (by `Object.is`).
 * @template T, S
 * @param {Store<T>} store
 * @param {(state: Readonly<T>) => S} select
 * @param {(value: S) => void} fn
 * @returns {() => void} the unsubscribe function
 */
export function watch(store, select, fn) {
  let current = select(store.get());
  fn(current);
  return store.subscribe((state) => {
    const next = select(state);
    if (Object.is(next, current)) return;
    current = next;
    fn(next);
  });
}
