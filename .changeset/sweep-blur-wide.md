---
"@lumiscia/lumen-bindings": patch
---

Wide radial blurs about the canvas centre and long directional blurs render about ten times faster: a 1080p spin of several hundred degrees, or a line blur thousands of pixels long, now takes around 10 ms on an integrated GPU instead of 130–180 ms. Output stays the same, apart from up to a pixel of softening on the edges of shapes that the spin leaves in place, such as circles about its centre.
