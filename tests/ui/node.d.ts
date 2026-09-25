// Minimal type declarations for the Node built-ins used by the UI tests, so `tsc` can check
// them without @types/node (the only dev package allowed is typescript, DEVELOPMENT.md §4.2).
// Extend as tests need more of the API.

declare module "node:test" {
  export function test(name: string, fn: () => void | Promise<void>): Promise<void>;
}

declare module "node:fs" {
  export function readFileSync(path: string | URL, encoding: "utf8"): string;
}

declare module "node:assert/strict" {
  interface Assert {
    (value: unknown, message?: string): asserts value;
    ok(value: unknown, message?: string): asserts value;
    equal<T>(actual: unknown, expected: T, message?: string): asserts actual is T;
    notEqual(actual: unknown, expected: unknown, message?: string): void;
    deepEqual<T>(actual: unknown, expected: T, message?: string): asserts actual is T;
    throws(fn: () => unknown, error?: Function | RegExp | object, message?: string): void;
    rejects(
      promise: Promise<unknown> | (() => Promise<unknown>),
      error?: Function | RegExp | object,
      message?: string,
    ): Promise<void>;
    match(value: string, regexp: RegExp, message?: string): void;
  }
  const assert: Assert;
  export default assert;
}
