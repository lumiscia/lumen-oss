import type { ClipProps, Color, CompositionTemplate, Vec2, Vec3 } from "@lumiscia/lumen-types";

import { Composition, captureClipTemplate, clipTemplate } from "./composition.js";
import { Expr, literal } from "./expression.js";
import { quote } from "./quote.js";
import type { Value } from "./expression.js";
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
 * `prop.color(..)`, `prop.vec2(..)` and `prop.vec3(..)` declare props for `defineComposition`,
 * with an optional default.
 */
export const prop = Object.assign((name: string): Expr => readProp(name), {
  string: (defaultValue?: string) => declaration("string", defaultValue),
  number: (defaultValue?: number) => declaration("number", defaultValue),
  color: (defaultValue?: Color) => declaration("color", defaultValue),
  vec2: (defaultValue?: Vec2) => declaration("vec2", defaultValue),
  vec3: (defaultValue?: Vec3) => declaration("vec3", defaultValue),
});

function readProp<T extends Value = Value>(name: string): Expr<T> {
  return new Expr<T>(`prop(${quote(name)})`);
}

function declaration<TType extends PropType>(
  type: TType,
  defaultValue: PropDeclaration<TType>["default"],
): PropDeclaration<TType> {
  return (
    defaultValue === undefined ? { type } : { type, default: defaultValue }
  ) as PropDeclaration<TType>;
}

/**
 * Defines a reusable composition. `build` returns the node it outputs, reading props through its
 * second argument: a node value, or a node it added to the fresh composition it is given. Each
 * `clip` plays it with its own timing and props.
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
    Object.keys(props).map((prop) => [prop, readProp(prop)]),
  ) as PropReads<TProps>;
  const composition = new Composition();
  composition.addOutput(options.build(composition, reads));

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
  const captured = captureClipTemplate(name, template, composition);
  return {
    name,
    template,
    clip: ({ id, props: values, ...timing } = {}) => {
      const node: NodeInput<"clip"> = {
        ...(id === undefined ? {} : { id }),
        type: "clip",
        params: {
          composition: name,
          ...timing,
          ...(values === undefined ? {} : { props: literalProps(values) }),
        },
      };
      return Object.assign(node, { [clipTemplate]: captured });
    },
  };
}

/** Text starting with `=` is escaped as `==`. `addNode` lowers the expressions. */
function literalProps(values: object): ClipProps {
  return Object.fromEntries(
    Object.entries(values).map(([name, value]: [string, unknown]) => [
      name,
      typeof value === "string" ? literal(value) : value,
    ]),
  ) as ClipProps;
}
