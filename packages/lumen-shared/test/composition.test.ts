import { describe, expect, test } from "vitest";

import { AudioTrack } from "../src/audio.js";
import { Composition } from "../src/composition.js";

describe("Composition audio helpers", () => {
  test("adds audio tracks with their clips using canonical JSON fields", () => {
    const composition = new Composition();
    const track = new AudioTrack({
      id: "music",
      name: "Music",
      volume: 0.5,
    });
    const clip = track.addClip({
      durationSeconds: 2.5,
      name: "Intro",
      sourceId: "audio:intro",
      sourceStartSeconds: 1.25,
      startSeconds: 0.5,
      volume: 0.8,
    });

    composition.addAudioTrack(track);

    expect(track.toJSON()).toEqual({
      id: "music",
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
      track_id: "music",
      volume: 0.8,
    });
    expect(composition.toJSON().audio).toEqual({
      clips: [clip],
      tracks: [track.toJSON()],
    });
  });

  test("keeps canonical clip input intact when it belongs to the track", () => {
    const composition = new Composition();
    const track = new AudioTrack({
      id: "voice",
      muted: true,
      name: "Voice",
    });
    const clip = track.addClip({
      duration_ms: 1_000,
      id: "voice-over",
      source_id: "audio:voice",
      start_ms: 0,
      track_id: track.id,
    });

    composition.addAudioTrack(track);

    expect(clip).toEqual({
      duration_ms: 1_000,
      id: "voice-over",
      source_id: "audio:voice",
      start_ms: 0,
      track_id: "voice",
    });
    expect(composition.toJSON().audio?.clips).toEqual([clip]);
  });

  test("rejects overlapping clips on the same audio track", () => {
    const track = new AudioTrack({ id: "music" });
    track.addClip({
      durationMs: 1_000,
      id: "intro",
      sourceId: "audio:intro",
      startMs: 0,
    });

    expect(() =>
      track.addClip({
        durationMs: 500,
        id: "overlap",
        sourceId: "audio:overlap",
        startMs: 999,
      }),
    ).toThrow("overlaps");
  });

  test("allows adjacent clips on the same audio track", () => {
    const track = new AudioTrack({ id: "music" });
    track.addClip({
      durationMs: 1_000,
      id: "intro",
      sourceId: "audio:intro",
      startMs: 0,
    });
    const next = track.addClip({
      durationMs: 500,
      id: "next",
      sourceId: "audio:next",
      startMs: 1_000,
    });

    expect(next.start_ms).toBe(1_000);
  });

  test("rejects frame-based audio clip timing on AudioTrack", () => {
    const track = new AudioTrack({ id: "music" });

    expect(() =>
      track.addClip({
        duration_frames: 24,
        id: "frame-based",
        source_id: "audio:music",
        start_frame: 0,
        track_id: "music",
      }),
    ).toThrow("frame-based timing");
  });
});
