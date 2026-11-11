import type {
  LumenModelAnimation,
  LumenModelCamera,
  LumenModelMaterial,
  VideoCodec,
} from "@lumiscia/lumen-types";

import type { RenderOutput } from "./output.js";

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
  readonly idempotencyKey?: string;
  /** Media by the alias the composition uses: a URL, or an upload to reference. */
  readonly media?: RenderMediaManifest;
  readonly webhookUrl?: string;
  /** Wins over the composition's `render_settings.quality`. Unset, the host's default applies. */
  readonly quality?: QualityPreset;
  /** Wins over the composition's `render_settings.codec`. Unset, the host's default applies. */
  readonly codec?: VideoCodec;
  /**
   * The format and frames to render. Unset, an MP4 of the whole timeline. `quality` and `codec`
   * apply to `mp4` only, and a host rejects them with another format.
   */
  readonly output?: RenderOutput;
}

/** `cancelled` is a render stopped by `cancelRender`, which only lumen-server supports. */
export type RenderStatus = "queued" | "processing" | "succeeded" | "failed" | "cancelled";

export interface RenderFailure {
  readonly code: string;
  readonly message: string;
  readonly retryable?: boolean;
}

/** A render as `render`, `getRender` and `waitForRender` return it. */
export interface Render {
  readonly id: string;
  readonly status: RenderStatus;
  /** From 0 to 1. Set when the API reported it, which `GET /renders/:id` does not. */
  readonly progress?: number;
  /** Absolute URL of the output, once the render has succeeded. Pass the render to `download`. */
  readonly url?: string;
  /** Why the render failed, when `status` is `failed`. */
  readonly error?: RenderFailure;
  /** Whether an earlier render with the same input answered this request. */
  readonly cached?: boolean;
  /** Render cost in millionths of a US dollar. Self-hosted servers report 0. */
  readonly costMicros?: number;
  readonly createdAt?: string;
  readonly inputHash?: string;
  readonly organizationId?: string;
  readonly outputExpiresAt?: string;
}

export type RenderMediaManifest = Record<string, string | TemporaryMedia>;

export type LumenMediaReference = `lumen:${string}`;

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

/** What the server decided an upload is by opening it, whatever its name and content type say. */
export type MediaKind = "image" | "video" | "audio" | "font" | "model" | "environment";

/** What opening an upload found. Each kind fills its own fields. */
export interface MediaMetadata {
  /** Images, video and environment maps. */
  readonly width?: number;
  readonly height?: number;
  /** Video and audio. */
  readonly durationMs?: number;
  readonly fps?: number;
  readonly frameCount?: number;
  readonly codec?: string;
  readonly hasAudio?: boolean;
  readonly sampleRate?: number;
  readonly channels?: number;
  /** Fonts. */
  readonly families?: readonly string[];
  readonly faces?: readonly {
    readonly family: string;
    readonly weight: number;
    readonly style: string;
  }[];
  /**
   * Models: the clips `animation`, the materials `texture_material` and the cameras `camera` of a
   * `model_3d` select.
   */
  readonly animations?: readonly LumenModelAnimation[];
  readonly materials?: readonly LumenModelMaterial[];
  readonly cameras?: readonly LumenModelCamera[];
}

export interface TemporaryMedia {
  readonly contentType: string;
  readonly createdAt: string;
  readonly expiresAt: string;
  readonly fileName: string;
  readonly id: string;
  readonly objectKey: string | null;
  readonly rejectionReason: string | null;
  readonly sizeBytes: number;
  readonly sourceType: "upload" | "url";
  readonly sourceUrl: string | null;
  readonly status: "pending" | "uploaded" | "validated" | "rejected";
  readonly uploadedBytes: number | null;
  /** Set once the upload is complete and was readable. */
  readonly kind?: MediaKind | null;
  /** Set with `kind`. */
  readonly metadata?: MediaMetadata | null;
}

export type UploadMediaBody = Blob | BufferSource | ReadableStream<Uint8Array>;

export interface UploadMediaOptions {
  readonly body: UploadMediaBody;
  /** Defaults to the file's name, or `upload`. */
  readonly fileName?: string;
  /** Defaults to the blob's type, then the file name's extension. A body with neither needs it. */
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
