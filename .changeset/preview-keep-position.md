---
"@lumiscia/lumen-preview": minor
---

Updating a preview's composition now keeps the playback position instead of rewinding to frame 0, and a preview that was playing keeps playing. The worker starts the new composition on the clock it was already following, so frequent updates, such as from a dragged slider, neither hold playback back nor step the frame backwards. A paused position past the new end parks on the last frame, and playback carries on round a shorter loop. Audio with an unchanged timeline plays on undisturbed. A changed audio timeline restarts the mix at the current position. A seek made during an update applies once it loads, clamped to the new length.
