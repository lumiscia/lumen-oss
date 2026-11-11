import { readFile } from "node:fs/promises";

import { expect, test } from "vitest";

import { compositionSchemaPath } from "../src/paths.js";
import { renderNodeReference } from "../src/render-reference.js";

test("lists every node kind with its ports and params, the same every time", async () => {
  const schema = JSON.parse(await readFile(compositionSchemaPath, "utf8"));
  const reference = renderNodeReference(schema);

  for (const kind of ["render_3d", "model_3d", "image_plane_3d", "instance_3d", "text_3d"]) {
    expect(reference).toContain(`## \`${kind}\``);
  }
  expect(reference).toContain("- `texture`: raster, optional");
  expect(reference).toContain("- `objects`: scene, optional, takes several connections");
  expect(reference).toMatch(/- \*\*`tone_mapping`\*\* \([^)]*literal only\)/u);
  expect(reference).toMatch(/- \*\*`position`\*\* \(\[x, y, z\]; default `\[0,0,0\]`\)/u);
  expect(renderNodeReference(schema)).toBe(reference);
});
