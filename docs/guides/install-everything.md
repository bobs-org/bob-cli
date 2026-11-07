---
title: "Install everything with one command"
description: "Run just install-all from a bob-cli checkout and read each component result."
tags:
  - CLI
  - Obsidian
  - macOS
---

# Install everything with one command

This guide installs Bob from a source checkout, deploys the Obsidian plugins when a
vault is present, and on macOS installs Mac Capture from a sibling checkout. The
installer pulls all available checkouts first, shows the commits since their last
successful install, then runs each build with a live progress line.

## Before you start

You need a `bob-cli` Git checkout, a current stable Rust toolchain that supports
edition 2024, `just`, and `git`. Keep sibling checkouts next to it when you want
plugins or Mac Capture:

```text
/home/alex/src/bobs-org/
├── bob-cli/
├── bob-plugins/
└── bob-mac-capture/
```

A vault at `BOB_DIR` or `~/bob` is required before the plugin step runs. See
[Requirements and compatibility](../reference/compatibility.md) for platforms
and floors. To install only the CLI, use [Install Bob](../start/install.md)
instead.

## 1. Run the installer from bob-cli

```bash
cd /home/alex/src/bobs-org/bob-cli
just install-all
```

The recipe pulls this checkout with `--ff-only`, then pulls available sibling
checkouts before it starts any build. It shows a compact “What’s new” digest for
commits since each checkout's last successful install. It then installs `bob`,
deploys `../bob-plugins` with `bob plugins sync --no-pull --repo`, and on macOS
installs `../bob-mac-capture`. During a build, one live line reports the current
activity and elapsed time. An ETA appears only after this host has enough
comparable build history. Cargo output is saved to a log; if a build fails, the
installer prints a short excerpt and the log path. Confirm each named row in the
summary: `bob-cli`, `bob-plugins`, `bob-mac-capture`, and `obsidian`.

```text { .no-copy title="Output while the CLI build runs" }
  ⇣  bob-cli           ✓  3 new commits · b6a96243 → 8935148f
  ⇣  bob-plugins       ✓  up to date · 5fbe82a
  ⇣  bob-mac-capture   ✓  1 new commit · a2aab30 → 11dcba7

📰  WHAT'S NEW
────────────────────────────────────────────────
  bob-cli · 3 commits since your last install 51m ago
  ⚠ feat!  capture  glue start and past timing to Pomodoro names         79709fed
                    breaking · the commit has no migration note
    feat   gmail    add durable send-recovery engine for compose drafts  8935148f
                    Implements versioned frontmatter, locks, and recovery.
                    Verified: cargo fmt --check clean, workspace clippy gate…
    fix    capture  repair successor verdict landing behavior            86b2501b

  ⏱ usually 9–12 min on this Mac · done by about 14:16

🦀  BOB-CLI  1/3
────────────────────────────────────────────────
  $ just --justfile /home/alex/src/bobs-org/bob-cli/justfile install
  ⠼ compiling bob-obsidian  ━━━━━━━━━━━━━━━╸────────  4:52 · about 2–4 min left
  ✓ built · 8m 12s · about usual
  ✓ 3 new commits · b6a96243 → 8935148f · installed @ 8935148f

✨  SUMMARY
────────────────────────────────────────────────
  ✓  bob-cli           3 new commits · installed @ 8935148f           8m 12s
  ✓  bob-plugins       up to date · 3 files copied · 9/9 in sync         3s
  ✓  bob-mac-capture   1 new commit · installed ~/Applications/…      1m 41s
  –  obsidian          plugins unchanged · restart not needed

  ✓  ALL INSTALLED · 10m 05s · about usual (9–12 min)
```

The digest shows commits since each checkout's last successful install on this
host. Before a repo has a watermark, it uses that run's pull range. In a
terminal, long `Verified:` paragraphs fold to one row. Without a terminal, the
digest prints plain commit subjects.

The estimate learns from this host's comparable runs. The first timed build for a
kind shows `first timed build on <host>`. Once there is a similar run, but fewer
than five consistent samples, it shows that run's total as `last similar build
7m 50s`. A range and progress bar appear after five consistent runs with the same
host, build kind, toolchain, profile, and incremental setting. For example,
`about 2–4 min left` is the estimated remaining range. If elapsed time passes the
usual high end, the line says `taking longer than usual (8–10 min)`. A change to
`Cargo.lock`, a `Cargo.toml`, or the toolchain file shows `Cargo.lock changed,
expect a longer build`. After a ranged build, `faster than usual`, `about usual`,
or `slower than usual` compares its duration with that range.

Glyphs mark `ok`, `warn`, `fail`, `skip`, or a restart. A failed row makes the
command exit 1. A terminal bell rings after a run of at least 60 seconds. On
macOS, a completion banner titled with the final status appears after at least 120
seconds. Neither cue appears after Ctrl-C or when `BOB_INSTALL_NOTIFY=0` is set.
Ctrl-C prints an interrupted row, exits 130, and skips the remaining steps.

Pass `-v` to stream compiler and plugin-sync output instead of keeping that
output in the step log:

```bash
just install-all -v
```

For `just install`, set `BOB_INSTALL_VERBOSE=1` to stream compiler output. This
also disables the live status line. Without a terminal, or when the helper is
unavailable, the install continues with ordinary command output.

These environment variables control the optional display and its local state:

| Variable | Effect |
| --- | --- |
| `BOB_INSTALL_UI=0` | Disable the helper UI and digest. |
| `BOB_INSTALL_UI_PYTHON=/path` | Use this Python interpreter when it is version 3.9 or newer. |
| `BOB_INSTALL_VERBOSE=1` | Stream raw build and sync output; disable live build rows. |
| `BOB_INSTALL_NOTIFY=0` | Suppress the completion bell and macOS banner. |
| `BOB_INSTALL_STATE_DIR=/path` | Store install-all state in this directory. |

By default, state lives in
`${XDG_STATE_HOME:-~/.local/state}/bob-cli/install-all/`. It contains
`installed.json` (successful-install watermarks), `runs.jsonl` (build timings),
`logs/<run>-<step>.log` (raw output, keeping the newest 10 logs per step),
`runs/<run>/` (per-run digest and scratch files), `crash.log` (helper exceptions),
and `obsidian-restart-pending` (a plugin update awaiting restart).

## 2. Read skips and warnings

Missing siblings are skipped unless the run is interactive and you accept a
clone into that sibling path. `bob-plugins` is offered only when the Obsidian
vault exists; `bob-mac-capture` only on macOS. A non-interactive run, a
declined clone, and a missing vault skip those steps. Pull failures warn and
keep the local checkout.

On macOS, `just install-all` passes no signing identity. Mac Capture then uses
the stable local identity when the login keychain has it. Signing details live
in [README › Signing](https://github.com/bobs-org/bob-mac-capture#signing)
and [Install › Signing](../reference/mac-capture/install.md#signing).

## 3. Confirm the CLI answers

```bash
bob --version
```

```text { .no-copy title="Output" }
bob 0.2.0
```

The printed version changes over time. The check passes when the command prints
a version and exits 0.

## Restart Obsidian

After a clean run, `just install-all` restarts a running Obsidian only when
`bob plugins sync` copied at least one file. It previews the sync with
`bob plugins sync --dry-run --format json`. On macOS it quits and relaunches
Obsidian; the first run may ask for Automation permission. Before the quit,
it posts a Notification Center banner titled `Restarting Obsidian` with
subtitle `bob install-all`. The banner titled `Obsidian needs a restart`
posts instead when a failed step or a failed quit/relaunch leaves the new
plugins unloaded. macOS files the banner under Script Editor. A denied
banner does not change the restart. On Linux it uses the Obsidian CLI
`restart` when that CLI is registered at `~/.local/bin/obsidian`. A stopped
Obsidian is never launched. The pending marker lives under
`~/.local/state/bob-cli/install-all/`.

## If it goes wrong

A failed clone, a failed `just install`, or a failed plugin sync exits 1 and
leaves earlier successful rows in place. For a build failure, inspect the log
path printed below the error excerpt; rerun with `just install-all -v` to see
raw build and plugin-sync output in the terminal. Rerun `just install-all` after
fixing the reported step. If plugins copied into the vault but Obsidian did not
reload, the next run retries that restart. Do not pass `--force` to the Cargo
install.

## Related

- [Install Bob](../start/install.md)
- [Install a specific Bob release](install-a-tagged-release.md)
- [Deploy the Obsidian plugins](deploy-obsidian-plugins.md)
- [Continue with Mac Capture](../start/mac-capture.md)
- [Requirements and compatibility](../reference/compatibility.md)
