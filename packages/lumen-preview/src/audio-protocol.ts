// Types and literals shared by `audio-engine.ts`, `audio-worker.ts` and `audio-worklet.ts`. The
// worker and the worklet are served as standalone scripts, so they import only types from here and
// declare their own copies of the constants, typed against these.

export const LUMEN_AUDIO_SAMPLE_RATE = 48_000;
export const LUMEN_AUDIO_CHANNELS = 2;

/** An audio clip as one instance of its composition plays it, with times in samples. */
export interface AudioEngineVoice {
  duration: number;
  fadeIn: number;
  fadeOut: number;
  sourceId: string;
  sourceStart: number;
}

/**
 * Timeline samples `start + i` for `i` below `frames`, where a voice plays its clip at
 * `position + step * i` samples from the clip's start, sounding only between `from` and `to`, at a
 * gain moving linearly from `gain[0]` to `gain[1]`.
 */
export interface AudioEngineSegment {
  frames: number;
  from: number;
  gain: [number, number];
  position: number;
  start: number;
  step: number;
  to: number;
  voice: number;
}

/** What the engine planned for a composition's audio over its timeline, which the mixer loops. */
export interface AudioEngineTimeline {
  durationFrames: number;
  fps: number;
  segments: AudioEngineSegment[];
  voices: AudioEngineVoice[];
}

/** The part of the timeline that plays, `endMs` exclusive. It repeats when `loop` is set. */
export interface AudioPlaybackRange {
  endMs: number;
  loop: boolean;
  startMs: number;
}

/** Interleaved samples the worker mixed for `startSample` onwards, which the worklet plays. */
export type AudioChunkMessage = {
  channelCount: number;
  frames: number;
  generation: number;
  sampleRate: number;
  samples: Float32Array;
  startSample: number;
  type: "chunk";
};

/** From the engine to the worklet. */
export type WorkletMessage =
  | AudioChunkMessage
  | {
      generation: number;
      holdFrames: number;
      rate: number;
      rateRevision: number;
      startSample: number;
      type: "play";
    }
  | {
      type: "pause";
    }
  | {
      rate: number;
      rateRevision: number;
      type: "rate";
    };

/** From the worklet to the engine. */
export type WorkletEventMessage =
  | {
      message?: string;
      type: "error";
    }
  | {
      contextTime: number;
      currentSample: number;
      generation: number;
      rate: number;
      /** The rate change the worklet had heard when it reported. */
      rateRevision: number;
      stalled: boolean;
      type: "position";
    };

/** From the engine to the worker. */
export type AudioWorkerMessage =
  | {
      id: string;
      type: "remove-source";
    }
  | {
      id: string;
      samples: Float32Array;
      type: "set-source";
    }
  | {
      range: AudioPlaybackRange | null;
      type: "set-range";
    }
  | {
      timeline: AudioEngineTimeline | null;
      type: "set-timeline";
    }
  | {
      chunkFrames: number;
      generation: number;
      startSample: number;
      type: "request-chunk";
    };

/** From the worker to the engine. */
export type AudioWorkerEventMessage = AudioChunkMessage;
