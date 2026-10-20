---
title: "Mac Capture development"
description: "Bob Mac Capture contributor reference: architecture, build recipes, CI gates, and the mounted developer pages."
---

Build and type-check from [Build](build.md). Sign, install, update, and uninstall from [Install](../../reference/mac-capture/install.md). CI gates are in [CI](ci.md).

- [Architecture](architecture.md) for source layout, process and cache boundaries, and failure rules.
- [Build](build.md) for toolchain selection, XCTest diagnosis, portable checks, bundle recipes, and glyph fixtures.
- [CI](ci.md) for the macOS 26 workflow, launch signpost, and installer restart gates.
- [Render fixtures](render-fixtures.md) for synthetic panel images, CI provenance, and documentation preview commands.
- [Reference hub](../../reference/mac-capture/index.md) for the user-facing install, settings, panel, picker, preview, keyboard, Refs, diagnostics, and protocol pages.
- [Develop Bob](../index.md) for the shared docs toolchain (`just docs-check`) and behavior contracts.

Doc changes here publish to the Mac Capture development mount and require `just docs-check` in bob-cli; the Mac repo gets docs, scripts, and recipes only, never Swift changes in this epic.
