---
keyword: Unblocked Tasks Are Offered, Never Auto-Linked
summary: Every would-be successor link from a managed Done gesture is offered for
  explicit selection; recovery remains automatic.
metadata:
  status: accepted
title: Unblocked Tasks Are Offered, Never Auto-Linked
aliases:
- unblocked-tasks-are-offered
---

# Unblocked Tasks Are Offered, Never Auto-Linked

*Requested · project*

aka successor offers, pull-in offers

**Applies to.** bob-cli, bob-plugins, Bob Mac Capture, vault.

**Claim.** Managed Done gestures keep recovery automatic: tasks still unblock, lose
`#prj_is_blocked`, and receive the `[unblocked::]` WRITE stamp when applicable, whether
or not anything is picked. Every would-be successor link is instead a
[[glossary:successor-offer]]: direct dependents (`hand_off`), open residents of a
recovered project (`project`), and project-finished pull-ins (`project_finished`). One
picker appears per gesture when offers exist; nothing is preselected and there is no
offer cap. Only picked rows are linked and raised to Next, receive a missing block ID,
and get `[fresh:: today]` (clearing `[unblocked::]` and `[keeps::]`). Unpicked rows
recover without a link and resurface through the existing freshness rules. `bob capture`
never prompts or auto-links: callers make an explicit selection, while the Mac presents
the choice in-panel and submits it to bob.

**Why.** A close once encountered eleven direct dependents and the all-or-nothing limit
linked none, leaving Bryan to hand-link selected tasks, including one in another
Pomodoro. A picker lets the person choose among all useful successors while retaining
automatic recovery and making project residents visible even when they are not direct
dependents. Project groups enumerate residents of the recovered project's note;
project-finished pull-ins retain their existing recovery and `#hide` removal, with the
link offered separately. The detailed contract is `docs/task-dependencies.md` §12 and
the accepted design is `plan:202610/successor_offers.md`.

Rejected alternatives:

- **Raise the cap.** Any finite cap still drops a valid fan-out, while an unbounded
automatic pass can plan too much work.
- **Auto-link direct dependents only.** It leaves project residents invisible and keeps
different successor classes inconsistent.
- **Skip the picker for one offer.** Even one candidate needs an explicit choice.
- **Choose after commit.** A later picker cannot reliably revalidate and atomically
apply the same gesture.
- **Use `>1,3` as the selection grammar.** Number and key selection is explicit in
capture; the UI owns its own multi-select interaction.
- **Put checkboxes in the Mac preview.** Choice is a distinct in-panel step, and bob
remains the sole capture and write authority.
- **Never prompt during the review walk.** A managed Done gesture has one consistent
offer choice, including when it began in the walk.
- **Use a tray of held offers.** Offers are tied to the close and are immediately
applied or left to recover; no separate queue is needed.

**Cost.** The Rust, cycler, navigation, and Mac surfaces must preserve one offer/key
contract. The picker pauses a managed Done gesture and each selected write must be
revalidated against current note text. Capture clients must understand additive offer
and pull-in output, and older Bob binaries need capability fallback in the Mac client.
Unpicked recovery continues to rely on the existing review tiers; no later rescue
command is included.

**Reopens when.** People need to retrieve offers after the gesture; offer sets become
too large for grouping and filtering; or a supported client cannot preserve the shared
selection and write contract.

Evidence: `plan:202610/successor_offers.md`; epic `bob-cli-7c` and phases
`bob-cli-7c.3`–`bob-cli-7c.7`; bob-cli `2d1adfa` (contract), `f5c82d6` (planner),
`e7a0c7b` (capture); Bob Mac Capture `6c4209b` (in-panel choice).

[[decisions:unblocked-tasks-resurface-as-ticklers]]

[[decisions:projects-block-from-prj-line]]

[[decisions:subprojects-inherit-project-blocking]]

[[decisions:mac-capture-is-a-thin-client]]
