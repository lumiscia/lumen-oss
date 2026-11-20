import type { NodeKind, NodeOutputName } from "@lumiscia/lumen-types";

import type { NodeOutputOf, TypedNode } from "./types.js";

/**
 * Output `output` of `node`, for a port to read in place of its default `output`, as
 * `outputOf(render, "depth")`. The name is checked against `node`'s kind.
 */
export function outputOf<TKind extends NodeKind, TOutput extends NodeOutputName<TKind>>(
  node: TypedNode<TKind> & { readonly type: TKind },
  output: TOutput,
): NodeOutputOf<TKind, TOutput> {
  return { node, output };
}

/** Whether `source` is an output `outputOf` returned rather than a node. */
export function isNodeOutput(
  source: number | object,
): source is { readonly node: TypedNode; readonly output: string } {
  return typeof source !== "number" && "node" in source && "output" in source;
}
