import { Composition } from "../src/composition.js";

const composition = new Composition();
const background = composition.addNode({ type: "background" });
const merge = composition.addNode({ type: "merge" });
const output = composition.addNode({ type: "media_output" });

composition.connect(background, merge, { toPort: "base" });
composition.connect(merge, output, { toPort: "source" });
composition.connect(merge.id, output.id, { toPort: "source" });

// @ts-expect-error the target port is required
composition.connect(merge, output);
// @ts-expect-error merge has no `source` input
composition.connect(background, merge, { toPort: "source" });
// @ts-expect-error background has no inputs
composition.connect(merge, background, { toPort: "source" });
// @ts-expect-error no node has an `input` port
composition.connect(merge.id, output.id, { toPort: "input" });
