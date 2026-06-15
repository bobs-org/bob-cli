---
create_time: 2026-06-15
status: research
topic: Whether to migrate the global Hammerspoon "Capture Task" hotkey to the Obsidian QuickAdd plugin
---
# Research: Migrating Hammerspoon Task Capture to QuickAdd

## Question

The chezmoi-managed Hammerspoon config (`~/.hammerspoon/init.lua`) binds a global hotkey
(`Cmd+Shift+Ctrl+I`) that pops a native prompt and writes a new `#task` line straight into
the Bob vault. Should this be migrated to the Obsidian **QuickAdd** plugin? If so, what are
the benefits, and what is the recommended end state?

## Short Answer

A *pure* migration — replacing Hammerspoon with QuickAdd — is **not** a clear win, because
QuickAdd lives inside Obsidian and Obsidian has no OS-global hotkey of its own. You would
still need an OS-level trigger (Hammerspoon, Raycast, Keyboard Maestro, or the Global Hotkeys
plugin), and you would *lose* the current system's best property: it captures even when
Obsidian is closed.

The defensible move is a **hybrid**: keep a thin Hammerspoon (or Raycast) trigger as the OS
hotkey, but move the *write* into a QuickAdd **Capture choice** invoked through the
`obsidian://quickadd` URI. That buys API-safe writes, centralized format logic, and
cross-device (mobile) capture — at the cost of requiring Obsidian to be running and
reintroducing a small amount of custom code (a QuickAdd macro) to reproduce the `@route`
routing and "insert after the last open task" behavior.

For *this* codebase there is a third option that may beat both: move the capture logic into
the **`bob` CLI** (`bob capture …`) and have Hammerspoon shell out to it, exactly as it
already does for `bob pomodoro`. That centralizes the format/routing/insertion logic in
tested Rust instead of Lua, keeps Obsidian-closed capture working, and is reusable from any
trigger. Recommendation below picks between these based on intent.

## Current System (verified from source)

From `~/.local/share/chezmoi/home/dot_hammerspoon/init.lua` (deployed to
`~/.hammerspoon/init.lua`), the `showTaskCapturePrompt` flow does the following:

- **Global trigger.** `hs.hotkey.bind({"cmd","shift","ctrl"}, "i", …)` registers an OS-level
  hotkey that fires regardless of the frontmost app and **whether or not Obsidian is
  running**.
- **Native prompt.** An `hs.webview` HTML panel ("Capture Task") collects one line of text;
  Enter submits, Escape cancels. It also collapses multi-line pastes to a single line.
- **Routing syntax.** `parseCapturedTaskTarget` accepts `@route text` *or* `text @route` and
  routes to `~/bob/<route>.md` (lower-cased). With no route, it falls back to
  `~/bob/mac_inbox.md`.
- **Task format.** `capturedTaskLine` writes `- [ ] #task <text> [created::YYYY-MM-DD]`.
- **Placement logic.** For routed files, `insertTaskAfterLastOpenTask` inserts the new task
  **after the last open `#task` block**, with ~150 lines of care for indented continuation
  lines and interleaved blank lines. The inbox path is a plain append.
- **Niceties.** It shows an `hs.notify` confirmation and **restores the previously focused
  app** after capture, so the workflow never visibly leaves the app you were in.
- **Direct file I/O.** All writes are plain `io.open`/`write` to vault files; Obsidian is
  never in the loop.

The key architectural fact: **this writes Markdown to disk directly and never touches
Obsidian**, which is exactly why it works when Obsidian is closed — and also why it carries a
small external-write race risk if Obsidian happens to have the target file open with unsaved
edits.

## What QuickAdd Provides

QuickAdd's **Capture choice** is purpose-built for this kind of capture and is configurable
enough to reproduce most of the format/placement behavior:

- **Dynamic target file.** The "Capture To" path accepts format syntax, e.g.
  `bins/daily/{{DATE:gggg-MM-DD}}.md`, so a routed target like `{{VALUE:route}}.md` is
  expressible — *if* the route is supplied as a named variable.
- **Capture format.** Defaults to `{{VALUE}}`; customizable to
  `#task {{VALUE}} [created::{{DATE:YYYY-MM-DD}}]`, and a **Task** toggle prepends `- [ ] `.
- **Placement.** "Insert after", "Insert before", "Bottom of file", top-of-file/after-
  frontmatter, and at/around cursor. "Insert after" adds rich sub-options: *insert at end of
  section*, *consider subsections*, *blank-line handling*, and *create line if not found*.
- **Format variables.** `{{VALUE}}`, named `{{VALUE:name}}`, `{{DATE:fmt}}`, `{{VDATE}}`
  (prompted/NLDates date), `{{FIELD:…}}`, `{{NAME}}`, `{{SELECTED}}`, `{{LINKCURRENT}}`, etc.
- **External triggering.** `obsidian://quickadd?vault=Bob&choice=<Name>&value-<VAR>=<text>`
  runs a named choice and fills *named* variables. Bare/unnamed `{{VALUE}}` **cannot** be
  filled from the URI and will instead prompt inside Obsidian, so a headless capture must use
  a named variable (e.g. `value-task=…` → `{{VALUE:task}}`). Values must be percent-encoded.
- **Ecosystem reach.** Because it runs inside Obsidian, a capture can use Templater, NLDates,
  metadata-menu fields/enums, and QuickAdd "one-page inputs" for multi-field capture.

## The Core Constraint (why "pure QuickAdd" can't fully replace Hammerspoon)

Obsidian has **no native OS-global hotkey.** Every "capture from anywhere" path still needs
an external trigger:

- The **Global Hotkeys** community plugin uses Electron's `globalShortcut`, so it fires even
  when Obsidian is unfocused — **but only while Obsidian is running.** Close Obsidian and the
  shortcut is gone.
- Firing `obsidian://…` from Hammerspoon/Raycast/Keyboard Maestro **activates (foregrounds)
  Obsidian**, and if Obsidian is closed it cold-launches it. QuickAdd's own docs warn that
  capturing before a device's Obsidian has opened can cause sync races / duplicate files.

So in every QuickAdd-based design you keep an OS-level trigger and you depend on Obsidian
being up. You are not removing a moving part; you are adding QuickAdd (and likely Advanced
URI) on top of a still-required trigger.

## Feature-Parity Assessment

| Current behavior | Under QuickAdd | Notes |
|---|---|---|
| Global hotkey, Obsidian closed | ✗ regression | Needs Obsidian running; cold-launch is slow + sync-race-prone |
| Native lightweight prompt | △ | Keep Hammerspoon's prompt (hybrid) or use QuickAdd's in-Obsidian prompt |
| `#task … [created::DATE]` format | ✓ clean | Capture format + Task toggle + `{{DATE}}` |
| `@route` / `route @` routing | △ macro | Built-in path can use `{{VALUE:route}}`, but parsing a free-text `@route` prefix needs a JS macro |
| Insert after **last** open task block | △ macro | QuickAdd "Insert after" targets the **first** match; "Bottom of file" changes semantics; exact behavior needs a macro |
| Inbox append | ✓ clean | "Bottom of file" |
| Restore previous app | △ | Hybrid can re-activate prior app, but Obsidian flashes to front first |
| API-safe write (no external race) | ✓ gain | Goes through Vault API; cache/Dataview stay consistent |
| Mobile / other desktops | ✓ gain | Same choice runs via Shortcuts/URI on iOS/Android |

## Benefits of Migrating (hybrid)

1. **Single source of truth for format.** The task template lives in one QuickAdd choice
   instead of being hard-coded in Lua, so changing the `#task` shape, adding a `due::`, or
   adding fields is a settings edit, not a code change.
2. **API-safe writes.** Captures go through Obsidian's Vault API, eliminating the
   external-write race and keeping the metadata cache / Dataview index consistent.
3. **Cross-device capture.** The identical choice is reachable from Obsidian mobile via
   Shortcuts and from any desktop — Hammerspoon is macOS-desktop-only.
4. **Ecosystem integration.** NLDates for natural-language due dates, metadata-menu
   field/enum prompts, Templater, and multi-field one-page inputs become available "for
   free."
5. **Less bespoke Lua.** The ~150-line insertion routine can shrink toward a thin URI shim
   (though some logic re-appears as a QuickAdd macro — see costs).

## Costs and Risks

1. **Loses Obsidian-closed capture** — the single biggest regression. Today you can fire the
   hotkey on a fresh login before Obsidian is up; the hybrid cannot.
2. **Focus stealing / latency.** The URI foregrounds Obsidian and round-trips through it —
   slower and more failure-prone than a direct `io.write`, and the "never leave your current
   app" feel is degraded.
3. **Custom code doesn't disappear, it moves.** Reproducing `@route` parsing and
   "insert-after-last-open-task" requires a QuickAdd JS macro. You trade Lua you own for JS
   you own, plus a new QuickAdd + (likely) Advanced URI dependency.
4. **Net dependency increase.** You still need Hammerspoon/Raycast for the hotkey *and* now
   QuickAdd *and* probably Advanced URI.

## Architecture Options

- **A. Pure QuickAdd (Obsidian hotkey only).** Simplest config, but only captures while
  Obsidian is focused. Clear regression; rejected.
- **B1. Hybrid — Hammerspoon prompt → QuickAdd URI (recommended QuickAdd path).** Keep the
  fast native prompt; on submit, fire
  `obsidian://quickadd?vault=Bob&choice=Capture&value-task=<encoded>`. A QuickAdd Capture
  choice (plus a small macro for routing + last-task insertion) does the write. Best
  capture UX of the QuickAdd options; requires Obsidian running.
- **B2. Hybrid — Hammerspoon fires bare URI, QuickAdd prompts.** Less Lua, but forces the
  prompt into Obsidian and always foregrounds it. Worse UX than B1.
- **C. Status quo (keep Hammerspoon direct writes).** Simplest, fastest, works Obsidian-
  closed; costs are Lua-duplicated format logic, no mobile, and the external-write race.
- **D. Centralize in the `bob` CLI (dark-horse, best fit for this repo).** Add
  `bob capture "@route text"` that owns the format/routing/insertion logic in Rust (tested
  in this repo), and have Hammerspoon shell out to it — mirroring the existing
  `exec bob pomodoro` integration. Keeps Obsidian-closed capture, removes the Lua logic,
  and is reusable from any trigger or from QuickAdd itself (a macro could shell to it).

## Recommended Solution

**Choose by intent:**

- **If the goal is cross-device / mobile capture or richer capture (due dates, fields):** go
  with **B1 (hybrid)**. Keep Hammerspoon as a thin OS trigger and native prompt, and move the
  write into a QuickAdd **Capture choice** invoked via `obsidian://quickadd` with a **named**
  `value-task` variable. Reproduce `@route` routing and last-open-task insertion in a small
  QuickAdd macro, or have that macro shell out to a `bob` CLI helper so the logic stays in one
  place. Accept that capture now requires Obsidian to be running.

- **If the goal is simplification / maintainability (not mobile):** do **not** migrate to
  QuickAdd. The current Hammerspoon design is actually *simpler and more robust* for
  desktop-only global capture. Instead pursue **Option D** — lift the format/routing/insertion
  logic out of Lua into `bob capture`, and reduce `init.lua` to "collect text → shell to
  `bob capture`." This delivers the "single source of truth" benefit *and* fixes the
  external-write race (have `bob capture` write safely), while preserving Obsidian-closed
  capture and adding zero Obsidian-runtime dependency.

**Net:** Migrating wholesale to QuickAdd is justified only when mobile/cross-device capture is
a real requirement; in that case use the B1 hybrid. If the real motivation is cleaner code,
centralize capture in the `bob` CLI (Option D) and keep — or thin out — the Hammerspoon
trigger rather than adopting QuickAdd. In both winning paths Hammerspoon stays as the OS-level
hotkey; what changes is *where the write logic lives*.

## Sources

- [QuickAdd — Capture choice](https://quickadd.obsidian.guide/docs/Choices/CaptureChoice/)
- [QuickAdd — Format syntax](https://quickadd.obsidian.guide/docs/FormatSyntax/)
- [QuickAdd — Open QuickAdd from an Obsidian URI](https://quickadd.obsidian.guide/docs/Advanced/ObsidianUri/)
- [QuickAdd — Open QuickAdd from your Desktop (Advanced URI / hotkey)](https://quickadd.obsidian.guide/docs/Misc/AHK_OpenQuickAddFromDesktop/)
- [QuickAdd — QuickAdd API (`executeChoice`)](https://quickadd.obsidian.guide/docs/QuickAddAPI/)
- [QuickAdd — One-page inputs](https://quickadd.obsidian.guide/docs/Advanced/onePageInputs)
- [QuickAdd plugin (community)](https://community.obsidian.md/plugins/quickadd)
- [obsidian-global-hotkeys (Electron globalShortcut; works only while Obsidian runs)](https://github.com/mjessome/obsidian-global-hotkeys)
- [Electron globalShortcut API](https://www.electronjs.org/docs/latest/api/global-shortcut)
- [Obsidian Quick Capture overview](https://obsidian.rocks/obsidian-quick-capture/)
- [Raycast — Obsidian Smart Capture](https://www.raycast.com/millin_gabani/obsidian-smart-capture)
- Local source inspected: `~/.local/share/chezmoi/home/dot_hammerspoon/init.lua`
  (deployed to `~/.hammerspoon/init.lua`).
</content>
</invoke>
