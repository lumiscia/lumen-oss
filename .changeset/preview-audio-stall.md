---
"@lumiscia/lumen-preview": patch
---

When audio starts and then stops arriving for good, the preview no longer freezes silently. After 10 seconds of continuous underrun during playback it reports `audio_failed` once and keeps the video playing without audio until the next play, seek or composition load.
