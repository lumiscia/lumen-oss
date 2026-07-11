import { describe, expect, test } from "vitest";

import { Lumen } from "../src/client.js";
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
});

function webSocketHarness(): {
  socket: LumenWebSocket;
  closeFromServer: (code: number, reason: string) => void;
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
    clientCloseCount: () => closeCount,
  };
}
