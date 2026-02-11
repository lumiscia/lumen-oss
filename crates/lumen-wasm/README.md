# lumen-wasm

`lumen-wasm` exposes a browser-facing runtime around the existing Rust `lumen` core.

## Goals
- Keep sequence parsing and compile validation in Rust.
- Keep editing state (`selectClip`, `updateTransform`) in Rust.
- Delegate browser video decode to a pluggable JS backend.

## JS backend contract
Register a backend object with:

```ts
type VideoDecodeRequest = {
  asset_id: string;
  source: string;
  source_frame: number;
  timeline_frame: number;
  fps_num: number;
  fps_den: number;
};

type VideoBackend = {
  decodeFrame(request: VideoDecodeRequest): Promise<unknown>;
};
```

`decodeFrame` can return any JS value your app understands (for example `VideoFrame`, `ImageBitmap`, RGBA bytes, etc).

## Runtime methods
- `loadSequenceJson(sequenceJson)`
- `loadSequence(sequenceObject)`
- `getState()`
- `selectClip(clipId)`
- `updateTransform(clipId, transform)`
- `frameSummary(frameIndex)`
- `videoDecodeRequests(frameIndex)`
- `setVideoBackend(backend)`
- `clearVideoBackend()`
- `decodeVideoFrame(request)`
