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

Regenerate the contract index, navigation, and README table after changing the contract
registry:

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
drift, a strict Zensical build, published URLs, offline links, search smoke cases, and
the documentation package tests. For a pinned production build, use
`uv run --project tools/docs bob-docs check --production --sources FILE`; production
requires an explicit clean SHA for bob-cli and every mounted source.

Generate the local fonts and identity assets after changing the bundled sources or the
mark geometry:

```bash
just docs-fonts    # subset OFL fonts to WOFF2 with --group fonts
just docs-assets   # mark, favicons, and social card
```

Both commands accept `-c/--check` to compare without writing. The WOFF2 total stays
under 450,000 bytes; record the sizes when regenerating.

Audit the built site for accessibility after theme or homepage changes:

```bash
just docs-a11y
```

Install browsers once with
`uv run --project tools/docs --group a11y playwright install chromium`.
Audits are dev-only and are not part of `just docs-check`.

The generated site is written to `target/docs/site/`. Building does not regenerate
tracked pages; run `just docs-gen`, `just docs-fonts`, or `just docs-assets`
explicitly and review their changes.

## Publish the public site

The live site is `https://bobs-org.github.io/`. Publication logic lives in this
repository; `bobs-org/bobs-org.github.io` holds only the caller workflow, a
README, and the durable `published.json` log. That log is not assumed to be
served as `/published.json`. Live provenance is `/build.json`.

Preview host bootstrap, then apply it only when you intend to create or resync
the deploy repository. Bootstrap never writes `published.json`.

```bash
just docs-host-bootstrap
just docs-host-bootstrap -- --yes
```

Pass `--deploy-dir` when an agent already opened the deploy checkout:

```bash
just docs-host-bootstrap -- --yes --deploy-dir /path/to/bobs-org.github.io
```

Dispatch a publish and watch the run that dispatch started. The command enables
the workflow first so a 60-day inactivity disable does not block it.

```bash
just docs-publish
just docs-publish -- --force
just docs-publish -- --drill
just docs-publish -- --pinned
just docs-status
```

`just docs-status` reports the host manifest, live `/build.json`, recent
publish and weekly runs, workflow state including `disabled_inactivity`, the
hold flag, and open managed blocked issues. A missing first site is a clear
status. GitHub authentication failure is an error.

### Rollback with held pinned SHAs

Edit `published.json` in the deploy repository: set `"hold": true` and the
source SHAs to serve. Dispatch a pinned publish:

```bash
just docs-publish -- --pinned
```

Scheduled and ordinary unpinned runs skip while hold is true, including force.
Pinned builds those exact SHAs and leaves the hold in place. Set `"hold": false`
when the rollback is finished.

### Recovery

A gate or deploy failure opens one `Docs publish blocked` issue
(`docs-publish`) and leaves the last good site live. The next ordinary
dispatch sees that issue and re-runs the gates even when the fingerprint is
unchanged:

```bash
just docs-publish
```

Do not use `--force` to recover. Force rebuilds unchanged inputs when nothing
is held. A successful live verification closes the issue.

### Inactivity

GitHub disables scheduled workflows on a public repository after 60 days
without a run (`disabled_inactivity`). `just docs-publish` enables the workflow
before dispatch. `just docs-status` reports that state.

### Failure boundaries

Before deploy, a planner skip, a held unpinned run, or a gate failure uploads
nothing and does not rewrite `published.json`. The last good site stays live.
`--drill` runs the real gates against a staged-only broken link, must fail
before upload, and opens the blocked issue.

After the gates pass, upload, Pages deploy, recording `published.json`, and
live verification run in that order. A deploy or record failure keeps the
previous live site. A live-verification failure means the new site may already
be serving; the blocked issue opens so the next ordinary run revalidates.
