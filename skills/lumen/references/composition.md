# Composition structure

The canonical definition is the JSON Schema at https://lumiscia.com/schema/composition.json (also `LumenComposition` in `@lumiscia/lumen-types`, re-exported by the SDK). Docs: https://docs.lumiscia.com/composition-format.md. Check exact params and defaults per node in the schema or the node pages linked below rather than guessing.

## Document

```json
{
  "$schema": "https://lumiscia.com/schema/composition.json",
  "metadata": { "name": "Launch card" },
  "timeline": { "fps": 30, "duration_frames": 150 },
  "render_settings": { "width": 1280, "height": 720, "background_color": [0, 0, 0, 255] },
  "nodes": [
    { "id": 1, "type": "background", "params": { "paint": [18, 24, 32, 255] } },
    {
      "id": 2,
      "type": "text",
      "params": { "content": "Launch", "font_size": 96, "position": [120, 300] }
    },
    { "id": 3, "type": "raster_multimerge" },
    { "id": 4, "type": "media_output" }
  ],
  "connections": [
    { "from_node": 1, "to_node": 3, "to_port": "layers" },
    { "from_node": 2, "to_node": 3, "to_port": "layers" },
    { "from_node": 3, "to_node": 4, "to_port": "source" }
  ]
}
```

| Field             | Required | Notes                                                                                |
| ----------------- | -------- | ------------------------------------------------------------------------------------ |
| `timeline`        | Yes      | `duration_frames` is the exact frame count. `fps` defaults to 30.                    |
| `render_settings` | Yes      | Output `width`/`height` in px. Optional `background_color`.                          |
| `nodes`           | Yes      | `{ id, type, params? }`. Integer ids, unique per graph. Omitted params use defaults. |
| `connections`     | Yes      | `{ from_node, from_port?, to_node, to_port }`. `from_port` defaults to `output`.     |
| `compositions`    | No       | Named templates that `clip` nodes play.                                              |
| `functions`       | No       | Reusable expression functions (see expressions.md).                                  |
| `audio`           | No       | `{ tracks, clips }`.                                                                 |
| `metadata`        | No       | Your data. Lumen reads `metadata.name`.                                              |

Unknown top-level fields (including `$schema`) are allowed, so editors can store state. Nodes are strict: unknown node types or params fail validation. Param names are snake_case; enums are string names (`"blend_mode": "screen"`).

Colors are `[r, g, b]` / `[r, g, b, a]` integers 0–255 or `"#rrggbb"` / `"#rrggbbaa"`. Paint params (`paint`, `fill_paint`, `stroke_paint`) also take linear/radial/conic gradient objects; see the schema's `paint` definition.

## Graph rules

- Exactly one `media_output`, connected to the stream you want rendered.
- Every required input connected, every connection pointing at a real node and port, no cycles.
- Every port carries `raster_frame`. Shape, path, and text nodes output rasters, so they connect anywhere.
- Nodes not upstream of `media_output` never render. Static branches (all-literal params) are cached.

## Node catalog and input ports

Docs page per node: `https://docs.lumiscia.com/nodes/<category>/<type with - for _>.md`.

| Category    | Node types                                                                                                                                                                                               | Inputs                                                                           |
| ----------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| source      | `background`, `media_in`, `text`                                                                                                                                                                         | none                                                                             |
| vector      | `shape`, `path`                                                                                                                                                                                          | none                                                                             |
| processing  | `transform`, `opacity`, `blur`, `shadow`, `crop`, `resize`, `exposure`, `levels`, `curves`, `color_grade`, `hue_saturation`, `channel_shuffle`, `alpha_premultiply`, `time_remap`, `memo`, `wgsl_shader` | `source` (`wgsl_shader` accepts several)                                         |
| compositing | `merge`                                                                                                                                                                                                  | `base`, `overlay`, optional `mask`                                               |
| compositing | `raster_multimerge`, `switch`                                                                                                                                                                            | `layers` (many; first connection at the bottom; `switch` shows `selected_layer`) |
| compositing | `boolean`                                                                                                                                                                                                | `a`, `b`                                                                         |
| compositing | `clip`                                                                                                                                                                                                   | optional `source` (used when `composition` is empty)                             |
| output      | `media_output`                                                                                                                                                                                           | `source`                                                                         |

Frequently used params:

- `background`: `paint`; `width`/`height` default to 0 = composition size.
- `text`: `content`, `font_family`, `font_size`, `font_weight`, `color`, `position`, `anchor`, `max_width`, `alignment_horizontal`, `line_height`, `letter_spacing`, `opacity`, per-glyph `glyph_offset`/`glyph_scale`/`glyph_rotation`/`glyph_opacity`.
- `shape`: `geometry_kind`, `width`, `height`, `border_radius`, `position`, `anchor`, `fill_paint`, `stroke_paint`, `stroke_width`, `opacity`.
- `media_in`: `kind` (`image` or `video`), `source` (manifest alias), `range_start`/`range_end` (source frames, end exclusive), `speed`, `loop_mode` (`clamp`, `repeat`, `ping_pong`).
- `transform`: `translate_x`, `translate_y`, `scale_x`, `scale_y`, `rotate`, `pivot_x`, `pivot_y`.
- `merge`: `blend_mode` (literal only), `opacity`, `mask_mode`, `invert`, `feather`.

## Media

`media_in.source` and audio clip `source_id` hold a manifest alias (up to 128 characters: letters, digits, `.`, `_`, `-`). The render request maps aliases to media:

```json
{ "composition": { "...": "..." }, "media": { "hero": "lumen:media_3f9a1c2b7d4e5f60" } }
```

The browser preview ignores the manifest and takes files registered under the same aliases (`mediaSources` / `audioSources`).

## Templates and clips

`compositions` holds named templates, each with its own `nodes`, `connections`, `media_output`, and optional `timeline`, `render_settings`, and `props` (`string`, `number`, `color`, `vec2`). Without `render_settings` a template takes the parent's size; without `timeline` it takes the parent's timeline. A `clip` node plays one: `composition`, `start`, `duration` (`-1` = to the end), `offset`, `speed`, `end` (`hide`, `hold`, `loop`), `props`. A clip draws from the top-left of the canvas at the template's size, so place it with a `transform`.

In TypeScript, `defineComposition` builds the template and `clip()` adds it:

```ts
import { Composition, defineComposition, prop } from "@lumiscia/lumen-shared";

const lowerThird = defineComposition({
  name: "lower_third",
  size: { width: 800, height: 160 },
  duration: 90,
  props: { name: prop.string("Untitled"), accent: prop.color([255, 196, 0, 255]) },
  build: (c, { name, accent }) =>
    c.addNode({ type: "text", params: { content: name, color: accent, font_size: 56 } }),
});

const composition = new Composition({
  renderSettings: { width: 1280, height: 720 },
  timeline: { fps: 30, durationSeconds: 5 },
});
const card = composition.addNode(lowerThird.clip({ start: 15, props: { name: "Ada Lovelace" } }));
const placed = composition.addNode({
  type: "transform",
  params: { translate_x: 80, translate_y: 520 },
});
const output = composition.addNode({ type: "media_output" });
composition
  .connect(card, placed, { toPort: "source" })
  .connect(placed, output, { toPort: "source" });
```

Inside a template, read props with `prop('name')` in expressions. Templates can nest but not cycle.

## Audio

```json
{
  "audio": {
    "tracks": [{ "id": "music", "volume": 0.8 }],
    "clips": [
      {
        "id": "intro",
        "track_id": "music",
        "source_id": "song",
        "start_ms": 0,
        "duration_ms": 3000
      }
    ]
  }
}
```

Clips time in ms (`start_ms`, `duration_ms`, `source_start_ms`) or frames (`start_frame`, `duration_frames`). Tracks take `volume`, `muted`, `solo`. In TypeScript:

```ts
import { AudioTrack } from "@lumiscia/lumen-shared";

const music = new AudioTrack({ id: "music", volume: 0.8 });
music.addClip({ sourceId: "song", startSeconds: 0, durationSeconds: 4 });
composition.addAudioTrack(music);
```

Clips on one track can't overlap.
