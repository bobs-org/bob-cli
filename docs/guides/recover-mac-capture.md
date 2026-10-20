---
title: "Recover a stuck Mac Capture panel"
description: "Recover retained drafts, block-ID errors, clipboard delay, a failed restart, or a missing menu-bar icon."
tags:
  - macOS
---

# Recover a stuck Mac Capture panel

This guide recovers the capture panel when a draft should still be on
screen, Add block ID fails or will not take typing, paste is slow,
Restart fails, or the menu-bar icon is missing. Capture failures keep
the complete draft and destination on screen.

## Before you start

Install Bob Mac Capture. Open the panel from the menu-bar item. A
missing `bob` is
[Fix Mac Capture when bob is not resolved](fix-mac-capture-bob-resolution.md).

## 1. Confirm the draft is still there

A failed capture always preserves the complete draft and keeps the
panel open. Use **Copy Diagnostic** next to the error for the exact
`bob` message (the trailing draft argument is redacted). A panel that
vanished after Return means the capture landed; the success
notification names the route.

Closing the panel never destroys a draft. **Discard** permanently
clears it. Control-C stashes a semantically nonempty draft before
clearing and closing. Restore from the footer **Stash** action
(Control-S) onto an empty editor. **Shift-D Delete All** in the stash
picker permanently clears every retained draft, including the on-disk
file; lowercase `d` does nothing.

## 2. Add block ID errors and focus

Duplicate IDs, stale task refs, terminal tasks, and file I/O errors are
Bob errors. The Add block ID card keeps the selected task and typed ID
visible so you can edit, retry, or press Escape to return to the
refreshed task list. The draft is not expanded until Bob confirms the
write.

If typing does not reach the Add block ID field, this is observation
only:

```bash
log show --signpost --predicate 'subsystem == "org.bobs.bob-mac-capture"'
```

Look for `block-id-focus-claimed` (normal), `block-id-focus-repaired`
(the safety net fired), or `block-id-focus-claim-failed` (report it).

## 3. Slow paste

With the source content still on the clipboard, compare rich flavors
with plain text, then rewrite the clipboard as plain text and paste
again:

```bash
osascript -e 'clipboard info'
pbpaste | pbcopy
```

`clipboard info` is read-only. `pbpaste | pbcopy` replaces the
clipboard with plain text only. If that second paste is instant, the
delay was rich flavor import.

## 4. Restart failed

**Restart Bob Mac Capture** refuses to quit rather than leaving no
menu-bar item behind. It reports `Restart failed` when the process is
running unbundled (`swift run BobMacCapture` or a raw `.build` binary)
or the installed bundle at the launch-time path is missing. Both cases
post a `Restart failed` notification and record the reason in Settings
› Diagnostics. Install with `just bundle` then `just install` and try
again.

## 5. Missing menu-bar icon

No crash dialog, hotkey does nothing, Settings will not open: the app
has no nib, so a broken entry point can launch an empty AppKit event
loop. Check for a live silent process before assuming it failed to
launch:

```bash
pgrep -fl BobMacCapture
log show --last 5m --predicate 'process == "BobMacCapture"'
```

Look for the `launch-complete` signpost (subsystem
`org.bobs.bob-mac-capture`). Its absence means
`applicationDidFinishLaunching` never ran. If the process is not
running, check `~/Library/Logs/DiagnosticReports/` for a crash report.

## If it goes wrong

If Restart still fails after `just install`, Diagnostics names a missing
bundle path: install into that same target. If the icon is missing and
`pgrep` shows a live process with no `launch-complete`, quit that
process from Activity Monitor and open the installed
`Bob Mac Capture.app` again. A `swift test` failure with
`no such module 'XCTest'` is toolchain selection; see
[Build](../develop/mac-capture/build.md).

## Related

- [Capture panel](../reference/mac-capture/capture-panel.md)
- [Mac Capture diagnostics](../reference/mac-capture/diagnostics.md)
- [Mac Capture install](../reference/mac-capture/install.md)
- [Fix Mac Capture when bob is not resolved](fix-mac-capture-bob-resolution.md)
- [Mac Capture build](../develop/mac-capture/build.md)
