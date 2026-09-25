// UI-internal types shared by app.js, the screens and the components (not IPC contracts; those
// are in ui/types.d.ts).

import type { ActiveJob, AppInfo, Settings, ToolStatus } from "../types";
import type { Store } from "./store.js";

export type Api = typeof import("../api/ipc.js");
export type ApiMode = "ipc" | "mock";

export type AppState = {
  mode: ApiMode;
  appInfo: AppInfo | null;
  settings: Settings | null;
  tools: ToolStatus[] | null;
  /** The one active job, app-wide; polled with `job_active` while non-null. */
  activeJob: ActiveJob | null;
  /** iLEAPP's own zone list for its installed version (lib/timezones.js); fallbacks are never cached. */
  timezones: { version: string; list: string[] } | null;
};

export type ScreenContext = {
  api: Api;
  store: Store<AppState>;
  /** The route's query parameters. */
  params: Record<string, string>;
  /** Goes to a `#/…` href (lib/router.js `routeHref`). */
  navigate: (href: string) => void;
};

/** What every screen and stateful component returns (DEVELOPMENT.md §4.6). */
export type View = { node: HTMLElement; dispose: () => void };
