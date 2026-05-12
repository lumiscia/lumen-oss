import { readCompositionSchemaManifest } from "./definitions.js";
import {
  compositionSchemaPath,
  generatedMetaTypesPath,
  generatedSchemaTypesPath,
} from "./paths.js";
import { renderCompositionTypes, renderMetaTypes, renderSchemaTypePrelude } from "./render.js";
import { writeGeneratedFile } from "./write.js";

const manifest = await readCompositionSchemaManifest(compositionSchemaPath);
const metaTypes = renderMetaTypes(manifest);
const schemaTypes = `${renderSchemaTypePrelude()}${renderCompositionTypes()}`;

await Promise.all([
  writeGeneratedFile(generatedSchemaTypesPath, schemaTypes),
  writeGeneratedFile(generatedMetaTypesPath, metaTypes),
]);
