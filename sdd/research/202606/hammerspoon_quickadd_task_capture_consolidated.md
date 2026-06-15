---
create_time: 2026-06-15
status: research
topic: Whether to migrate Bryan's Hammerspoon task-capture hotkey to Obsidian QuickAdd
---
# Research: Hammerspoon Task Capture and QuickAdd

## Question

The chezmoi-managed Hammerspoon config binds a global hotkey that captures new
tasks directly into the Bob Obsidian vault. Should that workflow migrate to the
Obsidian QuickAdd plugin? If so, what benefits justify the change, and what
should the target design be?

## Executive Answer

Do not do a pure migration from Hammerspoon to QuickAdd. QuickAdd is an
Obsidian plugin, so it does not replace the OS-global hotkey layer. A
QuickAdd-based workflow still needs Hammerspoon, Raycast, Keyboard Maestro, a
global-hotkey plugin, or some other external trigger, and it depends on an
Obsidian plugin runtime with the Bob vault open and QuickAdd enabled.

QuickAdd is still useful if the goal is Obsidian-native capture: command
palette reuse, mobile/Shortcuts capture, richer prompts, Tasks/Dataview/
metadata integration, and writes through Obsidian's vault API. The right
QuickAdd shape is a hybrid: keep Hammerspoon as the global trigger and either
pass the captured text to `obsidian://quickadd` or let QuickAdd prompt inside
Obsidian.

If the real goal is maintainability of the current desktop workflow, the better
default is not QuickAdd. Move the parsing/formatting/insertion algorithm into a
tested `bob capture` subcommand and reduce Hammerspoon to "prompt, shell out,
notify". That preserves the current best property, capture while GUI Obsidian
is closed, and keeps the logic in this Rust repo. It does not give the same
Obsidian API-safe write semantics as QuickAdd, but it is a cleaner fit for the
current macOS-only capture flow.

## Verification Performed

- Read the two completed research-agent transcripts:
  `~/.sase/chats/202606/bob_cli-ace_run-260615_154347.md` and
  `~/.sase/chats/202606/bob_cli-ace_run-260615_154350.md`.
- Read the two agent-created drafts:
  `sdd/research/202606/quickadd_hammerspoon_task_capture_migration.md` and
  `sdd/research/202606/hammerspoon_to_quickadd_task_capture.md`.
- Read audited Obsidian memory with `sase memory read long/obsidian.md`.
- Verified the relevant Hammerspoon implementation in
  `/home/bryan/.local/share/chezmoi/home/dot_hammerspoon/init.lua`.
- Verified local Bob vault plugin state under `/home/bryan/bob/.obsidian`.
- Checked the current QuickAdd and Obsidian docs and QuickAdd releases on
  2026-06-15.

Important conflicts resolved:

- One draft implied Advanced URI would likely be required. For this workflow,
  QuickAdd's own `obsidian://quickadd` URI is the relevant path; Advanced URI is
  not required unless a different URI strategy is chosen.
- One draft overstated `bob capture` as a fix for external-write races. A CLI
  helper would centralize and test the write logic, but it still writes outside
  the GUI Obsidian app unless it delegates to an Obsidian API/CLI runtime.
- Both drafts correctly rejected a pure QuickAdd Capture choice for the current
  routed insertion behavior.

## Current Local Workflow

The source of truth is the chezmoi-managed Hammerspoon file:

`/home/bryan/.local/share/chezmoi/home/dot_hammerspoon/init.lua`

Verified behavior:

- `Cmd+Ctrl+Shift+I` opens a Hammerspoon `hs.webview` prompt titled
  "Capture Task".
- The prompt normalizes input to one line.
- A task is formatted as `- [ ] #task <text> [created::YYYY-MM-DD]`.
- `@route task` and `task @route` both route to `~/bob/<route>.md`, with the
  route lower-cased.
- Unrouted tasks append to `~/bob/mac_inbox.md`.
- Routed tasks are inserted after the last top-level open `#task` line in the
  target file, after that task's indented continuation block.
- Hammerspoon writes Markdown directly with Lua file I/O and restores the
  previously focused application when the capture prompt closes.

That direct-file design is the reason capture works when GUI Obsidian is not
running. It is also the reason Obsidian is not aware of the write at the moment
it happens, which can matter if the target file is open with unsaved editor
state or if sync/cache timing is unlucky.

## Current Obsidian and QuickAdd State

Audited memory says `~/bob` is the active Obsidian vault, and this machine uses
`obsidian-headless` through the `ob` command for sync-oriented workflows rather
than requiring a full GUI Obsidian session.

Local vault evidence:

- QuickAdd is installed and enabled in `/home/bryan/bob`.
- Installed QuickAdd version is `2.12.3`.
- QuickAdd's `data.json` currently has `"choices": []`, so there is no existing
  Bob capture choice to reuse.
- Enabled plugins include `quickadd`, `dataview`, `obsidian-tasks-plugin`,
  `templater-obsidian`, `metadata-menu`, and several Bob-specific plugins.
- `~/.config/obsidian/obsidian.json` only listed the GUI vault
  `/home/bryan/var/obsidian/vaults/greatday`, not `/home/bryan/bob`. This does
  not prove Bob cannot be opened in GUI Obsidian, but it is a gate for any URI
  design that uses `vault=...`.
- The local `obsidian` CLI currently reports: "The CLI is unable to find
  Obsidian. Please make sure Obsidian is running and try again."
- The `ob` command is an obsidian-headless client for sync/publish commands; it
  is not a QuickAdd command runner.

## QuickAdd Capabilities That Matter

QuickAdd's choice types are Template, Capture, Macro, and Multi. The docs frame
Capture choices as the right choice for appending text to a journal, log, task
list, or existing file, while Macro choices are the right layer when scripting
or multiple steps are required.

Useful QuickAdd capabilities:

- Capture choices can target vault-relative files and folders, create missing
  files, format captured text as a task, and use a capture format with
  `{{DATE}}`, `{{VALUE}}`, named `{{VALUE:name}}`, and other format tokens.
- Capture placement supports top/bottom positions plus "Insert after" and
  "Insert before" a matching line, with options such as end-of-section handling,
  subsection handling, blank-line handling, and creating the line if missing.
- `obsidian://quickadd?choice=<choice>&value-name=<encoded>` can launch a
  choice externally. URI parameters must be encoded.
- URI-provided variables must be named. A bare `{{VALUE}}` cannot be filled by
  the URI and will prompt inside Obsidian.
- QuickAdd 2.12.0 added Obsidian CLI handlers on supported Obsidian versions:
  `quickadd`, `quickadd:run`, `quickadd:list`, and `quickadd:check`. The docs
  say this requires Obsidian `1.12.2+` and QuickAdd enabled in the target vault.
- The latest QuickAdd release observed on 2026-06-15 is `2.13.1` from
  2026-06-12. QuickAdd `2.13.0+` requires Obsidian `1.13.0+`; older Obsidian
  installs remain on `2.12.3`.

The current Bob capture algorithm needs more than a pure Capture choice. The
hard part is not formatting `- [ ] #task ...`; it is parsing route tokens from
free text and finding the insertion point after the last open task block. That
requires either a QuickAdd Macro/User Script or a changed note structure with
stable section anchors.

## Fit Against Current Requirements

| Current behavior | Pure QuickAdd Capture | QuickAdd Macro/User Script | `bob capture` helper |
| --- | --- | --- | --- |
| OS-global hotkey | Needs another trigger | Needs another trigger | Keeps Hammerspoon trigger |
| Capture when GUI Obsidian is closed | No | No | Yes |
| Native lightweight prompt | Only if Hammerspoon stays | Yes, if Hammerspoon keeps prompt | Yes |
| Current task format | Yes | Yes | Yes |
| Default append to `mac_inbox.md` | Yes | Yes | Yes |
| `@route task` and `task @route` | Awkward | Yes | Yes |
| Insert after last open task block | Not cleanly | Yes, custom script | Yes, tested code |
| Obsidian Vault API write | Yes | Yes | No, unless delegated |
| Mobile/Shortcuts reuse | Yes | Yes, if script is mobile-safe | Not directly |
| Configuration as code | Weak unless generated | Medium with script file | Strong in repo |

## Benefits of a QuickAdd Hybrid

1. Obsidian-native writes

   A QuickAdd script can use Obsidian's vault APIs instead of raw filesystem
   writes. That keeps Obsidian's metadata/cache layer closer to the write and
   reduces the class of problems caused by editing vault files from outside the
   app.

2. Reusable capture surface

   The same choice can be launched from Obsidian's command palette, Obsidian
   hotkeys, `obsidian://quickadd`, the Obsidian CLI when available, and mobile
   automation. The current Hammerspoon path is macOS desktop only.

3. Better future prompts

   QuickAdd already has a model for variables, dates, option lists, one-page
   inputs, and macros. It is a better fit than Hammerspoon for later fields such
   as due date, priority, context, project, or status.

4. Plugin ecosystem integration

   Because the workflow runs inside Obsidian, it can later lean on Dataview,
   Tasks, Metadata Menu, Templater, or Bob-specific Obsidian plugins without
   reimplementing all context discovery in Lua.

5. Existing dependency

   QuickAdd is already installed and enabled in the Bob vault. The migration
   would configure an existing plugin, not introduce a brand-new plugin to the
   vault.

## Costs and Risks of a QuickAdd Hybrid

1. Runtime dependency regression

   The current hotkey can write to `~/bob` even when GUI Obsidian is closed.
   QuickAdd requires an Obsidian plugin runtime with the Bob vault open and
   QuickAdd loaded. Cold-launching Obsidian from a hotkey is slower and more
   failure-prone than the current direct write.

2. Focus and latency

   URI launch generally brings Obsidian forward or at least routes through it.
   Hammerspoon can try to restore the previous app after submitting, but the
   current "never leave the active app" feel will be harder to preserve.

3. Bob vault registration

   The GUI Obsidian registry observed locally only listed `greatday`. A
   QuickAdd URI must address the Bob vault by a registered vault name or ID, or
   it risks running in the wrong vault or failing.

4. Custom logic moves rather than disappears

   Pure Capture handles inbox append and task formatting well, but it does not
   naturally preserve the current route parsing and "after last open task block"
   insertion. That code must move into a QuickAdd script or into a shared helper.

5. Sync timing

   QuickAdd's URI docs warn that file-based sync can create duplicate/stale file
   behavior if Obsidian has not opened and synced the vault first. This matters
   most for creating missing route files, but it is still a reason to keep a
   conservative trial period.

6. Mobile script portability

   A QuickAdd script that uses only Obsidian vault APIs is more portable. A
   script that shells out to `bob capture` may work on desktop but will not be a
   clean mobile solution.

## Architecture Options

### Option A: Keep Hammerspoon Direct Writes

Best when the priority is lowest-latency global capture that works whether or
not GUI Obsidian is running. It is operationally simple and already matches the
desired task placement, but parsing and Markdown mutation remain in Lua.

### Option B: Pure QuickAdd Capture Choice

Not recommended. It can append to `mac_inbox.md` and format task lines, but it
does not preserve the routed insertion rule without changing the note structure
to use stable anchors or accepting bottom-of-file insertion.

### Option C: Hammerspoon Trigger to QuickAdd Macro/User Script

Best if the goal is Obsidian-native or cross-device capture. Hammerspoon remains
the macOS global trigger; QuickAdd owns the capture behavior.

Two launch variants are viable:

- Hammerspoon keeps the current prompt and calls:
  `obsidian://quickadd?vault=<bob-vault-id>&choice=Bob%3A%20Capture%20Task&value-task=<encoded>`
- Hammerspoon launches QuickAdd and lets QuickAdd prompt:
  `obsidian://quickadd?vault=<bob-vault-id>&choice=Bob%3A%20Capture%20Task`

The first variant preserves more of the current capture feel. The second
removes more Hammerspoon UI code but foregrounds Obsidian more visibly.

### Option D: Move Capture Logic Into `bob capture`

Best if the goal is maintainability without losing current desktop behavior.
Add a native `bob capture` command that owns normalization, route parsing, task
formatting, file creation, and routed insertion. Hammerspoon shells out to it,
similar to the existing `bob pomodoro` integration.

This keeps the algorithm in the repo where it can be unit-tested and code
reviewed. It also creates a future bridge: a desktop QuickAdd macro could call
the same command, or a separate QuickAdd script could mirror the algorithm if
mobile becomes important. The tradeoff is that `bob capture` remains an
external writer unless it later delegates to an Obsidian runtime.

## Implementation Notes for the QuickAdd Path

Create a QuickAdd Macro choice named `Bob: Capture Task`.

The macro script should:

1. Read `variables.task`; if absent, call `quickAddApi.inputPrompt(...)`.
2. Normalize whitespace to match the current Hammerspoon behavior.
3. Parse `@route task` and `task @route`.
4. Build `- [ ] #task <text> [created::YYYY-MM-DD]`.
5. Append unrouted tasks to `mac_inbox.md`.
6. For routed tasks, open or create `<route>.md`, find the last top-level open
   `#task` line, skip that task's indented continuation block, and insert after
   it.
7. Use Obsidian notices for success/failure.

Before relying on this path:

1. Register/open `/home/bryan/bob` in GUI Obsidian and copy the vault ID.
2. Verify `obsidian://quickadd?vault=<bob-vault-id>&choice=...` runs in Bob,
   not `greatday`.
3. Verify whether the local Obsidian CLI can run `quickadd:list` and
   `quickadd:check` when the Bob vault is open.
4. Test unrouted capture, prefix route, suffix route, missing route file, and
   routed insertion after a task with nested continuation lines.
5. Keep the current Hammerspoon direct writer as a fallback until the new path
   has been exercised against real daily use.

## Source Notes

- QuickAdd Getting Started:
  https://quickadd.obsidian.guide/docs/
- QuickAdd Capture choice:
  https://quickadd.obsidian.guide/docs/Choices/CaptureChoice/
- QuickAdd Obsidian URI:
  https://quickadd.obsidian.guide/docs/Advanced/ObsidianUri/
- QuickAdd CLI:
  https://quickadd.obsidian.guide/docs/Advanced/CLI/
- QuickAdd releases:
  https://github.com/chhoumann/quickadd/releases
- Obsidian URI:
  https://obsidian.md/help/uri
- Global Hotkeys plugin reference:
  https://github.com/mjessome/obsidian-global-hotkeys
- Local Hammerspoon source:
  `/home/bryan/.local/share/chezmoi/home/dot_hammerspoon/init.lua`
- Local QuickAdd config:
  `/home/bryan/bob/.obsidian/plugins/quickadd/manifest.json` and
  `/home/bryan/bob/.obsidian/plugins/quickadd/data.json`

## Recommended Solution

Do not replace the Hammerspoon hotkey with a pure QuickAdd Capture choice.

Use this decision rule:

- If the goal is better desktop maintainability, build `bob capture` first and
  make Hammerspoon a thin prompt/launcher. This is the recommended default for
  the current workflow because it preserves fast OS-global capture while GUI
  Obsidian is closed, keeps task placement exactly as-is, and moves the brittle
  markdown algorithm into tested Rust.
- If the goal is mobile/cross-device capture, richer prompts, or Obsidian-native
  API writes, use the QuickAdd hybrid. Keep Hammerspoon as the global trigger,
  call a `Bob: Capture Task` QuickAdd Macro/User Script through
  `obsidian://quickadd`, and preserve the current route and insertion semantics
  in that script. Accept the runtime cost: Bob must be registered/openable in
  GUI Obsidian, QuickAdd must be enabled, and capture will no longer be
  independent of Obsidian.

The practical sequence is: implement `bob capture` as the canonical capture
algorithm, thin Hammerspoon to call it, then add a QuickAdd wrapper only when
Obsidian-native or mobile capture becomes a real requirement.
