import { Composition } from "../src/composition.js";
import type { NodeReference } from "../src/types.js";

const composition = new Composition();
const background = composition.addNode({ type: "background" });
const merge = composition.addNode({ type: "merge" });
const output = composition.addNode({ type: "media_output" });

composition.connect(background, merge, { toPort: "base" });
composition.connect(merge, output, { toPort: "source" });
composition.connect(merge.id, output.id, { toPort: "source" });
composition.connect<"merge">(background, merge, { toPort: "base" });

// @ts-expect-error the target port is required
composition.connect(merge, output);
// @ts-expect-error merge has no `source` input
composition.connect(background, merge, { toPort: "source" });
// @ts-expect-error background has no inputs
composition.connect(merge, background, { toPort: "source" });
// @ts-expect-error no node has an `input` port
composition.connect(merge.id, output.id, { toPort: "input" });

composition.addNode({ type: "merge", inputs: { base: background, overlay: merge.id } });
composition.addNode({ type: "raster_multimerge", inputs: { layers: [background, merge] } });
composition.addNode({ type: "switch", inputs: { layers: background } });

// @ts-expect-error merge has no `source` input
composition.addNode({ type: "merge", inputs: { source: background } });
// @ts-expect-error `base` takes one node
composition.addNode({ type: "merge", inputs: { base: [background, merge] } });
// @ts-expect-error background has no inputs
composition.addNode({ type: "background", inputs: { source: merge } });

const box = composition.addNode({ type: "shape_3d", params: { position: [0, 1, 0] } });
const group = composition.addNode({ type: "merge_3d", inputs: { objects: [box] } });
const render = composition.addNode({ type: "render_3d", inputs: { scene: group } });
composition.connect(box, group, { toPort: "objects" });
composition.connect(render, output, { toPort: "source" });

// @ts-expect-error a scene goes through a render_3d before a raster input
composition.connect(box, output, { toPort: "source" });
// @ts-expect-error a scene goes through a render_3d before a raster input
composition.addNode({ type: "opacity", inputs: { source: group } });
// @ts-expect-error background outputs a raster, not a scene
composition.addNode({ type: "render_3d", inputs: { scene: background } });
// @ts-expect-error a render_3d outputs a raster, not a scene
composition.addNode({ type: "merge_3d", inputs: { objects: [box, render] } });

// @ts-expect-error background outputs a raster, not a scene
const scene: NodeReference<"scene"> = background;
const sceneNode: NodeReference<"scene"> = group;
const sceneById: NodeReference<"scene"> = background.id;
const raster: NodeReference<"raster"> = render;
// @ts-expect-error shape_3d outputs a scene, not a raster
const rasterScene: NodeReference<"raster"> = box;
const any: NodeReference = merge;
void [scene, sceneNode, sceneById, raster, rasterScene, any];
