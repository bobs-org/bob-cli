# Bob vault Git sync runbook

This runbook covers the Bob vault's git-only sync channel between athena and
the MacBook. Obsidian Sync is no longer the automation path for `~/bob`.

## Sync cycle

`bob vault-sync` runs one lock-protected reconcile cycle against `BOB_DIR`
(`~/bob` by default):

1. Recover an interrupted merge, rebase, or cherry-pick before doing new work.
2. Read `git status --porcelain`; only stage when there are local changes.
3. Refuse any file at or above 95 MiB before staging, and warn at or above 50 MiB.
4. Commit staged local changes with a generated `vault(<host>): ...` message,
   unless `--message` supplies one.
5. Check `origin/master`, fetch only when needed, fast-forward or merge, and
   auto-resolve supported conflicts into `_conflicts/`.
6. Push, retrying bounded non-fast-forward races.
7. Write the status record used by `bob vault-sync status`.

The command shares the `bob_sync.lock` maintenance lock with `bob nightly`, so
background sync and nightly maintenance do not mutate the vault concurrently.

## Conflict policy

Remote content wins in-place during supported conflicts. The local version is
kept as a conflict copy under `_conflicts/`, and `_conflicts/sync_conflicts.md`
records the event. The conflict directory is excluded from Bob's vault walkers,
the Tasks global query, and Dataview's excluded folders, so quarantined copies
do not appear in task dashboards.

After a conflict, verify:

```bash
git -C ~/bob status --short
rg -n '<<<<<<<|=======|>>>>>>>' ~/bob
bob vault-sync status --json
```

Unhandled conflicts leave the merge aborted and the status record's
`last_error` set. Do not use `reset --hard`, force-push, or `-X ours/theirs`
to clear the vault.

## Credentials

athena uses a repository-scoped deploy key at `~/.ssh/id_bob_vault` and the
vault remote points at the `github-bob` host alias:

```sshconfig
Host github-bob
  HostName ssh.github.com
  Port 443
  User git
  IdentityFile ~/.ssh/id_bob_vault
  IdentitiesOnly yes
  AddKeysToAgent no
  ControlMaster auto
  ControlPath ~/.ssh/cm-%r@%h:%p
  ControlPersist 10m
```

The MacBook uses its normal `github.com` identity with the same ControlMaster
settings. Verify unattended access without a warm shell environment:

```bash
env -i HOME=/home/bryan PATH=/usr/bin:/bin git -C ~/bob ls-remote origin master
ssh mac 'env -i HOME=/Users/bbugyi PATH=/usr/bin:/bin:/usr/local/bin git -C ~/bob ls-remote origin master'
```

## Background triggers

athena runs a user systemd service:

```bash
systemctl --user status bob-vault-sync.service
systemctl --user enable --now bob-vault-sync.service
systemctl --user disable --now bob-vault-sync.service
journalctl --user -u bob-vault-sync.service -n 80
```

The unit executes `~/bin/bob_vault_sync_watch`, which waits on inotify with a
15-second timeout, debounces briefly, then runs `bob vault-sync -q`.

The MacBook runs the LaunchAgent at
`~/Library/LaunchAgents/com.bbugyi.bob-vault-sync.plist`:

```bash
ssh mac 'launchctl print gui/$(id -u)/com.bbugyi.bob-vault-sync'
ssh mac 'launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.bbugyi.bob-vault-sync.plist'
ssh mac 'launchctl bootout gui/$(id -u)/com.bbugyi.bob-vault-sync'
ssh mac 'tail -n 80 /var/tmp/com.bbugyi.bob-vault-sync.err'
```

The LaunchAgent runs `bob vault-sync -q` every 15 seconds and at load.

## Nightly maintenance

athena's cron entry runs `bob nightly` at 03:30. `bob nightly` now runs:

1. `bob vault-sync`
2. `bob move-done-tasks`
3. `bob vault-sync`

That ordering pulls the MacBook's latest notes before maintenance rewrites task
blocks and pushes the maintenance commit afterwards.

## Highlights bridge

`lit_review/` and `xlib/` are gitignored. `lit_review/` is out-of-band storage:
copy it explicitly when a second machine needs the PDFs.

`xlib/` is the Highlights intake bridge. The managed Bob config sets:

```yaml
highlights:
  pre_scan_command: PATH="$HOME/bin:$PATH" bob_xlib_pull
```

On the MacBook, the 15-minute `~/bin/maybe_bob_highlights_sync -w` cron job runs
`bob highlights scan`. The pre-scan command pulls `home:bob/xlib/` into the
MacBook's `~/bob/xlib/` with `rsync --remove-source-files`, removes empty source
directories on athena, and lets `bob highlights scan` move the PDFs into `lib/`
and write the matching `ref/` notes.

Useful checks:

```bash
bob highlights doctor
ssh mac 'PATH="$HOME/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/bin:/bin" bob highlights doctor'
```

## Custom plugins

The custom `bob-*` Obsidian plugins stay gitignored in the vault. The MacBook's
source checkout lives at `~/projects/github/bobs-org/bob-plugins`; refresh the
vault copy manually when needed:

```bash
ssh mac 'bob plugins list'
ssh mac 'bob plugins sync'
```

## Status checks

`bob vault-sync status` prints the last run in a human format. Use JSON for
automation:

```bash
bob vault-sync status --json
ssh mac 'bob vault-sync status --json'
```

The record includes attempt and success timestamps, local and remote SHAs,
files committed, push retries, duration, conflict-copy paths, interrupted-merge
recovery, and the last error.

## Rollback

Rollback to git, not Obsidian Sync. Tag the pre-cutover commit before risky
changes and use the filesystem backups from the migration if the vault must be
restored. Obsidian Sync was over quota when this channel replaced it, so it is
not a reliable rollback target.

Keep the disabled `ob-sync-bob.service` and `ob-sync-bob-poll` files through
the soak. Unlinking Obsidian Sync, logging out of the Sync account, removing
the old service files, and canceling the subscription are user-timed cleanup
steps after a clean soak.
