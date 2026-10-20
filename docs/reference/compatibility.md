---
title: "Requirements and compatibility"
description: "Tested snapshot plus the minimum Obsidian, platform, and toolchain versions Bob needs."
template: compatibility.html
---

# Requirements and compatibility

Bob runs one Markdown vault through a Rust CLI, Obsidian plugins, and an
optional Mac panel. The snapshot above names the exact revisions this site
was built from. The floors below are the minimum versions readers need.

## Minimum requirements

- **CLI platforms:** Linux and macOS. The CLI, clipboard discovery, and
  browser helpers cover both; Mac-only paths note it where they differ.
- **Obsidian:** 1.8.7 or later. Every bundled Bob plugin manifest sets
  `minAppVersion` 1.8.7, so the floor follows the manifests, not this
  site's builder version. See the mounted [Obsidian plugins](plugins/index.md)
  reference for current manifest versions.
- **Tasks plugin:** use a current Tasks release with the Dataview task
  format and the `#task` global filter. The native query engine follows
  Tasks 8.4.0 filter behavior for `status.symbol`; the parity fixture and
  goldens run headless without Obsidian. Desktop rendering inside Obsidian
  still awaits a human pass.
- **Rust:** a current stable compiler that supports edition 2024 from the
  [Rust toolchain](https://rust-lang.org/tools/install). No minimum version
  is pinned here.
- **Mac Capture (optional):** macOS 26 with Command Line Tools for Xcode
  26+ or Xcode 26+, plus a signed or ad-hoc signed `bob` on `PATH`. See the
  authoritative [Requirements](https://github.com/bobs-org/bob-mac-capture#requirements),
  [Development](https://github.com/bobs-org/bob-mac-capture#development),
  and [Signing](https://github.com/bobs-org/bob-mac-capture#signing) in the
  [Bob Mac Capture README](https://github.com/bobs-org/bob-mac-capture#readme).
  The Mac reference mount is not published yet, so this page links the
  source README instead of copying its protocol changelog.

## Tested snapshot

The block above the title renders the rolling snapshot from the site
build: per-repository revisions with dirty and branch marks, the build
time and builder, tool versions, and `build.json`. Absent checkouts
report an unknown revision rather than a guessed one, and optional tool
versions appear only when the builder records them.

## Related

- [Install Bob](../start/install.md)
- [Continue in Obsidian](../start/obsidian.md)
- [Continue with Mac Capture](../start/mac-capture.md)
- [Look up Bob](index.md)
