---
keyword: Rotten Keeps Are Logged
aliases:
  - rotten log
  - keep log
summary:
  "Every counted keep — Alt+F/Ctrl+Alt+F on an exact due Ready ROTTEN or
  scheduled TICKLER row, or the decay card's Keep — asks an optional reason
  and writes one dated 🥀 ROTTEN LOG entry with the stamp; the entry count is
  display-only history, never a field or a decay input."
metadata:
  status: accepted
  decided: 2026-10-23
---

**Applies to.** bob-plugins (bob-navigation-hotkeys, bob-ledger-tools), bob-cli
(managed-log registry, agenda JSON, note snapshots, docs), Bob Mac Capture (agenda
chip), and the vault.

**Claim.** A counted keep is an exact, due, Ready rotten/tickler target that the
single keep helper would increment: an exact pre-write queue row in the Ready lane
with tier rotten or scheduled tickler, not returned by an unblock, not already
stamped today, and confirmed by a dry run of the keep helper. Every counted keep —
a single Alt+F/Ctrl+Alt+F press below the decay limit, the decay card's Keep,
counted `N<Alt+F>` targets, and Task Link counted targets — first asks "Why keep
it as written?" and then writes exactly one dated entry under a `🥀 **ROTTEN LOG**`
direct child in the same transaction as the stamp, with one undo. The reason is
optional: a blank answer logs `🤷 no reason given`. Esc writes nothing: no stamp,
no entry, no advance. The entry reads `*YYYY-MM-DD* — <reason> · kept N×` with
the local review date, newest first directly under the marker; N is the
[[keep-streak]] after that keep, 1–999. The log is audit and display only: its
count ("N keeps logged") never resets, is never stored as a field, and nothing
reads it to change decay, buckets, tiers, lanes, Today, schedule, priority, or
[[glossary:task-freshness]]. NEW, early/FRESH, Pending, Next, PROJECTS, recurring,
checklist, closed, and unblock-tickler keeps never prompt and never write it;
Pending summaries stay in the [[glossary:work-log]] and deferrals in the
[[glossary:schedule-log]]. `bob capture`, Rust, hooks, randomize, and seed never write it.
There is no backfill: keeps before rollout leave no entries.

Rejected alternatives:

- **Nonblank-only entries.** A blank reason logs `🤷 no reason given`; the entry is
  never optional.
- **Logging every Alt+F.** Only counted keeps log; uncounted stamps stay silent.
- **ROTTEN tier only.** Scheduled ticklers log; unblock ticklers do not.
- **A lifetime field or numeral by the mark.** The entry count is display-only and
  never stored on the task line.
- **Schedule- or Work-Log storage.** Keeps get their own `🥀 **ROTTEN LOG**` child.
- **The journal as the store.** The log lives with the task and travels with it.
- **A second-keep gate.** Every counted keep prompts, from the first.

Evidence:
`research:202610/rotten_log_keep_history/rotten_log_keep_history.md`;
`plan:202610/rotten_log.md`;
`docs/freshness.md` §2a and `docs/task-dependencies.md` §2.4 in bob-cli.

**Cost.** One more kind in the closed registry across Rust and every plugin parser;
a two-key keep (Alt+F, then Enter); log growth of about four lines a month on a
weekly-kept reading task; two numbers to keep apart (resetting streak vs.
never-reset logged count); editable, not tamper-proof Markdown.

**Reopens when.** A month of entries is more than 80% `🤷`, or counted keeps exceed
about five per morning (consider a from-the-second-keep gate), or the log goes
unread.

Supersedes in part [[decisions/rotten-keeps-use-priority-decay]] for the
rejection of a new REVIEW LOG structure and "Keep anyway leaves only the line
increment" only; counting, explicit consent, the priority ladder, and the other
card outcomes stand.
