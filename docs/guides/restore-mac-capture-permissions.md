---
title: "Restore Mac Capture permissions"
description: "Restore notifications, Accessibility, stable signing, and launch-at-login for Bob Mac Capture."
tags:
  - macOS
---

# Restore Mac Capture permissions

This guide restores notification delivery, Highlights Accessibility,
and launch at login. macOS ties those grants to the installed
code-signing identity, not only the bundle identifier. Keep one stable
identity (`Bob Mac Capture Local`) for the life of the install.

## Before you start

Use the installed signed bundle (`just bundle` then `just install`),
not `swift run` or a raw `.build` binary. Settings › Diagnostics shows
the current signing state. Confirm a stable identity:

```bash
codesign -d -r- ~/Applications/"Bob Mac Capture.app"
```

A stable install shows `certificate leaf = H"…"`. An ad-hoc install
shows a `cdhash` designated requirement instead; ad-hoc reinstalls
silently revoke Accessibility while System Settings still shows the
switch on.

Signing identity creation is in
[README › Signing](https://github.com/bobs-org/bob-mac-capture#signing)
and [Install › Signing](../reference/mac-capture/install.md#signing).

## 1. Notifications

Open Settings › Notifications. Confirm the status is `Authorized`. Use
**Send Test Notification**. If the status is `Denied — enable in System
Settings`, use **Open System Notification Settings** and enable Bob Mac
Capture there, then send the test again.

Notification delivery requires that signed bundle. An install never
prompts for notification permission. Denied or not-yet-requested
authorization is silent and non-fatal.

The **Install complete** banner (`Bob Mac Capture restarted
successfully.`) posts only when install restarts an already-running
copy and notifications are already authorized. A stopped install,
**Restart Bob Mac Capture**, `swift run`, and denied authorization are
silent by design; installation still succeeds. Look for
`install-restart-notification-requested` with:

```bash
log show --signpost --predicate 'subsystem == "org.bobs.bob-mac-capture"'
```

## 2. Accessibility for Highlights tabs

Settings › References shows **Highlights tabs** as `Allowed` or `Not
allowed` and never prompts from that row. Tabs never asks at launch.
From the Tabs card, use **Open Accessibility Settings**: that clears a
stale grant and shows the system prompt, then allow Bob Mac Capture.

If the switch looks on but does nothing, the row is pinned to an older
build. Use that button, or run this reset and grant the app again:

```bash
tccutil reset Accessibility org.bobs.bob-mac-capture
```

That reset is a write. Grant the app from the next system prompt.
Later installs with the stable identity keep the grant. When an install
changes the code signature, the installer clears the stale
Accessibility row; grant the app again from Tabs › Open Accessibility
Settings.

## 3. Launch at login

Settings › Launch is driven by `SMAppService.mainApp`. If the status is
`Requires approval in System Settings`, open System Settings → General
→ Login Items and approve Bob Mac Capture there. The app cannot grant
that approval itself.

After uninstall, macOS may keep a stale Login Items row until the next
login. Remove it by hand from that same pane if it lingers.

## If it goes wrong

If tests never appear, Diagnostics › Signing is still ad-hoc or
unsigned: install with the stable identity and re-authorize. If
Accessibility flips off after every `just install`, the install is
ad-hoc; see [Install › Signing](../reference/mac-capture/install.md#signing).
Do not run `tccutil reset` in a loop; grant once from the prompt after
a single reset.

## Related

- [Mac Capture permissions](../reference/mac-capture/permissions.md)
- [Mac Capture install](../reference/mac-capture/install.md#signing)
- [Mac Capture settings](../reference/mac-capture/settings.md)
- [Recover Bob Refs](recover-bob-refs.md)
- [Continue with Mac Capture](../start/mac-capture.md)
