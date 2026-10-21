---
title: Releases and versions
description: Where Bob versions appear, how 0.x numbering works, and how to check the CLI you have installed.
---

# Releases and versions

This page records how Bob versions are published and how they relate to the
`bob` binary, Mac Capture, and the Obsidian plugins. Install a named version
with [Install a specific Bob release](../guides/install-a-tagged-release.md).

## Where releases appear

| Surface | What it shows |
| --- | --- |
| [GitHub Releases](https://github.com/bobs-org/bob-cli/releases) | Version notes and, when the train ships them, prebuilt archives |
| Git tags | `vX.Y.Z` on [bobs-org/bob-cli](https://github.com/bobs-org/bob-cli) |
| `CHANGELOG.md` | Keep a Changelog sections in the bob-cli repository |
| crates.io | The `bob-obsidian` package is not yet published. Install from Git or a GitHub Release |

The Cargo package name is `bob-obsidian`. The installed binaries keep their
names: `bob`, `bob_pomodoro`, `bob_notify`, and `tmux_bob_pomodoro`.
`cargo install bob-cli` installs an unrelated project.

## Cadence

The release train ships at most one version per America/New_York calendar
day, and only when a user-visible change landed since the last tag.
`feat`, `fix`, `perf`, and `revert` commits count. `docs`, `chore`,
`refactor`, `test`, `ci`, `style`, and `build` commits do not ship a
version on their own. A day with no qualifying commit produces no release.

The train is in rehearsal: it creates tags and GitHub Releases. It does
not publish to crates.io yet.

## Version numbers

Bob is on 0.x. On that series:

| Change since the last release | Version |
| --- | --- |
| `feat`, `fix`, `perf`, or `revert` | Patch, for example 0.2.0 → 0.2.1 |
| A Conventional Commit with `!` | Minor, for example 0.2.1 → 0.3.0 |
| `docs`, `chore`, `refactor`, `test`, `ci`, `style`, or `build` only | No release |

A `!` marker is the product contract break for CLI flags, CLI output, and
JSON consumers. The Rust library in this repository is an internal seam,
not a public API. CLI and JSON fields grow additively until a `!` commit
says otherwise.

## Check your version

```bash
bob --version
```

```text { .no-copy title="Output" }
bob 0.2.0
```

The printed number is the version compiled into that binary. Compare it
with the newest `vX.Y.Z` tag on GitHub Releases. A Git checkout of `master`
can be newer than the latest release by up to one day.

## Companion tools

Mac Capture and the Obsidian plugins track bob-cli `HEAD`. They call `bob`
and consume its JSON; they do not parse capture grammar themselves. The JSON
contract grows additively, so a companion built against an older schema
keeps working when new fields appear. A `!` commit is the signal that a
CLI or JSON consumer must change.

Pinning `bob` to a GitHub Release while the plugins and Mac Capture follow
`HEAD` is the usual split. Hosts that track `HEAD` for every component stay
on the same commit and can sit ahead of the latest tag.

## Prebuilt archives

Each GitHub Release can attach archives for:

| Platform | Target | Archive |
| --- | --- | --- |
| Linux x86_64 | `x86_64-unknown-linux-gnu` | `bob-x86_64-unknown-linux-gnu.tar.gz` |
| macOS arm64 | `aarch64-apple-darwin` | `bob-aarch64-apple-darwin.tar.gz` |

Each archive contains `bob`, `bob_pomodoro`, `bob_notify`, and
`tmux_bob_pomodoro`. The package also declares cargo-binstall metadata for
those URLs; `cargo binstall bob-obsidian` resolves them after the crate is
on crates.io. Until then, download the archive from the GitHub Release or
install from Git. See
[Install a specific Bob release](../guides/install-a-tagged-release.md).

## Related

- [Install a specific Bob release](../guides/install-a-tagged-release.md)
- [Install Bob](../start/install.md)
- [JSON outputs](json-outputs.md)
- [Requirements and compatibility](compatibility.md)
- [How the release train works](../develop/releases/how-the-release-train-works.md)
- [Release train reference](../develop/releases/reference.md)
