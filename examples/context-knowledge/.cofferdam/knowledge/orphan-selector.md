---
title: Orphan selector demo
match:
  paths: ["src/nonexistent/**"]
---
Demonstrates `--lint-knowledge`'s orphan-selector check: the glob
compiles fine, but it matches zero files in this fixture repo, so
`cofferdam context --lint-knowledge` reports it as an orphan selector
and exits nonzero.
