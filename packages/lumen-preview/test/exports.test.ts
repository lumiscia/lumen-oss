import { describe, expect, it } from "vitest";

import * as advanced from "../src/advanced.js";
import * as root from "../src/index.js";

describe("public exports", () => {
  it("pins the values the root entry exports", () => {
    expect(Object.keys(root).sort()).toEqual([
      "LumenPreviewContext",
      "LumenPreviewSession",
      "createLumenPreview",
      "mountPreview",
    ]);
  });

  it("exports only types from the advanced entry", () => {
    expect(Object.keys(advanced)).toEqual([]);
  });
});
