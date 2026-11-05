import { describe, expect, test } from "vitest";

import { Lumen } from "../src/client.js";
import { LumenError } from "../src/errors.js";

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
  test("render returns the queued render with an absolute url", async () => {
    const { requests, stub } = recordingFetch(() => ({
      cached: true,
      render: { id: "render-1", status: "succeeded", outputUrl: "/v1/renders/render-1/artifact" },
    }));
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    const { id, url, cached } = await lumen.render({} as never, { idempotencyKey: "key-1" });

    expect({ id, url, cached }).toEqual({
      id: "render-1",
      url: "https://api.test/v1/renders/render-1/artifact",
      cached: true,
    });
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
    if (path.includes("/parts/")) return { etag: "e", partNumber: 1 };
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

  test("a stream goes up in multipart parts with the upload id in the path", async () => {
    const sizes: number[] = [];
    const { requests, stub } = recordingFetch((request) => {
      if (request.url.includes("/parts/")) sizes.push((request.init.body as Uint8Array).byteLength);
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
  });

  test("a stream needs its size, and must match it", async () => {
    const { stub } = recordingFetch(upload);
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });
    const stream = () => new Blob(["abc"]).stream();

    await expect(lumen.uploadMedia({ body: stream() })).rejects.toMatchObject({
      code: "invalid_upload",
    });
    await expect(lumen.uploadMedia({ body: stream(), sizeBytes: 9 })).rejects.toMatchObject({
      code: "invalid_upload",
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
      await lumen.uploadMedia({ body: new Uint8Array(3) });
      authorization.push(new Headers(requests[1]?.init.headers).get("authorization"));
    }

    expect(authorization).toEqual(["Bearer test-key", null]);
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
