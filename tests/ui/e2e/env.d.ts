// Minimal ambient declarations for tests/ui/e2e/shots.mjs, so `npm run typecheck` checks it without
// @types/node or a playwright install (the only dev package allowed is typescript, DEVELOPMENT.md
// §4.2). Playwright itself is resolved at run time on the screenshot machine. Extend as needed.

declare module "node:child_process" {
  interface Readable {
    on(event: "data", listener: (chunk: { toString(): string }) => void): this;
    setEncoding(encoding: "utf8"): this;
  }
  interface ChildProcess {
    stdout: Readable;
    stderr: Readable;
    exitCode: number | null;
    kill(signal?: string): boolean;
    once(event: "exit", listener: (code: number | null) => void): this;
    once(event: "error", listener: (err: Error) => void): this;
  }
  export function spawn(command: string, args: string[], options?: { stdio?: ("pipe" | "ignore" | "inherit")[] }): ChildProcess;
}

declare module "node:fs/promises" {
  export function mkdir(path: string, options?: { recursive?: boolean }): Promise<unknown>;
  export function writeFile(path: string, data: string): Promise<void>;
}

declare module "node:path" {
  const path: { join(...parts: string[]): string; resolve(...parts: string[]): string };
  export default path;
}

declare module "node:process" {
  const process: {
    argv: string[];
    execPath: string;
    exitCode: number | undefined;
    stdout: { write(text: string): boolean };
    stderr: { write(text: string): boolean };
  };
  export default process;
}

declare module "playwright" {
  interface ConsoleMessage {
    type(): string;
    text(): string;
  }
  interface Locator {
    click(options?: { timeout?: number }): Promise<void>;
    check(): Promise<void>;
    uncheck(): Promise<void>;
    isChecked(): Promise<boolean>;
    isDisabled(): Promise<boolean>;
    inputValue(): Promise<string>;
    textContent(options?: { timeout?: number }): Promise<string | null>;
    focus(): Promise<void>;
    fill(value: string): Promise<void>;
    press(key: string): Promise<void>;
    waitFor(options?: { state?: "attached" | "detached" | "visible" | "hidden"; timeout?: number }): Promise<void>;
    screenshot(options: { path: string }): Promise<unknown>;
    first(): Locator;
    nth(index: number): Locator;
    locator(selector: string, options?: { hasText?: string | RegExp }): Locator;
    getByRole(role: string, options?: { name?: string | RegExp; exact?: boolean }): Locator;
    getByLabel(text: string | RegExp, options?: { exact?: boolean }): Locator;
    getByText(text: string | RegExp, options?: { exact?: boolean }): Locator;
    count(): Promise<number>;
  }
  interface Page {
    keyboard: { press(key: string): Promise<void> };
    goto(url: string): Promise<unknown>;
    on(event: "console", listener: (message: ConsoleMessage) => void): void;
    on(event: "pageerror", listener: (error: Error) => void): void;
    locator(selector: string, options?: { hasText?: string | RegExp }): Locator;
    getByRole(role: string, options?: { name?: string | RegExp; exact?: boolean }): Locator;
    getByLabel(text: string | RegExp, options?: { exact?: boolean }): Locator;
    getByText(text: string | RegExp, options?: { exact?: boolean }): Locator;
    waitForTimeout(ms: number): Promise<void>;
    screenshot(options: { path: string; fullPage?: boolean }): Promise<unknown>;
    evaluate<R>(fn: () => R | Promise<R>): Promise<R>;
    evaluate<R, A>(fn: (arg: A) => R | Promise<R>, arg: A): Promise<R>;
    close(): Promise<void>;
  }
  interface BrowserContext {
    newPage(): Promise<Page>;
    addInitScript(fn: () => void): Promise<void>;
    close(): Promise<void>;
  }
  interface Browser {
    newContext(options: {
      colorScheme: "light" | "dark";
      viewport: { width: number; height: number };
      deviceScaleFactor?: number;
    }): Promise<BrowserContext>;
    close(): Promise<void>;
  }
  export const chromium: { launch(options?: { headless?: boolean }): Promise<Browser> };
}
