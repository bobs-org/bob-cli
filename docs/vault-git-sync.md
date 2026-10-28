# Bob vault Git sync runbook

This runbook covers the Bob vault's git-only sync channel between athena, apollo, and
the MacBook. Obsidian Sync is no longer the automation path for `~/bob`.

## Sync cycle

`bob vault-sync` runs one lock-protected reconcile cycle against `BOB_DIR` (`~/bob` by
default):

1. Recover an interrupted merge, rebase, or cherry-pick before doing new work, and
   complete a fast-forward whose index and worktree were updated before a Git lock
   prevented `HEAD` from moving.
2. Read `git status --porcelain`; only stage when there are local changes.
3. Detect displaced-plus-foreign-identity rewrites of tracked notes and quarantine
   them (see [Clobbered notes](#clobbered-notes)) before staging. Dry runs report
   `would quarantine` and write nothing.
4. Refuse any file at or above 95 MiB before staging, and warn at or above 50 MiB.
5. Commit staged local changes with a generated `vault(<host>): ...` message, unless
   `--message` supplies one.
6. Check `origin/master`, fetch only when needed, fast-forward or merge, retry a
   fast-forward blocked by a Git lock, and auto-resolve supported conflicts into
   `_conflicts/`.
7. Push, retrying bounded non-fast-forward races.
8. Write the status record used by `bob vault-sync status`.

The command shares the `bob_sync.lock` maintenance lock with `bob nightly`, live
`bob task reconcile` runs, `bob task reroll`, `bob task move`, writing `bob ref scan`, and
`bob ref rename`, so background sync, nightly maintenance, task-status writes,
reroll re-rolls, writing scans, and reference renames do not mutate the vault
concurrently. Writing `bob ref scan` takes `bob_sync.lock` then `scan.lock` and
holds both through intake and note writes; it never syncs. `bob ref rename`
already uses that same lock order. `bob task reroll` holds the maintenance lock
across a sync-sandwiched scoped commit: it runs a vault-sync cycle, writes only
the re-rolled notes, commits exactly those notes as one `bob task reroll`
commit, then runs a second vault-sync cycle. See
[randomize.md](randomize.md#git-and-vault-sync). `bob task move` follows the
same sync-sandwiched scoped-commit pattern while repairing references to moved
task blocks. Its dry run skips the lock and every Git operation; `--offline`
keeps the lock and local scoped commit but skips both sync cycles. See
[`task-move.md`](task-move.md).

Every Git command bob runs for vault work disables detached auto-maintenance with
`maintenance.autoDetach=false` and `gc.autoDetach=false`. Git's detached maintenance
child can hold `.git/HEAD.lock` while a later vault operation updates refs; running
maintenance in the foreground keeps those operations serialized.

Set the same options in each vault repository for Git commands that bypass bob. Run on
athena, apollo, and the MacBook:

```bash
git -C ~/bob config maintenance.autoDetach false
git -C ~/bob config gc.autoDetach false
```

## Conflict policy

Remote content wins in-place during supported conflicts. The local version is kept as a
conflict copy under `_conflicts/`, and `_conflicts/sync_conflicts.md` records the event.
A whole-note clobber uses the same conflict copy and log: the working-tree bytes are
quarantined, `HEAD` is restored in place, and the cycle continues so the restore, copy,
and log commit together. Conflict copies are force-added, including paths ignored by
`.gitignore`, so every logged copy is committed and reaches the other hosts.
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

## Halted sync

If vault-sync encounters the same interrupted merge after it already aborted and retried
that merge once, it stops before changing the vault again. `last_error` starts with
`halted:`, and `halted_merge_head` records the SHA that triggered the halt. Later cycles
keep halting on the same in-progress merge until it is resolved by hand.

Inspect the merge and fix its cause:

```bash
git -C ~/bob status
```

Then either finish the merge or abort it:

```bash
git -C ~/bob add <resolved-paths>
git -C ~/bob commit --no-edit
# or, if the merge should be discarded:
git -C ~/bob merge --abort
```

Never use `reset --hard` or force-push. A successful later vault-sync clears the breaker
fields.

## Clobbered notes

Vault-sync flags a tracked `.md` note when the working tree looks like another live note
was saved over it. Added, deleted, renamed, and untracked paths are skipped, as are
`_conflicts/`, `done/`, `_templates/`, dot-directories (`.bob/`, `.obsidian/`, `.trash/`),
and canonical daily notes (`YYYY/YYYYMMDD*.md`).

A note A is flagged only when both hold:

1. **Displaced.** `HEAD` has at least 5 distinct non-blank lines, and at least 50% of
   them are absent from the working tree.
2. **Foreign identity.** Some other live note B matches at least one of:
   - A's working-tree frontmatter `created:` differs from `HEAD` and equals B's
     `created:`.
   - A's working-tree `^prj` task line differs from `HEAD` and is identical (trimmed) to
     B's.
   - At least 3 task block IDs (excluding `prj`) on A's working-tree task lines are
     absent from `HEAD` and present on task lines in B.

Legitimate rewrites do not match: a task move leaves the IDs, deleting half a note has
no foreign identity, a new-note copy is added rather than modified, and a single
duplicated task is one ID.

On a match, vault-sync writes the working-tree bytes to a conflict copy under
`_conflicts/`, restores the `HEAD` blob into A, appends `_conflicts/sync_conflicts.md`
with the matched note and evidence, prints a red warning on stderr even under `-q`,
records `clobbers` for this cycle plus a capped `recent_clobbers` (last 10) on the
status record, and fires a best-effort desktop notification (`osascript` on macOS,
`notify-send` on Linux). Set `BOB_VAULT_SYNC_NOTIFY=0` to suppress the notification.
Dry runs print `would quarantine A (matches B: evidence)` and change nothing.

Look at `_conflicts/sync_conflicts.md` and `bob vault-sync status` (human panel in red,
and `--json`). Copying the conflict copy back alone does not undo a false positive:
`HEAD` still holds the pre-quarantine note, so the next cycle sees the same
displaced-plus-foreign-identity signature and quarantines the same bytes again.

To accept the quarantined bytes after review, establish them as `HEAD` in a scoped
commit while the maintenance lock is held, then let sync resume. Do not run
`bob vault-sync` between the copy and the commit.

```bash
git -C ~/bob status --short
git -C ~/bob diff -- <note> _conflicts/<copy>
bob vault-sync status --json
cp ~/bob/_conflicts/<copy> ~/bob/<note>
lock="${BOB_VAULT_SYNC_LOCK_FILE:-${XDG_RUNTIME_DIR:-/tmp}/bob_sync.lock}"
flock -w 120 "$lock" bash -c 'git -C ~/bob add -- <note> && git -C ~/bob commit -m "vault: accept reviewed <note> after false-positive quarantine" -- <note>'
bob vault-sync
bob vault-sync status --json
```

The `git status` preview protects unrelated staged changes: the scoped
`git add`/`commit -- <note>` touches only the accepted note and leaves every other
staged or unstaged path alone. Holding `$BOB_VAULT_SYNC_LOCK_FILE`
(`bob_sync.lock`) with `flock` makes a concurrent background `bob vault-sync -q`
exit quietly instead of quarantining mid-acceptance; pausing the athena/apollo
systemd service or the Mac LaunchAgent works too. The follow-up `bob vault-sync`
should report no new `clobbers` entry because the working tree now matches `HEAD`.

If a clobber was already committed:

1. Find the commit with `git -C ~/bob log -p -- <note>`.
2. `git show <commit>^:<note> > <note>`; never `git revert` a mixed vault commit.
3. `bob task reconcile --dry-run`, then live, to drop stale LINKED FROM / BLOCKS
   mirrors.
4. `bob ref scan --dry-run`.
5. `bob vault-sync`.

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
launchd starts that command through `/bin/sh -c`, which `exec`s
`/Users/bbugyi/.cargo/bin/bob vault-sync -q`. Background Task Management then tracks
Apple-signed `/bin/sh` instead of the ad-hoc `bob` binary that `cargo install` replaces.
`launchctl print` shows `program = /bin/sh`.

Bob Mac Capture also runs `bob vault-sync -q` when the Refs panel opens (throttled to one pull per 5 s) and before a ⌘R refresh, so references pushed from athena or apollo show up without waiting for the 15 s LaunchAgent.

## Mac scheduled maintenance

The MacBook also runs three independent 15-minute cron jobs, staggered five minutes
apart so their writes do not collide with each other or with the 15-second
`bob-vault-sync` LaunchAgent:

```cron
0,15,30,45 * * * * ~/bin/maybe_bob_highlights_sync -w >> /var/tmp/maybe_bob_highlights_sync.log 2>&1
5,20,35,50 * * * * ~/.cargo/bin/bob projects sync >> /var/tmp/bob_projects.log 2>&1
10,25,40,55 * * * * ~/.cargo/bin/bob task reconcile --retry-timeout 120 >> /var/tmp/bob_task_status_hooks.log
```

An installed old line `bob task-status-hooks --retry-timeout 120` keeps working
as a permanent silent alias.

Highlights intake precedes project reconciliation, which precedes task-status
reconciliation. The 2-minute `--retry-timeout` on `bob task reconcile` fits
comfortably inside the 15-minute cadence and absorbs transient maintenance-lock
contention and concurrent-save races against the 15-second sync LaunchAgent; see
[Retries](task-status-hooks.md#retries) for the backoff policy and its allowed failure
reasons. Offsetting the three jobs by minutes reduces collisions between them, but
cannot guarantee exclusion from an open editor, a slow job, or the sync LaunchAgent —
correctness comes from the existing lock and guarded-write checks plus the fresh-attempt
retry behavior, not from the schedule alone.

This crontab is not chezmoi-managed; it is installed by hand with `crontab` directly on
the Mac. The highlights and projects jobs redirect both stdout and stderr with
`>> logfile 2>&1` (in that order). The reconcile job redirects stdout only:
routine retry progress and the final human result land in
`/var/tmp/bob_task_status_hooks.log`, while warnings and real terminal failures remain
on stderr so cron mail is still actionable.

```bash
ssh mac crontab -l
ssh mac 'tail -n 80 /var/tmp/bob_task_status_hooks.log'
ssh mac '~/.cargo/bin/bob task reconcile --help' # confirm --retry-timeout is listed before installing a crontab that uses it
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
2. `bob task archive`
3. `bob articles format --all --check` (report-only; pass unless bob fails)
4. `bob vault-sync`

That ordering pulls the MacBook's latest notes before maintenance rewrites task blocks
and pushes the maintenance commit afterwards.

## Google Keep import history

`.bob/gkeep/imports/*.json` receipts are tracked shared history. They travel
with the imported tasks on every host that runs `bob vault-sync`, including
the MacBook. The vault allowlist ignores everything by default (`*`) and
un-ignores selected note and attachment extensions; JSON is otherwise allowed
only under `.obsidian`, so the vault root `.gitignore` also carries:

```gitignore
!/.bob/gkeep/imports/*.json
```

Do not move or delete those files, and do not disable commits, to make a Keep
import succeed. A bob-cli binary update cannot change vault ignore rules; the
allowance has to reach each host through this Git sync workflow. After the
change is on `origin/master`, `bob vault-sync` on the MacBook is the path that
makes `bob gkeep pull` able to track a new receipt.

## Reference sources

`.bob/ref-sources/<ref_type>/<stem>--<hash12>/` holds the write-once saved
Markdown (`source.md`) plus its render-time code catalog (`code.json`).
They travel with the vault on every host that runs `bob vault-sync`.
`source.md` is covered by `!*.md`, but JSON is otherwise allowed only
under `.obsidian`, so the vault root `.gitignore` also carries:

```gitignore
!/.bob/ref-sources/**/*.json
```

Verify the path is not ignored:

```bash
git -C ~/bob check-ignore -v .bob/ref-sources/chat/probe--000000000000/code.json
```

A negated last match (`!/.bob/ref-sources/**/*.json`) means the file is
trackable. When `code.json` is ignored on a host, `bob ref create` still
files the reference and prints `warning: code catalogs are ignored...`.
See `docs/highlights-create.md#saved-markdown`.

## Stats review journal

`.bob/stats/review/*.jsonl` is the per-host review-walk journal. Each
host appends to its own monthly file; readers union the files and never
rewrite them. The files contain no task text and no derived metric
values. They travel with the vault on every host that runs
`bob vault-sync`, including the MacBook. JSON is otherwise allowed only
under `.obsidian`, so the vault root `.gitignore` also carries:

```gitignore
!/.bob/stats/**/*.jsonl
```

Verify the path is not ignored:

```bash
git -C ~/bob check-ignore -v .bob/stats/review/2026-10.probe.jsonl
```

A negated last match (`!/.bob/stats/**/*.jsonl`) means the file is
trackable. The directory is created on the first counted `]s` answer;
its absence is expected until then. Do not move or delete host files to
make a metric look complete. The journal contract lives in
[stats.md](stats.md#review-journal).

## Pomodoro theme journal

`.bob/themes/<host>.jsonl` is the per-host Pomodoro theme preference
journal (hide, unhide, color pin, and rename). Each host appends
to its own file; readers union every `*.jsonl` in the directory and
never rewrite them. Bob is the only writer. JSON is otherwise allowed
only under `.obsidian`, so the vault root `.gitignore` also carries:

```gitignore
!/.bob/themes/*.jsonl
```

Verify the path is not ignored:

```bash
git -C ~/bob check-ignore -v .bob/themes/apollo.jsonl
```

A negated last match (`!/.bob/themes/*.jsonl`) means the file is
trackable. The directory is created on the first hide, unhide, color, or
rename write; its absence is expected until then. A host that has not synced
this allowlist keeps its hides local until it does. The journal contract
lives in [pomodoro-themes.md](pomodoro-themes.md#pt4-preferences-journal).

`bob themes rename` commits once (`bob themes rename: OLD → NEW`) and
post-syncs; undo is `git revert` per the
[pomodoro-themes rename runbook](pomodoro-themes.md#pt10-global-rename).

`bob themes split` commits once (`bob themes split: OLD → T1, T2, …`) and
post-syncs; it writes no journal event. Undo is `git revert` per the
[pomodoro-themes split runbook](pomodoro-themes.md#pt11-split-by-entry).

`bob relations rename` also commits once (`bob relations rename: OLD → NEW`)
and post-syncs. Its verified rewrite changes relation-label spans only,
including `done/` and the previous daily note. Undo the whole operation,
including its journal event, with `git revert <rename-commit>`.

The vault root `.gitattributes` unions concurrent theme-journal appends:

```gitattributes
.bob/themes/*.jsonl merge=union
```

Check the attribute with:

```bash
git -C ~/bob check-attr merge -- .bob/themes/apollo.jsonl
```

The expected output ends with `merge: union`.

## Relation label journal

`.bob/relations/<host>.jsonl` is the per-host relation label
preference journal (hide, unhide, and rename). It follows the theme
journal's rule: each host appends to its own file, readers union
every `*.jsonl` in the directory, and bob is the only writer. The
vault root `.gitignore` also carries:

```gitignore
!/.bob/relations/*.jsonl
```

Verify the path is not ignored:

```bash
git -C ~/bob check-ignore -v .bob/relations/apollo.jsonl
```

A negated last match (`!/.bob/relations/*.jsonl`) means the file is
trackable. The directory is created on the first hide, unhide, or
rename write. The vault root `.gitattributes` unions concurrent
relation-journal appends:

```gitattributes
.bob/relations/*.jsonl merge=union
```

```bash
git -C ~/bob check-attr merge -- .bob/relations/apollo.jsonl
```

The journal contract lives in
[task-relations.md](task-relations.md#16-label-vocabulary).

## Authored content coverage

The vault allowlist ignores everything by default (`*`) and re-includes only
selected extensions. That allowlist used to miss authored content outside notes
and standard attachments: mind maps (`*.xmind`), zorg templates (`*.zot`),
diagrams (`*.puml`), text files (`*.txt`), textbundle bodies (`*.markdown`),
the textbundle manifest, one extensionless installer note, and the legacy
migration reports and helper sources. Those paths had no off-host copy once
Obsidian Sync was retired. The vault root `.gitignore` now carries narrow
re-includes for them:

```gitignore
!*.zot
!*.xmind
!*.puml
!*.tsv
!*.markdown
!*.txt
!/_meta/migration/inventory.json
!/_meta/migration/reports/*.tsv
!/_meta/migration/*.txt
!/_meta/migration/tools/*.py
!/lib/docs/sase_INSTALL
!/lib/docs/*.textbundle/*.json
!/lib/docs/*.textbundle/*.markdown
```

`*.py` and `*.json` stay narrowly scoped to those migration and textbundle
paths so caches, plugin builds, and machine-local state stay ignored. The
migration reports and helper sources were checked for secrets and regenerable
junk before inclusion; `__pycache__` output stays excluded.

Intentional exclusions that remain ignored: `lit_review/` and `xlib/` (out of
band by design), custom `bob-*` plugin builds including
`block-id-prompt` (redeploy from the `bob-plugins` source repo),
caches and bytecode, editor and OS state, `sdd/` machine-local state, and the
two over-100-MiB `old_lib` PDFs (GitHub rejects them; they stay in the athena
`old_lib` backup copy). `.bob/gkeep/migrate-tasks/*.json` receipts also stay
ignored. They are the crash-recovery and commit-retry journal of
`bob gkeep migrate-tasks` on the host that ran it ([gkeep.md](gkeep.md)), so
they are machine-local state. The migrated notes commit normally, and the
shared Keep import history in `.bob/gkeep/imports/` stays tracked.

Verify coverage with exit codes (`1` means trackable, `0` means ignored):

```bash
git -C ~/bob check-ignore -q xmind/zorg.xmind || echo "trackable"
git -C ~/bob check-ignore -q lit_review/acx_dev_syntax_1.pdf && echo "ignored"
```

Text files commit through the vault's `* text=auto eol=lf` normalization, so a
restored working-tree copy can differ from the pre-backup bytes by line
endings alone (the migration TSVs carry CRLF). Blob-level comparison is the
check that matters: every covered file's `git hash-object` matches its
`origin/master` blob.

The vault root `.gitattributes` also unions concurrent journal appends:

```gitattributes
.bob/stats/**/*.jsonl merge=union
.bob/themes/*.jsonl merge=union
```

Check the attribute with:

```bash
git -C ~/bob check-attr merge -- .bob/stats/review/2026-10.probe.jsonl
```

The expected output ends with `merge: union`.

## Highlights intake

`lit_review/` and `xlib/` are gitignored. `lit_review/` is out-of-band storage: copy it
explicitly when a second machine needs the PDFs.

`xlib/` is a gitignored, host-local intake queue. Never rsync it between hosts: a
cross-host copy can race the local drain and create two reference notes for one capture.

By default, `bob ref create`, the ref-jobs worker, and `bob gkeep pull` file the PDF,
reference note, and reading task together under `bob_sync.lock` then `scan.lock`, and
commit those paths as one change. If intake is kept or cannot safely run, the PDF stays
in that host's `xlib/`; no other host is needed to finish it.

On athena and apollo, chezmoi installs and enables `bob-ref-intake.timer`. It runs
`bob ref scan --intake-only` two minutes after boot and every two minutes after that.
The timer drains only that host's queue; an empty queue takes no lock and performs no
sync. Recover or drain a queue manually with:

```bash
ssh athena 'bob ref scan -i'
ssh apollo 'bob ref scan -i'
```

The MacBook's staggered 15-minute `maybe_bob_highlights_sync -w` cron job remains the
full annotation scan. It also drains files in the Mac's own `xlib/`; it never pulls the
Linux queues. The gate runs when `lib/` changed today or when `xlib/` contains a file,
so Mac-local queued creates are not missed. See
[Mac scheduled maintenance](#mac-scheduled-maintenance) for the exact crontab.

The old `bob_xlib_pull` bridge and its `pre_scan_hook` configuration are retired. Each
host now has exactly one intake drainer: the local timer on athena/apollo and the local
full scan on the Mac. Keep any custom pre-scan hook independent of intake; writing scans
already hold the maintenance and scan locks while it runs.

Useful checks:

```bash
bob ref doctor
ssh mac 'PATH="$HOME/bin:$HOME/.cargo/bin:/opt/homebrew/bin:/usr/bin:/bin" bob ref doctor'
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
committed, push retries, duration, conflict-copy paths, interrupted-merge recovery,
`aborted_merge_head`, `halted_merge_head`, `interrupted_fast_forward_recovered`, this
cycle's `clobbers` and the persistent `recent_clobbers` (last 10), and the last error.
The halted SHA and clobber rows are shown in red in the human status panel. `clobbers`
resets each cycle; `recent_clobbers` keeps the latest detections so a 15-second follow-up
cycle does not hide them.

## Rollback

Rollback to git, not Obsidian Sync. Tag the pre-cutover commit before risky changes and
use the filesystem backups from the migration if the vault must be restored. Obsidian
Sync was over quota when this channel replaced it, so it is not a reliable rollback
target. To undo a `bob ref migrate-tasks --write` or `bob ref migrate-zorg --write` commit, follow the rollback runbook in
`docs/ref.md` (`git revert` of the migration commit, then `bob vault-sync`).

Keep the disabled `ob-sync-bob.service` and `ob-sync-bob-poll` files through the soak.
Unlinking Obsidian Sync, logging out of the Sync account, removing the old service
files, and canceling the subscription are user-timed cleanup steps after a clean soak.
