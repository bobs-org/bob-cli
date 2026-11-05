---
kind: requested
scope: project
summary: Consuming Task Links keeps notes, removes only emptied entries, matches resolved identities, and retains closed recorded time.
metadata:
  status: accepted
---
*Requested · project*

aka consumption parity, only-emptied cleanup, keep Pomodoro time

**Applies to.** bob-cli and bob-plugins writers that consume or retire
Pomodoro Task Links.

**Claim.** Capture, `bob task reconcile`, and bob-plugins consume links
to effectively Blocked tasks by resolved `(vault-relative path, block
ID)` identity. When a consumed link-only bullet has authored child
notes, the writer replaces the link with the task's clean display text
and keeps the notes. A Pomodoro entry is removed only when that same
operation began with a list-item child and removed its last child;
unrelated childless placeholders stay. `task_complete` retirement
uses the same resolved identity and only-emptied rule when it strikes,
moves, or deduplicates completed-task links. A closed Pomodoro with a
recorded time span, `[t::]`, or `[off::]` remains in the ledger even
when childless, preserving its recorded time.

**Why.** `consumption_parity = yes` makes the reconcile and plugin
writers match capture's keep-notes, only-emptied, and identity rules.
`g4_close_policy = keep` makes recorded time survive a close whose
links defer. Different cleanup rules made the same link gesture lose
authored notes or remove an unrelated placeholder, while deleting an
empty closed timed row erased recorded work or off-clock time.

Rejected alternatives:

- **Remove a link-only bullet with its full subtree.** This discards
  authored notes that capture already preserves.
- **Delete every childless entry found during cleanup.** This removes
  placeholders the current operation did not empty.
- **Match the original link text.** Basenames, paths, case variants,
  aliases, and local links can name the same task.
- **Delete a closed timed row once it has no links.** This loses
  recorded session time and disagrees with the accepted close policy.

**Cost.** Notes can remain as plain text after their link is consumed,
and an unrelated empty placeholder can remain in the ledger. Each
writer must resolve link spellings to the same path-and-ID identity.

**Reopens when.** Bryan changes the accepted consumption or close
policy, or the vault gains a separate durable store for closed session
time that makes retaining the ledger row unnecessary.

Evidence: epic `bob-cli-h8` accepted decisions `consumption_parity =
yes`, `g4_close_policy = keep`, and `memory_consumption_record = yes`;
`bob-cli-7q`; `bob-cli-g4`; `plan:202610/capture_star_blocks.md` §8;
`plan:202611/top_ten_bead_bundles.md` §`consumption-bob`; and
`docs/task-dependencies.md` §§13.5, 13.14.
