import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { LumenMediaBridge } from "../../src/media/bridge.js";
import type { NativeVideoFrameTarget } from "../../src/media/types.js";

type FakeSample = {
  clone(): FakeSample;
  close(): void;
  displayHeight: number;
  displayWidth: number;
  rotation: number;
  timestamp: number;
  toVideoFrame(): FakeFrame;
};
type FakeFrame = {
  close: () => void;
  closed: boolean;
  copyTo: (rgba: Uint8Array) => Promise<void>;
};

const mock = vi.hoisted(() => ({
  frames: [] as { closed: boolean }[],
  inputs: [] as { disposed: boolean }[],
  openFailure: null as Error | null,
  // Decoded by the fake sink, one per request.
  sampleStream: null as ((timestamp: number) => AsyncGenerator<unknown, void, unknown>) | null,
}));

vi.mock("mediabunny", () => {
  class FakeSource {}
  return {
    ALL_FORMATS: [],
    BlobSource: FakeSource,
    BufferSource: FakeSource,
    UrlSource: FakeSource,
    Input: class {
      disposed = false;
      constructor() {
        mock.inputs.push(this);
      }
      dispose() {
        this.disposed = true;
      }
      getMimeType = async () => "video/mp4";
      getPrimaryVideoTrack = async () => {
        if (mock.openFailure) {
          throw mock.openFailure;
        }
        return {
          canDecode: async () => true,
          computeDuration: async () => 10,
          displayHeight: 1,
          displayWidth: 1,
          getCodecParameterString: async () => "avc1",
          getFirstTimestamp: async () => 0,
          id: 1,
          number: 1,
        };
      };
    },
    VideoSampleSink: class {
      samples = (timestamp: number) => mock.sampleStream!(timestamp);
      async *samplesAtTimestamps(timestamps: number[]) {
        for (const timestamp of timestamps) {
          const { value } = await mock.sampleStream!(timestamp).next();
          yield value as FakeSample;
        }
      }
    },
  };
});

function fakeFrame(): FakeFrame {
  const frame: FakeFrame = {
    close: () => {
      frame.closed = true;
    },
    closed: false,
    copyTo: async () => {},
  };
  mock.frames.push(frame);
  return frame;
}

function fakeSample(timestamp: number): FakeSample & { closed: boolean } {
  const sample = {
    clone: () => fakeSample(timestamp),
    close: () => {
      sample.closed = true;
    },
    closed: false,
    displayHeight: 1,
    displayWidth: 1,
    rotation: 0,
    timestamp,
    toVideoFrame: fakeFrame,
  };
  return sample;
}

/** Yields consecutive samples at 10 fps, throwing once `failAt` is reached. */
function steadySamples(failAt = Number.POSITIVE_INFINITY) {
  return async function* (start: number) {
    for (let frame = Math.round(start * 10); ; frame += 1) {
      if (frame >= failAt) {
        throw new Error("decoder failed");
      }
      yield fakeSample(frame / 10);
    }
  };
}

function fakeTarget(overrides: Partial<NativeVideoFrameTarget> = {}) {
  const frames = new Set<string>();
  const images = new Map<string, number>();
  const target = {
    clear: () => {},
    clearVideoSource: () => {},
    clearVideos: () => frames.clear(),
    hasImage: (id: string) => images.has(id),
    hasVideoFrame: (id: string, frame: number) => frames.has(`${id}:${frame}`),
    removeImageSource: (id: string) => images.delete(id),
    removeVideoSource: (id: string) => {
      for (const key of frames) {
        if (key.startsWith(`${id}:`)) frames.delete(key);
      }
    },
    setImage: vi.fn((id: string, width: number) => images.set(id, width)),
    setVideoFrame: vi.fn((id: string, frame: number) => frames.add(`${id}:${frame}`)),
    setVideoMetadata: vi.fn(),
    ...overrides,
  };
  return { frames, target };
}

const FPS = { fps: 10 };

function deferred<T = void>() {
  let resolve: (value: T) => void = () => {};
  let reject: (reason: unknown) => void = () => {};
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, reject, resolve };
}

describe("LumenMediaBridge", () => {
  beforeEach(() => {
    mock.frames.length = 0;
    mock.inputs.length = 0;
    mock.openFailure = null;
    mock.sampleStream = steadySamples();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  describe("video frames", () => {
    it("closes every frame collected before a decode fails, on both decode paths", async () => {
      const { target } = fakeTarget();
      const bridge = new LumenMediaBridge(target);
      await bridge.registerVideoSource("clip", "clip.mp4", FPS);

      mock.sampleStream = steadySamples(3);
      await expect(
        bridge.loadFrameRequirements({
          images: [],
          videos: [{ streamId: "clip", frames: [0, 1, 2, 3] }],
        }),
      ).rejects.toThrow();
      expect(mock.frames.length).toBeGreaterThan(0);
      expect(mock.frames.every((frame) => frame.closed)).toBe(true);

      // Going back to a frame before the last decoded one uses random access.
      mock.sampleStream = steadySamples();
      await bridge.loadVideoFrame("clip", 40);
      mock.frames.length = 0;
      mock.sampleStream = steadySamples(6);
      await expect(
        bridge.loadFrameRequirements({
          images: [],
          videos: [{ streamId: "clip", frames: [4, 5, 6] }],
        }),
      ).rejects.toThrow();
      expect(mock.frames).toHaveLength(2);
      expect(mock.frames.every((frame) => frame.closed)).toBe(true);
    });

    it("closes the frames it never uploaded when an upload fails", async () => {
      const { target } = fakeTarget({
        setVideoFrame: () => {
          throw new Error("upload failed");
        },
      });
      const bridge = new LumenMediaBridge(target);
      await bridge.registerVideoSource("clip", "clip.mp4", FPS);

      await expect(
        bridge.loadFrameRequirements({
          images: [],
          videos: [{ streamId: "clip", frames: [0, 1, 2] }],
        }),
      ).rejects.toThrow("upload failed");

      expect(mock.frames).toHaveLength(3);
      expect(mock.frames.every((frame) => frame.closed)).toBe(true);
    });

    it("drops frames decoded for a source that was replaced under the same id", async () => {
      const { target } = fakeTarget();
      const bridge = new LumenMediaBridge(target);
      await bridge.registerVideoSource("clip", "old.mp4", FPS);

      const gate = deferred();
      mock.sampleStream = async function* (start) {
        await gate.promise;
        yield* steadySamples()(start);
      };
      const stale = bridge.loadVideoFrame("clip", 1);
      await Promise.resolve();

      bridge.removeVideoSource("clip");
      await bridge.registerVideoSource("clip", "new.mp4", FPS);
      gate.resolve();
      await stale;

      expect(target.setVideoFrame).not.toHaveBeenCalled();
      expect(mock.frames.every((frame) => frame.closed)).toBe(true);
    });

    it("drops a frame whose source is replaced while its pixels are being read", async () => {
      const { target } = fakeTarget();
      const bridge = new LumenMediaBridge(target);
      await bridge.registerVideoSource("clip", "old.mp4", FPS);

      const gate = deferred();
      const read = fakeFrame();
      read.copyTo = () => gate.promise;
      mock.sampleStream = async function* () {
        yield { ...fakeSample(0), clone: () => ({ ...fakeSample(0), toVideoFrame: () => read }) };
        yield fakeSample(0.1);
      };
      const stale = bridge.loadVideoFrame("clip", 0);
      await vi.waitFor(() => expect(mock.frames.length).toBeGreaterThan(0));
      await new Promise((resolve) => setTimeout(resolve));

      bridge.removeVideoSource("clip");
      gate.resolve();
      await stale;

      expect(target.setVideoFrame).not.toHaveBeenCalled();
      expect(read.closed).toBe(true);
    });

    it("asks the target which frames it still holds", async () => {
      const { frames, target } = fakeTarget();
      const bridge = new LumenMediaBridge(target);
      await bridge.registerVideoSource("clip", "clip.mp4", FPS);

      await bridge.loadVideoFrame("clip", 0);
      await bridge.loadVideoFrame("clip", 0);
      expect(target.setVideoFrame).toHaveBeenCalledTimes(1);

      frames.clear();
      await bridge.loadVideoFrame("clip", 0);
      expect(target.setVideoFrame).toHaveBeenCalledTimes(2);
    });
  });

  describe("video sessions", () => {
    it("always disposes the input", async () => {
      const bridge = new LumenMediaBridge(fakeTarget().target);
      await bridge.registerVideoSource("clip", "clip.mp4", FPS);
      await bridge.loadVideoFrame("clip", 0);

      bridge.removeVideoSource("clip");

      await vi.waitFor(() => expect(mock.inputs[0]?.disposed).toBe(true));
    });

    it("retries a source that failed to open and disposes the failed input", async () => {
      const bridge = new LumenMediaBridge(fakeTarget().target);

      mock.openFailure = new Error("offline");
      await expect(bridge.registerVideoSource("clip", "clip.mp4", FPS)).rejects.toThrow("offline");
      expect(mock.inputs[0]?.disposed).toBe(true);

      mock.openFailure = null;
      await expect(bridge.registerVideoSource("clip", "clip.mp4", FPS)).resolves.toMatchObject({
        fps: 10,
      });
      expect(mock.inputs).toHaveLength(2);
    });

    it("waits for a decode in flight before dropping its samples", async () => {
      const { target } = fakeTarget();
      const bridge = new LumenMediaBridge(target);
      await bridge.registerVideoSource("clip", "clip.mp4", FPS);

      const gate = deferred();
      const held = fakeSample(0.1);
      mock.sampleStream = async function* () {
        yield fakeSample(0);
        yield held;
        await gate.promise;
        yield fakeSample(0.2);
      };
      await bridge.loadVideoFrame("clip", 0);

      const decoding = bridge.loadVideoFrame("clip", 1);
      await new Promise((resolve) => setTimeout(resolve));
      bridge.clearVideos();
      expect(held.closed).toBe(false);

      gate.resolve();
      await decoding;
      await vi.waitFor(() => expect(held.closed).toBe(true));
      expect(target.setVideoFrame).toHaveBeenCalledTimes(2);
    });

    it("drops a frame load whose source is removed and whose read is cancelled", async () => {
      const bridge = new LumenMediaBridge(fakeTarget().target);
      await bridge.registerVideoSource("clip", "clip.mp4", FPS);
      const gate = deferred();
      mock.sampleStream = async function* () {
        await gate.promise;
        yield fakeSample(0);
      };

      const loading = bridge.loadVideoFrame("clip", 0);
      await new Promise((resolve) => setTimeout(resolve));
      bridge.removeVideoSource("clip");
      gate.reject(new Error("Input has been disposed."));

      await expect(loading).resolves.toBeUndefined();
    });

    it("keeps a session for equal inputs and replaces it for a different request", async () => {
      const bridge = new LumenMediaBridge(fakeTarget().target);

      await bridge.registerVideoSource("clip", { kind: "url", url: "https://a.test/v.mp4" }, FPS);
      await bridge.registerVideoSource("clip", { kind: "url", url: "https://a.test/v.mp4" }, FPS);
      expect(mock.inputs).toHaveLength(1);

      const request = (token: string) =>
        new Request("https://a.test/v.mp4", { headers: { Authorization: token } });
      await bridge.registerVideoSource("clip", request("a"), FPS);
      expect(mock.inputs).toHaveLength(2);
      await bridge.registerVideoSource("clip", request("b"), FPS);
      expect(mock.inputs).toHaveLength(3);
    });
  });

  describe("whole syncs", () => {
    function stubImages(decodes: Map<string, Promise<void>>) {
      vi.stubGlobal("createImageBitmap", async (blob: Blob) => {
        const name = await blob.text();
        await decodes.get(name);
        return { close: () => {}, height: 1, width: Number(name) };
      });
      vi.stubGlobal(
        "OffscreenCanvas",
        class {
          getContext() {
            return {
              drawImage: () => {},
              getImageData: () => ({ data: new Uint8ClampedArray(4) }),
            };
          }
        },
      );
    }
    const image = (id: string, name: string) => ({
      id,
      kind: "image" as const,
      source: new Blob([name]),
    });
    const video = (id: string, url: string) => ({
      id,
      kind: "video" as const,
      source: url,
      ...FPS,
    });
    const uploads = (target: ReturnType<typeof fakeTarget>["target"]) =>
      vi.mocked(target.setImage).mock.calls.map(([id, width]) => [id, width]);

    it("stops an older sync from overwriting a newer image", async () => {
      const slow = deferred();
      stubImages(new Map([["1", slow.promise]]));
      const { target } = fakeTarget();
      const bridge = new LumenMediaBridge(target);

      const older = bridge.syncMediaSources([image("slow", "1"), image("logo", "2")]);
      await new Promise((resolve) => setTimeout(resolve));
      await bridge.syncMediaSources([image("logo", "3")]);
      slow.resolve();
      await older;

      expect(uploads(target)).toEqual([["logo", 3]]);
    });

    it("stops an older sync from overwriting a newer video", async () => {
      const slow = deferred();
      stubImages(new Map([["1", slow.promise]]));
      const bridge = new LumenMediaBridge(fakeTarget().target);

      const older = bridge.syncMediaSources([image("slow", "1"), video("clip", "old.mp4")]);
      await new Promise((resolve) => setTimeout(resolve));
      await bridge.syncMediaSources([video("clip", "new.mp4")]);
      slow.resolve();
      await older;

      expect(mock.inputs).toHaveLength(1);
      expect(mock.inputs[0]?.disposed).toBe(false);
    });

    it("registers and uploads nothing after clear", async () => {
      const slow = deferred();
      stubImages(new Map([["1", slow.promise]]));
      const { target } = fakeTarget();
      const bridge = new LumenMediaBridge(target);

      const older = bridge.syncMediaSources([
        image("slow", "1"),
        image("logo", "2"),
        video("clip", "clip.mp4"),
      ]);
      await new Promise((resolve) => setTimeout(resolve));
      bridge.clear();
      slow.resolve();
      await older;

      expect(uploads(target)).toEqual([]);
      expect(mock.inputs).toEqual([]);
    });

    it("doesn't report a registration it abandoned", async () => {
      const failing = deferred();
      stubImages(
        new Map([["1", failing.promise.then(() => Promise.reject(new Error("offline")))]]),
      );
      const bridge = new LumenMediaBridge(fakeTarget().target);

      const older = bridge.syncMediaSources([image("slow", "1")]);
      await new Promise((resolve) => setTimeout(resolve));
      bridge.clear();
      failing.resolve();

      await expect(older).resolves.toBeUndefined();
    });

    it("still reports a failure from the latest sync", async () => {
      stubImages(new Map([["1", Promise.reject(new Error("offline"))]]));
      const bridge = new LumenMediaBridge(fakeTarget().target);

      await expect(bridge.syncMediaSources([image("slow", "1")])).rejects.toThrow("offline");
    });
  });

  describe("images", () => {
    function stubImageDecoding(createImageBitmap: (blob: Blob) => Promise<unknown>) {
      vi.stubGlobal("createImageBitmap", createImageBitmap);
      vi.stubGlobal(
        "OffscreenCanvas",
        class {
          getContext() {
            return {
              drawImage: () => {},
              getImageData: () => ({ data: new Uint8ClampedArray(4) }),
            };
          }
        },
      );
    }
    const bitmap = (width: number) => ({ close: () => {}, height: 1, width });

    it("retries an image after a failed load", async () => {
      const { target } = fakeTarget();
      const bridge = new LumenMediaBridge(target);
      const source = new Blob(["image"]);

      stubImageDecoding(() => Promise.reject(new Error("offline")));
      await expect(bridge.registerImageSource("logo", source)).rejects.toThrow("offline");

      stubImageDecoding(async () => bitmap(2));
      await bridge.registerImageSource("logo", source);
      expect(target.setImage).toHaveBeenCalledOnce();

      await bridge.registerImageSource("logo", source);
      expect(target.setImage).toHaveBeenCalledOnce();
    });

    it("ignores a load from before the id was removed and registered again", async () => {
      const { target } = fakeTarget();
      const bridge = new LumenMediaBridge(target);
      const oldDecode = deferred<unknown>();
      const newDecode = deferred<unknown>();
      const decodes = [oldDecode, newDecode];
      stubImageDecoding(() => decodes.shift()!.promise);

      const stale = bridge.registerImageSource("logo", new Blob(["old"]));
      await new Promise((resolve) => setTimeout(resolve));
      bridge.removeImageSource("logo");
      const current = bridge.registerImageSource("logo", new Blob(["new"]));
      await new Promise((resolve) => setTimeout(resolve));

      newDecode.resolve(bitmap(2));
      await current;
      oldDecode.resolve(bitmap(1));
      await stale;

      expect(target.setImage).toHaveBeenCalledOnce();
      expect(vi.mocked(target.setImage).mock.calls[0]?.[1]).toBe(2);
    });
  });
});
