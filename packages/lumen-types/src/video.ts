/** A video codec a render can ask for: `h264`, `hevc` or `av1` in an MP4, `prores` in a MOV, `vp9` in a WebM. */
export type VideoCodec = "h264" | "hevc" | "av1" | "prores" | "vp9";

/** Bits per sample of a rendered video. */
export type VideoBitDepth = 8 | 10;
