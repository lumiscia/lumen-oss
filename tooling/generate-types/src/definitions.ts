import { readFile } from "node:fs/promises";

export interface MetaManifest {
  readonly schemaVersion: number;
  readonly nodeKinds: readonly string[];
  readonly nodeSpecs: Readonly<Record<string, NodeSpec>>;
}

export interface NodeSpec {
  readonly kind: string;
  readonly name: string;
  readonly description: string;
  readonly category: string;
  readonly inputs?: readonly NodePortSpec[];
  readonly outputs?: readonly NodePortSpec[];
  readonly params?: readonly NodeParamSpec[];
  readonly defaultParams?: Readonly<Record<string, unknown>>;
}

export interface NodePortSpec {
  readonly name: string;
  readonly kind: string;
  readonly optional?: boolean;
  readonly variadic?: boolean;
}

export interface NodeParamSpec {
  readonly id: string;
  readonly name: string;
  readonly kind: string;
  readonly description?: string;
  readonly defaultValue?: unknown;
  readonly enumOptions?: readonly NodeEnumOptionSpec[];
  readonly constraints?: ParamConstraintsSpec;
}

export interface ParamConstraintsSpec {
  readonly min?: number;
  readonly max?: number;
  readonly step?: number;
  readonly format?: string;
  readonly multiline?: boolean;
  readonly recommendedRows?: number;
  readonly role?: string;
}

export interface NodeEnumOptionSpec {
  readonly name: string;
  readonly label: string;
  readonly value: number;
}

interface JsonSchema {
  readonly [key: string]: unknown;
}

export async function readCompositionSchemaManifest(path: string): Promise<MetaManifest> {
  const raw = await readFile(path, "utf8");
  return manifestFromCompositionSchema(JSON.parse(raw) as JsonSchema);
}

function manifestFromCompositionSchema(schema: JsonSchema): MetaManifest {
  const schemaVersion = numberValue(schema["x-lumen-schemaVersion"]) ?? 1;
  const nodeVariants = arrayValue(
    objectValue(objectValue(schema.properties)?.nodes)?.items &&
      objectValue(objectValue(objectValue(schema.properties)?.nodes)?.items)?.oneOf,
  );
  const nodeSpecs = Object.fromEntries(
    nodeVariants.map((variant) => {
      const spec = nodeSpecFromSchema(objectValue(variant));
      return [spec.kind, spec];
    }),
  );
  return {
    schemaVersion,
    nodeKinds: Object.keys(nodeSpecs),
    nodeSpecs,
  };
}

function nodeSpecFromSchema(schema: JsonSchema): NodeSpec {
  const schemaProperties = objectValue(schema.properties);
  const kind = stringValue(objectValue(schemaProperties?.type)?.const);
  if (!kind) {
    throw new Error("composition schema node variant is missing properties.type.const");
  }
  const paramSchemas = objectValue(objectValue(schemaProperties?.properties)?.properties);
  const nodeParams = Object.entries(paramSchemas ?? {}).map(([id, param]) =>
    paramSpecFromSchema(id, objectValue(param)),
  );
  return {
    kind,
    name: stringValue(schema.title) ?? kind,
    description: stringValue(schema.description) ?? "",
    category: stringValue(schema["x-lumen-category"]) ?? "processing",
    inputs: arrayValue(schema["x-lumen-inputs"]).map((value) => nodePortSpec(value)),
    outputs: arrayValue(schema["x-lumen-outputs"]).map((value) => nodePortSpec(value)),
    params: nodeParams,
    defaultParams: Object.fromEntries(
      nodeParams
        .filter((param) => param.defaultValue !== undefined)
        .map((param) => [param.id, param.defaultValue]),
    ),
  };
}

function paramSpecFromSchema(id: string, schema: JsonSchema): NodeParamSpec {
  const constraints = constraintsFromSchema(schema);
  return {
    id,
    name: stringValue(schema.title) ?? id,
    kind: stringValue(schema["x-lumen-kind"]) ?? paramKindFromSchema(schema),
    description: stringValue(schema.description) ?? "",
    defaultValue: schema.default,
    enumOptions: arrayValue(schema["x-lumen-enumOptions"]).map((value) => enumOptionSpec(value)),
    ...(constraints === undefined ? {} : { constraints }),
  };
}

function constraintsFromSchema(schema: JsonSchema): ParamConstraintsSpec | undefined {
  const custom = objectValue(schema["x-lumen-constraints"]);
  const constraints: Record<string, unknown> = {};
  const min = numberValue(schema.minimum);
  const max = numberValue(schema.maximum);
  const step = numberValue(custom.step);
  const format = stringValue(custom.format);
  const multiline = booleanValue(custom.multiline);
  const recommendedRows = numberValue(custom.recommendedRows);
  const role = stringValue(custom.role);
  if (min !== undefined) constraints.min = min;
  if (max !== undefined) constraints.max = max;
  if (step !== undefined) constraints.step = step;
  if (format !== undefined) constraints.format = format;
  if (multiline !== undefined) constraints.multiline = multiline;
  if (recommendedRows !== undefined) constraints.recommendedRows = recommendedRows;
  if (role !== undefined) constraints.role = role;
  return Object.keys(constraints).length === 0 ? undefined : (constraints as ParamConstraintsSpec);
}

function paramKindFromSchema(schema: JsonSchema): string {
  if (Array.isArray(schema.enum)) return "enum";
  if (schema.$ref === "#/$defs/color") return "color";
  if (schema.$ref === "#/$defs/vec2") return "vec2";
  return stringValue(schema.type) ?? "string";
}

function nodePortSpec(value: unknown): NodePortSpec {
  const object = objectValue(value);
  const spec: Record<string, unknown> = {
    name: stringValue(object.name) ?? "",
    kind: stringValue(object.kind) ?? "raster_frame",
  };
  const optional = booleanValue(object.optional);
  const variadic = booleanValue(object.variadic);
  if (optional !== undefined) spec.optional = optional;
  if (variadic !== undefined) spec.variadic = variadic;
  return spec as unknown as NodePortSpec;
}

function enumOptionSpec(value: unknown): NodeEnumOptionSpec {
  const object = objectValue(value);
  return {
    name: stringValue(object.name) ?? "",
    label: stringValue(object.label) ?? "",
    value: numberValue(object.value) ?? 0,
  };
}

function objectValue(value: unknown): JsonSchema {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as JsonSchema)
    : {};
}

function arrayValue(value: unknown): readonly unknown[] {
  return Array.isArray(value) ? value : [];
}

function stringValue(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function numberValue(value: unknown): number | undefined {
  return typeof value === "number" ? value : undefined;
}

function booleanValue(value: unknown): boolean | undefined {
  return typeof value === "boolean" ? value : undefined;
}
