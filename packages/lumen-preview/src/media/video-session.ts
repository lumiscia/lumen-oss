import {
  ALL_FORMATS,
  Input,
  VideoSampleSink,
  type InputVideoTrack,
  type Source,
  type VideoSample,
} from "mediabunny";

import { createVideoFramePayload } from "./canvas.js";
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

export const DEFAULT_VIDEO_FRAME_CACHE_CAPACITY = 96;
const MIN_SEQUENTIAL_FRAME_GAP = 24;
const SEQUENTIAL_FRAME_GAP_SECONDS = 2;
const VIDEO_SAMPLE_TIMESTAMP_EPSILON_SECONDS = 1e-10;

type VideoSetup = {
  input: Input;
  inputOwned: boolean;
  source: Source;
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
  private operationQueue = Promise.resolve();
  private sequentialDecodeState: SequentialDecodeState | null = null;
  private setupPromise: Promise<VideoSetup> | null = null;

  constructor(
    private readonly streamId: string,
    private readonly registration: VideoSourceRegistration,
    private readonly options: Required<LumenMediaBridgeOptions>,
  ) {}

  dispose(): void {
    this.resetSequentialDecodeState();
    if (this.setupPromise) {
      void this.setupPromise.then(
        (setup) => {
          if (setup.inputOwned) {
            setup.input.dispose();
          }
        },
        () => undefined,
      );
    }
  }

  clearFrames(): void {
    // The wasm-side store owns uploaded frames, but the sequential decoder
    // keeps a couple of decoded samples alive to avoid restarting from the
    // previous keyframe on every playback tick.
    this.resetSequentialDecodeState();
  }

  async loadMetadata(): Promise<VideoFrameMetadata> {
    const setup = await this.ensureSetup();
    return setup.timeline.metadata;
  }

  async decodeFrames(frames: Iterable<number>): Promise<Map<number, VideoFramePayload>> {
    return this.serialize(async () => {
      const setup = await this.ensureSetup();
      const results = new Map<number, VideoFramePayload>();
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

      if (!this.shouldUseSequentialDecode(setup, pending)) {
        this.resetSequentialDecodeState();
        return this.decodeFramesRandomAccess(setup, pending);
      }

      return this.decodeFramesSequential(setup, pending);
    });
  }

  private async ensureSetup(): Promise<VideoSetup> {
    this.setupPromise ??= this.createSetup();
    return this.setupPromise;
  }

  private async createSetup(): Promise<VideoSetup> {
    try {
      const sourceResult = createInputSource(this.registration.source, {
        blobCacheBytes: this.options.blobCacheBytes,
        urlCacheBytes: this.options.urlCacheBytes,
      });
      const input = new Input({
        formats: this.registration.formats ?? this.options.formats,
        source: sourceResult.source,
      });

      const track = await this.resolveTrack(input);
      if (!(await track.canDecode())) {
        throw new LumenMediaError(
          "track_not_decodable",
          `video source "${this.streamId}" cannot be decoded in this browser`,
        );
      }

      const mimeType = await input.getMimeType().catch(() => null);
      const timeline = await createVideoTimeline({
        fps: normalizeFps(this.registration.fps),
        input,
        mimeType,
        ...(this.registration.timelineMode !== undefined
          ? { mode: this.registration.timelineMode }
          : {}),
        track,
      });

      return {
        input,
        inputOwned: sourceResult.owned,
        source: sourceResult.source,
        track,
        sink: new VideoSampleSink(track),
        timeline,
      };
    } catch (error) {
      throw toMediaError(
        "unsupported_container",
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
      );
    }

    return track;
  }

  private createPayloadFromSample(
    sample: VideoSample,
    metadata: VideoFrameMetadata,
  ): VideoFramePayload {
    const retainedSample = sample.clone();
    try {
      const width = Math.max(1, Math.round(sample.displayWidth ?? metadata.width));
      const height = Math.max(1, Math.round(sample.displayHeight ?? metadata.height));
      return createVideoFramePayload(retainedSample.toVideoFrame(), width, height);
    } finally {
      retainedSample.close();
    }
  }

  private async decodeFramesRandomAccess(
    setup: VideoSetup,
    pending: PendingFrameRequest[],
  ): Promise<Map<number, VideoFramePayload>> {
    const results = new Map<number, VideoFramePayload>();
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
          );
        }

        try {
          results.set(request.frame, this.createPayloadFromSample(sample, setup.timeline.metadata));
        } finally {
          sample.close();
          activeFrame = undefined;
        }
      }
    } catch (error) {
      throw toMediaError(
        "decode_failed",
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
        );
      }
    }

    return results;
  }

  private async decodeFramesSequential(
    setup: VideoSetup,
    pending: PendingFrameRequest[],
  ): Promise<Map<number, VideoFramePayload>> {
    const results = new Map<number, VideoFramePayload>();
    const state = await this.ensureSequentialDecodeState(setup, pending[0]?.timestamp ?? 0);
    let activeFrame: number | undefined;

    try {
      for (const request of pending) {
        activeFrame = request.frame;
        const sample = await this.readSequentialSample(state, request.timestamp);
        if (!sample) {
          throw new LumenMediaError(
            "frame_unavailable",
            `video frame ${request.frame} is unavailable for "${this.streamId}"`,
          );
        }

        results.set(request.frame, this.createPayloadFromSample(sample, setup.timeline.metadata));
        state.lastFrame = request.frame;
        state.lastTimestamp = request.timestamp;
        activeFrame = undefined;
      }
    } catch (error) {
      this.resetSequentialDecodeState();
      throw toMediaError(
        "decode_failed",
        activeFrame === undefined
          ? `video frames failed to decode for "${this.streamId}"`
          : `video frame ${activeFrame} failed to decode for "${this.streamId}"`,
        error,
      );
    }

    return results;
  }

  private async ensureSequentialDecodeState(
    setup: VideoSetup,
    startTimestamp: number,
  ): Promise<SequentialDecodeState> {
    if (this.sequentialDecodeState) {
      return this.sequentialDecodeState;
    }

    const iterator = setup.sink.samples(startTimestamp);
    const currentSample = await this.readNextSequentialSample(iterator);
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
