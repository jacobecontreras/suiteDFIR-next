// @ts-check
/**
 * Tiny DOM builder, the only sanctioned way to build UI from data (DEVELOPMENT.md §4.3).
 *
 * - Text children always become text nodes; nothing is ever parsed as HTML.
 * - Prop names are checked case-insensitively, because HTML `setAttribute` lowercases names:
 *   `STYLE` would otherwise slip past a check for `style`.
 * - `innerHTML`, `outerHTML` and `srcdoc` are refused (markup from a string).
 * - `style` is refused: a `style=""` attribute violates the CSP (use classes, or `el.style.x`
 *   after creation).
 * - `class` must be a string; `className` is refused (use `class`).
 * - `on<event>` props must be functions and are attached with `addEventListener`.
 */

/**
 * @typedef {Node | string | number | boolean | null | undefined | Child[]} Child
 * `false`, `true`, `null` and `undefined` children are skipped, so `cond && h(...)` works.
 */

/**
 * @typedef {Record<string, unknown>} Props
 * `class` sets `className`; `on<event>` adds a listener; `true` sets an empty attribute;
 * `false`, `null` and `undefined` are skipped; strings and numbers set attributes.
 */

/** Lowercased prop names that are never allowed. */
const FORBIDDEN_PROPS = new Set(["innerhtml", "outerhtml", "srcdoc", "style"]);

/**
 * Creates an element.
 * @param {string} tag
 * @param {Props | null} [props]
 * @param {...Child} children
 * @returns {HTMLElement}
 */
export function h(tag, props, ...children) {
  const el = document.createElement(tag);
  if (props) {
    for (const [key, value] of Object.entries(props)) {
      setProp(el, key, value);
    }
  }
  for (const child of flattenChildren(children)) {
    el.appendChild(typeof child === "string" ? document.createTextNode(child) : child);
  }
  return el;
}

/**
 * Replaces the children of `el`, with the same child rules as `h()` (text stays text; `false`,
 * `null` and `undefined` are skipped).
 * @param {Element} el
 * @param {...Child} children
 * @returns {Element}
 */
export function fill(el, ...children) {
  el.replaceChildren(...flattenChildren(children));
  return el;
}

/**
 * Runs `render`, which may detach and re-attach the focused control inside `container` (e.g. by
 * replacing children that include persistent inputs), and then gives the focus and the text caret
 * back to that control if it is still in the document, or to the new control with the same
 * `data-focus-key`. An examiner typing (or tabbing) while a section re-renders stays in place.
 * @param {Element} container
 * @param {() => void} render
 */
export function keepFocus(container, render) {
  const focused = document.activeElement;
  const inside = typeof HTMLElement !== "undefined" && focused instanceof HTMLElement && container.contains(focused) ? focused : null;
  // A control that is rebuilt on every render carries `data-focus-key`; its replacement gets the focus.
  const key = inside?.dataset.focusKey ?? null;
  const keep = inside && (inside instanceof HTMLInputElement || inside instanceof HTMLSelectElement) ? inside : null;
  const selection = keep && keep instanceof HTMLInputElement ? [keep.selectionStart, keep.selectionEnd] : null;
  render();
  if (key !== null && !inside?.isConnected) {
    for (const el of container.querySelectorAll("[data-focus-key]")) {
      if (el instanceof HTMLElement && el.dataset.focusKey === key) {
        el.focus();
        return;
      }
    }
    return;
  }
  if (!keep || !keep.isConnected || document.activeElement === keep) return;
  keep.focus();
  if (keep instanceof HTMLInputElement && selection && selection[0] !== null && selection[1] !== null) {
    try {
      keep.setSelectionRange(selection[0], selection[1]);
    } catch {
      // Not a text control (e.g. a checkbox): there is no caret to restore.
    }
  }
}

/**
 * Sets `textContent` only when it differs (replacing equal text still swaps the text node).
 * @param {Node} el
 * @param {string} text
 */
export function setText(el, text) {
  if (el.textContent !== text) el.textContent = text;
}

/**
 * @typedef {object} KeyedSlot
 * @property {Element} node
 * @property {(key: string, build: () => Child) => boolean} update Rebuilds the children (keeping
 *   the focus, see `keepFocus`) only when `key` differs from the last one; returns whether it did.
 * @property {() => void} reset The next `update` rebuilds whatever its key.
 */

/**
 * A container rebuilt only when what it shows changes. Screens that render on every job event
 * (several per second) use it so focusable controls stay put and a live region (`role="alert"` /
 * `"status"`) inside is inserted, and so announced, only when its content changes.
 * @param {Element} node
 * @returns {KeyedSlot}
 */
export function keyedSlot(node) {
  /** @type {string | null} */
  let last = null;
  return {
    node,
    update(key, build) {
      if (key === last) return false;
      last = key;
      keepFocus(node, () => fill(node, build()));
      return true;
    },
    reset() {
      last = null;
    },
  };
}

/**
 * Flattens nested child arrays, drops skipped values and turns numbers into strings.
 * @param {Child[]} children
 * @returns {(Node | string)[]}
 */
export function flattenChildren(children) {
  /** @type {(Node | string)[]} */
  const out = [];
  /** @param {Child} child */
  const visit = (child) => {
    if (child === null || child === undefined || typeof child === "boolean") return;
    if (Array.isArray(child)) {
      child.forEach(visit);
    } else if (typeof child === "number") {
      out.push(String(child));
    } else {
      out.push(child);
    }
  };
  children.forEach(visit);
  return out;
}

/**
 * @param {HTMLElement} el
 * @param {string} key
 * @param {unknown} value
 */
function setProp(el, key, value) {
  const name = key.toLowerCase();
  if (name === "classname") {
    throw new TypeError(`h(): prop "${key}" is not allowed; use "class"`);
  }
  if (FORBIDDEN_PROPS.has(name)) {
    throw new TypeError(`h(): prop "${key}" is not allowed`);
  }
  if (name.startsWith("on")) {
    if (typeof value !== "function") {
      throw new TypeError(`h(): event prop "${key}" must be a function`);
    }
    el.addEventListener(name.slice(2), /** @type {EventListener} */ (value));
    return;
  }
  if (value === null || value === undefined || value === false) return;
  if (name === "class") {
    if (typeof value !== "string") {
      throw new TypeError(`h(): prop "${key}" must be a string`);
    }
    el.className = value;
  } else if (value === true) {
    el.setAttribute(key, "");
  } else if (typeof value === "string" || typeof value === "number") {
    el.setAttribute(key, String(value));
  } else {
    throw new TypeError(`h(): prop "${key}" must be a string, number or boolean`);
  }
}
