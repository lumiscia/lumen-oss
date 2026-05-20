import { describe, expect, test } from "vitest";

import { Composition } from "../src/composition.js";

describe("Composition audio helpers", () => {
  test("creates audio tracks and clips with canonical JSON fields", () => {
    const composition = new Composition();
    const track = composition.addAudioTrack({
      name: "Music",
      volume: 0.5,
    });
    const clip = composition.addAudioClip(track, {
      durationSeconds: 2.5,
      name: "Intro",
      sourceId: "audio:intro",
      sourceStartSeconds: 1.25,
      startSeconds: 0.5,
      volume: 0.8,
    });

    expect(track).toEqual({
      id: "audio-track-1",
      name: "Music",
      volume: 0.5,
    });
    expect(clip).toEqual({
      duration_ms: 2_500,
      id: "audio-clip-1",
      name: "Intro",
      source_id: "audio:intro",
      source_start_ms: 1_250,
      start_ms: 500,
      track_id: "audio-track-1",
      volume: 0.8,
    });
    expect(composition.toJSON().audio).toEqual({
      clips: [clip],
      tracks: [track],
    });
  });

  test("uses explicit audio IDs and keeps canonical clip input intact", () => {
    const composition = new Composition();
    const track = composition.addAudioTrack("voice", {
      muted: true,
      name: "Voice",
    });
    const clip = composition.addAudioClip({
      duration_ms: 1_000,
      id: "voice-over",
      source_id: "audio:voice",
      start_ms: 0,
      track_id: track.id,
    });

    expect(track).toEqual({
      id: "voice",
      muted: true,
      name: "Voice",
    });
    expect(clip).toEqual({
      duration_ms: 1_000,
      id: "voice-over",
      source_id: "audio:voice",
      start_ms: 0,
      track_id: "voice",
    });
    expect(composition.toJSON().audio?.clips).toEqual([clip]);
  });

  test("normalizes frame-based audio clip options to milliseconds", () => {
    const composition = new Composition({
      timeline: {
        fps: 50,
      },
    });
    const track = composition.addAudioTrack("music");
    const clip = composition.addAudioClip(track, {
      durationFrames: 25,
      sourceId: "audio:music",
      startFrame: 10,
    });

    expect(clip).toEqual({
      duration_ms: 500,
      id: "audio-clip-1",
      source_id: "audio:music",
      start_ms: 200,
      track_id: "music",
    });
  });
});
