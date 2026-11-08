import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createModuleWorker } from "../src/module-worker.js";

const created: { options: WorkerOptions; url: string; worker: EventTarget }[] = [];
const revoked: string[] = [];
const blobs = new Map<string, Blob>();

beforeEach(() => {
  vi.stubGlobal("origin", "https://app.test");
  vi.stubGlobal("document", { baseURI: "https://app.test/editor/" });
  vi.stubGlobal(
    "Worker",
    class extends EventTarget {
      constructor(url: string, options: WorkerOptions) {
        super();
        created.push({ options, url, worker: this });
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

    expect(created).toMatchObject([
      { options: { type: "module" }, url: "https://app.test/worker.js" },
    ]);
    expect(blobs.size).toBe(0);
  });

  it.each(["data:text/javascript,postMessage(1)", "blob:https://app.test/0"])(
    "starts %s directly",
    (url) => {
      createModuleWorker(url);

      expect(created).toMatchObject([{ url }]);
      expect(blobs.size).toBe(0);
    },
  );

  it.each([
    "https://cdn.test/worker.js",
    "http://app.test/worker.js",
    "https://app.test:8443/w.js",
  ])("imports %s from a same-origin blob it revokes once the worker starts", async (url) => {
    createModuleWorker(new URL(url));

    const [blobUrl, blob] = [...blobs][0] as [string, Blob];
    expect(created).toMatchObject([{ options: { type: "module" }, url: blobUrl }]);
    expect(await blob.text()).toBe(`import ${JSON.stringify(url)};`);
    expect(revoked).toEqual([]);
    created[0]?.worker.dispatchEvent(new Event("message"));
    expect(revoked).toEqual([blobUrl]);
  });

  it("revokes the blob when the worker fails to start", () => {
    createModuleWorker("https://cdn.test/worker.js");

    created[0]?.worker.dispatchEvent(new Event("error"));
    expect(revoked).toEqual([[...blobs.keys()][0]]);
  });
});
