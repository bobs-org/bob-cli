---
type: core
parent: AGENTS.md
---

# Gotchas

bob-cli ships only through the scheduled release train: at most one version per New
York day after full CI on the exact SHA, with no release PR. Never edit `Cargo.toml`
versions or `CHANGELOG.md`; mark CLI or JSON contract breaks with `!`. See
[[decisions/bob-cli-releases-ride-a-daily-train]].
