---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-preview": minor
---

Preview audio now mixes like export and reads the timeline the engine parsed, stops as soon as a composition or media reload begins, and reports sources that fail to load instead of failing silently. Replaced or removed media is cancelled and can no longer overwrite newer sources. The package no longer ships its tests. Decoded frames and inputs no longer leak, and failed media loads can be retried. Breaking: `timelineMode`, `native-samples`, the `audioTimeline` session option, Mediabunny `Source` inputs and the audio schedule and parsing helpers are gone, and a video with no `fps` and no readable frame rate is now an error.
