import { describe, expect, it, vi } from "vitest";

import type { PlaybackRange } from "../src/playback-range.js";
import { FrameRequirementLoader } from "../src/requirements-loader.js";

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

  describe("lookahead", () => {
    async function prefetched(range: PlaybackRange | null, frame: number, lookahead: number) {
      const loaded: number[] = [];
      const loader = new FrameRequirementLoader({
        durationFrames: () => 10,
        requirements: (requested) => String(requested),
        load: async (json) => {
          loaded.push(Number(json));
        },
      });
      loader.setLookaheadCount(lookahead);
      loader.setRange(range);
      loader.prefetchWindow(frame);
      await vi.waitFor(() => expect(loaded.length).toBeGreaterThan(0));
      await new Promise((resolve) => setTimeout(resolve, 5));
      return loaded;
    }

    it("wraps round the timeline when no range limits it", async () => {
      expect(await prefetched(null, 8, 4)).toEqual([8, 9, 0, 1]);
    });

    it("stops at the end of a range that doesn't loop", async () => {
      expect(await prefetched({ inFrame: 2, loop: false, outFrame: 6 }, 4, 6)).toEqual([4, 5]);
    });

    it("carries on from the start of a range that loops", async () => {
      expect(await prefetched({ inFrame: 2, loop: true, outFrame: 6 }, 4, 6)).toEqual([
        4, 5, 2, 3, 4, 5,
      ]);
    });
  });
});
