import { afterEach, describe, expect, it, vi } from "vitest";

import { createLumenPreviewRuntime, type LumenPreviewBindings } from "../src/index.js";
import { LumenMediaError } from "../src/media/errors.js";
import { ImageSession } from "../src/media/image-session.js";
import { describeError } from "../src/session/errors.js";

function controllerFailingWith(error: unknown) {
  const renders: unknown[] = [];
  class Base {
    currentFrame(): number {
      return 0;
    }
    durationFrames(): number {
      return 10;
    }
    frameRequirements(): string {
      return JSON.stringify({ images: [], videos: [] });
    }
    renderNow(canvas: unknown): Promise<void> {
      renders.push(canvas);
      return renders.length === 1 ? Promise.reject(error) : Promise.resolve();
    }
  }
  const { LumenPreviewController } = createLumenPreviewRuntime({
    LumenMediaStore: Base,
    LumenPreviewController: Base,
    LumenRenderer: Base,
  } as unknown as LumenPreviewBindings);
  const controller = new LumenPreviewController();
  const canvas = {} as HTMLCanvasElement;
  return { render: () => controller.renderNowAsync(canvas), renders };
}

describe("preview errors", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("loads and renders again only for the frame-miss code", async () => {
    const miss = Object.assign(new Error("media frame 0 is out of range"), {
      code: "media_frame_out_of_range",
    });
    const retried = controllerFailingWith(miss);
    await retried.render();
    expect(retried.renders).toHaveLength(2);

    const sameWords = new Error('image "media frame out of range.png" is not registered');
    const failed = controllerFailingWith(sameWords);
    await expect(failed.render()).rejects.toBe(sameWords);
    expect(failed.renders).toHaveLength(1);
  });

  it("describes an error by its message, without the stack", () => {
    const error = new LumenMediaError("decode_failed", 'image source "logo" failed to decode');
    expect(error.stack).toContain("errors.test.ts");
    expect(describeError(error)).toBe('image source "logo" failed to decode');
    expect(describeError("plain text")).toBe("plain text");
  });

  it("names the source when a media failure didn't", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(new Response(null, { status: 404, statusText: "Not Found" }))),
    );

    const error = await new ImageSession("logo", "https://example.com/logo.png")
      .load()
      .catch((caught: unknown) => caught);

    expect(error).toBeInstanceOf(LumenMediaError);
    expect((error as LumenMediaError).code).toBe("invalid_source");
    expect((error as LumenMediaError).sourceId).toBe("logo");
    expect((error as LumenMediaError).message).toBe(
      'image source "logo" failed to decode: failed to load media source: 404 Not Found',
    );
  });
});
