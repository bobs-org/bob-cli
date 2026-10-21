---
title: Release checklist
description: Manual package, documentation, and vault smoke checks that complement the release train.
---

# Release checklist

The [release train](releases/index.md) already runs full CI and
`just package-check` on the SHA it ships. Use this page for the local
smoke checks that still need a human, before or after a train run.

## Automated gate

A real release requires the reusable CI workflow with `full: true` on
the exact SHA, then `release-plz` on that same revision. CI covers
`just fmt`, `just lint`, `just check-scripts`, `just check-release-tools`,
`cargo test --locked --no-fail-fast` on Linux and macOS, and
`just package-check` (fail above 9 MiB, warn above 8 MiB). Preview the
next version locally with `just release-preview`. See
[bob-cli CI](ci.md) and
[Release train reference](releases/reference.md).

## Local package and docs confirmation

From a clean worktree, you can re-run the same package and suite gates
the train already requires:

```bash
just check
just check-scripts
just package-list
just package-check
```

`just check` is the canonical verification gate: `cargo fmt --check`,
`cargo clippy --all-targets --all-features` with rustc unused and
private-interface warnings promoted to errors for bob-cli's targets, then
`cargo test --no-fail-fast`, which runs the lib, every `tests/*` binary,
and doctests even when one binary fails. `just test` runs the same
unmasked test step on its own. `just all` runs the same three checks
through separate recipes.

For documentation changes, run the documentation gate and the Rust docs
build. The train does not run `just docs-check`; the docs publishing
pipeline owns that gate.

```bash
just docs-check
cargo doc --locked --no-deps
cargo test --locked --doc
git diff --check
```

`just docs-check` is the Markdown and site gate: it assembles source
context, then checks the docs contract, generated-file drift, a strict
Zensical build, published URLs, offline links, search smoke cases, the
executable tutorial, CLI media determinism, and the documentation package
tests. See [Build the docs](build.md).

## Install and vault smoke tests

Run a local install smoke test:

```bash
just install-smoke
```

Run a tmux status smoke test after installing locally:

```bash
tmux display-message -p '#(bob pomodoro tmux)'
```

Before running `bob vault-sync` in a release smoke test, verify that
`BOB_DIR` points at the intended vault and that its Git remote can be
pushed without prompts. Before running `bob task archive` against the
real vault, verify that `~/bob` is the intended vault, inspect
`git -C ~/bob status --short`, and review any local edits that may be
included when touched candidate files are rewritten.

The default `bob query` smoke tests are local and headless. Before
running live Obsidian smoke tests, start desktop Obsidian, open the
target vault, enable Dataview, and use the explicit `--engine obsidian`
examples in [bob query](../dataview.md).

For an end-to-end collection smoke test, install the local binary, run
`bob task archive` against `~/bob`, then verify that archive notes under
`~/bob/done` include `parent: "[[source]]"` for the original note and
`type: "[[done]]"`, source notes include matching `done_tasks` links and
no longer contain the collected blocks, Obsidian links to moved
`^block-id` task blocks point at `done/..._done#^block-id`, and the vault
Git commit was pushed.

## Related

- [Releases](releases/index.md)
- [bob-cli CI](ci.md)
- [Build the docs](build.md)
- [Migration notes](migration-notes.md)
- [Install Bob](../start/install.md)
