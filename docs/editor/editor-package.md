# Editor package spec

## Summary
Create a new workspace package that owns all editor UIs for each template/style. The package is UI-
focused and outputs validated preset data + compiled render projects; it does **not** render videos
or call the renderer API directly.

**Proposed package:** `packages/editor` (`@lumiscia/editor`)

## Goals
- Provide a single home for editor UIs per style (ex: chat story v1).
- Enforce preset validation using existing Zod schemas from `@lumiscia/shared`.
- Expose a clean API for the editor app to render previews and request final renders.
- Keep the editor package UI-only (no Cloudflare, no HTTP clients).

## Non-goals
- No Cloudflare bindings, workers, or wrangler config.
- No direct video rendering or file upload logic.
- No server-side rendering or production hosting concerns.

## Dependencies
- `@lumiscia/templates` for template-to-Project compilation.
- `@lumiscia/shared` for Zod schemas and shared types.
- `@lumiscia/canvas-renderer` types only (preview wiring lives in the app).

## Public API (proposed)

### `EditorDefinition`
Represents a single editor style.

```
export interface EditorDefinition<Preset> {
	kind: string
	version: number
	label: string
	schema: ZodSchema<Preset>
	defaults: Preset
	Editor: React.ComponentType<EditorProps<Preset>>
	toProject: (preset: Preset) => Project
}
```

### `EditorRegistry`
Central registry of available editors.

```
export const editorRegistry: EditorDefinition<unknown>[]
export function getEditor(kind: string, version?: number): EditorDefinition<unknown> | undefined
```

## Editor module shape (per style)
Each style editor lives in its own module and exports a concrete `EditorDefinition`.

Example layout:

```
packages/editor/src/editors/
├── chat-story-v1/
│   ├── editor.tsx
│   ├── schema.ts
│   └── index.ts
└── index.ts
```

## Data flow
1. UI edits produce a `Preset` model for the style.
2. Validate with the Zod schema from `@lumiscia/shared` (fail fast in the UI).
3. Convert to a `Project` using `@lumiscia/templates`:
   - `compilePresetToProject(preset)`
4. Hand the `Project` to the app for preview or render API submission.

## Styling + UI conventions
- React + Tailwind v4 (same as the main app).
- Components should be editor-specific (no new shadcn primitives here).
- No inline styles; use Tailwind classes.

## Testing
- Add `typecheck` and `lint` scripts.
- Unit tests (if any) should validate schema + conversion output only.

## Open items
- If additional styles are added, each should be a self-contained editor module with its own schema
  and defaults.
- If preview needs a Project → RenderLayer adapter, add it **inside the editor package** so the app
  stays thin and reuseable.
