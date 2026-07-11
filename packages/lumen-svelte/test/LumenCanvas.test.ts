// @vitest-environment jsdom

import { flushSync, mount, tick, unmount } from "svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { LumenPreviewBindingSource } from "@lumiscia/lumen-preview";

import { createLumenPreview } from "../src/lib/preview.svelte.js";
import Harness from "./LumenCanvasHarness.svelte";

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

const bindings = {} as LumenPreviewBindingSource;

describe("LumenCanvas lifecycle", () => {
  beforeEach(() => {
    sessions.length = 0;
    document.body.replaceChildren();
  });

  it("replaces the session and ignores a stale attach failure", async () => {
    const firstPreview = createLumenPreview();
    const nextPreview = createLumenPreview();
    const component = mount(Harness, {
      target: document.body,
      props: { initialPreview: firstPreview, initialBindings: bindings },
    });
    flushSync();
    const firstSession = sessions[0];

    component.replace(nextPreview, bindings, "next composition");
    flushSync();
    const nextSession = sessions[1];

    expect(firstSession.disposed).toBe(true);
    expect(nextSession.options.preview).toBe(nextPreview.core);
    expect(nextSession.options.compositionJson).toBe("next composition");

    firstSession.attachResult.reject(new Error("stale failure"));
    await tick();

    expect(firstPreview.error).toBeNull();
    expect(nextPreview.error).toBeNull();

    await unmount(component);
  });

  it("reports the active attach failure but ignores one after unmount", async () => {
    const preview = createLumenPreview();
    const component = mount(Harness, {
      target: document.body,
      props: { initialPreview: preview, initialBindings: bindings },
    });
    flushSync();
    const session = sessions[0];

    session.attachResult.reject(new Error("attach failed"));
    await tick();
    expect(preview.error).toContain("attach failed");

    const unmountedPreview = createLumenPreview();
    component.replace(unmountedPreview, bindings, "replacement");
    flushSync();
    const unmountedSession = sessions[1];
    await unmount(component);

    unmountedSession.attachResult.reject(new Error("late failure"));
    await tick();

    expect(unmountedSession.disposed).toBe(true);
    expect(unmountedPreview.error).toBeNull();
  });

  it("routes prop updates only to the current session", async () => {
    const firstPreview = createLumenPreview();
    const nextPreview = createLumenPreview();
    const component = mount(Harness, {
      target: document.body,
      props: { initialPreview: firstPreview, initialBindings: bindings },
    });
    flushSync();
    const firstSession = sessions[0];

    component.replace(nextPreview, bindings, "replacement");
    flushSync();
    const nextSession = sessions[1];

    expect(firstSession.update).toHaveBeenCalledTimes(1);
    expect(nextSession.update).toHaveBeenLastCalledWith(
      expect.objectContaining({ compositionJson: "replacement" }),
    );

    await unmount(component);
  });
});
