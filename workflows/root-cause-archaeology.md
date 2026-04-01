---
id: root-cause-archaeology
title: Root Cause Archaeology
summary: Build a commit-backed narrative for a regression, outage, or suspicious behavior shift.
target_hint: symbol-or-range
---
1. Resolve the smallest symbol or line range that reproduces the behavior.
2. Run the default `why` report first to capture current risk and commit evidence.
3. Expand with `--blame-chain` or `--evolution` if the latest commit looks mechanical or rename-heavy.
4. Cross-check the top commits for issue refs, rollout markers, and migration language.
5. Summarize the likely change sequence, the operational pressure behind it, and what would have to stay true before editing.
