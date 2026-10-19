---
title: Build the docs
description: Generate and build Bob's documentation site from this repository.
---

# Build the docs

Run these recipes from the bob-cli repository root. `uv` creates and uses the locked
environment under `tools/docs/.venv`; generated files and the built site stay in the
repository's ignored `target/docs/` directory.

Regenerate the contract index, navigation, and README table after changing the contract
registry:

```bash
just docs-gen
```

Serve the site locally with live reload:

```bash
just docs-serve
```

Build the strict Zensical site:

```bash
just docs-build
```

Build the same configuration with the Material 9.7 fallback:

```bash
just docs-build-material
```

The generated site is written to `target/docs/site/`. Building does not regenerate
tracked pages; run `just docs-gen` explicitly and review its changes.
