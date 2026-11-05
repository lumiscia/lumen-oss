import { describe, expect, it, vi } from "vitest";

import { createVideoFramePayload, syncVideoFrameToTarget } from "../../src/media/canvas.js";
import type { NativeVideoFrameTarget } from "../../src/media/types.js";

function fakeFrame(format: string, colorSpace: VideoColorSpaceInit) {
  return {
    format,
    colorSpace,
    visibleRect: { width: 2, height: 2 },
    allocationSize: () => 6,
    copyTo: vi.fn(async () => [
      { offset: 0, stride: 2 },
      { offset: 4, stride: 2 },
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
  it("converts YUV with the source's matrix and the copied samples' range", async () => {
    const target = fakeTarget();
    // Safari reports its own BT.709 full-range output on the frame, whatever the source says.
    const frame = fakeFrame("NV12", { matrix: "bt709", fullRange: true });
    const payload = createVideoFramePayload(frame, 2, 2, { matrix: "smpte170m", fullRange: false });

    await syncVideoFrameToTarget(target, "clip", 3, payload);

    expect(target.setVideoFrameYuv).toHaveBeenCalledWith(
      "clip",
      3,
      2,
      2,
      "NV12",
      expect.any(Uint8Array),
      Uint32Array.from([0, 4]),
      Uint32Array.from([2, 2]),
      "smpte170m",
      true,
    );
    expect(target.setVideoFrame).not.toHaveBeenCalled();
    expect(frame.close).toHaveBeenCalled();
  });

  it("leaves the matrix to the engine for untagged sources", async () => {
    const target = fakeTarget();
    const payload = createVideoFramePayload(fakeFrame("I420", {}), 2, 2, { fullRange: false });

    await syncVideoFrameToTarget(target, "clip", 0, payload);

    expect(target.setVideoFrameYuv.mock.calls[0]?.slice(8)).toEqual([undefined, false]);
  });

  it("copies RGB frames as RGBA", async () => {
    const target = fakeTarget();
    const frame = fakeFrame("BGRA", {});
    vi.mocked(frame.copyTo).mockImplementation(async (destination) => {
      (destination as Uint8Array).set([1, 2, 3, 4]);
      return [];
    });

    await syncVideoFrameToTarget(target, "clip", 0, createVideoFramePayload(frame, 1, 1));

    expect(target.setVideoFrameYuv).not.toHaveBeenCalled();
    expect(target.setVideoFrame).toHaveBeenCalledWith(
      "clip",
      0,
      1,
      1,
      Uint8Array.from([3, 2, 1, 4]),
    );
  });
});
