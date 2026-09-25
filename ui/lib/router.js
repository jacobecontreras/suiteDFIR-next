// @ts-check
/**
 * Hash routes: `#/<name>?<query>`, e.g. `#/case?path=%2FUsers%2F…`. Paths travel in the query,
 * so they may contain `/`. Secrets never go into a route.
 */

/**
 * @typedef {object} Route
 * @property {string} name The first path segment; `cases` for an empty hash.
 * @property {Record<string, string>} params The decoded query parameters.
 */

/**
 * @param {string} hash `location.hash`, with or without the leading `#`.
 * @returns {Route}
 */
export function parseRoute(hash) {
  const raw = hash.startsWith("#") ? hash.slice(1) : hash;
  const q = raw.indexOf("?");
  const pathPart = q === -1 ? raw : raw.slice(0, q);
  const query = q === -1 ? "" : raw.slice(q + 1);
  const name = pathPart.split("/").filter(Boolean)[0] ?? "cases";
  /** @type {Record<string, string>} */
  const params = {};
  for (const [key, value] of new URLSearchParams(query)) params[key] = value;
  return { name, params };
}

/**
 * @param {string} name
 * @param {Record<string, string>} [params]
 * @returns {string} a `#/…` href
 */
export function routeHref(name, params = {}) {
  const query = new URLSearchParams(params).toString();
  return `#/${name}${query ? `?${query}` : ""}`;
}
