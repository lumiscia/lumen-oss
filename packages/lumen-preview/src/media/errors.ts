import type { MediaFailureKind } from "@lumiscia/lumen-types";

/**
 * Why a media source failed in the browser. They match the engine's reasons for a source it can't
 * use: `invalid_source`, `source_not_registered` and `track_not_found` are `media.not_found`;
 * `no_video_track` is `media.wrong_kind`; `track_not_decodable` and `unsupported_container` are
 * `media.unsupported`, for data the browser identifies and doesn't decode; `decode_failed` and
 * `frame_unavailable` are `media.decode`, for data that is corrupt or in no format it identifies.
 */
export type LumenMediaErrorCode =
  | "decode_failed"
  | "frame_unavailable"
  | "invalid_source"
  | "no_video_track"
  | "source_not_registered"
  | "track_not_decodable"
  | "track_not_found"
  | "unsupported_container";

/** What a media error says about the kinds of media involved. */
export interface LumenMediaErrorKinds {
  /** The kinds the use reads. */
  readonly wanted?: readonly MediaFailureKind[];
  /** The kind the source is, when it was established. */
  readonly found?: MediaFailureKind;
}

export class LumenMediaError extends Error {
  readonly code: LumenMediaErrorCode;
  readonly cause: unknown;
  /** The id of the image, model or video source that failed, when one is involved. */
  readonly sourceId: string | undefined;
  readonly wanted: readonly MediaFailureKind[];
  readonly found: MediaFailureKind | undefined;

  constructor(
    code: LumenMediaErrorCode,
    message: string,
    options: { cause?: unknown; sourceId?: string } & LumenMediaErrorKinds = {},
  ) {
    super(message);
    this.name = "LumenMediaError";
    this.code = code;
    this.cause = options.cause;
    this.sourceId = options.sourceId;
    this.wanted = options.wanted ?? [];
    this.found = options.found;
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
  kinds: LumenMediaErrorKinds = {},
): LumenMediaError {
  if (error instanceof LumenMediaError && error.sourceId !== undefined) {
    return error;
  }

  const causeMessage = errorMessage(error);
  return new LumenMediaError(
    error instanceof LumenMediaError ? error.code : code,
    causeMessage ? `${message}: ${causeMessage}` : message,
    { cause: error, sourceId, ...kinds },
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
