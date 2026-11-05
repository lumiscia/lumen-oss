export interface LumenOptions {
  readonly apiKey: string;
  /** API base URL. Defaults to the hosted Lumiscia API, `https://api.lumiscia.com/v1`. */
  readonly baseUrl?: string;
  readonly fetch?: typeof fetch;
}

export interface RenderOptions {
  readonly signal?: AbortSignal;
  readonly idempotencyKey?: string;
  /** Media by the alias the composition uses: a URL, or an upload to reference. */
  readonly media?: RenderMediaManifest;
  readonly webhookUrl?: string;
}

export type RenderStatus = "queued" | "processing" | "succeeded" | "failed";

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
}

export type UploadMediaBody = Blob | BufferSource | ReadableStream<Uint8Array>;

export interface UploadMediaOptions {
  readonly body: UploadMediaBody;
  /** Defaults to the file's name, or `upload`. */
  readonly fileName?: string;
  /** Defaults to the blob's type, or `application/octet-stream`. */
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
