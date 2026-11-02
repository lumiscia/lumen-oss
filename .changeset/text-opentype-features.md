---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
---

The text node takes a `font_features` param, and spans take the same field: OpenType features by four-character tag, such as `{ "tnum": 1, "liga": 0, "ss01": 1 }`. A value of `0` turns a feature off, `1` turns it on and larger values select an alternate. A span's features merge over the node's: the span's keys win and the rest are inherited. Tags must be exactly 4 printable ASCII characters and values non-negative integers, and errors name the field. Each value takes a literal or an `=expression`, but the whole map cannot be an expression.

Features shape the text, so `text_width`, `text_height` and `text_bounds` follow them. A word takes the features of the first span that starts it, because cosmic-text shapes the spans of a word together. Text without features renders as before.

For Rust: `lumen_text::TextSpan` has a `font_features` field of `FontFeature` settings, and the new `FontFeatures` param kind is generated as `FontFeatures` in `@lumiscia/lumen-types`.
