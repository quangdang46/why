---
id: regression-context-gathering
title: Regression Context Gathering
summary: Gather archaeology context around a suspicious diff before review or rollback decisions.
target_hint: staged-diff-or-symbol
---
1. Start from the staged diff or the symbol most likely to carry the regression.
2. Collect the default `why` report plus `--coupled` output for neighboring files.
3. Identify whether the change touches HIGH-risk history, transition logic, or ownership bottlenecks.
4. Capture the top commit summaries and any missing evidence you still need from issue trackers or deploy logs.
5. Produce reviewer notes that distinguish direct evidence, inference, and rollback risk.
