import { readFile } from "node:fs/promises";

import { expect, test } from "vitest";

import { compositionSchemaPath, openApiPath, runtimeSchemaPath } from "../src/paths.js";
import { renderApiTypes } from "../src/render-api.js";
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
    /export type DiagnosticCode =\n {2}\/\*\* The text is not JSON\. \*\/\n {2}\| "document\.syntax"[\s\S]*\| \(string & \{\}\);/u,
  );
  expect(types).toContain(
    '{ code: "media.not_found", severity: "error", foundBy: "check, render", meaning: "A media source isn\'t there: ',
  );
  expect(types).toContain("export interface Diagnostic {");
});

test("renders the API's open unions, its event union, the runtime types it uses and its operations", async () => {
  const [api, runtime] = await Promise.all(
    [openApiPath, runtimeSchemaPath].map(async (file) => JSON.parse(await readFile(file, "utf8"))),
  );
  const types = renderApiTypes(api, runtime);

  expect(types).toMatch(
    /export type ErrorCode =\n {2}\| "unauthorized"[\s\S]*\| \(string & \{\}\);/u,
  );
  expect(types).toMatch(
    /export type RenderStatus =[\s\S]*\| "cancelled"\n {2}\| \(string & \{\}\);/u,
  );
  expect(types).toContain("export type RenderEvent = {");
  expect(types).toContain("readonly objectKey: string | null;");
  expect(types).toContain('import type { LumenComposition } from "./schema.js";');
  expect(types).toContain('"jpeg": { contentType: "image/jpeg", extension: "jpg" },');
  expect(types).toContain(
    'getRender: { method: "GET", path: "/renders/{id}", request: "none", response: "json" },',
  );
  expect(types).toContain(
    "readonly createRender: { readonly request: RenderRequest; readonly response: CreateRenderResponse };",
  );
  expect(types).toContain(
    "readonly renderSocket: { readonly request: undefined; readonly response: RenderEvent };",
  );
});
