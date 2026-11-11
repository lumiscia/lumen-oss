import type { LumenDiagnostic, MediaFailure, MediaFailureCode } from "@lumiscia/lumen-types";

import { LumenMediaError, type LumenMediaErrorCode } from "../media/errors.js";

export function describeError(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** The diagnostics a wasm parse failure carries, or none. */
export function errorDiagnostics(error: unknown): LumenDiagnostic[] {
  const diagnostics: unknown =
    typeof error === "object" && error !== null ? Reflect.get(error, "diagnostics") : undefined;
  return Array.isArray(diagnostics) ? (diagnostics as LumenDiagnostic[]) : [];
}

const MEDIA_ERROR_CODES: Record<LumenMediaErrorCode, MediaFailureCode> = {
  decode_failed: "media.decode",
  frame_unavailable: "media.decode",
  invalid_source: "media.not_found",
  source_not_registered: "media.not_found",
  track_not_decodable: "media.unsupported",
  unsupported_container: "media.unsupported",
};

/**
 * The structured media failure an error carries, or null. The wasm bindings attach one to the
 * errors they throw for a source the engine can't use; a `LumenMediaError` from the browser side
 * of the preview names its source and is described in the same terms.
 */
export function errorMediaFailure(error: unknown): MediaFailure | null {
  if (error instanceof LumenMediaError) {
    return error.sourceId === undefined
      ? null
      : {
          code: MEDIA_ERROR_CODES[error.code],
          source: error.sourceId,
          wanted: [],
          details: error.message,
        };
  }
  const media: unknown =
    typeof error === "object" && error !== null ? Reflect.get(error, "media") : undefined;
  return typeof media === "object" &&
    media !== null &&
    typeof Reflect.get(media, "code") === "string" &&
    typeof Reflect.get(media, "source") === "string"
    ? (media as MediaFailure)
    : null;
}

export function reportConsoleError(scope: string, error: unknown): void {
  console.error(`[LumenPreviewSession] ${scope}`, error);
}
