---
create_time: 2026-06-15 16:20:45
status: done
prompt: sdd/prompts/202606/bob_capture_command.md
---
# Plan: `bob capture` — Native Task Capture Command + Thin Hammerspoon Keymap

## Goal

Move the task-capture _logic_ out of the Hammerspoon keymap and into a new, tested, beautiful `bob capture` subcommand,
then reduce the Hammerspoon hotkey to "prompt → shell out → notify". This is **Option D** from the consolidated research
note (`sdd/research/202606/hammerspoon_quickadd_task_capture_consolidated.md`): the recommended default because it
preserves fast OS-global capture _while GUI Obsidian is closed_, keeps task placement byte-for-byte identical, and moves
the brittle Markdown algorithm into unit-tested Rust in this repo.

The split is clean:

- **Moves into `bob capture`** (this repo): input normalization, `@route` parsing, task-line formatting, the
  `[created::YYYY-MM-DD]` stamp, inbox append, route-file creation, and the "insert after the last top-level task block"
  algorithm — plus a first-class CLI surface and color output.
- **Stays in Hammerspoon** (chezmoi repo): the global hotkey, the `hs.webview` "Capture Task" prompt + its HTML, focus
  save/restore, and the desktop notification. These are the OS-native parts a CLI cannot own.

The keymap "works seamlessly" because `bob capture` exposes a stable machine-readable contract (`--format json`) that
lets Hammerspoon rebuild today's exact notification (`"Captured task"` / route label / task text) from the command's
output, with exit codes for success/failure.

Work is split into **2 phases by repository**. Phase 1 (this `bob-cli` repo) delivers the command, tests, and docs and
is fully usable on its own. Phase 2 (the chezmoi repo) rewrites the keymap to delegate to it. Phase 1 is the headline
deliverable; Phase 2 is low-risk and reversible (keep the old Lua until the new path is exercised).

## Context Reviewed

- `memory/long/cli_rules.md` (read via `/sase_memory_read`, reason logged): make `-h/--help` excellent; **subcommands
  and options sorted alphabetically**; **every public long option needs a short alias**; prefer beautiful colored
  output. The implementing agent MUST re-run this `/sase_memory_read` procedure before touching the CLI surface.
- Consolidated research note `sdd/research/202606/hammerspoon_quickadd_task_capture_consolidated.md`: recommends exactly
  this command as the default; "Hammerspoon shells out to it, similar to the existing `bob pomodoro` integration."
- `src/runner.rs`: `SUBCOMMANDS` table (sorted; guarded by `subcommands_are_sorted_alphabetically`), the
  delegate-subcommand pattern (`trailing_var_arg` + `allow_hyphen_values` → each native module owns its own clap
  parser), `cli_styles()` ANSI palette, and the `AFTER_HELP` example list.
- `src/native/projects.rs` (newest sibling, the structural template): clap command via
  `try_get_matches_from_mut(once(COMMAND_NAME).chain(args))`, `print_clap_error`, `Styler`-based aligned/colored output,
  per-line success/warning rendering with a `success_prefix(dry_run)` marker, and a thorough `tests/cli.rs` +
  in-module-`tests` matrix.
- `src/native/env.rs`: `bob_dir()` (`BOB_DIR` override, default `~/bob`), `expand_tilde()`, and **`current_datetime()`**
  — honors `BOB_NOW`/`DATE`, so the `[created::]` stamp is deterministic under test. `bob capture` reuses all three.
- `src/native/style.rs`: `Styler::detect()` (color only on a TTY with `NO_COLOR` unset), `green/cyan/dim/...`,
  `success_prefix`, `pad_right` — the shared palette `bob capture` renders through.
- `src/native.rs`: `NativeCommand` enum + `run()` dispatch + `command_for_script()` (only legacy script-backed commands
  appear here; `capture` is native-only, so no entry).
- The live Hammerspoon source `/home/bryan/.local/share/chezmoi/home/dot_hammerspoon/init.lua` — the algorithm being
  ported (see Capture Contract) and the `bob pomodoro` shell-out precedent (`bobPomodoroCommand`: a login shell with a
  `PATH` that prepends `~/bin` and homebrew, plus an optional `DATE=gdate` shim).
- `tests/cli.rs` harness: `bob_command()`, `write_file()`, `TempDir`, `stdout/stderr`, `assert_success`,
  `assert_stdout_has_no_ansi`, `assert_text_order`; temp-vault tests drive behavior via `BOB_DIR` + `BOB_NOW`.
- `justfile` (`just all` = fmt + clippy + test; `install-smoke` enumerates every `--help`) and `README.md` Commands +
  Environment sections.

## Capture Contract (faithful port of the Hammerspoon algorithm)

`bob capture` must reproduce the current keymap behavior exactly, except for one explicitly-flagged hardening (see
"Intentional deviation"). Reference: `appendCapturedTask` / `parseCapturedTaskTarget` / `insertTaskAfterLastOpenTask` in
`init.lua`.

1. **Normalize** the raw text first: collapse every run of whitespace to a single space and trim ends (Lua `%s+`→`" "`
   then trim). All later steps operate on this one-line string.
2. **Route parse** (order matters — prefix wins):
   - **Prefix**: a leading `@<token>` followed by whitespace and more text → route = `<token>`, body = the rest. (Lua
     `^@([A-Za-z0-9_-]+)%s+(.+)$`.)
   - **Suffix**: otherwise, a trailing ` @<token>` at the very end → route = `<token>`, body = everything before it. The
     match is greedy, so for multiple `@`s the **last** trailing token wins (`a @b @c` → body `a @b`, route `c`). (Lua
     `^(.+)%s+@([A-Za-z0-9_-]+)$`.)
   - **Token charset** is exactly `[A-Za-z0-9_-]`; a bare `@route` with no accompanying text matches neither form and
     stays literal inbox text. Route names are **lower-cased**; body text keeps its original case.
3. **Task line**: `- [ ] #task <body> [created::YYYY-MM-DD]`, where the date comes from `bob_env::current_datetime()`
   (local date; `BOB_NOW`/`DATE` overridable).
4. **Unrouted** → append the task line to `<vault>/mac_inbox.md`.
5. **Routed** → target `<vault>/<route>.md` (route files live at the **vault root**, like the Lua). Create the file if
   missing, then **insert after the last top-level task block**:
   - A "top-level task line" is a line matching `^- [<any one char>] <ws>…#task` (column 0; any checkbox state — `[ ]`,
     `[x]`, `[X]`, `[-]`, `[/]`, `[B]` all count, matching the Lua `%[.%]`). Indented lines never match.
   - The insertion point is immediately **after that task line and its continuation block**: subsequent lines that are
     indented (`^[ \t]`), plus blank lines whose next non-blank line is indented, are skipped.
   - The **last** such block in the file wins.
   - Edge cases to preserve: empty/new file; a file with no task lines at all (append at end); a file not ending in a
     newline (the Lua adds a leading `\n` to the insertion / a separator on plain append — replicate the
     `needs_leading_newline` logic exactly); a final task line with a nested continuation block running to EOF.
6. The command is otherwise **lossless**: it splices one line and never re-serializes or reflows surrounding content.

**Intentional deviation (the one behavior change, flag it in docs):** the unrouted inbox append uses the same "add a
separating newline if the file doesn't already end in one" guard that the routed path already uses. The Lua inbox branch
appends blindly, which would concatenate onto a non-newline-terminated last line. The common case (files always end in
`\n`) is identical; this only hardens a corruption corner. If the reviewer prefers a 100% literal port, drop this guard
— it is isolated to the inbox branch.

## CLI Surface

```
bob capture [OPTIONS] [--] [TEXT]...        Capture a task into the Bob vault

Options (sorted; every long option has a short alias):
  -b, --bob-dir DIR     Bob vault root; defaults to BOB_DIR or ~/bob
  -d, --dry-run         Parse, format, and report the result without writing any file
  -f, --format FORMAT   Output format: human (default) or json
  -h, --help            Show help
  -r, --route NAME      Force the route to NAME.md; disables @route parsing of TEXT
```

- **TEXT** is a trailing, hyphen-tolerant positional (`trailing_var_arg` + `allow_hyphen_values`); multiple args are
  joined with single spaces before normalization, so `bob capture buy milk @groceries` works unquoted. Put flags before
  TEXT, or use `--` when the text itself starts with `-`.
- **stdin fallback**: if no TEXT positional is given and stdin is not a TTY, read one logical line from stdin (enables
  `echo "buy milk @groceries" | bob capture`). If both are empty → usage error.
- **`--route NAME`** is an additive convenience (lower-cased; routes to `<NAME>.md`) that bypasses `@`-parsing so the
  whole text becomes the body; a literal `@x` in the text is kept verbatim. Default (no flag) = current auto-routing.
- Registered in `src/runner.rs` `SUBCOMMANDS` in the alphabetical slot between `bulk-git-commit` and `dataview`
  (`name: "capture"`, `script_command: None`, `about: "Capture a task into the Bob vault"`,
  `native_command: NativeCommand::Capture`); new module `src/native/capture.rs`; new `NativeCommand::Capture` + `run`
  arm. Native-only (no legacy script, so no `command_for_script` entry).
- Help built with clap like `projects` (gives sorted options, short aliases, and a 2–3 line `after_help` examples
  block); `--help` exits 0 and emits no ANSI when piped.
- **Exit codes**: `0` success; `1` filesystem/write failure (`bob capture: <message>` on stderr); `2` usage error (clap,
  or empty input) via the `print_clap_error` pattern.

## Output Design

Color only on a TTY with `NO_COLOR` unset, via the shared `Styler` (same spirit as `projects`).

**human** (default) — a two-line confirmation: a green check + verb + cyan target label, then the rendered task line
dimmed beneath it:

```
✓ captured  groceries.md
  - [ ] #task buy milk [created::2026-06-15]
```

- Unrouted target label is `mac_inbox.md`.
- `--dry-run` uses the `success_prefix(dry_run)` marker / "would capture" phrasing and writes nothing.

**json** (`--format json`) — one stable snake_case object on stdout, the seam Hammerspoon parses:

```json
{
  "ok": true,
  "dry_run": false,
  "routed": true,
  "route": "groceries",
  "route_label": "groceries.md",
  "relative_target": "groceries.md",
  "target": "/home/bryan/bob/groceries.md",
  "text": "buy milk",
  "task_line": "- [ ] #task buy milk [created::2026-06-15]",
  "created": "2026-06-15",
  "placement": "inserted"
}
```

- `route`/`route_label` are `null`/`""` when unrouted (so `route_label` maps 1:1 to today's notification subtitle:
  `groceries.md` for routed, `""` for inbox).
- `placement` ∈ `created` (file did not exist) / `inserted` (spliced after a task block) / `appended` (no task block, or
  inbox append).
- On failure, json mode still prints a single object (`{"ok": false, "error": "..."}`) and exits non-zero, so the caller
  never has to distinguish "no output" from "crash".

## Hammerspoon Integration Contract (drives Phase 2)

The keymap keeps the hotkey, the webview prompt/HTML, focus save/restore, and the notification. On submit it shells out
asynchronously (so the UI never blocks), mirroring the `bob pomodoro` precedent:

- Command: a login shell that prepends `~/bin`/homebrew to `PATH` (same as `bobPomodoroCommand`) running
  `bob capture --format json -- <text>`.
- On exit 0: `hs.json.decode` stdout → `hs.notify.show("Captured task", result.route_label, result.text)`, then close
  the prompt and restore the previous app (today's exact notification).
- On non-zero: keep the prompt open (or reopen) and `hs.notify.show("Task capture failed", "", <stderr/error>)`.

All of `normalizeTaskText`, `parseCapturedTaskTarget`, the file-IO helpers, `insertTaskAfterLastOpenTask`, and the inbox
branch of `appendCapturedTask` are **deleted** from `init.lua`; only UI + notify + shell-out remain.

---

## Phase 1 — `bob capture` Command, Tests, Docs (this `bob-cli` repo)

**Purpose**: the complete, self-sufficient command.

### Scope

- Re-run `/sase_memory_read` on `memory/long/cli_rules.md` before touching the CLI.
- `src/native/capture.rs`: clap parser per **CLI Surface**; the **Capture Contract** algorithm (its own normalize /
  route-parse / task-format / insert-after-last-task helpers); `Styler` human output + `serde_json` json output; reuse
  `bob_env::{bob_dir, expand_tilde, current_datetime}`.
- Wire-up: `NativeCommand::Capture` + `run` arm in `src/native.rs`; `SUBCOMMANDS` row + an `AFTER_HELP` example (e.g.
  `bob capture buy milk @groceries`) in `src/runner.rs` (keep the example list and table alphabetical).
- `justfile` `install-smoke` gains `bob capture --help`; README Commands section gains a `bob capture` block
  (positional, options, the auto-route rules, the json contract, the Hammerspoon integration, and the
  `BOB_DIR`/`BOB_NOW` env notes), and the Environment section notes `BOB_NOW`/`DATE` affect the `[created::]` stamp.
- **Tests**:
  - In-module unit tests: normalization; prefix/suffix/multi-`@`/none route parsing + lower-casing + bare-`@route`
    literal; `--route` override; task-line format under a pinned `BOB_NOW`; and the insertion algorithm across the full
    edge set (empty/new file, no-task append, append-with-no-trailing-newline, after a single task, after a task with
    indented + blank-then-indented continuation lines, last-of-many tasks, leading-newline case); json field shape.
  - `tests/cli.rs` integration tests (temp vault via `BOB_DIR`, date via `BOB_NOW`): unrouted append to `mac_inbox.md`;
    prefix and suffix routed insert (incl. file auto-create); `--dry-run` writes nothing but prints the dry-run line;
    `--format json` emits a valid object with the documented fields; stdin fallback; empty input → exit 2; a
    `capture_help_is_native_only`-style test (no script extraction) plus a help test asserting sorted short/long options
    and `assert_stdout_has_no_ansi` when piped.

### Verification

- `just all` green (`cargo fmt --check`, `cargo clippy --all-targets`, `cargo test`).
- Manual smoke against a scratch `BOB_DIR` (never the live vault): unrouted, `@prefix`, `suffix @route`, and a route
  file that already has tasks + continuation lines — confirm placement matches the current keymap and json round-trips.
- `/sase_git_commit` of only this phase's files.

---

## Phase 2 — Thin the Hammerspoon Keymap (chezmoi repo)

**Purpose**: delegate the keymap to `bob capture` per the Integration Contract. Separate repo
(`/home/bryan/.local/share/chezmoi/home/dot_hammerspoon/init.lua`), separate commit.

### Scope

- Replace the capture logic in `init.lua` with: keep hotkey + webview prompt/HTML + focus save/restore + notification;
  on submit, async `hs.task` shell-out to `bob capture --format json -- <text>`; parse via `hs.json.decode`; notify with
  `route_label`/`text` on success, stderr/error on failure; delete the now-unused Lua helpers.
- Keep the prompt's existing one-line paste normalization (cheap client-side nicety; `bob capture` re-normalizes
  server-side regardless).

### Verification (manual, on the macOS host where Hammerspoon runs)

- `chezmoi apply` (or edit the source then apply), let the pathwatcher reload, and exercise: unrouted, `@prefix`,
  `suffix @route`, a brand-new route file, and a route file with an existing task + nested continuation — each lands
  identically to the pre-migration behavior, with the same notification.
- Confirm capture still works with GUI Obsidian fully closed (the property that justified Option D).
- Keep a copy of the old Lua until the new path has handled real daily captures; revert is a one-file restore.

> **Open decision for the reviewer**: Phase 2 edits a _different_ repo (chezmoi), outside this SASE workspace and not a
> configured sibling. The recommendation is to land Phase 1 here, then either (a) I apply Phase 2 in the chezmoi repo as
> a clearly-scoped follow-up with its own commit, or (b) hand off the drop-in Lua for Bryan to apply. Phase 1 stands
> alone regardless.

## Risks

- **Algorithm fidelity**: the "insert after last task block" + `needs_leading_newline` logic is subtle. Mitigation: port
  it line-for-line and pin every Lua edge case as a unit test before wiring the CLI.
- **Free-form text vs. flags**: task text containing leading `-` could be read as an option. Mitigation:
  `trailing_var_arg` + `allow_hyphen_values`, document "flags before text / use `--`", and have Hammerspoon always pass
  `--format json -- <text>`.
- **`bob` on Hammerspoon's PATH**: GUI-launched Hammerspoon has a minimal `PATH`. Mitigation: reuse the proven
  `bobPomodoroCommand` login-shell + PATH-prefix pattern.
- **Date source divergence**: `bob capture` uses `current_datetime()` (local) like the rest of the CLI; the Lua used
  `os.date`. Same local date in practice; tests pin `BOB_NOW`, and the optional `DATE=gdate` shim matches the pomodoro
  setup.
- **Cross-repo coupling**: a future change to the json field names would break the keymap. Mitigation: the json contract
  is documented in the README and asserted by a CLI test; treat it as a stable interface.

## Out of Scope (possible follow-ups)

- A QuickAdd hybrid / `obsidian://quickadd` path for mobile/Obsidian-native capture (the research note's Option C) —
  only worth it if cross-device capture becomes a real requirement; `bob capture` is the desktop default until then.
- New capture fields (due date, priority `[p::N]`, context/project) — the json contract leaves room, but they are not in
  this scope.
- `bob capture` writing through an Obsidian runtime instead of the filesystem (it remains an external writer, exactly as
  the keymap is today).
- A literal-parity mode that drops the inbox newline guard (only if the reviewer wants a 100% byte-identical port).
