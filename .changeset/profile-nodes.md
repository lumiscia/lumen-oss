---
"@lumiscia/lumen-bindings": patch
"@lumiscia/lumen-cli": patch
---

`lumen render --profile-nodes` prints which nodes take the bind (CPU) time. After the summary line it lists the ten slowest nodes by their own time per frame, averaged over the frames rendered, then what all nodes took against the frame's `bind_ms`, so the time outside nodes is visible. A node's own time leaves out the nodes it takes as input, and a node inside a template names the template. Nothing is timed without the flag. It works for videos, images and sequences. In the engine, `Mp4Render` and `ImageRender` take `set_profile_nodes(true)` and report `RenderReport::node_timing`.

```sh
lumen render film.json -o out.mp4 --profile-nodes
```

```text
bind by node (ms per frame, top 10)
  12  instance_3d  3.81
   7  text         1.20
   3  path_2d      0.44
  all nodes 5.45 of bind_ms 6.10
```
