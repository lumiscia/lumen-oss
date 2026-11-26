---
"@lumiscia/lumen-react": patch
---

`Player` no longer reads its composition again each time a parent re-renders with a prop written inline, such as `range={{ in: 0, out: 40 }}` or `onEnded={() => ...}`. It sends its inputs to the preview only when one changed: the composition by identity, and every other input by its content one level deep. A callback keeps one identity and always calls the newest function you passed.
