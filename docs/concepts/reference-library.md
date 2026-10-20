---
title: "The reference library"
description: "How sources, reference notes, and reading tasks fit together without a second workflow."
search:
  boost: 2
tags:
  - CLI
  - Obsidian
---

# The reference library

This concept explains how a source, its reference note, and its reading task stay distinct without becoming a second task system.

A source is the PDF or web material itself. A reference note is the vault file under `ref/` that gathers the source link, notes or highlights, and follow-up work. A reading task is the `^ref` task in an area, project, or inbox note that represents progress through that material. The note holds what was learned; the task holds whether reading is done.

```markdown
- [ ] #task Read the seed-starting notes ^ref-seed-starting
```

The demo vault's `ref/seed-starting.md` note is the reference; a task like this one, in whatever note owns the reading, is the handle that Today, lanes, and review all operate on. The note never doubles as the task, and the task never stores the annotations.

## Reading state without a second workflow

Reading state is derived — queued, started, finished, or dropped — from the reference note and its reading task, not tracked as an independent workflow. A reading task can be linked into Today and is reviewed by the same ordinary freshness rules as other tasks: an unconfirmed reading task is NEW, a resurfaced one is TICKLER, and an expired one is ROTTEN. Blocked is a separate overlay on reading state: a reading task waits like any other task when its prerequisites are open or its schedule is future, and unblocks the same way.

Intake is not yet the indexed library. A queued background capture job or a PDF waiting in `xlib/` does not appear in the reading queue until a scan files its note; only notes under `ref/` are indexed. The producing host normally files a reference at creation, under the maintenance and scan locks, so the note is available from the host that created it. When ingestion is unsafe, the PDF stays queued on that host for its local intake drain. Annotation synchronization remains separate: annotations are a scan snapshot, and syncing them is distinct from importing the reference. Import and recovery runbooks own those procedures; this page only sets the vocabulary.

## Identity that survives renames

Three identifiers serve three joins. URLs support deduplication: asking whether a source is already known compares its URL, not its filename. A recorded reference ID follows its note stem, with a numbered suffix only when two references would otherwise share an ID across `ref/`. The reading-task block ID (for example `^ref-seed-starting`) remains stable across supported bundle renames, so Today links, Task Links, and dependency edges keep resolving.

Rename guarantees cover supported unannotated references: moving the note, PDF, and same-stem companions within the reference directory while preserving the reading-task identity and updating inbound links. Arbitrary file moves and annotated references are outside that guarantee, because highlight anchors and follow-up digests depend on paths the rename does not re-key.

## Related

- [Your first Bob day](../start/first-bob-day.md) — save a link and read it from the queue.
- [Tasks and lanes](tasks-and-lanes.md) — how a reading task moves through lanes.
- [Freshness and the review walk](freshness-and-the-review-walk.md) — how reading tasks are reviewed.
- [Understand Bob](index.md) — the eight nouns and where references fit.
- [Reference library](../ref.md) — reading state, identity, and rename guarantees.
- [Ref jobs](../ref-jobs.md) — background capture progress, worker, fallback, and recovery.
- [Highlights create](../highlights-create.md) — vault-note snapshots, PDFs, and URL collections.
- [Highlights Reference Note Sync](../highlights-ref-sync.md) — PDF intake and annotation sync.
