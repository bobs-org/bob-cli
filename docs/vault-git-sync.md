# Bob vault Git sync runbook

This runbook covers the Bob vault's git-only sync channel between athena, apollo, and
the MacBook. Obsidian Sync is no longer the automation path for `~/bob`.

## Sync cycle

`bob vault-sync` runs one lock-protected reconcile cycle against `BOB_DIR` (`~/bob` by
default):

1. Recover an interrupted merge, rebase, or cherry-pick before doing new work.
2. Read `git status --porcelain`; only stage when there are local changes.
3. Refuse any file at or above 95 MiB before staging, and warn at or above 50 MiB.
4. Commit staged local changes with a generated `vault(<host>): ...` message, unless
   `--message` supplies one.
5. Check `origin/master`, fetch only when needed, fast-forward or merge, and
   auto-resolve supported conflicts into `_conflicts/`.
6. Push, retrying bounded non-fast-forward races.
7. Write the status record used by `bob vault-sync status`.

The command shares the `bob_sync.lock` maintenance lock with `bob nightly` and live
`bob task-status-hooks` runs, so background sync, nightly maintenance, and task-status
writes do not mutate the vault concurrently.

## Conflict policy

Remote content wins in-place during supported conflicts. The local version is kept as a
conflict copy under `_conflicts/`, and `_conflicts/sync_conflicts.md` records the event.
The conflict directory is excluded from Bob's vault walkers, the Tasks global query, and
Dataview's excluded folders, so quarantined copies do not appear in task dashboards.

After a conflict, verify:

```bash
git -C ~/bob status --short
rg -n '<<<<<<<|=======|>>>>>>>' ~/bob
bob vault-sync status --json
```

Unhandled conflicts leave the merge aborted and the status record's `last_error` set. Do
not use `reset --hard`, force-push, or `-X ours/theirs` to clear the vault.

## Credentials

athena and apollo each use a repository-scoped read-write deploy key at
`~/.ssh/id_bob_vault` (`bob-vault-sync@athena` and `bob-vault-sync@apollo`) and point
the vault remote at the `github-bob` host alias:

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

The MacBook uses its normal `github.com` identity with the same ControlMaster settings.
Verify unattended access without a warm shell environment:

```bash
env -i HOME=/home/bryan PATH=/usr/bin:/bin git -C ~/bob ls-remote origin master
ssh apollo 'env -i HOME=/home/bryan PATH=/usr/bin:/bin git -C ~/bob ls-remote origin master'
ssh mac 'env -i HOME=/Users/bbugyi PATH=/usr/bin:/bin:/usr/local/bin git -C ~/bob ls-remote origin master'
```

## Background triggers

athena and apollo each run the same user systemd service. On athena:

```bash
systemctl --user status bob-vault-sync.service
systemctl --user enable --now bob-vault-sync.service
systemctl --user disable --now bob-vault-sync.service
journalctl --user -u bob-vault-sync.service -n 80
```

On apollo, prefix the same commands with `ssh apollo`:

```bash
ssh apollo 'systemctl --user status bob-vault-sync.service'
ssh apollo 'systemctl --user enable --now bob-vault-sync.service'
ssh apollo 'systemctl --user disable --now bob-vault-sync.service'
ssh apollo 'journalctl --user -u bob-vault-sync.service -n 80'
```

The unit executes `~/bin/bob_vault_sync_watch`, which waits on inotify with a 15-second
timeout, debounces briefly, then runs `bob vault-sync -q`. That inotify wait requires
`inotify-tools`. When `inotifywait` is missing, the watch script silences the error and
the loop degrades to a roughly 5-second poll that still runs `bob vault-sync -q` each
pass.

The MacBook runs the LaunchAgent at
`~/Library/LaunchAgents/com.bbugyi.bob-vault-sync.plist`:

```bash
ssh mac 'launchctl print gui/$(id -u)/com.bbugyi.bob-vault-sync'
ssh mac 'launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.bbugyi.bob-vault-sync.plist'
ssh mac 'launchctl bootout gui/$(id -u)/com.bbugyi.bob-vault-sync'
ssh mac 'tail -n 80 /var/tmp/com.bbugyi.bob-vault-sync.err'
```

The LaunchAgent runs `bob vault-sync -q` every 15 seconds and at load.

## Mac scheduled maintenance

The MacBook also runs three independent 15-minute cron jobs, staggered five minutes
apart so their writes do not collide with each other or with the 15-second
`bob-vault-sync` LaunchAgent:

```cron
0,15,30,45 * * * * ~/bin/maybe_bob_highlights_sync -w >> /var/tmp/maybe_bob_highlights_sync.log 2>&1
5,20,35,50 * * * * ~/.cargo/bin/bob projects sync >> /var/tmp/bob_projects.log 2>&1
10,25,40,55 * * * * ~/.cargo/bin/bob task-status-hooks --retry-timeout 120 >> /var/tmp/bob_task_status_hooks.log
```

Highlights intake precedes project reconciliation, which precedes task-status
reconciliation. The 2-minute `--retry-timeout` on `task-status-hooks` fits comfortably
inside the 15-minute cadence and absorbs transient maintenance-lock contention and
concurrent-save races against the 15-second sync LaunchAgent; see
[Retries](task-status-hooks.md#retries) for the backoff policy and its allowed failure
reasons. Offsetting the three jobs by minutes reduces collisions between them, but
cannot guarantee exclusion from an open editor, a slow job, or the sync LaunchAgent —
correctness comes from the existing lock and guarded-write checks plus the fresh-attempt
retry behavior, not from the schedule alone.

This crontab is not chezmoi-managed; it is installed by hand with `crontab` directly on
the Mac. The highlights and projects jobs redirect both stdout and stderr with
`>> logfile 2>&1` (in that order). The `task-status-hooks` job redirects stdout only:
routine retry progress and the final human result land in
`/var/tmp/bob_task_status_hooks.log`, while warnings and real terminal failures remain
on stderr so cron mail is still actionable.

```bash
ssh mac crontab -l
ssh mac 'tail -n 80 /var/tmp/bob_task_status_hooks.log'
ssh mac '~/.cargo/bin/bob task-status-hooks --help' # confirm --retry-timeout is listed before installing a crontab that uses it
```

To change this schedule: re-read the live crontab, save a timestamped backup outside the
vault, and replace only the three matching Bob entries, leaving unrelated lines
untouched:

```bash
ssh mac 'crontab -l' > /tmp/mac-crontab-backup-$(date +%Y%m%dT%H%M%S).txt
# edit the three lines in a local copy, then:
ssh mac 'crontab -' < /tmp/mac-crontab-new.txt
ssh mac 'crontab -l' # confirm exactly one entry per job
```

Roll back by reinstalling the saved backup the same way, preserving any intervening
unrelated edits.

## Nightly maintenance

athena's cron entry runs `bob nightly` at 03:30. `bob nightly` now runs:

1. `bob vault-sync`
2. `bob move-done-tasks`
3. `bob vault-sync`

That ordering pulls the MacBook's latest notes before maintenance rewrites task blocks
and pushes the maintenance commit afterwards.

## Highlights bridge

`lit_review/` and `xlib/` are gitignored. `lit_review/` is out-of-band storage: copy it
explicitly when a second machine needs the PDFs.

`xlib/` is the Highlights intake bridge. athena and apollo each keep a gitignored
`~/bob/xlib/` source queue, and `bob_xlib_pull` drains both into the MacBook before
Highlights scanning starts. The managed Bob config sets:

```yaml
highlights:
  pre_scan_command: PATH="$HOME/bin:$PATH" bob_xlib_pull
```

On the MacBook, the 15-minute `~/bin/maybe_bob_highlights_sync -w` cron job runs
`bob highlights scan`. The pre-scan command probes athena and apollo in parallel.
Missing or empty queues skip rsync entirely, while nonempty queues are pulled one host
at a time into the MacBook's `~/bob/xlib/` with
`rsync --remove-source-files --ignore-existing`. The script reuses a private per-run SSH
control socket for each host's probe, transfer, and best-effort empty directory cleanup.

Destination files win collisions. If athena, apollo, or the local intake already has the
same relative path, the later source keeps its copy instead of overwriting the
destination; a later scan can handle the retained duplicate after the local intake
clears. Offline hosts and empty queues are no-work successes, so the scan still proceeds
with whatever is already local. Genuine traversal, local setup, or transfer failures are
reported after both hosts are accounted for and make the pre-scan hook fail, which stops
that `bob highlights scan` run before it consumes intake. Checking both hosts means a
fast reachable source can still wait for the other host's SSH timeout when that other
host is unavailable.

Useful checks:

```bash
bob highlights doctor
ssh mac 'PATH="$HOME/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/bin:/bin" bob highlights doctor'
```

## Custom plugins

The custom `bob-*` Obsidian plugins stay gitignored in the vault. The MacBook's source
checkout lives at `~/projects/github/bobs-org/bob-plugins`; refresh the vault copy
manually when needed:

```bash
ssh mac 'bob plugins list'
ssh mac 'bob plugins sync'
```

## Status checks

`bob vault-sync status` prints the last run in a human format. Use JSON for automation:

```bash
bob vault-sync status --json
ssh mac 'bob vault-sync status --json'
```

The record includes attempt and success timestamps, local and remote SHAs, files
committed, push retries, duration, conflict-copy paths, interrupted-merge recovery, and
the last error.

## Rollback

Rollback to git, not Obsidian Sync. Tag the pre-cutover commit before risky changes and
use the filesystem backups from the migration if the vault must be restored. Obsidian
Sync was over quota when this channel replaced it, so it is not a reliable rollback
target.

Keep the disabled `ob-sync-bob.service` and `ob-sync-bob-poll` files through the soak.
Unlinking Obsidian Sync, logging out of the Sync account, removing the old service
files, and canceling the subscription are user-timed cleanup steps after a clean soak.
