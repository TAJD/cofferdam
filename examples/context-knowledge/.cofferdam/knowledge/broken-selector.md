---
title: Broken selector demo
match:
  paths: ["src/[unterminated"]
---
Demonstrates load-time validation: `src/[unterminated` is not a valid
glob (unterminated character class), so this note's `paths` selector
is dropped and `cofferdam context` / `--lint-knowledge` warn loudly
about it instead of silently matching nothing. Run:

```
cargo run -p cofferdam-cli -- context --lint-knowledge
```

from this directory to see the warning.
