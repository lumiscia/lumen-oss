import { afterEach, describe, expect, it, vi } from "vitest";

import { createLumenPreviewRuntime, type LumenPreviewBindings } from "../src/runtime.js";
import {
  LumenMediaError,
  type LumenMediaErrorCode,
  type LumenMediaErrorKinds,
} from "../src/media/errors.js";
import { ImageSession } from "../src/media/image-session.js";
import { describeError, errorMediaFailure } from "../src/session/errors.js";

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
  const render = async () => {
    await controller.prepareFrame();
    await controller.renderPrepared(canvas);
  };
  return { render, renders };
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

describe("media failures from the bindings", () => {
  it("reads the structured fields the bindings attach to an error", () => {
    const media = {
      code: "media.too_large",
      source: "hero.glb",
      wanted: ["model"],
      details: "the model is larger than the 512 MiB a model may be",
      limit: 536_870_912,
      actual: 600_000_000,
    };
    const thrown = Object.assign(new Error("model `hero.glb` is too large"), {
      code: "media.too_large",
      media,
    });

    expect(errorMediaFailure(thrown)).toEqual(media);
    expect(errorMediaFailure(new Error("device lost"))).toBeNull();
    expect(errorMediaFailure("plain text")).toBeNull();

    const browserSide = new LumenMediaError("unsupported_container", "no demuxer for it", {
      sourceId: "clip",
    });
    expect(errorMediaFailure(browserSide)).toEqual({
      code: "media.unsupported",
      source: "clip",
      wanted: [],
      details: "no demuxer for it",
    });
    expect(errorMediaFailure(new LumenMediaError("decode_failed", "no source"))).toBeNull();

    // Each browser-side reason agrees with the engine's, with the kinds it knows.
    const mapped = (code: LumenMediaErrorCode, kinds: LumenMediaErrorKinds = {}) =>
      errorMediaFailure(new LumenMediaError(code, "x", { sourceId: "clip", ...kinds }))?.code;
    expect(mapped("source_not_registered")).toBe("media.not_found");
    expect(mapped("invalid_source")).toBe("media.not_found");
    expect(mapped("track_not_found")).toBe("media.not_found");
    expect(mapped("no_video_track", { wanted: ["video"], found: "audio" })).toBe(
      "media.wrong_kind",
    );
    expect(mapped("track_not_decodable")).toBe("media.unsupported");
    expect(mapped("decode_failed")).toBe("media.decode");
    expect(
      errorMediaFailure(
        new LumenMediaError("no_video_track", "x", {
          sourceId: "clip",
          wanted: ["video"],
          found: "audio",
        }),
      ),
    ).toMatchObject({ wanted: ["video"], found: "audio" });
    expect(errorMediaFailure(Object.assign(new Error("x"), { media: { code: 3 } }))).toBeNull();
  });
});
