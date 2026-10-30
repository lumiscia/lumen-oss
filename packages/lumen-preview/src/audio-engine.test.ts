import { afterEach, describe, expect, it, vi } from "vitest";

import {
  audioTimelineFromCompositionJson,
  LUMEN_AUDIO_CHANNELS,
  LUMEN_AUDIO_CLIP_EDGE_FADE_SAMPLES,
  LUMEN_AUDIO_SAMPLE_RATE,
  LumenAudioEngine,
  createLumenAudioSchedule,
  msToLumenAudioSample,
  type AudioSourceRegistration,
} from "./audio-engine.js";

describe("createLumenAudioSchedule", () => {
  it("uses Lumen sample timing for offsets, durations, and fades", () => {
    const [scheduled] = createLumenAudioSchedule(
      {
        durationFrames: 120,
        fps: 24,
        tracks: [{ id: "music", muted: false, solo: false, volume: 0.5 }],
        clips: [
          {
            durationMs: 1_000,
            id: "clip",
            sourceId: "song",
            sourceStartMs: 250,
            startMs: 500,
            trackId: "music",
            volume: 0.25,
          },
        ],
      },
      750,
    );

    expect(scheduled).toMatchObject({
      delaySeconds: 0,
      durationSeconds: 0.75,
      fadeSeconds: LUMEN_AUDIO_CLIP_EDGE_FADE_SAMPLES / 48_000,
      gain: 0.125,
      sourceOffsetSeconds: 0.5,
    });
  });

  it("matches engine solo and mute filtering", () => {
    const scheduled = createLumenAudioSchedule(
      {
        durationFrames: 1,
        fps: 24,
        tracks: [
          { id: "solo", muted: false, solo: true, volume: 1 },
          { id: "muted", muted: true, solo: false, volume: 1 },
          { id: "other", muted: false, solo: false, volume: 1 },
        ],
        clips: [clip("a", "solo"), clip("b", "muted"), clip("c", "other")],
      },
      0,
    );

    expect(scheduled.map((entry) => entry.clip.id)).toEqual(["a"]);
  });
});

describe("msToLumenAudioSample", () => {
  it("uses the engine 48 kHz floor conversion", () => {
    expect(msToLumenAudioSample(1_000)).toBe(48_000);
    expect(msToLumenAudioSample(1.5)).toBe(72);
  });
});

describe("audioTimelineFromCompositionJson", () => {
  it("maps the canonical composition audio contract into the preview engine timeline", () => {
    const timeline = audioTimelineFromCompositionJson(
      JSON.stringify({
        timeline: { fps: 24, duration_frames: 120 },
        audio: {
          tracks: [{ id: "music", muted: true, volume: 0.75 }],
          clips: [
            {
              id: "intro",
              source_id: "song",
              track_id: "music",
              start_frame: 12,
              duration_ms: 1_500,
              source_start_seconds: 0.25,
              volume: 0.5,
            },
          ],
        },
      }),
    );

    expect(timeline).toEqual({
      durationFrames: 120,
      fps: 24,
      tracks: [{ id: "music", muted: true, solo: false, volume: 0.75 }],
      clips: [
        {
          durationMs: 1_500,
          id: "intro",
          sourceId: "song",
          sourceStartMs: 250,
          startMs: 500,
          trackId: "music",
          volume: 0.5,
        },
      ],
    });
  });
});

describe("LumenAudioEngine", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("plays sources without decodable audio as silence and keeps mixing other clips", async () => {
    const posted: Array<{ type: string; samples?: Float32Array; startSample?: number }> = [];
    const decodeAudioData = vi.fn(async (bytes: ArrayBuffer) => {
      if (new Uint8Array(bytes)[0] === 0) {
        throw new DOMException("Unable to decode audio data", "EncodingError");
      }
      const channel = new Float32Array(LUMEN_AUDIO_SAMPLE_RATE).fill(0.5);
      return {
        length: channel.length,
        numberOfChannels: 1,
        sampleRate: LUMEN_AUDIO_SAMPLE_RATE,
        getChannelData: () => channel,
      };
    });
    vi.stubGlobal(
      "AudioContext",
      class {
        audioWorklet = { addModule: async () => {} };
        currentTime = 0;
        decodeAudioData = decodeAudioData;
        destination = {};
        resume = async () => {};
      },
    );
    vi.stubGlobal(
      "AudioWorkletNode",
      class {
        port = { postMessage: (message: (typeof posted)[number]) => posted.push(message) };
        connect() {}
      },
    );
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});

    const engine = new LumenAudioEngine({ workerModuleUrl: "", workletModuleUrl: "worklet.js" });
    engine.setAudioTimeline({
      durationFrames: 24,
      fps: 24,
      tracks: [{ id: "track", muted: false, solo: false, volume: 1 }],
      clips: [clip("muted-video", "track"), clip("music", "track")],
    });
    const sources: AudioSourceRegistration[] = [
      { id: "muted-video", kind: "audio", source: new Uint8Array([0]) },
      { id: "music", kind: "audio", source: new Uint8Array([1]) },
    ];
    await Promise.all([engine.syncAudioSources(sources), engine.syncAudioSources(sources)]);
    await engine.syncAudioSources(sources);

    expect(decodeAudioData).toHaveBeenCalledTimes(2);
    expect(warn).toHaveBeenCalledOnce();
    const chunk = posted
      .filter((message) => message.type === "chunk" && message.startSample === 0)
      .at(-1);
    expect(chunk?.samples?.[LUMEN_AUDIO_CLIP_EDGE_FADE_SAMPLES * LUMEN_AUDIO_CHANNELS]).toBe(0.5);

    engine.play(0);
    await vi.waitFor(() => expect(posted.at(-1)).toMatchObject({ type: "play" }));
  });
});

function clip(id: string, trackId: string) {
  return {
    durationMs: 1_000,
    id,
    sourceId: id,
    sourceStartMs: 0,
    startMs: 0,
    trackId,
    volume: 1,
  };
}
