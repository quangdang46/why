---
id: design-intent-lookup
title: Design Intent Lookup
summary: Recover the original design tradeoff behind a symbol or subsystem before a refactor.
target_hint: symbol
---
1. Query the public symbol or the smallest private symbol that anchors the design.
2. Inspect adjacent markers and comments for migration or compatibility language.
3. Compare early commits with the most recent hotfixes to separate original intent from later hardening.
4. Record the intended invariant, any surviving compatibility obligations, and which surrounding files co-change with the target.
5. Hand the resulting narrative to the refactor plan instead of rewriting the rationale from memory.
