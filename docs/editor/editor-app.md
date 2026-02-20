# Editor app spec

## Summary
Create a local-only editor app that hosts the UI from `@lumiscia/editor`, renders previews using
`@lumiscia/canvas-renderer`, and submits final renders to the Rust renderer API (`lumen-server`).
This app is **not** deployed to Cloudflare and should run locally only.

**Proposed app:** `apps/editor` (`@lumiscia/editor-app`)

## Goals
- Run a local dev editor UI for all styles.
- Preview frames with lumen-wasm via `@lumiscia/canvas-renderer`.
- Submit render jobs to the Rust renderer API.
- Keep configuration explicit via `.env`.

## Non-goals
- No Cloudflare workers or wrangler config.
- No production deploy pipeline.
- No shared auth/session system.

## Environment variables
- `VITE_LUMEN_SERVER_URL` — base URL for the Rust renderer API.
  - Example: `http://localhost:8080`

- `VITE_LUMEN_WASM_MODULE_URL` — JS module URL for the Emscripten loader (defaults to `/lumen-wasm/lumen_wasm.js`).
- `VITE_LUMEN_WASM_URL` — `.wasm` binary URL (defaults to `/lumen-wasm/lumen_wasm.wasm`).

> Note: `lumen-server` expects `Authorization: Bearer <SECRET>` (see `crates/lumen-server`).
> For local-only use, add `VITE_LUMEN_SERVER_SECRET` and include it in requests if set.

Add an `apps/editor/.env.example` with these keys.

## Dependencies
- `@lumiscia/editor` for editor UI + preset conversion.
- `@lumiscia/templates` for template compilation (transitively used by the editor package).
- `@lumiscia/canvas-renderer` for preview rendering.
- `@lumiscia/shared` for schema types.

## App architecture

### Routes/pages
- `/` — editor landing with style picker + editor workspace.

### Preview flow
1. User edits a preset via `@lumiscia/editor` UI.
2. Convert preset → `Project` using the editor package.
3. Load the project into `LumenPreviewRenderer` and query frame requirements.
4. Decode required media with Mediabunny (WebCodecs) and render frames in WASM.
5. Surface approximation badges per `docs/preview-parity-matrix.md`.

### Render flow (video generation)
1. Convert preset → `Project`.
2. `POST {VITE_LUMEN_SERVER_URL}/renders` with the `Project` JSON.
3. Poll/stream:
   - `GET /renders/{job_id}` for status.
   - `GET /renders/{job_id}/events` for SSE progress.
4. On completion, fetch artifact:
   - `GET /renders/{job_id}/artifact`.

### Error handling
- Surface validation errors from Zod in the UI.
- Render API errors should be shown as user-friendly toast messages; keep internal details out of
  the UI.

## Local-only tooling
- Use Vite dev server (`pnpm --filter @lumiscia/editor-app dev`).
- Build + sync the wasm bundle before previewing: `pnpm wasm:build` (requires Emscripten).
  - Copies artifacts into `apps/editor/public/lumen-wasm/`.
- If you already built the wasm artifacts, run `pnpm wasm:sync`.

- Do **not** add `wrangler.toml` or Cloudflare bindings.

## Suggested file layout
```
apps/editor/
├── src/
│   ├── routes/
│   ├── components/
│   ├── lib/
│   └── styles/
├── .env.example
├── package.json
└── vite.config.ts
```

## Guardrails
- Keep API calls scoped to the Rust renderer only.
- Do not introduce new top-level dependencies without approval.
- Use `@/` alias for app-internal imports.
