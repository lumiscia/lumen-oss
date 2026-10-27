import type { ClipProps, Color, CompositionTemplate, Vec2 } from "@lumiscia/lumen-types";

import { Composition, clipTemplate } from "./composition.js";
import { Expr, readProp } from "./expression.js";
import type {
  CompositionDefinition,
  CompositionDefinitionOptions,
  NodeInput,
  PropDeclaration,
  PropDeclarations,
  PropReads,
  PropType,
} from "./types.js";

/**
 * Reads a template prop by name, as `prop('title')`. `prop.string(..)`, `prop.number(..)`,
 * `prop.color(..)` and `prop.vec2(..)` declare props for `defineComposition`, with an optional
 * default.
 */
export const prop = Object.assign((name: string): Expr => readProp(name), {
  string: (defaultValue?: string) => declaration("string", defaultValue),
  number: (defaultValue?: number) => declaration("number", defaultValue),
  color: (defaultValue?: Color) => declaration("color", defaultValue),
  vec2: (defaultValue?: Vec2) => declaration("vec2", defaultValue),
});

function declaration<TType extends PropType>(
  type: TType,
  defaultValue: PropDeclaration<TType>["default"],
): PropDeclaration<TType> {
  return defaultValue === undefined ? { type } : { type, default: defaultValue };
}

/**
 * Defines a reusable composition. `build` adds its nodes to a fresh composition, reading props
 * through its second argument, and returns the node it outputs. Each `clip` plays it with its
 * own timing and props.
 */
export function defineComposition<TProps extends PropDeclarations = Record<never, never>>(
  options: CompositionDefinitionOptions<TProps>,
): CompositionDefinition<TProps> {
  const { name, size, fps, duration } = options;
  if (fps !== undefined && duration === undefined) {
    throw new Error(`Composition \`${name}\` sets \`fps\` without a \`duration\`.`);
  }

  const props: PropDeclarations = options.props ?? {};
  const reads = Object.fromEntries(
    Object.entries(props).map(([prop, { type }]) => [prop, readProp(prop, type)]),
  ) as PropReads<TProps>;
  const composition = new Composition();
  const source = options.build(composition, reads);
  composition.connect(source, composition.addNode({ type: "media_output" }), {
    toPort: "source",
  });

  const { connections, nodes } = composition.toJSON();
  const template: CompositionTemplate = {
    ...(size === undefined ? {} : { render_settings: { height: size.height, width: size.width } }),
    ...(duration === undefined
      ? {}
      : { timeline: { duration_frames: duration, ...(fps === undefined ? {} : { fps }) } }),
    ...(Object.keys(props).length === 0 ? {} : { props }),
    nodes,
    connections,
  };
  const definition: CompositionDefinition<TProps> = {
    name,
    template,
    composition,
    clip: ({ id, props: values, ...timing } = {}) => {
      const node: NodeInput<"clip"> = {
        ...(id === undefined ? {} : { id }),
        type: "clip",
        params: {
          composition: name,
          ...timing,
          ...(values === undefined ? {} : { props: lowerProps(values) }),
        },
      };
      return Object.assign(node, { [clipTemplate]: definition });
    },
  };
  return definition;
}

/** Expressions become `=...` strings, and literal text starting with `=` is escaped as `==`. */
function lowerProps(values: object): ClipProps {
  return Object.fromEntries(
    Object.entries(values).map(([name, value]: [string, unknown]) => [
      name,
      value instanceof Expr
        ? value.toString()
        : typeof value === "string" && value.startsWith("=")
          ? `=${value}`
          : value,
    ]),
  ) as ClipProps;
}
