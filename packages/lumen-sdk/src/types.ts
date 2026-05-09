export interface LumenOptions {
  readonly apiKey: string;
  readonly baseUrl?: string;
  readonly fetch?: typeof fetch;
  readonly websocket?: LumenWebSocketFactory;
}

export interface LumenWebSocketFactoryOptions {
  readonly headers: Headers;
}

export interface LumenWebSocketFactory {
  (url: URL, options: LumenWebSocketFactoryOptions): LumenWebSocket | Promise<LumenWebSocket>;
}

export interface LumenWebSocket {
  addEventListener(type: "close", listener: (event: CloseEvent) => void): void;
  addEventListener(type: "error", listener: (event: Event) => void): void;
  addEventListener(type: "message", listener: (event: MessageEvent) => void): void;
  addEventListener(type: "open", listener: () => void): void;
  accept?(options?: { allowHalfOpen?: boolean }): void;
  close(): void;
  send(data: string): void;
}

export interface RenderOptions {
  readonly signal?: AbortSignal;
  readonly idempotencyKey?: string;
  readonly media?: RenderMediaManifest;
  readonly webhookUrl?: string;
}

export interface RenderResult {
  readonly cached?: boolean;
  readonly id?: string;
  readonly render?: RenderJob;
  readonly error?: LumenApiError;
}

export interface RenderJob {
  readonly apiKeyId?: string;
  readonly costCents: number;
  readonly createdAt: string;
  readonly id: string;
  readonly inputHash: string;
  readonly organizationId: string;
  readonly outputExpiresAt?: string | null;
  readonly outputUrl?: string | null;
  readonly status: "failed" | "processing" | "queued" | "succeeded";
}

export type RenderMediaManifest = Record<string, string>;

export type LumenMediaReference = `lumen:${string}`;

export interface LumenApiError {
  readonly code: string;
  readonly message: string;
  readonly details?: unknown;
}

export type RenderEvent =
  | {
      readonly type: "render.queued";
      readonly renderId: string;
      readonly position?: number;
    }
  | {
      readonly type: "render.started";
      readonly renderId: string;
    }
  | {
      readonly type: "render.progress";
      readonly renderId: string;
      readonly progress: number;
      readonly frame?: number;
      readonly totalFrames?: number;
    }
  | {
      readonly type: "render.completed";
      readonly renderId: string;
      readonly url?: string;
      readonly artifactId?: string;
    }
  | {
      readonly type: "render.failed";
      readonly renderId: string;
      readonly error: LumenApiError;
    };

export interface RenderEventHandlers {
  readonly onEvent?: (event: RenderEvent) => void;
  readonly onError?: (error: unknown) => void;
  readonly onClose?: (event: CloseEvent) => void;
}

export interface RenderEventSubscription {
  readonly close: () => void;
}

export interface WaitForRenderOptions {
  readonly onEvent?: (event: RenderEvent) => void;
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

export interface CreateMediaUploadOptions {
  readonly contentType: string;
  readonly fileName: string;
  readonly sizeBytes: number;
  readonly signal?: AbortSignal;
}

export interface CreateMediaUploadResult {
  readonly maxBytes: number;
  readonly media: TemporaryMedia;
  readonly uploadUrl: string;
}

export type UploadMediaBody = Blob | BufferSource | ReadableStream<Uint8Array>;

export interface UploadMediaOptions extends CreateMediaUploadOptions {
  readonly body: UploadMediaBody;
  readonly multipart?: boolean | MultipartUploadOptions;
}

export interface MultipartUploadOptions {
  readonly partSizeBytes?: number;
}

export interface CreateUrlMediaOptions {
  readonly fileName?: string;
  readonly signal?: AbortSignal;
  readonly url: string | URL;
}

export interface MediaUploadPart {
  readonly etag: string;
  readonly partNumber: number;
}

export interface CreateMultipartUploadResult {
  readonly key: string;
  readonly uploadId: string;
}

export interface CompleteMultipartUploadOptions {
  readonly id: string;
  readonly parts: readonly MediaUploadPart[];
  readonly signal?: AbortSignal;
  readonly uploadId: string;
}
