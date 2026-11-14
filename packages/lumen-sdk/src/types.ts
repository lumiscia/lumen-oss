import type {
  Render as WireRender,
  RenderFailure,
  RenderOutput,
  VideoBitDepth,
  VideoCodec,
} from "@lumiscia/lumen-types";

export interface LumenOptions {
  readonly apiKey: string;
  /** API base URL. Defaults to the hosted Lumiscia API, `https://api.lumiscia.com/v1`. */
  readonly baseUrl?: string;
  readonly fetch?: typeof fetch;
}

/**
 * A quality preset name. Every host defines `draft`, `standard`, `high` and `lossless`; operators
 * can define others.
 */
export type QualityPreset = "draft" | "standard" | "high" | "lossless" | (string & {});

export interface RenderOptions {
  readonly signal?: AbortSignal;
  /** Only the hosted API honours it; `lumen-server` ignores the header and runs every request. */
  readonly idempotencyKey?: string;
  /** Remote media by the alias the composition uses, as an `http` or `https` URL. */
  readonly media?: RenderMediaManifest;
  readonly webhookUrl?: string;
  /** Unset, the host's default applies. */
  readonly quality?: QualityPreset;
  /** Unset, a `mov` or `webm` gets its own codec and an `mp4` the host's default. */
  readonly codec?: VideoCodec;
  /**
   * 10 encodes `hevc` or `av1` at 10 bits per sample. Unset, 8, or 10 for `prores`. A host that
   * can't write the depth rejects the render.
   */
  readonly bitDepth?: VideoBitDepth;
  /**
   * Keeps the frame's transparency as an alpha channel, in a `mov` (ProRes 4444) or `webm` (VP9).
   * Unset, the video is opaque. A host rejects alpha in a format or encoder that can't hold it
   * rather than rendering opaque.
   */
  readonly alpha?: boolean;
  /**
   * The format and frames to render. Unset, an MP4 of the whole timeline. `quality`, `codec`,
   * `bitDepth` and `alpha` apply to videos only, and a host rejects them with another format.
   */
  readonly output?: RenderOutput;
}

/** What every render has, whatever its status. */
interface RenderFields extends Partial<
  Pick<
    WireRender,
    "costMicros" | "createdAt" | "durationMs" | "inputHash" | "organizationId" | "outputBytes"
  >
> {
  readonly id: string;
  /** From 0 to 1. 0 when the API reports none. */
  readonly progress: number;
  /** Whether an earlier render with the same input answered this request. Only `render()` sets it. */
  readonly cached?: boolean;
  /** When the hosted API deletes the output. `lumen-server` sends none. */
  readonly outputExpiresAt?: string;
}

/** A render as `render`, `getRender` and `waitForRender` return it: its `status` says what else it has. */
export type Render =
  | (RenderFields & { readonly status: "queued" | "processing" })
  | SucceededRender
  | FailedRender;

export interface SucceededRender extends RenderFields {
  readonly status: "succeeded";
  /** Absolute URL of the output. Pass the render to `download`. */
  readonly url: string;
}

export interface FailedRender extends RenderFields {
  readonly status: "failed" | "cancelled";
  readonly error: RenderFailure;
}

/** The statuses a render can have. `cancelled` is a render stopped by `cancelRender`, which only lumen-server supports. */
export type RenderStatus = Render["status"];

/** Remote media by alias: the alias is what a node's source says, the value an `http` or `https` URL. */
export type RenderMediaManifest = Record<string, string>;

/** What a node's source says to use an uploaded file: `asset:<id>`. */
export type AssetSource = `asset:${string}`;

/** What `onProgress` receives while a render is waited on. */
export interface RenderProgress {
  readonly id: string;
  readonly status: RenderStatus;
  /** From 0 to 1. */
  readonly progress: number;
}

export interface WaitForRenderOptions {
  /** Called when the status or progress changes, from the render's socket and from reads. */
  readonly onProgress?: (progress: RenderProgress) => void;
  readonly signal?: AbortSignal;
}

export type UploadMediaBody = Blob | BufferSource | ReadableStream<Uint8Array>;

export interface UploadMediaOptions {
  readonly body: UploadMediaBody;
  /** Defaults to the file's name, or `upload`. */
  readonly fileName?: string;
  /**
   * Defaults to the blob's type, then the file name's extension, then `application/octet-stream`.
   * The host reads the file's bytes, not this, to tell what it is.
   */
  readonly contentType?: string;
  /** Required for a stream. Taken from any other body. */
  readonly sizeBytes?: number;
  readonly signal?: AbortSignal;
}

export interface CreateUrlMediaOptions {
  readonly fileName?: string;
  readonly signal?: AbortSignal;
  readonly url: string | URL;
}
