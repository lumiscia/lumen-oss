import { describe, expect, test } from "vitest";

import { Composition, Lumen } from "../src/index.js";
import type { LumenWebSocket, RenderEvent } from "../src/types.js";

describe("Lumen high-level render helpers", () => {
  test("renderAndWait submits a composition and resolves on completion", async () => {
    const socket = new TestSocket();
    const lumen = new Lumen({
      apiKey: "test-key",
      baseUrl: "https://example.test/api/v1",
      fetch: async () =>
        Response.json({
          render: renderJob("render_123", "queued"),
        }),
      websocket: () => socket,
    });
    const events: RenderEvent[] = [];
    const promise = lumen.renderAndWait(tinyComposition(), {
      onEvent: (event) => events.push(event),
    });

    await socket.ready;
    socket.open();
    socket.message({
      type: "render.completed",
      renderId: "render_123",
      url: "/renders/render_123/artifact",
    });

    await expect(promise).resolves.toMatchObject({
      completed: {
        renderId: "render_123",
        type: "render.completed",
      },
      render: {
        id: "render_123",
      },
    });
    expect(events.map((event) => event.type)).toEqual(["render.completed"]);
    expect(socket.sent).toEqual([JSON.stringify({ type: "auth", apiKey: "test-key" })]);
    expect(socket.closed).toBe(true);
  });

  test("renderArtifact downloads the completed render artifact", async () => {
    const socket = new TestSocket();
    const fetches: string[] = [];
    const lumen = new Lumen({
      apiKey: "test-key",
      baseUrl: "https://example.test/api/v1",
      fetch: async (input) => {
        fetches.push(String(input));
        if (String(input).endsWith("/artifact")) {
          return new Response("mp4", {
            headers: { "content-type": "video/mp4" },
          });
        }
        return Response.json({
          render: renderJob("render_456", "queued"),
        });
      },
      websocket: () => socket,
    });
    const promise = lumen.renderArtifact(tinyComposition());

    await socket.ready;
    socket.open();
    socket.message({
      type: "render.completed",
      renderId: "render_456",
      url: "/renders/render_456/artifact",
    });

    const result = await promise;
    expect(await result.artifact.text()).toBe("mp4");
    expect(fetches).toEqual([
      "https://example.test/api/v1/renders",
      "https://example.test/api/v1/renders/render_456/artifact",
    ]);
  });
});

function tinyComposition(): Composition {
  const composition = new Composition();
  const solid = composition.addNode({
    type: "solid_color",
    properties: {
      color: [255, 0, 0, 255],
      height: 1,
      width: 1,
    },
  });
  const output = composition.addNode({ type: "media_output" });
  composition.connect(solid, output, { toPort: "source" });
  return composition;
}

function renderJob(id: string, status: "queued" | "succeeded") {
  return {
    costCents: 0,
    createdAt: "2026-05-15T00:00:00.000Z",
    id,
    inputHash: "hash",
    organizationId: "org",
    status,
  };
}

class TestSocket implements LumenWebSocket {
  closed = false;
  ready: Promise<void>;
  sent: string[] = [];
  #listeners = new Map<string, Set<(event: unknown) => void>>();
  #resolveReady: () => void;

  constructor() {
    this.#resolveReady = () => {};
    this.ready = new Promise((resolve) => {
      this.#resolveReady = resolve;
    });
  }

  addEventListener(type: "close", listener: (event: CloseEvent) => void): void;
  addEventListener(type: "error", listener: (event: Event) => void): void;
  addEventListener(type: "message", listener: (event: MessageEvent) => void): void;
  addEventListener(type: "open", listener: () => void): void;
  addEventListener(type: string, listener: (event: unknown) => void): void {
    const listeners = this.#listeners.get(type) ?? new Set();
    listeners.add(listener);
    this.#listeners.set(type, listeners);
    if (type === "message") {
      this.#resolveReady();
    }
  }

  close(): void {
    this.closed = true;
  }

  send(data: string): void {
    this.sent.push(data);
  }

  open(): void {
    this.#emit("open", undefined);
  }

  message(event: RenderEvent): void {
    this.#emit("message", { data: JSON.stringify(event) });
  }

  #emit(type: string, event: unknown): void {
    for (const listener of this.#listeners.get(type) ?? []) {
      listener(event);
    }
  }
}
