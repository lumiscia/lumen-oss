import { describe, expect, it, vi } from "vitest";

import { createVideoFramePayload, syncVideoFrameToTarget } from "../../src/media/canvas.js";
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
      sourceColorSpace: { matrix: "smpte170m", fullRange: false },
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
    expect(target.setVideoFrameYuv.mock.calls[0]?.slice(8)).toEqual([undefined, true, 0, 4, 2]);
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
});
