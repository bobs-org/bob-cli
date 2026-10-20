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

## 1. Build from source

<!-- tutorial-test: skip Mac build needs macOS and the Apple toolchain -->
```bash
git clone https://github.com/bobs-org/bob-mac-capture.git
cd bob-mac-capture
just build
```

Building needs macOS 26 with Command Line Tools for Xcode 26+ or Xcode 26+,
with Swift resolved through the Apple toolchain. For signing, bundling, and
toolchain diagnosis, follow that repository's README instead of this page.

## 2. Select Bob and the demo vault

<!-- tutorial-test: skip panel setup needs the Mac app and Accessibility grant -->
```bash
which bob
echo "$BOB_DIR"
```

In the panel settings, select the installed `bob` executable and the demo
`$BOB_DIR` copy from the tutorial. The app finds `bob` at the usual install
locations when settings are untouched.

Grant Accessibility only for the Highlights tab feature the Mac README
names. The panel requests it once through the system prompt; capture and
the ledger agenda work without it.

## 3. Capture once and read the result

<!-- tutorial-test: skip first Mac capture needs the Mac panel -->
```bash
bob capture 'Water the seedlings @garden^seedlings'
```

Type the same text in the panel, read its preview, and submit. The panel
hides only after `bob` reports success for the whole draft, and the task
lands in `garden.md` exactly as the CLI writes it.

## Next

- [Your first Bob day](first-bob-day.md) — the CLI path this panel submits to
- [How Bob fits together](../concepts/how-bob-fits-together.md) — the capture round trip
- [Requirements and compatibility](../reference/compatibility.md) — Mac requirements
