---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-types": minor
"@lumiscia/lumen-shared": minor
---

`instance_3d` draws its input scene `count` times, from 0 to 100000, sharing the input's meshes, textures, materials and skinned models' joint matrices on the GPU instead of repeating a node for each copy. Each copy is placed by `position`, `rotation` and `scale`, which are evaluated once per copy, and two new expression globals say which copy that is: `instance` is its index from 0 and `instance_count` is `count`, so one node lays out a grid, a ring or a path (`path_point(d, instance / instance_count)`) without separate layout modes. They are only readable in this node's `position`, `rotation` and `scale`, and reading them elsewhere fails like `glyph` does outside a text node's `glyph_*` params. In the TypeScript SDK they are `instance` and `instanceCount`, exported with `frame` and `time`.

Only the input's objects are copied. Its lights and camera appear once, placed as without the node. Every copy of a skinned model plays the same clip at the same time, and shadows, fog and the environment treat copies like any other object. An `instance_3d` may hold another, up to 8 deep, with their counts multiplied. A scene draws at most 100000 objects in all, counting every copy's objects, and one that would draw more, or more vertices than fit one draw, fails the render naming the node before anything is copied. `count` 0 leaves the scene out entirely. A node whose params don't read `frame` or `time`, or another node, stays static when its input is, so its `render_3d` is kept across frames. The names `instance` and `instance_count` are now reserved, so a composition function can't use them as parameter names.

```json
{
  "nodes": [
    { "id": 1, "type": "shape_3d", "params": { "size": [0.6, 0.6, 0.6], "position": [0, 0.3, 0] } },
    {
      "id": 2,
      "type": "instance_3d",
      "params": {
        "count": 100,
        "position": "=vec3(instance % 10 * 1.5 - 6.75, 0, floor(instance / 10) * 1.5 - 6.75)"
      }
    },
    { "id": 3, "type": "render_3d" },
    { "id": 4, "type": "media_output" }
  ],
  "connections": [
    { "from_node": 1, "to_node": 2, "to_port": "scene" },
    { "from_node": 2, "to_node": 3, "to_port": "scene" },
    { "from_node": 3, "to_node": 4, "to_port": "source" }
  ]
}
```
