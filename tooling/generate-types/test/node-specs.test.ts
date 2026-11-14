import { readFile } from "node:fs/promises";

import { expect, test } from "vitest";

import {
  nodeSpecByKind,
  nodeSpecs,
} from "../../../packages/lumen-types/src/generated/node-specs.js";
import { compositionSchemaPath } from "../src/paths.js";
import { renderNodeSpecs } from "../src/render-node-specs.js";

const tinySchema = {
  properties: {
    nodes: {
      items: {
        oneOf: [
          {
            title: "Blur",
            description: "Blurs a raster.",
            properties: {
              type: { const: "blur" },
              params: {
                properties: {
                  radius: {
                    default: 4,
                    description: "Radius in pixels.",
                    maximum: 100,
                    title: "Radius",
                    "x-lumen-constraints": { min: 0, step: 0.5 },
                    "x-lumen-kind": "float",
                  },
                  mode: {
                    default: "soft",
                    description: "How it blurs.",
                    title: "Mode",
                    "x-lumen-enumOptions": [
                      { label: "Soft", name: "soft", value: 0 },
                      { label: "Hard", name: "hard", value: 1 },
                    ],
                    "x-lumen-kind": "enum",
                  },
                },
              },
            },
            "x-lumen-category": "processing",
            "x-lumen-inputs": [
              { kind: "raster", name: "source", optional: false, variadic: false },
            ],
            "x-lumen-outputs": [{ kind: "raster", name: "output" }],
          },
        ],
      },
    },
  },
};

test("renders a node's ports, params, defaults, limits and enum options", () => {
  const specs = renderNodeSpecs(tinySchema).split("export const nodeSpecs")[1]!.split("];")[0];
  expect(specs).toBe(`: readonly NodeSpec[] = [
  {
    kind: "blur",
    name: "Blur",
    description: "Blurs a raster.",
    category: "processing",
    inputs: [
      { name: "source", kind: "raster", optional: false, variadic: false },
    ],
    outputs: [
      { name: "output", kind: "raster" },
    ],
    params: [
      { id: "radius", name: "Radius", kind: "float", description: "Radius in pixels.", defaultValue: 4, constraints: { min: 0, max: 100, step: 0.5 } },
      { id: "mode", name: "Mode", kind: "enum", description: "How it blurs.", defaultValue: "soft", enumOptions: [{ name: "soft", label: "Soft", value: 0 }, { name: "hard", label: "Hard", value: 1 }] },
    ],
    defaultParams: { radius: 4, mode: "soft" },
  },
`);
});

test("has a spec for every node kind in the schema", async () => {
  const schema = JSON.parse(await readFile(compositionSchemaPath, "utf8"));
  const kinds = schema.properties.nodes.items.oneOf.map(
    (node: { properties: { type: { const: string } } }) => node.properties.type.const,
  );
  expect(nodeSpecs.map((spec) => spec.kind)).toEqual(kinds);
  expect(Object.keys(nodeSpecByKind)).toEqual(kinds);
});
