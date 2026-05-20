export * from "./lumen_wasm.js";
import type * as previewBindings from "./lumen_wasm.js";

export type LumenPreviewBindingsModule = typeof previewBindings;

export interface LumenBrowserBindingsOptions {
  previewWasmUrl?: string | URL | Request | Response | BufferSource | WebAssembly.Module;
  previewWorkerUrl?: string | URL;
}

export interface LumenBindingsLike {
  readonly target: "bundler" | "browser" | "node";
  preview(): Promise<LumenPreviewBindingsModule>;
  previewWorkerUrl?(): string | URL;
}

export class LumenBindings implements LumenBindingsLike {
  readonly target: "browser";
  constructor(options?: LumenBrowserBindingsOptions);
  preview(): Promise<LumenPreviewBindingsModule>;
  previewWorkerUrl(): string | URL | undefined;
}

export function createLumenBindings(options?: LumenBrowserBindingsOptions): LumenBindings;
