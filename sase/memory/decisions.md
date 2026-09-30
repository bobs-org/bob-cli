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
   - Only Bryan's explicit gestures add or remove #now; no automation infers, adds, or
     strips it. It never changes task status or feeds Next, and it stays a tag, never an
     inline field.
2. **Active Task Statuses Are Derived, Not Authored** (`task-status-is-derived`)
   - Today's Pomodoro ledger drives Next and In Progress; open dependencies and future
     scheduled dates drive Blocked. bob task-status-hooks reconciles them, so writers
     change those inputs, never just the checkbox.
3. **Bob Mac Capture Is A Thin Client Of bob** (`mac-capture-is-a-thin-client`)
   - Bob Mac Capture never parses capture grammar, computes previews, or writes the
     vault; it runs bob, renders the spans, candidates, and previews bob returns, and
     submits each draft as one bob capture call.

<!-- /sase:strands -->
