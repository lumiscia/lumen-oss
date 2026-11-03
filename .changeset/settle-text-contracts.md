---
"@lumiscia/lumen-bindings": minor
---

Text enum params now take their names from expressions, so `font_style: "=if(frame > 10, 'italic', 'normal')"` works as the TypeScript types say instead of failing mid-render; numbers keep working. `background` `width` and `height` reject expressions at load, since they were only read once at compile and animating them did nothing; the TypeScript types now say `number`. A span with no text no longer changes where other spans wrap, and `font_size` documents how a ligature across two spans takes its size.
