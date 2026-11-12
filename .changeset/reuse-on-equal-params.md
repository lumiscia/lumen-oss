---
"@lumiscia/lumen-bindings": patch
---

A node whose params read the frame is kept across frames while their values hold. Each frame its expressions are evaluated, along with those of the nodes it reads, and its output from an earlier frame is reused when they give the values it was drawn with. A key track that holds, a `switch` whose `selected_layer` changes once, or an `opacity` that steps from 0 to 1 now redraws only on the frames where a value changes, and so does everything downstream of it. A camera that sits still for 40 frames, moves for 9 and sits still again pays for the 9.

A shape, path, text or `transform` that blurs its own motion compares its values at either end of its shutter too, and a `motion_blur` over such a node is drawn on every frame, while the samples it takes of its source are reused when their values match. A text node whose `glyph_*` params read the frame, an `instance_3d` whose placement does, video, a `wgsl_shader` and a model playing a clip are drawn on every frame as before.
