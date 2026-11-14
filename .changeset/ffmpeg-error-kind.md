---
"@lumiscia/lumen-bindings": minor
---

`lumen_ffmpeg::FfmpegError` now carries a `kind: FfmpegErrorKind` (`Output`, `NoStream`, `Unsupported` or `Other`), set where the error is raised, and `is_output_failure`, `lacks_stream` and `is_unsupported` read it instead of matching `operation` and message text. `operation` is a label for messages only. Code that builds `FfmpegError` with a struct literal adds `kind`, or uses `FfmpegError::new(..).with_kind(..)`.
