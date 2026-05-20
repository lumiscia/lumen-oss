import type * as previewBindings from "./lumen_wasm.js";
export type { InitInput, InitOutput, SyncInitInput } from "./lumen_wasm.js";
export { initSync, LumenMediaStore, LumenPreviewController, LumenRenderer } from "./lumen_wasm.js";

export interface LumenBrowserBindingsOptions {
  previewWasmUrl?: string | URL | Request | Response | BufferSource | WebAssembly.Module;
  previewWorkerUrl?: string | URL;
}

export type LumenPreviewBindingsModule = typeof previewBindings;

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
