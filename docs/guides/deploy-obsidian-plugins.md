---
title: "Deploy the Obsidian plugins"
description: "List, preview, and sync Bob plugins from a source checkout into a vault."
tags:
  - CLI
  - Obsidian
---

# Deploy the Obsidian plugins

This guide deploys Bryan's custom Bob plugins from a `bob-plugins` checkout
into a vault, then reloads Obsidian so the desktop review surface matches the
checkout.

## Before you start

Install the CLI and choose a vault with `-b` or `BOB_DIR`. Clone
[bob-plugins](https://github.com/bobs-org/bob-plugins) and keep that checkout as
the source of truth. Do not edit copies under the vault's `.obsidian/plugins/`
folder; the next sync overwrites them. Sync copies plugin files. It does not
enable plugins or create dashboard notes.

## 1. List repo versions against the vault

```bash
bob plugins list --repo /home/alex/src/bob-plugins -b /path/to/vault
```

```text { .no-copy title="Output" }
Bob Plugins · 7 · /home/alex/src/bob-plugins
PLUGIN                    VERSION  SYNC     VAULT          DESCRIPTION
bob-ledger-tools          1.0.0    missing  not installed  …
bob-navigation-hotkeys    2.46.1   missing  not installed  …
7 not installed
```

Confirm each plugin row and the footer counts. `list` exits 0 when plugins are
missing or drifting; those are reportable states.

## 2. Preview the sync

```bash
bob plugins sync --repo /home/alex/src/bob-plugins -b /path/to/vault --dry-run
```

The dry run prints the same diffs and skip decisions as a real sync, with
`would copy` wording, and writes nothing. Pass `--repo` whenever you are
inside a different bob-plugins checkout; a bare sync from a foreign checkout
refuses instead of deploying the wrong tree.

## 3. Deploy and reload

```bash
bob plugins sync --repo /home/alex/src/bob-plugins -b /path/to/vault
```

Confirm `copied` or `up to date` for each managed file. Enable **Bob Ledger
Tools** and **Bob Navigation Hotkeys** in Obsidian's Community plugins
settings, then reload Obsidian. Open a task note and confirm the task card
renders.

## 4. Run the rollout smoke gate

Plugin rollouts finish with the packaged Obsidian startup smoke check from
the `bob-plugins` checkout. It launches an isolated scratch vault and profile
under Xvfb, never the live vault. The fixture verifies the managed Bob plugin
build, not the user's full third-party plugin/settings combination.

```bash
npm run smoke:obsidian -- --self-test
xvfb-run -a npm run smoke:obsidian -- --report /tmp/obsidian-smoke.json
npm run smoke:obsidian -- --deployed-vault /path/to/vault --report /tmp/obsidian-smoke.json
```

The first command is an offline check only, never an Obsidian pass. The
mandatory post-sync command verifies every expected repository plugin is
present in the deployed vault at the expected version with `sync: synced`;
`bob plugins list` exits 0 on drift, so the gate reads the JSON rows itself.
Fixed defaults apply: layout-ready within 30 seconds, sampled peak JS heap
below 1.5 GiB, the fixture relation indexed with zero non-Markdown reads, all
four editor mounts (daily plus ordinary note, Live Preview plus Source), and
no renderer crashes, plugin-load failures, or uncaught exceptions. On a
skipped, unrun, or failing check, do not claim rollout success.

## If it goes wrong

Uncommitted vault edits are skipped with `skipped <file> (dirty in vault; use
-F/--force)`. Read the printed diff, commit or discard the vault change, and
rerun the sync. Do not make `--force` a habit. A refused foreign checkout
exits 2 and prints the `--repo` command to run instead. Sync also refuses
another checkout's uncommitted or non-ancestor deploy (exit 1) and prints
preview and force recovery commands. In a Git vault, ignore
`.obsidian/plugins/*/.bob-deploy.json` so the provenance stamp stays out of
version control. When a plugin's whole vault folder (`.obsidian/plugins/<id>/`)
is not ignored or holds tracked files, sync and list print a `warning` line
and set `vault_ignore` in JSON without blocking the copy: add the folder line
to the vault `.gitignore` yourself — bob never edits it or untracks files.

## Related

- [Bob Plugins](../plugins.md)
- [bob plugins](../reference/cli/plugins.md)
- [Obsidian plugins](../reference/plugins/index.md)
- [Continue in Obsidian](../start/obsidian.md)
