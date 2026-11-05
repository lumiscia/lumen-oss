---
"@lumiscia/lumen-shared": minor
---

Audio has one API, `Composition.addAudio`, with every time in seconds: `composition.addAudio({ source: "music", start: 0.5, duration: 2.5, offset: 1.25, trackId: "music", gain: 0.8 })`. Seconds become the whole milliseconds the composition JSON uses, whatever the composition's fps. A clip's track is declared as a plain track if it isn't one yet, `trackId` defaults to `main`, and clips may overlap. `addAudioTrack({ id, name, muted, solo, gain })` sets a track's mute, solo and gain; call it before the track's first clip. `AudioTrack`, its `addClip`, and the `startMs`, `startSeconds`, `durationMs`, `durationSeconds`, `sourceId`, `sourceStartMs` and `sourceStartSeconds` clip options are gone, as are the builder's overlap check and its `AudioTimelineInput`, `AudioClipInput` and `AudioTrackInput` types. `@lumiscia/lumen-sdk` no longer re-exports `AudioTrack`.
