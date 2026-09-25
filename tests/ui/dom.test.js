// @ts-check
import { test } from "node:test";
import assert from "node:assert/strict";

import { flattenChildren, h } from "../../ui/lib/dom.js";

// Node has no DOM. This fake implements exactly the DOM surface h() uses, and keeps text
// nodes distinguishable from elements so the tests can prove text is never parsed as markup.
class FakeText {
  /** @param {string} data */
  constructor(data) {
    this.nodeType = 3;
    this.data = data;
  }
}

class FakeElement {
  /** @param {string} tagName */
  constructor(tagName) {
    this.nodeType = 1;
    this.tagName = tagName.toUpperCase();
    this.className = "";
    /** @type {Map<string, string>} */
    this.attributes = new Map();
    /** @type {(FakeElement | FakeText)[]} */
    this.childNodes = [];
    /** @type {Map<string, Function[]>} */
    this.listeners = new Map();
  }
  /** @param {string} name @param {string} value */
  setAttribute(name, value) {
    this.attributes.set(name, value);
  }
  /** @param {FakeElement | FakeText} node */
  appendChild(node) {
    this.childNodes.push(node);
    return node;
  }
  /** @param {string} type @param {Function} fn */
  addEventListener(type, fn) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), fn]);
  }
}

const fakeDocument = {
  /** @param {string} tag */
  createElement: (tag) => new FakeElement(tag),
  /** @param {string} data */
  createTextNode: (data) => new FakeText(data),
};

/** @type {any} */ (globalThis).document = fakeDocument;

/**
 * @param {string} tag
 * @param {import("../../ui/lib/dom.js").Props | null} [props]
 * @param {...import("../../ui/lib/dom.js").Child} children
 * @returns {FakeElement}
 */
const fh = (tag, props, ...children) => /** @type {any} */ (h(tag, props, ...children));

test("creates an element with the given tag", () => {
  const el = fh("section");
  assert.equal(el.tagName, "SECTION");
  assert.equal(el.childNodes.length, 0);
});

test("string children become text nodes, never markup", () => {
  const payload = '<img src=x onerror="alert(1)">';
  const el = fh("p", null, payload);
  assert.equal(el.childNodes.length, 1);
  const node = el.childNodes[0];
  assert.ok(node instanceof FakeText);
  assert.equal(node.data, payload);
});

test("numbers become text; null, undefined and booleans are skipped; arrays are flattened", () => {
  const el = fh("ul", null, [h("li"), [null, 42]], undefined, false, true, "x");
  assert.equal(el.childNodes.length, 3);
  assert.ok(el.childNodes[0] instanceof FakeElement);
  assert.deepEqual(
    el.childNodes.slice(1).map((n) => (n instanceof FakeText ? n.data : null)),
    ["42", "x"],
  );
});

test("class sets className; strings, numbers and true set attributes; false and null are skipped", () => {
  const el = fh("input", {
    class: "field wide",
    type: "text",
    maxlength: 80,
    required: true,
    disabled: false,
    placeholder: null,
    title: undefined,
  });
  assert.equal(el.className, "field wide");
  assert.deepEqual(Object.fromEntries(el.attributes), { type: "text", maxlength: "80", required: "" });
});

test("on<event> props are attached as listeners", () => {
  const onClick = () => {};
  const el = fh("button", { onClick }, "Go");
  assert.deepEqual(el.listeners.get("click"), [onClick]);
  assert.equal(el.attributes.size, 0);
});

test("unsafe props are refused", () => {
  assert.throws(() => h("div", { innerHTML: "<b>x</b>" }), TypeError);
  assert.throws(() => h("div", { outerHTML: "<b>x</b>" }), TypeError);
  assert.throws(() => h("div", { style: "color: red" }), TypeError);
  assert.throws(() => h("div", { onclick: "alert(1)" }), TypeError);
  assert.throws(() => h("div", { data: { a: 1 } }), TypeError);
});

test("flattenChildren keeps nodes and order", () => {
  const node = /** @type {Node} */ (/** @type {unknown} */ (new FakeText("n")));
  assert.deepEqual(flattenChildren(["a", [1, [node, null]], false, "b"]), ["a", "1", node, "b"]);
});
