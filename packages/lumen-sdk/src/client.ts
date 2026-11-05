import { Composition, type LumenComposition } from "@lumiscia/lumen-shared";

import { LumenError, errorFromResponse, isRecord } from "./errors.js";
import { isTerminal, parseProgress, parseRender, renderError } from "./render.js";
import { watchRender, type SocketUpdate } from "./socket.js";
import type {
  CreateUrlMediaOptions,
  LumenMediaReference,
  LumenOptions,
  Render,
  RenderMediaManifest,
  RenderOptions,
  RenderProgress,
  TemporaryMedia,
  UploadMediaOptions,
  WaitForRenderOptions,
} from "./types.js";
import { partsOf, singleRequestLimitBytes, sizeOf } from "./upload.js";

const defaultBaseUrl = "https://api.lumiscia.com/v1";

/** The longest one socket is held open before the render is read again. */
const socketSessionMs = 60_000;
/** Sockets are opened at least this far apart, and the gap doubles up to the maximum while they end early. */
const minSocketGapMs = 5_000;
const maxSocketGapMs = 30_000;
/** Consecutive transient failures reading a render before the wait gives up. */
const maxReadFailures = 8;
const maxReadBackoffMs = 30_000;

type SendInit = Omit<RequestInit, "signal"> & {
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

  constructor(options: LumenOptions) {
    this.#apiKey = options.apiKey;
    this.#baseUrl = new URL(options.baseUrl ?? defaultBaseUrl);
    const fetchImpl = options.fetch;
    this.#fetch = (...args) => (fetchImpl ?? globalThis.fetch)(...args);
  }

  /** Submits a render and returns it queued. Pass it to `waitForRender`, then to `download`. */
  async render(
    composition: Composition | LumenComposition,
    options: RenderOptions = {},
  ): Promise<Render> {
    const body = await this.#request("POST", "/renders", {
      body: {
        composition: composition instanceof Composition ? composition.toJSON() : composition,
        media: mediaManifest(options.media ?? {}),
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
   * Waits for a render to finish and returns it, or rejects with a `LumenError` if it failed.
   *
   * The render is read from `GET /renders/:id/progress`, which is the source of truth. Between
   * reads a socket is held open for up to a minute to hear about progress, but is never relied on:
   * if sockets fail, the wait reads the render every 5 to 30 seconds instead. Rate limits, server
   * errors and network failures are retried with backoff; a `LumenError` with any other status
   * ends the wait. An aborted `signal` rejects with its reason. Someone waiting on many renders at
   * once should use `webhookUrl` instead.
   */
  async waitForRender(
    render: string | Pick<Render, "id">,
    options: WaitForRenderOptions = {},
  ): Promise<Render> {
    const id = typeof render === "string" ? render : render.id;
    const { signal } = options;
    let lastReported = "";
    let lastProgress = 0;
    const report = (update: SocketUpdate) => {
      const progress: RenderProgress = {
        id: update.id,
        status: update.status,
        progress: update.progress ?? lastProgress,
      };
      lastProgress = progress.progress;
      const key = `${progress.status} ${progress.progress}`;
      if (key !== lastReported) {
        lastReported = key;
        options.onProgress?.(progress);
      }
    };

    let gapMs = minSocketGapMs;
    let nextSessionAt = 0;
    for (;;) {
      await sleep(nextSessionAt - Date.now(), signal);
      const current = await this.#readProgress(id, signal);
      report({ id, status: current.status, progress: current.progress ?? 0 });
      if (current.status === "succeeded") {
        return current;
      }
      if (isTerminal(current.status)) {
        throw renderError(current);
      }

      const startedAt = Date.now();
      const session = await watchRender({
        apiKey: this.#apiKey,
        fetch: this.#fetch,
        onProgress: report,
        signal,
        timeoutMs: socketSessionMs,
        url: this.#apiUrl(`/renders/${encodeURIComponent(id)}/socket`),
      });
      gapMs = session === "dropped" ? Math.min(gapMs * 2, maxSocketGapMs) : minSocketGapMs;
      nextSessionAt = session === "ended" ? 0 : startedAt + gapMs;
    }
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

  /**
   * Uploads a file for renders to use. A body over 64 MiB, and any stream, goes up in 8 MiB parts;
   * anything smaller in one request. Pass the result as a `media` value, or to `mediaReference`.
   */
  async uploadMedia(options: UploadMediaOptions): Promise<TemporaryMedia> {
    const { body, signal } = options;
    const sizeBytes = body instanceof ReadableStream ? options.sizeBytes : sizeOf(body);
    if (sizeBytes === undefined) {
      throw new LumenError("invalid_upload", "Uploading a stream requires sizeBytes.");
    }

    const contentType =
      options.contentType ?? (body instanceof Blob && body.type !== "" ? body.type : undefined);
    const fileName =
      options.fileName ??
      (body instanceof Blob && "name" in body && typeof body.name === "string"
        ? body.name
        : "upload");
    const upload = await this.#request<{ media: TemporaryMedia; uploadUrl: string }>(
      "POST",
      "/media",
      {
        body: {
          contentType: contentType ?? "application/octet-stream",
          fileName,
          sizeBytes,
        },
        signal,
      },
    );

    if (body instanceof ReadableStream || sizeBytes > singleRequestLimitBytes) {
      return this.#uploadParts(upload.media.id, body, sizeBytes, signal);
    }

    const response = await this.#send(new URL(upload.uploadUrl, this.#baseUrl), {
      body: body as BodyInit,
      headers: { "content-type": contentType ?? "application/octet-stream" },
      method: "PUT",
      signal,
    });
    return (await readJson<{ media: TemporaryMedia }>(response)).media;
  }

  /** Has the API fetch a file from a public URL, for renders to use. */
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

  async #uploadParts(
    id: string,
    body: UploadMediaOptions["body"],
    sizeBytes: number,
    signal: AbortSignal | undefined,
  ): Promise<TemporaryMedia> {
    const base = `/media/${encodeURIComponent(id)}/multipart`;
    const { uploadId } = await this.#request<{ uploadId: string }>("POST", base, { signal });
    const uploadPath = `${base}/${encodeURIComponent(uploadId)}`;

    const parts: unknown[] = [];
    for await (const part of partsOf(body, sizeBytes)) {
      const response = await this.#send(this.#apiUrl(`${uploadPath}/parts/${parts.length + 1}`), {
        body: part,
        method: "PUT",
        signal,
      });
      parts.push(await readJson(response));
    }

    const done = await this.#request<{ media: TemporaryMedia }>("POST", `${uploadPath}/complete`, {
      body: { parts },
      signal,
    });
    return done.media;
  }

  /** Reads a render and its progress, retrying transient failures. */
  async #readProgress(id: string, signal: AbortSignal | undefined): Promise<Render> {
    for (let failures = 1; ; failures += 1) {
      try {
        const body = await this.#request("GET", `/renders/${encodeURIComponent(id)}/progress`, {
          signal,
        });
        return parseProgress(body, this.#baseUrl);
      } catch (error) {
        if (!isTransient(error) || failures >= maxReadFailures) {
          throw error;
        }
        const backoffMs = error.retryAfterMs ?? Math.min(1000 * 2 ** failures, maxReadBackoffMs);
        await sleep(backoffMs * (1 + Math.random() * 0.25), signal);
      }
    }
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
}

export function mediaReference(media: TemporaryMedia | string): LumenMediaReference {
  const id = typeof media === "string" ? media : media.id;
  return id.startsWith("lumen:") ? (id as LumenMediaReference) : `lumen:${id}`;
}

function mediaManifest(media: RenderMediaManifest): Record<string, string> {
  return Object.fromEntries(
    Object.entries(media).map(([alias, value]) => [
      alias,
      typeof value === "string" ? value : mediaReference(value),
    ]),
  );
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

function isTransient(error: unknown): error is LumenError {
  return (
    error instanceof LumenError &&
    (error.code === "network_error" ||
      error.status === 429 ||
      (error.status !== undefined && error.status >= 500))
  );
}

/** Resolves after `ms`, or rejects with the reason `signal` aborts with. */
function sleep(ms: number, signal: AbortSignal | undefined): Promise<void> {
  return new Promise((resolve, reject) => {
    signal?.throwIfAborted();
    if (ms <= 0) {
      resolve();
      return;
    }
    const onAbort = () => {
      clearTimeout(timer);
      reject(signal?.reason);
    };
    const timer = setTimeout(
      () => {
        signal?.removeEventListener("abort", onAbort);
        resolve();
      },
      Math.max(0, ms),
    );
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}
