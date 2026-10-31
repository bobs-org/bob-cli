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
2. **A Pomodoro Theme Is One Name Component** (`pomodoro-theme-is-one-component`)
   - A theme is one + component of a Pomodoro's — NAME tail, keyed by
     normalize_component, with the catalog derived from canonical daily notes.
3. **A Task Move Never Advances The Walk** (`task-move-never-advances-the-walk`)
   - Ctrl+Shift+M never advances the ]s walk, even from a landed row: it follows the
     moved task to its destination note and parks the walk, so the next ]s / [s resumes
     at the moved row's walk neighbour.
4. **Active Task Statuses Are Derived, Not Authored** (`task-status-is-derived`)
   - _[partly superseded by `task-lanes-are-sticky`, `task-deps-are-depends-on-links`,
     `projects-block-from-prj-line`]_ Today's Pomodoro ledger drives Next and In
     Progress; open dependencies and future scheduled dates drive Blocked. bob
     task-status-hooks reconciles them, so writers change those inputs, never just the
     checkbox.
5. **An Empty Capture Draft Shows Today's Ledger Agenda**
   (`idle-capture-shows-ledger-agenda`) - An empty capture draft shows bob's
   `capture-pomodoros --tasks` agenda (Now, Next, Later), cached in app memory and
   revalidated on filtered vault events and show; folds go farthest-first (logs,
   one-line, one-row, name strip) below a fixed eye line; the app never reads the vault.
6. **Answering A Walk Landing Advances The Walk** (`answering-advances-the-walk`)
   - _[partly superseded by `task-move-never-advances-the-walk`]_ A gesture that started
     on the row ]s just landed on, and whose committed write takes that row out of
     today's walk, advances the walk exactly once to the next remaining review item:
     Ctrl+Enter close, Ctrl+Alt+F, Alt+N, the Ctrl+Shift+Enter link, a resolving Task
     Card commit, and Ctrl+Shift+M. Alt+F is the one stay answer; Ctrl+Enter never
     crosses the PRE/POST boundary; a short gesture lock swallows double presses; every
     walk landing records a <C-o> jump.
7. **Bob Cli Releases Ride A Daily Train** (`bob-cli-releases-ride-a-daily-train`)
   - bob-cli ships only through the scheduled release train, at most one version per New
     York day after full CI on the exact SHA, with no release PR; agents never edit the
     Cargo.toml version or CHANGELOG.md and mark CLI or JSON contract breaks with !.
8. **Bob Gcal Is Read Only And Proposes Once**
   (`bob-gcal-is-read-only-and-proposes-once`) - Google Calendar stays read-only; Bob
   writes each row and link once, updates only untouched Google spans, tombstones
   deletions, and cancels only untouched tasks.
9. **Bob Mac Capture Is A Thin Client Of bob** (`mac-capture-is-a-thin-client`)
   - Bob Mac Capture never parses capture grammar, computes previews, or writes the
     vault; it runs bob, renders the spans, candidates, and previews bob returns, and
     submits each draft as one bob capture call.
10. **Calendar Events Are Daily Note Event Tasks**
    (`calendar-events-are-daily-note-event-tasks`) - _[partly superseded by
    `decisions:synced-meetings-link-by-phase`]_ Synced calendar events live in their
    daily note's Events section with root attribution and one appended MEET.
11. **Capture Links A Dependency-Blocked Task By Dropping Its Prerequisites**
    (`capture-link-drops-blocking-deps`) - Typing an existing-task capture link on a
    task Blocked only by its own open prerequisites drops those links and then performs
    the gesture; plugins keep today's consumption.
12. **Capture Star Blocks Existing Tasks** (`capture-star-blocks-existing-tasks`)
    - Capture `*` makes the new task a prerequisite of an existing task.
13. **Code Copy Reads bob's Render-Time Catalog**
    (`code-copy-reads-render-time-catalog`) - bob records code blocks and page anchors
    during PDF rendering, and serves the catalog through bob ref code; clients present
    those bytes and never extract code from the PDF text layer or re-parse Markdown at
    read time.
14. **Decay Decisions Are Available Immediately**
    (`decay-decisions-are-available-immediately`) - Gesture-triggered approved-decay
    cards are available as soon as compatible plugins are installed; there is no
    calendar gate, replacement date, or counting-only period.
15. **Event Task Links Close By The Clock** (`event-task-links-close-by-the-clock`)
    - Timed closes classify same-day Event Task links by meeting and session clocks;
      explicit picks win, except not-started events stay out of wildcard completion and
      parking.
16. **Hiding A Theme Removes It From Recent Only** (`theme-hide-is-recent-only`)
    - Hiding a theme removes it from Recent suggestion rows only; today, Stats, colors,
      and typing the name stay unchanged, and using it never unhides it.
17. **Next And Pending Are Sticky Lanes; Only Blocked Is Derived**
    (`task-lanes-are-sticky`) - _[partly superseded by `projects-block-from-prj-line`]_
    Linking raises Ready to Next and an =x close sets In Progress (PENDING); no unlink,
    hooks run, or capture drop lowers them; only Alt+N release returns a task to Ready;
    Blocked stays derived.
18. **Note Ready Cap Counts The Lane** (`note-ready-cap-counts-the-lane`)
    - The per-note soft cap (plan.max_ready_per_note, default 5; ready_cap: N|off)
      counts each area/project note's whole Ready lane by residence, whatever its
      freshness.
19. **Open PDF Writes Are Coordinated** (`open-pdf-writes-are-coordinated`)
    - bob writes Highlights PDFs only through bounded .forMerging file coordination
      around the whole per-PDF write, holding when uncertain or while you read; never UI
      automation, never .forReplacing.
20. **Projects Block Their Tasks From The ^prj Line** (`projects-block-from-prj-line`)
    - _[partly superseded by `decisions:subprojects-inherit-project-blocking`,
      `decisions:capture-link-drops-blocking-deps`]_ A project's prerequisites and date
      live on its ^prj task; while that line is Blocked, every open resident is Blocked
      and tagged #prj_is_blocked, and Blocked tasks keep no Task Links under today's
      open Pomodoros.
21. **READY Is Freshness-Gated With NEW and ROTTEN Review** (`ready-is-freshness-gated`)
    - _[partly superseded by `note-ready-cap-counts-the-lane`, `review-walk-is-tiered`,
      `rotten-keeps-use-priority-decay`, `unblocked-tasks-resurface-as-ticklers`]_ READY
      is the freshness-gated confirmed/exempt backlog (visible TODO pool minus NEW and
      ROTTEN buckets) with TODAY → NEW → PENDING → NEXT → READY sections and
      NEW/PENDING/NEXT/READY/BLOCKED/ROTTEN/TODAY chips; review clears NEW then ROTTEN;
      no tags, fields, or status changes store review state.
22. **Recurring Occurrences Walk When Due** (`recurring-occurrences-walk-when-due`)
    - The ]s walk is PRE → NEW → PROJECTS → PENDING → NEXT → RECURRING → TICKLER →
      ROTTEN → POST; due recurring occurrences walk in RECURRING (never stamped); schema
      13 retired REFERENCES so refs use ordinary groups; a ^prj walks in PROJECTS only
      when it lacks #hide.
23. **Ref Rename Keeps Identity** (`ref-rename-keeps-identity`)
    - _[superseded by `decisions:reference-renames-keep-identity`]_ Ref renames change
      bundle paths while preserving reference and task identity.
24. **Reference Ids Follow File Stems** (`reference-ids-follow-file-stems`)
    - A reference id follows its note stem, with a numbered suffix only to avoid
      duplicate ids across ref/.
25. **Reference Renames Keep Identity** (`reference-renames-keep-identity`)
    - _[partly superseded by `decisions:reference-ids-follow-file-stems`]_ Renaming
      eligible references moves their bundle paths and inbound links while preserving
      reference and reading-task identity.
26. **Reference Sources Are Preserved Write-Once**
    (`reference-sources-are-preserved-write-once`) - bob preserves render-input Markdown
    and its derived code catalog in a hidden, content-addressed .bob store; source bytes
    are write-once and reference notes carry a note-local source_markdown pointer.
27. **References Are Ingested At Birth** (`references-are-ingested-at-birth`)
    - The creating host files each reference under the maintenance and scan locks,
      queues locally when unsafe, and leaves annotation sync to the Mac.
28. **Relation Card Tab Accepts Ctrl-D Hides**
    (`relation-card-tab-accepts-ctrl-d-hides`) - Ctrl+N/P navigate the relation card
    without touching the query, Tab accepts or completes the top match, selection is by
    key, and Ctrl+D hides what Return would insert.
29. **Relation Label Hide Is Suggestions-Only**
    (`relation-label-hide-is-suggestions-only`) - Hiding a relation label removes it
    from suggestions only; LINKS text, mirrors, chips, counts, and typed use stay
    unchanged, and using it never unhides it.
30. **Relation Label Prefs Live In A Per-Host Journal**
    (`relation-label-prefs-live-in-per-host-journal`) - Relation label hide, unhide, and
    rename events live in vault-synced .bob/relations/<host>.jsonl; fold is
    last-writer-wins and bob is the only writer.
31. **Relation Label Rename Rewrites History**
    (`relation-label-rename-rewrites-history`) - bob relations rename rewrites matching
    label spans everywhere, including done/ and the previous daily note, in one
    previewed transaction and one revertable commit.
32. **Review Walk Is Tiered With Daily Lane Review** (`review-walk-is-tiered`)
    - _[partly superseded by `recurring-occurrences-walk-when-due`]_ The ]s walk visits
      one shared queue in explicit tiers PRE → NEW → PROJECTS → PENDING → NEXT → TICKLER
      → REFERENCES → ROTTEN → POST; #gtd #pre/#post checklist rows resolve only by
      completion; Pending and Next come due daily under pending_interval / next_interval
      (default 1, false walks that lane off); tiers never feed buckets or chips; upkeep
      outside the lanes counts the budget; stamps stay and the seed never re-runs.
33. **Rotten Keeps Are Logged** (`rotten-keeps-are-logged`)
    - Every counted keep — Alt+F/Ctrl+Alt+F on an exact due Ready ROTTEN or scheduled
      TICKLER row, or the decay card's Keep — asks an optional reason and writes one
      dated 🥀 ROTTEN LOG entry with the stamp; the entry count is display-only history,
      never a field or a decay input.
34. **Rotten Keeps Decay Through The Priority Ladder**
    (`rotten-keeps-use-priority-decay`) - _[partly superseded by
    `decay-decisions-are-available-immediately`, `unblocked-tasks-resurface-as-ticklers`
    , `rotten-keeps-are-logged`]_ Repeated due-Ready keeps earn an explicit approved
    decision that enters the existing priority ladder; nothing decays silently and
    freshness itself still never changes priority or schedule.
35. **Similar Tasks Are A Settled Heads-Up** (`similar-tasks-are-a-settled-heads-up`)
    - Capture shows a "Similar tasks" band only after typing settles and only when a
      match clears bob's calibrated bars; every row action is an explicit draft edit and
      Return never changes.
36. **Subprojects Inherit Project Blocking** (`subprojects-inherit-project-blocking`)
    - _[partly superseded by `decisions:unblocked-tasks-are-offered`]_ A blocked project
      passes its dependency or schedule reason through its project descendants, with
      reconcile as the authority and the ^prj tag as the proxy for other engines.
37. **Synced Meetings Are Linked By Phase At Birth** (`synced-meetings-link-by-phase`)
    - A new Event Task is filed under a past, running, or planned MEET at birth: closed
      clusters for ended meetings, one open MEET for current and future.
38. **Task Dependencies Are Links On One Depends-On Line**
    (`task-deps-are-depends-on-links`) - _[partly superseded by
    `decisions:capture-link-drops-blocking-deps`]_ A task's prerequisites live as plain
    task dependency links on one managed Depends-On first-child line; that line is the
    source of truth and the [dependsOn::] / [id::] fields are derived from it.
39. **Theme Hue Is Config Then Pin Then Exempt Then Unconfigured Then Area Then Hash**
    (`theme-hue-is-config-pin-exempt-area-hash`) - A theme's hue is configured themes
    color, then journal pin, then plan.exempt mute, then the unconfigured mute, then
    top-level area stem, then name hash; clients render the hue Bob reports.
40. **Theme Hue Is Pin Then Exempt Then Area Then Hash**
    (`theme-hue-is-pin-exempt-area-hash`) - _[superseded by
    `decisions:theme-hue-is-config-pin-exempt-area-hash`]_ A theme's hue is journal pin,
    then plan.exempt mute, then top-level area stem, then name hash; clients render the
    hue Bob reports.
41. **Theme Preferences Live In A Per-Host Journal**
    (`theme-prefs-live-in-per-host-journal`) - Hide, unhide, color, and rename events
    live in vault-synced `.bob/themes/<host>.jsonl`; fold is last-writer-wins and bob is
    the only writer.
42. **Today Is Read From The Ledger, Never Written To Tasks**
    (`today-is-read-from-the-ledger`) - _[partly superseded by
    `ready-is-freshness-gated`]_ Today is the open tasks with a dedicated Task Link
    under today's open Pomodoros, computed at read time by bob plan and
    bob-ledger-tools; never a tag, task-line field, or file-path filter; #now is
    retired.
43. **Unblocked Tasks Are Offered, Never Auto-Linked** (`unblocked-tasks-are-offered`)
    - Every would-be successor link from a managed Done gesture is offered for explicit
      selection; recovery remains automatic.
44. **Unblocked Tasks Resurface As Ticklers** (`unblocked-tasks-resurface-as-ticklers`)
    - _[partly superseded by `decisions:unblocked-tasks-are-offered`]_ Every automatic
      Blocked-to-Ready recovery records [unblocked:: today] unless a WRITE skip applies,
      so the task is TICKLER in the next morning review; a keep on that row leaves the
      streak unchanged.

<!-- /sase:strands -->
