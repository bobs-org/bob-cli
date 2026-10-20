---
title: "Add prerequisites while capturing"
description: "Capture a dependent task with a stable block id, then confirm its Depends-On line and blocked state."
tags:
  - CLI
  - macOS
---

# Add prerequisites while capturing

This guide captures a new task that depends on an existing open task. `&`
writes the edge on the new task's Depends-On line. `*` is the reverse: the
named existing task depends on the captured one.

## Before you start

Install the CLI and point it at your vault. Have an existing open task with
a stable block id, for example `Prep the bench @workshop^prep`. Mac Capture
submits the same draft through `bob capture`.

## 1. Preview a dependent capture

```bash
bob capture --dry-run 'Sand the bench @workshop^sand &workshop:prep'
```

```text { .no-copy title="Output" }
[dry-run] ok would capture  workshop.md
  - [?] #task Sand the bench [created::2026-10-20] [id:: workshop__sand] [dependsOn:: workshop__prep] ^sand
  ⛓ would depend on "Prep the bench"  workshop.md ^prep  [ ] Ready
```

Confirm the preview names `workshop.md`, the new `^sand` id, and the
prerequisite `^prep`. An unfinished `&prep` picker is refused; type a
complete `&note:block-id`. Quote the draft so the shell leaves `&` alone.

## 2. Submit and confirm the Depends-On line

```bash
bob capture 'Sand the bench @workshop^sand &workshop:prep'
```

```text { .no-copy title="Output" }
✓ captured  workshop.md
  - [?] #task Sand the bench [created::2026-10-20] [id:: workshop__sand] [dependsOn:: workshop__prep] ^sand
  ⛓ depends on "Prep the bench"  workshop.md ^prep  [ ] Ready
```

Open `workshop.md` and confirm the new task is `[?]` with a first child:

```markdown
- [?] #task Sand the bench [created::2026-10-20] [id:: workshop__sand] [dependsOn:: workshop__prep] ^sand
	- ⛓️ **DEPENDS ON:** [[#^prep]]
```

The Depends-On child is the source of truth; the `[dependsOn::]` field is
derived. The prerequisite also gets a generated BLOCKS mirror.

On a Mac, type the same draft in the capture panel, confirm the live
preview, and submit. The panel hides only after `bob` reports success.

## 3. Choose `&` or `*` by direction

`&workshop:prep` means the captured task waits on `^prep`.
`*workshop:sand` means `^sand` waits on the captured task (quote `*` on the
shell). Exact token rules live in
[Capture syntax](../reference/capture-syntax.md#ampersand-prereq).

To add a prerequisite to an existing owner with no new task:

```bash
bob capture '^workshop:sand &workshop:prep'
```

## If it goes wrong

An unknown note or block id fails the item and rolls the whole draft back.
Fix the `&note:block-id` spelling (`&projects/foo:bar` for a nested note).
A mid-word `&` is ordinary text. If Mac Capture stays open, the preview
still has an error; complete the `&` target and submit again.

## Related

- [Capture](../capture.md)
- [Task dependency links](../task-dependencies.md)
- [Capture syntax](../reference/capture-syntax.md)
- [Capture a task from anywhere on your Mac](capture-from-your-mac.md)
