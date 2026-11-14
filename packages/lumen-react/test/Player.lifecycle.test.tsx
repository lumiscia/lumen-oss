// @vitest-environment jsdom

import { act, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createLumenPreview } from "@lumiscia/lumen-preview";

import { FakeWorker } from "../../lumen-preview/test/support/fake-worker.js";
import { Player } from "../src/index.js";

// The real session, with only the browser's Worker, canvas transfer and WebGPU faked.
describe("Player with a real session", () => {
  beforeEach(() => {
    FakeWorker.instances = [];
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    vi.stubGlobal("Worker", FakeWorker);
    vi.stubGlobal("OffscreenCanvas", class {});
    vi.stubGlobal("navigator", { gpu: {} });
    HTMLCanvasElement.prototype.transferControlToOffscreen = () => ({}) as OffscreenCanvas;
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("leaves one worker and one canvas under StrictMode, and neither after unmount", async () => {
    const container = document.createElement("div");
    const root = createRoot(container);
    const preview = createLumenPreview();

    await act(async () => {
      root.render(
        <StrictMode>
          <Player preview={preview} />
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

  it("sends the worker nothing when it renders again with the same inputs", async () => {
    const root = createRoot(document.createElement("div"));
    const preview = createLumenPreview();
    const composition = { metadata: { name: "first" } } as never;
    const onError = vi.fn();
    const render = () =>
      root.render(
        <Player
          preview={preview}
          composition={composition}
          onError={onError}
          range={{ in: 10, out: 40 }}
          rate={2}
        />,
      );

    await act(async () => render());
    const worker = FakeWorker.instances[0];
    await act(async () => {
      worker?.onmessage?.({ data: { type: "ready" } });
    });
    const posted = worker?.posted.length ?? 0;
    expect(worker?.posted.map((message) => (message as { type: string }).type)).toContain("load");

    await act(async () => render());
    expect(FakeWorker.instances).toHaveLength(1);
    expect(worker?.posted).toHaveLength(posted);

    await act(async () => root.unmount());
  });
});
