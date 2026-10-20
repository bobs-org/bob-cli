---
title: "Mac Capture development"
description: "Bob Mac Capture contributor reference: architecture and the mounted developer pages."
---

Build the app from the [Mac README](https://github.com/bobs-org/bob-mac-capture) (toolchain, signing, bundle and install recipes). This mount grows with the application-reference phase (build, CI, render fixtures).

- [Architecture](architecture.md) for source layout, process and cache boundaries, and failure rules.
- [Reference hub](../../reference/mac-capture/index.md) for the user-facing panel, picker, preview, keyboard, and protocol pages.
- [Develop Bob](../index.md) for the shared docs toolchain (`just docs-check`) and behavior contracts.

Doc changes here publish to the Mac Capture development mount and require `just docs-check` in bob-cli; the Mac repo gets docs, scripts, and recipes only, never Swift changes in this epic.
