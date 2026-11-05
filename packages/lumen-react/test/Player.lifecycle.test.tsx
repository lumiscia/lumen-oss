// @vitest-environment jsdom

import { act, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createLumenPreview, type LumenWorkerBindings } from "@lumiscia/lumen-preview";

import { Player } from "../src/Player.js";

class FakeWorker {
  static instances: FakeWorker[] = [];
  onmessage: ((event: { data: unknown }) => void) | null = null;
  terminated = false;
  constructor() {
    FakeWorker.instances.push(this);
  }
  postMessage() {}
  terminate() {
    this.terminated = true;
  }
}

const bindings: LumenWorkerBindings = {
  preview: () => Promise.reject(new Error("unused")),
  previewWasmUrl: () => "https://a.test/lumen.wasm",
  previewWorkerUrl: () => "https://a.test/worker.js",
};

// The real session, with only the browser's Worker, canvas transfer and WebGPU faked.
describe("Player with a real session", () => {
  beforeEach(() => {
    FakeWorker.instances = [];
    globalThis.IS_REACT_ACT_ENVIRONMENT = true;
    vi.stubGlobal("Worker", FakeWorker);
    vi.stubGlobal("OffscreenCanvas", class {});
    vi.stubGlobal("navigator", { gpu: {} });
    HTMLCanvasElement.prototype.transferControlToOffscreen = () => ({}) as OffscreenCanvas;
  });

  afterEach(() => {
    delete globalThis.IS_REACT_ACT_ENVIRONMENT;
    vi.unstubAllGlobals();
  });

  it("leaves one worker and one canvas under StrictMode, and neither after unmount", async () => {
    const container = document.createElement("div");
    const root = createRoot(container);
    const preview = createLumenPreview();

    await act(async () => {
      root.render(
        <StrictMode>
          <Player preview={preview} bindings={bindings} />
        </StrictMode>,
      );
    });
    const [first, second] = FakeWorker.instances;
    expect(FakeWorker.instances).toHaveLength(2);
    expect(first?.terminated).toBe(true);
    expect(second?.terminated).toBe(false);

    await act(async () => {
      second?.onmessage?.({ data: { type: "ready" } });
    });
    expect(container.querySelectorAll("canvas")).toHaveLength(1);
    expect(preview.getSnapshot().error).toBeNull();

    await act(async () => root.unmount());
    expect(second?.terminated).toBe(true);
    expect(container.querySelectorAll("canvas")).toHaveLength(0);
  });
});
