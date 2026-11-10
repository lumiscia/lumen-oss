export type LumenMediaErrorCode =
  | "decode_failed"
  | "frame_unavailable"
  | "invalid_source"
  | "source_not_registered"
  | "track_not_decodable"
  | "unsupported_container";

export class LumenMediaError extends Error {
  readonly code: LumenMediaErrorCode;
  readonly cause: unknown;
  /** The id of the image, model or video source that failed, when one is involved. */
  readonly sourceId: string | undefined;

  constructor(
    code: LumenMediaErrorCode,
    message: string,
    options: { cause?: unknown; sourceId?: string } = {},
  ) {
    super(message);
    this.name = "LumenMediaError";
    this.code = code;
    this.cause = options.cause;
    this.sourceId = options.sourceId;
  }
}

/**
 * `error` as a media error naming `sourceId`. One that already names its source is returned as it
 * is, and one that doesn't keeps its code.
 */
export function toMediaError(
  code: LumenMediaErrorCode,
  sourceId: string,
  message: string,
  error: unknown,
): LumenMediaError {
  if (error instanceof LumenMediaError && error.sourceId !== undefined) {
    return error;
  }

  const causeMessage = errorMessage(error);
  return new LumenMediaError(
    error instanceof LumenMediaError ? error.code : code,
    causeMessage ? `${message}: ${causeMessage}` : message,
    { cause: error, sourceId },
  );
}

function errorMessage(error: unknown): string | null {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "string") {
    return error;
  }
  return null;
}
