import type * as previewBindings from "./lumen_wasm.js";
export { LumenMediaStore, LumenPreviewController, LumenRenderer } from "./lumen_wasm.js";

export type LumenPreviewBindingsModule = typeof previewBindings;

export interface LumenBindingsLike {
  readonly target: "bundler" | "browser" | "node";
  preview(): Promise<LumenPreviewBindingsModule>;
  previewWorkerUrl?(): string | URL;
}

export class LumenBindings implements LumenBindingsLike {
  readonly target: "bundler";
  preview(): Promise<LumenPreviewBindingsModule>;
  previewWorkerUrl(): string;
}

export function createLumenBindings(): LumenBindings;
