import { nodeMediaSources, type LumenComposition } from "@lumiscia/lumen-types";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { resolvePreviewInputs } from "../../src/session/inputs.js";

function composition(
  nodes: readonly unknown[],
  templates: readonly unknown[] = [],
): LumenComposition {
  return {
    timeline: { duration_frames: 1 },
    nodes,
    compositions: Object.fromEntries(templates.map((nodes, index) => [`t${index}`, { nodes }])),
  } as unknown as LumenComposition;
}

const kindsOf = (document: LumenComposition, aliases: readonly string[]) =>
  Object.fromEntries(
    [...resolvePreviewInputs(document, Object.fromEntries(aliases.map((a) => [a, "x"]))).media].map(
      ([id, registration]) => [id, registration.kind],
    ),
  );

describe("media kinds from the composition", () => {
  beforeEach(() => {
    vi.stubGlobal("document", { baseURI: "https://a.test/" });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("reads each source's kind from the generated media table", () => {
    for (const [type, params] of Object.entries(nodeMediaSources)) {
      for (const [param, media] of Object.entries(params)) {
        if (media.kinds[0] === "environment") {
          continue;
        }
        const document = composition([{ id: 1, type, params: { [param]: "alias" } }]);
        expect(kindsOf(document, ["alias"]), `${type}.${param}`).toEqual({
          alias: media.kinds[0],
        });
      }
    }
  });

  it("lets a param's selector pick among its kinds, in templates too", () => {
    const media = (kind: string, source: string) => ({
      id: 1,
      type: "media_in",
      params: { kind, source },
    });
    const document = composition(
      [media("video", "clip"), media("image", "photo")],
      [[media("video", "inner"), media("=frame", "computed")]],
    );
    expect(kindsOf(document, ["clip", "photo", "inner", "computed"])).toEqual({
      clip: "video",
      photo: "image",
      inner: "video",
      computed: "image",
    });
  });

  it("knows no kind for sources of nodes outside the table", () => {
    const document = composition([{ id: 1, type: "blur", params: { source: "alias" } }]);
    expect(() => resolvePreviewInputs(document, { alias: "x" })).toThrow(/needs a kind/);
  });
});
