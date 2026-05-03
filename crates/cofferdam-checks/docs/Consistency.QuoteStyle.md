---
id: Consistency.QuoteStyle
category: Consistency
base_priority: -5
default_severity: Info
options: []
---

Mixed quote styles within a project hurt scannability. The full implementation is gated on the engine's two-pass mode (cd-d1y): pass 1 learns the dominant quote style across the corpus; pass 2 flags deviations. Today this is a stub that emits no findings.
