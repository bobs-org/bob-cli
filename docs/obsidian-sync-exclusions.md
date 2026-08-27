# Obsidian Sync folder exclusions

> **Historical.** Obsidian Sync is no longer the Bob vault's sync channel. The vault
> syncs through git only; see [vault-git-sync.md](vault-git-sync.md) for the live
> runbook. `ob-sync-bob.service` is disabled and the Obsidian Sync core plugin is off,
> so nothing here runs as part of normal operation. This page is kept because the
> exclusion semantics it records are not documented anywhere else and are easy to get
> wrong if Sync is ever relinked.
>
> **Exactly one sync engine may run against `~/bob`.** With both git sync and Obsidian
> Sync live, a delete propagated by one is resurrected by the other, indefinitely. Stop
> `bob-vault-sync.service` and the MacBook LaunchAgent before re-enabling Sync for any
> reason.

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
obsidian-headless sync config, and desktop Obsidian stores them in the app's
own IndexedDB state, outside the vault. Desktop Obsidian never writes a
`.obsidian/sync.json`, so the absence of that file says nothing about a
device's exclusions. Configure the same folder exclusion on every device that
should keep its local copy before any deletion-drain phase runs.

The `ignoreFolders` value is prefix-matched and case-sensitive. For the Bob
vault's archival library, the value is exactly:

```text
old_lib
```

Do not write `/old_lib`, `old_lib/`, or `Old_lib`.

## Procedure

1. Confirm that the folder is fully backed up outside Obsidian Sync. For
   `old_lib/`, the required durable backup is the vault Git repo; keep a second
   independent copy during any destructive sync window. Note that the vault Git
   repo does not cover every path: `lit_review/` and `xlib/` are gitignored by
   policy, and paths outside `.gitignore`'s extension allowlist are untracked.
2. Stop automated sync processes. Today that means `bob-vault-sync.service` on
   athena, the `com.bbugyi.bob-vault-sync` LaunchAgent on the MacBook, the 03:30
   `bob nightly` crontab line, and `ob-sync-bob.service` if it has been re-enabled.
   `bob nightly` no longer gates on `ob sync`; it runs `vault-sync`,
   `move-done-tasks`, `vault-sync`.
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
   systemctl --user start ob-sync-bob.service  # only if Sync is the live channel
   tail -f ~/.config/obsidian-headless/sync/8a259ad922718b6d8400c1f0e3ba8abe/sync.log | grep -i old_lib
   ```

Any `Uploading file old_lib/...` log line means the exclusion is not active.
Stop the service immediately and recheck the stored `ignoreFolders` value before
running another sync cycle.

## Verifying a desktop device

Set the exclusion in the GUI under Settings > Sync > Excluded folders. There is
no supported CLI for this, and there is no file in the vault to inspect
afterwards.

To confirm the saved value without the GUI, read the sync record out of
Obsidian's IndexedDB. Desktop Obsidian persists its sync settings with
`db.put("data", ...)`, and because that record carries the full local and remote
file maps it lands in the IndexedDB blob store rather than inline in LevelDB:

```bash
python3 - <<'EOF'
import os
root = os.path.expanduser(
    "~/Library/Application Support/obsidian/IndexedDB"
    "/app_obsidian.md_0.indexeddb.blob"
)
best = None
for dirpath, _, filenames in os.walk(root):
    for name in filenames:
        path = os.path.join(dirpath, name)
        data = open(path, "rb").read()
        if b"dataVer" not in data:
            continue
        if best is None or os.stat(path).st_mtime > best[0]:
            best = (os.stat(path).st_mtime, data)
blob = best[1]
segment = blob[blob.find(b"ignoreF"):blob.find(b"preventSleep")]
index = 0
while index < len(segment):
    if segment[index] == 0x22 and segment[index + 1] < 0x80:
        length = segment[index + 1]
        print(segment[index + 2:index + 2 + length].decode("utf-8", "replace"))
    index += 1
EOF
```

Each printed line is one entry of that device's `ignoreFolders`. Grepping the
blob for `ignoreFolders` does not work: the surrounding bytes are Snappy-encoded,
so the key itself is usually stored as a back-reference rather than as literal
text.
