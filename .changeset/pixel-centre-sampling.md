---
"@lumiscia/lumen-bindings": patch
---

Sample Transform and Resize at pixel centres so identity transforms with linear sampling reproduce the input exactly, and add a Transform `custom_pivot` option so a (0, 0) pivot can mean the top-left corner instead of the source centre.
