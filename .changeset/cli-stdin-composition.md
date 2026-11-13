---
"@lumiscia/lumen-cli": patch
---

`lumen render` and `lumen check` read the composition from stdin when it is `-`, so a script can pipe one in without writing a file:

```bash
node composition.js | lumen render - -o out.mp4
cat composition.json | lumen check - --format json
```

Messages and the `lumen check` summary call the source `<stdin>`. Media is found under `--media-root`, which defaults to the current directory. Empty input, or stdin that is a terminal, fails with an error instead of waiting. A file named `-` is reached as `./-`.
