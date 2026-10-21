---
title: "Install a specific Bob release"
description: "Install Bob from a Git tag or a GitHub Release archive and confirm the version."
tags:
  - CLI
---

# Install a specific Bob release

This guide installs a named Bob version, either by compiling the Git tag or
by unpacking a GitHub Release archive. For the latest checkout instead, use
[Install Bob](../start/install.md).

## Before you start

Pick a `vX.Y.Z` tag from
[GitHub Releases](https://github.com/bobs-org/bob-cli/releases) or
`CHANGELOG.md`. You need a directory on `PATH` that you can write.

From source you also need a Rust toolchain at 1.95 or later and `cargo`.
See [Requirements and compatibility](../reference/compatibility.md).
Prebuilt archives cover Linux x86_64 and macOS arm64 only.

The Cargo package is `bob-obsidian`. `cargo install bob-cli` installs a
different project.

## 1. Install Bob at that tag

Replace `vX.Y.Z` with the tag you chose. Use one of the two commands
below.

From source:

```bash
cargo install --git https://github.com/bobs-org/bob-cli.git --tag vX.Y.Z --locked bob-obsidian
```

Cargo writes `bob` and the companion binaries into
`${CARGO_INSTALL_ROOT:-${CARGO_HOME:-~/.cargo}}/bin`. Do not pass
`--force`.

On Linux x86_64 or macOS arm64, a GitHub Release archive skips the
compile. Download the matching file:

| Platform | Archive |
| --- | --- |
| Linux x86_64 | `bob-x86_64-unknown-linux-gnu.tar.gz` |
| macOS arm64 | `bob-aarch64-apple-darwin.tar.gz` |

```bash
curl -fsSL -o bob-x86_64-unknown-linux-gnu.tar.gz \
  https://github.com/bobs-org/bob-cli/releases/download/vX.Y.Z/bob-x86_64-unknown-linux-gnu.tar.gz
mkdir -p "$HOME/.local/bin"
tar -xzf bob-x86_64-unknown-linux-gnu.tar.gz -C "$HOME/.local/bin"
```

On Apple silicon, use `bob-aarch64-apple-darwin.tar.gz` and the same
`tar` layout. The archive contains `bob`, `bob_pomodoro`, `bob_notify`,
and `tmux_bob_pomodoro`. Put that directory on `PATH`.

## 2. Install shell completion

```bash
bob completion install
```

`bob completion install` never edits rc files. Inspect the state with
`bob completion` (same as `bob completion status`).

## 3. Confirm the version

```bash
bob --version
```

```text { .no-copy title="Output" }
bob 0.2.0
```

The printed number should match the tag you installed (`v0.2.0` prints
`bob 0.2.0`). The check passes when the command prints a version and
exits 0.

## If it goes wrong

If cargo reports `binary bob already exists … as part of bob-cli`, run
`cargo install --list`. For a path or Git `bob-cli` entry:

```bash
cargo uninstall bob-cli
```

Then retry the install command. For a registry `bob-cli` entry, that
package is a different project; do not uninstall it. Install Bob with
`--root` pointing at a directory you control, or unpack the prebuilt
archive there, and put that `bin` directory on `PATH` ahead of the other
`bob`. Do not pass `--force`.

## Related

- [Install Bob](../start/install.md)
- [Install everything with one command](install-everything.md)
- [Releases and versions](../reference/releases.md)
- [Requirements and compatibility](../reference/compatibility.md)
- [Shell completion](../completion.md)
