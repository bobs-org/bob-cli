---
title: Build the docs
description: Generate and build Bob's documentation site from this repository.
---

# Build the docs

Run these recipes from the bob-cli repository root. `uv` creates and uses the locked
environment under `tools/docs/.venv`; generated files and the built site stay in the
repository's ignored `target/docs/` directory.

## Build and test each repository

The documentation source is split across three repositories. Use the commands from the
repository that owns the source you changed.

### bob-cli

Run the canonical Rust gate from the repository root:

```bash
just check
```

This checks Rust formatting, Clippy, and all Rust tests. For Rust API documentation and
doctests, also run:

```bash
cargo doc --locked --no-deps
cargo test --locked --doc
```

The documentation package has its own checks below; it does not replace `just check`.

### bob-plugins

The plugin source of truth is the `bob-plugins` repository. For plugins that use
`src/fragments.json`, rebuild generated `main.js` files after changing a fragment:

```bash
npm run build
```

Then verify the bundles and plugin code:

```bash
npm run build:check
npm test
npm run validate
```

`npm test` and `npm run validate` check generated bundles before their other checks.
Generated entrypoints are committed outputs; do not edit them by hand. Plugin changes
are deployed to the vault with `bob plugins sync` as described in the plugin
repository's README.

### bob-mac-capture

The native menu-bar app lives in the `bob-mac-capture` repository. On macOS with Xcode,
run:

```bash
just format-lint
just build
just test
```

`just bundle` additionally assembles a local app bundle. On a host without Xcode, use
`just portable-check` for the portable Swift packages; it cannot type-check the macOS
app targets. See that repository's README for signing and installation steps.

### Documentation package

The tools and authored pages live in bob-cli. The documentation-specific commands below
build this site and check its contracts. They do not install Bob or change a vault.

Regenerate the contract index, navigation, README table, By component index, and
mounted plugin inventory after changing the contract registry, guide or concept
tags, or plugin facts:

```bash
just docs-gen
```

Serve the site locally with live reload:

```bash
just docs-serve
```

The server mirrors authored `docs/` edits into the staged tree while it runs,
preserving the staged `build.json`; `mkdocs.yml` and mount-table changes need
a restart.

Build the strict Zensical site:

```bash
just docs-build
```

Build the same configuration with the Material 9.7 fallback:

```bash
just docs-build-material
```

Run the full documentation gate after editing docs, the registry, generators, or the
build toolchain:

```bash
just docs-check
```

The gate assembles source context once, then checks the docs contract, generated-file
drift including the plugin inventory, a strict Zensical build, published URLs, offline
links, search smoke cases, the executable tutorial, CLI media determinism, and the
documentation package tests. For a pinned production build, use
`uv run --project tools/docs bob-docs check --production --sources FILE`; production
requires an explicit clean SHA for bob-cli and every mounted source and never
rediscovers plugin checkouts from the environment or cache.

Generate the local fonts, identity assets, and CLI captures after changing the bundled
sources, the mark geometry, or Bob's human output:

```bash
just docs-fonts    # subset OFL fonts to WOFF2 with --group fonts
just docs-assets   # mark, favicons, and social card
just docs-media    # PTY-capture Bob and render docs/assets/media/cli/*.svg
```

Those commands accept `-c/--check` to compare without writing (`just docs-media` is the
write form; check with `uv run --project tools/docs bob-docs media -c`). The WOFF2 total
stays under 450,000 bytes; record the sizes when regenerating. Media `--check` renders
twice and must not refresh `docs/_meta/media.yml`.

Audit the built site for accessibility after theme or homepage changes:

```bash
just docs-a11y
```

Install browsers once with
`uv run --project tools/docs --group a11y playwright install chromium`.
Audits are dev-only and are not part of `just docs-check`.

The generated site is written to `target/docs/site/`. Building does not regenerate
tracked pages; run `just docs-gen`, `just docs-fonts`, `just docs-assets`, or
`just docs-media` explicitly and review their changes.

## Publish the public site

The live site is `https://bobs-org.github.io/`. Publication logic lives in this
repository; `bobs-org/bobs-org.github.io` holds only the caller workflow, a
README, and the durable `published.json` log. That log is not assumed to be
served as `/published.json`. Live provenance is `/build.json`.

Preview host bootstrap, then apply it only when you intend to create or resync
the deploy repository. Bootstrap never writes `published.json`.

```bash
just docs-host-bootstrap
just -- docs-host-bootstrap --yes
```

Pass `--deploy-dir` when an agent already opened the deploy checkout:

```bash
just -- docs-host-bootstrap --yes --deploy-dir /path/to/bobs-org.github.io
```

Dispatch a publish and watch the run that dispatch started. The command enables
the workflow first so a 60-day inactivity disable does not block it.

```bash
just docs-publish
just -- docs-publish --force
just -- docs-publish --drill
just -- docs-publish --pinned
just docs-status
```

`just docs-status` reports the host manifest, live `/build.json`, recent
publish and weekly runs, workflow state including `disabled_inactivity`, the
hold flag, and open managed blocked issues. A missing first site is a clear
status. GitHub authentication failure is an error.

### Rollback with held pinned SHAs

Hand-editing source SHAs in `published.json` is rejected: the planner rebuilds
the inputs at those SHAs and requires them to match the stored fingerprint.
Restore a previous recorded manifest whole instead.

1. In the deploy repository, find the manifest commit to roll back to:

   ```bash
   git log --format='%h %ad %s' -- published.json
   ```

   Each `Record documentation publication` commit names its run.
2. Restore that record whole: `git show <commit>:published.json > published.json`.
   Then set `"hold": true`, commit only `published.json`, and push.
3. Dispatch a pinned publish:

   ```bash
   just -- docs-publish --pinned
   ```

   Pinned builds those exact SHAs and keeps the hold. Scheduled and unpinned
   runs, including `--force`, skip while held.
4. Release the hold by setting `"hold": false`, commit, and push. The next
   ordinary run publishes the current heads.

### Recovery

A failing publish opens or updates one `Docs publish blocked` issue
(`docs-publish`). The next ordinary dispatch sees that issue and re-runs the
gates even when the fingerprint is unchanged:

```bash
just docs-publish
```

Do not use `--force` to recover. Force rebuilds unchanged inputs when nothing
is held; recovery is the blocked-issue path. A successful live verification
closes the issue.

### Inactivity

GitHub disables scheduled workflows in a public repository after 60 days with
no repository activity (`disabled_inactivity`). No-op publish runs commit
nothing, so they do not count as activity. `just docs-publish` enables the
workflow before dispatch. `just docs-status` reports that state.

### Failure boundaries

Before deploy, a planner skip, a held unpinned run, or a gate failure uploads
nothing and does not rewrite `published.json`. The last good site and manifest
stay untouched. `--drill` runs the real gates against a staged-only broken
link, must fail before upload, and opens the blocked issue.

After the gates pass, upload, Pages deploy, recording `published.json`, and
live verification run in that order. Only failures before deploy promise that
the last good site and manifest are untouched.

- A Pages deploy failure normally leaves the previous deployment serving, but
  the workflow does not prove it. Check `just docs-status`, which reads live
  `/build.json`.
- A record failure happens after a successful deploy. The new site is live, but
  `published.json` still names the previous publication. This includes a
  compare-and-swap rejection: `record` refuses to overwrite a deploy-repo
  change made after planning.
- A live-verification failure means the new site may be serving.

All of these keep the blocked issue open. The next ordinary run takes the
recovery path: it rebuilds, deploys, records, verifies, and closes the issue.

### Weekly checks

The `weekly.yml` caller runs on `41 6 * * 1` and manual dispatch. It always
builds the strict Material site with the locked material group and runs
`bob-docs links --online` with bounded retries and timeouts, even when the
publication fingerprint is unchanged or a publish hold remains. Weekly cannot
release the hold or deploy, and it never writes `published.json`. A failure
opens or updates one `Docs weekly checks blocked` issue (`docs-weekly`); a
passing run closes only that issue.
