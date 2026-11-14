---
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
"@lumiscia/lumen-bindings": minor
---

**An audio track's or clip's level is `gain` in the document, as it is in the builder.** The JSON key was `volume` while `addAudio({ gain })` wrote it, so a hand-written `"gain"` was ignored and the clip played at full level. Audio tracks, clips and the `audio` object now reject keys they do not list, like the rest of the document, so a leftover `volume` (or any other stray key) is an error naming its path, such as `audio.clips[0].volume: unknown field`. `name` stays accepted on tracks and clips. Error messages and JSON pointers say `gain` too: `audio.tracks[0].gain: must be at least 0`.

Before:

```json
{
  "audio": {
    "tracks": [{ "id": "music", "volume": 0.5 }],
    "clips": [
      {
        "id": "a",
        "source_id": "music.mp3",
        "track_id": "music",
        "start_ms": 0,
        "duration_ms": 4000,
        "volume": "=tween(0, 1, 60, 15)"
      }
    ]
  }
}
```

After:

```json
{
  "audio": {
    "tracks": [{ "id": "music", "gain": 0.5 }],
    "clips": [
      {
        "id": "a",
        "source_id": "music.mp3",
        "track_id": "music",
        "start_ms": 0,
        "duration_ms": 4000,
        "gain": "=tween(0, 1, 60, 15)"
      }
    ]
  }
}
```

`addAudio` and `addAudioTrack` keep taking `gain`; they now write `gain`. In a connection, `from_port` may be left out: it defaults to `output`, the only output any node has, and the schema no longer lists it as required.
