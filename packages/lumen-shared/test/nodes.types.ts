import type { NodeParamPortNameClash } from "@lumiscia/lumen-types";

import { Composition } from "../src/composition.js";
import {
  cos,
  floor,
  frame,
  hex,
  instance,
  instanceCount,
  mod,
  progress,
  sin,
  vec3,
  when,
} from "../src/expression.js";
import { nodes as n } from "../src/nodes.js";
import { defineComposition } from "../src/template.js";
import type { NodeInput, NodeSource, NodeValue } from "../src/types.js";

const composition = new Composition();
const title = composition.addNode({ type: "text", params: { content: "Title" } });
const box = composition.addNode({ type: "shape_3d" });

const lit = n.raster_multimerge({ layers: [n.text({ content: "on" }), title] });
const glow: NodeInput<"glow"> = n.glow({ glow_only: true, radius: 46, source: lit });
n.merge({ base: glow, overlay: title, mask: n.shape(), opacity: progress(0, 30) });
n.text({ content: "Hi", color: when(frame.gte(10), hex("#ffffff"), [0, 0, 0]) });
n.switch({ layers: glow, selected_layer: 1 });
n.camera_3d();
n.media_in({ source: "media:intro" });
composition.addNode({ type: "raster_multimerge", inputs: { layers: [title, glow] } });
composition.addOutput(lit);

// @ts-expect-error merge has no `source` input
n.merge({ source: glow });
// @ts-expect-error glow has no `radiuss` param
n.glow({ radiuss: 4 });
// @ts-expect-error `radius` takes a number
n.glow({ radius: "4" });
// @ts-expect-error `radius` takes a number expression, not a colour
n.glow({ radius: hex("#ffffff") });
// @ts-expect-error `base` takes one node
n.merge({ base: [glow, title] });
// @ts-expect-error background has no inputs
n.background({ source: glow });
// @ts-expect-error media_output takes no params
n.media_output({ source: glow, opacity: 1 });

const scene = n.merge_3d({ objects: [n.shape_3d({ shape: "box" }), box, n.light_3d()] });
const sceneValue: NodeValue<"scene"> = scene;
const raster: NodeSource<"raster"> = n.render_3d({ scene, width: 480 });
n.image_plane_3d({ source: n.render_3d({ scene }) });
n.model_3d({ source: "asset:tv", texture: glow });
const ring = instance.div(instanceCount).mul(2 * Math.PI);
n.render_3d({
  scene: n.instance_3d({
    scene: n.model_3d({ source: "asset:tree" }),
    count: 12,
    position: vec3(cos(ring).mul(4), 0, sin(ring).mul(4)),
    rotation: vec3(0, instance.mul(30), 0),
  }),
});
n.instance_3d({
  scene: n.merge_3d({ objects: [box, n.light_3d()] }),
  count: 100,
  position: vec3(mod(instance, 10).mul(1.5), 0, floor(instance.div(10)).mul(1.5)),
  scale: [0.5, 0.5, 0.5],
});
// @ts-expect-error an instance_3d takes a scene, not a raster
n.instance_3d({ scene: glow });

// @ts-expect-error a scene goes through a render_3d before a raster input
n.opacity({ source: scene });
// @ts-expect-error a scene goes through a render_3d before a raster input
n.raster_multimerge({ layers: [glow, n.shape_3d()] });
// @ts-expect-error a scene goes through a render_3d before a raster input
composition.addNode({ type: "opacity", inputs: { source: n.shape_3d() } });
// @ts-expect-error a scene goes through a render_3d before a raster input
composition.addOutput(scene);
// @ts-expect-error background outputs a raster, not a scene
n.render_3d({ scene: n.background() });
// @ts-expect-error a render_3d outputs a raster, not a scene
n.merge_3d({ objects: [box, n.render_3d({ scene })] });
// @ts-expect-error an image plane shows a raster, not a scene
n.image_plane_3d({ source: scene });
n.shape_3d({ color: "#ff0000" });
const gradient = {
  type: "linear",
  stops: [
    { offset: 0, color: "#000000" },
    { offset: 1, color: "#ffffff" },
  ],
} as const;
// @ts-expect-error a 3D colour is solid, never a gradient
n.shape_3d({ color: gradient });
// @ts-expect-error shape_3d outputs a scene, not a raster
const rasterScene: NodeValue<"raster"> = n.shape_3d();

defineComposition({ name: "card", build: () => n.text({ content: "Card" }) });
// @ts-expect-error a template outputs a raster, not a scene
defineComposition({ name: "card", build: () => scene });

// No kind gives a param and an input port the same name.
// @ts-expect-error `NodeParamPortNameClash` is `never`
const clash: NodeParamPortNameClash = "source";

void [sceneValue, raster, rasterScene, clash];
