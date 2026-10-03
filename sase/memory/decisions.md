---
web: true
description:
  Accepted architecture and policy decisions for bob-cli, bob-plugins, Bob Mac Capture,
  and the Bob vault — the choice, its rejected alternatives, and what would reopen it.
roster: list
roster_label: DECISIONS
strand_noun: decision
---

# Decisions

Accepted architecture and policy decisions spanning bob-cli, bob-plugins, Bob Mac
Capture, and the Bob vault. Each roster summary is a rule to follow as written. Before
changing behavior a record governs, or proposing one of its rejected alternatives, read
it with `sase memory read decisions:<keyword> -r "<why>"`; each record states where it
applies, the claim, why it beat the credible alternatives, what it costs, and what would
reopen it. A record is not a design doc, runbook, command contract, or keymap — those
live in `docs/` and each repo's README. Records cite checkable evidence (a commit, doc,
plan, or research ref); when no source records the reason, ask Bryan rather than infer
one. A record is immutable once accepted: if course changes, write a new record and mark
the old one with `metadata.status` plus `superseded_by` and a `[[...]]` back-link, never
edited in place.

<!-- sase:strands -->

1. **#now Is A User-Owned Weekly Bet, Never A Status** (`now-tag-is-user-owned`)
   - _[superseded by `task-lanes-are-sticky`, `today-is-read-from-the-ledger`]_ Only
     Bryan's explicit gestures add or remove #now; no automation infers, adds, or strips
     it. It never changes task status or feeds Next, and it stays a tag, never an inline
     field.
2. **Active Task Statuses Are Derived, Not Authored** (`task-status-is-derived`)
   - _[partly superseded by `task-lanes-are-sticky`, `task-deps-are-depends-on-links`]_
     Today's Pomodoro ledger drives Next and In Progress; open dependencies and future
     scheduled dates drive Blocked. bob task-status-hooks reconciles them, so writers
     change those inputs, never just the checkbox.
3. **Bob Mac Capture Is A Thin Client Of bob** (`mac-capture-is-a-thin-client`)
   - Bob Mac Capture never parses capture grammar, computes previews, or writes the
     vault; it runs bob, renders the spans, candidates, and previews bob returns, and
     submits each draft as one bob capture call.
4. **Next And Pending Are Sticky Lanes; Only Blocked Is Derived**
   (`task-lanes-are-sticky`) - Linking raises Ready to Next and an =x close sets In
   Progress (PENDING); no unlink, hooks run, or capture drop lowers them; only Alt+N
   release returns a task to Ready; Blocked stays derived.
5. **Note Ready Cap Counts The Lane** (`note-ready-cap-counts-the-lane`)
   - The per-note soft cap (plan.max_ready_per_note, default 5; ready_cap: N|off) counts
     each area/project note's whole Ready lane by residence, whatever its freshness.
6. **READY Is Freshness-Gated With NEW and ROTTEN Review** (`ready-is-freshness-gated`)
   - _[partly superseded by `note-ready-cap-counts-the-lane`, `review-walk-is-tiered`]_
     READY is the freshness-gated confirmed/exempt backlog (visible TODO pool minus NEW
     and ROTTEN buckets) with TODAY → NEW → PENDING → NEXT → READY sections and
     NEW/PENDING/NEXT/READY/BLOCKED/ROTTEN/TODAY chips; review clears NEW then ROTTEN;
     no tags, fields, or status changes store review state.
7. **Review Walk Is Tiered With Daily Lane Review** (`review-walk-is-tiered`)
   - The ]s walk visits one shared queue in explicit tiers NEW → PENDING → NEXT →
     RETURNED → ROTTEN; Pending and Next tasks come due for daily review under
     pending_interval / next_interval (default 1, false walks that lane off); tiers
     never feed buckets or chips; upkeep outside the lanes counts the budget; stamps
     stay and the seed never re-runs.
8. **Task Dependencies Are Links On One Depends-On Line**
   (`task-deps-are-depends-on-links`) - A task's prerequisites live as plain task
   dependency links on one managed Depends-On first-child line; that line is the source
   of truth and the [dependsOn::] / [id::] fields are derived from it.
9. **Today Is Read From The Ledger, Never Written To Tasks**
   (`today-is-read-from-the-ledger`) - _[partly superseded by `ready-is-freshness-gated`
   ]_ Today is the open tasks with a dedicated Task Link under today's open Pomodoros,
   computed at read time by bob plan and bob-ledger-tools; never a tag, task-line field,
   or file-path filter; #now is retired.

<!-- /sase:strands -->
