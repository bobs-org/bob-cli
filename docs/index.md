---
title: Bob
description: Capture a thought, choose today's work, and keep an honest record with Bob.
hide:
  - navigation
  - toc
---

# Bob

**Capture a thought. Choose today's work. Keep an honest record.**

Bob is a Rust command-line tool with Obsidian plugins for planning and review, plus an
optional Mac menu-bar panel for capture. The vault stays in Obsidian and syncs through
Git; the Mac panel only submits drafts and never owns the workflow.

Linux and macOS · Obsidian

[Is Bob for me? Start with getting started](getting-started.md).
[Read the behavior contracts](develop/contracts.md).

## One thought · three surfaces · one vault

<div class="bob-home-cards" markdown="1">

**Terminal.** Type the capture and see the inbox line it writes.

```bash
bob capture "Review the queue input"
```

```text { .no-copy title="Output" }
✓ captured  mac_inbox.md
  - [ ] #task Review the queue input [created::2026-10-20]
```

The inbox note holds `- [ ] #task Review the queue input [created::2026-10-20]`,
verified against a synthetic scratch vault with a fixed clock.

**Obsidian.** The same line renders as a task card.

```bob-task
- [ ] #task Review the queue input [created:: 2026-10-20]
- [*] #task Fix the parser crash [scheduled:: 2026-10-21]
- [x] #task Ship the queue filter [completion:: 2026-10-19]
```

**Menu bar.** The optional Mac panel shows the mark and today's queue.

![Bob mark](assets/bob-mark.svg)

The panel is illustrative; the Mac phase owns the real screenshot.

</div>

## A Bob day

[Morning review](freshness.md) clears the queue, [capture](capture.md) takes the next
thought, [Pomodoros](pomodoro-homes.md) hold today's work, [close](task-dependencies.md)
links what unblocked, and [sync](vault-git-sync.md) keeps the vault honest.

## Doors

<div class="bob-doors" markdown="1">

**Develop.** Build and maintain the site.

- [Build the docs](develop/build.md)
- [Writing docs](develop/writing-docs.md)
- [Behavior contracts](develop/contracts.md)

**Look up.** Reference that never drifts from the tools it describes.

- [Look up Bob](reference/index.md) for the CLI reference, glossary, and design decisions.

</div>

## Source

Bob lives in three public repositories: [bob-cli](https://github.com/bobs-org/bob-cli),
[bob-plugins](https://github.com/bobs-org/bob-plugins), and
[bob-mac-capture](https://github.com/bobs-org/bob-mac-capture), under the
[MIT license](https://github.com/bobs-org/bob-cli/blob/master/LICENSE).
