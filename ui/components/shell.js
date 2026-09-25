// @ts-check
/**
 * The app chrome: top bar (app name, nav, active-job indicator), the persistent MOCK DATA and
 * DEV OVERRIDE banners, and the `<main>` element screens render into.
 */
import { h } from "../lib/dom.js";
import { routeHref } from "../lib/router.js";
import { watch } from "../lib/store.js";
import { icon, phaseLabel, toolName } from "../lib/view.js";

/** @typedef {import("../lib/context").AppState} AppState */
/** @typedef {import("../lib/store.js").Store<AppState>} AppStore */
/** @typedef {import("../types").ActiveJob} ActiveJob */

const NAV = [
  { name: "cases", label: "Cases", matches: ["cases", "case", "new-run"] },
  { name: "settings", label: "Settings", matches: ["settings"] },
];

/**
 * @param {{ store: AppStore }} ctx
 * @returns {{ node: HTMLElement, main: HTMLElement, setRoute: (name: string) => void, dispose: () => void }}
 */
export function shell({ store }) {
  const links = NAV.map((item) => ({ item, a: h("a", { class: "nav-link", href: routeHref(item.name) }, item.label) }));
  const indicator = h("div", { class: "job-indicator-slot", "aria-live": "polite" });
  const banners = h("div", { class: "banners" });
  const main = h("main", { class: "content", id: "main", tabindex: "-1" });

  const node = h(
    "div",
    { class: "shell" },
    h("a", { class: "skip-link", href: "#main", onClick: skipToMain }, "Skip to content"),
    h(
      "header",
      { class: "topbar" },
      h("a", { class: "app-name", href: routeHref("cases") }, "suiteDFIR"),
      h("nav", { class: "nav", "aria-label": "Main" }, links.map((l) => l.a)),
      h("div", { class: "topbar-spacer" }),
      indicator,
    ),
    banners,
    main,
  );

  /** @param {MouseEvent} event */
  function skipToMain(event) {
    event.preventDefault();
    main.focus();
  }

  const unwatchJob = watch(store, (s) => s.activeJob, (job) => indicator.replaceChildren(jobIndicator(job) ?? ""));
  const unwatchBanners = watch(
    store,
    (s) => `${s.mode}|${devOverride(s)}`,
    () => {
      const state = store.get();
      banners.replaceChildren(
        ...[
          state.mode === "mock" &&
            h(
              "div",
              { class: "banner banner-mock", role: "note" },
              icon("info"),
              h("strong", null, "MOCK DATA"),
              h("span", null, "Development mock: nothing here is real, and no parser or device is used."),
            ),
          devOverride(state) &&
            h(
              "div",
              { class: "banner banner-dev", role: "note" },
              icon("alert-triangle"),
              h("strong", null, "DEV OVERRIDE"),
              h("span", null, "A development test double replaces the real tools. Results are not evidence."),
            ),
        ].filter((x) => x instanceof Node),
      );
    },
  );

  return {
    node,
    main,
    setRoute(name) {
      for (const { item, a } of links) {
        if (item.matches.includes(name)) a.setAttribute("aria-current", "page");
        else a.removeAttribute("aria-current");
      }
    },
    dispose() {
      unwatchJob();
      unwatchBanners();
    },
  };
}

/** @param {Readonly<AppState>} s */
function devOverride(s) {
  return s.appInfo?.dev_override === true || (s.tools ?? []).some((t) => t.state === "dev_override");
}

/**
 * @param {ActiveJob | null} job
 * @returns {HTMLElement | null}
 */
function jobIndicator(job) {
  if (!job) return null;
  const what = job.kind === "run" ? `Run in progress: ${toolName(job.tool)}` : "Acquisition in progress";
  return h(
    "a",
    { class: "job-indicator", href: routeHref("case", { path: job.case_path }), title: "Open the case of the active job" },
    h("span", { class: "pulse", "aria-hidden": "true" }),
    h("span", null, what),
    h("span", { class: "job-phase" }, phaseLabel(job.phase)),
  );
}
