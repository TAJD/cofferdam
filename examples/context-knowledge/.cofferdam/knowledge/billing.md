---
title: Billing invariants
match:
  paths: ["src/billing/**"]
  layers: ["billing"]
priority: high
---
Billing code must never round intermediate values; all money math goes
through a `Money` type. Schema changes here require a migration
reviewed by a human.
