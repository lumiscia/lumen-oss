import { readMetaManifest } from "./definitions.js";
import {
  generatedMetaTypesPath,
  generatedSchemaTypesPath,
  metaPath,
} from "./paths.js";
import {
  renderCompositionTypes,
  renderMetaTypes,
  renderSchemaTypePrelude,
} from "./render.js";
import { writeGeneratedFile } from "./write.js";

const manifest = await readMetaManifest(metaPath);
const metaTypes = renderMetaTypes(manifest);
const schemaTypes = `${renderSchemaTypePrelude()}${renderCompositionTypes()}`;

await Promise.all([
  writeGeneratedFile(generatedSchemaTypesPath, schemaTypes),
  writeGeneratedFile(generatedMetaTypesPath, metaTypes),
]);
