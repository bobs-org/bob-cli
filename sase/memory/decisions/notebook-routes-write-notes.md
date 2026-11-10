---
keyword: A Notebook Route Writes Notes; `@@` Stays Draft-Wide
aliases:
  - notebook-routes-write-notes
  - destination decides
summary:
  Notebooks hold only note bullets; on a notebook route the existing `@nb` forms
  write notes, `@@` stays draft-wide, `@parent^id@ %n` creates a notebook, and
  bob owns each `## Notebooks` catalog's content.
metadata:
  status: accepted
  decided: 2026-11-09
---

**Applies to.** bob-cli, bob-plugins, Bob Mac Capture, and the vault.

**Claim.** A [[glossary:notebook]] holds only note bullets and never a checkbox.
On a notebook route, the existing `@nb` forms write notes: `@nb` a top-level
note, `@nb#section` a section note, `@nb^id` or `@nb#section^id` a named note,
and `@nb+id` a child of that note bullet. `@@` stays the draft-wide declaration
and never takes a place suffix; on a notebook, unrouted items become notes.
`@parent^id@ %n` creates a notebook whose `summary` is the item text. Bob owns
each area or project's `## Notebooks` catalog content; Bryan owns its position.

**Why.** Research verifications V1–V6 decide it. `@@route` is already a
whitespace-free, draft-wide declaration (V1). Suffixed `@@` forms were removed
on 2026-10-31 (`bob-cli-ey.1.1.1`; V2), and `capture-rewrite` already refuses
place suffixes on absorption (V3). Rewrite is purely lexical and cannot depend
on whether a route is a notebook (V4). Every requested alias already works on
the `@nb` forms (V5). The `@nb` tokens already parsed; only what they write on a
retyped file had to change (V6). Rejected alternatives:

- **Item-local `@@nb`.** It would give `@@` a second scope and collide with V1.
- **Suffixed `@@` forms** (`@@nb#…`, `@@nb^…`, `@@nb+…`). They would reverse the
  2026-10-31 removal.
- **A `#notebook` tag.** It cannot change capture, validation, or discovery.
- **Sub-areas.** They drag in Ready caps, `## Tasks`, Stats roots, and tasks.
- **Folders.** Capture routes are vault-root stems.
- **Record types.** Records are one-entity notes with a schema; a notebook is
  one free-form outline.
- **A Dataview-only index.** Invisible to git, agents, and mobile, and it
  cannot roll up through sub-projects.
- **A `^^` seal.** `%n` already means "how", and it is Bryan's stated intent.

**Cost.** Retyping a file changes what `@x` writes, so the capture grammar
bumped. Every host must run a bob with notebook capture before any vault retype.

**Reopens when.** Bryan wants tasks in notebooks, or a second `@@` scope.

**Evidence.** `research:202611/bob_notebooks/bob_notebooks.md` (V1–V6);
`plan:202611/bob_notebooks.md`; bob-cli `0a74731b` (recognition), `55468bff`
(catalogs), `8d098e55` (note writes), `ab021e2d` (creation), `93cf930c`
(completion and `@@` collapse). See also [[glossary:notebook]],
[[glossary:capture]], [[glossary:project-note-capture]], and
[[decisions:capture-sigils-say-where-what-how]].
