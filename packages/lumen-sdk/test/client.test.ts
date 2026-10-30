import { describe, expect, test } from "vitest";

import { Lumen } from "../src/client.js";
import { LumenRenderError, normalizeRenderEvent } from "../src/events.js";
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

  test("rejects with a LumenRenderError when the hosted API reports a failure", async () => {
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
    expect(error).toBeInstanceOf(LumenRenderError);
    expect(error).toMatchObject({
      code: "render_failed",
      message: "Render render-1 failed: Out of GPU memory",
      renderId: "render-1",
    });
    expect(harness.clientCloseCount()).toBe(1);
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
