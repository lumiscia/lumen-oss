// @vitest-environment jsdom

import { flushSync, mount, unmount } from "svelte";
import { beforeEach, describe, expect, it, vi } from "vitest";

import * as defaults from "@lumiscia/lumen-bindings/urls";
import * as defaults2d from "@lumiscia/lumen-bindings/urls-2d";
import type { LumenPreviewComposition } from "@lumiscia/lumen-preview";

import Player2d from "../src/lib/Player2d.svelte";
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
  setInputs: ReturnType<typeof vi.fn>;
}

const sessions = vi.hoisted(() => [] as MockSession[]);

vi.mock("@lumiscia/lumen-preview", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@lumiscia/lumen-preview")>()),
  mountPreview: (host: HTMLElement, options: MockSession["options"]) => {
    const session = { dispose: vi.fn(), host, options, setInputs: vi.fn() };
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

  it("finds the runtime files in runtimeUrl when it is set", async () => {
    const component = mount(Harness, {
      target: document.body,
      props: { initialOptions: { runtimeUrl: "/lumen/" }, initialPreview: createLumenPreview() },
    });
    flushSync();
    expect(sessions[0]?.options).toMatchObject({
      runtimeUrl: "/lumen/",
      wasmUrl: new URL("/lumen/lumen_wasm_bg.wasm", document.baseURI).href,
      workerUrl: new URL("/lumen/worker.js", document.baseURI).href,
    });

    await unmount(component);
  });

  it("starts the 2D wasm from the 2d entry point", async () => {
    const component = mount(Harness, {
      target: document.body,
      props: { initialPreview: createLumenPreview(), player: Player2d },
    });
    flushSync();
    expect(sessions[0]?.options).toMatchObject({
      wasmUrl: defaults2d.wasmUrl.href,
      workerUrl: defaults2d.workerUrl.href,
    });
    await unmount(component);

    const inRuntime = mount(Harness, {
      target: document.body,
      props: {
        initialOptions: { runtimeUrl: "/lumen" },
        initialPreview: createLumenPreview(),
        player: Player2d,
      },
    });
    flushSync();
    expect(sessions[1]?.options).toMatchObject({
      wasmUrl: new URL("/lumen/lumen_wasm_2d_bg.wasm", document.baseURI).href,
      workerUrl: new URL("/lumen/worker-2d.js", document.baseURI).href,
    });
    await unmount(inRuntime);
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
    expect(sessions[0]?.setInputs).toHaveBeenLastCalledWith(
      expect.objectContaining({ composition }),
    );

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
    expect(sessions[0]?.setInputs).toHaveBeenLastCalledWith(
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
    expect(sessions[0]?.setInputs).toHaveBeenLastCalledWith(
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
    const before = preview.state;
    void preview.seek(12);
    expect(preview.state.frame).toBe(12);
    expect(preview.state).toBe(preview.getSnapshot());
    expect(preview.state).not.toBe(before);
  });
});
