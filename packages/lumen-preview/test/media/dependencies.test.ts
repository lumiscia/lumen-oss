import type { MediaInventory, MediaSourceKind } from "@lumiscia/lumen-types";
import { describe, expect, it } from "vitest";

import { previewDependencies, type PreviewMediaEntry } from "../../src/media/dependencies.js";
// lumen-server checks the same cases against its media root and asset store.
import rulesSource from "../../../../crates/server/tests/fixtures/source-rules.json?raw";

type Rules = { cases: { source: string; provided: boolean; code: string | null }[] };

const rules = JSON.parse(rulesSource) as Rules;

function inventory(
  media: { source: string; kinds: MediaSourceKind[] }[],
  dynamic: MediaSourceKind[][] = [],
): MediaInventory {
  return {
    dynamic: dynamic.map((kinds, index) => ({ kinds, node: 10 + index, param: "source" })),
    fonts: [],
    media: media.map(({ kinds, source }) => ({
      kinds,
      source,
      uses: [{ at: { node: 1, param: "source", type: "node" }, kinds }],
    })),
  };
}

const entries = (media: Record<string, PreviewMediaEntry>) => new Map(Object.entries(media));

describe("previewDependencies", () => {
  it("accepts the sources lumen-server accepts, and fails the others with its code", () => {
    for (const { code, provided, source } of rules.cases) {
      const media = entries(provided ? { [source]: { source: "https://a.test/file" } } : {});
      const [dependency] = previewDependencies(
        inventory([{ kinds: ["model"], source }]),
        media,
        new Map(),
      );
      const failure = dependency && "failure" in dependency.load ? dependency.load.failure : null;
      expect(failure?.code ?? null, source).toBe(code);
    }
  });

  it("names where a missing source is used and what it is read as", () => {
    const [dependency] = previewDependencies(
      inventory([{ kinds: ["environment"], source: "sky.hdr" }]),
      new Map(),
      new Map(),
    );

    expect(dependency).toMatchObject({
      key: "environment:sky.hdr",
      load: {
        failure: {
          at: { node: 1, param: "source", type: "node" },
          code: "media.not_found",
          source: "sky.hdr",
          wanted: ["environment"],
        },
      },
    });
  });

  it("loads a source read as video and as audio once for each, and nothing unnamed", () => {
    const dependencies = previewDependencies(
      inventory([{ kinds: ["video", "audio"], source: "clip.mp4" }]),
      entries({ "clip.mp4": { source: "https://a.test/clip.mp4" }, unused: { source: "x" } }),
      new Map([["Inter", "https://a.test/inter.ttf"]]),
    );

    expect(dependencies.map(({ key }) => key)).toEqual([
      "audio:clip.mp4",
      "video:clip.mp4",
      "font:Inter",
    ]);
  });

  it("reads a source of several kinds as its entry or blob says, else as its first", () => {
    const either = inventory([
      { kinds: ["image", "video"], source: "a" },
      { kinds: ["image", "video"], source: "b" },
      { kinds: ["image", "video"], source: "c" },
    ]);
    const kinds = previewDependencies(
      either,
      entries({
        a: { kind: "video", source: "a.mp4" },
        b: { source: new Blob([], { type: "video/mp4" }) },
        c: { source: "c.bin" },
      }),
      new Map(),
    ).map(({ kind }) => kind);

    expect(kinds).toEqual(["video", "video", "image"]);
  });

  it("fails an entry whose kind the composition doesn't read it as", () => {
    const [dependency] = previewDependencies(
      inventory([{ kinds: ["model"], source: "fox" }]),
      entries({ fox: { kind: "image", source: "fox.png" } }),
      new Map(),
    );

    expect(dependency?.load).toMatchObject({
      failure: { code: "media.wrong_kind", found: "image", wanted: ["model"] },
    });
  });

  it("loads the entries only an expression can name, by the kind they say", () => {
    const media = entries({
      named: { source: "named.png" },
      typed: { source: new Blob([], { type: "image/png" }) },
    });
    const dependencies = previewDependencies(
      inventory([{ kinds: ["image"], source: "named" }], [["image", "video"]]),
      media,
      new Map(),
    );
    expect(dependencies.map(({ key }) => key)).toEqual(["image:named", "image:typed"]);

    expect(() =>
      previewDependencies(
        inventory([], [["image", "video"]]),
        entries({ untyped: { source: "x" } }),
        new Map(),
      ),
    ).toThrow(/needs a kind/);
  });
});
