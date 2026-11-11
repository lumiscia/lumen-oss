---
"@lumiscia/lumen-preview": minor
"@lumiscia/lumen-react": minor
"@lumiscia/lumen-svelte": minor
---

The preview player can loop, change speed, change volume, play a section and tell you when it ended. `mountPreview`, `session.update()` and the React and Svelte `Player` take these, and apply them live without a remount or a restart:

- `loop`: plays on from the start of the range at its end, with no gap and no repeated or dropped frame, audio included.
- `rate`: 0.25 to 4. Audio follows it, with its pitch (it is resampled, not time-stretched). Changing it while playing keeps the current time.
- `volume` (0 to 1) and `muted`: applied in a gain node over about 50 ms, so they don't click. Un-muting restores the volume.
- `range: { in, out }`: plays frames `in` to `out - 1` (`out` is exclusive). Seeks land inside it, lookahead decoding stays inside it, and `null` restores the whole timeline. An `out` past the end is cut to it with a console warning.
- `onEnded`: called once when playback reaches the end of the range without `loop`.
- `onTimeUpdate({ frame, timeMs })`: called at most once for each frame shown. It costs nothing while unset and doesn't re-render a React or Svelte component.

A value that can't be used (a `rate` outside 0.25 to 4, a `volume` outside 0 to 1, a malformed `range`, or a non-finite number) is reported through `onError` and the state's `error` with the new code `"invalid_option"`, and the previous options stay in effect. `session.loop`, `rate`, `volume`, `muted` and `range` read the options in effect.

```tsx
<Player
  preview={preview}
  composition={composition}
  range={{ in: 30, out: 90 }}
  loop
  rate={1.5}
  volume={0.8}
  onEnded={() => setPlaying(false)}
/>
```

**Breaking:** playback no longer loops by default. Before, the preview wrapped round the whole composition forever; now it stops on the last frame and calls `onEnded`, and playing again from there starts over. Pass `loop` to keep the old behaviour. `LumenPreviewErrorCode` gains `"invalid_option"`, so an exhaustive `switch` over it needs a new case.
