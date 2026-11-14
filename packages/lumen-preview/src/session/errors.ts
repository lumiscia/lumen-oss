import type { Diagnostic } from "@lumiscia/lumen-types";

export { errorMediaFailure } from "../media/errors.js";

export function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** The diagnostics a wasm parse failure carries, or none. */
export function errorDiagnostics(error: unknown): Diagnostic[] {
  const diagnostics: unknown =
    typeof error === "object" && error !== null ? Reflect.get(error, "diagnostics") : undefined;
  return Array.isArray(diagnostics) ? (diagnostics as Diagnostic[]) : [];
}

/**
 * The engine's code on a failed render as the one diagnostic it is, or none for an error that has
 * no `render.*` code, such as a media failure, which `media` says.
 */
export function errorRenderDiagnostics(error: unknown): Diagnostic[] {
  const code: unknown =
    typeof error === "object" && error !== null ? Reflect.get(error, "code") : undefined;
  return typeof code === "string" && code.startsWith("render.")
    ? [{ severity: "error", code, path: "", message: describeError(error) }]
    : [];
}

export function reportConsoleError(scope: string, error: unknown): void {
  console.error(`[LumenPreviewSession] ${scope}`, error);
}
