import type { LumenPreviewController, VideoFrameMetadata } from "../index.js";
import type { LumenPreviewContext } from "../preview.js";

export function createWorkerControllerProxy(
  preview: LumenPreviewContext,
  timing: () => { fps: number; targetFrameDurationMs: number },
): LumenPreviewController {
  return {
    clear: () => {},
    clearMedia: () => {},
    clearVideos: () => {},
    clearVideoSource: () => {},
    currentFrame: () => preview.getSnapshot().frame,
    durationFrames: () => preview.getSnapshot().totalFrames,
    frameRequirements: () => "[]",
    frameRequirementsWindow: () => "[]",
    fps: () => timing().fps,
    targetFrameDurationMs: () => timing().targetFrameDurationMs,
    hasFont: () => false,
    hasImage: () => false,
    hasVideoFrame: () => false,
    height: () => preview.getSnapshot().height,
    isPlaying: () => preview.getSnapshot().isPlaying,
    loadComposition: () => {},
    loadFrameRequirements: async () => {},
    loadVideoFrame: async () => {},
    pause: () => {},
    play: () => {},
    registerFontFamily: () => {},
    registerFontFamilyBytes: () => {},
    registerImageSource: async () => {},
    registerVideoSource: async () => emptyVideoFrameMetadata(),
    registerVideoSourceBytes: async () => emptyVideoFrameMetadata(),
    removeFontFamily: () => {},
    removeImageSource: () => {},
    removeVideoSource: () => {},
    renderNow: async () => {},
    renderNowAsync: async () => {},
    setFont: () => {},
    setFrame: () => {},
    setImage: () => {},
    setLogLevel: () => {},
    setLookaheadCount: () => {},
    setVideoFrame: () => {},
    setVideoMetadata: () => {},
    syncMediaSources: async () => {},
    targetFrameForTimeMs: (timeMs) => Math.floor((timeMs / 1_000) * Math.max(timing().fps, 1)),
    tick: async () => false,
    tickAsync: async () => false,
    width: () => preview.getSnapshot().width,
  };
}

function emptyVideoFrameMetadata(): VideoFrameMetadata {
  return {
    width: 0,
    height: 0,
    duration: 0,
    fps: 0,
    frameCount: 0,
    firstTimestamp: 0,
    mimeType: null,
    trackId: 0,
    trackNumber: 0,
    codec: null,
    timelineMode: "fixed-fps",
  };
}
