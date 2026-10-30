# @lumiscia/lumen-preview

## 0.3.1

### Patch Changes

- c44cca1: Preview audio now loops with the video. When playback passes the end of the timeline, the audio engine wraps back to 0 at `duration_frames / fps` instead of playing on past the end. Nothing on the audio timeline at or past the end is mixed, and `currentTimeMs()` returns the position within the loop instead of clamping to the duration. With the worker driver, audio and video now jump back to 0 together. With the main-thread driver, video no longer freezes on the last frame while audio carries on. Loop lengths that aren't a whole number of samples, such as at 29.97 fps, stay in phase over repeated loops. A timeline with a zero or invalid duration doesn't wrap.
- b506afc: Play audio clips whose source has no decodable audio, such as a video without an audio track, as silence instead of failing the preview.

## 0.3.0

### Patch Changes

- 482ec57: Update runtime dependencies to their latest releases.
- e030363: Keep preview commands applied while a render is in flight, reuse the GPU device across composition and media changes, and bound the wasm video frame cache by bytes through `frameCacheCapacity`.

## 0.2.2

## 0.2.0

### Minor Changes

- 072169a: Replace legacy node property metadata with typed node params.
- cb6f4dc: Add external GPU frame targets and GPU media import plumbing for lower-copy render and encode paths.

## 0.1.0

### Minor Changes

- 906481a: Prepare the Lumen TypeScript package family for coordinated public releases.
