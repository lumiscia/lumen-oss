import { nodeInputPorts } from "@lumiscia/lumen-types";

import { withOutputs } from "./output.js";
import type { NodeFunctions } from "./types.js";

/**
 * Node values by kind, as `nodes.glow({ radius: 46, source })`: each function sorts its object
 * into the node's `params` and its `inputs` by the kind's input port names. A value added to a
 * composition, or read by a node that is, becomes one node however many nodes read it. A kind
 * that names outputs beside `output` gives its values an `out`, as `render.out.depth`.
 */
export const nodes = Object.fromEntries(
  Object.entries(nodeInputPorts).map(([type, ports]) => [
    type,
    (props: Readonly<Record<string, unknown>> = {}) => {
      const params: Record<string, unknown> = {};
      const inputs: Record<string, unknown> = {};
      for (const [name, value] of Object.entries(props)) {
        if (value !== undefined) {
          (Object.hasOwn(ports, name) ? inputs : params)[name] = value;
        }
      }
      return withOutputs({
        type,
        ...(Object.keys(params).length === 0 ? {} : { params }),
        ...(Object.keys(inputs).length === 0 ? {} : { inputs }),
      });
    },
  ]),
) as unknown as NodeFunctions;
