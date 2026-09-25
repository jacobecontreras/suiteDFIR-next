// @ts-check
import { h } from "./lib/dom.js";

/** @returns {HTMLElement} */
function renderShell() {
  return h(
    "div",
    { class: "shell" },
    h("header", { class: "topbar" }, h("span", { class: "app-name" }, "suiteDFIR")),
    h("main", { class: "content", id: "main" }),
  );
}

const root = document.getElementById("app");
if (root) {
  root.replaceChildren(renderShell());
}
