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

  constructor(code: LumenMediaErrorCode, message: string, options: { cause?: unknown } = {}) {
    super(message);
    this.name = "LumenMediaError";
    this.code = code;
    this.cause = options.cause;
  }
}

export function toMediaError(
  code: LumenMediaErrorCode,
  message: string,
  error: unknown,
): LumenMediaError {
  if (error instanceof LumenMediaError) {
    return error;
  }

  const causeMessage = errorMessage(error);
  return new LumenMediaError(code, causeMessage ? `${message}: ${causeMessage}` : message, {
    cause: error,
  });
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
