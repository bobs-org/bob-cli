---
title: "Continue with Mac Capture"
description: "Build Mac Capture, point it at Bob and the demo vault, and capture once."
search:
  boost: 2
---

# Continue with Mac Capture

Bob Mac Capture is an optional native menu-bar panel for macOS. It is a thin
client of the CLI: it never parses capture grammar or writes the vault
itself, it runs `bob`, renders the preview, and submits one `bob capture`
call per draft. The automated tutorial replay runs on Linux and never
exercises this app; follow the steps on a Mac by hand.

## 1. Build, bundle, and launch from source

<!-- tutorial-test: skip Mac build needs macOS and the Apple toolchain -->
```bash
git clone https://github.com/bobs-org/bob-mac-capture.git
cd bob-mac-capture
just build
```

<!-- tutorial-test: skip Mac bundle needs macOS and the Apple toolchain -->
```bash
just bundle
```

`just bundle` creates `.build/bundle/Bob Mac Capture.app`. Install it with:

<!-- tutorial-test: skip Mac install needs macOS and the Apple toolchain -->
```bash
just install ~/Applications
```

Then launch the installed copy:

<!-- tutorial-test: skip Mac launch needs the Mac app bundle -->
```bash
open ~/Applications/"Bob Mac Capture.app"
```

Building needs macOS 26 with Command Line Tools for Xcode 26+ or Xcode 26+,
with Swift resolved through `./Scripts/xcode-swift.sh` rather than a bare
`swift` on `PATH`. For the full toolchain, signing, and diagnosis steps, see
[Install](../reference/mac-capture/install.md),
[Build](../develop/mac-capture/build.md), and
[README › Signing](https://github.com/bobs-org/bob-mac-capture#signing).

## 2. Select Bob and the demo vault

<!-- tutorial-test: skip panel setup needs the Mac app -->
```bash
which bob
echo "$BOB_DIR"
```

In Settings, under the **Bob** section, set **Executable override** to the
installed `bob` executable and **Vault path** to the demo `$BOB_DIR` copy
from the tutorial. **Resolved path** shows what the app actually uses. When
no override is set, the app finds `bob` at `~/.cargo/bin/bob`, `~/bin/bob`,
`/opt/homebrew/bin/bob`, or `/usr/local/bin/bob`; use **Recheck Bob** after
changing either field.

Grant Accessibility only for Highlights tabs. The panel requests it once
through the system prompt; capture and the ledger agenda work without it.
Recovery is in
[Restore Mac Capture permissions](../guides/restore-mac-capture-permissions.md).

## 3. Capture once and read the result

Submit this new draft in the panel (it uses a fresh ID the tutorial never
creates):

```text
Sharpen the shears @workshop^shears
```

<!-- tutorial-test: skip first Mac capture needs the Mac panel -->
```bash
bob capture 'Sharpen the shears @workshop^shears'
```

The fenced command shows the CLI semantics of the same draft: after the
tutorial, `^seed-packet` and `^seedlings` already exist, so this fresh
`^shears` ID routes a new task to `workshop.md`. Type the plain text in the
panel, read its preview, and submit. The panel hides only after `bob`
reports success for the whole draft, and the task lands in `workshop.md`
exactly as the CLI writes it. This Mac panel path was validated against the
post-tutorial demo state from source; Linux replay never runs the Mac GUI.

## Next

- [Your first Bob day](first-bob-day.md) — the CLI path this panel submits to
- [How Bob fits together](../concepts/how-bob-fits-together.md) — the capture round trip
- [Mac Capture reference](../reference/mac-capture/index.md) — install, keys, and protocol
- [Fix Mac Capture when bob is not resolved](../guides/fix-mac-capture-bob-resolution.md)
