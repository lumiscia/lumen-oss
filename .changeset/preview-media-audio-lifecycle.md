---
"@lumiscia/lumen-bindings": minor
"@lumiscia/lumen-preview": minor
---

Preview audio now mixes like export and reads the timeline the engine parsed, stops with the video when a composition reloads, and reports sources that fail to load instead of failing silently. Decoded frames and inputs no longer leak, and failed media loads can be retried. Breaking: `timelineMode`, `native-samples`, the `audioTimeline` session option, Mediabunny `Source` inputs and the audio schedule and parsing helpers are gone, and a video with no `fps` and no readable frame rate is now an error.
