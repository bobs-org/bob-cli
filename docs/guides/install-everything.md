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
vault is present, and on macOS installs Mac Capture from a sibling checkout.

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

The recipe pulls this checkout with `--ff-only`, installs `bob`, then pulls and
deploys `../bob-plugins` with `bob plugins sync --no-pull --repo`. On macOS it
also pulls and installs `../bob-mac-capture`. Confirm each named row in the
summary: `bob-cli`, `bob-plugins`, `bob-mac-capture`, and `obsidian`.

```text { .no-copy title="Output" }
  ✓  bob-cli           installed
  ✓  bob-plugins       synced
  -  bob-mac-capture   skipped (not macOS)
  -  obsidian          skipped (not running)
```

Glyphs mark `ok`, `warn`, `fail`, `skip`, or a restart. A failed row makes the
command exit 1.

## 2. Read skips and warnings

Missing siblings are skipped unless the run is interactive and you accept a
clone into that sibling path. `bob-plugins` is offered only when the Obsidian
vault exists; `bob-mac-capture` only on macOS. A non-interactive run, a
declined clone, and a missing vault skip those steps. Pull failures warn and
keep the local checkout.

On macOS, `just install-all` passes no signing identity. Mac Capture then uses
the stable local identity when the login keychain has it. Signing details live
in the
[Bob Mac Capture README › Signing](https://github.com/bobs-org/bob-mac-capture#signing).

## 3. Confirm the CLI answers

```bash
bob --version
```

```text { .no-copy title="Output" }
bob 0.1.0
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
leaves earlier successful rows in place. Rerun `just install-all` after fixing
the reported step. If plugins copied into the vault but Obsidian did not
reload, the next run retries that restart. Do not pass `--force` to the Cargo
install.

## Related

- [Install Bob](../start/install.md)
- [Deploy the Obsidian plugins](deploy-obsidian-plugins.md)
- [Continue with Mac Capture](../start/mac-capture.md)
- [Requirements and compatibility](../reference/compatibility.md)
