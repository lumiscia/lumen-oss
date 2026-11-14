---
"@lumiscia/lumen-shared": minor
"@lumiscia/lumen-sdk": minor
---

**`addAudio` names the unit of its times, and a timeline takes one length.** Audio clip options were `start`, `duration`, `offset`, `fadeIn` and `fadeOut`, in seconds, next to visual clip nodes whose `start` and `duration` are frames. They are now `startSeconds`, `durationSeconds`, `offsetSeconds`, `fadeInSeconds` and `fadeOutSeconds`. The JSON they write (`start_ms` and the rest) is unchanged.

Before:

```ts
composition.addAudio({
  source: "music.mp3",
  start: 2,
  duration: 10,
  offset: 1.5,
  fadeIn: 1,
  fadeOut: 0.5,
});
```

After:

```ts
composition.addAudio({
  source: "music.mp3",
  startSeconds: 2,
  durationSeconds: 10,
  offsetSeconds: 1.5,
  fadeInSeconds: 1,
  fadeOutSeconds: 0.5,
});
```

`timeline` takes `duration_frames` or `durationSeconds`, not both. TypeScript rejects passing both, and a JavaScript caller that does gets a `TypeError` instead of the seconds silently winning.

```ts
new Composition({ timeline: { durationSeconds: 3 } });
new Composition({ timeline: { duration_frames: 90 } });
new Composition({ timeline: { duration_frames: 90, durationSeconds: 3 } }); // type error, throws
```
