import { readFile } from "node:fs/promises";

import { expect, test } from "vitest";

import { compositionSchemaPath } from "../src/paths.js";
import { renderNodeReference, renderNodeReferenceParts } from "../src/render-reference.js";

type Json = Record<string, unknown>;

const loadSchema = async (): Promise<Json> =>
  JSON.parse(await readFile(compositionSchemaPath, "utf8")) as Json;

/** Every `$defs` name reachable from a node's params, found without the renderer. */
function reachableDefs(schema: Json): Set<string> {
  const defs = schema.$defs as Record<string, unknown>;
  const found = new Set<string>();
  const visit = (value: unknown): void => {
    if (Array.isArray(value)) return value.forEach(visit);
    if (typeof value !== "object" || value === null) return;
    for (const [key, child] of Object.entries(value)) {
      if (key === "$ref" && typeof child === "string") {
        const name = child.replace("#/$defs/", "");
        if (name !== "expression" && !found.has(name)) {
          found.add(name);
          visit(defs[name]);
        }
      } else {
        visit(child);
      }
    }
  };
  const nodes = (((schema.properties as Json).nodes as Json).items as Json).oneOf as Json[];
  for (const node of nodes) visit((node.properties as Json).params);
  return found;
}

test("lists every node kind with its ports and params, the same every time", async () => {
  const schema = await loadSchema();
  const reference = renderNodeReference(schema);

  for (const kind of ["render_3d", "model_3d", "image_plane_3d", "instance_3d", "text_3d"]) {
    expect(reference).toContain(`## \`${kind}\``);
  }
  expect(reference).toContain("- `texture`: raster, optional");
  expect(reference).toContain("- `objects`: scene, optional, takes several connections");
  expect(reference).toMatch(/- \*\*`tone_mapping`\*\* \([^)]*literal only\)/u);
  expect(reference).toMatch(
    /- \*\*`position`\*\* \(\[number, number, number\]; default `\[0,0,0\]`\)/u,
  );
  expect(renderNodeReference(schema)).toBe(reference);
});

test("gives every definition a node param reaches either a section or its place in the param", async () => {
  const schema = await loadSchema();
  const { markdown, linked, inlined } = renderNodeReferenceParts(schema);

  const reachable = reachableDefs(schema);
  const rendered = new Set([...linked, ...inlined]);
  expect([...reachable].filter((name) => !rendered.has(name))).toEqual([]);
  expect([...rendered].filter((name) => !reachable.has(name))).toEqual([]);
  for (const name of linked) expect(markdown).toContain(`### Type \`${name}\``);
  expect(markdown.match(/^### Type /gmu)?.length).toBe(linked.size);
});

test("marks expressions where the schema allows them, not on a container that only holds some", async () => {
  const { markdown } = renderNodeReferenceParts(await loadSchema());
  const line = (prefix: string): string =>
    markdown.split("\n").find((candidate) => candidate.startsWith(prefix)) ?? "";

  // A map whose values take expressions is still literal as a whole.
  expect(line("- **`font_features`**")).toContain("literal as a whole");
  expect(line("- **`repeat`**")).toContain("literal as a whole");
  expect(line("- **`stroke_dash`**")).toContain("literal as a whole");
  expect(line("- **`highlight`**")).toContain("literal as a whole");
  // The values and elements inside say that they take expressions.
  expect(markdown).toContain(
    "Value: map from names matching `^[\\x20-\\x7E]+$`, 4 characters to integer, 0 to 4294967295 | expression.",
  );
  expect(markdown).toMatch(/Value: array of \(number, [^)]*\| expression\), at most 16 items\./u);
  // A plain number or vector param takes one.
  expect(line("- **`position`** ([number, number, number]")).not.toContain("literal");
});

test("shows what a shared type holds", async () => {
  const { markdown } = renderNodeReferenceParts(await loadSchema());
  const section = (name: string): string => {
    const start = markdown.indexOf(`### Type \`${name}\``);
    const end = markdown.indexOf("\n### ", start + 1);
    return markdown.slice(start, end === -1 ? undefined : end);
  };

  expect(section("repeat")).toMatch(/`count`\*\* \(required; integer, 0 to 1024\)/u);
  expect(section("repeat")).toContain("`columns`");
  expect(section("repeat")).toContain("`scale_offset`");
  expect(section("dashPattern")).toContain("at most 16 items");
});

test("fails on a schema keyword or shape it does not understand", async () => {
  const schema = await loadSchema();
  const nodes = (((schema.properties as Json).nodes as Json).items as Json).oneOf as Json[];
  const withParam = (param: Json): Json => {
    const copy = structuredClone(schema);
    const first = (((copy.properties as Json).nodes as Json).items as Json).oneOf as Json[];
    const params = (((first[0] as Json).properties as Json).params as Json).properties as Json;
    params.invented = param;
    return copy;
  };
  expect(nodes.length).toBeGreaterThan(0);

  expect(() => renderNodeReference(withParam({ type: "number", multipleOf: 3 }))).toThrow(
    /unsupported schema keyword `multipleOf`/u,
  );
  expect(() => renderNodeReference(withParam({ type: "tuple" }))).toThrow(/unsupported schema/u);
  expect(() => renderNodeReference(withParam({ $ref: "#/$defs/missing" }))).toThrow(
    /unknown \$ref/u,
  );
  expect(() =>
    renderNodeReference(
      withParam({ type: "object", properties: { a: { type: "number" } }, patternProperties: {} }),
    ),
  ).toThrow(/unsupported schema keyword `patternProperties`/u);
  expect(() =>
    renderNodeReference(withParam({ type: "array", items: { type: "number" }, uniqueItems: true })),
  ).toThrow(/unsupported schema keyword `uniqueItems`/u);
});
