---
title: "Capture a task from anywhere on your Mac"
description: "Open Mac Capture, check Bob and vault settings, submit a draft, and find the task."
tags:
  - macOS
  - CLI
---

# Capture a task from anywhere on your Mac

This guide captures one task from the Mac menu-bar panel after the app is
installed. The panel is a thin client of `bob`: it never parses capture grammar
or writes the vault itself.

## Before you start

Install the CLI, point it at your vault, and install Bob Mac Capture. The
[Continue with Mac Capture](../start/mac-capture.md) tutorial covers build,
bundle, and install. Signing lives in
[README › Signing](https://github.com/bobs-org/bob-mac-capture#signing)
and [Install › Signing](../reference/mac-capture/install.md#signing).
This page assumes the app is already on the Mac.

## 1. Confirm Bob and the vault in Settings

Open the menu-bar icon's menu and choose **Settings**. Under the **Bob**
section, set **Executable override** to the `bob` you installed and **Vault
path** to your vault. **Resolved path** shows what the app will use. Click
**Recheck Bob** after changing either field.

```bash
which bob
echo "$BOB_DIR"
```

Confirm **Resolved path** matches those two values. Capture works without an
Accessibility grant; that grant is only for Highlights tabs.

## 2. Invoke the global shortcut and read the preview

The production shortcut is Control-Shift-Command-I. A development shortcut
Control-Shift-Command-R is selectable in Settings. `Command-\` opens the
same panel unless it is turned off in Settings › Hotkey. See
[Keyboard](../reference/mac-capture/keyboard.md)
for the current menu and shortcut list.

Type a new task with a stable block id, for example
`Sharpen the shears @workshop^shears`. Confirm the live preview names
`workshop.md` and the new `^shears` task before you submit.

## 3. Submit and find the task

Submit the draft from the panel footer. The panel hides only after `bob`
reports success for the whole draft. Open `workshop.md` and confirm the new
open task with `^shears`.

The same draft from the CLI is:

```bash
bob capture 'Sharpen the shears @workshop^shears'
```

## If it goes wrong

If the menu-bar icon tooltip reads `Bob Mac Capture — bob is not resolved.
Check Settings.`, follow
[Fix Mac Capture when bob is not resolved](fix-mac-capture-bob-resolution.md).
If the panel stays open, the submit did not succeed; fix the
preview error and submit again. Do not edit the vault from the panel.

## Related

- [Continue with Mac Capture](../start/mac-capture.md)
- [How Bob fits together](../concepts/how-bob-fits-together.md)
- [Capture](../capture.md)
- [Mac Capture reference](../reference/mac-capture/index.md)
- [Fix Mac Capture when bob is not resolved](fix-mac-capture-bob-resolution.md)
