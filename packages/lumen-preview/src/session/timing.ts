const DEFAULT_PREVIEW_FPS = 30;
const MS_PER_SECOND = 1_000;

export interface LumenPreviewTiming {
  fps: number;
  frameDurationMs: number;
}

export function previewTimingFromCompositionJson(
  compositionJson: string | null | undefined,
): LumenPreviewTiming {
  const fps = readCompositionFps(compositionJson) ?? DEFAULT_PREVIEW_FPS;
  return {
    fps,
    frameDurationMs: MS_PER_SECOND / fps,
  };
}

function readCompositionFps(compositionJson: string | null | undefined): number | null {
  if (!compositionJson) {
    return null;
  }

  let composition: unknown;
  try {
    composition = JSON.parse(compositionJson);
  } catch {
    return null;
  }

  if (!isRecord(composition) || !isRecord(composition.timeline)) {
    return null;
  }

  const { fps } = composition.timeline;
  return typeof fps === "number" && Number.isFinite(fps) && fps > 0 ? fps : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}
