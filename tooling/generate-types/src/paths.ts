import path from "node:path";
import { fileURLToPath } from "node:url";

const dirname = path.dirname(fileURLToPath(import.meta.url));

export const repoRoot = path.resolve(dirname, "../../..");

export const metaPath = path.join(
  repoRoot,
  "vendor/lumen-definitions/meta.json",
);

export const generatedSchemaTypesPath = path.join(
  repoRoot,
  "packages/lumen-types/src/generated/schema.ts",
);

export const generatedMetaTypesPath = path.join(
  repoRoot,
  "packages/lumen-types/src/generated/meta.ts",
);
