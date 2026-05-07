import { EncodedPacketSink, type Input, type InputVideoTrack } from "mediabunny";

import { LumenMediaError } from "./errors.js";
import type { VideoFrameMetadata, VideoTimelineMode } from "./types.js";

const VIDEO_FRAME_TIME_EPSILON_SECONDS = 0.001;

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
  input: Input;
  mimeType: string | null;
  mode?: VideoTimelineMode;
  track: InputVideoTrack;
}

export async function createVideoTimeline({
  fps: requestedFps,
  input,
  mimeType,
  mode,
  track,
}: CreateVideoTimelineOptions): Promise<VideoTimeline> {
  const stats = await track.computePacketStats(100);
  const nativeFps = normalizeFps(stats.averagePacketRate);
  const timelineMode: VideoTimelineMode = mode ?? (requestedFps ? "fixed-fps" : "native-samples");
  const duration = await track.computeDuration();
  const firstTimestamp = await track.getFirstTimestamp();

  if (!Number.isFinite(duration) || duration <= 0) {
    throw new LumenMediaError("decode_failed", "video track has an invalid duration");
  }

  const width = Math.max(1, Math.round(track.displayWidth));
  const height = Math.max(1, Math.round(track.displayHeight));
  const codec = await track.getCodecParameterString();

  if (timelineMode === "fixed-fps") {
    const fps = normalizeFps(requestedFps) ?? nativeFps;
    if (!fps) {
      throw new LumenMediaError("decode_failed", "video source requires a positive frame rate");
    }

    const frameCount = estimateVideoFrameCount(duration, fps);
    const playbackStart = Math.max(0, firstTimestamp);
    const latestTimestamp = Math.max(playbackStart, duration - VIDEO_FRAME_TIME_EPSILON_SECONDS);

    return {
      metadata: {
        width,
        height,
        duration,
        fps,
        frameCount,
        firstTimestamp,
        mimeType,
        trackId: track.id,
        trackNumber: track.number,
        codec,
        timelineMode,
      },
      timestampForFrame(frame: number) {
        assertFrameInRange(frame, frameCount);
        return Math.min(Math.max(frame / fps, playbackStart), latestTimestamp);
      },
    };
  }

  const timestamps = await readPresentationTimestamps(track);
  if (timestamps.length === 0) {
    throw new LumenMediaError("frame_unavailable", "video track does not contain frames");
  }

  const filteredTimestamps = timestamps.filter((timestamp) => timestamp >= 0);
  const nativeTimestamps = filteredTimestamps.length > 0 ? filteredTimestamps : timestamps;
  const fps = nativeFps ?? estimateNativeFps(nativeTimestamps, duration);

  return {
    metadata: {
      width,
      height,
      duration,
      fps,
      frameCount: nativeTimestamps.length,
      firstTimestamp,
      mimeType: await input.getMimeType().catch(() => mimeType),
      trackId: track.id,
      trackNumber: track.number,
      codec,
      timelineMode,
    },
    timestampForFrame(frame: number) {
      assertFrameInRange(frame, nativeTimestamps.length);
      const timestamp = nativeTimestamps[frame];
      if (timestamp === undefined) {
        throw new LumenMediaError("frame_unavailable", `video frame ${frame} is unavailable`);
      }
      return timestamp;
    },
  };
}

async function readPresentationTimestamps(track: InputVideoTrack): Promise<number[]> {
  const sink = new EncodedPacketSink(track);
  const timestamps: Array<{ sequenceNumber: number; timestamp: number }> = [];

  for await (const packet of sink.packets(undefined, undefined, { metadataOnly: true })) {
    timestamps.push({
      sequenceNumber: packet.sequenceNumber,
      timestamp: packet.timestamp,
    });
  }

  return timestamps
    .sort((left, right) => {
      const timestampOrder = left.timestamp - right.timestamp;
      return timestampOrder === 0 ? left.sequenceNumber - right.sequenceNumber : timestampOrder;
    })
    .map((packet) => packet.timestamp);
}

function estimateNativeFps(timestamps: number[], duration: number): number {
  if (timestamps.length <= 1) {
    return 1;
  }

  const first = timestamps[0] ?? 0;
  const last = timestamps.at(-1) ?? duration;
  const span = Math.max(last - first, duration, Number.EPSILON);
  return Math.max(1, timestamps.length / span);
}

function assertFrameInRange(frame: number, frameCount: number): void {
  if (!Number.isInteger(frame) || frame < 0 || frame >= frameCount) {
    throw new LumenMediaError("frame_unavailable", "video frame index out of range");
  }
}
