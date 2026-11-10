import { describe, expect, test, vi } from "vitest";

import { Lumen } from "../src/client.js";
import { LumenError } from "../src/errors.js";
import { outputFile } from "../src/output.js";

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

  test("an error body becomes a LumenError with its code, status, request id and diagnostics", async () => {
    const missing = {
      severity: "error",
      code: "graph.missing_media_output",
      path: "/nodes",
      message: "no output node",
    };
    const unknownCode = { ...missing, code: "graph.some_future_check", node: 3 };
    const lumen = lumenWith(() =>
      Response.json(
        {
          code: "invalid_composition",
          error: "no output node",
          requestId: "req-1",
          diagnostics: [missing, unknownCode, { code: "not.a.diagnostic" }],
        },
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
    expect((error as LumenError).diagnostics).toEqual([missing, unknownCode]);
    expect(new LumenError("network_error", "down").diagnostics).toEqual([]);
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

  test("render sends quality, codec, bit depth and alpha only when they are set", async () => {
    const { requests, stub } = recordingFetch(() => ({
      render: { id: "render-1", status: "queued" },
    }));
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    await lumen.render({} as never);
    await lumen.render({} as never, { codec: "hevc", quality: "high", bitDepth: 10 });
    await lumen.render({} as never, { alpha: true, output: { format: "webm" } });

    const bodies = requests.map((request) => JSON.parse(String(request.init.body)));
    expect(bodies[0]).not.toHaveProperty("quality");
    expect(bodies[0]).not.toHaveProperty("codec");
    expect(bodies[0]).not.toHaveProperty("bit_depth");
    expect(bodies[0]).not.toHaveProperty("alpha");
    expect(bodies[1]).toMatchObject({ codec: "hevc", quality: "high", bit_depth: 10 });
    expect(bodies[2]).toMatchObject({ alpha: true, output: { format: "webm" } });
  });

  test("render sends the output description in the server's shape, and only when it is set", async () => {
    const { requests, stub } = recordingFetch(() => ({
      render: { id: "render-1", status: "queued" },
    }));
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    await lumen.render({} as never);
    await lumen.render({} as never, {
      output: { format: "jpeg_sequence", frames: { start: 30, end: 60 }, jpegQuality: 80 },
    });
    await lumen.render({} as never, { output: { frames: { end: 24 } } });

    const bodies = requests.map((request) => JSON.parse(String(request.init.body)));
    expect(bodies[0]).not.toHaveProperty("output");
    expect(bodies[1].output).toEqual({
      format: "jpeg_sequence",
      frames: { start: 30, end: 60 },
      jpeg_quality: 80,
    });
    expect(bodies[2].output).toEqual({ frames: { end: 24 } });
  });

  test("outputFile names the content type and extension of each format", () => {
    const files = (
      ["mp4", "mov", "webm", "png", "jpeg", "png_sequence", "jpeg_sequence"] as const
    ).map((format) => Object.values(outputFile({ format })));
    expect(files).toEqual([
      ["video/mp4", "mp4"],
      ["video/quicktime", "mov"],
      ["video/webm", "webm"],
      ["image/png", "png"],
      ["image/jpeg", "jpg"],
      ["application/zip", "zip"],
      ["application/zip", "zip"],
    ]);
    expect(outputFile()).toEqual({ contentType: "video/mp4", extension: "mp4" });
  });

  test("download hands back the artifact's own content type and filename", async () => {
    const stub = (async () =>
      new Response("zip", {
        headers: {
          "content-type": "application/zip",
          "content-disposition": 'attachment; filename="r.zip"',
        },
      })) as unknown as typeof fetch;
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    const response = await lumen.download({
      id: "r",
      url: "https://api.test/v1/renders/r/artifact",
    });

    expect(response.headers.get("content-type")).toBe("application/zip");
    expect(response.headers.get("content-disposition")).toContain("r.zip");
  });

  test("cancelRender posts to the cancel route and returns the render", async () => {
    const { requests, stub } = recordingFetch(() => ({
      render: {
        id: "r 1",
        status: "cancelled",
        error: { code: "render_cancelled", message: "the render was cancelled", retryable: false },
      },
    }));
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    const render = await lumen.cancelRender("r 1");

    expect(requests.map((request) => [request.init.method, request.url])).toEqual([
      ["POST", "https://api.test/v1/renders/r%201/cancel"],
    ]);
    expect(render).toEqual({
      id: "r 1",
      status: "cancelled",
      error: { code: "render_cancelled", message: "the render was cancelled", retryable: false },
    });
  });

  test("download of a cancelled render fails with its code", async () => {
    const { stub } = recordingFetch(() => ({
      render: {
        id: "r",
        status: "cancelled",
        error: { code: "render_cancelled", message: "the render was cancelled" },
      },
    }));
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    await expect(lumen.download("r")).rejects.toMatchObject({ code: "render_cancelled" });
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
  const upload = (request: RecordedRequest) => {
    const path = new URL(request.url).pathname;
    if (path === "/v1/media") return { media, uploadUrl: "/v1/media/media%201/content" };
    if (path.endsWith("/multipart")) return { uploadId: "up/1" };
    if (path.includes("/parts/")) {
      const partNumber = Number(path.split("/").at(-1));
      return { etag: `etag-${partNumber}`, partNumber };
    }
    return { media };
  };

  test("a small body goes up in one request without an explicit length", async () => {
    const { requests, stub } = recordingFetch(upload);
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    await lumen.uploadMedia({ body: new Blob(["abc"], { type: "video/mp4" }), fileName: "a.mp4" });

    expect(requests.map((request) => request.url)).toEqual([
      "https://api.test/v1/media",
      "https://api.test/v1/media/media%201/content",
    ]);
    expect(JSON.parse(String(requests[0]?.init.body))).toEqual({
      contentType: "video/mp4",
      fileName: "a.mp4",
      sizeBytes: 3,
    });
    expect(requests[1]?.init.body).toBeInstanceOf(Blob);
  });

  test("a model upload reports its kind, clips and materials", async () => {
    const model = {
      id: "media 1",
      kind: "model",
      metadata: {
        animations: [{ index: 0, name: "Walk", duration: 0.5 }],
        materials: [{ index: 0, hasBaseColorTexture: true }],
      },
    };
    const { stub } = recordingFetch((request) =>
      new URL(request.url).pathname === "/v1/media"
        ? { media: model, uploadUrl: "/v1/media/media%201/content" }
        : { media: model },
    );
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    const uploaded = await lumen.uploadMedia({ body: new Uint8Array(3), fileName: "fox.glb" });

    expect(uploaded.kind).toBe("model");
    expect(uploaded.metadata?.animations?.map((clip) => clip.name)).toEqual(["Walk"]);
    expect(uploaded.metadata?.materials?.[0]?.hasBaseColorTexture).toBe(true);
  });

  test("a stream goes up in multipart parts with the upload id in the path", async () => {
    const sizes: number[] = [];
    let completed: unknown;
    const { requests, stub } = recordingFetch((request) => {
      if (request.url.includes("/parts/")) sizes.push((request.init.body as Uint8Array).byteLength);
      if (request.url.endsWith("/complete"))
        completed = JSON.parse(String(request.init.body)).parts;
      return upload(request);
    });
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });
    const size = 8 * 1024 * 1024 + 5;

    await lumen.uploadMedia({
      body: new Blob([new Uint8Array(size)]).stream(),
      contentType: "video/mp4",
      fileName: "a.mp4",
      sizeBytes: size,
    });

    expect(requests.map((request) => request.url).slice(1)).toEqual([
      "https://api.test/v1/media/media%201/multipart",
      "https://api.test/v1/media/media%201/multipart/up%2F1/parts/1",
      "https://api.test/v1/media/media%201/multipart/up%2F1/parts/2",
      "https://api.test/v1/media/media%201/multipart/up%2F1/complete",
    ]);
    expect(sizes).toEqual([8 * 1024 * 1024, 5]);
    expect(completed).toEqual([
      { etag: "etag-1", partNumber: 1 },
      { etag: "etag-2", partNumber: 2 },
    ]);
  });

  test("a stream needs its size, and must match it", async () => {
    const { stub } = recordingFetch(upload);
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });
    const stream = () => new Blob(["abc"]).stream();
    const contentType = "video/mp4";

    await expect(lumen.uploadMedia({ body: stream(), contentType })).rejects.toMatchObject({
      code: "invalid_upload",
    });
    await expect(
      lumen.uploadMedia({ body: stream(), contentType, sizeBytes: 9 }),
    ).rejects.toMatchObject({
      code: "invalid_upload",
      message: expect.stringContaining("produced 3 bytes"),
    });
  });

  test("the API key is only sent to the API origin", async () => {
    const uploadUrls = ["/v1/media/media1/content", "https://storage.example/put"];
    const authorization: (string | null)[] = [];

    for (const uploadUrl of uploadUrls) {
      const { requests, stub } = recordingFetch(({ url }) =>
        url.endsWith("/media") ? { media, uploadUrl } : { media },
      );
      const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });
      await lumen.uploadMedia({ body: new Uint8Array(3), contentType: "video/mp4" });
      authorization.push(new Headers(requests[1]?.init.headers).get("authorization"));
    }

    expect(authorization).toEqual(["Bearer test-key", null]);
  });

  test("the content type is the given one, the blob's, or the file name's", async () => {
    const sent: string[] = [];
    const { stub } = recordingFetch((request) => {
      if (new URL(request.url).pathname === "/v1/media") {
        sent.push(JSON.parse(String(request.init.body)).contentType);
      }
      return upload(request);
    });
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    await lumen.uploadMedia({
      body: new Uint8Array(3),
      contentType: "audio/ogg",
      fileName: "a.mp4",
    });
    await lumen.uploadMedia({ body: new Blob(["abc"], { type: "image/png" }), fileName: "a.mp4" });
    await lumen.uploadMedia({ body: new Uint8Array(3), fileName: "Clip.MOV" });

    expect(sent).toEqual(["audio/ogg", "image/png", "video/quicktime"]);
  });

  test("an unknown content type fails before any request", async () => {
    const { requests, stub } = recordingFetch(upload);
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    await expect(lumen.uploadMedia({ body: new Uint8Array(3) })).rejects.toMatchObject({
      code: "invalid_upload",
      message: expect.stringContaining("contentType"),
    });
    await expect(
      lumen.uploadMedia({ body: new Uint8Array(3), fileName: "notes.txt" }),
    ).rejects.toMatchObject({ code: "invalid_upload" });
    expect(requests).toHaveLength(0);
  });

  test("aborting a stalled stream rejects at once and cancels the producer", async () => {
    const cancel = vi.fn();
    const body = new ReadableStream<Uint8Array>({ cancel });
    const { stub } = recordingFetch(upload);
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });
    const controller = new AbortController();
    const reason = new Error("stop");

    const uploading = lumen.uploadMedia({
      body,
      contentType: "video/mp4",
      sizeBytes: 1,
      signal: controller.signal,
    });
    await new Promise((resolve) => setTimeout(resolve, 5));
    controller.abort(reason);

    await expect(uploading).rejects.toBe(reason);
    expect(cancel).toHaveBeenCalledOnce();
    expect(body.locked).toBe(false);
  });

  test("a stream that exceeds its size is rejected without being read to the end", async () => {
    const cancel = vi.fn();
    const body = new ReadableStream<Uint8Array>({
      pull(controller) {
        controller.enqueue(new Uint8Array(4));
      },
      cancel,
    });
    const { requests, stub } = recordingFetch(upload);
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    await expect(
      lumen.uploadMedia({ body, contentType: "video/mp4", sizeBytes: 6 }),
    ).rejects.toMatchObject({ code: "invalid_upload" });

    expect(cancel).toHaveBeenCalledOnce();
    expect(requests.some((request) => request.url.endsWith("/complete"))).toBe(false);
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
      lumen.uploadMedia({ body: new Uint8Array(3), contentType: "video/mp4" }),
    ).rejects.toMatchObject({ code: "invalid_response" });
  });

  test("an upload can be passed where a render takes media", async () => {
    const { requests, stub } = recordingFetch(() => ({ render: { id: "r", status: "queued" } }));
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    await lumen.render({} as never, {
      media: { hero: media as never, logo: "https://example.com/logo.png" },
    });

    expect(JSON.parse(String(requests[0]?.init.body)).media).toEqual({
      hero: "lumen:media 1",
      logo: "https://example.com/logo.png",
    });
  });
});

describe("Lumen check", () => {
  test("posts the composition and media manifest and returns the diagnostics", async () => {
    const warning = { severity: "warning", code: "font.fallback", path: "/nodes/1", message: "x" };
    const { requests, stub } = recordingFetch(() => ({
      ok: true,
      diagnostics: [warning, { code: "malformed" }],
    }));
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    const result = await lumen.check({ nodes: [] } as never, { media: { hero: "lumen:m1" } });

    expect(result).toEqual({ ok: true, diagnostics: [warning] });
    expect(requests[0]?.url).toBe("https://api.test/v1/renders/check");
    expect(requests[0]?.init.method).toBe("POST");
    expect(JSON.parse(String(requests[0]?.init.body))).toEqual({
      composition: { nodes: [] },
      media: { hero: "lumen:m1" },
    });
  });

  test("a response without ok is malformed", async () => {
    const { stub } = recordingFetch(() => ({ diagnostics: [] }));
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    await expect(lumen.check({} as never)).rejects.toMatchObject({ code: "invalid_response" });
  });
});

describe("Lumen media", () => {
  test("getMedia reads the media by its escaped id", async () => {
    const media = { id: "media 1" };
    const { requests, stub } = recordingFetch(() => ({ media }));
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    await expect(lumen.getMedia("media 1")).resolves.toEqual(media);
    expect(requests[0]?.url).toBe("https://api.test/v1/media/media%201");
    expect(requests[0]?.init.method).toBe("GET");
  });

  test("deleteMedia accepts an empty 204 and rejects a missing media", async () => {
    const calls: string[] = [];
    const lumen = new Lumen({
      apiKey: "test-key",
      baseUrl: "https://api.test/v1",
      fetch: (async (input: RequestInfo | URL, init: RequestInit = {}) => {
        calls.push(`${init.method} ${String(input)}`);
        return calls.length === 1
          ? new Response(null, { status: 204 })
          : Response.json({ code: "media_not_found", error: "gone" }, { status: 404 });
      }) as typeof fetch,
    });

    await expect(lumen.deleteMedia("m1")).resolves.toBeUndefined();
    await expect(lumen.deleteMedia("m1")).rejects.toMatchObject({
      code: "media_not_found",
      status: 404,
    });
    expect(calls[0]).toBe("DELETE https://api.test/v1/media/m1");
  });
});

describe("Lumen multipart part retries", () => {
  type Step = "network" | "body" | 500 | 429 | 400 | "ok";

  function uploadWith(steps: Step[]) {
    let partRequests = 0;
    const stub = (async (input: RequestInfo | URL) => {
      const path = new URL(String(input)).pathname;
      if (path === "/v1/media") {
        return Response.json({ media: { id: "m" }, uploadUrl: "/v1/media/m/content" });
      }
      if (path.endsWith("/multipart")) return Response.json({ uploadId: "u" });
      if (path.includes("/parts/")) {
        const step = steps[partRequests] ?? "ok";
        partRequests += 1;
        if (step === "network") throw new TypeError("socket hang up");
        if (step === "body") {
          return new Response(
            new ReadableStream({
              start: (controller) => controller.error(new TypeError("terminated")),
            }),
          );
        }
        if (step !== "ok") {
          return Response.json(
            { code: "part_failed", error: "no" },
            { status: step, headers: { "retry-after": "0" } },
          );
        }
        return Response.json({ etag: "e", partNumber: 1 });
      }
      return Response.json({ media: { id: "m" } });
    }) as typeof fetch;
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });
    const upload = () =>
      lumen.uploadMedia({
        body: new Blob(["abc"]).stream(),
        contentType: "video/mp4",
        sizeBytes: 3,
      });
    return { partRequests: () => partRequests, upload };
  }

  test("a part is sent again after a network error, a server error or a rate limit", async () => {
    const mixed = uploadWith(["network", 500, "ok"]);
    await expect(mixed.upload()).resolves.toMatchObject({ id: "m" });
    expect(mixed.partRequests()).toBe(3);

    const limited = uploadWith([429, "ok"]);
    await expect(limited.upload()).resolves.toMatchObject({ id: "m" });
    expect(limited.partRequests()).toBe(2);
  });

  test("a part is sent again when the connection drops while its response is read", async () => {
    const { partRequests, upload } = uploadWith(["body", "ok"]);

    await expect(upload()).resolves.toMatchObject({ id: "m" });
    expect(partRequests()).toBe(2);
  });

  test("a part whose response is not JSON is not sent again", async () => {
    let partRequests = 0;
    const stub = (async (input: RequestInfo | URL) => {
      const path = new URL(String(input)).pathname;
      if (path === "/v1/media") {
        return Response.json({ media: { id: "m" }, uploadUrl: "/v1/media/m/content" });
      }
      if (path.endsWith("/multipart")) return Response.json({ uploadId: "u" });
      partRequests += 1;
      return new Response("not json");
    }) as typeof fetch;
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    await expect(
      lumen.uploadMedia({
        body: new Blob(["abc"]).stream(),
        contentType: "video/mp4",
        sizeBytes: 3,
      }),
    ).rejects.toMatchObject({ code: "invalid_response" });
    expect(partRequests).toBe(1);
  });

  test("a part fails with its last error after three attempts", async () => {
    const { partRequests, upload } = uploadWith([500, 500, 500, "ok"]);

    await expect(upload()).rejects.toMatchObject({ code: "part_failed", status: 500 });
    expect(partRequests()).toBe(3);
  });

  test("a client error other than 429 is not retried", async () => {
    const { partRequests, upload } = uploadWith([400, "ok"]);

    await expect(upload()).rejects.toMatchObject({ status: 400 });
    expect(partRequests()).toBe(1);
  });
});
