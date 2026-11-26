// @vitest-environment jsdom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { runtime } from "@lumiscia/lumen-bindings/urls";
import { runtime2d } from "@lumiscia/lumen-bindings/urls-2d";
import { createLumenPreview } from "@lumiscia/lumen-preview/preview";

interface MockSession {
  host: HTMLElement;
  options: {
    composition?: unknown;
    onEnded?: () => void;
    preview: unknown;
    runtime: unknown;
    runtimeUrl?: unknown;
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

import { Player as Player2d } from "../src/2d.js";
import { Player } from "../src/index.js";

// These cover the wiring between props and a mocked session. Player.lifecycle.test.tsx runs the
// real one, for what actually gets cleaned up.
describe("Player", () => {
  beforeEach(() => {
    sessions.length = 0;
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("mounts into its host and replaces the session with a new preview", async () => {
    const firstPreview = createLumenPreview();
    const nextPreview = createLumenPreview();
    const container = document.createElement("div");
    const root = createRoot(container);

    await act(async () => {
      root.render(<Player preview={firstPreview} className="stage" />);
    });
    expect(sessions[0]?.host.className).toBe("stage");

    await act(async () => {
      root.render(<Player preview={nextPreview} composition={null} />);
    });
    expect(sessions[0]?.dispose).toHaveBeenCalledOnce();
    expect(sessions[1]?.options.preview).toBe(nextPreview);

    await act(async () => root.unmount());
    expect(sessions[1]?.dispose).toHaveBeenCalledOnce();
  });

  it("sends a changed input to the current session only", async () => {
    const preview = createLumenPreview();
    const root = createRoot(document.createElement("div"));
    const composition = { toJSON: () => ({}) } as never;

    await act(async () => {
      root.render(<Player preview={preview} />);
    });
    await act(async () => {
      root.render(<Player preview={preview} composition={composition} />);
    });
    expect(sessions).toHaveLength(1);
    expect(sessions[0]?.setInputs).toHaveBeenLastCalledWith(
      expect.objectContaining({ composition }),
    );

    await act(async () => root.unmount());
  });

  it("starts a session with the playback options, and updates them without mounting again", async () => {
    const preview = createLumenPreview();
    const root = createRoot(document.createElement("div"));
    const onEnded = vi.fn();
    const onTimeUpdate = vi.fn();
    const options = {
      loop: true,
      muted: true,
      range: { in: 10, out: 40 },
      rate: 2,
      volume: 0.5,
    };

    await act(async () => {
      root.render(
        <Player preview={preview} {...options} onEnded={onEnded} onTimeUpdate={onTimeUpdate} />,
      );
    });
    expect(sessions[0]?.options).toMatchObject(options);

    await act(async () => {
      root.render(
        <Player
          preview={preview}
          {...options}
          onEnded={onEnded}
          onTimeUpdate={onTimeUpdate}
          loop={false}
          muted={false}
          range={null}
          rate={0.5}
          volume={0.2}
        />,
      );
    });
    expect(sessions).toHaveLength(1);
    expect(sessions[0]?.dispose).not.toHaveBeenCalled();
    expect(sessions[0]?.setInputs).toHaveBeenLastCalledWith(
      expect.objectContaining({
        loop: false,
        muted: false,
        onEnded: expect.any(Function),
        onTimeUpdate: expect.any(Function),
        range: null,
        rate: 0.5,
        volume: 0.2,
      }),
    );

    await act(async () => root.unmount());
  });

  it("sends inputs again only when their content changed, and calls the newest callback", async () => {
    const preview = createLumenPreview();
    const root = createRoot(document.createElement("div"));
    const composition = { toJSON: () => ({}) } as never;
    const calls: string[] = [];
    const render = (out: number, label: string) =>
      root.render(
        <Player
          preview={preview}
          composition={composition}
          range={{ in: 0, out }}
          onEnded={() => calls.push(label)}
        />,
      );

    await act(async () => render(40, "first"));
    const setInputs = sessions[0]?.setInputs;
    const initial = setInputs?.mock.calls.length;

    await act(async () => render(40, "second"));
    expect(setInputs).toHaveBeenCalledTimes(initial ?? 0);

    await act(async () => render(60, "third"));
    expect(setInputs).toHaveBeenCalledTimes((initial ?? 0) + 1);
    expect(setInputs).toHaveBeenLastCalledWith(
      expect.objectContaining({ composition, range: { in: 0, out: 60 } }),
    );

    const onEnded = sessions[0]?.options.onEnded;
    onEnded?.();
    expect(calls).toEqual(["third"]);
    expect(setInputs?.mock.lastCall?.[0]).toMatchObject({ onEnded });

    await act(async () => root.unmount());
  });

  it("starts the runtime this package ships unless given another", async () => {
    const preview = createLumenPreview();
    const root = createRoot(document.createElement("div"));

    await act(async () => {
      root.render(<Player preview={preview} />);
    });
    expect(sessions[0]?.options.runtime).toBe(runtime);
    expect(String(runtime.worker.url)).toMatch(/\/lumen-bindings\/dist\/browser\/worker\.js$/);
    expect(String(runtime.wasm.url)).toMatch(/\/lumen-bindings\/wasm\/lumen_wasm_bg\.wasm$/);
    expect(String(runtime.audioWorker.url)).toMatch(/\/dist\/browser\/audio-worker\.js$/);
    expect(String(runtime.audioWorklet.url)).toMatch(/\/dist\/browser\/audio-worklet\.js$/);

    await act(async () => {
      root.render(<Player preview={preview} runtimeUrl="/lumen/" />);
    });
    expect(sessions[0]?.dispose).toHaveBeenCalledOnce();
    expect(sessions[1]?.options).toMatchObject({ runtime, runtimeUrl: "/lumen/" });

    const custom = () => ({ ...runtime, wasm: { file: "lumen.wasm", url: "/lumen.wasm" } });
    await act(async () => {
      root.render(<Player preview={preview} runtime={custom()} />);
    });
    expect(sessions[2]?.options.runtime).toEqual(custom());

    // A manifest rebuilt with the same files keeps the session.
    await act(async () => {
      root.render(<Player preview={preview} runtime={custom()} />);
    });
    expect(sessions).toHaveLength(3);

    await act(async () => root.unmount());
  });

  it("starts the 2D runtime from the 2d entry point", async () => {
    const preview = createLumenPreview();
    const root = createRoot(document.createElement("div"));

    await act(async () => {
      root.render(<Player2d preview={preview} />);
    });
    expect(sessions[0]?.options.runtime).toBe(runtime2d);
    expect(String(runtime2d.wasm.url)).toMatch(/\/lumen-bindings\/wasm\/lumen_wasm_2d_bg\.wasm$/);
    expect(String(runtime2d.worker.url)).toMatch(/\/lumen-bindings\/dist\/browser\/worker-2d\.js$/);

    await act(async () => root.unmount());
  });
});
