import { describe, expect, it, vi } from "vitest";

import {
  createVideoFramePayload,
  decodeImageBitmapToRgba,
  decodeImageBlob,
  syncVideoFrameToTarget,
} from "../../src/media/canvas.js";
import type { NativeVideoFrameTarget } from "../../src/media/types.js";

function fakeFrame(format: string, colorSpace: VideoColorSpaceInit) {
  return {
    format,
    colorSpace,
    visibleRect: { width: 4, height: 2 },
    allocationSize: () => 12,
    copyTo: vi.fn(async () => [
      { offset: 0, stride: 4 },
      { offset: 8, stride: 4 },
    ]),
    close: vi.fn(),
  } as unknown as VideoFrame;
}

/** A 2D canvas whose pixels read back as `pixels`, as `OffscreenCanvas` or `document`'s. */
function stubCanvas(pixels: number[], kind: "offscreen" | "document" = "offscreen") {
  const context = {
    drawImage: vi.fn(),
    getImageData: vi.fn(() => ({ data: Uint8ClampedArray.from(pixels) })),
  };
  const getContext = vi.fn(() => context);
  if (kind === "offscreen") {
    vi.stubGlobal(
      "OffscreenCanvas",
      class {
        getContext = getContext;
      },
    );
  } else {
    vi.stubGlobal("document", { createElement: () => ({ getContext }) });
  }
  return { context, getContext };
}

function fakeTarget() {
  return {
    setVideoFrame: vi.fn(),
    setVideoFrameYuv: vi.fn(),
  } as unknown as NativeVideoFrameTarget & {
    setVideoFrame: ReturnType<typeof vi.fn>;
    setVideoFrameYuv: ReturnType<typeof vi.fn>;
  };
}

describe("syncVideoFrameToTarget", () => {
  it("sends native planes with the source's matrix, the copied samples' range and the rotation", async () => {
    const target = fakeTarget();
    // Safari reports its own BT.709 full-range output on the frame, whatever the source says.
    const frame = fakeFrame("NV12", { matrix: "bt709", fullRange: true });
    const payload = createVideoFramePayload(frame, 2, 4, {
      sourceColorSpace: {
        matrix: "smpte170m",
        fullRange: false,
        // TypeScript's DOM types don't list the wide-gamut and HDR tags yet.
        primaries: "smpte432" as VideoColorPrimaries,
        transfer: "hlg" as VideoTransferCharacteristics,
      },
      rotation: 90,
    });

    await syncVideoFrameToTarget(target, "clip", 3, payload);

    expect(frame.copyTo).toHaveBeenCalledWith(expect.any(Uint8Array));
    expect(target.setVideoFrameYuv).toHaveBeenCalledWith(
      "clip",
      3,
      4,
      2,
      "NV12",
      expect.any(Uint8Array),
      Uint32Array.from([0, 8]),
      Uint32Array.from([4, 4]),
      "smpte170m",
      true,
      90,
      2,
      4,
      "smpte432",
      "hlg",
    );
    expect(target.setVideoFrame).not.toHaveBeenCalled();
    expect(frame.close).toHaveBeenCalled();
  });

  it("leaves the matrix to the engine for untagged sources and takes the source's range", async () => {
    const target = fakeTarget();
    const payload = createVideoFramePayload(fakeFrame("I420P10", {}), 4, 2, {
      sourceColorSpace: { fullRange: true },
    });

    await syncVideoFrameToTarget(target, "clip", 0, payload);

    expect(target.setVideoFrameYuv.mock.calls[0]?.slice(4, 5)).toEqual(["I420P10"]);
    expect(target.setVideoFrameYuv.mock.calls[0]?.slice(8)).toEqual([
      undefined,
      true,
      0,
      4,
      2,
      undefined,
      undefined,
    ]);
  });

  it("copies RGB frames natively and swizzles them to RGBA", async () => {
    const target = fakeTarget();
    const frame = fakeFrame("BGRX", {});
    vi.mocked(frame.copyTo).mockImplementation(async (destination) => {
      (destination as Uint8Array).set([1, 2, 3, 0]);
      return [];
    });

    await syncVideoFrameToTarget(target, "clip", 0, createVideoFramePayload(frame, 1, 1));

    expect(target.setVideoFrameYuv).not.toHaveBeenCalled();
    expect(frame.copyTo).toHaveBeenCalledWith(expect.any(Uint8Array), {
      layout: [{ offset: 0, stride: 4 }],
    });
    expect(target.setVideoFrame).toHaveBeenCalledWith(
      "clip",
      0,
      1,
      1,
      Uint8Array.from([3, 2, 1, 255]),
    );
  });

  it("copies RGB frames tagged sRGB, BT.709 or BT.601 natively", async () => {
    for (const colorSpace of [
      { primaries: "bt709", transfer: "iec61966-2-1" },
      { primaries: "bt709", transfer: "bt709" },
      { primaries: "smpte170m", transfer: "smpte170m" },
    ] as const) {
      const target = fakeTarget();
      const frame = fakeFrame("RGBA", colorSpace);

      await syncVideoFrameToTarget(target, "clip", 0, createVideoFramePayload(frame, 1, 1));

      expect(frame.copyTo).toHaveBeenCalledWith(expect.any(Uint8Array), {
        layout: [{ offset: 0, stride: 4 }],
      });
    }
  });

  it("converts RGB frames in other primaries or transfers to sRGB on a canvas", async () => {
    const colorSpaces: VideoColorSpaceInit[] = [
      // TypeScript's DOM types don't list the wide-gamut and linear tags yet.
      { primaries: "smpte432" as VideoColorPrimaries, transfer: "iec61966-2-1" },
      { primaries: "bt709", transfer: "linear" as VideoTransferCharacteristics },
    ];
    for (const colorSpace of colorSpaces) {
      const { context, getContext } = stubCanvas([213, 115, 0, 255]);
      try {
        const target = fakeTarget();
        const frame = fakeFrame("BGRA", colorSpace);

        await syncVideoFrameToTarget(target, "clip", 0, createVideoFramePayload(frame, 1, 1));

        expect(frame.copyTo).not.toHaveBeenCalled();
        expect(getContext).toHaveBeenCalledWith("2d", { colorSpace: "srgb" });
        expect(context.drawImage).toHaveBeenCalledWith(frame, 0, 0, 1, 1);
        expect(context.getImageData).toHaveBeenCalledWith(0, 0, 1, 1, { colorSpace: "srgb" });
        expect(target.setVideoFrame).toHaveBeenCalledWith(
          "clip",
          0,
          1,
          1,
          Uint8Array.from([213, 115, 0, 255]),
        );
        expect(frame.close).toHaveBeenCalled();
      } finally {
        vi.unstubAllGlobals();
      }
    }
  });

  it("asks the browser for sRGB RGBA from frames the target can't take as planes", async () => {
    const target = { setVideoFrame: vi.fn() } as unknown as NativeVideoFrameTarget;
    const frame = fakeFrame("NV12", { primaries: "smpte432" as VideoColorPrimaries });

    await syncVideoFrameToTarget(target, "clip", 0, createVideoFramePayload(frame, 2, 1));

    expect(frame.copyTo).toHaveBeenCalledWith(expect.any(Uint8Array), {
      format: "RGBA",
      layout: [{ offset: 0, stride: 8 }],
      colorSpace: "srgb",
    });
  });
});

describe("decodeImageBitmapToRgba", () => {
  it("draws and reads the image back in sRGB, on either kind of canvas", async () => {
    for (const kind of ["offscreen", "document"] as const) {
      const { context, getContext } = stubCanvas([1, 2, 3, 4], kind);
      try {
        const bitmap = { width: 1, height: 1, close: vi.fn() } as unknown as ImageBitmap;

        const frame = await decodeImageBitmapToRgba(bitmap);

        expect(getContext).toHaveBeenCalledWith(
          "2d",
          kind === "document"
            ? { colorSpace: "srgb", willReadFrequently: true }
            : { colorSpace: "srgb" },
        );
        expect(context.drawImage).toHaveBeenCalledWith(bitmap, 0, 0);
        expect(context.getImageData).toHaveBeenCalledWith(0, 0, 1, 1, { colorSpace: "srgb" });
        expect(frame).toEqual({ width: 1, height: 1, rgba: Uint8Array.from([1, 2, 3, 4]) });
        expect(bitmap.close).toHaveBeenCalled();
      } finally {
        vi.unstubAllGlobals();
      }
    }
  });
});

describe("decodeImageBlob", () => {
  it("asks the browser to honour the image's colour profile", async () => {
    const createImageBitmap = vi.fn(async () => ({}));
    vi.stubGlobal("createImageBitmap", createImageBitmap);
    try {
      const blob = new Blob([]);
      await decodeImageBlob(blob);
      expect(createImageBitmap).toHaveBeenCalledWith(blob, { colorSpaceConversion: "default" });
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
