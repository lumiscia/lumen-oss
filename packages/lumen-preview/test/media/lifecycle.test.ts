import { describe, expect, it } from "vitest";

import {
  MediaLifecycle,
  type MediaDependency,
  type MediaLoader,
} from "../../src/media/lifecycle.js";
import { LumenMediaError } from "../../src/media/errors.js";

type Input = { bytes: string };

function deferred() {
  let resolve: () => void = () => {};
  let reject: (error: unknown) => void = () => {};
  const promise = new Promise<void>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, reject, resolve };
}

/** A loader whose loads finish when the test says, recording what it loads and releases. */
function fakeLoader() {
  const calls: string[] = [];
  const loads = new Map<string, ReturnType<typeof deferred>>();
  const loader: MediaLoader<Input> = {
    load: ({ key }, { bytes }) => {
      calls.push(`load ${key} ${bytes}`);
      const load = deferred();
      loads.set(key, load);
      return load.promise;
    },
    release: ({ key }) => {
      calls.push(`release ${key}`);
    },
    same: (left, right) => left.bytes === right.bytes,
  };
  return { calls, loader, loads };
}

function image(source: string, bytes: string): MediaDependency<Input> {
  return {
    at: { node: 1, param: "source", type: "node" },
    key: `image:${source}`,
    kind: "image",
    load: { input: { bytes } },
    source,
  };
}

const flush = () => new Promise((resolve) => setTimeout(resolve));

describe("MediaLifecycle", () => {
  it("loads what's new or changed, keeps what's unchanged and releases what's gone", () => {
    const { calls, loader } = fakeLoader();
    const lifecycle = new MediaLifecycle(loader, () => {});

    lifecycle.sync([image("a", "1"), image("b", "1")]);
    lifecycle.sync([image("a", "1"), image("b", "2")]);
    lifecycle.sync([image("b", "2")]);

    expect(calls).toEqual([
      "load image:a 1",
      "load image:b 1",
      "release image:b",
      "load image:b 2",
      "release image:a",
    ]);
  });

  it("goes from pending to ready or failed, naming where the composition uses a failure", async () => {
    const { loader, loads } = fakeLoader();
    const states: string[][] = [];
    const lifecycle = new MediaLifecycle(loader, () =>
      states.push(lifecycle.entries().map(({ source, status }) => `${source} ${status}`)),
    );

    lifecycle.sync([image("a", "1"), image("b", "1")]);
    loads.get("image:a")?.resolve();
    loads
      .get("image:b")
      ?.reject(new LumenMediaError("invalid_source", "offline", { sourceId: "b" }));
    await flush();

    expect(states).toEqual([
      ["a pending", "b pending"],
      ["a ready", "b pending"],
      ["a ready", "b failed"],
    ]);
    expect(lifecycle.entries()[1]?.failure).toMatchObject({
      at: { node: 1, param: "source", type: "node" },
      code: "media.not_found",
      source: "b",
    });
  });

  it("tries a failed dependency again on the next sync", async () => {
    const { calls, loader, loads } = fakeLoader();
    const lifecycle = new MediaLifecycle(loader, () => {});
    lifecycle.sync([image("a", "1")]);
    loads.get("image:a")?.reject(new Error("corrupt"));
    await flush();

    lifecycle.sync([image("a", "1")]);

    expect(calls).toEqual(["load image:a 1", "release image:a", "load image:a 1"]);
    expect(lifecycle.entries()[0]?.status).toBe("pending");
  });

  it("ignores a load that finishes after its dependency was released, and aborts it", async () => {
    const { loader, loads } = fakeLoader();
    let signal: AbortSignal | undefined;
    const lifecycle = new MediaLifecycle(
      {
        ...loader,
        load: (dependency, input, abort) => {
          signal = abort;
          return loader.load(dependency, input, abort);
        },
      },
      () => {},
    );
    lifecycle.sync([image("a", "1")]);
    lifecycle.sync([]);
    loads.get("image:a")?.resolve();
    await flush();

    expect(signal?.aborted).toBe(true);
    expect(lifecycle.entries()).toEqual([]);
  });

  it("fails a dependency that resolved to nothing without loading it", () => {
    const { calls, loader } = fakeLoader();
    const lifecycle = new MediaLifecycle(loader, () => {});
    const failure = {
      code: "media.not_found",
      details: "`media` has no entry by this name",
      source: "logo.png",
      wanted: ["image"],
    } as const;

    lifecycle.sync([
      { key: "image:logo.png", kind: "image", load: { failure }, source: "logo.png" },
    ]);
    lifecycle.sync([]);

    expect(calls).toEqual([]);
  });

  it("settles once nothing it waits for is pending, with the first failure", async () => {
    const { loader, loads } = fakeLoader();
    const lifecycle = new MediaLifecycle(loader, () => {});
    lifecycle.sync([image("a", "1"), { ...image("song", "1"), key: "audio:song", kind: "audio" }]);
    let settled: unknown = "pending";
    void lifecycle.settled(({ kind }) => kind !== "audio").then((failure) => (settled = failure));

    loads.get("image:a")?.reject(new Error("corrupt"));
    await flush();

    expect(settled).toMatchObject({ code: "media.decode", details: "corrupt", source: "a" });
    expect(lifecycle.entries().map(({ status }) => status)).toEqual(["failed", "pending"]);
  });
});
