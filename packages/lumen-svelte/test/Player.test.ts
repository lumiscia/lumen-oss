// @vitest-environment jsdom

import { flushSync, mount, unmount } from "svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import * as defaults from "@lumiscia/lumen-bindings/urls";
import type { LumenPreviewComposition } from "@lumiscia/lumen-preview";

import { createLumenPreview } from "../src/lib/preview.svelte.js";
import Harness from "./PlayerHarness.svelte";

interface MockSession {
  host: HTMLElement;
  options: {
    loop?: boolean;
    preview: unknown;
    rate?: number;
    wasmUrl: unknown;
    workerUrl: unknown;
  };
  dispose: ReturnType<typeof vi.fn>;
  update: ReturnType<typeof vi.fn>;
}

const sessions = vi.hoisted(() => [] as MockSession[]);

vi.mock("@lumiscia/lumen-preview", () => ({
  mountPreview: (host: HTMLElement, options: MockSession["options"]) => {
    const session = { dispose: vi.fn(), host, options, update: vi.fn() };
    sessions.push(session);
    return session;
  },
}));

describe("Player", () => {
  beforeEach(() => {
    sessions.length = 0;
    document.body.replaceChildren();
  });

  it("mounts into its host and replaces the session with a new preview", async () => {
    const firstPreview = createLumenPreview();
    const nextPreview = createLumenPreview();
    const component = mount(Harness, {
      target: document.body,
      props: { initialPreview: firstPreview },
    });
    flushSync();
    expect(sessions[0]?.host.className).toBe("stage");
    expect(sessions[0]?.options).toMatchObject({
      wasmUrl: defaults.wasmUrl.href,
      workerUrl: defaults.workerUrl.href,
    });

    component.setPreview(nextPreview);
    flushSync();
    expect(sessions[0]?.dispose).toHaveBeenCalledOnce();
    expect(sessions[1]?.options.preview).toBe(nextPreview);

    await unmount(component);
    expect(sessions[1]?.dispose).toHaveBeenCalledOnce();
  });

  it("sends a changed input to the current session", async () => {
    const component = mount(Harness, {
      target: document.body,
      props: { initialPreview: createLumenPreview() },
    });
    flushSync();
    const composition = { toJSON: () => ({}) } as unknown as LumenPreviewComposition;

    component.setComposition(composition);
    flushSync();
    expect(sessions).toHaveLength(1);
    expect(sessions[0]?.update).toHaveBeenLastCalledWith(expect.objectContaining({ composition }));

    await unmount(component);
  });

  it("updates the playback options of the current session without mounting again", async () => {
    const component = mount(Harness, {
      target: document.body,
      props: { initialPreview: createLumenPreview() },
    });
    flushSync();
    const onEnded = vi.fn();
    const onTimeUpdate = vi.fn();

    component.setOptions({
      loop: true,
      muted: true,
      onEnded,
      onTimeUpdate,
      range: { in: 10, out: 40 },
      rate: 2,
      volume: 0.5,
    });
    flushSync();
    expect(sessions).toHaveLength(1);
    expect(sessions[0]?.dispose).not.toHaveBeenCalled();
    expect(sessions[0]?.update).toHaveBeenLastCalledWith(
      expect.objectContaining({
        loop: true,
        muted: true,
        onEnded,
        onTimeUpdate,
        range: { in: 10, out: 40 },
        rate: 2,
        volume: 0.5,
      }),
    );

    component.setOptions({ rate: 0.5 });
    flushSync();
    expect(sessions[0]?.update).toHaveBeenLastCalledWith(
      expect.objectContaining({ loop: undefined, rate: 0.5, range: undefined }),
    );

    await unmount(component);
  });

  it("starts a session with the playback options", async () => {
    const component = mount(Harness, {
      target: document.body,
      props: { initialPreview: createLumenPreview(), initialOptions: { loop: true, rate: 2 } },
    });
    flushSync();
    expect(sessions[0]?.options).toMatchObject({ loop: true, rate: 2 });

    await unmount(component);
  });

  it("exposes the core state through one reactive snapshot", () => {
    const preview = createLumenPreview();
    preview.update({ frame: 12, totalFrames: 90 });
    expect(preview.state.frame).toBe(12);
    expect(preview.state.totalFrames).toBe(90);
  });
});
