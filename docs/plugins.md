# Bob Plugins

`bob plugins` manages Bryan's custom Bob Obsidian plugins from the
[`bobs-org/bob-plugins`](https://github.com/bobs-org/bob-plugins) repo, which
is the source of truth for the custom plain-JavaScript community plugins.
Typical order: `bob plugins` (or `bob plugins list`) to see repo versions
versus the vault, `bob plugins sync --dry-run` to preview, then
`bob plugins sync` to deploy. The repo holds one folder per plugin under
`plugins/<id>/`, each with a `manifest.json`, a `main.js`, and an optional
`styles.css`. Do not edit installed copies under `~/bob/.obsidian/plugins/` as
the source of truth — those files are overwritten on the next sync.

## Contents

- [Commands](#commands)
- [Repo refresh](#repo-refresh)
- [Discovery](#discovery)
- [Columns](#columns)
- [Header and footer](#header-and-footer)
- [Exit status](#exit-status)
- [JSON output](#json-output)
- [Sync](#sync)
- [Examples](#examples)

## Commands

```bash
bob plugins [-b|--bob-dir DIR] [-f|--format table|json] [-n|--no-pull] [-r|--repo DIR]
bob plugins list [-b|--bob-dir DIR] [-f|--format table|json] [-n|--no-pull] [-r|--repo DIR]
bob plugins sync [-B|--backup-dir DIR] [-b|--bob-dir DIR] [-d|--dry-run] [-F|--force] [-f|--format table|json] [-n|--no-pull] [-p|--plugin ID] [-r|--repo DIR]
```

`list` is read-only. Running `bob plugins` with no subcommand runs `list` with
the same options. `sync` deploys only the plugin ids listed in `plugins.sync`
in bob's config file into the vault (see [Sync](#sync)).
When a plugin has a `.bob-deploy.json` stamp, list prints a dimmed provenance
line under the row — including the exact phrase `deployed from uncommitted build`
for a dirty source checkout — without changing the SYNC/VAULT columns or counts.
Rows whose id the allowlist leaves out carry a dim
`not in plugins.sync; bob plugins sync skips it` line (suppressed when the
list itself is unset or invalid) without changing the SYNC/VAULT columns,
counts, header, or footer.
In a Git vault, list also prints a `warning` line under the row when the
plugin's whole vault folder is not ignored or holds tracked files (see
[Vault-ignore warnings](#vault-ignore-warnings)); the warning never changes
the SYNC/VAULT columns or counts.

## Repo Refresh

Before `list` or `sync` analyzes plugin files, it runs a non-interactive
`git pull` in the plugins repo so the checked-out files reflect the latest
committed source. Pass `-n, --no-pull` to skip this refresh and use the current
checkout exactly as-is.

A repo path that is not a Git worktree, or a system without `git`, skips the
refresh silently. If `git pull` fails because the network is unavailable,
authentication would require a prompt, histories diverged, or local edits block
the pull, the refresh falls back to a non-interactive `git fetch` (default
remote and refspec) so remote-tracking refs stay fresh while checkout files
are left untouched. When the fallback fetch succeeds the command writes one
stderr warning naming the pull failure and noting that remote refs were
fetched instead; when the fetch also fails it keeps the single pull-failure
warning. `--no-pull` skips both pull and fetch. Pull and fetch output never
goes to stdout, so table and JSON output stay parseable.

## Discovery

`list` reads the repo's `plugins/` directory and builds one row per plugin
folder. The plugin id, version, and description come from that folder's
`manifest.json`; an empty or absent manifest `id` falls back to the folder name.
A folder whose `manifest.json` is missing or unparseable is reported as an error
on stderr and the command exits non-zero, but the remaining plugins still list.

Three inputs feed the report:

- **Repo root.** Resolves from `-r, --repo`, then the enclosing
  bob-plugins checkout when the cwd sits inside one, then the
  `BOB_PLUGINS_DIR` environment variable, then the default
  `~/projects/github/bobs-org/bob-plugins`. `list`, the bare default, and
  `sync` all resolve the same repo from the cwd, so the table header, JSON
  `repo`, and the deploy target always agree. Plugins live under
  `<repo>/plugins/<id>/`.
- **Vault root.** Resolves from `-b, --bob-dir`, then `BOB_DIR`, then `~/bob`.
  Installed plugins live under `<bob-dir>/.obsidian/plugins/<id>/`, and the
  enabled set is read from `<bob-dir>/.obsidian/community-plugins.json`.
- **Config allowlist.** The `plugins.sync` list in bob's config file
  (`BOB_CONFIG_FILE`, then `$XDG_CONFIG_HOME/bob/config.yml`, then
  `~/.config/bob/config.yml`). `list` marks each row `configured: true`
  when its id is on the list and `false` otherwise; when the list is unset
  or invalid, every row reads `configured: false` and list notes the problem
  on stderr without failing. `sync` deploys only listed ids.

## Columns

| Column        | Source                                                                 |
| ------------- | ---------------------------------------------------------------------- |
| `PLUGIN`      | manifest `id` (repo folder name when the manifest omits it)            |
| `VERSION`     | manifest `version`                                                     |
| `SYNC`        | repo files vs. vault files                                             |
| `VAULT`       | `community-plugins.json` plus the installed-folder check               |
| `DESCRIPTION` | manifest `description`, truncated to the remaining terminal width (`COLUMNS` when it is a positive integer, otherwise 100 columns) |

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
  "repo_source": "default",
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
      "vault": "enabled",
      "deployment": {
        "status": "clean",
        "source_head": "777df22f0123456789abcdef0123456789abcdef",
        "host": "athena",
        "deployed_at": "2026-10-26T18:30:00Z"
      }
    }
  ]
}
```

`repo_source` is `flag`, `enclosing_checkout`, `env`, or `default`,
naming which resolution rule selected the repo root. The `sync` field is
`synced`, `drift`, or `missing`; the `vault` field is
`enabled`, `disabled`, or `not_installed`. `configured` is `true` when the
plugin id is listed in `plugins.sync` in bob's config file and `false`
otherwise — including every row when the list is unset or invalid, in which
case list also notes the problem on stderr and stays exit `0`.
`deployment` is omitted when the
plugin has no stamp (legacy installs and plugins that have never been synced
by a stamp-aware `bob`). When present it includes `status`, `source_root`,
`source_head`, `host`, and `deployed_at`. `status` is `clean`, `uncommitted`,
`unknown`, `incomplete`, or `mismatched`. A missing stamp is an ordinary
state, not a list error. `vault_ignore` is omitted when the vault is not a
Git repo or the plugin folder is fully ignored and untracked; otherwise it
carries the warning as `{"status": "tracked" | "not_ignored" | "unknown",
"warning": "..."}`. On error, JSON mode prints `{"ok": false, "error": "..."}` instead.

## Sync

`sync` deploys the repo into the vault. For each plugin id listed in
`plugins.sync` in bob's config file, it copies the managed
files — `manifest.json`, `main.js`, and `styles.css` when the repo has one —
from `<repo>/plugins/<id>/` into `<bob-dir>/.obsidian/plugins/<id>/`. Runtime
files such as `data.json` are never read or written, so plugin settings survive
a sync. Repo plugins the list leaves out are skipped entirely: no copies,
stamps, backups, or guard evaluation, one dim
`not in plugins.sync; skipped` row per id in table output, and their ids in
the JSON `not_configured` array. Each listed id with no matching repo plugin
is an error naming the id, while the other plugins still sync. A missing list
or an invalid `plugins` block exits 2 before any pull, lock, vault read, or
write; table mode explains the setup error on stderr and JSON mode prints
`{"ok": false, "error": "..."}`. A missing list ends its message with a
ready-to-paste `plugins.sync` YAML snippet listing every plugin id in the
current checkout. Each real deploy writes a `.bob-deploy.json` stamp beside the plugin
recording the canonical source checkout, HEAD, a tri-state dirty result (clean,
dirty, or unknown), host, UTC time, optional `SASE_AGENT_ID`, SHA-256
fingerprints of the managed assets, and whether the deploy finished. The stamp
is Git-ignored machine data, not a managed plugin file. The repo and vault
roots resolve exactly as they do for `list`.
The bob-cli `just install-all` command runs this sync against the sibling
`bob-plugins` checkout after pulling it. It previews the sync with
`bob plugins sync --dry-run --format json` to decide whether to restart
Obsidian and, on macOS, posts a notification before restarting Obsidian.
During `just install-all`, the sync's diff output goes to the step log, one row
per changed plugin plus any warnings is shown, and `-v` streams the raw output.

Pre-stamp vault copies cannot be protected retroactively: the first stamp-aware
sync bootstraps provenance after a successful copy. Existing binaries that do
not understand stamps cannot enforce the guard; deploy the updated `bob`
binary before relying on it.

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
- `-d, --dry-run` previews every action, including diffs, backup paths, and
  deploy-guard refusals, without writing vault files, stamps, backups, or lock
  files.
- `-F, --force` overwrites a protected plugin deploy or a vault file with
  uncommitted Git changes. It does not bypass a tracked or unignored stamp,
  lock contention, or I/O failures.
- `-f, --format <FORMAT>` is `table` (default) or `json`.
- `-n, --no-pull` skips the default `git pull` before analyzing the plugins
  repo.
- `-p, --plugin <ID>` syncs a single plugin instead of all listed ones. The
  id must also be listed in `plugins.sync`: an unlisted id copies nothing and
  is an error telling you to add it there. An id that the repo does not
  contain is an error.
- `-r, --repo <DIR>` selects the plugins repo root. An explicit flag
  always wins; without it the enclosing bob-plugins checkout wins over
  `BOB_PLUGINS_DIR` and the default.

### Enclosing-checkout resolution

A bare sync (no `--repo`) deploys the enclosing bob-plugins checkout when the
cwd sits inside one — for example a SASE linked worktree — even when
`BOB_PLUGINS_DIR` or the default points somewhere else. The resolution order
is `--repo`, then the enclosing checkout, then `BOB_PLUGINS_DIR`, then
`~/projects/github/bobs-org/bob-plugins`.

When the enclosing checkout wins and its toplevel differs from what the
env/default rules would have resolved, the command prints one stderr notice
naming both paths:

```text
bob plugins: using enclosing checkout <toplevel> (default is <resolved>; pass --repo to override)
```

No notice is printed when the cwd sits inside the env/default checkout
itself (resolution is unchanged) or outside any bob-plugins checkout. The
notice rides on stderr, so stdout JSON stays parseable.

A checkout counts as a bob-plugins checkout when its `origin` remote points
at `bobs-org/bob-plugins`, or when its root holds a `plugins/` dir plus the
monorepo `package.json`. An explicit `--repo` always names the checkout the
caller means and never prints the notice. From a SASE worktree, a bare
`bob plugins sync -p <id>` deploys that worktree.

### Deploy-protection guard

Before copying any file of a plugin, `sync` reads that plugin's stamp and
refuses the overwrite when it would erase another checkout's uncommitted or
unincorporated build. A blocked plugin has no copied assets, stamp changes, or
backups. Unrelated plugins in the same invocation may still progress; the
command exits `1` if any deploy is refused.

Without `--force`:

- No stamp: bootstrap using existing asset behavior and write provenance after
  a successful sync.
- Incoming assets exactly equal the deployed bundle: permit. Provenance is
  preserved unless a verifiably clean Git checkout of the exact full bundle
  (including whether `styles.css` is present) reconciles an old
  dirty/unknown/stale stamp. Leftover destination `styles.css` that the source
  does not have is not deleted, and that leftover cannot certify an exact
  match.
- Dirty stamp from a different canonical checkout or host: permit without
  `--force` only from a verifiably clean checkout when one commit reachable
  from its HEAD holds the stamped bundle byte for byte; the sync names that
  proving commit. Otherwise refuse, even when HEADs match. Per-file matches
  drawn from different commits never qualify, and a vault edited after the
  deploy (digest mismatch) still refuses.
- Dirty stamp from the same canonical checkout and host: permit iterative
  deploys only while the stamp still matches the vault and its HEAD is an
  ancestor of the incoming HEAD.
- Clean stamp: permit when the stamped commit is a known ancestor of incoming
  HEAD; otherwise refuse.
- Unknown, malformed, unsupported, pending, or digest-mismatched provenance:
  refuse changing assets. Exact equality may reconcile only from a verifiably
  clean complete incoming bundle.

A non-Git source directory can bootstrap, but later differing assets require
`--force` because ancestry is unavailable. A clean unpushed commit is protected
when it is absent from incoming history. Protection never depends on a remote
fetch: the refresh fetches, the guard still only reads local state. Because
the refresh falls back to `git fetch` when `git pull` cannot advance the
worktree, the refusal's landed-build hint usually names the landed commit and
ref instead of the generic fetch hint, and it never changes the decision.

In a Git vault the stamp path must be ignored (for example
`.obsidian/plugins/*/.bob-deploy.json`) and must not be tracked. `--force`
does not bypass that storage requirement. A non-Git vault needs no ignore
rule. `sync` never edits `.gitignore` or the index.

Same-host syncs to one vault take a nonblocking lock in host-local state keyed
by the canonical vault path. Contention fails promptly with a retry message.
List and dry-run take no lock.

To recover an uncommitted deploy that has since been committed, sync a clean
checkout whose HEAD reaches a commit holding the stamped bundle byte for
byte: the sync overwrites without `--force` and names that proving commit in
table output and as `proving_commit` in JSON. Syncing the identical bytes
still reconciles to the clean revision, and a later descendant can update
normally. If the checkout that needs to recover also has its own uncommitted
work, stash it, bring the checkout up to the landed ref, sync from the clean
tree, then restore and redeploy the stashed changes. If no single commit holds
the stamped bundle, review the diff and pass `--force` explicitly.

A protection refusal names the plugin and the earlier deployment (including
the deploying agent when recorded), explains the cause, and gives the
landed-build remedy before the preview and force commands. That remedy names a
commit on an already-fetched remote-tracking ref that holds the deployed
build, or otherwise tells you to fetch.

### Dirty-file guard

Before overwriting an existing vault file that differs from the repo, `sync`
runs `git status --porcelain` on it. If the file has uncommitted changes it is
skipped with a warning so local edits are never clobbered silently; pass
`-F, --force` to overwrite anyway. A vault that is not a Git repo has no
committed state to protect, so the copy proceeds. A file that already matches
the repo is reported as unchanged and never triggers the guard.

### Vault-ignore warnings

In a Git vault, `sync` and `list` check every plugin's whole vault folder
(`.obsidian/plugins/<id>/`) plus each managed file against the vault's ignore
rules with `git check-ignore`, and list tracked files under the folder with
`git ls-files` — the same probes the deploy-stamp storage check uses. When the
folder is not ignored or holds tracked files, the command prints a `warning`
line under the plugin row and sets `vault_ignore` in JSON output (`sync` and
`list`, including `--dry-run` previews). The warning never blocks the copy,
never changes the exit status, and never edits `.gitignore` or the index:
add `.obsidian/plugins/<id>/` to the vault `.gitignore` yourself, and untrack
with care — bob never runs `git rm --cached` automatically, because untracking
on one host deletes the files on the others at their next pull. A vault that
is not a Git repo needs no ignore rule, so there is no warning.

Dry-run mode follows the same decision path and prints the same diffs, but it
uses `would copy` and `would back up to` wording and writes nothing. This makes
`bob plugins sync -d` the review step before a real sync.

### Exit status

`sync` exits `0` even when it skips dirty vault files — that skip is a
deliberate warning, matching how `list` treats drift. Deploy-protection
refusals, unreadable repos, unknown `--plugin` ids, listed ids missing from
the repo, unlisted `-p` ids, lock contention, unsafe
stamp storage, and failed copies exit `1` and write the cause to stderr.
A missing `plugins.sync` list or an invalid `plugins` block exits `2` with
the setup error (table mode on stderr, JSON mode as
`{"ok": false, "error": "..."}` on stdout), as do clap usage errors.
Dry-run uses the same exit codes and writes nothing.

### JSON output

`-f json` prints a single object and no table or diffs. Pull diagnostics stay
on stderr.

```json
{
  "ok": true,
  "dry_run": false,
  "repo": "/home/bryan/projects/github/bobs-org/bob-plugins",
  "repo_source": "default",
  "bob_dir": "/home/bryan/bob",
  "copied": 1,
  "skipped": 0,
  "unchanged": 5,
  "plugins": [
    {
      "id": "bob-project-tasks",
      "files": [
        { "name": "manifest.json", "action": "unchanged", "backup": null },
        {
          "name": "main.js",
          "action": "updated",
          "backup": "/home/bryan/.local/state/bob-cli/plugin-backups/20261004-120000/bob-project-tasks/main.js"
        }
      ],
      "deployment": {
        "status": "clean",
        "source_head": "777df22f0123456789abcdef0123456789abcdef",
        "host": "athena",
        "deployed_at": "2026-10-26T18:30:00Z"
      }
    }
  ]
}
```

`repo_source` is `flag`, `enclosing_checkout`, `env`, or `default`.
`action` is `created`, `updated`, `forced`, `unchanged`, `skipped_dirty`, or
`failed`. `backup` is the backup path when the file is backed up (or, in a dry
run, would be), otherwise `null`. `copied` counts managed assets, not stamp
writes, so installer restart detection is unchanged for stamp-only
reconciliation. In a dry run, `copied` counts the files that would be copied.
`not_configured` lists every repo plugin id the `plugins.sync` allowlist
leaves out; it is always present and `[]` when everything is listed.
Each plugin entry also carries `vault_ignore` when its vault folder is not
ignored or holds tracked files (see [Vault-ignore warnings](#vault-ignore-warnings));
it is omitted otherwise. A sync that overwrites a dirty deploy through an
inclusion proof also carries `proving_commit` (the full commit SHA holding the
stamped bundle); it is omitted otherwise. On error, including a deploy-protection refusal, JSON mode prints
`{"ok": false, "error": "..."}` instead of echoing issues to stderr.

## Examples

```bash
bob plugins
bob plugins list
bob plugins list -f json
bob plugins list -b ~/bob -r ~/projects/github/bobs-org/bob-plugins
bob plugins sync --dry-run
bob plugins sync -d -f json
bob plugins sync --no-pull --dry-run
bob plugins sync -p bob-project-tasks
bob plugins sync -F -b ~/bob -r ~/projects/github/bobs-org/bob-plugins
```
