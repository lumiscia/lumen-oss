---
"@lumiscia/lumen-preview": patch
"@lumiscia/lumen-bindings": patch
---

The preview converts video from YUV to RGB itself, with the same matrix as a server render, instead of leaving it to the browser. Untagged video now looks the same in the preview and in a render: below 720 rows it's BT.601, and from 720 rows it's BT.709. Chrome used to pick BT.709 for sizes like 960×540 and 720×576, and Safari for everything, including video tagged BT.601. Video also shows in Safari again, which ignored the RGBA conversion and left frames black. Frames in another format, and rotated video, still go through the browser's conversion.
