// @vitest-environment jsdom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import * as defaults from "@lumiscia/lumen-bindings/urls";
import { createLumenPreview } from "@lumiscia/lumen-preview/preview";

interface MockSession {
  host: HTMLElement;
  options: { composition?: unknown; preview: unknown; wasmUrl: unknown; workerUrl: unknown };
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

import { Player } from "../src/Player.js";

// These cover the wiring between props and a mocked session. Player.lifecycle.test.tsx runs the
// real one, for what actually gets cleaned up.
describe("Player", () => {
  beforeEach(() => {
    sessions.length = 0;
    globalThis.IS_REACT_ACT_ENVIRONMENT = true;
  });

  afterEach(() => {
    delete globalThis.IS_REACT_ACT_ENVIRONMENT;
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
    expect(sessions[0]?.update).toHaveBeenLastCalledWith(expect.objectContaining({ composition }));

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
      onEnded,
      onTimeUpdate,
      range: { in: 10, out: 40 },
      rate: 2,
      volume: 0.5,
    };

    await act(async () => {
      root.render(<Player preview={preview} {...options} />);
    });
    expect(sessions[0]?.options).toMatchObject(options);

    await act(async () => {
      root.render(
        <Player
          preview={preview}
          {...options}
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
    expect(sessions[0]?.update).toHaveBeenLastCalledWith(
      expect.objectContaining({
        loop: false,
        muted: false,
        onEnded,
        onTimeUpdate,
        range: null,
        rate: 0.5,
        volume: 0.2,
      }),
    );

    await act(async () => root.unmount());
  });

  it("doesn't update the session for a range that is equal but a new object", async () => {
    const preview = createLumenPreview();
    const root = createRoot(document.createElement("div"));

    await act(async () => {
      root.render(<Player preview={preview} range={{ in: 10, out: 40 }} />);
    });
    const updates = sessions[0]?.update.mock.calls.length;
    await act(async () => {
      root.render(<Player preview={preview} range={{ in: 10, out: 40 }} />);
    });
    expect(sessions[0]?.update).toHaveBeenCalledTimes(updates ?? 0);

    await act(async () => {
      root.render(<Player preview={preview} range={{ in: 10, out: 50 }} />);
    });
    expect(sessions[0]?.update).toHaveBeenLastCalledWith(
      expect.objectContaining({ range: { in: 10, out: 50 } }),
    );

    await act(async () => root.unmount());
  });

  it("starts the worker this package ships unless told where another is", async () => {
    const preview = createLumenPreview();
    const root = createRoot(document.createElement("div"));

    await act(async () => {
      root.render(<Player preview={preview} />);
    });
    expect(sessions[0]?.options).toMatchObject({
      wasmUrl: defaults.wasmUrl.href,
      workerUrl: defaults.workerUrl.href,
    });
    expect(defaults.workerUrl.pathname).toMatch(/\/lumen-bindings\/dist\/browser\/worker\.js$/);
    expect(defaults.wasmUrl.pathname).toMatch(/\/lumen-bindings\/wasm\/lumen_wasm_bg\.wasm$/);

    await act(async () => {
      root.render(<Player preview={preview} wasmUrl="/lumen.wasm" workerUrl="/worker.js" />);
    });
    expect(sessions[0]?.dispose).toHaveBeenCalledOnce();
    expect(sessions[1]?.options).toMatchObject({ wasmUrl: "/lumen.wasm", workerUrl: "/worker.js" });

    await act(async () => root.unmount());
  });
});
