---
type: reference
parent: AGENTS.md
description: Read anytime new CLI subcommands or options are added.
---

# CLI Rules

When adding or changing CLI subcommands or options:

- Make `-h|--help` output excellent: clear, complete, consistent, and easy to scan.
- Keep options sorted alphabetically. Keep subcommands alphabetical within each help
  section; `bob --help` orders its sections by the daily workflow (Daily workflow, Tasks
  and projects, Vault, Integrations, Setup, Capture protocol), and `bob <TAB>` uses the
  same sections as completion groups.
- Give every public long option a short alias; this does not apply to internal
  subprocess arguments.
- Prefer beautiful, colored output over black-and-white output when color improves readability.
- Never hard-rename or remove a command: the old spelling becomes a permanent hidden
  alias with byte-identical behavior and no deprecation output. Help, diagnostics, logs,
  and commit subjects print only the canonical path.
- Nest only under a noun with a stated membership rule. `bob task <verb>` is reserved
  for commands that rewrite task lines across the whole vault; read-only reports stay
  top-level.
- A bare group may default only to a flag-only, read-only member; `vault-sync` (bare =
  `run`) is the grandfathered exception and is labeled in help.
- Never add a subcommand under `bob capture` (its free-text TEXT would swallow it); the
  `capture-*` protocol names are frozen siblings that may only grow additively.
