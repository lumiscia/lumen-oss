---
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-bindings": minor
---

Add expression animation builtins: `progress`, `tween`, the `in`/`out`/`in_out` quad, cubic, expo and back easings, `cubic_bezier` and `spring`, plus `s` and `ms` suffixes on numbers. `vec2` values now support `+`, `-` and scaling, and `lerp`/`tween` work on numbers, `vec2` values and colours. Colours are expression values (`#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()`), so colour and paint params accept expressions. Compositions can declare reusable `functions`, such as `"rise(start)": "tween(vec2(0, 60), vec2(0, 0), start, 16, 'out_expo')"`, which expand where they're called. The builtin table, easing names and globals are published in the schema and as `expressionBuiltins`, `easingNames`/`EasingName` and `expressionGlobals` in the types.
