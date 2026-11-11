import type { LumenComposition } from "@lumiscia/lumen-types";
import { describe, expect, it } from "vitest";

import { resolvePreviewInputs } from "../../src/session/inputs.js";

describe("resolvePreviewInputs", () => {
  it("decodes the audio a template's clips name", () => {
    const document: LumenComposition = {
      compositions: {
        sting: {
          audio: {
            clips: [
              { duration_ms: 500, id: "a", source_id: "whoosh", start_ms: 0, track_id: "main" },
            ],
            tracks: [{ id: "main" }],
          },
          connections: [],
          nodes: [{ id: 1, type: "media_output" }],
        },
      },
      connections: [],
      nodes: [{ id: 1, type: "media_output" }],
      render_settings: { height: 4, width: 4 },
      timeline: { duration_frames: 30, fps: 30 },
    };

    // An untyped blob says nothing of its kind, so only the template names it as audio.
    const inputs = resolvePreviewInputs(document, { whoosh: new Blob([]) });

    expect(inputs.audio.map(({ id }) => id)).toEqual(["whoosh"]);
    expect(inputs.media.size).toBe(0);
  });
});
