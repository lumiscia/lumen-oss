// @vitest-environment jsdom

import { act, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { LumenWorkerBindings } from "@lumiscia/lumen-preview";
import { createLumenPreview } from "@lumiscia/lumen-preview/preview";

interface MockSession {
  host: HTMLElement;
  options: { composition?: unknown; preview: unknown };
  dispose: ReturnType<typeof vi.fn>;
  update: ReturnType<typeof vi.fn>;
}

const sessions = vi.hoisted(() => [] as MockSession[]);

vi.mock("@lumiscia/lumen-preview", () => ({
  mountPreview: (host: HTMLElement, options: MockSession["options"]) => {
    const session = { dispose: vi.fn(), host, options, update: vi.fn() };
    sessions.push(session);
    return session;
  },
}));

import { LumenCanvas } from "../src/LumenCanvas.js";

const bindings = {} as LumenWorkerBindings;

describe("LumenCanvas", () => {
  beforeEach(() => {
    sessions.length = 0;
    globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  });

  afterEach(() => {
    delete globalThis.IS_REACT_ACT_ENVIRONMENT;
  });

  it("mounts into its host and replaces the session with a new preview", async () => {
    const firstPreview = createLumenPreview();
    const nextPreview = createLumenPreview();
    const container = document.createElement("div");
    const root = createRoot(container);

    await act(async () => {
      root.render(<LumenCanvas preview={firstPreview} bindings={bindings} className="stage" />);
    });
    expect(sessions[0]?.host.className).toBe("stage");

    await act(async () => {
      root.render(<LumenCanvas preview={nextPreview} bindings={bindings} composition={null} />);
    });
    expect(sessions[0]?.dispose).toHaveBeenCalledOnce();
    expect(sessions[1]?.options.preview).toBe(nextPreview);

    await act(async () => root.unmount());
    expect(sessions[1]?.dispose).toHaveBeenCalledOnce();
  });

  it("disposes the session StrictMode remounts", async () => {
    const preview = createLumenPreview();
    const root = createRoot(document.createElement("div"));

    await act(async () => {
      root.render(
        <StrictMode>
          <LumenCanvas preview={preview} bindings={bindings} />
        </StrictMode>,
      );
    });
    expect(sessions).toHaveLength(2);
    expect(sessions[0]?.dispose).toHaveBeenCalledOnce();
    expect(sessions[1]?.dispose).not.toHaveBeenCalled();

    await act(async () => root.unmount());
  });

  it("sends a changed input to the current session only", async () => {
    const preview = createLumenPreview();
    const root = createRoot(document.createElement("div"));
    const composition = { toJSON: () => ({}) } as never;

    await act(async () => {
      root.render(<LumenCanvas preview={preview} bindings={bindings} />);
    });
    await act(async () => {
      root.render(<LumenCanvas preview={preview} bindings={bindings} composition={composition} />);
    });
    expect(sessions).toHaveLength(1);
    expect(sessions[0]?.update).toHaveBeenLastCalledWith(expect.objectContaining({ composition }));

    await act(async () => root.unmount());
  });
});
