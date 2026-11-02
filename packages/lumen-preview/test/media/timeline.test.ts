import type { InputVideoTrack } from "mediabunny";
import { describe, expect, it, vi } from "vitest";

import { createVideoTimeline } from "../../src/media/timeline.js";

function fakeTrack(averagePacketRate: number) {
  const computePacketStats = vi.fn(async () => ({
    averageBitrate: 0,
    averagePacketRate,
    packetCount: 0,
  }));
  const track = {
    computeDuration: async () => 2,
    computePacketStats,
    displayHeight: 90,
    displayWidth: 160,
    getCodecParameterString: async () => "avc1",
    getFirstTimestamp: async () => 0,
    id: 1,
    number: 1,
  } as unknown as InputVideoTrack;
  return { computePacketStats, track };
}

const options = { mimeType: "video/mp4", streamId: "clip" };

describe("createVideoTimeline", () => {
  it("numbers frames at the stream's rate when no fps is requested", async () => {
    const { track } = fakeTrack(25);
    const timeline = await createVideoTimeline({ ...options, fps: null, track });

    expect(timeline.metadata).toMatchObject({ fps: 25, frameCount: 50 });
    expect(timeline.timestampForFrame(5)).toBeCloseTo(0.2);
  });

  it("uses a requested fps without reading any packets", async () => {
    const { computePacketStats, track } = fakeTrack(25);
    const timeline = await createVideoTimeline({ ...options, fps: 10, track });

    expect(timeline.metadata).toMatchObject({ fps: 10, frameCount: 20 });
    expect(timeline.timestampForFrame(5)).toBeCloseTo(0.5);
    expect(computePacketStats).not.toHaveBeenCalled();
  });

  it.each([0, Number.POSITIVE_INFINITY, Number.NaN])(
    "fails rather than guessing when the stream's rate is %s",
    async (rate) => {
      const { track } = fakeTrack(rate);
      await expect(createVideoTimeline({ ...options, fps: null, track })).rejects.toThrow(
        'video source "clip": unable to determine stream frame rate',
      );
    },
  );
});
