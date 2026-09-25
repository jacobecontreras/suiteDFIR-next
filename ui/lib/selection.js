// @ts-check
/**
 * Module-picker selection logic (tests/ui/selection.test.js): search, category groups, tri-state
 * group checkboxes and the unknown-module diff (names a profile or selection has that the
 * installed tool does not, ARCHITECTURE.md F7, D12).
 */

/** @typedef {import("../types").ModuleInfo} ModuleInfo */
/** @typedef {"all" | "some" | "none"} TriState */
/** @typedef {{ category: string, modules: ModuleInfo[] }} ModuleGroup */

/**
 * @param {string} query
 * @returns {string[]} lowercase search tokens; every token must match
 */
export function queryTokens(query) {
  return query.toLowerCase().split(/\s+/).filter(Boolean);
}

/**
 * The lowercase text a module is searched by: display name, artifact name, category, description.
 * @param {ModuleInfo} m
 * @returns {string}
 */
export function searchText(m) {
  return `${m.display_name}\n${m.name}\n${m.category}\n${m.description ?? ""}`.toLowerCase();
}

/**
 * @param {string} text from `searchText`
 * @param {readonly string[]} tokens from `queryTokens`
 * @returns {boolean}
 */
export function matchesTokens(text, tokens) {
  return tokens.every((t) => text.includes(t));
}

/**
 * @param {readonly ModuleInfo[]} modules
 * @param {string} query
 * @returns {ModuleInfo[]} the matching modules, in order
 */
export function filterModules(modules, query) {
  const tokens = queryTokens(query);
  if (tokens.length === 0) return [...modules];
  return modules.filter((m) => matchesTokens(searchText(m), tokens));
}

/**
 * Groups by category in first-seen order (`modules.json` is sorted by category, then name).
 * @param {readonly ModuleInfo[]} modules
 * @returns {ModuleGroup[]}
 */
export function groupByCategory(modules) {
  /** @type {Map<string, ModuleInfo[]>} */
  const groups = new Map();
  for (const m of modules) {
    const list = groups.get(m.category);
    if (list) list.push(m);
    else groups.set(m.category, [m]);
  }
  return [...groups].map(([category, list]) => ({ category, modules: list }));
}

/**
 * The state of a group checkbox over `names` (the group's visible members).
 * @param {readonly string[]} names
 * @param {ReadonlySet<string>} selected
 * @returns {TriState}
 */
export function triState(names, selected) {
  let count = 0;
  for (const n of names) if (selected.has(n)) count += 1;
  if (count === 0) return "none";
  return count === names.length ? "all" : "some";
}

/**
 * Clicking a group checkbox: a fully selected group is cleared; otherwise all of it is selected
 * (so a mixed group becomes fully selected, like a file-manager tree).
 * @param {ReadonlySet<string>} selected
 * @param {readonly string[]} names
 * @returns {Set<string>} the new selection
 */
export function toggleGroup(selected, names) {
  return setMany(selected, names, triState(names, selected) !== "all");
}

/**
 * @param {ReadonlySet<string>} selected
 * @param {readonly string[]} names
 * @param {boolean} on
 * @returns {Set<string>} the new selection
 */
export function setMany(selected, names, on) {
  const next = new Set(selected);
  for (const n of names) {
    if (on) next.add(n);
    else next.delete(n);
  }
  return next;
}

/**
 * Selected names the installed tool does not know, in selection order.
 * @param {Iterable<string>} selected
 * @param {ReadonlySet<string>} known the installed module names
 * @returns {string[]}
 */
export function unknownNames(selected, known) {
  return [...selected].filter((n) => !known.has(n));
}

/**
 * The selection without unknown names ("remove unknown").
 * @param {Iterable<string>} selected
 * @param {ReadonlySet<string>} known
 * @returns {Set<string>}
 */
export function withoutUnknown(selected, known) {
  return new Set([...selected].filter((n) => known.has(n)));
}

/**
 * The selection as an array: known names in module order, then unknown names in selection order.
 * @param {ReadonlySet<string>} selected
 * @param {readonly ModuleInfo[]} modules
 * @returns {string[]}
 */
export function orderedSelection(selected, modules) {
  const known = new Set(modules.map((m) => m.name));
  return [...modules.filter((m) => selected.has(m.name)).map((m) => m.name), ...unknownNames(selected, known)];
}
