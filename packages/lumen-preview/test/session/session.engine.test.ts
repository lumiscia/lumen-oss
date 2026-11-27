import { describe, expect, it, vi } from "vitest";

import { composition, createSession, started, useSessionEnvironment } from "./support.js";

vi.mock("../../src/audio-engine.js", async () => ({
  LumenAudioEngine: (await import("./fake-audio-engine.js")).FakeAudioEngine,
}));

describe("LumenPreviewSession: engine calls", () => {
  useSessionEnvironment();

  it("runs each call in the worker once it is ready, and resolves with its reply", async () => {
    const { attached, session, worker } = createSession();
    const checked = session.check(composition("other"), ["Inter"]);
    expect(worker.ofType("engine-call")).toHaveLength(0);

    worker.emit({ type: "ready" });
    await attached;
    const evaluated = session.evaluate(composition("other"), { frame: 3, node: 2, param: "x" });
    const inventory = session.mediaDependencies(composition("other"));
    const posted = worker.ofType("engine-call");
    expect(posted.map(({ call }) => call)).toEqual([
      { compositionJson: '{"metadata":{"name":"other"}}', fonts: ["Inter"], kind: "check" },
      {
        compositionJson: '{"metadata":{"name":"other"}}',
        kind: "evaluate",
        request: { frame: 3, node: 2, param: "x" },
      },
      { compositionJson: '{"metadata":{"name":"other"}}', kind: "media-dependencies" },
    ]);

    const results = [
      [{ code: "font.fallback", message: "Inter", path: "", severity: "warning" }],
      12,
      { dynamic: [], fonts: [], media: [] },
    ];
    for (const [index, { requestId }] of posted.entries()) {
      worker.emit({ error: null, requestId, result: results[index], type: "engine-call-settled" });
    }
    await expect(Promise.all([checked, evaluated, inventory])).resolves.toEqual(results);
    expect(worker.ofType("load")).toHaveLength(1);
  });

  it("rejects with the engine's message and diagnostics when the call throws", async () => {
    const { session, worker } = await started();
    const inventory = session.mediaDependencies(composition("broken"));
    const diagnostics = [
      { code: "document.invalid", message: "bad", path: "/nodes", severity: "error" as const },
    ];
    worker.emit({
      error: { code: "document.invalid", diagnostics, message: "the composition is invalid" },
      requestId: worker.ofType("engine-call")[0]!.requestId,
      result: null,
      type: "engine-call-settled",
    });

    await expect(inventory).rejects.toThrow("the composition is invalid");
    await expect(inventory).rejects.toMatchObject({ code: "document.invalid", diagnostics });
  });

  it("rejects a call made after the worker failed to start, and waits again after a new attach", async () => {
    const { attached, session, worker } = createSession();
    worker.emit({ code: "webgpu_unavailable", message: "no adapter", type: "startup-failed" });
    await attached;

    await expect(session.check(composition("other"))).rejects.toThrow("no adapter");

    void session.attach(document.createElement("div"));
    const checked = session.check(composition("other"));
    session.dispose();
    await expect(checked).rejects.toMatchObject({ name: "AbortError" });
  });

  it("rejects a call still pending when the session is disposed", async () => {
    const { session } = await started();
    const checked = session.check(composition("other"));
    session.dispose();

    await expect(checked).rejects.toMatchObject({
      message: "the preview stopped",
      name: "AbortError",
    });
  });
});
