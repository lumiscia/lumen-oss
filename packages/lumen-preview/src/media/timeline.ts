import type { InputVideoTrack } from "mediabunny";

import { LumenMediaError } from "./errors.js";
import type { VideoFrameMetadata } from "./types.js";

const VIDEO_FRAME_TIME_EPSILON_SECONDS = 0.001;
const FRAME_RATE_PACKET_SAMPLE = 100;

export function normalizeFps(value: number | null | undefined): number | null {
  if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) {
    return null;
  }

  return value;
}

export function estimateVideoFrameCount(duration: number, fps: number): number {
  return Math.max(1, Math.round(duration * fps));
}

export interface VideoTimeline {
  metadata: VideoFrameMetadata;
  timestampForFrame(frame: number): number;
}

export interface CreateVideoTimelineOptions {
  fps: number | null;
  mimeType: string | null;
  streamId: string;
  track: InputVideoTrack;
}

/** Numbers source frames in fixed time steps, as native decoding buckets timestamps. */
export async function createVideoTimeline({
  fps: requestedFps,
  mimeType,
  streamId,
  track,
}: CreateVideoTimelineOptions): Promise<VideoTimeline> {
  const fps = requestedFps ?? (await streamFrameRate(track));
  if (!fps) {
    throw new LumenMediaError(
      "decode_failed",
      `video source "${streamId}": unable to determine stream frame rate`,
    );
  }

  const duration = await track.computeDuration();
  if (!Number.isFinite(duration) || duration <= 0) {
    throw new LumenMediaError("decode_failed", "video track has an invalid duration");
  }

  const firstTimestamp = await track.getFirstTimestamp();
  const frameCount = estimateVideoFrameCount(duration, fps);
  const playbackStart = Math.max(0, firstTimestamp);
  const latestTimestamp = Math.max(playbackStart, duration - VIDEO_FRAME_TIME_EPSILON_SECONDS);

  return {
    metadata: {
      width: Math.max(1, Math.round(track.displayWidth)),
      height: Math.max(1, Math.round(track.displayHeight)),
      duration,
      fps,
      frameCount,
      firstTimestamp,
      mimeType,
      trackId: track.id,
      trackNumber: track.number,
      codec: await track.getCodecParameterString(),
    },
    timestampForFrame(frame: number) {
      assertFrameInRange(frame, frameCount);
      return Math.min(Math.max(frame / fps, playbackStart), latestTimestamp);
    },
  };
}

async function streamFrameRate(track: InputVideoTrack): Promise<number | null> {
  const stats = await track.computePacketStats(FRAME_RATE_PACKET_SAMPLE);
  return normalizeFps(stats.averagePacketRate);
}

function assertFrameInRange(frame: number, frameCount: number): void {
  if (!Number.isInteger(frame) || frame < 0 || frame >= frameCount) {
    throw new LumenMediaError("frame_unavailable", "video frame index out of range");
  }
}
