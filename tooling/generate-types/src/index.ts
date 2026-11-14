import { readFile } from "node:fs/promises";

import {
  compositionSchemaPath,
  generatedExpressionsPath,
  generatedMetaTypesPath,
  generatedRuntimeTypesPath,
  generatedSchemaTypesPath,
  nodeReferencePath,
  runtimeSchemaPath,
} from "./paths.js";
import type { JsonSchema } from "./render-defs.js";
import { expressionTables, renderExpressions } from "./render-expressions.js";
import { renderNodeReference } from "./render-reference.js";
import { renderRuntimeTypes } from "./render-runtime.js";
import { renderTypes } from "./render.js";
import { writeGeneratedFile } from "./write.js";

const schema = JSON.parse(await readFile(compositionSchemaPath, "utf8")) as JsonSchema;
const { meta, root } = renderTypes(schema);
const runtimeSchema = JSON.parse(await readFile(runtimeSchemaPath, "utf8")) as JsonSchema;

await Promise.all([
  writeGeneratedFile(generatedSchemaTypesPath, root),
  writeGeneratedFile(generatedMetaTypesPath, meta),
  writeGeneratedFile(generatedRuntimeTypesPath, renderRuntimeTypes(runtimeSchema)),
  writeGeneratedFile(generatedExpressionsPath, renderExpressions(expressionTables(schema))),
  writeGeneratedFile(nodeReferencePath, renderNodeReference(schema)),
]);
