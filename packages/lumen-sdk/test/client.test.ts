import { describe, expect, test } from "vitest";

import { Lumen } from "../src/client.js";
import { LumenError } from "../src/errors.js";
import { normalizeRenderEvent } from "../src/events.js";
import type { LumenWebSocket } from "../src/types.js";

describe("Lumen.waitForRender", () => {
  test("rejects when the server closes gracefully before a terminal event", async () => {
    const harness = webSocketHarness();
    const lumen = new Lumen({
      apiKey: "test-key",
      baseUrl: "http://localhost:8787",
      websocket: () => harness.socket,
    });

    const waiting = lumen.waitForRender("render-1");
    await Promise.resolve();
    await Promise.resolve();
    harness.closeFromServer(1000, "normal closure");

    await expect(waiting).rejects.toThrow(
      "Render subscription closed before completion: 1000 normal closure",
    );
    expect(harness.clientCloseCount()).toBe(1);
  });

  test("rejects with a LumenError when the hosted API reports a failure", async () => {
    const harness = webSocketHarness();
    const lumen = new Lumen({
      apiKey: "test-key",
      baseUrl: "http://localhost:8787",
      websocket: () => harness.socket,
    });

    const waiting = lumen.waitForRender("render-1");
    await Promise.resolve();
    await Promise.resolve();
    harness.messageFromServer({
      type: "render.failed",
      state: { renderId: "render-1", state: "failed", progress: 1, error: "Out of GPU memory" },
    });

    const error = await waiting.catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(LumenError);
    expect(error).toMatchObject({
      code: "render_failed",
      message: "Out of GPU memory",
      renderId: "render-1",
    });
    expect(harness.clientCloseCount()).toBe(1);
  });

  test("resolves with the render read after the completed event", async () => {
    const harness = webSocketHarness();
    const { stub } = recordingFetch(() => ({
      render: { id: "render-1", status: "succeeded", outputUrl: "/renders/render-1/artifact" },
    }));
    const lumen = new Lumen({
      apiKey: "test-key",
      baseUrl: "http://localhost:8787",
      fetch: stub,
      websocket: () => harness.socket,
    });

    const waiting = lumen.waitForRender("render-1");
    await Promise.resolve();
    await Promise.resolve();
    harness.messageFromServer({ type: "render.completed", renderId: "render-1", url: "/x" });

    await expect(waiting).resolves.toMatchObject({
      id: "render-1",
      status: "succeeded",
      url: "http://localhost:8787/renders/render-1/artifact",
    });
  });
});

describe("normalizeRenderEvent", () => {
  test("flattens hosted API events", () => {
    const state = { organizationId: "org-1", renderId: "render-1", updatedAt: "now" };

    expect(
      normalizeRenderEvent({
        type: "render.progress",
        state: { ...state, state: "processing", progress: 0.5 },
      }),
    ).toEqual({ type: "render.progress", renderId: "render-1", progress: 0.5 });
    expect(
      normalizeRenderEvent({
        type: "render.completed",
        state: { ...state, state: "succeeded", progress: 1, artifactUrl: "https://a/b.mp4" },
      }),
    ).toEqual({ type: "render.completed", renderId: "render-1", url: "https://a/b.mp4" });
    expect(
      normalizeRenderEvent({ type: "render.failed", state: { ...state, state: "failed" } }),
    ).toEqual({
      type: "render.failed",
      renderId: "render-1",
      error: { code: "render_failed", message: "Render failed." },
    });
  });

  test("passes through lumen-server events", () => {
    expect(
      normalizeRenderEvent({ type: "render.completed", renderId: "render-1", url: "https://a" }),
    ).toEqual({ type: "render.completed", renderId: "render-1", url: "https://a" });
    expect(
      normalizeRenderEvent({
        type: "render.failed",
        renderId: "render-1",
        error: { code: "render_failed", message: "Decoder crashed" },
      }),
    ).toEqual({
      type: "render.failed",
      renderId: "render-1",
      error: { code: "render_failed", message: "Decoder crashed" },
    });
  });

  test("ignores messages that are not render events", () => {
    expect(normalizeRenderEvent({ type: "pong" })).toBeUndefined();
  });
});

function webSocketHarness(): {
  socket: LumenWebSocket;
  closeFromServer: (code: number, reason: string) => void;
  messageFromServer: (data: unknown) => void;
  clientCloseCount: () => number;
} {
  const listeners = new Map<string, Array<(event: unknown) => void>>();
  let closeCount = 0;
  const socket = {
    addEventListener(type: string, listener: (event: unknown) => void) {
      const registered = listeners.get(type) ?? [];
      registered.push(listener);
      listeners.set(type, registered);
    },
    close() {
      closeCount += 1;
    },
    send() {},
  } as LumenWebSocket;

  return {
    socket,
    closeFromServer(code, reason) {
      for (const listener of listeners.get("close") ?? []) {
        listener({ code, reason } satisfies Partial<CloseEvent>);
      }
    },
    messageFromServer(data) {
      for (const listener of listeners.get("message") ?? []) {
        listener({ data: JSON.stringify(data) } satisfies Partial<MessageEvent>);
      }
    },
    clientCloseCount: () => closeCount,
  };
}

interface RecordedRequest {
  readonly init: RequestInit & { duplex?: string };
  readonly url: string;
}

function recordingFetch(respond: (request: RecordedRequest) => unknown) {
  const requests: RecordedRequest[] = [];
  const stub = async (input: RequestInfo | URL, init: RequestInit = {}) => {
    const request = { init, url: String(input) };
    requests.push(request);
    return Response.json(respond(request));
  };
  return { requests, stub: stub as typeof fetch };
}

describe("Lumen fetch", () => {
  test("calls the configured fetch without the client as receiver", async () => {
    const stub = function (this: unknown) {
      if (this !== undefined) {
        throw new TypeError("Illegal invocation");
      }
      return Promise.resolve(Response.json({ render: { id: "render-1", status: "queued" } }));
    } as unknown as typeof fetch;
    const unbound = new Lumen({ apiKey: "test-key", fetch: stub });

    await expect(unbound.getRender("render-1")).resolves.toMatchObject({ id: "render-1" });
  });
});

describe("Lumen errors", () => {
  const lumenWith = (respond: () => Response | Promise<Response>) =>
    new Lumen({
      apiKey: "test-key",
      baseUrl: "https://api.test/v1",
      fetch: (async () => respond()) as typeof fetch,
    });

  test("an error body becomes a LumenError with its code, status and request id", async () => {
    const lumen = lumenWith(() =>
      Response.json(
        { code: "invalid_composition", error: "no output node", requestId: "req-1" },
        { status: 422 },
      ),
    );

    const error = await lumen.render({} as never).catch((caught: unknown) => caught);

    expect(error).toBeInstanceOf(LumenError);
    expect(error).toMatchObject({
      code: "invalid_composition",
      message: "no output node",
      requestId: "req-1",
      status: 422,
    });
  });

  test("a response without a body still names its status", async () => {
    const lumen = lumenWith(() => new Response("<html>bad gateway</html>", { status: 502 }));

    await expect(lumen.getRender("render-1")).rejects.toMatchObject({
      code: "http_502",
      status: 502,
    });
  });

  test("a failed fetch is a network_error, but an abort stays an abort", async () => {
    const down = new Lumen({
      apiKey: "test-key",
      fetch: (async () => {
        throw new TypeError("fetch failed");
      }) as typeof fetch,
    });
    await expect(down.getRender("render-1")).rejects.toMatchObject({
      code: "network_error",
      message: "fetch failed",
    });

    const controller = new AbortController();
    const aborted = new Lumen({
      apiKey: "test-key",
      fetch: (async () => {
        controller.abort();
        throw new DOMException("aborted", "AbortError");
      }) as typeof fetch,
    });
    await expect(
      aborted.getRender("render-1", { signal: controller.signal }),
    ).rejects.toMatchObject({ name: "AbortError" });
  });

  test("an abort while the body is read rejects with the signal's reason", async () => {
    const controller = new AbortController();
    const reason = new Error("stop");
    const stalled = {
      ok: true,
      status: 200,
      json: () =>
        new Promise((_resolve, reject) => {
          controller.signal.addEventListener("abort", () => reject(reason), { once: true });
          queueMicrotask(() => controller.abort(reason));
        }),
    } as unknown as Response;
    const lumen = new Lumen({
      apiKey: "test-key",
      fetch: (async () => stalled) as typeof fetch,
    });

    await expect(lumen.getRender("r", { signal: controller.signal })).rejects.toBe(reason);
  });

  test("a success body that is not a render is rejected", async () => {
    const malformed = lumenWith(() => new Response("ok"));
    await expect(malformed.getRender("render-1")).rejects.toMatchObject({
      code: "invalid_response",
    });

    const wrongShape = lumenWith(() => Response.json({ render: { id: "render-1" } }));
    await expect(wrongShape.getRender("render-1")).rejects.toMatchObject({
      code: "invalid_response",
    });
  });
});

describe("Lumen render and download", () => {
  test("render returns the queued render and sends its idempotency key", async () => {
    const { requests, stub } = recordingFetch(() => ({
      cached: true,
      render: { id: "render-1", status: "queued" },
    }));
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    const render = await lumen.render({} as never, { idempotencyKey: "key-1" });

    expect(render).toEqual({ id: "render-1", status: "queued", cached: true });
    expect(new Headers(requests[0]?.init.headers).get("idempotency-key")).toBe("key-1");
  });

  test("download sends the key to the API origin only", async () => {
    const seen: (string | null)[] = [];
    const stub = (async (_url: unknown, init: RequestInit = {}) => {
      seen.push(new Headers(init.headers).get("authorization"));
      return new Response("bytes");
    }) as typeof fetch;
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    await lumen.download({ id: "a", url: "https://api.test/v1/renders/a/artifact" });
    await lumen.download({ id: "b", url: "https://cdn.example/b.mp4" });

    expect(seen).toEqual(["Bearer test-key", null]);
  });

  test("a relative output url is resolved under the API base, with or without a trailing slash", async () => {
    for (const baseUrl of ["https://host/lumen", "https://host/lumen/", "https://host"]) {
      const seen: string[] = [];
      const stub = (async (url: URL) => {
        seen.push(String(url));
        return String(url).endsWith("/renders/r")
          ? Response.json({
              render: { id: "r", status: "succeeded", outputUrl: "/renders/r/artifact" },
            })
          : new Response("bytes");
      }) as unknown as typeof fetch;
      const lumen = new Lumen({ apiKey: "test-key", baseUrl, fetch: stub });

      const render = await lumen.getRender("r");
      await lumen.download(render);

      const prefix = baseUrl.replace(/\/$/, "");
      expect(render.url).toBe(`${prefix}/renders/r/artifact`);
      expect(seen).toEqual([`${prefix}/renders/r`, `${prefix}/renders/r/artifact`]);
    }
  });

  test("an absolute output url is left alone", async () => {
    const { stub } = recordingFetch(() => ({
      render: { id: "r", status: "succeeded", outputUrl: "https://cdn.example/r.mp4" },
    }));
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    await expect(lumen.getRender("r")).resolves.toMatchObject({ url: "https://cdn.example/r.mp4" });
  });

  test("download looks the render up by id, and fails when it has no output", async () => {
    const { requests, stub } = recordingFetch(() => ({
      render: { id: "r", status: "processing" },
    }));
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    await expect(lumen.download("r")).rejects.toMatchObject({
      code: "render_not_ready",
      renderId: "r",
    });
    expect(requests.map((request) => request.url)).toEqual(["https://api.test/v1/renders/r"]);
  });
});

describe("Lumen media uploads", () => {
  const media = { id: "media 1" };

  test("multipart parts and completion carry the upload id and real part numbers", async () => {
    const completed: unknown[] = [];
    const { requests, stub } = recordingFetch(({ url, init }) => {
      if (url.endsWith("/media")) {
        return { media, uploadUrl: "/v1/media/media%201/content" };
      }
      if (url.endsWith("/multipart")) {
        return { uploadId: "up/1" };
      }
      if (url.endsWith("/complete")) {
        completed.push(...JSON.parse(String(init.body)).parts);
        return { media };
      }
      const partNumber = Number(url.split("/").at(-1));
      return { etag: `etag-${partNumber}`, partNumber };
    });
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    await lumen.uploadMedia({
      body: new Uint8Array(5),
      contentType: "video/mp4",
      fileName: "a.mp4",
      multipart: { partSizeBytes: 2 },
      sizeBytes: 5,
    });

    expect(requests.map((request) => request.url).slice(1)).toEqual([
      "https://api.test/v1/media/media%201/multipart",
      "https://api.test/v1/media/media%201/multipart/up%2F1/parts/1",
      "https://api.test/v1/media/media%201/multipart/up%2F1/parts/2",
      "https://api.test/v1/media/media%201/multipart/up%2F1/parts/3",
      "https://api.test/v1/media/media%201/multipart/up%2F1/complete",
    ]);
    expect(completed).toEqual([
      { etag: "etag-1", partNumber: 1 },
      { etag: "etag-2", partNumber: 2 },
      { etag: "etag-3", partNumber: 3 },
    ]);
  });

  test("a media response without a media is invalid_response, not undefined", async () => {
    const lumen = new Lumen({
      apiKey: "test-key",
      baseUrl: "https://api.test/v1",
      fetch: (async () => Response.json({})) as typeof fetch,
    });

    await expect(lumen.createUrlMedia({ url: "https://example.test/a.mp4" })).rejects.toMatchObject(
      {
        code: "invalid_response",
      },
    );
    await expect(
      lumen.uploadMedia({
        body: new Uint8Array(3),
        contentType: "video/mp4",
        fileName: "a.mp4",
        sizeBytes: 3,
      }),
    ).rejects.toMatchObject({ code: "invalid_response" });
  });

  test("stream bodies are sent half-duplex", async () => {
    const { requests, stub } = recordingFetch(({ url }) =>
      url.endsWith("/media") ? { media, uploadUrl: "/v1/media/media1/content" } : { media },
    );
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    await lumen.uploadMedia({
      body: new Blob(["abc"]).stream(),
      contentType: "video/mp4",
      fileName: "a.mp4",
      sizeBytes: 3,
    });

    expect(requests[1]?.init.duplex).toBe("half");
  });

  test("the API key is only sent to the API origin", async () => {
    const uploadUrls = ["/v1/media/media1/content", "https://storage.example/put"];
    const authorization: (string | null)[] = [];

    for (const uploadUrl of uploadUrls) {
      const { requests, stub } = recordingFetch(({ url }) =>
        url.endsWith("/media") ? { media, uploadUrl } : { media },
      );
      const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });
      await lumen.uploadMedia({
        body: new Uint8Array(3),
        contentType: "video/mp4",
        fileName: "a.mp4",
        sizeBytes: 3,
      });
      authorization.push(new Headers(requests[1]?.init.headers).get("authorization"));
    }

    expect(authorization).toEqual(["Bearer test-key", null]);
  });
});
