// @ts-check
import { h } from "../lib/dom.js";
import { routeHref } from "../lib/router.js";

/** @typedef {import("../lib/context").View} View */

/**
 * Shown for a route this build has no screen for.
 * @returns {View}
 */
export function notFoundScreen() {
  const node = h(
    "section",
    { class: "screen" },
    h("h1", { tabindex: "-1" }, "Page not found"),
    h("p", null, "This screen does not exist. ", h("a", { href: routeHref("cases") }, "Go to Cases"), "."),
  );
  return { node, dispose() {} };
}
