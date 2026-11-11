import { array, object, type JsonSchema } from "./render-defs.js";

const categoryOrder = ["source", "vector", "processing", "compositing", "scene", "output"];

const expressionRef = "#/$defs/expression";

const code = (value: string): string => `\`${value}\``;

/** A description with its source line wraps joined, keeping paragraph breaks. */
const paragraphs = (description: unknown): string[] =>
  typeof description === "string"
    ? description
        .split("\n\n")
        .map((paragraph) => paragraph.replace(/\s*\n\s*/gu, " ").trim())
        .filter((paragraph) => paragraph !== "")
    : [];

/** Whether an expression is among the values of `param`, through the definitions it refers to. */
function takesExpression(param: JsonSchema, defs: JsonSchema): boolean {
  const seen = new Set<string>();
  const pending: unknown[] = [param];
  while (pending.length > 0) {
    const text = JSON.stringify(pending.pop());
    if (text.includes(expressionRef)) return true;
    for (const [, name] of text.matchAll(/"\$ref":"#\/\$defs\/([^"]+)"/gu)) {
      if (name !== undefined && !seen.has(name)) {
        seen.add(name);
        pending.push(defs[name]);
      }
    }
  }
  return false;
}

function paramType(param: JsonSchema): string {
  const kind = String(param["x-lumen-kind"]);
  const constraints = object(param["x-lumen-constraints"]);
  const { min, max } = constraints;
  const range =
    typeof min === "number" && typeof max === "number"
      ? `, ${min} to ${max}`
      : typeof min === "number"
        ? `, at least ${min}`
        : typeof max === "number"
          ? `, at most ${max}`
          : "";
  switch (kind) {
    case "float":
      return `number${range}`;
    case "int":
      return `integer${range}`;
    case "bool":
      return "boolean";
    case "vec2":
      return "[x, y]";
    case "vec3":
      return "[x, y, z]";
    case "enum": {
      const options = array(object(array(param.anyOf)[0]).enum ?? param.enum);
      return options.map((option) => code(JSON.stringify(option))).join(" | ");
    }
    default:
      return kind;
  }
}

function paramLine(name: string, param: JsonSchema, defs: JsonSchema): string {
  const details = [paramType(param)];
  const media = object(param["x-lumen-media"]).kinds;
  if (Array.isArray(media)) details.push(`media: ${media.join(", ")}`);
  if ("default" in param) details.push(`default ${code(JSON.stringify(param.default))}`);
  if (!takesExpression(param, defs)) details.push("literal only");
  const [first = "", ...rest] = paragraphs(param.description);
  const head = `- **${code(name)}** (${details.join("; ")}) ${first}`.trimEnd();
  return [head, ...rest.map((paragraph) => `\n  ${paragraph}`)].join("\n");
}

function portLine(port: JsonSchema): string {
  const flags = [
    port.optional === true ? "optional" : "required",
    ...(port.variadic === true ? ["takes several connections"] : []),
  ];
  return `- ${code(String(port.name))}: ${port.kind}, ${flags.join(", ")}`;
}

function nodeSection(node: JsonSchema, defs: JsonSchema): string {
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
      ...params.map(([name, param]) => paramLine(name, object(param), defs)),
      "",
    );
  }
  return lines.join("\n");
}

/** One Markdown reference of every node kind in `schema`, grouped by category. */
export function renderNodeReference(schema: JsonSchema): string {
  const defs = object(schema.$defs);
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
  return [
    "# Node reference",
    "",
    "Every node kind, with its inputs and params. This file is generated from `definitions/composition.schema.json`: run `just generate-types` to update it.",
    "",
    "A param takes an expression, a string that starts with `=`, unless it is marked literal only. Defaults are the JSON values a node uses when it leaves a param out.",
    "",
    ...sorted.flatMap((node) => [nodeSection(node, defs), ""]),
  ]
    .join("\n")
    .replace(/\n{3,}/gu, "\n\n")
    .trimEnd()
    .concat("\n");
}
