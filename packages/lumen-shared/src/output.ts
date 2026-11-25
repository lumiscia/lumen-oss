import { nodeNamedOutputs } from "@lumiscia/lumen-types";

import type { TypedNode } from "./types.js";

/**
 * Gives `node` an `out` holding each output its kind names beside `output`, as
 * `{ node, output }`. `out` isn't enumerable, so spreading the node or writing it as JSON leaves
 * it out.
 */
export function withOutputs<TNode extends { readonly type: string }>(node: TNode): TNode {
  if (!Object.hasOwn(nodeNamedOutputs, node.type)) {
    return node;
  }
  const names: readonly string[] = nodeNamedOutputs[node.type as keyof typeof nodeNamedOutputs];
  const out = Object.freeze(Object.fromEntries(names.map((output) => [output, { node, output }])));
  return Object.defineProperty(node, "out", { value: out });
}

/** Whether `source` is one of a node's `out` outputs rather than a node. */
export function isNodeOutput(
  source: number | object,
): source is { readonly node: TypedNode; readonly output: string } {
  return typeof source !== "number" && "node" in source && "output" in source;
}
