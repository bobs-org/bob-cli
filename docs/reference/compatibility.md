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
- **Rust:** a current stable compiler that supports edition 2024 from the
  [Rust toolchain](https://rust-lang.org/tools/install). No minimum version
  is pinned here.
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

## Runtime dependencies

Native command execution does not require Bash or Perl. Forced shell
fallback with `BOB_CLI_USE_SCRIPT=1` requires Bash, and the Pomodoro-based
fallback scripts also require Perl.

The documented workflows use these external-tool integrations:

- `obsidian` CLI plus a running desktop Obsidian vault with the Dataview
  plugin only when using `bob query --engine obsidian`
- `git` for `bob vault-sync`, Git-backed `bob task archive`, plugin
  dirty-file checks, and the default `bob plugins` repository refresh;
  remote operations also need the credentials required by the configured
  remote
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
  Google Chrome app, then Playwright's bundled Chromium. Set `BOB_CHROME`
  to another executable, including a system Chromium package. On Linux,
  `Xvfb` lets it retry bot-protected sites headed; on macOS the retry uses
  an off-screen window
- `bash` for the embedded shell fallback and for sourcing
  `~/.ssh-agent-thing`; the Pomodoro shell fallback additionally uses
  `perl`
- `uv` for `bob gkeep`: it fetches Python ≥3.10 and the pinned `gkeepapi`
  on first run (override the whole spawn with `BOB_GKEEP_ADAPTER`)
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
