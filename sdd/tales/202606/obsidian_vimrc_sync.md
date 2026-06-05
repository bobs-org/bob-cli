---
create_time: 2026-06-05 16:12:32
status: wip
prompt: sdd/prompts/202606/obsidian_vimrc_sync.md
---
# Fix Obsidian VimRC Sync

## Problem

Bryan's Bob vault uses the Vimrc Support community plugin. The active config file is `/home/bryan/bob/.obsidian.vimrc`,
configured via `/home/bryan/bob/.obsidian/plugins/obsidian-vimrc-support/data.json`.

The file is clean and tracked in the vault Git repo, so this is not a Git staging or ignore problem. The root cause is
Obsidian Sync's hidden-file policy: files and folders whose names begin with `.` are excluded from Obsidian Sync, except
for the vault configuration folder itself (`.obsidian`). The local `ob sync-status --path /home/bryan/bob` output also
shows only selected attachment/config categories, so a plain non-Markdown `obsidian.vimrc` file would still depend on
per-device "unsupported/other file type" sync settings. That would be fragile on the MacBook.

## Approach

Make the VimRC source a normal Markdown vault file so it is covered by Obsidian Sync's default note syncing and by the
vault's existing Git allow rules.

1. Preserve the current VimRC commands exactly in a new syncable Markdown file, likely `obsidian_vimrc.md` at the vault
   root. The Vimrc Support plugin reads raw file contents from the vault adapter, so the file extension does not need to
   be `.vimrc`.
2. Update `.obsidian/plugins/obsidian-vimrc-support/data.json` so `vimrcFileName` points to the new Markdown file.
3. Keep `.obsidian.vimrc` in place for compatibility during rollout, but stop relying on it. Avoid deleting it in this
   change because deleting a hidden file would not propagate through Obsidian Sync anyway and could surprise a machine
   still using the old plugin setting.
4. Stage/commit only the task-related vault files after verification, per `/home/bryan/bob/AGENTS.md`. Leave the many
   unrelated pre-existing vault changes untouched.

## Verification

1. Confirm the new Markdown VimRC file is not ignored by Git and appears in `git -C /home/bryan/bob status --short`.
2. Validate `.obsidian/plugins/obsidian-vimrc-support/data.json` as JSON.
3. Verify the plugin code path accepts the configured relative filename, which it does via
   `this.app.vault.adapter.read(fileName)`.
4. Run a focused `ob sync --path /home/bryan/bob` after edits to upload the syncable file and synced plugin data, then
   inspect `ob sync-status --path /home/bryan/bob`.
5. Commit only the new VimRC Markdown file and Vimrc Support `data.json` with the SASE git commit workflow. Do not stage
   unrelated hotkey, plugin, note, generated query, or restaurant-note changes currently present in the vault.

## Risks

- The MacBook must have Obsidian Sync enabled for Markdown notes and community plugin data/config. Markdown note sync is
  fundamental and should be present; plugin data is a sync category that may need to be enabled on the MacBook if it has
  been disabled there.
- If the MacBook already has a local conflicting Vimrc Support `data.json`, Obsidian may create a conflict file. The
  Markdown VimRC file itself should still sync normally.
