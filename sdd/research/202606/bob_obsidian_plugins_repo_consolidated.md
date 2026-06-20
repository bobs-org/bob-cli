---
create_time: 2026-06-20
status: research
topic: Consolidated research on a dedicated GitHub repo for Bob Obsidian plugins
---
# Research: Dedicated Repo for Bob Obsidian Plugins

## Question

Should the custom Obsidian plugins currently living under
`~/bob/.obsidian/plugins/` move into a dedicated GitHub repository, and what
should the best solution look like?

## Answer

Yes. Create one private GitHub monorepo, probably
`bbugyi200/bob-obsidian-plugins`, and make it the source of truth for the six
Bryan-authored plugins. Keep the live vault folders under
`~/bob/.obsidian/plugins/<plugin-id>/` as installation targets populated from
that repo.

The best first version should stay close to the current system:

- keep the plugins as plain JavaScript;
- keep each plugin folder in the same shape Obsidian loads:
  `manifest.json`, `main.js`, and optional `styles.css`;
- add lightweight validation and linting, but no TypeScript or bundling yet;
- deploy into the vault with a small copy-sync command, ideally
  `bob obsidian-plugins sync`;
- split a plugin into its own public repo only if it is later published through
  BRAT or the official Obsidian community plugin directory.

That resolves the main conflict in the earlier notes: a build/dist layer may be
useful later, but the current `main.js` files are the source, not generated
artifacts. The first migration should separate code from notes without also
rewriting the development model.

## Verified Current State

Checked on 2026-06-20.

Six custom plugins have `author: Bryan` and are enabled in
`~/bob/.obsidian/community-plugins.json`:

| Folder | Name | Version | Files | `main.js` LOC |
| --- | --- | ---: | --- | ---: |
| `block-id-prompt` | Block ID Prompt | 1.0.0 | `manifest.json`, `main.js` | 2,220 |
| `bob-ledger-tools` | Bob Ledger Tools | 1.0.0 | `manifest.json`, `main.js` | 1,891 |
| `bob-navigation-hotkeys` | Bob Navigation Hotkeys | 1.0.0 | `manifest.json`, `main.js`, `styles.css` | 7,420 |
| `bob-project-tasks` | Bob Project Tasks | 1.0.0 | `manifest.json`, `main.js` | 276 |
| `bob-vim-surround` | Bob Vim Surround | 1.2.0 | `manifest.json`, `main.js` | 1,146 |
| `task-status-cycler` | Task Status Cycler | 1.0.0 | `manifest.json`, `main.js` | 4,283 |

Total plugin code is 17,236 lines of JavaScript. The custom folders contain no
`package.json`, `tsconfig.json`, esbuild config, sourcemaps, or TypeScript
source. They are CommonJS-style Obsidian plugins using `require(...)` and
`module.exports`.

Other installed plugins, such as Dataview, Tasks, Templater, QuickAdd,
Metadata Menu, Linter, Vimrc Support, and Relative Line Numbers, are
third-party bundles and should not move into the custom plugin repo.

The vault itself is a Git repo with remote `git@github.com:bbugyi200/bob.git`.
Its `.gitignore` currently allows `.obsidian/**/*.js`, `.json`, and `.css`, so
both custom plugin bundles and third-party plugin bundles are tracked with the
notes. `~/bob/.obsidian/sync.json` is ignored and was not present in this
working tree; regardless of whether Obsidian Sync is used on another machine,
symlink-based plugin installs should be treated as machine-local.

One important migration blocker is already present:
`~/bob/.obsidian/plugins/bob-vim-surround/main.js` is modified in the vault.
Resolve that change before any script overwrites vault plugin files.

## Constraints

- Obsidian loads a plugin from
  `<vault>/.obsidian/plugins/<plugin-id>/manifest.json` and `main.js`, with
  `styles.css` optional. The manifest `id` should match the folder name for
  local development behavior.
- Official community-plugin distribution expects a root `manifest.json` and
  `README.md` in the plugin repo, plus GitHub release assets containing
  `manifest.json`, `main.js`, and optional `styles.css`. The release tag must
  match the manifest version.
- The community plugin registry maps one plugin id to one GitHub repository.
  A multi-plugin monorepo is therefore a poor fit for direct official
  publishing.
- BRAT also centers on GitHub releases and release assets. Recent BRAT behavior
  is more release-driven than older `manifest-beta.json` workflows, but a
  multi-plugin monorepo is still awkward for reliable auto-update because one
  repository has one release stream and plugin versions collide.
- The new repo and the vault must not both be edited as sources of truth. One
  side should own the code; the other should be generated, ignored, or
  deliberately transitional.

## Recommendation

Use a single private monorepo for personal Bob development.

Suggested shape:

```text
bob-obsidian-plugins/
  README.md
  LICENSE
  package.json
  scripts/
    validate-manifests.mjs
    sync-local.mjs
  plugins/
    block-id-prompt/
      manifest.json
      main.js
    bob-ledger-tools/
      manifest.json
      main.js
    bob-navigation-hotkeys/
      manifest.json
      main.js
      styles.css
    bob-project-tasks/
      manifest.json
      main.js
    bob-vim-surround/
      manifest.json
      main.js
    task-status-cycler/
      manifest.json
      main.js
```

`package.json` is for repo tooling, not for bundling. Start with checks that
provide immediate value:

- every manifest parses and required fields are present;
- each manifest `id` matches its folder;
- every plugin version is valid `x.y.z`;
- every `main.js` parses under Node;
- optional ESLint for obvious JavaScript mistakes.

Do not add TypeScript or shared packages in the first extraction. The current
duplication between plugins is intentional in places, and moving source first
keeps the migration reviewable. After the repo exists and tests cover pure
helpers, consider extracting shared parsing code for wikilinks, task lines,
block IDs, frontmatter, Vim-mode access, and leaf reuse.

## Vault Sync

Prefer a copy-based deploy loop:

```bash
bob obsidian-plugins sync --repo ~/src/bob-obsidian-plugins --vault ~/bob
bob obsidian-plugins sync --plugin bob-project-tasks --vault ~/bob
```

The command can live in `bob` because the existing CLI is already vault-aware.
If this becomes an implementation task, read `memory/cli_rules.md` first as
required by the repo instructions for new CLI subcommands or options.

Minimum behavior for the sync command:

- copy only `manifest.json`, `main.js`, and `styles.css` if present;
- preserve `data.json` and any runtime settings in the vault;
- refuse to overwrite dirty tracked vault files unless `--force` is passed;
- support `--dry-run` and a status/check mode;
- optionally install all six plugins or a single named plugin.

After the new repo is trusted, add the six custom plugin folders to the vault
ignore rules and remove them from the vault index with `git rm --cached`. That
makes the monorepo the only source of truth. During transition, it is acceptable
to keep generated runtime files tracked in the vault briefly, but that should
be explicit and short-lived to avoid drift.

Symlinks are useful for one desktop and a fast edit loop, but they are not the
best default. They do not travel cleanly through Git, do not help machines that
do not have the monorepo clone at the same path, and can behave differently
across Obsidian platforms. Use symlinks only as a local development shortcut.

Avoid Git submodules and subtrees for the active daily workflow. They add
coordination overhead while still leaving Obsidian's required runtime files
inside the vault.

## Release and Publishing Strategy

Keep independent versions in each plugin's `manifest.json`; do not force all
six plugins to version together. `bob-vim-surround` is already at `1.2.0` while
the others are at `1.0.0`, which is the right pattern.

For private monorepo releases, use explicit per-plugin tags such as:

```text
block-id-prompt/1.0.1
bob-vim-surround/1.2.1
task-status-cycler/1.0.1
```

Treat those as internal/manual-install releases, not official Obsidian or BRAT
auto-update releases.

If a plugin should be shared publicly, split that plugin into its own repo.
That repo should follow the normal Obsidian shape with a root `manifest.json`,
root `README.md`, root `LICENSE`, and GitHub releases whose tags exactly match
manifest versions. Good public candidates are `bob-vim-surround` and
`block-id-prompt`; the ledger, navigation, project, and task-status plugins are
more tightly coupled to Bob conventions.

## Migration Plan

1. Reconcile live vault changes first, especially the dirty
   `bob-vim-surround/main.js`.
2. Decide whether preserving history is worth the setup cost. If yes, run
   `git filter-repo` or `git subtree split` from a throwaway clone of the vault,
   not from live `~/bob`. If no, snapshot-import the six folders.
3. Create the private `bob-obsidian-plugins` repo and import only the six
   Bryan-authored plugin folders.
4. Add README, license, manifest validation, syntax checks, and optional ESLint.
5. Implement the copy-sync path and test it on `bob-project-tasks` first
   because it is the smallest plugin.
6. Test all six plugins in Obsidian after syncing, then commit source changes
   only in the new repo.
7. Update the vault ignore/index state so generated plugin runtime files do not
   continue to masquerade as source.
8. Add tests and shared helpers only after the extraction is stable.

Example history-preserving filter from a throwaway clone:

```bash
git filter-repo \
  --path .obsidian/plugins/block-id-prompt \
  --path .obsidian/plugins/bob-ledger-tools \
  --path .obsidian/plugins/bob-navigation-hotkeys \
  --path .obsidian/plugins/bob-project-tasks \
  --path .obsidian/plugins/bob-vim-surround \
  --path .obsidian/plugins/task-status-cycler \
  --path-rename .obsidian/plugins/:plugins/
```

## What Not To Move

Do not put these in the new plugin repo:

- third-party community plugin bundles;
- plugin `data.json` files unless a file is intentionally a versioned default;
- personal notes, attachments, generated tag pages, or memory files;
- `~/bob/.obsidian/hotkeys.json` as authoritative source.

Do document command IDs and expectations that live outside plugin code, such as
hotkey bindings, `obsidian_vimrc.md` dependencies, daily-note conventions,
project frontmatter conventions, and task/Pomodoro formats.

## Open Questions

- Is the dedicated repo intended to stay private, or should any plugin be
  published? This decides whether per-plugin split repos matter soon.
- Should plugin history be preserved, or is a clean snapshot enough?
- Should multi-machine setup require cloning the new repo everywhere, or should
  the vault temporarily keep generated runtime files until the rollout is
  proven?
- Should the first sync tool be a `bob` subcommand, a standalone script, or a
  `just`/`make` target?

## Sources

Local sources:

- Audited memory read: `sase memory read obsidian.md`.
- Prior agent transcripts:
  `~/.sase/chats/202606/bob_cli-ace_run-260620_101024.md` and
  `~/.sase/chats/202606/bob_cli-ace_run-260620_101026.md`.
- Local vault manifests and files under `~/bob/.obsidian/plugins/`.
- Local vault Git state from `git -C ~/bob status --short`,
  `git -C ~/bob remote -v`, and `git -C ~/bob ls-files .obsidian/plugins`.

External sources:

- [Obsidian sample plugin](https://github.com/obsidianmd/obsidian-sample-plugin)
- [Obsidian submit-your-plugin docs](https://docs.obsidian.md/Plugins/Releasing/Submit+your+plugin)
- [Obsidian manifest reference](https://docs.obsidian.md/Reference/Manifest)
- [Obsidian versions reference](https://docs.obsidian.md/Reference/Versions)
- [obsidianmd/obsidian-releases](https://github.com/obsidianmd/obsidian-releases)
- [BRAT guide for plugin developers](https://tfthacker.com/brat-developers)
- [Obsidian forum: Plugin Monorepo](https://forum.obsidian.md/t/plugin-monorepo/90167)
- [GitHub: splitting a subfolder out into a new repository](https://docs.github.com/en/get-started/using-git/splitting-a-subfolder-out-into-a-new-repository)
