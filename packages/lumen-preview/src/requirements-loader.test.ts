import { describe, expect, it } from "vitest";

import { FrameRequirementLoader } from "./requirements-loader.js";

describe("FrameRequirementLoader", () => {
  it("reloads frames evicted while a joined prefetch was pending", async () => {
    const resident = new Set<string>();
    const pendingUploads = new Map<string, () => void>();
    const loader = new FrameRequirementLoader({
      durationFrames: () => 10,
      requirements: (frame) => JSON.stringify(["a", "b"].map((stream) => `${stream}:${frame}`)),
      load: async (json) => {
        for (const key of JSON.parse(json) as string[]) {
          if (resident.has(key)) {
            continue;
          }
          await new Promise<void>((resolve) => {
            if (key === "b:1" && !pendingUploads.has(key)) {
              pendingUploads.set(key, resolve);
            } else {
              resolve();
            }
          });
          resident.add(key);
        }
      },
    });

    const prefetch = loader.loadFrame(1);
    await Promise.resolve();
    await Promise.resolve();
    expect(resident.has("a:1")).toBe(true);

    resident.delete("a:1");
    const foreground = loader.loadFrame(1);
    pendingUploads.get("b:1")?.();
    await Promise.all([prefetch, foreground]);

    expect(resident.has("a:1")).toBe(true);
    expect(resident.has("b:1")).toBe(true);
  });
});
