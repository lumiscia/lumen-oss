---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
"@lumiscia/lumen-preview": minor
---

Audio clips fade, volumes animate, and templates carry their own sound.

- A track's or clip's `volume` takes an `=expression` as well as a number, evaluated in the time of the composition the audio is in: `frame`, `time`, `fps`, `width`, `height` and, inside a template, props. Gains are evaluated every 512 samples and move linearly in between, so they never step. A volume expression that reads a node, a glyph or text, or a prop the template doesn't declare, fails the check with `audio.invalid`; a value below 0 plays as silence.
- Clips take `fade_in_ms` and `fade_out_ms`, which fade from and to silence at the clip's start and end on an equal-power curve. Fades longer than a short clip multiply where they overlap.
- A template in `compositions` can have its own `audio`. It plays wherever a `clip` that reaches the output plays the template, at the clip's start, duration, offset, speed and end, as its frames do, through nested clips. Speed resamples, so pitch follows it; past a `hold` window the audio is silent. A `switch` plays only its selected layer's audio, and a `time_remap` plays its source's audio at the remapped time, silent where that time holds or runs backwards.
- Audio planning is bounded. The root and each template may hold at most 512 audio tracks and 8192 audio clips, and clips that play templates with sound more than 4096 times in all, counting the clips inside the templates they play, fail the check with `audio.invalid`; planning an interval only looks at the clips that overlap it. While rendering or previewing, a 512-sample interval may be inside at most 1024 clips and time remaps (a loop may restart at most 32 times in it) and sound at most 1024 clips, one planning call (the server mixes 30 s at a time) returns at most 500,000 segments, and the preview's plan of the whole timeline covers at most about 3 hours and 200,000 segments; past a limit the render or the preview load fails with the new `audio.limit` code.
- A prop a clip sets by expression moves a gain smoothly, as the gain's own expression does. Nested audio keeps sounding after a parent loop restarts, including under a `switch` or another loop, and a looping `time_remap` no longer silences the 512 samples around its wrap, whatever its loop bounds.
- The builder's `addAudio` takes `fadeIn` and `fadeOut` in seconds and `gain` as a number or an `Expr<number>`, `addAudioTrack` takes an `Expr<number>` gain, and `defineComposition` keeps the audio its `build` adds.

```json
{
  "compositions": {
    "whoosh": {
      "timeline": { "fps": 30, "duration_frames": 30 },
      "props": { "level": { "type": "number", "default": 1 } },
      "audio": {
        "tracks": [{ "id": "main" }],
        "clips": [
          {
            "id": "a",
            "source_id": "whoosh.wav",
            "track_id": "main",
            "start_ms": 0,
            "duration_ms": 500,
            "fade_out_ms": 100,
            "volume": "=prop('level')"
          }
        ]
      },
      "nodes": [
        { "id": 1, "type": "background" },
        { "id": 2, "type": "media_output" }
      ],
      "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
    }
  },
  "audio": {
    "tracks": [{ "id": "music", "volume": "=if(time >= 2 && time < 6, 0.25, 1)" }],
    "clips": [
      {
        "id": "bed",
        "source_id": "music.mp3",
        "track_id": "music",
        "start_ms": 0,
        "duration_ms": 10000,
        "fade_in_ms": 1000,
        "fade_out_ms": 1000
      }
    ]
  }
}
```

The browser preview plays the same mix: the engine plans it and the preview's audio worker plays the plan.

Breaking changes:

- `LumenPreviewController.audioTimeline()` returns the engine's audio plan, `{ voices, segments, durationFrames, fps }`, instead of the parsed tracks and clips, and `LumenAudioEngine.loadAudioTimeline` takes that JSON. Code that only passes `audioTimeline()` through to `loadAudioTimeline` needs no change.
- The generated `AudioClip` and `AudioTrack` types have `volume: ExpressionValue<number>`, and the root `audio` is the new `AudioTimeline` type.
- In Rust, `AudioMixer::new` takes the `Composition` instead of its timeline, and `mix_range` takes `&mut self` and fails with a `LumenError`. `AudioTrack::volume` and `AudioClip::volume` are an `AudioGain`, and `CompositionTemplate` has an `audio` field. Use `Composition::has_audio` to ask whether a render has sound.
