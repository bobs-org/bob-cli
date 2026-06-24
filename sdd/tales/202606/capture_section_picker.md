---
create_time: 2026-06-24 08:20:32
status: wip
prompt: sdd/prompts/202606/capture_section_picker.md
---
# Plan: Section picker for trailing `#` capture marker

## Goal

Extend the Hammerspoon task-capture flow (the `<cmd+ctrl+shift+i>` prompt) so that when the typed text ends in a
**bare** bullet marker (`@#` or `@route#`, with _no_ characters after the `#`), the user is offered a chooser of the
sections in the selected note before the bullet is written. The section chooser appears **after** the area/project note
chooser when one is needed, and only when the chosen note has **2 or more** non-`Tasks` sections. With 0 or 1
non-`Tasks` sections, the bullet is placed silently in the single non-`Tasks` section (current behavior), so no extra
prompt is introduced where there is nothing to choose.

This mirrors the existing area/project note picker that fires on a trailing `@` / `@#`.

## Decisions locked from Q&A

- **Q1 — trigger scope: both `@#` and `@route#`.**
  - Trailing bare `@#` → note chooser, then (conditionally) the section chooser.
  - Trailing bare `@route#` (explicit route) → skip the note chooser (note already known), go straight to the
    (conditional) section chooser.
- **Q2 — a typed prefix keeps current behavior.** `@#Idea` and `@route#Idea` are unchanged: the section is considered
  already chosen, the existing `@route#Idea` synthesis / passthrough runs, and Bob prefix-matches the heading. The
  section chooser never appears when a prefix is present.
- **Q3 — list all non-`Tasks` ATX headings, any level (H1–H6), in document order.** This is exactly the set Bob can
  already target for a bullet, minus `Tasks`.

## Current architecture (what exists today)

Capture is a two-process dance driven entirely from Hammerspoon
(`~/.local/share/chezmoi/home/dot_hammerspoon/init.lua`):

1. `parseCaptureRequest(rawText)` inspects the **final** whitespace-delimited token and recognizes two opt-in markers,
   returning `(text, pickerRequested, bulletSuffix)`:
   - `… @` → `(text, true, nil)` — open the area/project picker, capture a **task**.
   - `… @#<prefix>` → `(text, true, "#"..prefix)` — open the picker, then synthesize a leading `@route#<prefix>` bullet
     token. A bare `@#` currently yields `bulletSuffix = "#"`.
   - anything else → `(text, false, nil)` — handed verbatim to `bob capture`, which owns its own `@route` /
     `@route#section` parsing.
2. `startTargetsStage` runs `bob capture-targets --format json` (lists inbox/area/active-project routes), then
   `showTaskCaptureChooser` renders area/project rows. On selection it either forces the route
   (`runFinalCapture(text, route, …)`) or, for a bullet, synthesizes
   `runFinalCapture("@"..route..bulletSuffix.." "..text, nil, …)`.
3. `runFinalCapture` shells out via `taskCaptureCommand`: `$1` = text, `$2` = optional forced route →
   `bob capture --format json [--route "$2"] -- "$1"`.

On the Rust side (`src/native/capture.rs`):

- A trailing/leading `@route#section` token parses to `CaptureKind::Bullet { section_prefix }`.
- `target_bullet_section` / `heading_matches_bullet_prefix` place the bullet in the first non-`Tasks` heading whose
  title **starts with** the prefix (case-insensitive), preferring a non-H1 match and falling back to an H1 match, then
  to the pre-heading (zeroth) section. A bare `#` (`section_prefix = None`) matches any non-`Tasks` heading.
- `markdown_headings` already collects every ATX heading (skipping YAML frontmatter and fenced code) with
  `(line_index, level, title)`; `Tasks` is filtered by `title != "Tasks"`.
- `bob capture-targets` (`src/native/capture_targets.rs`) is the existing precedent for a small, read-only,
  JSON-emitting helper command that exists purely to feed the Hammerspoon chooser. It is not documented in `README.md`;
  it lives entirely behind excellent `--help`.

Today, both bare `@#` (after the note pick) and explicit `@route#` resolve **silently** to the first non-`Tasks`
section. This plan turns that silent resolution into a chooser when there is a real choice (2+ non-`Tasks` sections).

## Key correctness consideration: exact vs. prefix section selection

The chooser presents **exact** heading titles and the user picks exactly one. Reusing Bob's existing prefix matcher
(synthesizing `@route#<chosenTitle>`) would misfire in the case where the chosen title is a strict prefix of an
earlier/higher-priority sibling heading (e.g. picking `Idea` when an `Ideas` heading sorts first), routing the bullet to
the wrong section — defeating the purpose of a picker whose whole point is precise selection.

**Therefore the section chooser targets the chosen heading by exact, case-insensitive title match, via a new `--section`
option on `bob capture`.** Prefix matching is retained unchanged for the typed `@route#prefix` token path (Q2). Exact
selection only governs picker results.

## Design

### Part A — Rust: new read-only helper `bob capture-sections`

New module `src/native/capture_sections.rs`, registered like `capture-targets`:

- `src/native.rs`: add `mod capture_sections;`, a `NativeCommand::CaptureSections` variant, and the dispatch arm.
- `src/runner.rs`: add a `SUBCOMMANDS` entry (kept alphabetically sorted — it sits right after `capture-targets`). About
  line: e.g. "List the non-Tasks sections of a capture note".

Command shape (options sorted alphabetically, each with a short alias per CLI rules):

- `-b, --bob-dir DIR` — vault root; defaults to `BOB_DIR` or `~/bob` (mirror `capture-targets`).
- `-f, --format human|json` — default `human`.
- `-h, --help`.
- `-r, --route NAME` — **required**; the note whose sections to list. Validated/lower-cased with the same rules as a
  capture route (`capture::is_route_token`, lower-cased), resolving to `<bob_dir>/<route>.md`.

Behavior (read-only):

- Missing target file → success with an empty section list (`count: 0`). A note that does not yet exist simply has
  nothing to choose, so the caller skips the chooser; Bob will create the file on the eventual capture. (Do **not**
  treat a missing note as an error.)
- Existing file → read it and list **all** non-`Tasks` ATX headings (any level) in document order.
- Real read/IO failure → JSON `{ "ok": false, "error": … }` / nonzero exit, matching the `capture-targets` error
  contract.

Stable JSON contract consumed by Hammerspoon:

```json
{
  "ok": true,
  "route": "cash",
  "count": 2,
  "sections": [
    { "title": "Ideas", "level": 2 },
    { "title": "Log", "level": 3 }
  ]
}
```

Human format: a short, scannable, colorized list (heading title + level), consistent with the project's other helpers
and the CLI-rules "prefer colored output" guidance.

Reuse, don't duplicate, the heading parser: add one minimal `pub(crate)` helper to `capture.rs`, e.g.
`pub(crate) fn non_tasks_section_headings(contents: &str) -> Vec<SectionHeading>` (where
`SectionHeading { title: String, level: usize }`), implemented on top of the existing private `line_spans` +
`markdown_headings` + `title != "Tasks"` filter. `capture_sections.rs` calls this so the picker's notion of "section"
stays identical to Bob's bullet-placement notion.

### Part B — Rust: exact section targeting in `bob capture`

Add a public `-s, --section TITLE` option to `bob capture` (`src/native/capture.rs`):

- Requires `--route` (the picker always knows the route); `--section` without `--route` is a usage error.
- Forces **bullet** mode and selects the section by **exact, case-insensitive title** match among non-`Tasks` headings,
  keeping the existing non-H1-preferred ordering and the zeroth-section fallback when no title matches. Keeps `@tokens`
  in the body literal, exactly as `--route` does.
- Empty `--section ""` is a usage error.

Implementation notes (localized, low blast-radius):

- Thread the route + section through `CaptureRequest` (new `forced_section: Option<String>`).
- In `parse_capture_text`, handle `forced_section` before the auto-route logic: build
  `ParsedCaptureText { route: Some(route), kind: Bullet{…}, body }` directly.
- Extend the bullet kind to carry an exactness signal — simplest is
  `CaptureKind::Bullet { section_prefix: Option<String>, exact: bool }` (token paths set `exact: false`; `--section`
  sets `Some(title)` + `exact: true`). Thread `exact` into `insert_bullet_line` → `target_bullet_section` → the title
  matcher, where `exact` switches `starts_with` to case-insensitive equality. A `None` selector (bare `#`) is unaffected
  by `exact`.
- This leaves every existing token-driven prefix path byte-for-byte unchanged.

### Part C — Hammerspoon: trigger parsing and the section chooser

All changes in `~/.local/share/chezmoi/home/dot_hammerspoon/init.lua`. Deploy with `chezmoi apply` (its path watcher
auto-reloads Hammerspoon); never edit the deployed `~/.hammerspoon/init.lua` directly.

1. **Richer request parsing.** Replace the `(text, pickerRequested, bulletSuffix)` tuple from `parseCaptureRequest` with
   a small request descriptor carrying a `mode`. Recognized trailing forms (each still requires the marker to be the
   final token, preceded by whitespace, as today):
   - `… @#<prefix>` with **non-empty** prefix → `mode = "note_bullet"` (note chooser, then synthesize `@route#<prefix>`;
     unchanged behavior).
   - `… @#` (empty prefix) → `mode = "note_section"` (note chooser, then conditional section chooser). _This replaces
     the previous silent bare-`@#` behavior._
   - `… @route#` (route is a valid token, empty prefix) → `mode = "section"`, capturing the route (matched with
     `[%w_-]+`, lower-cased). No note chooser.
   - `… @` → `mode = "note"` (note chooser, capture a task; unchanged).
   - otherwise → `mode = "none"` (verbatim to `bob capture`; unchanged). `@route#prefix`, `@route`, mid-text `@`, etc.
     continue to fall here.

   Order the checks so the `@#`/`@#prefix` form is tested before the `@route#` form, and validate the route token so
   `@#` never matches the route branch.

2. **Note chooser dispatch.** `showTaskCaptureChooser` branches on `mode` when a row is picked:
   - `note` → `runFinalCapture(text, route, …)` (force route, task).
   - `note_bullet` → `runFinalCapture("@"..route.."#"..prefix.." "..text, nil, …)` (unchanged).
   - `note_section`→ `startSectionStage(text, route, pickedName, pickedKind)` (new).

3. **Section stage (new).** `startSectionStage(text, route, pickedName, pickedKind)`:
   - Runs `bob capture-sections --format json --route <route>` through the existing `startCaptureStage` machinery (with
     its in-flight guard and JSON decoding).
   - `count >= 2` → `showTaskSectionChooser(text, route, sections, pickedName, pickedKind)`.
   - `count <= 1` → place silently in the single (or zeroth) non-`Tasks` section by synthesizing a bare `@route#`
     capture: `runFinalCapture("@"..route.."# "..text, nil, …)` — identical to today's silent resolution.
   - A `capture-sections` failure surfaces the existing dedicated picker-failure notification (never a silent
     fall-through to the inbox).

4. **Section chooser (new).** `showTaskSectionChooser` builds one row per section (`text = title`, `subText` showing the
   heading level to disambiguate same-named or nested headings). Dismissing it refocuses the prompt with the typed text
   intact (mirroring the note chooser). On selection it calls the capture with the **exact** chosen section.

5. **Final capture with a section.** Extend `runFinalCapture` with an optional `section` argument and
   `taskCaptureCommand` with a `$3` positional:

   ```sh
   if   [ -n "${3:-}" ]; then exec bob capture --format json --route "$2" --section "$3" -- "$1"
   elif [ -n "${2:-}" ]; then exec bob capture --format json --route "$2" -- "$1"
   fi
   exec bob capture --format json -- "$1"
   ```

   `section` is only ever passed together with a route, so the `$3` branch always has a valid `$2`. The notification
   path reuses `pickedName`/`pickedKind`; for the direct `@route#` flow (no note pick) pass the route as the destination
   name.

6. **Update the explanatory comments** above `parseCaptureRequest`, `showTaskCaptureChooser`, and `startTargetsStage` to
   describe the new bare-`#` section-picker forms and the exact-match contract, keeping them as accurate as the current
   ones.

### Concurrency / lifecycle

The section stage starts only from inside a chooser/stage callback, after the prior `hs.task` has cleared
`taskCaptureTask` and the prior chooser has cleared `taskCaptureChooser`, so the existing single-in-flight guards
continue to hold. The prompt-close path (`cancelTaskCaptureTask` / `cancelTaskCaptureChooser`) already tears down
whichever stage is live.

## End-to-end behavior matrix

| Typed trailing form                 | Note chooser | Section chooser                          | Result                                          |
| ----------------------------------- | ------------ | ---------------------------------------- | ----------------------------------------------- |
| `… @`                               | yes          | —                                        | task forced to picked route                     |
| `… @#`                              | yes          | if picked note has ≥2 non-Tasks sections | bullet → chosen (or only) section               |
| `… @#Idea`                          | yes          | never                                    | bullet synthesized `@route#Idea` (prefix match) |
| `… @cash#`                          | no           | if `cash.md` has ≥2 non-Tasks sections   | bullet → chosen (or only) section               |
| `… @cash#Idea`                      | no           | never                                    | verbatim to Bob → `@cash#Idea` (prefix match)   |
| `… @cash`, mid-text `@`, plain text | no           | —                                        | unchanged                                       |

## Testing

**`src/native/capture.rs` unit tests**

- Exact selection: exact title wins over a prefix-sibling that would otherwise capture it; exact non-H1 preference;
  exact case-insensitive; exact no-match falls back to the zeroth section; bare `#` (`None`) unaffected by `exact`.
- Parse: `--section` forces `Bullet{exact:true}` with the forced route; `--section` without `--route` is a usage error;
  empty `--section` is a usage error; existing token-prefix tests still pass (updated only for the new `exact: false`
  field).

**`src/native/capture_sections.rs` unit tests**

- `non_tasks_section_headings` excludes `Tasks`, includes all heading levels in document order, and skips headings
  inside frontmatter / fenced code.
- Route validation/lower-casing; missing-file → empty list; JSON shape stability.

**`tests/cli.rs` integration**

- `bob capture-sections --route foo --format json` lists sections in order; missing note → `count:0`; invalid/missing
  `--route` errors cleanly.
- `bob capture --route foo --section "Ideas" -- text` lands the bullet in `Ideas` exactly (including the
  prefix-collision case); `--section` without `--route` is a usage error.
- Add `capture-sections` to the subcommand-help enumeration tests (the three `&[(&[&str], &str)]` arrays) and add a
  native-only `--help` test plus an alphabetical-options `--help` test mirroring the `capture-targets` ones. The
  `subcommands_are_sorted_alphabetically` guard in `runner.rs` covers ordering automatically.

**Hammerspoon** — no automated harness exists; validate manually after `chezmoi apply` across the matrix above: bare
`@#` into a 2+/1/0-section note, `@cash#` into a 2+/1-section note, `@#Idea` and `@cash#Idea` unchanged, chooser-dismiss
preserves typed text, and a forced `capture-sections` failure surfaces the picker-failure notification.

## Documentation

- `bob capture` `--help` / `long_about` and the `README.md` capture section: document `--section` (exact,
  case-insensitive; requires `--route`; forces a bullet) alongside the existing `#`-token description.
- `bob capture-sections` lives behind excellent `--help` only, matching the undocumented-in-README precedent of
  `capture-targets`.
- No SASE memory files are modified.

## Out of scope / non-goals

- No change to typed-prefix (`@route#prefix`) semantics — still prefix-matched by Bob (Q2).
- No section _creation_: the chooser only lists existing non-`Tasks` headings.
- No ambiguity resolution for two headings sharing an identical title (inherently indistinguishable in a title-based
  chooser); exact match keeps the existing non-H1-preferred, document-order tiebreak and the `subText` level hint helps
  the user tell them apart.
