# @lumiscia/lumen-shared

The `Composition` builder for Lumen. You add nodes to an explicit node graph, wire them with typed `inputs`, and get composition JSON. It depends only on `@lumiscia/lumen-types`.

```bash
pnpm add @lumiscia/lumen-shared
```

```ts
import { Composition } from "@lumiscia/lumen-shared";

const composition = new Composition({ timeline: { durationSeconds: 3 } });
const bg = composition.addNode({ type: "background", params: { paint: [24, 28, 36, 255] } });
const title = composition.addNode({
  type: "text",
  params: { content: "Hello, Lumen", font_size: 120, position: [96, 480] },
});
const merged = composition.addNode({ type: "merge", inputs: { base: bg, overlay: title } });
composition.addNode({ type: "media_output", inputs: { source: merged } });

console.log(JSON.stringify(composition));
```

The same graph can be written as node values. `nodes` has one function per node kind, named after its `type`, taking its params and input ports in one object. A value adds nothing until a node that reads it is added, here by `addOutput`, which adds a `media_output`. A value read by two nodes is one node with two readers.

```ts
import { Composition, nodes as n } from "@lumiscia/lumen-shared";

const composition = new Composition({ timeline: { durationSeconds: 3 } }).addOutput(
  n.merge({
    base: n.background({ paint: [24, 28, 36, 255] }),
    overlay: n.text({ content: "Hello, Lumen", font_size: 120, position: [96, 480] }),
  }),
);
```

Both produce the same JSON, and they mix: a node `addNode` returned can be a value's input, and a value can be one of `addNode`'s `inputs`. A `defineComposition` `build` can return a value too, and its `clip()` nests like any other node.

`@lumiscia/lumen-sdk` re-exports `Composition`, so you don't need both.

## Audio

`addAudio` places a clip with times in seconds. `fadeIn` and `fadeOut` fade it from and to silence on an equal-power curve, and `gain` takes a number or an `Expr<number>` evaluated in the composition's time, so a track can duck under a voice-over:

```ts
import { Composition, time, when } from "@lumiscia/lumen-shared";

const composition = new Composition({ timeline: { durationSeconds: 10 } });
composition
  .addAudioTrack({ id: "music", gain: when(time.gte(2).and(time.lt(6)), 0.25, 1) })
  .addAudio({
    source: "music.mp3",
    start: 0,
    duration: 10,
    trackId: "music",
    fadeIn: 1,
    fadeOut: 1,
  })
  .addAudio({ source: "voice.wav", start: 2, duration: 4, trackId: "voice" });
```

A template can have its own sound. It plays wherever a `clip` plays the template, following the clip's start, trim, speed and end like its frames; speed changes pitch:

```ts
import { Composition, defineComposition, prop } from "@lumiscia/lumen-shared";

const whoosh = defineComposition({
  name: "whoosh",
  duration: 30,
  props: { level: prop.number(1) },
  build: (composition, { level }) => {
    composition.addAudio({ source: "whoosh.wav", start: 0, duration: 0.5, gain: level });
    return composition.addNode({ type: "background", params: { paint: [0, 0, 0, 0] } });
  },
});

const composition = new Composition({ timeline: { durationSeconds: 3 } });
const first = composition.addNode(whoosh.clip({ start: 0 }));
const second = composition.addNode(whoosh.clip({ start: 45, speed: 2, props: { level: 0.5 } }));
const merged = composition.addNode({ type: "merge", inputs: { base: first, overlay: second } });
composition.addNode({ type: "media_output", inputs: { source: merged } });
```

See the [Lumen repository](https://github.com/lumiscia/lumen-oss#readme) for the full quickstart and `examples/`.
