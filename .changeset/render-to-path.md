---
"@lumiscia/lumen-bindings": minor
---

The Rust render library (`lumen_engine::render`) has a new function, `render_project_to_path`, that renders a composition straight to a file and returns the output's `OutputFormat`, so a long video, a ProRes file or a large image sequence is never held in memory. The file is written beside the destination and renamed onto it once complete, so a failed or cancelled render leaves no file and keeps any earlier one. `render_project` and `RenderedOutput` are unchanged: `render_project` still returns the output's bytes.
