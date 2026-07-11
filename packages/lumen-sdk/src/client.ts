import { Composition, type LumenComposition } from "@lumiscia/lumen-shared";

import { normalizeApiError } from "./errors.js";
import type {
  CompleteMultipartUploadOptions,
  CreateMediaUploadOptions,
  CreateMediaUploadResult,
  CreateMultipartUploadResult,
  CreateUrlMediaOptions,
  LumenOptions,
  MediaUploadPart,
  RenderEvent,
  RenderEventHandlers,
  RenderEventSubscription,
  LumenWebSocketFactory,
  RenderOptions,
  RenderResult,
  TemporaryMedia,
  UploadMediaOptions,
  WaitForRenderOptions,
} from "./types.js";

const defaultMultipartPartSizeBytes = 8 * 1024 * 1024;
const defaultBaseUrl = "https://lumiscia.com/api/v1";

export class Lumen {
  readonly #apiKey: string;
  readonly #baseUrl: URL;
  readonly #fetch: typeof fetch;
  readonly #websocket: LumenWebSocketFactory | undefined;

  constructor(options: LumenOptions) {
    this.#apiKey = options.apiKey;
    this.#baseUrl = new URL(options.baseUrl ?? defaultBaseUrl);
    this.#fetch = options.fetch ?? globalThis.fetch;
    this.#websocket = options.websocket;
  }

  async render(
    composition: Composition | LumenComposition,
    options: RenderOptions = {},
  ): Promise<RenderResult> {
    const response = await this.#fetch(
      this.#apiUrl("/renders"),
      this.#jsonRequest(
        "POST",
        {
          composition: composition instanceof Composition ? composition.toJSON() : composition,
          media: options.media ?? {},
          ...(options.webhookUrl !== undefined ? { webhookUrl: options.webhookUrl } : {}),
        },
        {
          ...(options.idempotencyKey !== undefined
            ? { idempotencyKey: options.idempotencyKey }
            : {}),
          ...(options.signal !== undefined ? { signal: options.signal } : {}),
        },
      ),
    );
    const body = (await response.json().catch(() => undefined)) as
      | Partial<RenderResult>
      | undefined;

    if (!response.ok) {
      return {
        error: normalizeApiError(body, response),
      };
    }

    return renderResult(body);
  }

  async getRender(id: string, options: { signal?: AbortSignal } = {}): Promise<RenderResult> {
    const response = await this.#fetch(this.#apiUrl(`/renders/${encodeURIComponent(id)}`), {
      headers: this.#headers(),
      method: "GET",
      ...(options.signal !== undefined ? { signal: options.signal } : {}),
    });
    const body = (await response.json().catch(() => undefined)) as
      | Partial<RenderResult>
      | undefined;

    if (!response.ok) {
      return {
        error: normalizeApiError(body, response),
      };
    }

    return renderResult(body);
  }

  async getRenderArtifact(id: string, options: { signal?: AbortSignal } = {}): Promise<Blob> {
    const response = await this.#fetch(
      this.#apiUrl(`/renders/${encodeURIComponent(id)}/artifact`),
      {
        headers: this.#headers(),
        method: "GET",
        ...(options.signal !== undefined ? { signal: options.signal } : {}),
      },
    );

    if (!response.ok) {
      const body = (await response.json().catch(() => undefined)) as
        | Partial<RenderResult>
        | undefined;
      throw normalizeApiError(body, response);
    }

    return response.blob();
  }

  async createMediaUpload(options: CreateMediaUploadOptions): Promise<CreateMediaUploadResult> {
    const response = await this.#fetch(
      this.#apiUrl("/media"),
      this.#jsonRequest(
        "POST",
        {
          contentType: options.contentType,
          fileName: options.fileName,
          sizeBytes: options.sizeBytes,
        },
        optionalSignal(options.signal),
      ),
    );

    return this.#expectJson<CreateMediaUploadResult>(response);
  }

  async uploadMedia(options: UploadMediaOptions): Promise<TemporaryMedia> {
    if (options.multipart) {
      return this.uploadMediaMultipart(options);
    }

    const upload = await this.createMediaUpload(options);
    const response = await this.#fetch(this.#url(upload.uploadUrl), {
      body: bodyAsRequestBody(options.body),
      headers: this.#headers({
        "content-length": options.sizeBytes.toString(),
        "content-type": options.contentType,
      }),
      method: "PUT",
      ...(options.signal !== undefined ? { signal: options.signal } : {}),
    });
    const body = await this.#expectJson<{ media: TemporaryMedia }>(response);
    return body.media;
  }

  async createUrlMedia(options: CreateUrlMediaOptions): Promise<TemporaryMedia> {
    const response = await this.#fetch(
      this.#apiUrl("/media/url"),
      this.#jsonRequest(
        "POST",
        {
          ...(options.fileName !== undefined ? { fileName: options.fileName } : {}),
          url: String(options.url),
        },
        optionalSignal(options.signal),
      ),
    );
    const body = await this.#expectJson<{ media: TemporaryMedia }>(response);
    return body.media;
  }

  async createMediaMultipartUpload(
    id: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<CreateMultipartUploadResult> {
    const response = await this.#fetch(this.#apiUrl(`/media/${id}/multipart`), {
      headers: this.#headers(),
      method: "POST",
      ...(options.signal !== undefined ? { signal: options.signal } : {}),
    });
    return this.#expectJson<CreateMultipartUploadResult>(response);
  }

  async uploadMediaMultipartPart(options: {
    readonly body: BodyInit;
    readonly id: string;
    readonly partNumber: number;
    readonly signal?: AbortSignal;
    readonly uploadId: string;
  }): Promise<MediaUploadPart> {
    const response = await this.#fetch(
      this.#apiUrl(
        `/media/${options.id}/multipart/parts/${options.partNumber}?uploadId=${encodeURIComponent(
          options.uploadId,
        )}`,
      ),
      {
        body: options.body,
        headers: this.#headers(),
        method: "PUT",
        ...(options.signal !== undefined ? { signal: options.signal } : {}),
      },
    );
    return this.#expectJson<MediaUploadPart>(response);
  }

  async completeMediaMultipartUpload(
    options: CompleteMultipartUploadOptions,
  ): Promise<TemporaryMedia> {
    const response = await this.#fetch(
      this.#apiUrl(
        `/media/${options.id}/multipart/complete?uploadId=${encodeURIComponent(options.uploadId)}`,
      ),
      this.#jsonRequest("POST", { parts: options.parts }, optionalSignal(options.signal)),
    );
    const body = await this.#expectJson<{ media: TemporaryMedia }>(response);
    return body.media;
  }

  async uploadMediaMultipart(options: UploadMediaOptions): Promise<TemporaryMedia> {
    const upload = await this.createMediaUpload(options);
    const multipart = await this.createMediaMultipartUpload(upload.media.id, {
      ...optionalSignal(options.signal),
    });
    const partSizeBytes =
      typeof options.multipart === "object"
        ? (options.multipart.partSizeBytes ?? defaultMultipartPartSizeBytes)
        : defaultMultipartPartSizeBytes;
    const parts = await this.#uploadMultipartBody({
      body: options.body,
      id: upload.media.id,
      partSizeBytes,
      ...optionalSignal(options.signal),
      sizeBytes: options.sizeBytes,
      uploadId: multipart.uploadId,
    });

    return this.completeMediaMultipartUpload({
      id: upload.media.id,
      parts,
      ...optionalSignal(options.signal),
      uploadId: multipart.uploadId,
    });
  }

  async subscribeToRender(
    renderId: string,
    handlers: RenderEventHandlers,
  ): Promise<RenderEventSubscription> {
    if (!this.#websocket) {
      throw new Error("WebSocket is not available. Pass a WebSocket implementation to Lumen.");
    }

    const socket = await this.#websocket(this.#websocketApiUrl(`/renders/${renderId}/socket`), {
      headers: this.#headers(),
    });

    socket.addEventListener("open", () => {
      socket.send(
        JSON.stringify({
          type: "auth",
          apiKey: this.#apiKey,
        }),
      );
    });

    socket.addEventListener("message", (message) => {
      try {
        const event = JSON.parse(String(message.data)) as RenderEvent;
        handlers.onEvent?.(event);
      } catch (error) {
        handlers.onError?.(error);
      }
    });

    socket.addEventListener("error", (event) => {
      handlers.onError?.(webSocketError(event));
    });

    socket.addEventListener("close", (event) => {
      handlers.onClose?.(event);
    });

    return {
      close: () => socket.close(),
    };
  }

  async waitForRender(
    renderId: string,
    options: WaitForRenderOptions = {},
  ): Promise<Extract<RenderEvent, { type: "render.completed" }>> {
    if (options.signal?.aborted) {
      throw abortError();
    }

    return new Promise((resolve, reject) => {
      let settled = false;
      let subscription: RenderEventSubscription | undefined;

      const cleanup = () => {
        options.signal?.removeEventListener("abort", onAbort);
      };
      const fail = (error: unknown) => {
        if (settled) {
          return;
        }
        settled = true;
        cleanup();
        subscription?.close();
        reject(error);
      };
      const onAbort = () => fail(abortError());
      options.signal?.addEventListener("abort", onAbort, { once: true });

      void this.subscribeToRender(renderId, {
        onEvent: (event) => {
          options.onEvent?.(event);
          if (event.type === "render.completed") {
            if (settled) {
              return;
            }
            settled = true;
            cleanup();
            subscription?.close();
            resolve(event);
          }
          if (event.type === "render.failed") {
            fail(event.error);
          }
        },
        onError: fail,
        onClose: (event) => {
          if (!settled) {
            const reason = event.reason ? ` ${event.reason}` : "";
            fail(new Error(`Render subscription closed before completion: ${event.code}${reason}`));
          }
        },
      })
        .then((nextSubscription) => {
          subscription = nextSubscription;
          if (settled) {
            subscription.close();
          }
        })
        .catch(fail);
    });
  }

  #headers(headers: Record<string, string> = {}): Headers {
    const nextHeaders = new Headers(headers);
    nextHeaders.set("authorization", `Bearer ${this.#apiKey}`);
    return nextHeaders;
  }

  #url(path: string): URL {
    return new URL(path, this.#baseUrl);
  }

  #apiUrl(path: string): URL {
    const basePath = this.#baseUrl.pathname.endsWith("/")
      ? this.#baseUrl.pathname
      : `${this.#baseUrl.pathname}/`;
    const url = new URL(this.#baseUrl);
    url.pathname = `${basePath}${path.replace(/^\/+/, "")}`;
    return url;
  }

  #websocketApiUrl(path: string): URL {
    const url = this.#apiUrl(path);
    url.protocol = url.protocol === "http:" ? "ws:" : "wss:";
    return url;
  }

  #jsonRequest(
    method: string,
    body: unknown,
    options: { idempotencyKey?: string | undefined; signal?: AbortSignal | undefined } = {},
  ): RequestInit {
    return {
      body: JSON.stringify(body),
      headers: this.#headers({
        "content-type": "application/json",
        ...(options.idempotencyKey ? { "idempotency-key": options.idempotencyKey } : {}),
      }),
      method,
      ...(options.signal !== undefined ? { signal: options.signal } : {}),
    };
  }

  async #expectJson<T>(response: Response): Promise<T> {
    const body = (await response.json().catch(() => undefined)) as
      | (T & { error?: unknown })
      | undefined;

    if (!response.ok) {
      throw normalizeApiError(body, response);
    }

    return body as T;
  }

  async #uploadMultipartBody(options: {
    readonly body: UploadMediaOptions["body"];
    readonly id: string;
    readonly partSizeBytes: number;
    readonly signal?: AbortSignal;
    readonly sizeBytes: number;
    readonly uploadId: string;
  }): Promise<MediaUploadPart[]> {
    const parts: MediaUploadPart[] = [];
    let partNumber = 1;

    for await (const part of multipartChunks(options.body, options.partSizeBytes)) {
      parts.push(
        await this.uploadMediaMultipartPart({
          body: part,
          id: options.id,
          partNumber,
          ...optionalSignal(options.signal),
          uploadId: options.uploadId,
        }),
      );
      partNumber += 1;
    }

    if (parts.length === 0 && options.sizeBytes > 0) {
      throw new Error("Multipart upload body did not yield any parts.");
    }

    return parts;
  }
}

export function mediaReference(media: TemporaryMedia | string): `lumen:${string}` {
  const id = typeof media === "string" ? media : media.id;
  return id.startsWith("lumen:") ? (id as `lumen:${string}`) : `lumen:${id}`;
}

function renderResult(body: Partial<RenderResult> | undefined): RenderResult {
  const render = body?.render;
  const renderId = render?.id ?? body?.id;

  return {
    ...(body?.cached !== undefined ? { cached: body.cached } : {}),
    ...(renderId !== undefined ? { id: renderId } : {}),
    ...(render !== undefined ? { render } : {}),
    ...(body?.error !== undefined ? { error: body.error } : {}),
  };
}

function webSocketError(event: unknown): Error {
  if (
    typeof event === "object" &&
    event !== null &&
    "error" in event &&
    event.error instanceof Error
  ) {
    return event.error;
  }

  if (
    typeof event === "object" &&
    event !== null &&
    "message" in event &&
    typeof event.message === "string" &&
    event.message.length > 0
  ) {
    return new Error(event.message);
  }

  return new Error("Lumen render WebSocket connection failed.");
}

function abortError(): Error {
  return new DOMException("The operation was aborted.", "AbortError");
}

function bodyAsRequestBody(body: UploadMediaOptions["body"]): BodyInit {
  if (body instanceof Blob || body instanceof ReadableStream) {
    return body;
  }

  return bytesToBody(toUint8Array(body));
}

async function* multipartChunks(
  body: UploadMediaOptions["body"],
  partSizeBytes: number,
): AsyncGenerator<BodyInit> {
  if (!Number.isInteger(partSizeBytes) || partSizeBytes <= 0) {
    throw new Error("partSizeBytes must be a positive integer.");
  }

  if (body instanceof Blob) {
    for (let offset = 0; offset < body.size; offset += partSizeBytes) {
      yield body.slice(offset, Math.min(offset + partSizeBytes, body.size));
    }
    return;
  }

  if (body instanceof ReadableStream) {
    for await (const chunk of streamMultipartChunks(body, partSizeBytes)) {
      yield bytesToBody(chunk);
    }
    return;
  }

  const bytes = toUint8Array(body);
  for (let offset = 0; offset < bytes.byteLength; offset += partSizeBytes) {
    yield bytesToBody(bytes.slice(offset, Math.min(offset + partSizeBytes, bytes.byteLength)));
  }
}

async function* streamMultipartChunks(
  stream: ReadableStream<Uint8Array>,
  partSizeBytes: number,
): AsyncGenerator<Uint8Array> {
  const reader = stream.getReader();
  let buffer = new Uint8Array(0);

  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) {
        break;
      }

      buffer = concatBytes(buffer, value);
      while (buffer.byteLength >= partSizeBytes) {
        yield buffer.slice(0, partSizeBytes);
        buffer = buffer.slice(partSizeBytes);
      }
    }

    if (buffer.byteLength > 0) {
      yield buffer;
    }
  } finally {
    reader.releaseLock();
  }
}

function concatBytes(left: Uint8Array, right: Uint8Array): Uint8Array<ArrayBuffer> {
  const result = new Uint8Array(left.byteLength + right.byteLength);
  result.set(left, 0);
  result.set(right, left.byteLength);
  return result;
}

function toUint8Array(body: BufferSource): Uint8Array {
  if (ArrayBuffer.isView(body)) {
    return new Uint8Array(body.buffer, body.byteOffset, body.byteLength);
  }

  return new Uint8Array(body);
}

function bytesToBody(bytes: Uint8Array): BodyInit {
  const copy = new Uint8Array(bytes.byteLength);
  copy.set(bytes);
  return new Blob([copy]);
}

function optionalSignal(signal: AbortSignal | undefined): { signal?: AbortSignal } {
  return signal === undefined ? {} : { signal };
}
