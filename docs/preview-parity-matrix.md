# Preview Parity Matrix

- Version: 0.1
- Date: 2026-02-13
- Canonical reference: `docs/render-spec.md`

## Policy

1. Server render is authoritative for export.
2. Preview is best-effort and must never silently drift.
3. Unsupported or approximate preview features are first-class behavior and must be labeled in UI.
4. When approximation is active, preview must show an "approximate" badge and expose fallback
   reason.

## Feature Table

| Feature | Server Behavior | Preview Behavior | Preview Status | UI Signal |
|---|---|---|---|---|
| Clip ordering (`z_index`, stable tie-break) | Deterministic source-over draw order | Must match server ordering exactly | Required parity | none |
| Solid color clip | Exact RGBA fill | Exact | Required parity | none |
| Shapes (rect/ellipse) | Exact geometric fill | Exact | Required parity | none |
| Image/video fit (`fill/contain/cover`) | Canonical layout rules | Must match canonical layout rules | Required parity | none |
| Opacity composition | Source-over with effective alpha scaling | Must match opacity and stacking order | Required parity | none |
| Rotation transform | Center rotation on resolved draw rect | Match if possible; use nearest available transform stack | Approximation allowed if backend limits | approximation badge if degraded |
| Text layout and align | Canonical font metrics and alignment | Best-effort by browser text stack | Approximation expected | approximation badge + "font/layout" reason |
| Backdrop blur/effects | Canonical server effect pipeline | If unsupported, disable effect and render unblurred content | Explicit fallback | approximation badge + "effect unsupported" reason |
| Video frame decode timing | Canonical source pipeline mapping | Match frame index mapping; if decode drops, keep last-good frame | Degradation allowed | approximation badge + "dropped frame" reason |
| Color management edge cases | Server-defined assumptions | Best-effort sRGB path | Approximation allowed | approximation badge + "color path" reason |

## Fallback Modes

1. `Exact`: no known semantic differences.
2. `Approximate`: same intent with measurable visual drift.
3. `Unsupported`: effect disabled or substituted.

Preview runtime must return fallback metadata per frame so UI can surface reasoned diagnostics.

## Drift Handling

1. Any parity test failure must add/update a row in this matrix.
2. New renderer features must define preview behavior in this file before implementation.
3. Silent fallback is prohibited.
