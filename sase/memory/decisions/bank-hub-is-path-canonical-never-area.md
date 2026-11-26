---
origin: requested
scope: project
aliases:
  - bank hub is path-canonical
  - bank is not an area
summary: "The untyped, taskless bank.md hub is identified by path and excluded from the Area graph."
---

**Applies to.** bob-cli, bob-plugins, and the Bob vault. Bob Mac Capture
renders Bob's capture refusal unchanged under its thin-client contract.

**Claim.** The vault-root `bank.md` is the untyped, taskless bank hub, identified
by its canonical path. Its `parent: "[[cash]]"` supplies display affinity only;
it is not an Area graph edge. The hub has one owner, the Bank Card, gated by
`props_v1`, with no Area Settings or task semantics. `@bank` refuses task
captures and directs money tasks to `@cash`. The subscriptions album belongs
under cash.

**Why.** The Bank Card already identifies the fixed hub path, while Area notes
are identified by `type: "[[area]]"`. Keeping the Area type gives one note two
title, foot, and frontmatter owners and imports Area graph and task behavior
that the hub does not need. A separate `bank_hub` type adds a taxonomy and
classifiers for an identity already fixed by path. Keeping the Area type and
adding arbitration preserves the same conflicting identities and hidden
settings. A task-route refusal protects the taskless contract even though a
direct `@bank` capture could otherwise write a task.

**Cost.** `@bank` no longer accepts task writes; money tasks use `@cash`.
Subscriptions move under cash, and the bank hub's parent is display affinity,
not Area ancestry.

**Reopens when.** Bryan wants the bank hub to hold tasks or to participate in
the Area graph as a real Area, or a bank behavior requires a type that cannot
be identified by the fixed hub path.

**Evidence.** The canceled `bob-cli-pl` and `bob-cli-q6` beads both record the
same close reason: `~/bob/bank.md` should never have been an area note. The
consolidated research is `research:202611/bank_hub_demotion/bank_hub_demotion.md`;
the approved plan is `plan:202611/bank_hub_demotion.md`. Implementation is in
bob-cli `478d5233` (Area-only props and `@bank` refusal), bob-plugins `8e6a5a61`
(single-owner Bank Card), and bob-plugins `19e895f0` (remove the stale
arbitration fallback).

See [[glossary:bank-card]], [[glossary:property-card]], and
[[decisions:notebook-routes-write-notes]].
