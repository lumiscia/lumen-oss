import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createModuleWorker } from "../src/module-worker.js";

const created: { options: WorkerOptions; url: string }[] = [];
const revoked: string[] = [];
const blobs = new Map<string, Blob>();

beforeEach(() => {
  vi.stubGlobal("location", { origin: "https://app.test" });
  vi.stubGlobal("document", { baseURI: "https://app.test/editor/" });
  vi.stubGlobal(
    "Worker",
    class {
      constructor(url: string, options: WorkerOptions) {
        created.push({ options, url });
      }
    },
  );
  vi.spyOn(URL, "createObjectURL").mockImplementation((blob) => {
    const url = `blob:https://app.test/${blobs.size}`;
    blobs.set(url, blob as Blob);
    return url;
  });
  vi.spyOn(URL, "revokeObjectURL").mockImplementation((url) => revoked.push(url));
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  created.length = 0;
  revoked.length = 0;
  blobs.clear();
});

describe("createModuleWorker", () => {
  it("starts a same-origin script directly", () => {
    createModuleWorker("../worker.js");

    expect(created).toEqual([{ options: { type: "module" }, url: "https://app.test/worker.js" }]);
    expect(blobs.size).toBe(0);
  });

  it.each([
    "https://cdn.test/worker.js",
    "http://app.test/worker.js",
    "https://app.test:8443/w.js",
  ])("imports %s from a revoked same-origin blob", async (url) => {
    createModuleWorker(new URL(url));

    const [blobUrl, blob] = [...blobs][0] as [string, Blob];
    expect(created).toEqual([{ options: { type: "module" }, url: blobUrl }]);
    expect(await blob.text()).toBe(`import ${JSON.stringify(url)};`);
    expect(revoked).toEqual([blobUrl]);
  });
});
