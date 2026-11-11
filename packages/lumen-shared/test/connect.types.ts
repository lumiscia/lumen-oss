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

// Every node so far outputs a raster, which a scene port doesn't take.
// @ts-expect-error background outputs a raster, not a scene
const scene: NodeReference<"scene"> = background;
const sceneById: NodeReference<"scene"> = background.id;
const raster: NodeReference<"raster"> = merge;
const any: NodeReference = merge;
void [scene, sceneById, raster, any];
