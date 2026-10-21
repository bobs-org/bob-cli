---
title: Keep the package under the size cap
description: Use the package checker to keep bob-obsidian below its crates.io size budget.
---

# Keep the package under the size cap

Keep the compressed .crate archive below the checker thresholds and preserve the license
notices required by embedded assets.

## Before you start

Use Cargo and the pinned Rust toolchain. The package checker runs cargo package with
--locked, verifies required notices and forbidden paths, and reports the archive size.

## 1. Check the package

~~~bash
just package-check
~~~

The checker warns above 8 MiB and fails above 9 MiB. The crates.io upload cap is
10 MiB, so the checker leaves room for growth. The latest rehearsal check measured
7.79 MiB.

## 2. Find the largest contents

If the checker warns or fails, inspect its largest directory contributors and the
Cargo.toml include list. Keep source files, runtime scripts, embedded assets, and every
license or notice required by those assets. The checker specifically rejects docs,
tests, tools, vault, sase, and .github from the package.

## 3. Recheck the normal package

After changing the include list, run just package-check without skipping Cargo's build
verification. A quick local size-only run may use
python3 tools/release/package_check.py --no-verify, but the release gate uses the full
check.

If the embedded assets themselves push the package toward the upload cap, propose
splitting them into a separate bob-obsidian-assets crate. That split is a separate
design decision. Never configure release-plz to publish without verification.

## If it goes wrong

If a required notice is missing, restore it to the package include list even if it
changes the size. If the archive exceeds 9 MiB, reduce the package or make the assets
split decision before approaching crates.io's 10 MiB limit.

## Related

- [Release train reference](reference.md)
- [Build the docs](../build.md)
