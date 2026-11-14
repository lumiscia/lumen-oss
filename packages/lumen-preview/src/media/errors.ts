import type { MediaFailure, MediaFailureCode, AssetKind } from "@lumiscia/lumen-types";

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
  readonly wanted?: readonly AssetKind[];
  /** The kind the source is, when it was established. */
  readonly found?: AssetKind;
}

export class LumenMediaError extends Error {
  readonly code: LumenMediaErrorCode;
  readonly cause: unknown;
  /** The id of the image, model or video source that failed, when one is involved. */
  readonly sourceId: string | undefined;
  readonly wanted: readonly AssetKind[];
  readonly found: AssetKind | undefined;

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

const MEDIA_ERROR_CODES: Record<LumenMediaErrorCode, MediaFailureCode> = {
  decode_failed: "media.decode",
  frame_unavailable: "media.decode",
  invalid_source: "media.not_found",
  no_video_track: "media.wrong_kind",
  source_not_registered: "media.not_found",
  track_not_decodable: "media.unsupported",
  track_not_found: "media.not_found",
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
          wanted: error.wanted,
          ...(error.found === undefined ? {} : { found: error.found }),
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
