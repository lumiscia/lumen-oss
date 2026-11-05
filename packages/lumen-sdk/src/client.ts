import { Composition, type LumenComposition } from "@lumiscia/lumen-shared";

import { LumenError, errorFromResponse, isRecord } from "./errors.js";
import { normalizeRenderEvent } from "./events.js";
import { parseRender, renderError } from "./render.js";
import type {
  CompleteMultipartUploadOptions,
  CreateMediaUploadOptions,
  CreateMediaUploadResult,
  CreateMultipartUploadResult,
  CreateUrlMediaOptions,
  LumenOptions,
  LumenWebSocketFactory,
  MediaUploadPart,
  Render,
  RenderEventHandlers,
  RenderEventSubscription,
  RenderOptions,
  TemporaryMedia,
  UploadMediaOptions,
  WaitForRenderOptions,
} from "./types.js";

const defaultMultipartPartSizeBytes = 8 * 1024 * 1024;
const defaultBaseUrl = "https://api.lumiscia.com/v1";

type SendInit = Omit<RequestInit, "signal"> & {
  readonly duplex?: "half";
  readonly signal?: AbortSignal | undefined;
};

interface RequestOptions {
  readonly body?: unknown;
  readonly idempotencyKey?: string | undefined;
  readonly signal?: AbortSignal | undefined;
}

export class Lumen {
  readonly #apiKey: string;
  readonly #baseUrl: URL;
  readonly #fetch: typeof fetch;
  readonly #websocket: LumenWebSocketFactory | undefined;

  constructor(options: LumenOptions) {
    this.#apiKey = options.apiKey;
    this.#baseUrl = new URL(options.baseUrl ?? defaultBaseUrl);
    const fetchImpl = options.fetch;
    this.#fetch = (...args) => (fetchImpl ?? globalThis.fetch)(...args);
    this.#websocket = options.websocket;
  }

  /** Submits a render and returns it queued. Pass it to `waitForRender`, then to `download`. */
  async render(
    composition: Composition | LumenComposition,
    options: RenderOptions = {},
  ): Promise<Render> {
    const body = await this.#request("POST", "/renders", {
      body: {
        composition: composition instanceof Composition ? composition.toJSON() : composition,
        media: options.media ?? {},
        ...(options.webhookUrl !== undefined ? { webhookUrl: options.webhookUrl } : {}),
      },
      idempotencyKey: options.idempotencyKey,
      signal: options.signal,
    });
    return parseRender(
      body.render,
      this.#baseUrl,
      typeof body.cached === "boolean" ? { cached: body.cached } : {},
    );
  }

  async getRender(id: string, options: { signal?: AbortSignal } = {}): Promise<Render> {
    const body = await this.#request("GET", `/renders/${encodeURIComponent(id)}`, options);
    return parseRender(body.render, this.#baseUrl);
  }

  /**
   * Fetches a succeeded render's output. The API key is only sent when the output is on the API's
   * own origin. Rejects with a `LumenError` when the render has no output.
   */
  async download(
    render: string | Pick<Render, "id" | "url">,
    options: { signal?: AbortSignal } = {},
  ): Promise<Response> {
    const id = typeof render === "string" ? render : render.id;
    let url = typeof render === "string" ? undefined : render.url;
    if (url === undefined) {
      const current = await this.getRender(id, options);
      if (current.status === "failed") {
        throw renderError(current);
      }
      url = current.url;
    }
    if (url === undefined) {
      throw new LumenError("render_not_ready", `Render ${id} has no output yet.`, {
        renderId: id,
      });
    }
    return this.#send(new URL(url, this.#baseUrl), { method: "GET", ...options });
  }

  async createMediaUpload(options: CreateMediaUploadOptions): Promise<CreateMediaUploadResult> {
    return this.#request<CreateMediaUploadResult>("POST", "/media", {
      body: {
        contentType: options.contentType,
        fileName: options.fileName,
        sizeBytes: options.sizeBytes,
      },
      signal: options.signal,
    });
  }

  async uploadMedia(options: UploadMediaOptions): Promise<TemporaryMedia> {
    if (options.multipart) {
      return this.uploadMediaMultipart(options);
    }

    const upload = await this.createMediaUpload(options);
    const response = await this.#send(new URL(upload.uploadUrl, this.#baseUrl), {
      body: bodyAsRequestBody(options.body),
      ...(options.body instanceof ReadableStream ? { duplex: "half" } : {}),
      headers: {
        "content-length": options.sizeBytes.toString(),
        "content-type": options.contentType,
      },
      method: "PUT",
      signal: options.signal,
    });
    return (await readJson<{ media: TemporaryMedia }>(response)).media;
  }

  async createUrlMedia(options: CreateUrlMediaOptions): Promise<TemporaryMedia> {
    const body = await this.#request<{ media: TemporaryMedia }>("POST", "/media/url", {
      body: {
        ...(options.fileName !== undefined ? { fileName: options.fileName } : {}),
        url: String(options.url),
      },
      signal: options.signal,
    });
    return body.media;
  }

  async createMediaMultipartUpload(
    id: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<CreateMultipartUploadResult> {
    return this.#request<CreateMultipartUploadResult>(
      "POST",
      `/media/${encodeURIComponent(id)}/multipart`,
      options,
    );
  }

  async uploadMediaMultipartPart(options: {
    readonly body: BodyInit;
    readonly id: string;
    readonly partNumber: number;
    readonly signal?: AbortSignal;
    readonly uploadId: string;
  }): Promise<MediaUploadPart> {
    const path = `/media/${encodeURIComponent(options.id)}/multipart/${encodeURIComponent(
      options.uploadId,
    )}/parts/${options.partNumber}`;
    const response = await this.#send(this.#apiUrl(path), {
      body: options.body,
      method: "PUT",
      signal: options.signal,
    });
    return readJson<MediaUploadPart>(response);
  }

  async completeMediaMultipartUpload(
    options: CompleteMultipartUploadOptions,
  ): Promise<TemporaryMedia> {
    const body = await this.#request<{ media: TemporaryMedia }>(
      "POST",
      `/media/${encodeURIComponent(options.id)}/multipart/${encodeURIComponent(
        options.uploadId,
      )}/complete`,
      { body: { parts: options.parts }, signal: options.signal },
    );
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
      headers: this.#authHeaders(),
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
        const event = normalizeRenderEvent(JSON.parse(String(message.data)));
        if (event !== undefined) {
          handlers.onEvent?.(event);
        }
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

  /** Resolves with the render once it has succeeded, and rejects with a `LumenError` if it fails. */
  async waitForRender(renderId: string, options: WaitForRenderOptions = {}): Promise<Render> {
    if (options.signal?.aborted) {
      throw abortError();
    }

    await new Promise<void>((resolve, reject) => {
      let settled = false;
      let subscription: RenderEventSubscription | undefined;

      const cleanup = () => {
        options.signal?.removeEventListener("abort", onAbort);
      };
      const settle = (error?: unknown) => {
        if (settled) {
          return;
        }
        settled = true;
        cleanup();
        subscription?.close();
        if (error === undefined) {
          resolve();
        } else {
          reject(error);
        }
      };
      const onAbort = () => settle(abortError());
      options.signal?.addEventListener("abort", onAbort, { once: true });

      void this.subscribeToRender(renderId, {
        onEvent: (event) => {
          options.onEvent?.(event);
          if (event.type === "render.completed") {
            settle();
          }
          if (event.type === "render.failed") {
            settle(
              new LumenError(event.error.code, event.error.message, {
                renderId: event.renderId,
                ...(event.error.retryable !== undefined
                  ? { retryable: event.error.retryable }
                  : {}),
              }),
            );
          }
        },
        onError: settle,
        onClose: (event) => {
          if (!settled) {
            const reason = event.reason ? ` ${event.reason}` : "";
            settle(
              new LumenError(
                "socket_error",
                `Render subscription closed before completion: ${event.code}${reason}`,
              ),
            );
          }
        },
      })
        .then((nextSubscription) => {
          subscription = nextSubscription;
          if (settled) {
            subscription.close();
          }
        })
        .catch(settle);
    });

    return this.getRender(renderId, options.signal ? { signal: options.signal } : {});
  }

  /**
   * The one place a request is built and checked: adds the key for the API's own origin, and turns
   * network and HTTP failures into `LumenError`.
   */
  async #send(url: URL, init: SendInit): Promise<Response> {
    const headers = new Headers(init.headers);
    if (url.origin === this.#baseUrl.origin) {
      headers.set("authorization", `Bearer ${this.#apiKey}`);
    }

    let response: Response;
    try {
      response = await this.#fetch(url, { ...init, headers, signal: init.signal ?? null });
    } catch (cause) {
      if (init.signal?.aborted) {
        throw cause;
      }
      throw new LumenError(
        "network_error",
        cause instanceof Error ? cause.message : "Lumen API request failed.",
        { cause },
      );
    }

    if (!response.ok) {
      throw await errorFromResponse(response);
    }
    return response;
  }

  /** Calls an authenticated JSON route of the API and returns its success body. */
  async #request<T extends object = Record<string, unknown>>(
    method: string,
    path: string,
    options: RequestOptions = {},
  ): Promise<T> {
    const response = await this.#send(this.#apiUrl(path), {
      method,
      ...(options.body !== undefined ? { body: JSON.stringify(options.body) } : {}),
      headers: {
        ...(options.body !== undefined ? { "content-type": "application/json" } : {}),
        ...(options.idempotencyKey ? { "idempotency-key": options.idempotencyKey } : {}),
      },
      signal: options.signal,
    });
    return readJson<T>(response);
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

  #authHeaders(): Headers {
    return new Headers({ authorization: `Bearer ${this.#apiKey}` });
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

function webSocketError(event: unknown): LumenError {
  if (
    typeof event === "object" &&
    event !== null &&
    "error" in event &&
    event.error instanceof Error
  ) {
    return new LumenError("socket_error", event.error.message, { cause: event.error });
  }

  if (
    typeof event === "object" &&
    event !== null &&
    "message" in event &&
    typeof event.message === "string" &&
    event.message.length > 0
  ) {
    return new LumenError("socket_error", event.message);
  }

  return new LumenError("socket_error", "Lumen render WebSocket connection failed.");
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

async function readJson<T extends object>(response: Response): Promise<T> {
  const body: unknown = await response.json().catch(() => undefined);
  if (!isRecord(body)) {
    throw new LumenError("invalid_response", "Lumen API returned a malformed response body.", {
      status: response.status,
    });
  }
  return body as T;
}
