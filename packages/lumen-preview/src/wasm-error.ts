import type { Diagnostic, DiagnosticCode, MediaFailure } from "@lumiscia/lumen-types";

/** What an error thrown by the WASM bindings carries besides its message. */
export interface WasmErrorFields {
  /** The engine's diagnostic code; absent on the plain-string failures of a caller mistake. */
  code?: DiagnosticCode;
  media?: MediaFailure;
  diagnostics: Diagnostic[];
  /** A video frame that isn't loaded yet: load it and render again. */
  frameNotLoaded: boolean;
}

/** Reads the fields the bindings set on an error; anything else reads as having none. */
export function readWasmError(error: unknown): WasmErrorFields {
  if (typeof error !== "object" || error === null) {
    return { diagnostics: [], frameNotLoaded: false };
  }
  const code: unknown = Reflect.get(error, "code");
  const media: unknown = Reflect.get(error, "media");
  const diagnostics: unknown = Reflect.get(error, "diagnostics");
  return {
    ...(typeof code === "string" ? { code } : {}),
    ...(isMediaFailure(media) ? { media } : {}),
    diagnostics: Array.isArray(diagnostics) ? (diagnostics as Diagnostic[]) : [],
    frameNotLoaded: Reflect.get(error, "frameNotLoaded") === true,
  };
}

function isMediaFailure(value: unknown): value is MediaFailure {
  return (
    typeof value === "object" &&
    value !== null &&
    typeof Reflect.get(value, "code") === "string" &&
    typeof Reflect.get(value, "source") === "string"
  );
}
