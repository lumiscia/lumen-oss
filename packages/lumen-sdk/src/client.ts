import type {
  CheckReport,
  CompositionSource,
  LumenComposition,
  Media,
} from "@lumiscia/lumen-types";

import { LumenError, errorFromResponse, isRecord, readBody, readDiagnostics } from "./errors.js";
import { parseMedia, parsePart, parseUploadId, parseUploadTarget } from "./media.js";
import { isTerminal, parseRender, renderError, resolveUrl } from "./render.js";
import { watchRender, type SocketUpdate } from "./socket.js";
import type {
  AssetSource,
  CreateUrlMediaOptions,
  LumenOptions,
  Render,
  RenderMediaManifest,
  RenderOptions,
  RenderStatus,
  SucceededRender,
  UploadMediaOptions,
  WaitForRenderOptions,
} from "./types.js";
import { contentTypeOf, partsOf, singleRequestLimitBytes, sizeOf } from "./upload.js";

const defaultBaseUrl = "https://api.lumiscia.com/v1";

/** The longest one socket is held open before the render is read again. */
const socketSessionMs = 60_000;
/** Sockets are opened at least this far apart, and the gap doubles up to the maximum while they end early. */
const minSocketGapMs = 5_000;
const maxSocketGapMs = 30_000;
/** A wait gives up when reads have failed with server or network errors for this long. */
const giveUpAfterMs = 10 * 60_000;
const maxBackoffMs = 30_000;
const maxRateLimitBackoffMs = 60_000;
/** Spreads waits that started together: a delay grows by up to this fraction. */
const jitter = 0.25;

function statusOrder(status: RenderStatus): number {
  return isTerminal(status) ? 2 : status === "processing" ? 1 : 0;
}

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
  /** Waits on this client send no request before this time, after the API rate limited one. */
  #cooldownUntil = 0;
  #rateLimitStreak = 0;

  constructor(options: LumenOptions) {
    this.#apiKey = options.apiKey;
    this.#baseUrl = new URL(options.baseUrl ?? defaultBaseUrl);
    const fetchImpl = options.fetch;
    this.#fetch = (...args) => (fetchImpl ?? globalThis.fetch)(...args);
  }

  /** Submits a render and returns it queued. Pass it to `waitForRender`, then to `download`. */
  async render(composition: CompositionSource, options: RenderOptions = {}): Promise<Render> {
    const body = await this.#request("POST", "/renders", {
      body: {
        composition: compositionJson(composition),
        media: options.media ?? {},
        ...(options.webhookUrl !== undefined ? { webhookUrl: options.webhookUrl } : {}),
        ...(options.quality !== undefined ? { quality: options.quality } : {}),
        ...(options.codec !== undefined ? { codec: options.codec } : {}),
        ...(options.bitDepth !== undefined ? { bitDepth: options.bitDepth } : {}),
        ...(options.alpha !== undefined ? { alpha: options.alpha } : {}),
        ...(options.output !== undefined ? { output: options.output } : {}),
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

  /** Checks a composition without rendering it: what `render` would refuse, and warnings. */
  async check(
    composition: CompositionSource,
    options: { media?: RenderMediaManifest; signal?: AbortSignal } = {},
  ): Promise<CheckReport> {
    const body = await this.#request("POST", "/renders/check", {
      body: { composition: compositionJson(composition), media: options.media ?? {} },
      signal: options.signal,
    });
    if (typeof body.ok !== "boolean" || !Array.isArray(body.diagnostics)) {
      throw new LumenError("invalid_response", "Lumen API returned a malformed check report.");
    }
    return { ok: body.ok, diagnostics: readDiagnostics(body.diagnostics) };
  }

  async getRender(id: string, options: { signal?: AbortSignal } = {}): Promise<Render> {
    const body = await this.#request("GET", `/renders/${encodeURIComponent(id)}`, options);
    return parseRender(body.render, this.#baseUrl);
  }

  /**
   * Asks lumen-server to stop a render and returns it. A queued render comes back `cancelled`; a
   * running one comes back still `processing` and ends as `cancelled` at its next frame, so follow
   * with `waitForRender`, which then rejects with the `render_cancelled` code (not retryable). A
   * render that already ended is returned as it is, and one that finishes while the request is in
   * flight may still end `succeeded`. Only lumen-server has this route.
   */
  async cancelRender(id: string, options: { signal?: AbortSignal } = {}): Promise<Render> {
    const body = await this.#request("POST", `/renders/${encodeURIComponent(id)}/cancel`, options);
    return parseRender(body.render, this.#baseUrl);
  }

  /**
   * Waits for a render to finish and returns it, or rejects with a `LumenError` if it failed or
   * was cancelled.
   *
   * The render is read from `GET /renders/:id/progress`, which is the source of truth. Between
   * reads a socket is held open for up to a minute to hear about progress, but is never relied on:
   * if sockets fail, the wait reads the render every 5 to 30 seconds instead, and the 5 second
   * minimum between sockets holds even when a socket reports an end the next read doesn't show.
   * `onProgress` never goes backwards.
   *
   * A rate limit never ends a wait: it makes every wait on this client hold off, for the API's
   * `Retry-After` or else a growing delay. Server errors and network failures are retried with
   * backoff and end the wait only after ten minutes without a successful read. Any other
   * `LumenError`, such as a 401 or 404, ends it at once, and an aborted `signal` rejects with its
   * reason from wherever the wait is. Someone waiting on many renders at once should use
   * `webhookUrl` instead.
   */
  async waitForRender(
    render: string | Pick<Render, "id">,
    options: WaitForRenderOptions = {},
  ): Promise<SucceededRender> {
    const id = typeof render === "string" ? render : render.id;
    const { signal } = options;
    let status: RenderStatus = "queued";
    let progress = 0;
    let lastReported = "";
    const report = (update: SocketUpdate) => {
      if (statusOrder(update.status) >= statusOrder(status)) {
        status = update.status;
      }
      progress = Math.max(progress, update.progress ?? 0);
      const key = `${status} ${progress}`;
      if (key !== lastReported) {
        lastReported = key;
        options.onProgress?.({ id, status, progress });
      }
    };

    const reads = { lastSucceededAt: Date.now() };
    let gapMs = minSocketGapMs;
    let nextReadAt = 0;
    let nextSessionAt = 0;
    for (;;) {
      await sleep(nextReadAt - Date.now(), signal);
      const current = await this.#readProgress(id, signal, reads);
      report({
        id,
        status: current.status,
        ...(current.progress !== undefined ? { progress: current.progress } : {}),
      });
      if (current.status === "succeeded") {
        return current;
      }
      if (current.status === "failed" || current.status === "cancelled") {
        throw renderError(current);
      }

      await sleep(nextSessionAt - Date.now(), signal);
      await this.#cooldown(signal);
      const startedAt = Date.now();
      const session = await watchRender({
        apiKey: this.#apiKey,
        fetch: this.#fetch,
        onProgress: report,
        signal,
        timeoutMs: socketSessionMs,
        url: this.#apiUrl(`/renders/${encodeURIComponent(id)}/socket`),
      });
      if (session.rejection?.status === 429) {
        this.#rateLimited(session.rejection);
      }
      gapMs = session.end === "dropped" ? Math.min(gapMs * 2, maxSocketGapMs) : minSocketGapMs;
      nextSessionAt = startedAt + jittered(gapMs);
      nextReadAt = session.end === "ended" ? 0 : nextSessionAt;
    }
  }

  /**
   * Fetches a succeeded render's output: the response's `Content-Type` and `Content-Disposition`
   * filename say what it is (`outputFile` gives the same for the `output` a render was requested
   * with). The API key is only sent when the output is on the API's
   * own origin. Rejects with a `LumenError` when the render has no output.
   */
  async download(
    render: string | { id: string; url?: string },
    options: { signal?: AbortSignal } = {},
  ): Promise<Response> {
    const id = typeof render === "string" ? render : render.id;
    let url = typeof render === "string" ? undefined : render.url;
    if (url === undefined) {
      const current = await this.getRender(id, options);
      if (current.status === "failed" || current.status === "cancelled") {
        throw renderError(current);
      }
      if (current.status !== "succeeded") {
        throw new LumenError("render_not_ready", `Render ${id} has no output yet.`, {
          renderId: id,
        });
      }
      url = current.url;
    }
    return this.#send(new URL(resolveUrl(url, this.#baseUrl)), { method: "GET", ...options });
  }

  /**
   * Uploads a file for renders to use. A body over 64 MiB, and any stream, goes up in 8 MiB parts;
   * anything smaller in one request. Write `assetSource(upload)` in a node's source to use it.
   */
  async uploadMedia(options: UploadMediaOptions): Promise<Media> {
    const { body, signal } = options;
    const sizeBytes = body instanceof ReadableStream ? options.sizeBytes : sizeOf(body);
    if (sizeBytes === undefined) {
      throw new LumenError("invalid_upload", "Uploading a stream requires sizeBytes.");
    }

    const fileName =
      options.fileName ??
      (body instanceof Blob && "name" in body && typeof body.name === "string"
        ? body.name
        : undefined);
    const contentType =
      options.contentType ??
      (body instanceof Blob && body.type !== "" ? body.type : undefined) ??
      (fileName !== undefined ? contentTypeOf(fileName) : undefined) ??
      "application/octet-stream";

    const created = await this.#request("POST", "/media", {
      body: { contentType, fileName: fileName ?? "upload", sizeBytes },
      signal,
    });
    const upload = parseUploadTarget(created);

    if (body instanceof ReadableStream || sizeBytes > singleRequestLimitBytes) {
      return this.#uploadParts(upload.media.id, body, sizeBytes, signal);
    }

    const response = await this.#send(new URL(upload.uploadUrl, this.#baseUrl), {
      body: body as BodyInit,
      headers: { "content-type": contentType },
      method: "PUT",
      signal,
    });
    return parseMedia((await readJson(response, signal)).media);
  }

  /** Has the API fetch a file from a public URL, for renders to use. */
  async createUrlMedia(options: CreateUrlMediaOptions): Promise<Media> {
    const body = await this.#request("POST", "/media/url", {
      body: {
        ...(options.fileName !== undefined ? { fileName: options.fileName } : {}),
        url: String(options.url),
      },
      signal: options.signal,
    });
    return parseMedia(body.media);
  }

  /** Reads uploaded media by id (with or without the `asset:` prefix). */
  async getMedia(
    media: string | Pick<Media, "id">,
    options: { signal?: AbortSignal } = {},
  ): Promise<Media> {
    const body = await this.#request(
      "GET",
      `/media/${encodeURIComponent(mediaId(media))}`,
      options,
    );
    return parseMedia(body.media);
  }

  /**
   * Deletes uploaded media. A render that is using it keeps reading it until that render ends,
   * and lumen-server removes the files then; a render queued afterwards finds none. Rejects with
   * `media_not_found` when the media doesn't exist or has expired.
   */
  async deleteMedia(
    media: string | Pick<Media, "id">,
    options: { signal?: AbortSignal } = {},
  ): Promise<void> {
    await this.#send(this.#apiUrl(`/media/${encodeURIComponent(mediaId(media))}`), {
      method: "DELETE",
      signal: options.signal,
    });
  }

  async #uploadParts(
    id: string,
    body: UploadMediaOptions["body"],
    sizeBytes: number,
    signal: AbortSignal | undefined,
  ): Promise<Media> {
    const base = `/media/${encodeURIComponent(id)}/multipart`;
    const uploadId = parseUploadId(await this.#request("POST", base, { signal }));
    const uploadPath = `${base}/${encodeURIComponent(uploadId)}`;

    const parts: { etag: string; partNumber: number }[] = [];
    for await (const part of partsOf(body, sizeBytes, signal)) {
      const response = await this.#send(this.#apiUrl(`${uploadPath}/parts/${parts.length + 1}`), {
        body: part,
        method: "PUT",
        signal,
      });
      parts.push(parsePart(await readJson(response, signal)));
    }

    const done = await this.#request("POST", `${uploadPath}/complete`, {
      body: { parts },
      signal,
    });
    return parseMedia(done.media);
  }

  /**
   * Reads a render and its progress. Rate limits wait out the client's cooldown and server and
   * network errors back off, until a read has not succeeded for ten minutes.
   */
  async #readProgress(
    id: string,
    signal: AbortSignal | undefined,
    reads: { lastSucceededAt: number },
  ): Promise<Render> {
    for (let failures = 0; ;) {
      await this.#cooldown(signal);
      try {
        const body = await this.#request("GET", `/renders/${encodeURIComponent(id)}/progress`, {
          signal,
        });
        const render = parseRender(body.render, this.#baseUrl);
        this.#rateLimitStreak = 0;
        reads.lastSucceededAt = Date.now();
        return render;
      } catch (error) {
        if (!(error instanceof LumenError)) {
          throw error;
        }
        if (error.status === 429) {
          this.#rateLimited(error);
          reads.lastSucceededAt = Date.now();
        } else if (isTransient(error) && Date.now() - reads.lastSucceededAt < giveUpAfterMs) {
          failures += 1;
          const backoffMs = error.retryAfterMs ?? Math.min(1000 * 2 ** failures, maxBackoffMs);
          await sleep(jittered(backoffMs), signal);
        } else {
          throw error;
        }
      }
    }
  }

  /** Waits out the cooldown a rate limit started. */
  async #cooldown(signal: AbortSignal | undefined): Promise<void> {
    // Another wait can extend the cooldown during the sleep.
    for (;;) {
      const remainingMs = this.#cooldownUntil - Date.now();
      if (remainingMs <= 0) {
        return;
      }
      await sleep(jittered(remainingMs), signal);
    }
  }

  /** Makes every wait on this client hold off, until the latest deadline any rate limit gave. */
  #rateLimited(error: LumenError): void {
    if (Date.now() >= this.#cooldownUntil) {
      this.#rateLimitStreak += 1;
    }
    const delayMs =
      error.retryAfterMs ?? Math.min(1000 * 2 ** this.#rateLimitStreak, maxRateLimitBackoffMs);
    this.#cooldownUntil = Math.max(this.#cooldownUntil, Date.now() + jittered(delayMs));
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
      throw await errorFromResponse(response, init.signal);
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
    return readJson<T>(response, options.signal);
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

/** The value to write in a node's source field for an uploaded file: `asset:<id>`. */
export function assetSource(media: string | Pick<Media, "id">): AssetSource {
  return `asset:${mediaId(media)}`;
}

/** The id of media given as an id, with or without its `asset:` prefix, or as the media itself. */
function mediaId(media: string | Pick<Media, "id">): string {
  const id = typeof media === "string" ? media : media.id;
  return id.startsWith("asset:") ? id.slice("asset:".length) : id;
}

function compositionJson(source: CompositionSource): LumenComposition {
  return hasToJSON(source) ? source.toJSON() : source;
}

function hasToJSON(source: CompositionSource): source is { toJSON(): LumenComposition } {
  return "toJSON" in source && typeof source.toJSON === "function";
}

async function readJson<T extends object = Record<string, unknown>>(
  response: Response,
  signal: AbortSignal | undefined,
): Promise<T> {
  const body = await readBody(response, signal);
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

/** `ms`, lengthened by up to 25% so that waits started together drift apart. */
function jittered(ms: number): number {
  return ms * (1 + Math.random() * jitter);
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
