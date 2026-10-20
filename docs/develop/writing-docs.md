---
title: Writing docs
description: Choose the right Bob documentation page and keep it accurate, portable, and easy to maintain.
---

# Writing docs

This page explains where Bob documentation belongs and how to keep its content useful
and maintainable. The `bob-docs lint` command checks the mechanical rules; authors still
choose what readers need.

## Page types and placement

The directory is the page type. Keep authored content in a typed directory; root-level
Markdown files are reserved for the homepage and the behavior contracts that stay beside
the code they specify.

- `start/` holds tutorials that walk through one path from beginning to end.
- `guides/` holds instructions for completing one user goal.
- `concepts/` explains how a feature or model works without asking the reader to perform
  a task.
- `reference/` records exact commands, options, formats, and behavior.
- `develop/` holds contributor instructions, build guidance, design context, and
  maintenance procedures.

Use two questions to place a page:

1. Is the reader doing something, or trying to understand something? Doing belongs in a
   tutorial or guide; understanding belongs in a concept or reference.
2. Is the reader learning, or completing a task? Learning belongs in a tutorial or
   concept; completing a task belongs in a guide or reference.

A tutorial teaches one route without alternatives. A guide answers one goal; split a
guide that has grown into several goals. A concept may use one diagram. Reference titles
name the thing being documented, while guide titles name the reader's goal.

Behavior contracts are specifications: they define observable behavior and should remain
precise enough to compare with implementation and conformance examples. User guides
explain how to use that behavior. Link to a contract instead of copying its full rules
into a guide.

## Templates

Copy the closest starting point from `docs/_templates/`. Templates are excluded from the
published site.

- `tutorial.md` starts with the outcome, lists prerequisites, gives ordered steps with
  visible confirmations, and ends with what the reader has achieved and links for what
  to do next. Keep the `search` boost.
- `how-to.md` has a goal title, prerequisites, confirmed steps, one failure section, and
  related links.
- `concept.md` explains one idea, uses at most one diagram, and links to the guide and
  reference that apply. Keep the `search` boost.
- `reference.md` records facts in tables or short lists. Generated values belong inside
  marked generated blocks and are never edited by hand.
- `hub.md` gives one sentence of orientation and groups links by reader need.

Every page needs front matter with a `title` and a one-line `description` of at most 160
characters. Use only supported front matter keys. Finish a reader-facing page with
`Next` or `Related` links so the reader can continue.

## Page budgets

Budgets keep each page focused. Word counts exclude front matter, fenced code, HTML
comments, and generated blocks. The current defaults are:

| Page type | Limit |
| --- | --- |
| Tutorials | 2,500 words per page; 10,000 across the type |
| Guides | 1,500 word target; 2,500 hard limit |
| Concepts | 3,000 per page; 25,000 across the type |
| Hubs and homepage | 400 per page |
| Hand-written reference | 6,000 per page |
| Develop | 8,000 per page, warning only |

`docs/_meta/budgets.yml` can set a tighter page ceiling. The lint's explicit `--fix`
mode lowers a ceiling when its page shrinks; normal lint and builds do not update
ceilings. Split a page that exceeds its limit and link related facts instead of repeating
them.

## Generated content and source ownership

Generated blocks use matching `BEGIN GENERATED: name` and `END GENERATED: name`
markers. Preserve their markers and run `just docs-gen` to regenerate contract indexes,
navigation, and the README contract table. Use the generator's check mode when reviewing
drift; do not hand-edit generated output. Whole generated pages identify their generator
at the top.

The contract registry in `docs/_meta/contracts.yml` owns contract titles and summaries.
Mounted reference pages are owned by their source repository. Edit a mounted page in
that repository and update its local nav metadata; do not edit its copied site output.
The bob-cli docs repository owns `docs/develop/`, authored contract landing pages, site
navigation, and documentation metadata.

## Writing and verification

Use plain Markdown that works in the strict Zensical build and the Material fallback.
Prefer relative links for pages in the same source tree. Use stable published URLs when
linking across repository or mount boundaries. Give headings stable, descriptive text
because published links may include their anchors. Document current behavior, commands,
and output only; if source and prose disagree, fix or report the source discrepancy
before publishing a claim.

Keep command examples short and verify flags against the current `--help` output. Tests
must not pin ordinary guide prose; put testable behavior in contracts and executable
conformance cases. Use temporary directories or a temporary `HOME` for commands that
write files or inspect user state. Run `just docs-check` for the complete documentation
gate, and run the repository checks for code changes.

### Portable Markdown

Lint rejects markup that only one builder renders. Follow these rules on every page:

- In `bash`, `sh`, `shell`, `zsh`, and `console` fences, do not start a command with a
  `$` prompt. Write the command alone.
- Show command output in a separate `text` fence titled `Output` and marked `.no-copy`:

  ````markdown
  ```bash
  bob plan -b /path/to/vault
  ```

  ```text { .no-copy title="Output" }
  bob plan - Tue 2026-10-20 - no daily note yet
  ```
  ````

- Do not use rendered `!!!` admonitions, `===` content tabs, or `++key++` key markup.
  Use GitHub callouts such as `> [!NOTE]` and `<kbd>` for keys.
- Use `bob-task` fences for static task examples. The `today` flag is an explicit
  illustration annotation for open rows and is never inferred:

  ````markdown
  ```bob-task
  - [ ] #task Review the queue input [created:: 2026-10-20]
  ```
  ````

  An exact `#task` token renders as the teal identity mark; other tags render as
  pills. Canonical dates render as absolute marks with the year.
- Do not add MkDocs `hooks:` or plugins outside the allowlist (`search`, `redirects`,
  `tags`, `llmstxt`). Generate extra files in `tools/` before the build.

### Hygiene

Start, guides, and concepts pages must not name bead IDs, cite `src/` paths, carry epic
or phase labels, or use history wording (`formerly`, `no longer`, `legacy`, `renamed`).
Reference pages must not name bead IDs. Put implementation paths and tracker IDs in
develop pages or in code, not in reader docs.

Record images in `docs/_meta/media.yml` with their source and creation date. Do not add
unlicensed assets, screenshots with private data, credentials, or personal vault content.
The privacy patterns and baseline under `docs/_meta/` are part of lint and must be
updated only when the inspected content is intentionally public.

Navigation is authored in `mkdocs.yml`; add a page there when it should appear in the
site. Do not add empty navigation groups or placeholder pages. Keep mount placeholders
aligned with `docs/_meta/mounts.yml` and the source repository's nav. A page move changes
information architecture and published URLs, so make it under an approved plan and
preserve its old URL with a redirect or a useful compatibility page.

## Related

- [Build the docs](build.md) — commands for the documentation site and each source repository.
- [Behavior contracts](contracts.md) — specifications for Bob's user-visible behavior.
