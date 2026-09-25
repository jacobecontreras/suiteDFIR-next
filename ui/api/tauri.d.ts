// The subset of the Tauri 2 global API (`app.withGlobalTauri`) that ui/api/ipc.js uses. Only
// ipc.js may touch `window.__TAURI__`; api/index.js may only test for its existence
// (DEVELOPMENT.md §4.6).

interface TauriChannel<T> {
  onmessage: (message: T) => void;
}

interface TauriDialogFilter {
  name: string;
  extensions: string[];
}

interface TauriGlobal {
  core: {
    invoke(cmd: string, args?: Record<string, unknown>): Promise<unknown>;
    Channel: new <T>() => TauriChannel<T>;
  };
  dialog: {
    open(options: {
      title?: string;
      directory?: boolean;
      multiple?: false;
      filters?: TauriDialogFilter[];
      defaultPath?: string;
    }): Promise<string | string[] | null>;
    save(options: { title?: string; filters?: TauriDialogFilter[]; defaultPath?: string }): Promise<string | null>;
  };
}

interface Window {
  __TAURI__?: TauriGlobal;
}
