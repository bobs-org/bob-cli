---
type: reference
parent: AGENTS.md
description:
  Read before changing docs/, mkdocs.yml, tools/docs, README front doors, or
  user-visible CLI help.
---

Read before changing docs/, mkdocs.yml, tools/docs, README front doors, or user-visible
CLI help.

Bob documentation uses URL=path: directory names identify page types, root Markdown
files hold the homepage and behavior contracts, and contracts specify observable
behavior. Guides explain user goals; references own exact behavior and CLI facts.

| Change | Documentation route |
| --- | --- |
| New CLI flag | `just docs-gen` and clap help |
| New hotkey | Plugin reference |
| Behavior change | Contract/reference, plus only guide steps made incorrect |
| New goal | Guide candidate |
| Design decision | Decision record |

Run `just docs-check` for the documentation gate. Mounted sibling docs are owned by
their source repository. Information architecture moves require an approved plan and
preserved published URLs.
