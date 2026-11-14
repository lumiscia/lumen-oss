import path from "node:path";
import { fileURLToPath } from "node:url";

const dirname = path.dirname(fileURLToPath(import.meta.url));

export const repoRoot = path.resolve(dirname, "../../..");

export const compositionSchemaPath = path.join(repoRoot, "definitions/composition.schema.json");

export const generatedSchemaTypesPath = path.join(
  repoRoot,
  "packages/lumen-types/src/generated/schema.ts",
);

export const generatedMetaTypesPath = path.join(
  repoRoot,
  "packages/lumen-types/src/generated/meta.ts",
);

export const generatedNodeSpecsPath = path.join(
  repoRoot,
  "packages/lumen-types/src/generated/node-specs.ts",
);

export const runtimeSchemaPath = path.join(repoRoot, "definitions/runtime.schema.json");

export const generatedRuntimeTypesPath = path.join(
  repoRoot,
  "packages/lumen-types/src/generated/runtime.ts",
);

export const nodeReferencePath = path.join(repoRoot, "docs/node-reference.md");

export const generatedExpressionsPath = path.join(
  repoRoot,
  "packages/lumen-shared/src/generated/expressions.ts",
);

export const engineReadmePath = path.join(repoRoot, "crates/engine/README.md");
