import { createHash } from "node:crypto";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import type { Duplex } from "node:stream";

import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import { Lumen } from "../src/client.js";
import { LumenError } from "../src/errors.js";
import { watchRender, type SocketUpdate } from "../src/socket.js";
import type { RenderProgress } from "../src/types.js";

function progressBody(
  status: "queued" | "processing" | "succeeded" | "failed" | "cancelled",
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
  static startedAt: number[] = [];
  /** Whether each socket opens, in order. Sockets past the end of the plan open. */
  static plan: boolean[] = [];
  /** A message every socket delivers as soon as it is listened to. */
  static greeting: unknown;
  readonly listeners = new Map<string, ((event: unknown) => void)[]>();
  closed = false;

  constructor(
    readonly url: URL,
    readonly init: { headers: Record<string, string> },
  ) {
    FakeSocket.startedAt.push(Date.now());
    if (FakeSocket.plan.shift() === false) {
      throw new SyntaxError("headers are not supported");
    }
    FakeSocket.instances.push(this);
  }

  addEventListener(type: string, listener: (event: unknown) => void) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
    if (type === "message" && FakeSocket.greeting !== undefined) {
      queueMicrotask(() => listener({ data: JSON.stringify(FakeSocket.greeting) }));
    }
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

/** Answers `GET /renders/:id/progress` from a script, and records when each read happened. */
function scriptedApi(script: (read: number, id: string) => Response) {
  const reads: number[] = [];
  const fetchStub = (async (url: URL) => {
    const match = /^\/v1\/renders\/(\w+)\/progress$/.exec(new URL(url).pathname);
    expect(match).not.toBeNull();
    reads.push(Date.now());
    return script(reads.length, match![1]!);
  }) as typeof fetch;
  const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: fetchStub });
  return { lumen, reads };
}

const seconds = (times: number[]) => times.map((at) => (at - times[0]!) / 1000);

beforeEach(() => {
  vi.useFakeTimers();
  vi.spyOn(Math, "random").mockReturnValue(0);
  vi.stubGlobal("WebSocket", FakeSocket);
  FakeSocket.instances = [];
  FakeSocket.startedAt = [];
  FakeSocket.plan = [];
  FakeSocket.greeting = undefined;
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
      url: "https://api.test/v1/renders/r1/artifact",
    });
    expect(seen.map((progress) => [progress.status, progress.progress])).toEqual([
      ["queued", 0],
      ["processing", 0],
      ["processing", 0.5],
      ["succeeded", 1],
    ]);
    expect(socket.closed).toBe(true);
  });

  test("hosted events: the state wins over the type, and progress never goes back", async () => {
    const { lumen } = scriptedApi((read) =>
      Response.json(read === 1 ? progressBody("processing", 0.5) : progressBody("succeeded", 1)),
    );
    const seen: RenderProgress[] = [];

    const waiting = lumen.waitForRender("r1", { onProgress: (progress) => seen.push(progress) });
    await vi.advanceTimersByTimeAsync(0);
    const socket = FakeSocket.instances[0]!;
    const say = (type: string, state: string, progress: number) =>
      socket.say({ type, state: { renderId: "r1", state, progress } });
    say("render.progress", "queued", 0.1);
    say("render.progress", "processing", 0.25);
    say("render.completed", "processing", 0.75);
    expect(socket.closed).toBe(false);
    socket.emit("close");
    await vi.advanceTimersByTimeAsync(10_000);
    await waiting;

    expect(seen.map((progress) => [progress.status, progress.progress])).toEqual([
      ["processing", 0.5],
      ["processing", 0.75],
      ["succeeded", 1],
    ]);
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

  test("a cancelled render throws render_cancelled, and its socket event ends the session", async () => {
    const { lumen } = scriptedApi((read) =>
      Response.json(
        read === 1
          ? progressBody("processing", 0.3)
          : progressBody("cancelled", 0.3, {
              error: "the render was cancelled",
              errorCode: "render_cancelled",
              retryable: false,
            }),
      ),
    );
    const seen: RenderProgress[] = [];

    const waiting = lumen
      .waitForRender("r1", { onProgress: (progress) => seen.push(progress) })
      .catch((caught: unknown) => caught);
    await vi.advanceTimersByTimeAsync(0);
    const socket = FakeSocket.instances[0]!;
    socket.say({ type: "render.cancelled", renderId: "r1" });

    const error = await waiting;

    expect(error).toBeInstanceOf(LumenError);
    expect(error).toMatchObject({
      code: "render_cancelled",
      message: "the render was cancelled",
      renderId: "r1",
      retryable: false,
    });
    expect(socket.closed).toBe(true);
    expect(seen.at(-1)).toEqual({ id: "r1", status: "cancelled", progress: 0.3 });
  });

  test("a cancelled render without an error body still raises render_cancelled", async () => {
    const { lumen } = scriptedApi(() => Response.json(progressBody("cancelled", 0)));

    await expect(lumen.waitForRender("r1")).rejects.toMatchObject({
      code: "render_cancelled",
      message: "Render r1 was cancelled.",
    });
  });

  test("a failed render's code is hosted's stage when there is no errorCode", async () => {
    FakeSocket.plan = [false];
    const { lumen } = scriptedApi((read) =>
      Response.json(
        read === 1
          ? progressBody("processing", 0.2, { stage: "rendering" })
          : progressBody("failed", 1, {
              error: "media hero download failed",
              stage: "media_download_failed",
            }),
      ),
    );

    const waiting = lumen.waitForRender("r1").catch((caught: unknown) => caught);
    await vi.advanceTimersByTimeAsync(10_000);

    expect(await waiting).toMatchObject({
      code: "media_download_failed",
      message: "media hero download failed",
    });
  });

  test("a finished render is understood when hosted has no progress for it", async () => {
    const { lumen } = scriptedApi(() =>
      Response.json({ ...progressBody("succeeded", 1), progress: null }),
    );

    await expect(lumen.waitForRender("r1")).resolves.toMatchObject({
      status: "succeeded",
      url: "https://api.test/v1/renders/r1/artifact",
    });
  });

  test("without working sockets it reads every 10, 20, then 30 seconds", async () => {
    FakeSocket.plan = Array(100).fill(false);
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

  test("a session that lasts resets a gap that had grown", async () => {
    FakeSocket.plan = [false, false, true, false, true];
    const { lumen, reads } = scriptedApi((read) =>
      Response.json(read < 5 ? progressBody("processing", 0.1) : progressBody("succeeded", 1)),
    );

    const waiting = lumen.waitForRender("r1");
    await vi.advanceTimersByTimeAsync(120_000);
    await waiting;

    // Two dropped sockets grow the gap to 10 s, then 20 s. The third runs its 60 s, so the fourth
    // dropping starts from 5 s again and the next read is 10 s later, not 30.
    expect(seconds(reads)).toEqual([0, 10, 30, 90, 100]);
  });

  test("a socket that reports the end before the read does not skip the 5 second gap", async () => {
    FakeSocket.greeting = {
      type: "render.completed",
      state: { renderId: "r1", state: "succeeded", progress: 1 },
    };
    const { lumen, reads } = scriptedApi((read) =>
      Response.json(read < 5 ? progressBody("processing", 0.9) : progressBody("succeeded", 1)),
    );

    const waiting = lumen.waitForRender("r1");
    await vi.advanceTimersByTimeAsync(30_000);
    await waiting;

    expect(seconds(FakeSocket.startedAt)).toEqual([0, 5, 10, 15]);
    expect(reads).toHaveLength(5);
  });

  test("rate limits wait out Retry-After and server errors back off", async () => {
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
    await vi.advanceTimersByTimeAsync(7_000 + 2_000);
    await waiting;

    expect(seconds(reads)).toEqual([0, 7, 9]);
  });

  test("a rate limit never ends a wait", async () => {
    const { lumen, reads } = scriptedApi((read) =>
      read <= 40
        ? Response.json({ error: "API key rate limit exceeded" }, { status: 429 })
        : Response.json(progressBody("succeeded", 1)),
    );

    const waiting = lumen.waitForRender("r1");
    await vi.advanceTimersByTimeAsync(40 * 60_000);

    await expect(waiting).resolves.toMatchObject({ status: "succeeded" });
    expect(reads).toHaveLength(41);
  });

  test("a rate limit makes every wait on the client hold off, without Retry-After too", async () => {
    const { lumen, reads } = scriptedApi((read, id) =>
      id === "r1" && read === 1
        ? Response.json({ error: "API key rate limit exceeded" }, { status: 429 })
        : Response.json(progressBody("succeeded", 1)),
    );

    const first = lumen.waitForRender("r1");
    await vi.advanceTimersByTimeAsync(1_000);
    const second = lumen.waitForRender("r2");
    await vi.advanceTimersByTimeAsync(0);
    expect(reads).toHaveLength(1);

    await vi.advanceTimersByTimeAsync(1_000);
    await Promise.all([first, second]);
    expect(seconds(reads)).toEqual([0, 2, 2]);
  });

  test("the longest rate limit wins, and a wait that wakes early holds off again", async () => {
    const { lumen, reads } = scriptedApi((read) =>
      read === 1
        ? Response.json({ error: "API key rate limit exceeded" }, { status: 429 })
        : read === 2
          ? Response.json(
              { error: "API key rate limit exceeded" },
              { status: 429, headers: { "retry-after": "20" } },
            )
          : Response.json(progressBody("succeeded", 1)),
    );

    const waits = Promise.all([lumen.waitForRender("r1"), lumen.waitForRender("r2")]);
    await vi.advanceTimersByTimeAsync(20_000);
    await waits;
    expect(seconds(reads)).toEqual([0, 0, 20, 20]);
  });

  test("a socket session that has ended reports nothing more", async () => {
    const seen: string[] = [];
    const session = watchRender({
      apiKey: "test-key",
      fetch,
      onProgress: (update) => seen.push(update.status),
      signal: undefined,
      timeoutMs: 60_000,
      url: new URL("https://api.test/v1/renders/r1/socket"),
    });
    await vi.advanceTimersByTimeAsync(0);
    const socket = FakeSocket.instances[0]!;
    socket.say({ type: "render.completed", renderId: "r1" });
    await session;
    socket.say({ type: "render.progress", renderId: "r1", progress: 0.5 });
    expect(seen).toEqual(["succeeded"]);
  });

  test("an unknown render and a bad key end the wait", async () => {
    for (const [status, code] of [
      [404, "render_not_found"],
      [401, "unauthorized"],
    ] as const) {
      const { lumen, reads } = scriptedApi(() => Response.json({ code, error: "no" }, { status }));

      await expect(lumen.waitForRender("r1")).rejects.toMatchObject({ code, status });
      expect(reads).toHaveLength(1);
    }
  });

  test("server errors are retried for ten minutes, then the last one is thrown", async () => {
    const { lumen, reads } = scriptedApi(() => Response.json({ error: "down" }, { status: 503 }));

    const waiting = lumen.waitForRender("r1").catch((caught: unknown) => caught);
    await vi.advanceTimersByTimeAsync(11 * 60_000);

    expect(await waiting).toMatchObject({ status: 503 });
    expect(seconds(reads).at(-1)).toBeGreaterThanOrEqual(600);
    expect(seconds(reads).at(-1)).toBeLessThan(660);
  });

  test("25 waits whose sockets work cost two requests each a minute", async () => {
    const controller = new AbortController();
    const { lumen, reads } = scriptedApi(() => Response.json(progressBody("processing", 0.1)));

    const waits = Array.from({ length: 25 }, () =>
      lumen.waitForRender("r1", { signal: controller.signal }).catch(() => undefined),
    );
    await vi.advanceTimersByTimeAsync(119_999);
    const requests = reads.length + FakeSocket.startedAt.length;
    controller.abort();
    await Promise.all(waits);

    expect(requests).toBe(25 * 2 * 2);
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

describe("Workers socket upgrade", () => {
  const url = new URL("https://api.test/v1/renders/r1/socket");

  function pendingUpgrade() {
    let release!: (response: Response) => void;
    let signal: AbortSignal | null | undefined;
    const stub = ((_url: unknown, init: RequestInit) => {
      signal = init.signal;
      return new Promise<Response>((resolve) => (release = resolve));
    }) as typeof fetch;
    const socket = { accept: vi.fn(), addEventListener: vi.fn(), close: vi.fn() };
    return {
      release: () => release({ webSocket: socket } as unknown as Response),
      signal: () => signal,
      socket,
      stub,
    };
  }

  beforeEach(() => {
    vi.stubGlobal("WebSocketPair", class {});
  });

  test("aborting a pending upgrade ends the session, and a late socket is closed", async () => {
    const upgrade = pendingUpgrade();
    const controller = new AbortController();

    const session = watchRender({
      apiKey: "test-key",
      fetch: upgrade.stub,
      onProgress: () => {},
      signal: controller.signal,
      timeoutMs: 60_000,
      url,
    });
    controller.abort();

    await expect(session).resolves.toEqual({ end: "dropped" });
    expect(upgrade.signal()?.aborted).toBe(true);
    upgrade.release();
    await vi.advanceTimersByTimeAsync(0);
    expect(upgrade.socket.close).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
  });

  test("the session timeout covers the upgrade", async () => {
    const upgrade = pendingUpgrade();

    const session = watchRender({
      apiKey: "test-key",
      fetch: upgrade.stub,
      onProgress: () => {},
      signal: undefined,
      timeoutMs: 60_000,
      url,
    });
    await vi.advanceTimersByTimeAsync(60_000);

    await expect(session).resolves.toEqual({ end: "timeout" });
    expect(upgrade.signal()?.aborted).toBe(true);
  });

  test("a rejected upgrade makes every wait hold off for Retry-After", async () => {
    const reads: number[] = [];
    const stub = (async (requested: URL) => {
      if (new URL(requested).pathname.endsWith("/socket")) {
        return Response.json(
          { error: "rate limited" },
          { status: 429, headers: { "retry-after": "20" } },
        );
      }
      reads.push(Date.now());
      return Response.json(
        reads.length < 2 ? progressBody("processing", 0.1) : progressBody("succeeded", 1),
      );
    }) as typeof fetch;
    const lumen = new Lumen({ apiKey: "test-key", baseUrl: "https://api.test/v1", fetch: stub });

    const waiting = lumen.waitForRender("r1");
    await vi.advanceTimersByTimeAsync(20_000);
    await waiting;

    // Without the cooldown the dropped session's 10 s gap would bring the second read at 10 s.
    expect(seconds(reads)).toEqual([0, 20]);
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

    const seen: SocketUpdate[] = [];
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
