---
"@lumiscia/lumen-bindings": minor
---

`lumen-engine` no longer has `json`, `msdf` or `webgl` Cargo features: JSON parsing and MSDF text are always compiled in, so `--features json` and `--features msdf` are no longer needed or accepted. The engine's `LogLevel`, `set_log_level` and `log_level_enabled` are gone; the browser build now sets its console level itself, and `setLogLevel` accepts only `off`, `error`, `warn`, `info`, `debug` and `trace`.
