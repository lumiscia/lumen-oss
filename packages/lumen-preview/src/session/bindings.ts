import type { LumenBindings, LumenPreviewBindingSource } from "../index.js";
import type { LumenWorkerBindings } from "./types.js";

export function isLumenBindings(bindings: LumenPreviewBindingSource): bindings is LumenBindings {
  return typeof (bindings as LumenBindings).preview === "function";
}

export function hasPreviewWorker(
  bindings: LumenPreviewBindingSource,
): bindings is LumenWorkerBindings {
  return isLumenBindings(bindings) && typeof bindings.previewWorkerUrl === "function";
}
