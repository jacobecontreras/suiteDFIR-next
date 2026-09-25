// @ts-check
/**
 * Module picker (ROADMAP D3): search, collapsible category groups with tri-state checkboxes,
 * select all/none (of the shown modules), the selected count, and the unknown modules with
 * "Remove unknown".
 *
 * Built for ~1,300 modules (DEVELOPMENT.md §4.6): every row is created once; searching only toggles
 * `hidden` and updates the group checkboxes, and a selection change updates one group.
 */
import { h } from "../lib/dom.js";
import { formatCount } from "../lib/format.js";
import {
  groupByCategory,
  matchesTokens,
  orderedSelection,
  queryTokens,
  searchText,
  setMany,
  toggleGroup,
  triState,
  unknownNames,
  withoutUnknown,
} from "../lib/selection.js";
import { icon, uid } from "../lib/view.js";

/** @typedef {import("../types").ModuleInfo} ModuleInfo */
/** @typedef {import("../lib/context").View} View */

/**
 * @typedef {object} GroupView
 * @property {string} category
 * @property {number[]} members Indexes into the module list.
 * @property {HTMLElement} node
 * @property {HTMLElement} list
 * @property {HTMLButtonElement} toggle
 * @property {HTMLInputElement} check
 * @property {HTMLElement} count
 * @property {boolean} expanded The examiner's choice (search may show a group without changing it).
 */

/**
 * @typedef {View & {
 *   selected: () => string[],
 *   setSelected: (names: readonly string[]) => void,
 *   setQuery: (query: string) => void,
 * }} ModulePicker
 */

/**
 * @param {object} spec
 * @param {readonly ModuleInfo[]} spec.modules The installed tool's modules (`tool_modules`).
 * @param {readonly string[]} spec.selected May contain names the tool does not know.
 * @param {string} spec.toolLabel e.g. "iLEAPP v2026.4.2", for the unknown-modules message.
 * @param {(selected: string[]) => void} spec.onChange
 * @returns {ModulePicker}
 */
export function modulePicker(spec) {
  const modules = spec.modules;
  const known = new Set(modules.map((m) => m.name));
  const texts = modules.map(searchText);
  /** @type {Set<string>} */
  let selected = new Set(spec.selected);
  /** @type {string[]} */
  let tokens = [];

  const baseId = uid("picker");
  const searchId = `${baseId}-search`;
  const search = /** @type {HTMLInputElement} */ (
    h("input", { class: "input picker-search-input", type: "search", id: searchId, placeholder: "Search modules", autocomplete: "off", spellcheck: "false" })
  );
  const selectAll = h("button", { class: "btn btn-sm", type: "button" }, "Select all");
  const selectNone = h("button", { class: "btn btn-sm", type: "button" }, "Select none");
  const count = h("span", { class: "picker-count", "aria-live": "polite" });
  const unknownBox = h("div", { class: "picker-unknown banner banner-warn", hidden: true });
  const empty = h("p", { class: "picker-empty muted", hidden: true });

  /** @type {HTMLInputElement[]} */
  const checks = [];
  /** @type {HTMLElement[]} */
  const rows = [];
  /** @type {number[]} module index → group index */
  const groupOf = [];
  /** @type {GroupView[]} */
  const groups = [];

  let index = 0;
  for (const group of groupByCategory(modules)) {
    const g = groups.length;
    const listId = `${baseId}-g${g}`;
    /** @type {number[]} */
    const members = [];
    /** @type {HTMLElement[]} */
    const items = [];
    for (const m of group.modules) {
      const check = /** @type {HTMLInputElement} */ (h("input", { type: "checkbox", "data-index": index }));
      check.checked = selected.has(m.name);
      const row = h(
        "li",
        { class: "picker-row" },
        h(
          "label",
          { title: m.description ?? null },
          check,
          h("span", { class: "picker-name" }, m.display_name),
          h("span", { class: "picker-id mono" }, m.name),
        ),
      );
      checks.push(check);
      rows.push(row);
      groupOf.push(g);
      members.push(index);
      items.push(row);
      index += 1;
    }
    const check = /** @type {HTMLInputElement} */ (h("input", { type: "checkbox", "data-group": g }));
    const toggle = /** @type {HTMLButtonElement} */ (
      h(
        "button",
        { class: "picker-toggle", type: "button", "aria-expanded": "false", "aria-controls": listId, "data-toggle": g },
        icon("chevron-right", "icon picker-chevron"),
        h("span", { class: "visually-hidden" }, `Show ${group.category}`),
      )
    );
    const countEl = h("span", { class: "picker-group-count" });
    const list = h("ul", { class: "picker-list", id: listId, hidden: true }, items);
    const node = h(
      "li",
      { class: "picker-group" },
      h(
        "div",
        { class: "picker-group-head" },
        toggle,
        h("label", { class: "picker-group-label" }, check, h("span", null, group.category)),
        countEl,
      ),
      list,
    );
    groups.push({ category: group.category, members, node, list, toggle, check, count: countEl, expanded: false });
  }

  const groupList = h("ul", { class: "picker-groups", "aria-label": "Modules by category" }, groups.map((g) => g.node));
  const node = h(
    "div",
    { class: "picker" },
    h(
      "div",
      { class: "picker-toolbar" },
      h(
        "div",
        { class: "picker-search" },
        icon("search"),
        h("label", { class: "visually-hidden", for: searchId }, "Search modules"),
        search,
      ),
      selectAll,
      selectNone,
      count,
    ),
    unknownBox,
    empty,
    groupList,
  );

  /** @param {number} i */
  const isShown = (i) => tokens.length === 0 || matchesTokens(texts[i], tokens);

  /** @param {GroupView} g */
  function shownNames(g) {
    return g.members.filter(isShown).map((i) => modules[i].name);
  }

  /** @param {GroupView} g */
  function updateGroup(g) {
    const names = shownNames(g);
    const state = triState(names, selected);
    g.check.checked = state === "all";
    g.check.indeterminate = state === "some";
    let n = 0;
    for (const name of names) if (selected.has(name)) n += 1;
    g.count.textContent = `${n}/${names.length}`;
  }

  function updateCount() {
    const knownSelected = [...selected].filter((n) => known.has(n)).length;
    count.textContent = `${formatCount(knownSelected)} of ${formatCount(modules.length)} selected`;
  }

  function updateUnknown() {
    const unknown = unknownNames(selected, known);
    unknownBox.hidden = unknown.length === 0;
    if (unknown.length === 0) {
      unknownBox.replaceChildren();
      return;
    }
    unknownBox.replaceChildren(
      icon("alert-triangle"),
      h(
        "div",
        { class: "stack-sm" },
        h(
          "p",
          null,
          h("strong", null, unknown.length === 1 ? "1 selected module is unknown" : `${unknown.length} selected modules are unknown`),
          ` to ${spec.toolLabel}. The run is blocked until they are removed.`,
        ),
        h("ul", { class: "list-compact mono" }, unknown.map((n) => h("li", null, n))),
        h(
          "div",
          null,
          h(
            "button",
            {
              class: "btn btn-sm",
              type: "button",
              onClick: () => {
                selected = withoutUnknown(selected, known);
                updateUnknown();
                updateCount();
                emit();
              },
            },
            "Remove unknown",
          ),
        ),
      ),
    );
  }

  function emit() {
    spec.onChange(orderedSelection(selected, modules));
  }

  /** @param {GroupView} g @param {boolean} open */
  function showGroup(g, open) {
    g.list.hidden = !open;
    g.toggle.setAttribute("aria-expanded", String(open));
    g.toggle.classList.toggle("is-open", open);
    const label = g.toggle.querySelector(".visually-hidden");
    if (label) label.textContent = `${open ? "Hide" : "Show"} ${g.category}`;
  }

  function applyFilter() {
    let shownGroups = 0;
    for (let i = 0; i < rows.length; i++) rows[i].hidden = !isShown(i);
    for (const g of groups) {
      const anyShown = g.members.some(isShown);
      g.node.hidden = !anyShown;
      if (anyShown) shownGroups += 1;
      // While searching, matching groups open; clearing the search restores the examiner's choice.
      showGroup(g, tokens.length > 0 ? anyShown : g.expanded);
      updateGroup(g);
    }
    const filtering = tokens.length > 0;
    selectAll.textContent = filtering ? "Select shown" : "Select all";
    selectNone.textContent = filtering ? "Clear shown" : "Select none";
    empty.hidden = shownGroups > 0;
    empty.textContent = shownGroups > 0 ? "" : `No module matches “${search.value.trim()}”.`;
  }

  /** @param {boolean} on */
  function setShown(on) {
    const names = modules.filter((_, i) => isShown(i)).map((m) => m.name);
    selected = setMany(selected, names, on);
    for (let i = 0; i < checks.length; i++) if (isShown(i)) checks[i].checked = on;
    for (const g of groups) updateGroup(g);
    updateCount();
    emit();
  }

  search.addEventListener("input", () => {
    tokens = queryTokens(search.value);
    applyFilter();
  });
  selectAll.addEventListener("click", () => setShown(true));
  selectNone.addEventListener("click", () => setShown(false));

  groupList.addEventListener("change", (event) => {
    const target = /** @type {HTMLElement} */ (event.target);
    if (!(target instanceof HTMLInputElement)) return;
    if (target.dataset.index !== undefined) {
      const i = Number(target.dataset.index);
      if (target.checked) selected.add(modules[i].name);
      else selected.delete(modules[i].name);
      updateGroup(groups[groupOf[i]]);
    } else if (target.dataset.group !== undefined) {
      const g = groups[Number(target.dataset.group)];
      const names = shownNames(g);
      selected = toggleGroup(selected, names);
      for (const i of g.members) if (isShown(i)) checks[i].checked = selected.has(modules[i].name);
      updateGroup(g);
    } else {
      return;
    }
    updateCount();
    emit();
  });

  groupList.addEventListener("click", (event) => {
    const button = /** @type {HTMLElement} */ (event.target).closest("button[data-toggle]");
    if (!(button instanceof HTMLButtonElement)) return;
    const g = groups[Number(button.dataset.toggle)];
    const open = button.getAttribute("aria-expanded") !== "true";
    g.expanded = open;
    showGroup(g, open);
  });

  for (const g of groups) updateGroup(g);
  updateCount();
  updateUnknown();

  return {
    node,
    selected: () => orderedSelection(selected, modules),
    setSelected(names) {
      selected = new Set(names);
      for (let i = 0; i < checks.length; i++) checks[i].checked = selected.has(modules[i].name);
      for (const g of groups) updateGroup(g);
      updateCount();
      updateUnknown();
    },
    setQuery(query) {
      search.value = query;
      tokens = queryTokens(query);
      applyFilter();
    },
    dispose() {},
  };
}
