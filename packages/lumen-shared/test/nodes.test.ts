import { describe, expect, test } from "vitest";

import { Composition } from "../src/composition.js";
import { expr, frame, progress, when } from "../src/expression.js";
import { nodes as n } from "../src/nodes.js";
import { defineComposition, prop } from "../src/template.js";
import type { NodeInput } from "../src/types.js";

describe("nodes", () => {
  test("sorts props into params and input ports by the kind's port names", () => {
    const background = n.background({ paint: "#101820", width: 640 });
    const glow = n.glow({ glow_only: true, radius: 46, source: background });

    expect(background).toEqual({ type: "background", params: { paint: "#101820", width: 640 } });
    expect(glow).toEqual({
      type: "glow",
      params: { glow_only: true, radius: 46 },
      inputs: { source: background },
    });
    expect(n.raster_multimerge({ blend_mode: "screen", layers: [background, glow] })).toEqual({
      type: "raster_multimerge",
      params: { blend_mode: "screen" },
      inputs: { layers: [background, glow] },
    });
    expect(n.media_output({ source: glow })).toEqual({
      type: "media_output",
      inputs: { source: glow },
    });
    // `media_in` has a `source` param and no inputs.
    expect(n.media_in({ source: "media:intro" })).toEqual({
      type: "media_in",
      params: { source: "media:intro" },
    });
    expect(n.camera_3d()).toEqual({ type: "camera_3d" });
  });

  test("leaves expressions for addNode to lower", () => {
    const fade = when(frame.gte(30), 1, progress(0, 30));
    const title = n.text({ content: "Hi", opacity: fade, position: expr`[10, ${frame}]` });

    expect(new Composition().addNode(title).params).toEqual({
      content: "Hi",
      position: "=[10, frame]",
      opacity: "=if((frame >= 30), 1, progress(0, 30))",
    });
  });
});

describe("Composition with node values", () => {
  test("adds a tree leaves first, as the same addNode calls would", () => {
    const tree = new Composition().addOutput(treeOf());

    const calls = new Composition();
    const background = calls.addNode({ type: "background", params: { paint: "#123d2b" } });
    const path = calls.addNode({ type: "path", params: { data: "M0 0" } });
    const blur = calls.addNode({ type: "blur", params: { radius: 2 }, inputs: { source: path } });
    const shadow = calls.addNode({
      type: "shadow",
      params: { radius: 8 },
      inputs: { source: blur },
    });
    const text = calls.addNode({ type: "text", params: { content: "on" } });
    const shape = calls.addNode({ type: "shape" });
    const merged = calls.addNode({ type: "raster_multimerge", inputs: { layers: [text, shape] } });
    const glow = calls.addNode({
      type: "glow",
      params: { glow_only: true, radius: 46 },
      inputs: { source: merged },
    });
    const root = calls.addNode({
      type: "raster_multimerge",
      inputs: { layers: [background, shadow, glow, merged] },
    });
    calls.addNode({ type: "media_output", inputs: { source: root } });

    expect(JSON.stringify(tree)).toBe(JSON.stringify(calls));
    expect(JSON.stringify(new Composition().addOutput(treeOf()))).toBe(JSON.stringify(tree));
  });

  test("makes a value read twice one node with two readers", () => {
    const composition = new Composition();
    const shared = n.text({ content: "twice" });
    composition.addOutput(
      n.merge({ base: n.blur({ radius: 4, source: shared }), overlay: shared }),
    );

    const { nodes, connections } = composition.toJSON();
    expect(nodes.map((node) => node.type)).toEqual(["text", "blur", "merge", "media_output"]);
    expect(connections).toEqual([
      { from_node: 0, to_node: 1, to_port: "source" },
      { from_node: 1, to_node: 2, to_port: "base" },
      { from_node: 0, to_node: 2, to_port: "overlay" },
      { from_node: 2, to_node: 3, to_port: "source" },
    ]);
  });

  test("addNode always adds its own node, and a value it added is that node to later readers", () => {
    const composition = new Composition();
    const base = { type: "text" as const, params: { content: "first" } };
    const first = composition.addNode(base);
    base.params.content = "second";
    const second = composition.addNode(base);
    expect(second).not.toBe(first);
    expect(composition.toJSON().nodes.map((node) => node.params)).toEqual([
      { content: "first" },
      { content: "second" },
    ]);

    composition.addOutput(n.blur({ source: base }));
    expect(composition.toJSON().connections[0]).toEqual({
      from_node: first.id,
      to_node: 2,
      to_port: "source",
    });
  });

  test("adds a chain thousands of nodes deep", () => {
    let source: NodeInput<"text" | "blur"> = n.text();
    for (let index = 0; index < 5_000; index += 1) {
      source = n.blur({ source });
    }
    const composition = new Composition().addOutput(source);
    expect(composition.toJSON().nodes).toHaveLength(5_002);
  });

  test("feeds one scene to two render_3d nodes with their own params", () => {
    const scene = n.merge_3d({
      objects: [
        n.camera_3d({ position: [0, 1, 6] }),
        n.light_3d({ light: "directional", intensity: 2 }),
        n.shape_3d({ shape: "torus", color: "#ee5b24" }),
      ],
    });
    const composition = new Composition().addOutput(
      n.raster_multimerge({
        layers: [
          n.render_3d({ scene, tone_mapping: "agx" }),
          n.opacity({ opacity: 0.5, source: n.render_3d({ scene, exposure: -1, width: 480 }) }),
        ],
      }),
    );

    const { nodes, connections } = composition.toJSON();
    expect(nodes.map((node) => node.type)).toEqual([
      "camera_3d",
      "light_3d",
      "shape_3d",
      "merge_3d",
      "render_3d",
      "render_3d",
      "opacity",
      "raster_multimerge",
      "media_output",
    ]);
    expect(nodes[4]?.params).toEqual({ tone_mapping: "agx" });
    expect(nodes[5]?.params).toEqual({ exposure: -1, width: 480 });
    expect(connections.filter((connection) => connection.from_node === 3)).toEqual([
      { from_node: 3, to_node: 4, to_port: "scene" },
      { from_node: 3, to_node: 5, to_port: "scene" },
    ]);
  });

  test("mixes addNode references and node values", () => {
    const composition = new Composition();
    const title = composition.addNode({ type: "text", params: { content: "Title" } });
    const glow = composition.addNode({
      type: "glow",
      inputs: { source: n.blur({ radius: 3, source: title }) },
    });
    composition.addOutput(n.raster_multimerge({ layers: [glow, n.opacity({ source: title })] }));

    expect(composition.toJSON().connections).toEqual([
      { from_node: 0, to_node: 1, to_port: "source" },
      { from_node: 1, to_node: 2, to_port: "source" },
      { from_node: 0, to_node: 3, to_port: "source" },
      { from_node: 2, to_node: 4, to_port: "layers" },
      { from_node: 3, to_node: 4, to_port: "layers" },
      { from_node: 4, to_node: 5, to_port: "source" },
    ]);
  });

  test("adds nothing when a nested value fails", () => {
    const composition = new Composition();
    composition.addNode({ id: 3, type: "background" });
    const before = JSON.stringify(composition);

    expect(() =>
      composition.addOutput(n.merge({ base: n.text(), overlay: { ...n.shape(), id: 3 } })),
    ).toThrow("Node `3` already exists.");
    expect(JSON.stringify(composition)).toBe(before);
    expect(composition.addNode(n.text()).id).toBe(0);
  });

  test("rejects a value mutated into a cycle", () => {
    const blur: { type: "blur"; inputs: { source?: NodeInput<"blur"> } } = {
      type: "blur",
      inputs: {},
    };
    blur.inputs.source = blur;

    expect(() => new Composition().addNode(blur)).toThrow(
      "A `blur` node value reads itself through its inputs.",
    );
  });

  test("rejects a node of another composition", () => {
    const foreign = new Composition().addNode(n.background());

    expect(() => new Composition().addOutput(n.blur({ source: foreign }))).toThrow(
      "not a node of this composition",
    );
  });
});

describe("defineComposition with node values", () => {
  const card = defineComposition({
    name: "card",
    props: { title: prop.string("Untitled") },
    build: (_, p) =>
      n.raster_multimerge({
        layers: [n.background({ paint: "#000000" }), n.text({ content: p.title })],
      }),
  });

  test("builds the template from the returned value and nests its clip", () => {
    const composition = new Composition().addOutput(
      n.glow({ radius: 12, source: card.clip({ start: 5, props: { title: "One" } }) }),
    );
    const json = composition.toJSON();

    expect(json.compositions?.card?.nodes).toEqual([
      { type: "background", params: { paint: "#000000" }, id: 0 },
      { type: "text", params: { content: "=prop('title')" }, id: 1 },
      { type: "raster_multimerge", id: 2 },
      { type: "media_output", id: 3 },
    ]);
    expect(json.nodes.map((node) => node.type)).toEqual(["clip", "glow", "media_output"]);
    expect(json.nodes[0]?.params).toEqual({
      composition: "card",
      start: 5,
      props: { title: "One" },
    });
  });

  test("matches a template built with addNode", () => {
    const withCalls = defineComposition({
      name: "card",
      props: { title: prop.string("Untitled") },
      build(c, p) {
        const background = c.addNode({ type: "background", params: { paint: "#000000" } });
        const title = c.addNode({ type: "text", params: { content: p.title } });
        return c.addNode({ type: "raster_multimerge", inputs: { layers: [background, title] } });
      },
    });

    expect(JSON.stringify(card.template)).toBe(JSON.stringify(withCalls.template));
  });
});

function treeOf(): NodeInput<"raster_multimerge"> {
  const lit = n.raster_multimerge({ layers: [n.text({ content: "on" }), n.shape()] });
  return n.raster_multimerge({
    layers: [
      n.background({ paint: "#123d2b" }),
      n.shadow({ radius: 8, source: n.blur({ radius: 2, source: n.path({ data: "M0 0" }) }) }),
      n.glow({ glow_only: true, radius: 46, source: lit }),
      lit,
    ],
  });
}
