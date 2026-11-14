import { describe, expect, it } from "vitest";

import { readWasmError } from "../src/wasm-error.js";

describe("readWasmError", () => {
  it("reads a plain string as having no fields", () => {
    expect(readWasmError("composition not loaded")).toEqual({
      diagnostics: [],
      frameNotLoaded: false,
    });
  });

  it("reads an error with nothing set as having no fields", () => {
    expect(readWasmError(new Error("boom"))).toEqual({ diagnostics: [], frameNotLoaded: false });
  });

  it("reads the code and media failure of a failed file", () => {
    const media = { code: "media.decode", source: "hero.glb", wanted: ["model"], details: "bad" };
    const error = Object.assign(new Error("bad"), { code: "media.decode", media });

    expect(readWasmError(error)).toEqual({
      code: "media.decode",
      media,
      diagnostics: [],
      frameNotLoaded: false,
    });
    expect(
      readWasmError(Object.assign(new Error("x"), { media: { code: "media.decode" } })),
    ).toEqual({ diagnostics: [], frameNotLoaded: false });
  });

  it("reads the diagnostics of a composition that doesn't parse", () => {
    const diagnostics = [{ severity: "error", code: "document.invalid", path: "", message: "no" }];
    const error = Object.assign(new Error("no"), { code: "document.invalid", diagnostics });

    expect(readWasmError(error)).toMatchObject({ code: "document.invalid", diagnostics });
  });

  it("reads a frame that is not loaded yet", () => {
    const error = Object.assign(new Error("frame 3"), {
      code: "media.frame_out_of_range",
      frameNotLoaded: true,
    });

    expect(readWasmError(error)).toMatchObject({
      code: "media.frame_out_of_range",
      frameNotLoaded: true,
    });
  });
});
