import { afterEach, beforeEach, vi } from "vitest";

import type { LumenComposition } from "@lumiscia/lumen-types";

import { LumenPreviewContext } from "../../src/preview.js";
import { LumenPreviewSession } from "../../src/session/session.js";
import { FakeWorker } from "../support/fake-worker.js";
import { audio } from "./fake-audio-engine.js";

export class FakeCanvas {
  removed = false;
  style = {};
  remove() {
    this.removed = true;
  }
  transferControlToOffscreen() {
    return {};
  }
}

/** The plan the engine serializes. */
export function audioTimeline(clip: string | null = "music"): string {
  return JSON.stringify({
    segments: clip === null ? [] : [{ voice: 0 }],
    voices: clip === null ? [] : [{ sourceId: clip }],
  });
}

export function composition(name: string): LumenComposition {
  return { metadata: { name } } as unknown as LumenComposition;
}

/** A composition that can't be read, which fails before anything is sent. */
export function unreadable() {
  return {
    toJSON(): LumenComposition {
      throw new Error("the composition could not be read");
    },
  };
}

export const RUNTIME = {
  audioWorker: { file: "audio-worker.js", url: "https://a.test/audio-worker.js" },
  audioWorklet: { file: "audio-worklet.js", url: "https://a.test/audio-worklet.js" },
  wasm: { file: "lumen_wasm_2d_bg.wasm", url: "https://a.test/lumen.wasm" },
  worker: { file: "worker-2d.js", url: "https://a.test/worker.js" },
};

export function createSession(
  inputs: ConstructorParameters<typeof LumenPreviewSession>[0] | object = {},
) {
  const preview = new LumenPreviewContext();
  const canvases: unknown[] = [];
  const host = { append: (canvas: unknown) => canvases.push(canvas) } as unknown as HTMLElement;
  const session = new LumenPreviewSession({
    composition: composition("first"),
    runtime: RUNTIME,
    preview,
    ...inputs,
  });
  const attached = session.attach(host);
  const worker = FakeWorker.instances.at(-1) as FakeWorker;
  return { attached, canvases, preview, session, worker };
}

export async function started(inputs?: object) {
  const fake = createSession(inputs);
  fake.worker.emit({ type: "ready" });
  await fake.attached;
  return fake;
}

export async function playable(
  inputs?: object,
  audioTimelineJson: string | null = audioTimeline(),
) {
  const fake = await started(inputs);
  const { loadId } = fake.worker.ofType("load").at(-1) as { loadId: number };
  fake.worker.emit({
    audioTimelineJson,
    fps: 10,
    height: 720,
    loadId,
    totalFrames: 100,
    type: "loaded",
    width: 1280,
  });
  return fake;
}

/** Stubs the browser a session runs in for each test, and puts it back after. */
export function useSessionEnvironment() {
  beforeEach(() => {
    FakeWorker.instances = [];
    audio.clockRunning = false;
    audio.instances = [];
    audio.playing = false;
    audio.positionMs = 0;
    vi.spyOn(console, "error").mockImplementation(() => {});
    vi.stubGlobal("Worker", FakeWorker);
    vi.stubGlobal("OffscreenCanvas", class {});
    vi.stubGlobal("HTMLCanvasElement", FakeCanvas);
    vi.stubGlobal("navigator", { gpu: {} });
    vi.stubGlobal("location", { origin: "https://a.test" });
    vi.stubGlobal("origin", "https://a.test");
    vi.stubGlobal("document", {
      baseURI: "https://a.test/",
      createElement: () => new FakeCanvas(),
    });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });
}
