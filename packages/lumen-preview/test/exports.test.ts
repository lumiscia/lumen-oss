import { describe, expect, it } from "vitest";

import * as advanced from "../src/advanced.js";
import advancedSource from "../src/advanced.ts?raw";
import * as root from "../src/index.js";
import rootSource from "../src/index.ts?raw";

// The entry files hold only `export … from` statements, so the names they list are the exports.
function exportedNames(source: string): string[] {
  const statements = source
    .split(";")
    .map((statement) => statement.trim())
    .filter(Boolean);
  return statements
    .flatMap((statement) => {
      const list = /^export\s+(?:type\s+)?\{([^}]*)\}\s+from\s+"[^"]+"$/s.exec(statement);
      if (!list?.[1]) throw new Error(`not a re-export: ${statement}`);
      return list[1].split(",").map((name) => name.trim().replace(/^type\s+/, ""));
    })
    .filter(Boolean)
    .sort();
}

describe("public exports", () => {
  it("pins the names the root entry exports", () => {
    expect(exportedNames(rootSource)).toEqual([
      "BlobMediaSourceInput",
      "BufferMediaSourceInput",
      "LumenAudioEngineOptions",
      "LumenLogLevel",
      "LumenPreviewComposition",
      "LumenPreviewContext",
      "LumenPreviewError",
      "LumenPreviewErrorCode",
      "LumenPreviewFonts",
      "LumenPreviewListener",
      "LumenPreviewMedia",
      "LumenPreviewMediaEntry",
      "LumenPreviewMediaKind",
      "LumenPreviewPatch",
      "LumenPreviewRange",
      "LumenPreviewSession",
      "LumenPreviewSessionInputs",
      "LumenPreviewSessionOptions",
      "LumenPreviewState",
      "LumenPreviewStats",
      "LumenPreviewStatsCallback",
      "LumenPreviewTimeUpdate",
      "LumenPreviewTimeUpdateCallback",
      "LumenPreviewTransport",
      "MediaFailure",
      "MediaSourceInput",
      "UrlMediaSourceInput",
      "createLumenPreview",
      "mountPreview",
    ]);
    expect(Object.keys(root).sort()).toEqual([
      "LumenPreviewContext",
      "LumenPreviewSession",
      "createLumenPreview",
      "mountPreview",
    ]);
  });

  it("pins the names the advanced entry exports, all types", () => {
    expect(exportedNames(advancedSource)).toEqual([
      "LumenBindings",
      "LumenMediaTarget",
      "LumenPreviewBindings",
      "LumenPreviewControllerBinding",
      "MediaRegistration",
      "PreviewClock",
      "RegisteredImageSource",
      "RegisteredModelSource",
      "RegisteredVideoSource",
      "VideoSourceRegistration",
    ]);
    expect(Object.keys(advanced)).toEqual([]);
  });
});
