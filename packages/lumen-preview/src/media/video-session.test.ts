import { ALL_FORMATS } from "mediabunny";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { DEFAULT_BLOB_CACHE_BYTES, DEFAULT_URL_CACHE_BYTES } from "./source.js";
import {
  DEFAULT_VIDEO_FRAME_CACHE_CAPACITY,
  VideoSession,
  createVideoRegistration,
} from "./video-session.js";

/** Uses real Mediabunny against a network read that never answers. */
describe("VideoSession disposal", () => {
  let fetches: AbortSignal[];

  function createSession(): VideoSession {
    return new VideoSession("clip", createVideoRegistration("https://a.test/v.mp4", { fps: 10 }), {
      blobCacheBytes: DEFAULT_BLOB_CACHE_BYTES,
      formats: ALL_FORMATS,
      frameCacheCapacity: DEFAULT_VIDEO_FRAME_CACHE_CAPACITY,
      urlCacheBytes: DEFAULT_URL_CACHE_BYTES,
    });
  }

  beforeEach(() => {
    fetches = [];
    vi.stubGlobal("fetch", (_url: unknown, init: RequestInit) => {
      fetches.push(init.signal as AbortSignal);
      return new Promise(() => {});
    });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("aborts a stalled read straight away and fails what was waiting on it", async () => {
    const session = createSession();
    const metadata = session.loadMetadata();
    const decode = session.decodeFrames([0]);
    await vi.waitFor(() => expect(fetches).toHaveLength(1));

    session.dispose();

    expect(fetches[0]?.aborted).toBe(true);
    await expect(metadata).rejects.toThrow();
    await expect(decode).rejects.toThrow();
  });

  it("opens nothing once disposed", async () => {
    const session = createSession();
    session.dispose();

    await expect(session.loadMetadata()).rejects.toThrow();
    await expect(session.decodeFrames([0])).resolves.toEqual(new Map());
    expect(fetches).toEqual([]);
  });
});
