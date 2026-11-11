import { ALL_FORMATS } from "mediabunny";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { LumenMediaError } from "../../src/media/errors.js";
import { errorMediaFailure } from "../../src/session/errors.js";
import { DEFAULT_BLOB_CACHE_BYTES, DEFAULT_URL_CACHE_BYTES } from "../../src/media/source.js";
import {
  DEFAULT_VIDEO_FRAME_CACHE_CAPACITY,
  VideoSession,
  createVideoRegistration,
} from "../../src/media/video-session.js";

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

/** A WAV file of `samples` 16-bit mono samples of silence. */
function wav(samples: number): Uint8Array<ArrayBuffer> {
  const bytes = new Uint8Array(44 + samples * 2);
  const view = new DataView(bytes.buffer);
  const text = (at: number, value: string) =>
    [...value].forEach((char, index) => view.setUint8(at + index, char.charCodeAt(0)));
  text(0, "RIFF");
  view.setUint32(4, bytes.length - 8, true);
  text(8, "WAVEfmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true);
  view.setUint16(22, 1, true);
  view.setUint32(24, 8000, true);
  view.setUint32(28, 16000, true);
  view.setUint16(32, 2, true);
  view.setUint16(34, 16, true);
  text(36, "data");
  view.setUint32(40, samples * 2, true);
  return bytes;
}

describe("VideoSession failures", () => {
  async function failure(bytes: Uint8Array<ArrayBuffer>) {
    const session = new VideoSession(
      "clip",
      createVideoRegistration({ kind: "buffer", bytes }, { fps: 10 }),
      {
        blobCacheBytes: DEFAULT_BLOB_CACHE_BYTES,
        formats: ALL_FORMATS,
        frameCacheCapacity: DEFAULT_VIDEO_FRAME_CACHE_CAPACITY,
        urlCacheBytes: DEFAULT_URL_CACHE_BYTES,
      },
    );
    const error = await session.loadMetadata().catch((caught: unknown) => caught);
    session.dispose();
    expect(error).toBeInstanceOf(LumenMediaError);
    return errorMediaFailure(error);
  }

  it("says an audio-only file is the wrong kind, as the engine does", async () => {
    expect(await failure(wav(800))).toMatchObject({
      code: "media.wrong_kind",
      source: "clip",
      wanted: ["video"],
      found: "audio",
    });
  });

  it("says a file in no format it identifies is corrupt, not unsupported", async () => {
    const failed = await failure(
      Uint8Array.from([..."these bytes are no media at all"].map((char) => char.charCodeAt(0))),
    );
    expect(failed).toMatchObject({ code: "media.decode", source: "clip", wanted: ["video"] });
    expect(failed).not.toHaveProperty("found");
  });
});
