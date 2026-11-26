---
"@lumiscia/lumen-react": minor
---

`useLumenPreview` takes an optional selector, so a component that reads one part of the preview state no longer re-renders on every played frame: `const isPlaying = useLumenPreview(preview, (state) => state.isPlaying)`. The selector should return a field or a primitive; the one-argument form is unchanged.
