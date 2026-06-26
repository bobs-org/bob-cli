# Bob Plugins

`bob plugins` manages Bryan's custom Bob Obsidian plugins from the
[`bobs-org/bob-plugins`](https://github.com/bobs-org/bob-plugins) repo, which
is the source of truth for the six plain-JavaScript community plugins. The repo
holds one folder per plugin under `plugins/<id>/`, each with a `manifest.json`,
a `main.js`, and an optional `styles.css`.

## Commands

```bash
bob plugins [-b|--bob-dir DIR] [-f|--format table|json] [-r|--repo DIR]
bob plugins list [-b|--bob-dir DIR] [-f|--format table|json] [-r|--repo DIR]
bob plugins sync [-B|--backup-dir DIR] [-b|--bob-dir DIR] [-d|--dry-run] [-F|--force] [-p|--plugin ID] [-r|--repo DIR]
```

`list` is read-only. Running `bob plugins` with no subcommand runs `list` with
the same options. `sync` deploys the repo into the vault (see [Sync](#sync)).

## Discovery

`list` reads the repo's `plugins/` directory and builds one row per plugin
folder. The plugin id, version, and description come from that folder's
`manifest.json`; an empty or absent manifest `id` falls back to the folder name.
A folder whose `manifest.json` is missing or unparseable is reported as an error
on stderr and the command exits non-zero, but the remaining plugins still list.

Two roots feed the report:

- **Repo root.** Resolves from `-r, --repo`, then the `BOB_PLUGINS_DIR`
  environment variable, then the default
  `~/projects/github/bobs-org/bob-plugins`. Plugins live under
  `<repo>/plugins/<id>/`.
- **Vault root.** Resolves from `-b, --bob-dir`, then `BOB_DIR`, then `~/bob`.
  Installed plugins live under `<bob-dir>/.obsidian/plugins/<id>/`, and the
  enabled set is read from `<bob-dir>/.obsidian/community-plugins.json`.

## Columns

| Column        | Source                                                                 |
| ------------- | ---------------------------------------------------------------------- |
| `PLUGIN`      | manifest `id` (repo folder name when the manifest omits it)            |
| `VERSION`     | manifest `version`                                                     |
| `SYNC`        | repo files vs. vault files                                             |
| `VAULT`       | `community-plugins.json` plus the installed-folder check               |
| `DESCRIPTION` | manifest `description`, truncated to the remaining terminal width      |

### SYNC state

`SYNC` byte-compares the managed files — `manifest.json`, `main.js`, and
`styles.css` when the repo has one — against the vault copy:

- `synced` — every managed repo file is present and byte-identical in the vault.
- `drift` — the vault has the plugin folder, but at least one managed file is
  missing or differs.
- `missing` — the vault has no folder for this plugin.

Only the managed files are compared. Runtime files such as `data.json` are
never read.

### VAULT state

`VAULT` reports the plugin's enable state in the vault:

- `enabled` — the id is listed in `community-plugins.json`.
- `disabled` — the plugin folder exists in the vault but the id is not enabled.
- `not installed` — the vault has no folder for this plugin.

A missing or unreadable `community-plugins.json` is treated as "nothing
enabled" rather than an error, so installed plugins then read as `disabled`.

## Header and Footer

The header names the repo and the plugin count, such as
`Bob Plugins · 6 · /home/bryan/projects/github/bobs-org/bob-plugins`. The
footer summarizes the sync states, such as
`6 synced · 0 drift · 0 not installed`. On a non-color or piped stream the
separator renders as `-` and the colored state glyphs are dropped.

## Exit Status

`list` exits `0` even when plugins drift or are not installed — those are
reportable states, not failures. It exits `1` only on a real error, such as an
unreadable repo `plugins/` directory or an unparseable manifest, and writes the
error to stderr.

## JSON Output

`-f, --format json` prints a single stable object for scripting:

```json
{
  "ok": true,
  "repo": "/home/bryan/projects/github/bobs-org/bob-plugins",
  "bob_dir": "/home/bryan/bob",
  "count": 6,
  "synced": 6,
  "drift": 0,
  "not_installed": 0,
  "plugins": [
    {
      "id": "block-id-prompt",
      "version": "1.0.0",
      "description": "Prompt for a custom block ID when a wiki block link uses the ^^ marker.",
      "sync": "synced",
      "vault": "enabled"
    }
  ]
}
```

The `sync` field is `synced`, `drift`, or `missing`; the `vault` field is
`enabled`, `disabled`, or `not_installed`. On error, JSON mode prints
`{"ok": false, "error": "..."}` instead.

## Sync

`sync` deploys the repo into the vault. For each plugin it copies the managed
files — `manifest.json`, `main.js`, and `styles.css` when the repo has one —
from `<repo>/plugins/<id>/` into `<bob-dir>/.obsidian/plugins/<id>/`. Runtime
files such as `data.json` are never read or written, so plugin settings survive
a sync. The repo and vault roots resolve exactly as they do for `list`.

Before any existing vault file is overwritten, `sync` copies the current vault
file to a timestamped backup directory. Backups default to
`~/.local/state/bob-cli/plugin-backups/<timestamp>/<plugin-id>/<file>`, outside
the vault Git repo. The base directory resolves from `-B, --backup-dir`, then
`BOB_PLUGIN_BACKUPS_DIR`, then the default above. If a backup cannot be written,
that file is not overwritten and the command exits non-zero.

For every changed text file, `sync` prints a unified diff from the current vault
copy to the incoming repo copy. Non-UTF-8 or large minified files are summarized
by byte size instead of dumping unreadable content. New files get a compact
`(new file, N lines)` note because no existing vault content is at risk.

For every managed file, `sync` reports one of:

- `copied <file>` — the vault file was missing (`(new)`) or differed and was
  rewritten from the repo. Existing overwritten files are backed up first, and
  the backup path is printed.
- `up to date` — every managed file already matched the repo byte-for-byte, so
  nothing was written.
- `skipped <file> (dirty in vault; use -F/--force)` — the vault file differs and
  has uncommitted changes in the vault Git repo, so it was left untouched. The
  diff is still printed so you can decide whether to rerun with `--force`.

### Options

- `-B, --backup-dir <DIR>` writes overwrite backups under this directory instead
  of `BOB_PLUGIN_BACKUPS_DIR` or
  `~/.local/state/bob-cli/plugin-backups`.
- `-b, --bob-dir <DIR>` selects the vault root.
- `-d, --dry-run` previews every action, including diffs and backup paths,
  without writing vault files or backups.
- `-F, --force` overwrites a vault file even when it has uncommitted Git changes.
- `-p, --plugin <ID>` syncs a single plugin instead of all of them. An id that
  the repo does not contain is an error.
- `-r, --repo <DIR>` selects the plugins repo root.

### Dirty-file guard

Before overwriting an existing vault file that differs from the repo, `sync`
runs `git status --porcelain` on it. If the file has uncommitted changes it is
skipped with a warning so local edits are never clobbered silently; pass
`-F, --force` to overwrite anyway. A vault that is not a Git repo has no
committed state to protect, so the copy proceeds. A file that already matches
the repo is reported as unchanged and never triggers the guard.

Dry-run mode follows the same decision path and prints the same diffs, but it
uses `would copy` and `would back up to` wording and writes nothing. This makes
`bob plugins sync -d` the review step before a real sync.

### Exit status

`sync` exits `0` even when it skips dirty files — a refusal is a deliberate
warning, not a failure, matching how `list` treats drift. It exits `1` only on a
real error such as an unreadable repo, an unknown `--plugin` id, or a failed
copy, and writes the cause to stderr.

## Examples

```bash
bob plugins
bob plugins list
bob plugins list -f json
bob plugins list -b ~/bob -r ~/projects/github/bobs-org/bob-plugins
bob plugins sync --dry-run
bob plugins sync -p bob-project-tasks
bob plugins sync -F -b ~/bob -r ~/projects/github/bobs-org/bob-plugins
```
