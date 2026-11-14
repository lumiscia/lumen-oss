import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { check, initSync, modelInfo } from "../wasm/lumen_wasm_2d.js";

initSync({ module: readFileSync(new URL("../wasm/lumen_wasm_2d_bg.wasm", import.meta.url)) });

const timeline = { fps: 30, duration_frames: 90 };
const render_settings = { width: 8, height: 4 };

assert.deepEqual(
  check(
    JSON.stringify({
      timeline,
      render_settings,
      nodes: [
        { id: 1, type: "background", params: { width: 8, height: 4, paint: [0, 0, 0, 255] } },
        { id: 2, type: "media_output" },
      ],
      connections: [{ from_node: 1, from_port: "output", to_node: 2, to_port: "source" }],
    }),
  ),
  [],
);

const diagnostics = check(
  JSON.stringify({
    timeline,
    render_settings,
    nodes: [
      { id: 1, type: "shape_3d", params: { shape: "box" } },
      { id: 2, type: "render_3d" },
      { id: 3, type: "media_output" },
    ],
    connections: [
      { from_node: 1, to_node: 2, to_port: "scene" },
      { from_node: 2, to_node: 3, to_port: "source" },
    ],
  }),
);
assert.ok(diagnostics.length > 0);
assert.ok(
  diagnostics.every(({ severity, code }) => severity === "error" && code === "node.unsupported"),
  JSON.stringify(diagnostics),
);
assert.throws(() => modelInfo(new Uint8Array(8)), /no 3D support/);

console.log("the 2D wasm loaded and rejected 3D nodes");
