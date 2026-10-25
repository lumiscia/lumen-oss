// @vitest-environment jsdom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { LumenPreviewBindingSource } from "@lumiscia/lumen-preview";
import { createLumenPreview } from "@lumiscia/lumen-preview/preview";

interface PreviewTarget {
  update(patch: { error: string }): void;
}

interface SessionOptions {
  preview: PreviewTarget;
  bindings: LumenPreviewBindingSource;
  compositionJson?: string | null;
}

interface DeferredAttach {
  promise: Promise<void>;
  reject(error: unknown): void;
}

interface MockSession {
  readonly options: SessionOptions;
  readonly attachResult: DeferredAttach;
  readonly updates: object[];
  disposed: boolean;
  attach: ReturnType<typeof vi.fn>;
  update: ReturnType<typeof vi.fn>;
  dispose: ReturnType<typeof vi.fn>;
}

const sessions = vi.hoisted(() => [] as MockSession[]);

vi.mock("@lumiscia/lumen-preview", () => {
  function deferredAttach(): DeferredAttach {
    let reject!: (error: unknown) => void;
    const promise = new Promise<void>((_resolve, rejectPromise) => {
      reject = rejectPromise;
    });
    return { promise, reject };
  }

  return {
    LumenPreviewSession: class implements MockSession {
      readonly attachResult = deferredAttach();
      readonly updates: object[] = [];
      disposed = false;
      readonly attach = vi.fn(() => this.attachResult.promise);
      readonly update = vi.fn((options: object) => this.updates.push(options));
      readonly dispose = vi.fn(() => {
        this.disposed = true;
      });

      constructor(readonly options: SessionOptions) {
        sessions.push(this);
      }
    },
  };
});

import { LumenCanvas } from "../src/LumenCanvas.js";

const bindings = {} as LumenPreviewBindingSource;

describe("LumenCanvas lifecycle", () => {
  beforeEach(() => {
    sessions.length = 0;
    globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  });

  afterEach(() => {
    delete globalThis.IS_REACT_ACT_ENVIRONMENT;
    document.body.replaceChildren();
  });

  it("replaces the session and ignores a stale attach failure", async () => {
    const firstPreview = createLumenPreview();
    const nextPreview = createLumenPreview();
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(<LumenCanvas preview={firstPreview} bindings={bindings} />);
    });
    const firstSession = sessions[0];

    await act(async () => {
      root.render(
        <LumenCanvas
          preview={nextPreview}
          bindings={bindings}
          compositionJson="next composition"
        />,
      );
    });
    const nextSession = sessions[1];

    expect(firstSession.disposed).toBe(true);
    expect(nextSession.options.preview).toBe(nextPreview);
    expect(nextSession.options.compositionJson).toBe("next composition");

    await act(async () => {
      firstSession.attachResult.reject(new Error("stale failure"));
      await Promise.resolve();
    });

    expect(firstPreview.getSnapshot().error).toBeNull();
    expect(nextPreview.getSnapshot().error).toBeNull();

    await act(async () => root.unmount());
  });

  it("reports the active attach failure but ignores one after unmount", async () => {
    const activePreview = createLumenPreview();
    const container = document.createElement("div");
    const root = createRoot(container);

    await act(async () => {
      root.render(<LumenCanvas preview={activePreview} bindings={bindings} />);
    });
    const activeSession = sessions[0];

    await act(async () => {
      activeSession.attachResult.reject(new Error("attach failed"));
      await Promise.resolve();
    });
    expect(activePreview.getSnapshot().error).toContain("attach failed");

    const unmountedPreview = createLumenPreview();
    await act(async () => {
      root.render(<LumenCanvas preview={unmountedPreview} bindings={bindings} />);
    });
    const unmountedSession = sessions[1];
    await act(async () => root.unmount());

    unmountedSession.attachResult.reject(new Error("late failure"));
    await Promise.resolve();

    expect(unmountedSession.disposed).toBe(true);
    expect(unmountedPreview.getSnapshot().error).toBeNull();
  });

  it("routes prop updates only to the current session", async () => {
    const firstPreview = createLumenPreview();
    const nextPreview = createLumenPreview();
    const container = document.createElement("div");
    const root = createRoot(container);

    await act(async () => {
      root.render(<LumenCanvas preview={firstPreview} bindings={bindings} />);
    });
    const firstSession = sessions[0];

    await act(async () => {
      root.render(
        <LumenCanvas
          preview={nextPreview}
          bindings={bindings}
          compositionJson="replacement"
          lookaheadCount={3}
        />,
      );
    });
    const nextSession = sessions[1];

    expect(firstSession.update).toHaveBeenCalledTimes(1);
    expect(nextSession.update).toHaveBeenLastCalledWith(
      expect.objectContaining({ compositionJson: "replacement", lookaheadCount: 3 }),
    );

    await act(async () => root.unmount());
  });
});
