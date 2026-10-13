---
aliases:
  - sub-project cascade
  - inherited project blocking
  - parent project blocking
title: Sub-projects Inherit Project Blocking
origin: requested
scope: project
summary: A blocked project passes its dependency or schedule reason through its project descendants, with reconcile as the authority and the ^prj tag as the proxy for other engines.
---
**Applies to.** bob-cli, bob-plugins, vault.

**Claim.** A project note has at most one parent, stored as a scalar frontmatter
`parent:` wikilink. The exact vault-relative path wins; otherwise only a unique,
case-insensitive basename resolves. List-form, ambiguous, unresolved, self, terminal,
and non-project parents do not create inheritance. Every active project inherits each
ancestor's own `^prj` Blocked reason, nearest ancestor first. An inherited `^prj` is
Blocked `[?]` and tagged `#prj_is_blocked`; every open resident is Blocked and tagged
when its project is blocked by its own or an inherited reason. A project's own `^prj`
reason does not tag that same `^prj` line. Reconcile derives and reverses the cascade;
the tag is the stored proxy read by engines that do not walk the tree.

The Task Card applies and reverses the cascade in the same write for Depends on changes
and, under `schedule_cascade = yes`, Schedule changes to `^prj`. Other writers respect
the tag and leave descendant recovery to the next reconcile; `close_recovers_subprojects
= no`. When closing a child project's `^prj` makes its parent project empty, the parent
`^prj` is pulled in as Next (`docs/task-dependencies.md` §12.10 trigger (c)). A project's
prerequisite may not target a task in its own descendant.

**Why.** A blocked project's work must not reappear as Ready merely because it lives in a
child note. Reconcile already knows the project tree and all project reasons. Storing
the inherited fact on the child's `^prj` lets close paths and plugin link guards enforce
the same rule without copying dependency edges or walking the tree on every gesture.
Trigger (c) keeps a parent project moving when its last open child project closes.

Rejected alternatives:

- **Residence-only blocking.** A child's Ready lane would surface work that remains
  blocked by an ancestor.
- **Copying ancestor prerequisites or dates into descendants.** Duplicate edges and
  schedule fields create new writers and drift; the tag records the inherited fact.
- **Immediate descendant recovery on close or status cycling.** That expands each close
  into a tree write; reconcile repairs the tags on its next pass.
- **Stem matching ambiguous parents.** It can select the wrong note; only a unique
  basename may resolve when the exact path is absent.

Evidence: `plan:202610/subproject_cascade.md`; `docs/task-dependencies.md` §§12.10–13;
epic `bob-cli-78` and its contract, reconcile, close, and cycler phases; task-status-cycler
1.37.0 and block-id-prompt 1.30.0.

**Cost.** Non-Task-Card writers can leave descendants tagged until reconcile runs. The
project tree and tag proxy must stay consistent across Rust and plugin readers. A parent
`^prj` with a descendant target is refused to avoid a dependency cycle.

**Reopens when.** Projects need multiple parents, descendant recovery must happen
immediately outside the Task Card, or the parent relationship stops being a scalar
frontmatter wikilink.

Supersedes in part [[decisions:projects-block-from-prj-line]]: blocking by residence
alone is retired; a project's own prerequisites and date still originate on its `^prj`.
