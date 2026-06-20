---
create_time: 2026-06-20
status: research
topic: Dedicated GitHub repo for Bob Obsidian plugins
---
# Research: Dedicated Repo for Bob Obsidian Plugins

## Question

Should the custom Obsidian plugins currently living under `~/bob/.obsidian/plugins/`
move into a dedicated GitHub repository, and what should the best solution look like?

## Recommendation

Create a private GitHub monorepo, probably `bbugyi200/bob-obsidian-plugins`, as the
source of truth for Bob-authored Obsidian plugins. Keep the live vault copy under
`~/bob/.obsidian/plugins/<plugin-id>/` as a generated runtime install target, populated
by a deploy script from the new repo.

This fits the current Bob setup better than one repo per plugin because the plugins are
small-to-medium, tightly coupled to the same vault conventions, and often change together
with shared task, wikilink, Vim, project, and daily-note behavior. The monorepo should
use workspaces, shared tooling, a shared test harness, and explicit per-plugin build and
deploy commands.

Do not clone the monorepo directly into `.obsidian/plugins/`. Obsidian expects each
plugin to live in its own folder with a matching `manifest.json` and `main.js`, while a
multi-plugin repo needs its own root files. Keep the source checkout somewhere like
`~/src/bob-obsidian-plugins` or `~/code/bob-obsidian-plugins`, then copy build artifacts
into `~/bob/.obsidian/plugins/<id>/`.

## Current Vault State

Memory context: `~/bob/` is the Obsidian vault, and this machine uses
`obsidian-headless` via `ob` for local workflows and Obsidian Sync support.

Observed custom plugin inventory:

| Plugin ID | Version | Runtime files | `main.js` lines | Notes |
| --- | ---: | --- | ---: | --- |
| `block-id-prompt` | 1.0.0 | `main.js`, `manifest.json` | 2220 | Block ID rename/completion workflows. |
| `bob-ledger-tools` | 1.0.0 | `main.js`, `manifest.json` | 1891 | Daily-note snippets, Pomodoro ledger ranges, daily jumps. |
| `bob-navigation-hotkeys` | 1.0.0 | `main.js`, `manifest.json`, `styles.css` | 7420 | Largest plugin; owns most note navigation and creation commands. |
| `bob-project-tasks` | 1.0.0 | `main.js`, `manifest.json` | 276 | Project task count frontmatter automation. |
| `bob-vim-surround` | 1.2.0 | `main.js`, `manifest.json` | 1146 | Vim-surround `ys`, `cs`, and `ds` support. Currently dirty in the vault. |
| `task-status-cycler` | 1.0.0 | `main.js`, `manifest.json` | 4283 | Task status cycling, task promotion/demotion, Pomodoro task helpers. |

Other installed plugins under `~/bob/.obsidian/plugins/` are third-party community
plugins and should not move into the custom plugin source repo. The vault repo currently
tracks both Bob-authored plugin bundles and third-party plugin bundles because
`~/bob/.gitignore` explicitly allows `.obsidian/**/*.json`, `.obsidian/**/*.js`,
`.obsidian/**/*.mjs`, `.obsidian/**/*.cjs`, `.obsidian/**/*.css`, and `.obsidian/**/*.md`.

`~/bob` is already a Git repo with remote `git@github.com:bbugyi200/bob.git`. Plugin
history is mixed with note/vault history. That is workable, but it means code review,
tests, releases, and refactors compete with normal note sync commits.

## External Constraints

Obsidian's plugin contract is simple but important:

- A local plugin folder is expected to contain `manifest.json`, `main.js`, and optionally
  `styles.css`.
- The plugin `id` should match the local folder name for development behavior to work
  correctly.
- Community plugin releases are GitHub-release based: Obsidian reads the root
  `manifest.json`/`README.md`, then downloads release assets `manifest.json`, `main.js`,
  and optional `styles.css`.
- The official sample plugin uses source files plus a build step that compiles
  `src/main.ts` to `main.js`.
- Official guidance says generated `main.js` belongs in releases, not as hand-maintained
  source in the repo.

Those constraints strongly favor a source repo that builds runtime assets, then installs
those assets into the vault.

## Options

| Option | Fit | Pros | Cons |
| --- | --- | --- | --- |
| Keep status quo in `~/bob` | Poor long-term | Zero migration. Obsidian loads directly. Git history already exists. | Source and runtime bundles are the same files. No package tooling. Plugin and note history are mixed. Hard to test or release cleanly. |
| One repo per plugin, cloned into each `.obsidian/plugins/<id>` folder | Good if publishing each plugin publicly | Matches Obsidian community plugin conventions. Easy per-plugin releases. | Six repos for a single personal system. Cross-plugin changes become fragmented. Shared helpers require publishing or copying a library. |
| One private monorepo outside the vault with deploy script | Best Bob fit | One source of truth. Atomic cross-plugin changes. Shared utilities/tests/tooling. Keeps vault as runtime target. | Needs a small build/deploy layer. Public community release story is less direct. |
| Git submodules in `.obsidian/plugins/<id>` | Weak | Keeps external Git history while Obsidian sees normal plugin folders. | Requires one submodule per plugin, not one monorepo. Submodules add detached-HEAD and push-order footguns. |
| Git subtree from a plugin repo into the vault | Weak | Avoids submodule checkout behavior. | Sync is manual and easy to confuse. Still duplicates source into the vault repo. |

Recommendation: use the monorepo/deploy-script option. Avoid submodules and subtrees for
active daily development.

## Proposed Repo Shape

```text
bob-obsidian-plugins/
  README.md
  package.json
  pnpm-workspace.yaml
  tsconfig.base.json
  eslint.config.mjs
  scripts/
    build-plugin.mjs
    deploy-local.mjs
    bump-plugin-version.mjs
    check-runtime-assets.mjs
  packages/
    shared/
      src/
        obsidian-links.js
        markdown-tasks.js
        vim-mode.js
      package.json
    block-id-prompt/
      manifest.json
      package.json
      src/main.js
      test/
    bob-ledger-tools/
      manifest.json
      package.json
      src/main.js
      test/
    bob-navigation-hotkeys/
      manifest.json
      package.json
      src/main.js
      styles.css
      test/
    bob-project-tasks/
      manifest.json
      package.json
      src/main.js
      test/
    bob-vim-surround/
      manifest.json
      package.json
      src/main.js
      test/
    task-status-cycler/
      manifest.json
      package.json
      src/main.js
      test/
  dist/
    <plugin-id>/
      manifest.json
      main.js
      styles.css
```

Use JavaScript first, not an immediate TypeScript rewrite. The current plugins are plain
CommonJS-style JavaScript and are actively evolving. A source move plus build/deploy
split is already a large enough change. TypeScript can be introduced plugin-by-plugin
after the repo exists and tests are in place.

Use `pnpm` or `npm` workspaces. `pnpm` is a good default if there is no existing package
manager preference because it has first-class monorepo support via `pnpm-workspace.yaml`.
`npm` workspaces would also be fine and slightly simpler if avoiding another tool matters.

## Development Workflow

1. Edit source under `bob-obsidian-plugins/packages/<plugin-id>/src/`.
2. Run focused checks:

```bash
pnpm --filter <plugin-id> check
pnpm --filter <plugin-id> test
pnpm --filter <plugin-id> build
```

3. Deploy explicit runtime files into the live vault:

```bash
pnpm deploy:local --plugin <plugin-id> --vault ~/bob
```

4. Reload Obsidian or disable/enable the plugin.
5. Commit the source repo change.
6. During the transition period, commit generated vault runtime files in `~/bob` only
   when they need to travel with the vault and Obsidian Sync/device setup.

The deploy script should copy only `manifest.json`, `main.js`, and `styles.css` when
present. It should not delete `data.json` or other runtime state files, because future
plugin settings may live there.

The deploy script should also refuse to overwrite a dirty vault plugin file unless passed
an explicit `--force` flag. This matters immediately because
`~/bob/.obsidian/plugins/bob-vim-surround/main.js` is currently modified.

## Versioning

Use independent per-plugin versions, stored in each package's `manifest.json`. Do not
force all plugins to release together. `bob-vim-surround` already has a different version
(`1.2.0`) from the rest (`1.0.0`), which is the right signal.

Suggested internal tags:

```text
block-id-prompt@1.0.1
bob-ledger-tools@1.0.1
bob-navigation-hotkeys@1.0.1
bob-project-tasks@1.0.1
bob-vim-surround@1.2.1
task-status-cycler@1.0.1
```

If any plugin is ever submitted to the official Obsidian community plugin directory,
consider splitting that plugin into its own public repo. The community release flow is
oriented around one root `manifest.json` and GitHub release tags that match the manifest
version exactly. A multi-plugin monorepo can work for private builds, but it is awkward
for official community distribution.

## Migration Plan

1. Reconcile current vault work first.
   - Decide whether to commit, stash, or intentionally carry forward the dirty
     `bob-vim-surround/main.js` change.
   - Do not run a migration script that overwrites the live vault copy before this is
     resolved.

2. Create a throwaway clone of `bbugyi200/bob`.
   - Do not run history rewriting tools inside the live `~/bob` vault.

3. Preserve plugin history with `git filter-repo`, if the extra effort is acceptable.
   - GitHub documents this as the supported path for turning a folder into a new repo.
   - For all six plugins, filter only the custom plugin paths, then rename
     `.obsidian/plugins/` to `packages/`.
   - If preserving history becomes fiddly, a clean snapshot import is acceptable because
     the original history remains in `bbugyi200/bob`.

Example sketch, to be run in a clone:

```bash
git filter-repo \
  --path .obsidian/plugins/block-id-prompt \
  --path .obsidian/plugins/bob-ledger-tools \
  --path .obsidian/plugins/bob-navigation-hotkeys \
  --path .obsidian/plugins/bob-project-tasks \
  --path .obsidian/plugins/bob-vim-surround \
  --path .obsidian/plugins/task-status-cycler \
  --path-rename .obsidian/plugins/:packages/
```

4. Convert folders from runtime layout to source layout.
   - Move `packages/<id>/main.js` to `packages/<id>/src/main.js`.
   - Keep `manifest.json` at the package root.
   - Keep `styles.css` at the package root or under `src/` with an explicit copy step.
   - Add a build step that writes `dist/<id>/main.js`.

5. Add root-level tooling.
   - `check`: syntax check, lint, manifest validation.
   - `test`: focused unit tests for parsers and pure helpers.
   - `build`: build all plugin runtime assets.
   - `deploy:local`: copy selected dist assets into `~/bob/.obsidian/plugins/<id>/`.

6. Add generated-file guardrails in the vault.
   - At minimum, add a short generated banner to deployed `main.js`.
   - Optionally update `~/bob/.gitignore` later so Bob-authored plugin bundles are not
     tracked in the vault repo. Do this only after confirming the cross-device install
     and Obsidian Sync story.

7. Move shared code only after the first extraction works.
   - Do not start by aggressively deduplicating.
   - Good first shared modules: wikilink parsing, task-line parsing, block-id parsing,
     frontmatter/property helpers, Vim-mode detection, and open-leaf reuse helpers.

## What Belongs in the New Repo

Include:

- Bob-authored plugin source, manifests, and plugin CSS.
- Shared helpers and tests.
- Build, deploy, version, and manifest-validation scripts.
- README documentation for local install, reload workflow, and release workflow.
- Notes about dependent vault config such as `obsidian_vimrc.md` and `hotkeys.json`.

Do not include:

- Third-party community plugin bundles from `.obsidian/plugins/`.
- Plugin `data.json` settings unless they are intentionally versioned defaults.
- Personal notes, generated tag pages, vault attachments, or memory files.
- `~/bob/.obsidian/hotkeys.json` as authoritative source. Keep it in the vault repo,
  but document command IDs that plugins expose.

## Key Risks

- `bob-vim-surround/main.js` is already dirty in the live vault. Treat it as real work,
  not generated output, until the migration is complete.
- A monorepo is not the cleanest shape for official Obsidian community publishing. It is
  the cleanest shape for Bob-only private development.
- Copy-based deploy creates duplicate runtime files in the vault. That is acceptable
  during transition and better than symlink surprises with Obsidian Sync, Git, or
  another machine.
- Source extraction plus TypeScript conversion in the same step would be too much churn.
  Extract first, type later.
- Hotkeys and Vim mappings span plugin code, `hotkeys.json`, and `obsidian_vimrc.md`.
  The repo should document these contracts but not blindly own all vault configuration.

## Best First Implementation Batch

1. Create private repo `bbugyi200/bob-obsidian-plugins`.
2. Import the six custom plugin folders, preserving history if practical.
3. Add workspace scaffolding and a no-op build that can reproduce the current runtime
   assets byte-for-byte or near-byte-for-byte.
4. Add `deploy:local` with dirty-file protection.
5. Prove the flow on the smallest plugin, `bob-project-tasks`.
6. Move `bob-vim-surround` only after its current dirty change is resolved.
7. Add tests around the pure parser/helper code before refactoring shared modules.

## Sources

- Local memory read: `sase memory read obsidian.md --reason "Need Bob Obsidian vault and plugin workflow context before recommending repository structure"`.
- Local vault manifests: `~/bob/.obsidian/plugins/*/manifest.json`.
- Local vault Git state: `git -C ~/bob status --short`, `git -C ~/bob ls-files .obsidian/plugins`.
- [Obsidian sample plugin README](https://raw.githubusercontent.com/obsidianmd/obsidian-sample-plugin/master/README.md).
- [Obsidian sample plugin package.json](https://raw.githubusercontent.com/obsidianmd/obsidian-sample-plugin/master/package.json).
- [Obsidian manifest reference](https://docs.obsidian.md/Reference/Manifest).
- [Obsidian releases README](https://raw.githubusercontent.com/obsidianmd/obsidian-releases/master/README.md).
- [Obsidian October plugin self-critique checklist](https://docs.obsidian.md/oo/plugin).
- [GitHub Docs: Splitting a subfolder out into a new repository](https://docs.github.com/en/get-started/using-git/splitting-a-subfolder-out-into-a-new-repository).
- [Pro Git: Submodules](https://git-scm.com/book/en/v2/Git-Tools-Submodules).
- [GitHub Docs: About Git subtree merges](https://docs.github.com/en/get-started/using-git/about-git-subtree-merges).
- [pnpm workspaces documentation](https://pnpm.io/workspaces).
