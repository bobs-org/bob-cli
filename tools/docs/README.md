# bob-docs

Build and validate Bob's documentation site. Run from the bob-cli repository root:

The docs linter inventories complete `BOB_*` identifiers in Rust string
literals under production `src/` code and checks them against
`docs/reference/configuration.md`. Its lexical scan respects Rust strings,
raw strings, nested comments, and item braces; it excludes Rust test source
files and items gated by `#[cfg(test)]`, including inline test modules, while
continuing through later production items. It intentionally does not resolve
which strings reach an environment reader, so historic help text and private
process controls need exact, specific reasons in
`docs/_meta/environment_ignore.yml`. Public runtime controls belong on the
configuration page instead of that exception list.

```bash
just docs-gen             # regenerate tracked contract indexes
just docs-serve           # assemble and serve locally with live reload
just docs-build           # strict Zensical build of the assembled site
just docs-build-material  # strict Material 9.7 fallback build
just docs-check           # lint, drift, build, links, search smoke, and tests
just docs-tutorial        # execute the published first-day Bash tutorial in isolation
just docs-fonts           # subset bundled OFL fonts to WOFF2 (needs --group fonts)
just docs-assets          # generate identity assets: mark, favicons, social card
just docs-a11y            # Playwright + axe audit of the built site (dev only)
uv run --project tools/docs bob-docs lint
uv run --project tools/docs bob-docs lint --fix   # ratchet ceilings / privacy baseline
uv run --project tools/docs bob-docs tutorial     # direct tutorial replay
```

Install audit browsers once with
`uv run --project tools/docs --group a11y playwright install chromium`.
The audit injects the vendored `tools/docs/vendor/axe.min.js` (axe-core 4.14.0,
MPL-2.0) locally; it never downloads an unpinned CDN script at runtime.

`bob-docs check --production --sources FILE` runs the same gates using one explicit,
clean, SHA-pinned source set for bob-cli and every mounted repository.

## Publication planning (`bob-docs publish-plan`)

`bob-docs publish-plan` reads a durable `published.json` manifest, resolves immutable
source commits, and writes transient plan files. The command never updates the durable
manifest. For example, a workflow can run:

```bash
bob-docs publish-plan \
  --manifest ../deploy/published.json \
  --output-dir target/docs/publish-plan \
  --work-dir target/docs/publish-work \
  --github-output "$GITHUB_OUTPUT"
```

The `--output-dir` contains:

- `publish-plan.json`: `should_build`, `should_deploy`, `should_record`, a reason,
  selected sources, the input fingerprint, and paths to the other outputs.
- `production-sources.json`: exactly the `{"sources": {repo: {"path", "sha"}}}`
  shape accepted by `bob-docs check --production --sources ...`. Each path resolves to
  a dedicated clean checkout pinned to that SHA, never the tooling or developer
  checkout.
- `next-publication.json`: a transient candidate containing `schema_version`, `hold`,
  source identities, input records, and the fingerprint. After a successful deploy,
  the workflow adds `tools`, `built_at`, and `run_url` before committing it as
  `published.json`.

The durable version 1 `published.json` record has this shape:

```json
{
  "schema_version": 1,
  "hold": false,
  "sources": {
    "bob-cli": {
      "remote": "https://github.com/bobs-org/bob-cli.git",
      "branch": "master",
      "sha": "<40-character commit SHA>"
    }
  },
  "inputs": [
    {
      "repo": "bob-cli",
      "path": "docs",
      "kind": "tree",
      "object_id": "<40-character Git object ID>"
    }
  ],
  "fingerprint": "<64-character SHA-256>",
  "tools": {"bob-docs": "0.1.0", "zensical": "0.0.71"},
  "built_at": "2026-10-20T00:00:00Z",
  "run_url": "https://github.com/bobs-org/bob-cli/actions/runs/42"
}
```

Mounted input records also include `target`, so two mounts from one repository remain
distinct. Fingerprints use sorted input identities and Git tree/blob IDs only; checkout
paths, mtimes, and poll timestamps do not affect them. The inputs are bob-cli `docs`,
`mkdocs.yml`, `tools/docs`, every `.github/workflows/docs-*.yml`, and every mounted
source directory. Code-only commits that leave those trees unchanged do not publish.

The manifest must be valid and versioned; missing state means first publication, while
malformed or unsupported state fails closed. A hold skips every unpinned run, including
force, recovery, and drill. `--pinned` builds the exact recorded SHAs and preserves the
hold. `--drill` runs the gates but marks both deploy and record false. `--force` and
`--recovery` rebuild equal inputs when no unpinned hold applies. When `GITHUB_OUTPUT` is
set (or `--github-output` is passed), the planner writes stable `should_build`,
`should_deploy`, `should_record`, `reason`, `fingerprint`, `sources_file`, and
`next_publication_file` values for workflow conditions.

Assembly never edits tracked sources. Generated output lives under `target/docs/`:
the assembled `mkdocs.yml`, `build.json` provenance manifest, and the built site.
Tracked files are written only by `just docs-gen`, the explicit asset generators
(`just docs-fonts` for WOFF2, `just docs-assets` for the mark, favicons, and
social card), and explicit `bob-docs lint --fix` ceiling/baseline ratchets.
Normal lint never writes. Browser audits (`just docs-a11y`) are dev-only and are
not part of `just docs-check`.

## Executable tutorial (`bob-docs tutorial`)

The tutorial runner follows the Get started navigation order and executes each
published Bash fence in a persistent shell. It copies `examples/demo-vault` into
a temporary workspace, isolates `HOME`, XDG directories, and the Bob environment,
and verifies state at each `tutorial-test: checkpoint` marker. Use
`just docs-tutorial` to run it directly. It needs Cargo to build this checkout or
an explicit executable path in `BOB_BIN`; without either, the docs-check gate
reports a capability skip. A direct tutorial run fails with an actionable error.

In Get started pages, every unmarked Bash fence executes. A
`<!-- tutorial-test: skip reason -->` immediately before one Bash fence skips
that fence and reports its source and reason. A
`<!-- tutorial-test: checkpoint id -->` immediately after a Bash fence checks
the effects before the next fence runs. Keep network, install, and GUI steps
complete for readers, and mark them skipped only when replay cannot safely
exercise the step. The runner rejects malformed, unknown, unused, and missing
required checkpoint markers, and rejects start pages omitted from navigation.

## Task fences (`bob-task`)

Static, accessible task examples use `bob-task` fences. `today` is an explicit
illustration flag for open rows, never inferred from dates:

````markdown
```bob-task
- [ ] #task Review the queue input [created:: 2026-10-20]
```

```bob-task today
- [*] #task Fix the parser crash ^fix
```
````

Statuses render with accessible names (Ready, Next, In Progress, Blocked, Done,
Cancelled). An exact `#task` token becomes the teal identity mark; `#prj`
renders violet and other tags render as pills. Canonical date fields render as
absolute monochrome marks with the year; malformed or duplicate fields stay
literal. Unknown lines appear as escaped text and never fail the build.

## Mount table (`docs/_meta/mounts.yml`)

The table starts empty. Each entry mounts one source directory from a sibling
repository into the site:

```yaml
- repo: bob-plugins
  remote: https://github.com/bobs-org/bob-plugins.git
  branch: main
  source: docs
  target: reference/plugins
  nav_title: Obsidian plugins
```

A repo may have several mounts (for example `docs/` → `reference/plugins/` and
`docs/develop/` → `develop/plugins/`). When source directories nest, the assembler
partitions files by the most specific source root, so `docs/develop/` pages are
copied only to `develop/plugins/` and never duplicated under `reference/plugins/`.

Sibling discovery resolves each unique repo once, in this order:

1. `$BOB_DOCS_<REPO>_DIR` (uppercased, non-alphanumerics become `_`; for example
   `$BOB_DOCS_BOB_PLUGINS_DIR`). An explicitly set override must be a Git working
   tree rooted at that path, with a resolvable HEAD and an origin matching the
   mount remote. SSH and HTTPS spellings of the same GitHub repository match;
   `file://` and local paths to the same directory match. An override that exists
   but is not that source fails immediately and never falls through.
2. `../<repo>` (the `install-all` checkout layout), with the same working-tree
   and origin checks.
3. `sase/repos/linked/<repo>` (agent workspaces), with the same checks.
4. A shallow branch clone of `remote` into `~/.cache/bob-docs/<repo>`
   (`$XDG_CACHE_HOME/bob-docs/<repo>` when set). An existing cache is identity-
   checked, then shallow-fetched to the requested branch and hard-reset to the
   fetched commit. Only a genuine connectivity failure keeps a previously
   validated cache, with an explicit warning; a missing branch, authentication
   failure, missing repository, or other Git error fails closed. Discovery never
   fetches or resets user-owned override, sibling, or linked worktrees.

A repo that resolves nowhere fails with the full list of attempted locations.

## Mounted sources

Validation runs before any write, so a failure leaves the tree unchanged:

- `source` must stay inside its repo; `target` must stay beneath `docs/`;
  targets must not overlap.
- Symlinks pointing outside their repo are rejected; nothing is created as a
  symlink at the destination.
- Hub ownership has two compatible forms. When the selected source has no
  `index.md`, bob-cli keeps a committed `<target>/index.md` landing page and
  gitignores every other copied path. When the source supplies `index.md`,
  that file is the hub: it is copied and stamped like any other page, and the
  destination including `index.md` must be gitignored and untracked. A
  previously copied hub may be refreshed or removed through the ownership
  manifest. An authored landing page is never replaced.
- Every copied destination path must itself be gitignored. Repeat assembly
  overwrites only previously copied outputs, removes stale copied pages, and
  refuses to overwrite any other file.
- `nav.yml` and `media.yml` are support metadata, never public pages, and are
  not copied. Dotfiles are not copied.
- Every copied Markdown page gets `source: <repo>:<path>` (repo-relative path)
  stamped into its front matter; a source page that already sets `source` is
  rejected as spoofing. Other files are copied byte-identical without link
  rewriting — links that cross mount boundaries must therefore use absolute
  site URLs, which the URL gate validates.

## Mount navigation

Each mount ships a `nav.yml` next to its source `index.md` listing its pages in
order; paths are relative to the source root:

```yaml
- title: Setup
  path: setup.md
```

Titles and order are preserved, paths are qualified with the target, and
escaping, missing, duplicate, or non-Markdown entries fail. `index.md` must not
be listed. The hub, either the source `index.md` or the committed bob-cli
landing page, is included exactly once, first, under the mount's `nav_title`.
Source hubs are copied and stamped before their titles are read. The authored
`mkdocs.yml` holds one placeholder line per mount
(`- Plugins: reference/plugins/index.md`); assembly replaces it with the
expanded section. A mount with no placeholder, or a placeholder with no mount
and no page, fails the build.

## Production assembly (`--production --sources FILE`)

Publish builds pin every source explicitly and refuse discovery:

```bash
bob-docs assemble --production --sources docs/_meta/production-sources.json
bob-docs build --production --sources docs/_meta/production-sources.json
```

The JSON file has this shape; relative paths resolve against the JSON file:

```json
{
  "sources": {
    "bob-cli": {"path": "../..", "sha": "<40-hex HEAD>"},
    "bob-plugins": {"path": "../../../bob-plugins", "sha": "<40-hex HEAD>"}
  }
}
```

Entries are required for `bob-cli` and every mounted repo — no more, no fewer.
The `bob-cli` path must resolve to the actual build root. Every source must be
a git checkout whose exact `HEAD` equals the recorded SHA with a clean
tracked/untracked state (ignored build outputs do not count). Any
`$BOB_DOCS_*_DIR` override in the environment fails production outright, and
production never clones, fetches, or changes its inputs. Cleanliness is checked
again after mount copies land.

## Provenance (`target/docs/build.json`)

Each assembly writes repo-keyed SHAs, dirty flags, branches, the UTC build
time, the builder used (`zensical` or `material`), and installed tool versions,
and injects them plus the contract registry into the assembled config as
`extra.build` / `extra.contracts`. The same payload is also written into the
staged docs tree, so the site build copies the manifest to `/build.json`. Build
time may change between runs; generated tracked content stays deterministic.

## Serve (`just docs-serve`)

`bob-docs serve` assembles once, then serves the staged tree with Zensical.
While the server runs, a background thread mirrors authored `docs/` edits into
the staged tree about twice a second: additions and edits appear, deletions
disappear, file-to-directory and directory-to-file replacements succeed,
escaping symlinks are rejected before any staged mutation, and the staged
`build.json` plus anything else assembly wrote is preserved. Restart the server
to pick up `mkdocs.yml` or mount-table changes; those never mirror.

## Published URLs (`bob-docs urls`)

`bob-docs urls` verifies every published-URL obligation against the built
site (run `just docs-build` first so `target/docs/site/` exists):

1. It writes sorted `target/docs/site/urls.txt` from the built content and
   redirect pages (support outputs and the error page excluded), then compares
   the live org-root `/urls.txt` against the new build: every previously
   published page URL must still resolve. An unreachable site or a 404 warns
   explicitly (offline machine, first publish); a reachable but malformed list
   or a missing previously published page fails.
2. It scans every literal `docs/<path>.md(#fragment)` mention in UTF-8 text
   under `src/` and `tests/` (skipping undecodable files and build/vendor
   directories), the root `README.md`, and `docs/`, plus every absolute site
   URL in the assembled docs and each discovered source repo's README, and
   resolves each against an index built from the actual output HTML paths and
   `id`/`name` anchors. Directory URLs, index pages, percent-encoded
   paths/fragments, and redirects (with bounded loop detection) resolve; path
   escapes and broken targets fail with `path:line:` provenance.
3. It resolves every curated absolute site URL in
   `docs/_meta/url_contract.txt`.

`docs/_meta/url_mentions_ignore.txt` holds narrowly scoped
`MATCH | SCOPE | REASON` exclusions for demonstrated non-public references
(the excluded repository hub, fixture-vault paths). MATCH must name a page
(a `docs/<page>.md` path, or an absolute site URL with a page path). SCOPE
must name files: no `*`, no `**`, no wildcard-only globs, and no trailing
`/`. Never exclude a real path printed by bob, a broken anchor, or an entire
subtree; stale entries fail. `bob-docs urls --refresh` (`-r`) applies those
ignores first, then validates the contract and appends newly observed
resolving site URLs deterministically; it never deletes obligations.

## Offline link check (`bob-docs links`)

`bob-docs links` checks the built site with a pinned lychee release:

```bash
lychee --offline --no-progress --include-fragments --index-files index.html \
  --root-dir <site> --remap "https://bobs-org.github.io/ file://<site>/" <site>
```

Lychee resolves from `$LYCHEE`, then `PATH`, then `docs/_meta/lychee.yml`'s
pinned release (Linux x86_64/aarch64, macOS arm64), downloaded into
`~/.cache/bob-docs/bin/` and SHA-256 verified before extraction or execution.
A missing or corrupt checker fails; it is never a green skip. Offline
fragment checking uses `--include-fragments --index-files index.html` so
directory URLs such as `/page/#anchor` resolve against each directory's
`index.html`. `bob-docs urls` still owns mention, curated, and live-URL
resolution. Online external checks belong to the weekly publish phase, not
this gate.
