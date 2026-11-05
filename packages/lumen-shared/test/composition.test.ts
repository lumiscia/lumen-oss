import { describe, expect, test } from "vitest";

import { Composition } from "../src/composition.js";
import { frame, hex, progress, uppercase, vec2 } from "../src/expression.js";
import { defineComposition } from "../src/template.js";

describe("Composition node IDs", () => {
  test("skips explicit IDs when allocating automatic IDs", () => {
    const composition = new Composition();

    const explicit = composition.addNode({ id: 1, type: "media_output" });
    const firstAutomatic = composition.addNode({ type: "media_output" });
    const secondAutomatic = composition.addNode({ type: "media_output" });

    expect([explicit.id, firstAutomatic.id, secondAutomatic.id]).toEqual([1, 0, 2]);
  });

  test.each([
    [
      "an explicit ID",
      (composition: Composition) => composition.addNode({ id: 4, type: "media_output" }).id,
    ],
    [
      "an automatic ID",
      (composition: Composition) => composition.addNode({ type: "media_output" }).id,
    ],
  ])("rejects an explicit ID already taken by %s", (_taken, addFirst) => {
    const composition = new Composition();
    const id = addFirst(composition);

    expect(() => composition.addNode({ id, type: "media_output" })).toThrow(
      `Node \`${id}\` already exists.`,
    );
    expect(composition.toJSON().nodes).toHaveLength(1);
  });
});

describe("Composition connections", () => {
  test("connects to the named target port, by node or by id", () => {
    const composition = new Composition();
    const background = composition.addNode({ type: "background" });
    const title = composition.addNode({ type: "text" });
    const merge = composition.addNode({ type: "merge" });

    composition.connect(background, merge, { toPort: "base" });
    composition.connect(title.id, merge.id, { toPort: "overlay" });

    expect(composition.toJSON().connections).toEqual([
      { from_node: 0, to_node: 2, to_port: "base" },
      { from_node: 1, to_node: 2, to_port: "overlay" },
    ]);
  });

  test("rejects handles of another composition but leaves ids to the engine", () => {
    const composition = new Composition();
    const output = composition.addNode({ type: "media_output" });
    const foreign = new Composition().addNode({ type: "background" });

    expect(() => composition.connect(foreign, output, { toPort: "source" })).toThrow(
      "not a node of this composition",
    );
    expect(composition.toJSON().connections).toEqual([]);

    composition.connect(4, 5, { toPort: "source" });
    expect(composition.toJSON().connections).toEqual([
      { from_node: 4, to_node: 5, to_port: "source" },
    ]);
  });
});

describe("Composition inputs", () => {
  test("connects in the order written, fanning one node out and filling a variadic port", () => {
    const composition = new Composition();
    const background = composition.addNode({ type: "background" });
    const title = composition.addNode({ type: "text" });
    const matte = composition.addNode({ type: "shape" });
    const merge = composition.addNode({
      type: "merge",
      inputs: { base: background, overlay: title, mask: matte },
    });
    const blur = composition.addNode({ type: "blur", inputs: { source: merge } });
    const glow = composition.addNode({ type: "opacity", inputs: { source: merge } });
    const stack = composition.addNode({
      type: "raster_multimerge",
      inputs: { layers: [blur, glow.id, title] },
    });

    expect(composition.toJSON().connections).toEqual([
      { from_node: 0, to_node: 3, to_port: "base" },
      { from_node: 1, to_node: 3, to_port: "overlay" },
      { from_node: 2, to_node: 3, to_port: "mask" },
      { from_node: 3, to_node: 4, to_port: "source" },
      { from_node: 3, to_node: 5, to_port: "source" },
      { from_node: 4, to_node: stack.id, to_port: "layers" },
      { from_node: 5, to_node: stack.id, to_port: "layers" },
      { from_node: 1, to_node: stack.id, to_port: "layers" },
    ]);
    expect(composition.toJSON().nodes.every((node) => !("inputs" in node))).toBe(true);
  });

  test("a failed addNode leaves the composition and the next automatic ID as they were", () => {
    const composition = new Composition();
    const foreign = new Composition().addNode({ type: "background" });
    const card = defineComposition({
      name: "atomic",
      build: (c) => c.addNode({ type: "background" }),
    });
    const throwing = Object.defineProperty({}, "title", {
      enumerable: true,
      get: () => {
        throw new Error("boom");
      },
    });
    const before = JSON.stringify(composition);

    expect(() => composition.addNode({ type: "blur", inputs: { source: foreign } })).toThrow(
      "not a node of this composition",
    );
    expect(() =>
      composition.addNode({
        ...card.clip(),
        params: { composition: "atomic", props: throwing },
      }),
    ).toThrow("boom");

    expect(JSON.stringify(composition)).toBe(before);
    expect(composition.addNode({ type: "media_output" }).id).toBe(0);
  });
});

describe("Composition audio", () => {
  test("converts seconds to whole milliseconds whatever the fps, and keeps track settings", () => {
    const composition = new Composition({ timeline: { fps: 24, durationSeconds: 5 } });

    composition.addAudioTrack({ id: "music", name: "Music", gain: 0.5, solo: true }).addAudio({
      source: "audio:intro",
      start: 0.5,
      duration: 2.5,
      offset: 1.25,
      trackId: "music",
      gain: 0.8,
      id: "intro",
      name: "Intro",
    });

    expect(composition.toJSON().audio).toEqual({
      clips: [
        {
          duration_ms: 2_500,
          id: "intro",
          name: "Intro",
          source_id: "audio:intro",
          source_start_ms: 1_250,
          start_ms: 500,
          track_id: "music",
          volume: 0.8,
        },
      ],
      tracks: [{ id: "music", name: "Music", solo: true, volume: 0.5 }],
    });
  });

  test("declares the track a clip plays on, numbers default ids, and allows overlap", () => {
    const composition = new Composition();

    composition
      .addAudio({ source: "audio:a", start: 0, duration: 2 })
      .addAudio({ source: "audio:b", start: 1, duration: 2 })
      .addAudio({ source: "audio:c", start: 0, duration: 1, trackId: "voice" });

    const audio = composition.toJSON().audio;
    expect(audio?.tracks).toEqual([{ id: "main" }, { id: "voice" }]);
    expect(audio?.clips.map((clip) => clip.id)).toEqual([
      "main-clip-1",
      "main-clip-2",
      "voice-clip-1",
    ]);
    expect(() => composition.addAudioTrack({ id: "voice", muted: true })).toThrow(
      "Audio track `voice` already exists.",
    );
  });
});

describe("Composition timeline defaults", () => {
  test("defaults to 30 fps", () => {
    const composition = new Composition({ timeline: { durationSeconds: 2 } });

    expect(composition.toJSON().timeline).toEqual({ duration_frames: 60, fps: 30 });
  });

  test("converts durationSeconds rather than emitting a key the engine rejects", () => {
    const composition = new Composition({ timeline: { durationSeconds: 2.5, fps: 24 } });

    expect(composition.toJSON().timeline).toEqual({ duration_frames: 60, fps: 24 });
  });
});

describe("Composition rich text", () => {
  test("lowers expressions nested in spans and decorations", () => {
    const composition = new Composition();

    composition.addNode({
      type: "text",
      params: {
        content: [
          { text: uppercase("x"), font_size: frame.mul(2), color: hex("#ffffff") },
          { text: "=literal", underline: { extent: vec2(0, progress(0, 30)) }, highlight: null },
        ],
        strikethrough: { offset: frame.sub(4), style: "dashed" },
      },
    });

    expect(composition.toJSON().nodes[0]?.params).toEqual({
      content: [
        { text: "=uppercase('x')", font_size: "=(frame * 2)", color: "=#ffffff" },
        { text: "=literal", underline: { extent: "=vec2(0, progress(0, 30))" }, highlight: null },
      ],
      strikethrough: { offset: "=(frame - 4)", style: "dashed" },
    });
  });
});
