---
"@lumiscia/lumen-bindings": patch
---

**A node that fails while rendering inside a template is located in that template.** Before, the diagnostic pointed at the root node that happened to share the failing node's id. For a `motion_blur` with `"samples": "=0 * frame"` as node 2 of the template `card`:

Before:

```json
{
  "code": "render.node_evaluation",
  "path": "/nodes/0",
  "message": "motion_blur node 2 evaluation failed: `samples` is 0; it must be from 1 to 64"
}
```

After:

```json
{
  "code": "render.node_evaluation",
  "path": "/compositions/card/nodes/1",
  "message": "composition `card`: motion_blur node 2 evaluation failed: `samples` is 0; it must be from 1 to 64"
}
```

A font a `text_3d` names that fails to load now points at that node's `font_family` (`/nodes/0/params/font_family`), instead of at the `environment_map` of the `render_3d` that draws it.
