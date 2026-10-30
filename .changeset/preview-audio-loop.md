---
"@lumiscia/lumen-preview": patch
---

Preview audio now loops with the video. When playback passes the end of the timeline, the audio engine wraps back to 0 at `duration_frames / fps` instead of playing on past the end. Nothing on the audio timeline at or past the end is mixed, and `currentTimeMs()` returns the position within the loop instead of clamping to the duration. With the worker driver, audio and video now jump back to 0 together. With the main-thread driver, video no longer freezes on the last frame while audio carries on. Loop lengths that aren't a whole number of samples, such as at 29.97 fps, stay in phase over repeated loops. A timeline with a zero or invalid duration doesn't wrap.
