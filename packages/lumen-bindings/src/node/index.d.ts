export * from "./lumen_wasm.js";
import type * as previewBindings from "./lumen_wasm.js";

export type LumenPreviewBindingsModule = typeof previewBindings;

export interface LumenBindingsLike {
  readonly target: "bundler" | "browser" | "node";
  preview(): Promise<LumenPreviewBindingsModule>;
  previewWorkerUrl?(): string | URL;
}

export class LumenBindings implements LumenBindingsLike {
  readonly target: "node";
  preview(): Promise<LumenPreviewBindingsModule>;
}

export function createLumenBindings(): LumenBindings;
