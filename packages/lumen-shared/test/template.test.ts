import { describe, expect, test } from "vitest";

import { Composition } from "../src/composition.js";
import { fn, frame, progress, tween, type Num } from "../src/expression.js";
import { defineComposition, prop } from "../src/template.js";

const fadeIn = fn("fade_in(start)", (start: Num) => tween(0, 1, start, 10));

const titleCard = defineComposition({
  name: "title_card",
  size: { width: 960, height: 360 },
  fps: 24,
  duration: 120,
  props: { title: prop.string("Untitled"), index: prop.number(1), accent: prop.color() },
  build(c, p) {
    c.addFunction(fadeIn);
    return c.addNode({
      type: "text",
      params: { content: p.title, color: p.accent, opacity: fadeIn(p.index.mul(4)) },
    });
  },
});

describe("defineComposition", () => {
  test("emits the template and a clip per use with its own timing and props", () => {
    const root = new Composition();
    root.addNode(titleCard.clip({ start: 10, props: { title: "Shapes", index: 2 } }));
    root.addNode({
      ...titleCard.clip({
        start: frame.mul(0),
        end: "hold",
        props: { title: "=not an expression", accent: "#22d3ee", index: progress(0, 30) },
      }),
      id: 7,
    });
    const json = root.toJSON();

    expect(json.compositions).toEqual({
      title_card: {
        render_settings: { height: 360, width: 960 },
        timeline: { duration_frames: 120, fps: 24 },
        props: {
          title: { type: "string", default: "Untitled" },
          index: { type: "number", default: 1 },
          accent: { type: "color" },
        },
        nodes: [
          {
            id: 0,
            type: "text",
            params: {
              content: "=prop('title')",
              color: "=prop('accent')",
              opacity: "=fade_in((prop('index') * 4))",
            },
          },
          { id: 1, type: "media_output" },
        ],
        connections: [{ from_node: 0, to_node: 1, to_port: "source" }],
      },
    });
    expect(json.functions).toEqual({ "fade_in(start)": "tween(0, 1, start, 10)" });
    expect(json.nodes).toEqual([
      {
        id: 0,
        type: "clip",
        params: { composition: "title_card", start: 10, props: { title: "Shapes", index: 2 } },
      },
      {
        id: 7,
        type: "clip",
        params: {
          composition: "title_card",
          start: "=(frame * 0)",
          end: "hold",
          props: {
            title: "==not an expression",
            accent: "#22d3ee",
            index: "=progress(0, 30)",
          },
        },
      },
    ]);
  });

  test("registers nested templates and rejects a second template of the same name", () => {
    const strip = defineComposition({
      name: "strip",
      build: (c) => c.addNode(titleCard.clip({ props: { title: "Nested" } })),
    });
    const root = new Composition();
    root.addNode(strip.clip());
    expect(Object.keys(root.toJSON().compositions ?? {})).toEqual(["strip", "title_card"]);
    expect(strip.template.timeline).toBeUndefined();

    const impostor = defineComposition({
      name: "title_card",
      build: (c) => c.addNode({ type: "background" }),
    });
    expect(() => root.addNode(impostor.clip())).toThrow("Composition `title_card` already exists.");
    expect(() => defineComposition({ name: "x", fps: 30, build: () => 0 })).toThrow(
      "Composition `x` sets `fps` without a `duration`.",
    );
  });
});
