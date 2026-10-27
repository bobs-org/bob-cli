---
title: "Resolve a Mac Capture hotkey conflict"
description: "Free a conflicting shortcut, recheck Bob, or switch the production capture binding."
tags:
  - macOS
---

# Resolve a Mac Capture hotkey conflict

This guide restores the capture and Bob Refs global shortcuts when
another app already owns the chord. One Carbon handler owns every Bob
hotkey. A failed registration leaves every other registration intact
and reports the conflict instead of swallowing the key.

## Before you start

Install Bob Mac Capture and open Settings. Default chords:

- Capture: Control-Shift-Command-I (production) or
  Control-Shift-Command-R (development)
- Capture alias: `Command-\` (on by default; Settings › Hotkey)
- Bob Refs: Control-Shift-Command-O
- Refs alias: `Command-/` (on by default; Settings › References)
- Highlights-frontmost open: Command-O (configurable)

Settings › Hotkey shows the live capture chord as **Active binding**.

## 1. Read the conflict

Open Settings › Diagnostics. A failed capture registration reports
`Hotkey conflict` with the Carbon status. A failed Refs binding reports
`Refs hotkey conflict: …`. A failed Command-\ alias reports
`Capture alias hotkey conflict: …`. A failed Command-/ alias reports
`Refs alias hotkey conflict: …`. The same event is in Recent Activity
(`HH:mm:ss  message`). Confirm that status string before changing a
binding.

The capture hotkey then does nothing until the chord is free. Other
Bob keys keep working.

## 2. Free the shortcut or switch the binding

Quit or rebind the other app that owns the chord. Then choose
**Recheck Bob** from the menu-bar item so the handler registers again.

If you need the development capture chord instead, turn off **Use
production Control-Shift-Command-I** in Settings › Hotkey. That selects
Control-Shift-Command-R. Turn the production toggle off only when you
need that development binding.

Refs chords re-register as soon as you toggle **Open Bob Refs with
Control-Shift-Command-O** or the Highlights-frontmost open key. The
Capture alias toggle **Also open Capture with Command-\\** re-registers
immediately. **Also open Bob Refs with Command-/** re-registers
immediately. Each action registers independently.

## 3. Confirm the live chord

Settings › Hotkey **Active binding** shows the live capture chord.
Press it and confirm the capture panel opens. For Refs, press
Control-Shift-Command-O and `Command-/` and confirm Bob Refs opens.
Diagnostics shows no `Hotkey conflict`, `Refs hotkey conflict`,
or alias conflict.

## If it goes wrong

If Recheck Bob still reports the conflict, another process still owns
that exact chord. Check Shortcuts, other menu-bar apps, and the
Highlights-frontmost open key (Command-O or Control-O) separately from
the global Refs toggle. Apps such as 1Password, whose autofill shortcut
defaults to `Command-\`, commonly claim the Capture alias. In-app
shortcuts such as editors' Toggle comment never block the Command-/
alias; they just stop receiving it while the alias is on. Only another
global hotkey owner blocks it. Launch-at-login
approval is a different permission; use
[Restore Mac Capture permissions](restore-mac-capture-permissions.md)
when Settings › Launch reads `Requires approval in System Settings`.

## Related

- [Mac Capture settings](../reference/mac-capture/settings.md)
- [Mac Capture keyboard](../reference/mac-capture/keyboard.md)
- [Mac Capture diagnostics](../reference/mac-capture/diagnostics.md)
- [Bob configuration](../reference/configuration.md#mac-capture-hotkeys)
- [Restore Mac Capture permissions](restore-mac-capture-permissions.md)
