import { readFile } from "node:fs/promises";

import { expect, test } from "vitest";

import { compositionSchemaPath, runtimeSchemaPath } from "../src/paths.js";
import { renderRuntimeTypes } from "../src/render-runtime.js";
import { renderTypes } from "../src/render.js";

test("rejects a kind with a param named like one of its input ports", async () => {
  const schema = JSON.parse(await readFile(compositionSchemaPath, "utf8"));
  const blur = schema.properties.nodes.items.oneOf.find(
    (variant: { properties: { type: { const: string } } }) =>
      variant.properties.type.const === "blur",
  );
  blur.properties.params.properties.source = { type: "number", "x-lumen-kind": "float" };

  expect(() => renderTypes(schema)).toThrow("`blur` has a param and an input port named `source`");
});

test("renders the media params each node kind declares, the same every time", async () => {
  const schema = JSON.parse(await readFile(compositionSchemaPath, "utf8"));
  const { meta } = renderTypes(schema);

  expect(meta).toContain(
    '"media_in": { "source": { kinds: ["image", "video"], selectedBy: "kind" } },',
  );
  expect(meta).toContain('"render_3d": { "environment_map": { kinds: ["environment"] } },');
  expect(meta).toContain('"blur": {},');
  expect(meta).toContain('export type MediaSourceKind = "audio" | "environment"');
  expect(renderTypes(schema).meta).toBe(meta);
});

test("renders diagnostic codes as an open union beside their table", async () => {
  const schema = JSON.parse(await readFile(runtimeSchemaPath, "utf8"));
  const types = renderRuntimeTypes(schema);

  expect(types).toMatch(
    /export type DiagnosticCode =\n {2}\| "audio\.invalid"[\s\S]*\| \(string & \{\}\);/u,
  );
  expect(types).toContain('{ code: "media.not_found", severity: "error" },');
  expect(types).toContain("export interface Diagnostic {");
});
