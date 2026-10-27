---
title: Keep the package under the size cap
description: Use the package checker to keep bob-obsidian and bob-obsidian-assets below their crates.io size budgets.
---

# Keep the package under the size cap

Keep each compressed .crate archive below the checker thresholds and preserve the
license notices required by that package. bob-obsidian and bob-obsidian-assets
version independently on the shared daily train; each archive has its own 8 MiB
warning and 9 MiB failure budget.

## Before you start

Use Cargo and the pinned Rust toolchain. The package checker enumerates both
publishable workspace members, runs cargo package --workspace with --locked,
verifies required notices and forbidden paths per archive, and reports each
size.

## 1. Check both packages

~~~bash
just package-check
~~~

The checker warns each archive independently above 8 MiB and fails above 9 MiB.
The crates.io upload cap is 10 MiB per crate, so the checker leaves room for
growth. The latest post-split check measured:

| Package | Compressed size | Headroom before 8 MiB warning |
| --- | --- | --- |
| bob-obsidian 0.4.0 | 3,217,425 bytes (3.07 MiB) | 4.93 MiB |
| bob-obsidian-assets 0.1.0 | 5,004,764 bytes (4.77 MiB) | 3.23 MiB |

`just package-list` prints each inventory separately so the root listing is not
mixed with the assets listing.

## 2. Find the largest contents

If the checker warns or fails, inspect that archive's largest directory
contributors and the matching Cargo.toml include list.

bob-obsidian keeps source files, runtime scripts, small native assets, README,
CHANGELOG, the project MIT license, and the Moment notice. Font payloads, web-clip
vendor bundles, and their OFL, DejaVu, Defuddle, and Mermaid notices live in
bob-obsidian-assets.

The checker rejects docs, tests, tools, vault, sase, and .github from every
archive, and it rejects the moved payload tree from the root archive.

## 3. Recheck the normal packages

After changing an include list, run just package-check without skipping Cargo's
build verification. A quick local size-only run may use
python3 tools/release/package_check.py --no-verify, but the release gate uses the
full check. Full verification must succeed before bob-obsidian-assets exists on
crates.io.

Never configure release-plz to publish without verification, and do not set
publish = false, release = false, or publish_no_verify = true to bypass the gate.

## If it goes wrong

If a required notice is missing, restore it to that package's include list even
if it changes the size. If either archive exceeds 9 MiB, reduce that package
before approaching crates.io's 10 MiB limit.

## Related

- [Release train reference](reference.md)
- [Build the docs](../build.md)
