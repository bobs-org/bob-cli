# Obsidian Sync folder exclusions

This runbook covers removing an already-synced folder from Obsidian Sync while
preserving the local copy in the Bob vault.

## Key rules

Obsidian Sync exclusions are not remote delete commands. Adding a path to the
excluded folders list stops future sync consideration for that path, but it does
not remove files that are already present in the remote vault.

For an already-synced folder, push the remote deletions before setting the
exclusion. If the exclusion is set first, the sync client filters those paths
out and will skip the remote-deletion branch for them.

Exclusions are device-local. The headless client stores them under its
obsidian-headless sync config, and desktop Obsidian stores them in vault-local
device state. Configure the same folder exclusion on every device that should
keep its local copy before any deletion-drain phase runs.

The `ignoreFolders` value is prefix-matched and case-sensitive. For the Bob
vault's archival library, the value is exactly:

```text
old_lib
```

Do not write `/old_lib`, `old_lib/`, or `Old_lib`.

## Procedure

1. Confirm that the folder is fully backed up outside Obsidian Sync. For
   `old_lib/`, the required durable backup is the vault Git repo; keep a second
   independent copy during any destructive sync window.
2. Stop automated sync processes, including `ob-sync-bob.service` and any cron
   job that can run `bob nightly`.
3. Move the target folder out of the sync client's view without copying bytes,
   for example by staging it under a dot-prefixed vault directory:

   ```bash
   mkdir -p ~/bob/.old_lib_migrating
   mv ~/bob/old_lib/* ~/bob/.old_lib_migrating/
   rmdir ~/bob/old_lib
   ```

4. Run foreground sync cycles until the remote has no live entries for the
   original folder. Do not proceed while any remote entry remains live.
5. Set the device-local exclusion while the folder is still staged and sync
   automation is still stopped:

   ```bash
   ob sync-config --path ~/bob --excluded-folders old_lib
   ```

   `--excluded-folders` replaces the whole list. If other exclusions already
   exist, pass the full comma-separated list.
6. Verify both the CLI output and the raw config before restoring the folder:

   ```bash
   ob sync-config --path ~/bob | grep 'Excluded folders'
   python3 -c "import json; p='/home/bryan/.config/obsidian-headless/sync/8a259ad922718b6d8400c1f0e3ba8abe/config.json'; print(json.load(open(p)).get('ignoreFolders'))"
   ```

   The expected value for this migration is `['old_lib']`.
7. Restore the local folder, restart automation, and watch several sync cycles
   for accidental uploads:

   ```bash
   mkdir -p ~/bob/old_lib
   mv ~/bob/.old_lib_migrating/* ~/bob/old_lib/
   rmdir ~/bob/.old_lib_migrating
   systemctl --user start ob-sync-bob.service
   tail -f ~/.config/obsidian-headless/sync/8a259ad922718b6d8400c1f0e3ba8abe/sync.log | grep -i old_lib
   ```

Any `Uploading file old_lib/...` log line means the exclusion is not active.
Stop the service immediately and recheck the stored `ignoreFolders` value before
running another sync cycle.
