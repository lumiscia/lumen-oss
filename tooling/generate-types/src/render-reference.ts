import { array, object, type JsonSchema } from "./render-defs.js";

const categoryOrder = ["source", "vector", "processing", "compositing", "scene", "output"];

const expressionRef = "#/$defs/expression";
const hexColorPattern = "^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$";

/** Keywords that say what a value is. Each is rendered. */
const typeKeywords = new Set([
  "$ref",
  "const",
  "enum",
  "anyOf",
  "oneOf",
  "type",
  "minimum",
  "maximum",
  "pattern",
  "minLength",
  "maxLength",
  "items",
  "prefixItems",
  "minItems",
  "maxItems",
  "properties",
  "required",
  "additionalProperties",
  "propertyNames",
  "not",
]);

/**
 * Keywords that are shown separately or only guide editors. `x-lumen-constraints` gives a range
 * when the schema has none; its other fields (`step`, `role`, `format`, `multiline`,
 * `recommendedRows`) only guide editors.
 */
const annotationKeywords = new Set([
  "$schema",
  "title",
  "description",
  "default",
  "x-lumen-kind",
  "x-lumen-constraints",
  "x-lumen-enumOptions",
  "x-lumen-type",
  "x-lumen-media",
]);

const constraintKeys = new Set([
  "min",
  "max",
  "step",
  "role",
  "format",
  "multiline",
  "recommendedRows",
]);

const code = (value: string): string => `\`${value}\``;

const slug = (name: string): string => `type-${name.toLowerCase()}`;

/** A description with its source line wraps joined, keeping paragraph breaks. */
const paragraphs = (description: unknown): string[] =>
  typeof description === "string"
    ? description
        .split("\n\n")
        .map((paragraph) => paragraph.replace(/\s*\n\s*/gu, " ").trim())
        .filter((paragraph) => paragraph !== "")
    : [];

interface Range {
  readonly min: number | undefined;
  readonly max: number | undefined;
}

const noRange: Range = { min: undefined, max: undefined };

/** What the reference has rendered, and the definitions it gives a section of their own. */
interface Context {
  readonly defs: JsonSchema;
  /** Definitions with a section of their own. */
  readonly linked: Set<string>;
  /** Definitions written out where they are used. */
  readonly inlined: Set<string>;
}

/** A value's type: its alternatives, and whether one of them is an expression. */
interface Described {
  readonly members: string[];
  readonly expression: boolean;
  /** Whether it is an array, object or map, whose own fields or elements carry the marks. */
  readonly container: boolean;
}

function assertKeywords(schema: JsonSchema): void {
  for (const keyword of Object.keys(schema)) {
    if (!typeKeywords.has(keyword) && !annotationKeywords.has(keyword)) {
      throw new Error(`unsupported schema keyword \`${keyword}\` in ${JSON.stringify(schema)}`);
    }
  }
  const constraints = object(schema["x-lumen-constraints"]);
  for (const key of Object.keys(constraints)) {
    if (!constraintKeys.has(key)) throw new Error(`unsupported constraint \`${key}\``);
  }
}

const number = (value: unknown): number | undefined =>
  typeof value === "number" ? value : undefined;

function rangeText({ min, max }: Range): string {
  if (min !== undefined && max !== undefined) return `, ${min} to ${max}`;
  if (min !== undefined) return `, at least ${min}`;
  if (max !== undefined) return `, at most ${max}`;
  return "";
}

/** A definition small enough to write where it is used. */
function isSimple(schema: JsonSchema): boolean {
  if (schema.const !== undefined || Array.isArray(schema.enum)) return true;
  if (typeof schema.type === "string" && schema.type !== "object" && schema.type !== "array") {
    return true;
  }
  const prefixItems = array(schema.prefixItems);
  return (
    schema.type === "array" &&
    prefixItems.length > 0 &&
    prefixItems.every((item) => object(item).type === "number")
  );
}

function describe(schema: JsonSchema, ctx: Context, inherited: Range): Described {
  assertKeywords(schema);
  const constraints = object(schema["x-lumen-constraints"]);
  const bounds: Range = {
    min: number(schema.minimum) ?? number(constraints.min) ?? inherited.min,
    max: number(schema.maximum) ?? number(constraints.max) ?? inherited.max,
  };

  if (typeof schema.$ref === "string") {
    if (schema.$ref === expressionRef) return { members: [], expression: true, container: false };
    const name = schema.$ref.replace("#/$defs/", "");
    const def = object(ctx.defs[name]);
    if (!schema.$ref.startsWith("#/$defs/") || Object.keys(def).length === 0) {
      throw new Error(`unknown $ref \`${schema.$ref}\``);
    }
    if (isSimple(def)) {
      ctx.inlined.add(name);
      return describe(def, ctx, bounds);
    }
    ctx.linked.add(name);
    return { members: [`[${name}](#${slug(name)})`], expression: false, container: true };
  }
  if (schema.const !== undefined) {
    return { members: [code(JSON.stringify(schema.const))], expression: false, container: false };
  }
  if (Array.isArray(schema.enum)) {
    return {
      members: schema.enum.map((option) => code(JSON.stringify(option))),
      expression: false,
      container: false,
    };
  }
  const branches = array(schema.anyOf ?? schema.oneOf);
  if (branches.length > 0) {
    const described = branches.map((branch) => describe(object(branch), ctx, bounds));
    return {
      members: described.flatMap((branch) => branch.members),
      expression: described.some((branch) => branch.expression),
      container: described.some((branch) => branch.container),
    };
  }
  switch (schema.type) {
    case "boolean":
      return { members: ["boolean"], expression: false, container: false };
    case "null":
      return { members: ["`null`"], expression: false, container: false };
    case "integer":
    case "number":
      return {
        members: [`${schema.type === "integer" ? "integer" : "number"}${rangeText(bounds)}`],
        expression: false,
        container: false,
      };
    case "string": {
      const { pattern, minLength, maxLength } = schema;
      const notes = [
        pattern === hexColorPattern
          ? "hex colour, `#rrggbb` or `#rrggbbaa`"
          : typeof pattern === "string"
            ? `matching ${code(pattern)}`
            : "",
        number(minLength) !== undefined ? `at least ${minLength} characters` : "",
        number(maxLength) !== undefined ? `at most ${maxLength} characters` : "",
        schema.not !== undefined ? notText(object(schema.not)) : "",
      ].filter((note) => note !== "");
      return {
        members: [notes.length > 0 ? `string ${notes.join(", ")}` : "string"],
        expression: false,
        container: false,
      };
    }
    case "array":
      return { members: [arrayText(schema, ctx, bounds)], expression: false, container: true };
    case "object":
      return {
        members: [
          Object.keys(object(schema.properties)).length > 0
            ? shapeText(schema, ctx)
            : mapText(schema, ctx),
        ],
        expression: false,
        container: true,
      };
    default:
      throw new Error(`unsupported schema ${JSON.stringify(schema)}`);
  }
}

function notText(not: JsonSchema): string {
  if (not.pattern === "^=([^=]|$)") return "not an expression";
  throw new Error(`unsupported \`not\` ${JSON.stringify(not)}`);
}

const inline = (described: Described): string =>
  [...described.members, ...(described.expression ? ["expression"] : [])].join(" | ");

function arrayText(schema: JsonSchema, ctx: Context, inherited: Range): string {
  const prefixItems = array(schema.prefixItems);
  const { minItems, maxItems } = schema;
  if (prefixItems.length > 0) {
    const fixed = minItems === prefixItems.length && maxItems === prefixItems.length;
    if (schema.items !== false && !fixed) {
      throw new Error(`unsupported array ${JSON.stringify(schema)}`);
    }
    return `[${prefixItems.map((item) => inline(describe(object(item), ctx, inherited))).join(", ")}]`;
  }
  if (typeof schema.items !== "object" || schema.items === null) {
    throw new Error(`unsupported array ${JSON.stringify(schema)}`);
  }
  const limits = [
    number(minItems) !== undefined ? `at least ${minItems}` : "",
    number(maxItems) !== undefined ? `at most ${maxItems}` : "",
  ].filter((limit) => limit !== "");
  const items = inline(describe(object(schema.items), ctx, noRange));
  return `array of ${items.includes(" | ") ? `(${items})` : items}${limits.length > 0 ? `, ${limits.join(", ")} items` : ""}`;
}

/** An object written out in one line: its fields, each with the types it takes. */
function shapeText(schema: JsonSchema, ctx: Context): string {
  if (schema.additionalProperties !== false) {
    throw new Error(`unsupported object ${JSON.stringify(schema)}`);
  }
  const required = new Set(array(schema.required));
  const fields = Object.entries(object(schema.properties)).map(
    ([field, value]) =>
      `${field}${required.has(field) ? "" : "?"}: ${inline(describe(object(value), ctx, noRange))}`,
  );
  return `{ ${fields.join("; ")} }`;
}

/** An object with no listed fields: a map from names to one type of value. */
function mapText(schema: JsonSchema, ctx: Context): string {
  const values = schema.additionalProperties;
  if (Object.keys(object(schema.properties)).length > 0 || typeof values !== "object" || !values) {
    throw new Error(`unsupported object ${JSON.stringify(schema)}`);
  }
  const names = object(schema.propertyNames);
  const keyNotes = [
    typeof names.pattern === "string" ? `matching ${code(names.pattern)}` : "",
    names.minLength !== undefined && names.minLength === names.maxLength
      ? `${names.minLength} characters`
      : "",
  ].filter((note) => note !== "");
  const keys = keyNotes.length > 0 ? `names ${keyNotes.join(", ")}` : "names";
  return `map from ${keys} to ${inline(describe(object(values), ctx, noRange))}`;
}

/** The details after a param's or field's name. */
function details(schema: JsonSchema, ctx: Context, required: boolean): string {
  const described = describe(schema, ctx, noRange);
  const parts = [...(required ? ["required"] : []), inline({ ...described, expression: false })];
  const media = object(schema["x-lumen-media"]);
  if (Array.isArray(media.kinds)) {
    const chosen =
      typeof media.selectedBy === "string" ? `, chosen by ${code(media.selectedBy)}` : "";
    parts.push(`media: ${media.kinds.join(", ")}${chosen}`);
  }
  if ("default" in schema) parts.push(`default ${code(JSON.stringify(schema.default))}`);
  if (!described.expression)
    parts.push(described.container ? "literal as a whole" : "literal only");
  return parts.join("; ");
}

function valueLine(name: string, schema: JsonSchema, ctx: Context, required = false): string {
  const [first = "", ...rest] = paragraphs(schema.description);
  const head = `- **${code(name)}** (${details(schema, ctx, required)}) ${first}`.trimEnd();
  return [head, ...rest.map((paragraph) => `\n  ${paragraph}`)].join("\n");
}

function portLine(port: JsonSchema): string {
  const flags = [
    port.optional === true ? "optional" : "required",
    ...(port.variadic === true ? ["takes several connections"] : []),
  ];
  return `- ${code(String(port.name))}: ${port.kind}, ${flags.join(", ")}`;
}

function nodeSection(node: JsonSchema, ctx: Context): string {
  const type = String(object(object(node.properties).type).const);
  const params = Object.entries(object(object(object(node.properties).params).properties));
  const inputs = array(node["x-lumen-inputs"]).map((port) => portLine(object(port)));
  const outputs = array(node["x-lumen-outputs"]).map((output) => String(object(output).kind));
  const lines = [`## ${code(type)}`, "", ...paragraphs(node.description).flatMap((p) => [p, ""])];
  lines.push(`Category: ${node["x-lumen-category"]}. Output: ${outputs.join(", ") || "none"}.`, "");
  if (inputs.length > 0) lines.push("Inputs:", "", ...inputs, "");
  if (params.length > 0) {
    lines.push(
      "Params:",
      "",
      ...params.map(([name, param]) => valueLine(name, object(param), ctx)),
      "",
    );
  }
  return lines.join("\n");
}

/** A definition's section: the fields of an object, or what else the value may be. */
function typeSection(name: string, def: JsonSchema, ctx: Context): string {
  assertKeywords(def);
  const lines = [
    `### Type ${code(name)}`,
    "",
    ...paragraphs(def.description).flatMap((p) => [p, ""]),
  ];
  if (def.type === "object" && Object.keys(object(def.properties)).length > 0) {
    const required = new Set(array(def.required));
    const fields = Object.entries(object(def.properties));
    for (const key of required) {
      if (typeof key !== "string" || !fields.some(([field]) => field === key)) {
        throw new Error(`\`${name}\` requires a field it doesn't have`);
      }
    }
    const extra =
      def.additionalProperties === false
        ? "No other fields."
        : def.additionalProperties === true
          ? "Other fields are allowed."
          : undefined;
    if (extra === undefined) throw new Error(`unsupported object \`${name}\``);
    lines.push(
      "Fields:",
      "",
      ...fields.map(([field, schema]) =>
        valueLine(field, object(schema), ctx, required.has(field)),
      ),
      "",
      extra,
      "",
    );
  } else {
    lines.push(`Value: ${inline(describe(def, ctx, noRange))}.`, "");
  }
  return lines.join("\n");
}

export interface NodeReference {
  readonly markdown: string;
  /** The definitions the reference gives a section, and those it writes out where they are used. */
  readonly linked: ReadonlySet<string>;
  readonly inlined: ReadonlySet<string>;
}

/**
 * One Markdown reference of every node kind in `schema`, grouped by category, with the shared
 * types its params use after them. A schema shape it doesn't understand is an error, so a field
 * can't be dropped quietly.
 */
export function renderNodeReferenceParts(schema: JsonSchema): NodeReference {
  const defs = object(schema.$defs);
  const ctx: Context = { defs, linked: new Set(), inlined: new Set() };
  const nodes = array(object(object(object(schema.properties).nodes).items).oneOf).map(object);
  const category = (node: JsonSchema): string => String(node["x-lumen-category"]);
  const rank = (node: JsonSchema): number => {
    const index = categoryOrder.indexOf(category(node));
    return index === -1 ? categoryOrder.length : index;
  };
  const type = (node: JsonSchema): string => String(object(object(node.properties).type).const);
  const sorted = [...nodes].sort(
    (a, b) =>
      rank(a) - rank(b) || category(a).localeCompare(category(b)) || type(a).localeCompare(type(b)),
  );
  const nodeSections = sorted.map((node) => nodeSection(node, ctx));

  // Rendering a type can link more types, so go on until none is new.
  const typeSections = new Map<string, string>();
  for (let pending = [...ctx.linked]; pending.length > 0;) {
    for (const name of pending) {
      typeSections.set(name, typeSection(name, object(defs[name]), ctx));
    }
    pending = [...ctx.linked].filter((name) => !typeSections.has(name));
  }
  const types = [...typeSections.entries()].sort(([a], [b]) => a.localeCompare(b));

  const markdown = [
    "# Node reference",
    "",
    "Every node kind, with its inputs and params, then the types they share. This file is generated from `definitions/composition.schema.json`: run `just generate-types` to update it.",
    "",
    "A param or field takes an expression, a string that starts with `=`, unless it is marked. **Literal only** means the value is a plain JSON value. **Literal as a whole** means an object, array or map is never an expression itself, but its fields or elements are marked the same way: in a one-line type such as `{ offset: number }` or `array of (number | expression)`, a field or element takes an expression only where `expression` is among its types. Defaults are the JSON values a node uses when it leaves a param out.",
    "",
    ...nodeSections.flatMap((section) => [section, ""]),
    ...(types.length > 0 ? ["## Types", ""] : []),
    ...types.flatMap(([, section]) => [section, ""]),
  ]
    .join("\n")
    .replace(/\n{3,}/gu, "\n\n")
    .trimEnd()
    .concat("\n");
  return { markdown, linked: ctx.linked, inlined: ctx.inlined };
}

export const renderNodeReference = (schema: JsonSchema): string =>
  renderNodeReferenceParts(schema).markdown;
