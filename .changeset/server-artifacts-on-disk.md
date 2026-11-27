---
"@lumiscia/lumen-bindings": patch
---

`lumen-server` keeps each finished render as a file on disk instead of in memory, and streams it from `GET /renders/{id}/artifact` with the same status codes and headers as before (`content-type`, `content-length`, `content-disposition`). The files live in a `lumen-server-renders-<random>` directory the server creates in the system temporary directory at startup. A render's file is deleted when the render is evicted past `--retained-renders`, and on `SIGTERM` or Ctrl-C the server cancels its running renders and deletes the directory. A 622 MB render took the server's peak memory to about 676 MiB before and about 170 MiB now. In the server crate, `Artifact` holds `file: Arc<ArtifactFile>` in place of `bytes`, `ArtifactFile` is new, `CompletedRender` holds `file: ArtifactFile` in place of `bytes`, and `RenderJob` has a new `destination` field, the path `execute_render_job` writes the render to with `render_project_to_path`.
