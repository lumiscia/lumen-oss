import type { LumenDiagnostic } from "@lumiscia/lumen-types";

export { errorMediaFailure } from "../media/errors.js";

export function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** The diagnostics a wasm parse failure carries, or none. */
export function errorDiagnostics(error: unknown): LumenDiagnostic[] {
  const diagnostics: unknown =
    typeof error === "object" && error !== null ? Reflect.get(error, "diagnostics") : undefined;
  return Array.isArray(diagnostics) ? (diagnostics as LumenDiagnostic[]) : [];
}

export function reportConsoleError(scope: string, error: unknown): void {
  console.error(`[LumenPreviewSession] ${scope}`, error);
}
