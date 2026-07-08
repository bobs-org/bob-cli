---
create_time: 2026-07-08 12:48:27
status: done
prompt: sdd/prompts/202607/replace_blocked_with_next_task_status.md
---
# Plan: Replace the `[B]` "Blocked" task status with a `[*]` "Next" status

## Summary

Retire the Obsidian **Blocked** task status (checkbox symbol `[B]`) and replace it with a new **Next** status (checkbox
symbol `[*]`). Add a new **NEXT Tasks** dashboard section to `~/bob/dash.md` (placed immediately after **WIP Tasks**)
that reuses the exact query body of the existing sections but matches only tasks that carry the new "next" status.

The Blocked status is not defined in one place — it is wired across five surfaces. The Rust CLI has **no** first-class
Blocked logic (any non-terminal marker already collapses to "open"), so `[*]` will behave exactly as `[B]` did there
without a logic change; the CLI work is limited to documentation and test fixtures.

## Background — where the Blocked `[B]` status lives today

1. **Obsidian Tasks plugin config** — `~/bob/.obsidian/plugins/obsidian-tasks-plugin/data.json` defines the status under
   `statusSettings.customStatuses`:
   `{ "symbol": "B", "name": "Blocked", "nextStatusSymbol": " ", "availableAsCommand": true, "type": "ON_HOLD" }`. This
   is the canonical definition that makes `[B]` a recognized status and drives the `status.*` filters used in `dash.md`.
2. **CSS snippet** — `~/bob/.obsidian/snippets/task-statuses.css` styles the status via both symbol selectors
   (`[data-task="B"]`) and type selectors (`[data-task-status-type="ON_HOLD"]`), plus `--task-status-blocked*` color
   variables.
3. **Custom plugins** (source of truth in the `bob-plugins` linked repo; deployed to the vault via `bob plugins sync`) —
   four plugins hardcode `"B"` in their "open task status" sets / status cycle, and two render a `B → "blocked"` status
   pill.
4. **bob-cli docs** — `docs/projects.md` documents `[B]  open` in the task-status convention table.
5. **bob-cli tests/fixtures** — a few Rust tests use `- [B]` as a representative "open" marker.

### Facts established during investigation (de-risking)

- **No existing `[B]` task lines** exist anywhere in the vault (outside `_templates`). Removing the status orphans
  nothing.
- **`[*]` is already used informally** as a checkbox marker in ~29 pomodoro/daily notes, but **none of those lines carry
  the `#task` global filter**, so formalizing `*` as a status will not unexpectedly populate the new dashboard section.
  Those existing `[*]` lines will simply pick up the new "Next" styling in-note (desired).
- The Rust CLI classifies checkbox markers with a `_ => Open` fallthrough (only `x`/`X` = done and `-` = canceled are
  special). `[B]` is already treated as an open task; `[*]` will be too. No Rust logic change is required.
- The `is not blocked` line in the `dash.md` queries is the Tasks plugin's **dependency-based** filter (tasks with unmet
  `dependsOn` links). It is unrelated to the `[B]` status and must be **retained** — removing the Blocked status does
  not touch it.

## Goal

- The `[B]` Blocked status no longer exists as a defined/recognized status anywhere.
- A `[*]` Next status exists and is styled/handled everywhere `[B]` was.
- `~/bob/dash.md` gains a **NEXT Tasks** section after **WIP Tasks** that lists only next-status tasks, using the same
  query body as the sibling sections.

## Design decisions (recommended defaults — please confirm during review)

1. **Next status type = `ON_HOLD` (reuse Blocked's slot).** The Tasks plugin has no "NEXT" type; the available types are
   TODO, IN*PROGRESS, DONE, CANCELLED, NON_TASK, ON_HOLD. Keeping `ON_HOLD` means "Next" inherits every place Blocked
   was wired by _type* (the `data.json` entry and the `[data-task-status-type="ON_HOLD"]` CSS selectors keep working
   unchanged) and, critically, keeps next tasks **out of** the existing WIP (`status.type is IN_PROGRESS`) and READY
   (`status.type is TODO`) sections — so the new NEXT section is the single home for next tasks, exactly mirroring how
   Blocked tasks appeared in no auto section. The user-facing name is "Next"; the internal type label is not shown in
   the dashboard. _Alternative:_ set type `TODO` if next tasks should also appear under READY (as a highlighted subset).
   This is not recommended — it duplicates next tasks across two sections and forces the type-based CSS selectors to be
   reworked.

2. **NEXT dashboard query matches by status name: `status.name is Next`.** This selects exactly the next status (only
   `*` has name "Next") and reads naturally alongside the sibling `status.type is …` lines. Everything else in the query
   body is copied verbatim from the WIP/READY queries, including `is not blocked`.

3. **`nextStatusSymbol` for Next = `x` (Done).** Toggling/completing a next task via the Tasks plugin advances it to
   Done, matching the core convention (Todo→Done, In Progress→Done). _Alternative:_ `/` (In Progress) if a next action
   should first move to in-progress. Note Bryan's `task-status-cycler` plugin has its own cycle and is the more likely
   day-to-day driver; this field only affects the Tasks-plugin-native toggle.

4. **Next styling = a distinct accent color + `*` glyph.** Blocked rendered orange with a `!` glyph. Next should be
   visually distinct; recommend repurposing the CSS variables to a fresh accent (e.g. a purple/green) and a `*` glyph.
   Exact color is cosmetic and tunable by Bryan.

## Changes by surface

### A. Obsidian Tasks plugin config — `~/bob/.obsidian/plugins/obsidian-tasks-plugin/data.json`

Replace the Blocked custom status entry with a Next entry:

- from: `{ "symbol": "B", "name": "Blocked", "nextStatusSymbol": " ", "availableAsCommand": true, "type": "ON_HOLD" }`
- to: `{ "symbol": "*", "name": "Next",    "nextStatusSymbol": "x", "availableAsCommand": true, "type": "ON_HOLD" }`

This is the definitional change that removes Blocked and introduces Next.

### B. Dashboard — `~/bob/dash.md`

Insert a new section **between** the existing `### WIP Tasks` and `### READY Tasks` sections. Its ```tasks block is
identical to the sibling blocks except the status line:

```tasks
folder does not include _templates
status.name is Next
is not blocked
filter by function task.file.path !== query.file.path
filter by function !task.scheduled.moment || task.scheduled.moment.isSameOrBefore(moment(), "day")
filter by function !task.tags.includes("#hide")
group by path
sort by function task.file.path
sort by function task.lineNumber
short mode
hide toolbar
```

Heading: `### NEXT Tasks`. Order after the change: WIP Tasks → NEXT Tasks → READY Tasks. The WIP and READY queries are
left unchanged.

### C. CSS snippet — `~/bob/.obsidian/snippets/task-statuses.css`

- Rename the color variables `--task-status-blocked` / `--task-status-blocked-bg` to `--task-status-next` /
  `--task-status-next-bg`, and give Next a distinct accent color (currently orange; pick a fresh accent per decision
  #4).
- Replace every symbol selector `[data-task="B"]` with `[data-task="*"]` (all occurrences).
- Leave the `[data-task-status-type="ON_HOLD"]` selectors as-is — they now style Next because Next keeps type `ON_HOLD`.
- Update the checked-checkbox glyph `--task-status-symbol: "!"` (in the Blocked selector block) to `"*"` (cosmetic).

### D. Custom plugins — `bob-plugins` linked repo

Edit the **source** in the `bob-plugins` repo (do not edit the deployed copies under `~/bob/.obsidian/plugins/`; they
are overwritten on sync). Open the linked repo with `sase workspace open -p bob-plugins -r "<reason>" <workspace_num>`
and use the printed path.

Symbol-set / cycle swaps (`"B"` → `"*"`):

- `plugins/bob-project-tasks/main.js` — `OPEN_TASK_STATUSES` set: `[" ", "/", "B"]` → `[" ", "/", "*"]`.
- `plugins/block-id-prompt/main.js` — `OPEN_OBSIDIAN_TASK_STATUSES` set: `[" ", "/", "B"]` → `[" ", "/", "*"]`.
- `plugins/bob-navigation-hotkeys/main.js` — `OPEN_OBSIDIAN_TASK_STATUSES` set and `PROJECT_OPEN_TASK_STATUSES` set:
  both `[" ", "/", "B"]` → `[" ", "/", "*"]`.
- `plugins/task-status-cycler/main.js` — `FIXED_SYMBOLS` cycle: `[" ", "/", "B", "x", "-"]` →
  `[" ", "/", "*", "x", "-"]` (Next takes Blocked's position in the cycle).

Status-pill label / CSS swaps (`B → "blocked"` becomes `* → "next"`):

- `plugins/block-id-prompt/main.js` — the `taskStatusClass` helper: `if (status === "B") return "blocked";` →
  `if (status === "*") return "next";`. Verify the adjacent `taskStatusLabel` renders the symbol generically (so `[*]`
  displays); update if it hardcodes `B`.
- `plugins/block-id-prompt/styles.css` — rename `.bid-tlp-status-pill.is-blocked` → `.bid-tlp-status-pill.is-next` and
  pick a next-appropriate color (currently error red).
- `plugins/bob-navigation-hotkeys/main.js` — the equivalent `taskStatusClass` helper: same `B → "blocked"` →
  `* → "next"` swap.
- `plugins/bob-navigation-hotkeys/styles.css` — rename `.bob-cnp-status-pill.is-blocked` →
  `.bob-cnp-status-pill.is-next` and recolor.

After editing, deploy with `bob plugins sync` so the vault picks up the new plugin builds.

### E. bob-cli — docs and test fixtures (repo-relative paths)

Required (user-facing convention doc):

- `docs/projects.md` — in the task-status table, change `[B]  open` to `[*]  open`.

Recommended cleanup (purge `[B]` from fixtures; behavior is unchanged because `*` still classifies as Open):

- `src/native/projects.rs` — the parser test fixture line `- [B] #task shown blocked` → `- [*] #task shown next`. The
  `open_task_count == 6` assertion still holds.
- `tests/cli.rs` — the fixture line `- [B] #task shown blocked` → `- [*] #task shown next`.
- `src/native/capture.rs` — the two test fixtures `"- [B] #task old"` → `"- [*] #task old"` (marker is incidental to
  what these tests assert).
- Run `cargo test` to confirm the suite stays green.

## Out of scope (deliberately not changed)

- **Rust status-classification logic.** `[*]` already maps to Open via the existing `_ => Open` fallthrough; no new
  branch is added.
- **Expanding `*` into pomodoro / transcluded start-toggle / ledger flows.** Those code paths (`task-status-cycler`
  start-toggle predicates that branch on ` `,`/`,`x`; the pomodoro navigation set `[" ","/","x","X"]`; the ledger regex
  `[ /xX-]`) never handled `B`, so a like-for-like swap keeps `*` out of them too. Wiring "next" into those flows would
  be a separate enhancement.
- **The dependency-based blocking feature** (`dependsOn` / `isBlocked` / `is not blocked`). It is orthogonal to the
  checkbox status and is retained everywhere, including in the new NEXT query.
- **Historical `sdd/` design docs** that mention the old `B`=Blocked convention — left as point-in-time records.
- **Other embedded task queries** (e.g. daily notes using `status.type is TODO/IN_PROGRESS`). Only `dash.md` gains a
  NEXT section per the request; next tasks (type `ON_HOLD`) will not appear in those existing `status.type` queries,
  consistent with the dashboard.

## Verification

1. **bob-cli:** `cargo test` passes; `docs/projects.md` shows `[*]` and no longer `[B]`.
2. **bob-plugins:** `node scripts/validate-manifests.mjs` (or the repo's check) passes; grep the plugin sources to
   confirm no `"B"` remains in the status sets/cycle and no `is-blocked` / `"blocked"` label remains. Run
   `bob plugins sync`.
3. **Vault/Obsidian:** after reloading Obsidian (or reopening with the changes applied), confirm:
   - The Tasks plugin settings list a **Next** status (`*`) and no **Blocked** status.
   - `dash.md` renders a **NEXT Tasks** section between WIP and READY.
   - A scratch `- [*] #task try next thing` line (with a scheduled date of today or none, no `#hide`) appears under NEXT
     Tasks and not under WIP/READY.
   - `[*]` tasks render with the new Next styling (accent + glyph).

## Rollout / operational notes

- `~/bob` is a git repo (`bobs-org/bob`); `data.json`, the CSS snippet, and `dash.md` are all tracked there. The Tasks
  plugin `data.json` and CSS snippets are read by a **running** Obsidian instance — apply these edits with Obsidian
  closed, or reload Obsidian afterward, so the running app does not overwrite `data.json` or serve a stale snippet.
- Suggested order: (1) edit `bob-plugins` source + `bob plugins sync`; (2) edit the vault files (`data.json`,
  `task-statuses.css`, `dash.md`); (3) edit bob-cli docs/tests and run `cargo test`; (4) reload Obsidian and verify.
- Committing the `~/bob`, `bob-plugins`, and bob-cli changes is left to the normal per-repo commit flow and is not
  performed as part of implementation unless requested.
