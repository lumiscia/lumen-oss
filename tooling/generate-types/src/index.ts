import { readFile } from "node:fs/promises";

import {
  compositionSchemaPath,
  generatedApiTypesPath,
  generatedArityPath,
  openApiPath,
  generatedExpressionsPath,
  generatedMetaTypesPath,
  generatedRuntimeTypesPath,
  engineReadmePath,
  generatedNodeSpecsPath,
  generatedSchemaTypesPath,
  nodeReferencePath,
  runtimeSchemaPath,
} from "./paths.js";
import { renderApiTypes } from "./render-api.js";
import type { JsonSchema } from "./render-defs.js";
import { renderCodeTable, replaceCodeTable } from "./render-code-table.js";
import { expressionTables, renderArity, renderExpressions } from "./render-expressions.js";
import { renderNodeReference } from "./render-reference.js";
import { renderNodeSpecs } from "./render-node-specs.js";
import { codeEntries, renderRuntimeTypes } from "./render-runtime.js";
import { renderTypes } from "./render.js";
import { writeGeneratedFile } from "./write.js";

const schema = JSON.parse(await readFile(compositionSchemaPath, "utf8")) as JsonSchema;
const { meta, root } = renderTypes(schema);
const tables = expressionTables(schema);
const runtimeSchema = JSON.parse(await readFile(runtimeSchemaPath, "utf8")) as JsonSchema;
const openApi = JSON.parse(await readFile(openApiPath, "utf8")) as JsonSchema;

const diagnosticCodeDef = (runtimeSchema.$defs as Record<string, JsonSchema>).DiagnosticCode;
const engineReadme = replaceCodeTable(
  await readFile(engineReadmePath, "utf8"),
  renderCodeTable(codeEntries(diagnosticCodeDef!)),
);

await Promise.all([
  writeGeneratedFile(generatedSchemaTypesPath, root),
  writeGeneratedFile(generatedMetaTypesPath, meta),
  writeGeneratedFile(generatedNodeSpecsPath, renderNodeSpecs(schema)),
  writeGeneratedFile(generatedRuntimeTypesPath, renderRuntimeTypes(runtimeSchema)),
  writeGeneratedFile(generatedApiTypesPath, renderApiTypes(openApi, runtimeSchema)),
  writeGeneratedFile(generatedExpressionsPath, renderExpressions(tables)),
  writeGeneratedFile(generatedArityPath, renderArity(tables)),
  writeGeneratedFile(nodeReferencePath, renderNodeReference(schema)),
  writeGeneratedFile(engineReadmePath, engineReadme),
]);
