---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

Add the `text_content`, `text_line` and `text_highlight` param kinds for rich text. No node uses them yet. Text content is a string or an array of spans. Each span needs `text` and can override the node's font family, size, weight, style, letter spacing and color, and can set an underline, strikethrough or highlight; `null` turns off a decoration the span would otherwise inherit. Span fields, including enums, accept `=expressions` like node params, and evaluation errors name their path, such as `content[1].font_weight`. The composition schema describes these kinds in `$defs` (`textContent`, `textSpan`, `textLine`, `textHighlight`, `textLineStyle`), which are generated from the Rust param definitions. `@lumiscia/lumen-types` exports matching `TextContent`, `TextSpan`, `TextLine`, `TextHighlight` and `TextLineStyle` types.

For Rust: nested `Delegate` structs can now have `Option<T>` fields, which are left out when absent, and `Override<T>` fields, which inherit, clear or set a value. A struct delegate marked `#[delegate(properties)]` parses its nested JSON field by field the way node params do, rejects unknown fields, and has a `to_json` method; other struct delegates still deserialize through serde. `PropertyDef` has a new `nullable` field. `TextFontStyle` accepts expressions, which also lets the text node's `font_style` take one.
