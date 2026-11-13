---
"@lumiscia/lumen-cli": minor
---

**Breaking: `lumen` now has two commands, `render` and `check`, and takes the composition as a positional argument.** The `--composition` and `--check` flags are gone, with no aliases, and `--output` has a short form, `-o`.

Before:

```bash
lumen --composition composition.json --output out.mp4 --codec hevc
lumen --composition composition.json --output frame.png --frame 30
lumen --composition composition.json --check --format json
```

After:

```bash
lumen render composition.json -o out.mp4 --codec hevc
lumen render composition.json -o frame.png --frame 30
lumen check composition.json --format json
```

Each command takes only its own flags: `check` has `--format`, `--media-root` and `--font-dir`, and rejects the video, image and `--max-text-glyphs` flags it used to ignore, while `render` has no `--format`. The other flags and the environment variables keep their names and meanings. Bare `lumen` prints the two commands.

`lumen render -h` and `lumen check -h` now print a line per flag, grouped into general, video and image options, and `--help` a little more. The reference that the help used to print in full (encoder order, which encoders take `--crf`, `--qp` and `--preset`, sequence numbering, environment variables) is in the [CLI README](https://github.com/lumiscia/lumen-oss/tree/main/crates/cli#readme), which the help points to.
