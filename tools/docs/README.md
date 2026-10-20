# bob-docs

Build and validate Bob's documentation site. Run from the bob-cli repository root:

```bash
just docs-gen             # regenerate tracked contract indexes
just docs-serve            # assemble and serve locally with live reload
just docs-build            # strict Zensical build of the assembled site
just docs-build-material   # strict Material 9.7 fallback build
just docs-check            # lint, drift, build, links, search smoke, and tests
uv run --project tools/docs bob-docs lint
uv run --project tools/docs bob-docs lint --fix   # ratchet ceilings / privacy baseline
```

`bob-docs check --production --sources FILE` runs the same gates using one explicit,
clean, SHA-pinned source set for bob-cli and every mounted repository.

Assembly never edits tracked sources. Generated output lives under `target/docs/`:
the assembled `mkdocs.yml`, `build.json` provenance manifest, and the built site.
Only `just docs-gen` and explicit `bob-docs lint --fix` ratchet updates modify
tracked files. Normal lint never writes.

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
   `$BOB_DOCS_BOB_PLUGINS_DIR`). An explicitly set override that is not a usable
   directory fails immediately; it never falls through to the next location.
2. `../<repo>` (the `install-all` checkout layout).
3. `sase/repos/linked/<repo>` (agent workspaces).
4. A shallow branch clone of `remote` into `~/.cache/bob-docs/<repo>`
   (`$XDG_CACHE_HOME/bob-docs/<repo>` when set).

A repo that resolves nowhere fails with the full list of attempted locations.

## Mounted sources

Validation runs before any write, so a failure leaves the tree unchanged:

- `source` must stay inside its repo; `target` must stay beneath `docs/`;
  targets must not overlap.
- Symlinks pointing outside their repo are rejected; nothing is created as a
  symlink at the destination.
- The target directory is gitignored apart from its committed `index.md`
  landing page. Repeat assembly overwrites only previously copied outputs,
  removes stale copied pages, and refuses to overwrite any other file.
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
be listed: the committed `<target>/index.md` landing page is always included
exactly once, first, under the mount's `nav_title`. The authored `mkdocs.yml`
holds one placeholder line per mount (`- Plugins: reference/plugins/index.md`);
assembly replaces it with the expanded section. A mount with no placeholder,
or a placeholder with no mount and no page, fails the build.

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
disappear, escaping symlinks are rejected, and the staged `build.json` plus
anything else assembly wrote is preserved. Restart the server to pick up
`mkdocs.yml` or mount-table changes; those never mirror.

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
