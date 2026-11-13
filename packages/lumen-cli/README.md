# @lumiscia/lumen-cli

The `lumen` command checks and renders [Lumen](https://github.com/lumiscia/lumen-oss) compositions on a local GPU.

```bash
npm install -g @lumiscia/lumen-cli

lumen check composition.json
lumen render composition.json -o out.mp4
lumen render composition.json -o frame.png --frame 30
lumen preview composition.json -o sheet.png
```

It installs a prebuilt binary for Linux on x64 and arm64 (glibc 2.28 or later) and for macOS on Apple silicon, with FFmpeg linked in. Rendering needs a GPU, with Vulkan on Linux and Metal on macOS.

`lumen render --help`, `lumen check --help` and `lumen preview --help` list every flag, and the [CLI README](https://github.com/lumiscia/lumen-oss/tree/main/crates/cli#readme) explains them.

## From a script

The same package renders from a script, so running the script that builds a composition is enough. It works under Node 18 or later, Bun and Deno, and pipes the composition to the installed `lumen` binary.

```ts
import { Composition } from "@lumiscia/lumen-sdk";
import { check, LumenCliError, preview, render } from "@lumiscia/lumen-cli";

const composition = new Composition({/* ... */});

const report = await check(composition); // { ok, diagnostics }
await preview(composition, { output: "sheet.png", count: 20, columns: 5 });
await render(composition, { output: "out.mp4", quality: "high", frames: { start: 30, end: 90 } });
```

```bash
node composition.js
bun composition.ts
deno run -A composition.ts
```

A composition is JSON, or anything with `toJSON()`. `render` and `preview` need `output`, and the other options are the flags of the same name in camelCase: `frames`, `frame`, `mediaRoot`, `fontDir`, `maxTextGlyphs`, `codec`, `quality`, `qualityConfig`, `bitDepth`, `alpha`, `encoder`, `bitrate`, `crf`, `qp`, `preset` and `jpegQuality` for `render`, and `count`, `columns`, `width`, `frames`, `mediaRoot`, `fontDir` and `maxTextGlyphs` for `preview`. `check` takes `mediaRoot` and `fontDir`. They resolve with `{ output }`, and `check` resolves with the report whether or not the composition has errors.

`lumen`'s progress and warnings go to this process's stdout and stderr, as they do on the command line, and `quiet: true` hides them. `signal` stops `lumen` when it aborts. A run that fails rejects with a `LumenCliError`, which holds `exitCode`, `signal` and the end of `stderr`:

```ts
try {
  await render(composition, { output: "out.mp4", quiet: true });
} catch (error) {
  if (error instanceof LumenCliError) {
    console.error(error.exitCode, error.stderr);
  }
}
```

`preview` needs a `lumen` that has the `lumen preview` command.

## License

This package is Apache-2.0. The binary it installs comes from a platform package such as `@lumiscia/lumen-cli-linux-x64` and contains FFmpeg with x264 and x265, so it is distributed under GPL-3.0-or-later.
