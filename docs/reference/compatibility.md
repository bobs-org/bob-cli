---
title: "Requirements and compatibility"
description: "Tested snapshot plus the minimum Obsidian, platform, and toolchain versions Bob needs."
template: compatibility.html
---

# Requirements and compatibility

Bob runs one Markdown vault through a Rust CLI, Obsidian plugins, and an
optional Mac panel. The snapshot above names the exact revisions this site
was built from. The floors below are the minimum versions readers need.

## Minimum requirements

- **CLI platforms:** Linux and macOS. The CLI, clipboard discovery, and
  browser helpers cover both; Mac-only paths note it where they differ.
- **Obsidian:** 1.8.7 or later. Every bundled Bob plugin manifest sets
  `minAppVersion` 1.8.7, so the floor follows the manifests, not this
  site's builder version. The mounted [Obsidian plugins inventory](plugins/index.md)
  is generated from the manifests, making it the source of truth for this floor
  and current manifest versions.
- **Tasks plugin:** use a current Tasks release with the Dataview task
  format and the `#task` global filter. The native query engine follows
  Tasks 8.4.0 filter behavior for `status.symbol`; the parity fixture and
  goldens run headless without Obsidian. Desktop rendering inside Obsidian
  still awaits a human pass.
- **Rust:** version 1.95.0 or later from the
  [Rust toolchain](https://rust-lang.org/tools/install). The repository pins
  1.95.0 in `rust-toolchain.toml`, and the package declares `rust-version = "1.95"`.
- **Mac Capture (optional):** macOS 26 with Command Line Tools for Xcode
  26+ or Xcode 26+, plus a signed or ad-hoc signed `bob` on `PATH`. See
  [Install](https://bobs-org.github.io/reference/mac-capture/install/),
  [Build](https://bobs-org.github.io/develop/mac-capture/build/),
  [README › Signing](https://github.com/bobs-org/bob-mac-capture#signing),
  and the [protocol compatibility table](https://bobs-org.github.io/reference/mac-capture/protocol/).

## Tested snapshot

The block above the title renders the rolling snapshot from the site
build: per-repository revisions with dirty and branch marks, the build
time and builder, tool versions, and `build.json`. Absent checkouts
report an unknown revision rather than a guessed one, and optional tool
versions appear only when the builder records them.

## Compatibility shims

The installed compatibility binaries map to the preferred interface as
follows:

| Compatibility binary | Preferred command |
| --- | --- |
| `bob_notify` | `bob pomodoro notify` |
| `bob_pomodoro` | `bob pomodoro` |
| `tmux_bob_pomodoro` | `bob pomodoro tmux` |

By default they call the same native Rust implementations as the preferred
commands. With `BOB_CLI_USE_SCRIPT=1`, the notification and Pomodoro
commands and their shims delegate to their embedded shell assets.
Native-only commands ignore the fallback setting. Extracted assets are
cached in a version-and-content-specific subdirectory of
`$XDG_CACHE_HOME/bob-cli/scripts/`. If `XDG_CACHE_HOME` is unset or empty,
the base is `$HOME/.cache`; if neither variable is available, Bob uses the
system temporary directory.

Old top-level spellings remain as hidden aliases. See
[Migration notes](../develop/migration-notes.md).

## Capture-language revision

`capture-parse` and `capture-complete` keep `schema_version: 1` and report the
additive top-level `grammar: 23` revision. Clients can use it to distinguish
capture-language updates while continuing to read the same versioned JSON
envelope. Grammar 23 splits `#section^id` into a section selector plus an
optional named bullet ID (a caret inside a quoted title stays title text) and
claims notebook list, global, and forced destinations as notes. Grammar 22
makes sized `Nx` closes execute (older clients parse the size as literal
task text) with an additive `pomodoro_close.size` object.
Grammar 20 claims `@dir/` record routes (older clients parse them as
literal task text) and reports record parse metadata; `capture-complete` adds
`record_field`/`record_value` menus, `-n/--next-missing-field`,
`-F/--record-field KEY`, `record.edit`, `record.assist_version: 1`, and
optional candidate `continuation` without bumping the revision. Grammar 18 treats trailing `+` after a bare start name as later-start
timing (`=fix2+`, `=bugs+2`) and uses `#` for names that end in `+` (`=#c++`).
Grammar 17 adds signed later offsets to whole-item, named, and linked
Pomodoro starts, and reports nonzero `later_units`. Grammar 16 adds offline
Gmail spans, per-item preview data, and saved-thread link substitution at
capture submit. Grammar 8 kept grammar 7's note-pull behavior: an exactly lone
`@route` task-list header is a note pull, `@route` with bullets remains a
list, and a lone `@route#section` remains an empty-list refusal. Grammar 8
also gives `- ^<id>` and `- :<id>` move meanings inside sealed project-note
items. At that position a lone `- ^` or `- :` now needs `project_task_move`
instead of the former `block_id` picker need, so editor clients must handle
that context before enabling the grammar. Grammar 9 adds adjacent target runs
of `@route+block-id` markers at the start or end of a capture line; the run
fills one route slot and applies the same item semantics to each task. Parse
JSON adds `targets[]` for runs and the `duplicate_capture_target` and
`split_capture_targets` diagnostics, without changing `schema_version`.
Grammar 15 adds `<` merge-in grammar, editor parse, and teaching refusals; `bob capture` executes a complete `<` clause as one explicit merge into the new task. Grammar 10 adds `>` transfer lines (`@a+x @b+y > @dest`, always quoted so
the shell keeps the `>`): a complete transfer reports mode `task_move` or
`task_merge` with the `transfer` object and the `transfer_source`,
`transfer_arrow`, and `transfer_destination` spans, while the legacy
`route`/`block_id` describe the destination. A line that used to file the
note `> @dest` now claims a transfer directive, so clients must handle the
new modes before enabling the grammar.

Grammar 11 adds the `^^` project-task move choice to sealed project-note
items. A whole first-level `- ^^` bullet is a move-choice slot, not a
literal task named `^^`. `capture-parse` reports
`project_task_move_choice_marker` and `sub_bullet_move_choices[]`, keeps the
parse complete, and leaves the corresponding `sub_bullet_moves` entry null
so an older client never sees an unfinished `- ^`. `capture-complete` adds
`action_continuation_keys: ["^"]` on a lone `- ^`. `bob capture` adds
`-m/--move-in`, and every capture JSON result carries
`move_choice.supported: true` for capability detection.

The old-bob and old-Mac combinations behave as follows:

| Mac × bob | Behavior |
| --- | --- |
| new × new | Full move-choice flow. |
| old Mac × new bob | The app sends no `--move-in`; bob refuses `move_choice_required` without writing. |
| new Mac × old bob | On the first ⌘↩ for a whole-line `- ^^`, warn `This bob predates - ^^; it would capture a task named ^^ · update bob`. Send `--move-in` only after a preview advertised support. |
| old × old | Unchanged; `^^` remains a literal task. |

Grammar 12 claims whole-item `x=` as a past Pomodoro record: stacks end at
the next five-minute boundary and fill backward, while `!` can trim only an
edge of an older closed row. It adds the `pomodoro_past` mode and object;
clients should treat that as a distinct session operation rather than a
close of the running Pomodoro.

Grammar 13 glues start and past timing to bare Pomodoro names, permits timing
on either side of a bare start name, and retires `@` timing separators and
redundant `#` prefixes. Bare names contain no `-` and do not end in an ASCII
digit; `#` remains for names that cannot be spelled bare.

Grammar 14 records past Pomodoros with an optional segment suffix
(`x=E[s][!][=X]`): `~` starts where the last closed Pomodoro ended, `~n`
starts at segment `n`, `~n+` ends at segment `n`, and `~~` ends where the
first Pomodoro starts. A run hangs from one pin, typed counts stay exact,
untyped counts stop at the segment's far edge, nothing records past now, and
`!` still only trims one edge. Capture JSON gains `pomodoro_past.segment`,
`stack.pin`, and `window.requested_minutes`/`cut`/`wall`/`fit`, all omitted
for unsuffixed drafts.

## Runtime dependencies

Native command execution does not require Bash or Perl. Forced shell
fallback with `BOB_CLI_USE_SCRIPT=1` requires Bash, and the Pomodoro-based
fallback scripts also require Perl.

The documented workflows use these external-tool integrations:

- `obsidian` CLI plus a running desktop Obsidian vault with the Dataview
  plugin only when using `bob query --engine obsidian`
- `git` for `bob vault-sync`, Git-backed `bob task archive`, plugin
  dirty-file checks, plugin deploy-stamp ignore and ancestry probes, and
  the default `bob plugins` repository refresh; remote operations also
  need the credentials required by the configured remote
- `notify-send` for desktop notifications from `bob pomodoro notify`; Bob
  also rings the terminal bell whether or not `notify-send` is available
- platform clipboard tools for `bob capture` clipboard input: `pbpaste` on
  macOS; `wl-paste`, `xclip`, or `xsel` on Linux; or `tmux show-buffer` in
  a display-less tmux session (see `BOB_CLIPBOARD_CMD` in
  [configuration](configuration.md) for the exact fallback order)
- `pandoc` and `xelatex` for `bob ref create` Markdown targets, plus the
  LaTeX packages `bob ref doctor` checks; override pandoc with
  `BOB_PANDOC_COMMAND`. Doctor prints the `tlmgr install` line for any
  missing packages
- `pandoc` also saves web articles and collections as Markdown with code/link
  catalogs. Without it, web PDF capture can still succeed, but reports
  `source_markdown: none (pandoc unavailable)`; XeLaTeX is not needed for
  this conversion. Reading an existing catalog with `bob ref code` or
  `bob ref links` needs neither renderer
- `pdftotext` from Poppler for `bob ref attach-source` to verify saved or
  candidate Markdown against a library PDF. Search candidates also use
  `pandoc` to record their code/link catalog; `bob ref recatalog` uses
  Pandoc to add links to an existing catalog. Neither command needs XeLaTeX
- `uv` and a discovered browser are optional for Mermaid fences in
  Markdown imports; without them, Mermaid prints as source with a visible
  note
- `curl` for `bob ref create` PDF URL and arXiv targets; override with
  `BOB_HIGHLIGHTS_CURL`
- A narration frontend for `bob ref create --listen`, configured as
  `highlights.listen_command` (`sase listen render ...`). Installing Bob
  does not install one. The SASE plugin (`sase plugin install listen`)
  provides `sase listen render {target} -e full -o {audio}`; a direct
  `sase-listen render ...` setting migrates automatically, and
  wrappers/shell programs are rejected
- `uv` plus a browser for `bob ref create` web article targets. `uv`
  fetches Python ≥3.10 and the pinned Playwright, Pillow, and nh3 on first
  run (override the whole spawn with `BOB_WEB_CLIP_ADAPTER`). Discovery
  uses Google Chrome at `/opt/google/chrome/chrome`,
  `/usr/bin/google-chrome`, `/usr/bin/google-chrome-stable`, or the macOS
  Google Chrome app, then Playwright's bundled Chromium, newest installed
  revision first. Bundled Chromium launches by its discovered path, so the
  pinned Playwright's default revision need not be installed (as on apollo
  after revision 1248 replaced 1234). Current macOS Chrome-for-Testing and
  legacy Chromium app layouts are supported. Set `BOB_CHROME` to another
  executable, including a system Chromium package. On Linux,
  `Xvfb` lets it retry bot-protected sites headed; on macOS the retry uses
  an off-screen window
- `bash` for the embedded shell fallback and for sourcing
  `~/.ssh-agent-thing`; the Pomodoro shell fallback additionally uses
  `perl`
- `uv` for `bob gkeep`: it fetches Python ≥3.10 and the pinned `gkeepapi`
  on first run (override the whole spawn with `BOB_GKEEP_ADAPTER`)
- `gog` ≥0.44.0 for `bob gcal`, with a Google Desktop-app OAuth client and
  a read-only Calendar grant established through `bob gcal login`
- `pass` for the default `bob gkeep` token store (`token_command` and
  `token_store_command` in the `gkeep:` config section)

No old chezmoi script files are required after installation. Cargo
installs the Rust binaries, and the binaries carry the script assets they
need. Environment variable defaults live in
[configuration](configuration.md).

## Related

- [Install Bob](../start/install.md)
- [Continue in Obsidian](../start/obsidian.md)
- [Continue with Mac Capture](../start/mac-capture.md)
- [Look up Bob](index.md)
- [Configuration](configuration.md)
- [Migration notes](../develop/migration-notes.md)
