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
