// @ts-check
/**
 * Tiny DOM builder, the only sanctioned way to build UI from data (DEVELOPMENT.md §4.3).
 *
 * - Text children always become text nodes; nothing is ever parsed as HTML.
 * - `innerHTML`/`outerHTML` props are refused.
 * - `style` is refused: a `style=""` attribute violates the CSP (use classes, or `el.style.x` after creation).
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

const FORBIDDEN_PROPS = new Set(["innerHTML", "outerHTML", "style"]);

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
  if (FORBIDDEN_PROPS.has(key)) {
    throw new TypeError(`h(): prop "${key}" is not allowed`);
  }
  if (key.startsWith("on")) {
    if (typeof value !== "function") {
      throw new TypeError(`h(): event prop "${key}" must be a function`);
    }
    el.addEventListener(key.slice(2).toLowerCase(), /** @type {EventListener} */ (value));
    return;
  }
  if (value === null || value === undefined || value === false) return;
  if (key === "class") {
    el.className = String(value);
  } else if (value === true) {
    el.setAttribute(key, "");
  } else if (typeof value === "string" || typeof value === "number") {
    el.setAttribute(key, String(value));
  } else {
    throw new TypeError(`h(): prop "${key}" must be a string, number or boolean`);
  }
}
