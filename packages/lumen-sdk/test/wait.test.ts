import { createHash } from "node:crypto";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import type { Duplex } from "node:stream";

import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import { Lumen } from "../src/client.js";
import { LumenError } from "../src/errors.js";
import { watchRender } from "../src/socket.js";
import type { RenderProgress } from "../src/types.js";

function progressBody(
  status: "queued" | "processing" | "succeeded" | "failed",
  progress: number,
  extra: Record<string, unknown> = {},
) {
  return {
    progress: { progress, state: status, ...extra },
    render: {
      id: "r1",
      status,
      ...(status === "succeeded" ? { outputUrl: "/renders/r1/artifact" } : {}),
    },
  };
}

class FakeSocket {
  static instances: FakeSocket[] = [];
  static failToOpen = false;
  readonly listeners = new Map<string, ((event: unknown) => void)[]>();
  closed = false;

  constructor(
    readonly url: URL,
    readonly init: { headers: Record<string, string> },
  ) {
    if (FakeSocket.failToOpen) {
      throw new SyntaxError("headers are not supported");
    }
    FakeSocket.instances.push(this);
  }

  addEventListener(type: string, listener: (event: unknown) => void) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }

  close() {
    this.closed = true;
  }

  emit(type: string, event: unknown = {}) {
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }

  say(message: unknown) {
    this.emit("message", { data: JSON.stringify(message) });
  }
}

/** Answers `GET /renders/r1/progress` from a script, and records when each read happened. */
function scriptedApi(script: (read: number) => Response) {
  const reads: number[] = [];
  const fetchStub = (async (url: URL) => {
    expect(new URL(url).pathname).toBe("/v1/renders/r1/progress");
    reads.push(Date.now());
    return script(reads.length);
  }) as typeof fetch;
  const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: fetchStub });
  return { lumen, reads };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.spyOn(Math, "random").mockReturnValue(0);
  vi.stubGlobal("WebSocket", FakeSocket);
  FakeSocket.instances = [];
  FakeSocket.failToOpen = false;
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("Lumen.waitForRender", () => {
  test("hears progress on the socket and returns what the read says", async () => {
    const { lumen } = scriptedApi((read) =>
      Response.json(read === 1 ? progressBody("queued", 0) : progressBody("succeeded", 1)),
    );
    const seen: RenderProgress[] = [];

    const waiting = lumen.waitForRender("r1", { onProgress: (progress) => seen.push(progress) });
    await vi.advanceTimersByTimeAsync(0);

    const socket = FakeSocket.instances[0]!;
    expect(socket.url.href).toBe("wss://api.test/v1/renders/r1/socket");
    expect(socket.init.headers.authorization).toBe("Bearer test-key");
    socket.say({ type: "render.started", renderId: "r1" });
    socket.say({ type: "render.progress", renderId: "r1", progress: 0.5 });
    socket.say({ type: "render.completed", renderId: "r1", url: "/x" });

    await expect(waiting).resolves.toMatchObject({
      id: "r1",
      status: "succeeded",
      url: "https://api.test/renders/r1/artifact",
    });
    expect(seen.map((progress) => [progress.status, progress.progress])).toEqual([
      ["queued", 0],
      ["processing", 0],
      ["processing", 0.5],
      ["succeeded", 1],
    ]);
    expect(socket.closed).toBe(true);
  });

  test("reads the hosted event shape too", async () => {
    const { lumen } = scriptedApi((read) =>
      Response.json(read === 1 ? progressBody("queued", 0) : progressBody("succeeded", 1)),
    );
    const seen: RenderProgress[] = [];

    const waiting = lumen.waitForRender("r1", { onProgress: (progress) => seen.push(progress) });
    await vi.advanceTimersByTimeAsync(0);
    FakeSocket.instances[0]!.say({
      type: "render.progress",
      state: { renderId: "r1", state: "processing", progress: 0.25 },
    });
    FakeSocket.instances[0]!.emit("close");
    await vi.advanceTimersByTimeAsync(10_000);
    await waiting;

    expect(seen[1]).toEqual({ id: "r1", status: "processing", progress: 0.25 });
  });

  test("a failed render throws its code and message from progress", async () => {
    const { lumen } = scriptedApi(() =>
      Response.json(
        progressBody("failed", 0.4, {
          error: "media 'hero': not found",
          errorCode: "media_download_failed",
          retryable: true,
        }),
      ),
    );

    const error = await lumen.waitForRender("r1").catch((caught: unknown) => caught);

    expect(error).toBeInstanceOf(LumenError);
    expect(error).toMatchObject({
      code: "media_download_failed",
      message: "media 'hero': not found",
      renderId: "r1",
      retryable: true,
    });
    expect(FakeSocket.instances).toHaveLength(0);
  });

  test("without working sockets it reads every 10, 20, then 30 seconds", async () => {
    FakeSocket.failToOpen = true;
    const { lumen, reads } = scriptedApi((read) =>
      Response.json(
        read < 5 ? progressBody("processing", 0.1 * read) : progressBody("succeeded", 1),
      ),
    );

    const waiting = lumen.waitForRender("r1");
    await vi.advanceTimersByTimeAsync(100_000);
    await waiting;

    expect(reads.map((at) => (at - reads[0]!) / 1000)).toEqual([0, 10, 30, 60, 90]);
  });

  test("a session that lasts resets the gap", async () => {
    const { lumen, reads } = scriptedApi((read) =>
      Response.json(read < 3 ? progressBody("processing", 0.1) : progressBody("succeeded", 1)),
    );

    const waiting = lumen.waitForRender("r1");
    await vi.advanceTimersByTimeAsync(60_000);
    await vi.advanceTimersByTimeAsync(60_000);
    await waiting;

    expect(reads.map((at) => (at - reads[0]!) / 1000)).toEqual([0, 60, 120]);
    expect(FakeSocket.instances).toHaveLength(2);
  });

  test("rate limits and server errors are retried, after Retry-After", async () => {
    const { lumen, reads } = scriptedApi((read) => {
      if (read === 1) {
        return Response.json(
          { code: "rate_limited", error: "slow down" },
          { status: 429, headers: { "retry-after": "7" } },
        );
      }
      if (read === 2) {
        return Response.json({ error: "bad gateway" }, { status: 502 });
      }
      return Response.json(progressBody("succeeded", 1));
    });

    const waiting = lumen.waitForRender("r1");
    await vi.advanceTimersByTimeAsync(7_000 + 4_000);
    await waiting;

    expect(reads.map((at) => (at - reads[0]!) / 1000)).toEqual([0, 7, 11]);
  });

  test("an unknown render and a bad key end the wait", async () => {
    const { lumen, reads } = scriptedApi(() =>
      Response.json({ code: "render_not_found", error: "render was not found" }, { status: 404 }),
    );

    await expect(lumen.waitForRender("r1")).rejects.toMatchObject({
      code: "render_not_found",
      status: 404,
    });
    expect(reads).toHaveLength(1);
  });

  test("persistent server errors give up", async () => {
    const { lumen, reads } = scriptedApi(() => Response.json({ error: "down" }, { status: 503 }));

    const waiting = lumen.waitForRender("r1").catch((caught: unknown) => caught);
    await vi.advanceTimersByTimeAsync(10 * 60_000);

    expect(await waiting).toMatchObject({ status: 503 });
    expect(reads).toHaveLength(8);
  });

  test("aborting stops the wait with the signal's reason, and closes the socket", async () => {
    const { lumen, reads } = scriptedApi(() => Response.json(progressBody("processing", 0.1)));
    const controller = new AbortController();

    const waiting = lumen.waitForRender("r1", { signal: controller.signal });
    const outcome = waiting.catch((caught: unknown) => caught);
    await vi.advanceTimersByTimeAsync(0);
    controller.abort(new Error("stop"));

    expect(await outcome).toMatchObject({ message: "stop" });
    expect(FakeSocket.instances[0]!.closed).toBe(true);
    await vi.advanceTimersByTimeAsync(60_000);
    expect(reads).toHaveLength(1);
  });
});

describe("render socket", () => {
  test("Node's WebSocket sends the Authorization header on the upgrade", async () => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
    let authorization: string | undefined;
    let upgraded: Duplex | undefined;
    const server = createServer();
    server.on("upgrade", (request, socket) => {
      authorization = request.headers.authorization;
      upgraded = socket;
      const accept = createHash("sha1")
        .update(`${request.headers["sec-websocket-key"]}258EAFA5-E914-47DA-95CA-C5AB0DC85B11`)
        .digest("base64");
      socket.write(
        `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`,
      );
      const payload = Buffer.from(
        JSON.stringify({ type: "render.progress", renderId: "r1", progress: 0.5 }),
      );
      socket.write(Buffer.concat([Buffer.from([0x81, payload.length]), payload]));
    });
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    const { port } = server.address() as AddressInfo;

    const seen: RenderProgress[] = [];
    await watchRender({
      apiKey: "test-key",
      fetch,
      onProgress: (progress) => {
        seen.push(progress);
        upgraded?.destroy();
      },
      signal: undefined,
      timeoutMs: 5_000,
      url: new URL(`http://127.0.0.1:${port}/renders/r1/socket`),
    });
    server.close();

    expect(authorization).toBe("Bearer test-key");
    expect(seen).toEqual([{ id: "r1", status: "processing", progress: 0.5 }]);
  });
});
