import { describe, expect, it } from "vitest";

import { assetHint } from "../src/asset-hint.js";

describe("assetHint", () => {
  it("names the Vite setting only for a pre-bundled path", () => {
    expect(
      assetHint("http://localhost:5173/node_modules/.vite/deps/dist/browser/worker.js"),
    ).toContain("optimizeDeps");
    expect(assetHint("https://cdn.test/worker.js?from=/.vite/deps/")).toContain("pass `workerUrl`");
  });

  it("uses the given fallback elsewhere", () => {
    expect(assetHint("https://cdn.test/audio-worker.js", "audio fix")).toBe("audio fix");
  });
});
