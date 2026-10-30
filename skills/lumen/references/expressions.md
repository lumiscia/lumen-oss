# Expressions and animation

Full reference: https://docs.lumiscia.com/expressions.md. Patterns: https://docs.lumiscia.com/guides/animation.md.

## Syntax

- A param string starting with `=` is evaluated every frame: `"opacity": "=progress(0, 30)"`.
- `==` escapes a literal: `"==total"` is the text `=total`.
- Most number, boolean, text, vec2, and paint params accept expressions. Enum params such as `merge.blend_mode` and `transform.sampling`, plus `clip.composition` and `clip.end`, are literal only. The node pages mark which.
- The result must fit the param: number params take numbers or booleans, vec2 params take `vec2(...)`, paint params take a color (solid, not a gradient).

Literals: `42`, `true`, `'text'`, `#ff8000`, `1.5s` / `300ms` (converted to frames at the composition fps), `vec2(10, 20)`, `rgb(...)`/`rgba(...)` (0–255).

Globals: `frame` (from 0), `time` (seconds), `fps`, `width`, `height`. Inside a template, `frame` and `time` follow the clip's clock. There is no global for the timeline length, so write the frame count as a number.

Operators: `+ - * / %`, `> < >= <=`, `== !=`, `&& || !`, and `if(cond, a, b)`. Vec2 values combine component-wise. Colors have no arithmetic (use `lerp`).

## Functions

| Function                                                                   | Use                                                                  |
| -------------------------------------------------------------------------- | -------------------------------------------------------------------- |
| `progress(start, end)`                                                     | 0 before `start`, 1 after `end`, linear between. Fades.              |
| `tween(from, to, start, duration, ease)`                                   | Eased move of numbers, vec2s, or colors. `ease` defaults `'linear'`. |
| `lerp(from, to, t)`                                                        | Interpolate numbers, vec2s, colors.                                  |
| `spring(t, stiffness?, damping?)`                                          | 0 → 1 spring, `t` seconds after release (170 / 26 defaults).         |
| `cubic_bezier(x1, y1, x2, y2, t)`                                          | CSS-style curve.                                                     |
| `min max abs floor ceil round fract mod pow sin cos clamp smoothstep step` | Math.                                                                |
| `uppercase lowercase text_width text_height`                               | Text. `text_width(node(8))` measures a text node.                    |
| `node(id, 'param')`                                                        | Another node's current param value (number, bool, text, vec2).       |
| `prop('name')`                                                             | Template prop, only inside a template.                               |

Easing names: `'linear'`, `'in_quad'`, `'out_quad'`, `'in_out_quad'`, `'in_cubic'`, `'out_cubic'`, `'in_out_cubic'`, `'in_expo'`, `'out_expo'`, `'in_out_expo'`, `'in_back'`, `'out_back'`, `'in_out_back'`.

`text` evaluates `glyph_offset`, `glyph_scale`, `glyph_rotation`, and `glyph_opacity` once per glyph, where `glyph`, `word`, `line` (and `glyph_count`, `word_count`, `line_count`) are available. Those globals are errors anywhere else.

## Examples

```json
{
  "functions": { "rise(start)": "tween(0, 1, start, 12, 'out_cubic')" },
  "nodes": [
    {
      "id": 1,
      "type": "background",
      "params": { "paint": "=lerp(#1e293b, #0f766e, progress(0, 90))" }
    },
    {
      "id": 2,
      "type": "text",
      "params": {
        "content": "One glyph at a time",
        "font_size": 72,
        "position": "=tween(vec2(120, 340), vec2(120, 300), 10, 0.5s, 'out_expo')",
        "glyph_opacity": "=tween(0, 1, 10 + glyph * 2, 12)"
      }
    },
    {
      "id": 3,
      "type": "transform",
      "params": { "scale_x": "=spring((frame - 12) / fps)", "rotate": "=time * 12" }
    },
    { "id": 4, "type": "opacity", "params": { "opacity": "=rise(20)" } }
  ]
}
```

## TypeScript helpers

`@lumiscia/lumen-shared` exports a typed helper per built-in in camelCase (`tween`, `progress`, `spring`, `outExpo`, `textWidth`, ...), the globals (`frame`, `time`, `fps`, `width`, `height`), `ease.*` names, `sec()`/`ms()` durations, and `hex()`/`rgb()`/`rgba()`/`vec2()`. They return `Expr` values that typed params accept and that serialize to `=` strings. Expression values chain with methods such as `.mul()` and `.div()`.

```ts
import {
  Composition,
  ease,
  fn,
  frame,
  progress,
  sec,
  sin,
  tween,
  type Num,
} from "@lumiscia/lumen-shared";

const composition = new Composition({
  renderSettings: { width: 1280, height: 720 },
  timeline: { fps: 30, durationSeconds: 3 },
});

const rise = fn("rise(start)", (start: Num) => tween(0, 1, start, 12, "out_cubic"));
composition.addFunction(rise);

composition.addNode({
  type: "transform",
  params: {
    translate_x: tween(-400, 0, 0, sec(0.6), ease.outCubic), // "=tween(-400, 0, 0, 0.6s, 'out_cubic')"
    rotate: sin(frame.div(10)).mul(5), // "=(sin((frame / 10)) * 5)"
  },
});
composition.addNode({ type: "opacity", params: { opacity: rise(10) } }); // "=rise(10)"
composition.addNode({ type: "opacity", params: { opacity: progress(0, sec(1)) } });
```

## Common expression errors

Missing leading `=` (read as a literal), `=` where `==` was meant, unknown function/global/easing names, `glyph` outside `glyph_*` params, `prop()` outside a template, division by zero, and a result type the param can't take (e.g. vec2 for a number).
