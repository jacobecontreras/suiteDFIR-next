// UI-internal types shared by app.js, the screens and the components (not IPC contracts; those
// are in ui/types.d.ts).

import type { ActiveJob, AppInfo, Settings, ToolId, ToolStatus } from "../types";
import type { InstallProgress } from "./install.js";
import type { JobStreams } from "./jobstream.js";
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
  /** Parser installs and imports started on the Settings screen, by tool (lib/install.js). */
  installs: Partial<Record<ToolId, InstallProgress>>;
};

export type ScreenContext = {
  api: Api;
  store: Store<AppState>;
  /** The route's query parameters. */
  params: Record<string, string>;
  /** Goes to a `#/…` href (lib/router.js `routeHref`). */
  navigate: (href: string) => void;
  /** The active job's event stream (lib/jobstream.js), shared by New run, Run and Acquire. */
  jobs: JobStreams;
};

/** What every screen and stateful component returns (DEVELOPMENT.md §4.6). */
export type View = { node: HTMLElement; dispose: () => void };
