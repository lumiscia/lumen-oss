import {
  ALL_FORMATS,
  Input,
  VideoSampleSink,
  type InputVideoTrack,
  type VideoSample,
} from "mediabunny";

import {
  closeVideoFrame,
  createUprightVideoFrame,
  createVideoFramePayload,
  isYuvVideoFrame,
} from "./canvas.js";
import { LumenMediaError, toMediaError } from "./errors.js";
import { createInputSource } from "./source.js";
import { createVideoTimeline, normalizeFps, type VideoTimeline } from "./timeline.js";
import type {
  LumenMediaBridgeOptions,
  MediaSourceInput,
  VideoFrameMetadata,
  VideoFramePayload,
  VideoSourceRegistration,
} from "./types.js";

/** Default byte budget for decoded video frames cached in the wasm media store. */
export const DEFAULT_VIDEO_FRAME_CACHE_CAPACITY = 512 * 1024 * 1024;
const MIN_SEQUENTIAL_FRAME_GAP = 24;
const SEQUENTIAL_FRAME_GAP_SECONDS = 2;
const VIDEO_SAMPLE_TIMESTAMP_EPSILON_SECONDS = 1e-10;

type VideoSetup = {
  colorSpace: VideoColorSpaceInit;
  input: Input;
  track: InputVideoTrack;
  sink: VideoSampleSink;
  timeline: VideoTimeline;
};

type PendingFrameRequest = {
  frame: number;
  timestamp: number;
};

type SequentialDecodeState = {
  currentSample: VideoSample | null;
  iterator: AsyncGenerator<VideoSample, void, unknown>;
  lastFrame: number;
  lastTimestamp: number;
  nextSample: VideoSample | null;
};

export class VideoSession {
  private disposed = false;
  private input: Input | null = null;
  private operationQueue = Promise.resolve();
  private sequentialDecodeState: SequentialDecodeState | null = null;
  private setupPromise: Promise<VideoSetup> | null = null;

  constructor(
    private readonly streamId: string,
    private readonly registration: VideoSourceRegistration,
    private readonly options: Required<LumenMediaBridgeOptions>,
  ) {}

  /**
   * Cancels reads straight away, because a decode holding the queue may be stalled on one. Its
   * samples are dropped once that decode has unwound.
   */
  dispose(): void {
    this.disposed = true;
    this.input?.dispose();
    void this.serialize(async () => this.resetSequentialDecodeState());
  }

  clearFrames(): void {
    // The wasm-side store owns uploaded frames, but the sequential decoder
    // keeps a couple of decoded samples alive to avoid restarting from the
    // previous keyframe on every playback tick.
    void this.serialize(async () => this.resetSequentialDecodeState());
  }

  async loadMetadata(): Promise<VideoFrameMetadata> {
    const setup = await this.ensureSetup();
    return setup.timeline.metadata;
  }

  /** The caller owns every returned frame. A failed decode closes what it had collected. */
  async decodeFrames(frames: Iterable<number>): Promise<Map<number, VideoFramePayload>> {
    return this.serialize(async () => {
      const results = new Map<number, VideoFramePayload>();
      if (this.disposed) {
        return results;
      }

      const setup = await this.ensureSetup();
      const pending: PendingFrameRequest[] = [];

      for (const frame of dedupeFrameNumbers(frames)) {
        if (frame < 0 || frame >= setup.timeline.metadata.frameCount) {
          continue;
        }

        pending.push({
          frame,
          timestamp: setup.timeline.timestampForFrame(frame),
        });
      }

      if (pending.length === 0) {
        return results;
      }

      try {
        if (this.shouldUseSequentialDecode(setup, pending)) {
          await this.decodeFramesSequential(setup, pending, results);
        } else {
          this.resetSequentialDecodeState();
          await this.decodeFramesRandomAccess(setup, pending, results);
        }
        return results;
      } catch (error) {
        for (const payload of results.values()) {
          payload.videoFrame.close();
        }
        throw error;
      }
    });
  }

  private ensureSetup(): Promise<VideoSetup> {
    this.setupPromise ??= this.createSetup().catch((error: unknown) => {
      this.setupPromise = null;
      throw error;
    });
    return this.setupPromise;
  }

  private async createSetup(): Promise<VideoSetup> {
    let input: Input | undefined;
    try {
      if (this.disposed) {
        throw new LumenMediaError(
          "source_not_registered",
          `video source "${this.streamId}" was removed`,
          { sourceId: this.streamId },
        );
      }
      input = new Input({
        formats: this.registration.formats ?? this.options.formats,
        source: createInputSource(this.registration.source, {
          blobCacheBytes: this.options.blobCacheBytes,
          urlCacheBytes: this.options.urlCacheBytes,
        }),
      });
      this.input = input;

      const track = await this.resolveTrack(input);
      if (!(await track.canDecode())) {
        throw new LumenMediaError(
          "track_not_decodable",
          `video source "${this.streamId}" cannot be decoded in this browser`,
          { sourceId: this.streamId },
        );
      }

      const timeline = await createVideoTimeline({
        fps: normalizeFps(this.registration.fps),
        mimeType: await input.getMimeType().catch(() => null),
        streamId: this.streamId,
        track,
      });

      return {
        colorSpace: await track.getColorSpace(),
        input,
        track,
        sink: new VideoSampleSink(track),
        timeline,
      };
    } catch (error) {
      input?.dispose();
      throw toMediaError(
        "unsupported_container",
        this.streamId,
        `video source "${this.streamId}" could not be opened`,
        error,
      );
    }
  }

  private async resolveTrack(input: Input): Promise<InputVideoTrack> {
    if (this.registration.track === undefined || this.registration.track === "primary") {
      const track = await input.getPrimaryVideoTrack();
      if (!track) {
        throw new LumenMediaError(
          "track_not_decodable",
          `video source "${this.streamId}" does not contain a video track`,
          { sourceId: this.streamId },
        );
      }
      return track;
    }

    const tracks = await input.getVideoTracks();
    const track = tracks.find((candidate) => candidate.id === this.registration.track);
    if (!track) {
      throw new LumenMediaError(
        "track_not_decodable",
        `video source "${this.streamId}" does not contain video track ${this.registration.track}`,
        { sourceId: this.streamId },
      );
    }

    return track;
  }

  private createPayloadFromSample(sample: VideoSample, setup: VideoSetup): VideoFramePayload {
    const { metadata } = setup.timeline;
    const width = Math.max(1, Math.round(sample.displayWidth ?? metadata.width));
    const height = Math.max(1, Math.round(sample.displayHeight ?? metadata.height));
    const retainedSample = sample.clone();
    try {
      const videoFrame = retainedSample.toVideoFrame();
      // The engine rotates YUV planes itself; other formats are turned upright here.
      if (isYuvVideoFrame(videoFrame)) {
        return createVideoFramePayload(videoFrame, width, height, {
          sourceColorSpace: setup.colorSpace,
          rotation: sample.rotation,
        });
      }
      if (sample.rotation !== 0) {
        closeVideoFrame(videoFrame);
        return createVideoFramePayload(
          createUprightVideoFrame(sample, width, height),
          width,
          height,
        );
      }
      return createVideoFramePayload(videoFrame, width, height, {
        sourceColorSpace: setup.colorSpace,
      });
    } finally {
      retainedSample.close();
    }
  }

  private async decodeFramesRandomAccess(
    setup: VideoSetup,
    pending: PendingFrameRequest[],
    results: Map<number, VideoFramePayload>,
  ): Promise<void> {
    let index = 0;
    let activeFrame: number | undefined;

    try {
      for await (const sample of setup.sink.samplesAtTimestamps(
        pending.map((request) => request.timestamp),
      )) {
        const request = pending[index];
        index += 1;
        if (!request) {
          sample?.close();
          continue;
        }
        activeFrame = request.frame;

        if (!sample) {
          throw new LumenMediaError(
            "frame_unavailable",
            `video frame ${request.frame} is unavailable for "${this.streamId}"`,
            { sourceId: this.streamId },
          );
        }

        try {
          results.set(request.frame, this.createPayloadFromSample(sample, setup));
        } finally {
          sample.close();
          activeFrame = undefined;
        }
      }
    } catch (error) {
      throw toMediaError(
        "decode_failed",
        this.streamId,
        activeFrame === undefined
          ? `video frames failed to decode for "${this.streamId}"`
          : `video frame ${activeFrame} failed to decode for "${this.streamId}"`,
        error,
      );
    }

    for (const request of pending.slice(index)) {
      if (!results.has(request.frame)) {
        throw new LumenMediaError(
          "frame_unavailable",
          `video frame ${request.frame} is unavailable for "${this.streamId}"`,
          { sourceId: this.streamId },
        );
      }
    }
  }

  private async decodeFramesSequential(
    setup: VideoSetup,
    pending: PendingFrameRequest[],
    results: Map<number, VideoFramePayload>,
  ): Promise<void> {
    let activeFrame: number | undefined;

    try {
      const state = await this.ensureSequentialDecodeState(setup, pending[0]?.timestamp ?? 0);
      for (const request of pending) {
        activeFrame = request.frame;
        const sample = await this.readSequentialSample(state, request.timestamp);
        if (!sample) {
          throw new LumenMediaError(
            "frame_unavailable",
            `video frame ${request.frame} is unavailable for "${this.streamId}"`,
            { sourceId: this.streamId },
          );
        }

        results.set(request.frame, this.createPayloadFromSample(sample, setup));
        state.lastFrame = request.frame;
        state.lastTimestamp = request.timestamp;
        activeFrame = undefined;
      }
    } catch (error) {
      this.resetSequentialDecodeState();
      throw toMediaError(
        "decode_failed",
        this.streamId,
        activeFrame === undefined
          ? `video frames failed to decode for "${this.streamId}"`
          : `video frame ${activeFrame} failed to decode for "${this.streamId}"`,
        error,
      );
    }
  }

  private async ensureSequentialDecodeState(
    setup: VideoSetup,
    startTimestamp: number,
  ): Promise<SequentialDecodeState> {
    if (this.sequentialDecodeState) {
      return this.sequentialDecodeState;
    }

    const iterator = setup.sink.samples(startTimestamp);
    let currentSample: VideoSample | null = null;
    try {
      currentSample = await this.readNextSequentialSample(iterator);
      const nextSample = await this.readNextSequentialSample(iterator);
      const state: SequentialDecodeState = {
        currentSample,
        iterator,
        lastFrame: -1,
        lastTimestamp: -Infinity,
        nextSample,
      };

      this.sequentialDecodeState = state;
      return state;
    } catch (error) {
      currentSample?.close();
      void iterator.return(undefined);
      throw error;
    }
  }

  private async readNextSequentialSample(
    iterator: AsyncGenerator<VideoSample, void, unknown>,
  ): Promise<VideoSample | null> {
    const result = await iterator.next();
    return result.done ? null : result.value;
  }

  private async readSequentialSample(
    state: SequentialDecodeState,
    targetTimestamp: number,
  ): Promise<VideoSample | null> {
    let currentSample = state.currentSample;
    if (!currentSample) {
      return null;
    }

    if (currentSample.timestamp - targetTimestamp > VIDEO_SAMPLE_TIMESTAMP_EPSILON_SECONDS) {
      return null;
    }

    while (
      state.nextSample &&
      state.nextSample.timestamp <= targetTimestamp + VIDEO_SAMPLE_TIMESTAMP_EPSILON_SECONDS
    ) {
      currentSample.close();
      state.currentSample = state.nextSample;
      state.nextSample = await this.readNextSequentialSample(state.iterator);
      currentSample = state.currentSample;
      if (!currentSample) {
        return null;
      }
    }

    return currentSample;
  }

  private resetSequentialDecodeState(): void {
    const state = this.sequentialDecodeState;
    this.sequentialDecodeState = null;
    if (!state) {
      return;
    }

    state.currentSample?.close();
    state.nextSample?.close();
    void state.iterator.return(undefined);
  }

  private shouldUseSequentialDecode(setup: VideoSetup, pending: PendingFrameRequest[]): boolean {
    const firstRequest = pending[0];
    if (!firstRequest) {
      return false;
    }

    const state = this.sequentialDecodeState;
    if (!state) {
      return true;
    }

    if (firstRequest.timestamp + VIDEO_SAMPLE_TIMESTAMP_EPSILON_SECONDS < state.lastTimestamp) {
      return false;
    }

    if (firstRequest.frame <= state.lastFrame) {
      return false;
    }

    const maxGapFrames = Math.max(
      MIN_SEQUENTIAL_FRAME_GAP,
      Math.ceil(setup.timeline.metadata.fps * SEQUENTIAL_FRAME_GAP_SECONDS),
    );
    return firstRequest.frame - state.lastFrame <= maxGapFrames;
  }

  private serialize<T>(operation: () => Promise<T>): Promise<T> {
    const next = this.operationQueue.then(operation, operation);
    this.operationQueue = next.then(
      () => undefined,
      () => undefined,
    );
    return next;
  }
}

export function createVideoRegistration(
  source: MediaSourceInput,
  options: Omit<VideoSourceRegistration, "source"> = {},
): VideoSourceRegistration {
  return {
    ...options,
    formats: options.formats ?? ALL_FORMATS,
    source,
  };
}

export function dedupeFrameNumbers(frames: Iterable<number>): number[] {
  return [...new Set(frames)].sort((left, right) => left - right);
}
