import { readFile } from "node:fs/promises";

import {
  compositionSchemaPath,
  generatedMetaTypesPath,
  generatedSchemaTypesPath,
  nodeReferencePath,
} from "./paths.js";
import type { JsonSchema } from "./render-defs.js";
import { renderNodeReference } from "./render-reference.js";
import { renderTypes } from "./render.js";
import { writeGeneratedFile } from "./write.js";

const schema = JSON.parse(await readFile(compositionSchemaPath, "utf8")) as JsonSchema;
const { meta, root } = renderTypes(schema);

await Promise.all([
  writeGeneratedFile(generatedSchemaTypesPath, root),
  writeGeneratedFile(generatedMetaTypesPath, meta),
  writeGeneratedFile(nodeReferencePath, renderNodeReference(schema)),
]);
