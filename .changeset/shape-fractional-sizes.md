---
"@lumiscia/lumen-bindings": minor
---

Shape `width` and `height` are now floats, so an animated or measured size no longer snaps to whole pixels. Whole-number sizes render exactly as before; a composition whose expression gives a fractional size, such as `=text_width("Open editor", 20) + 48`, now draws that size instead of truncating it. Sizes between 0 and 1 draw one pixel across.

Int, uint and u8 params still truncate fractional expression results toward zero, but NaN and infinity are now a property error instead of silently becoming 0 or a saturated value.
