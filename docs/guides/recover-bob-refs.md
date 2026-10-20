---
title: "Recover Bob Refs"
description: "Recover library load, scan, Highlights app, tab-close, and open-history failures in Bob Refs."
tags:
  - macOS
  - CLI
---

# Recover Bob Refs

This guide recovers Bob Refs when the library snapshot fails to load,
a scan fails, Highlights is missing, a tab will not close, or open
history needs a reset. Bob Refs never writes the vault; `bob` does.
Reuse the vault and ref-job guides below for shared CLI failures.

## Before you start

Install the CLI and Bob Mac Capture. Open Bob Refs (Control-Shift-Command-O).
These commands are read-only:

```bash
bob ref list -R all -A -f json | head
bob ref doctor
```

Confirm `bob ref list` prints JSON. A stuck clip job is
[Recover a stuck reference job](recover-reference-job.md), not this
panel.

## 1. Load failure

`Bob couldn't load your references` is the centered card when no cache
exists and refresh failed. The bounded error names the cause. **Retry**
(⌘R) re-reads the snapshot. **Copy Diagnostic** copies the message.

Confirm Retry fills the list, or that Copy Diagnostic names a readable
vault and a `bob` that supports `ref list`.

## 2. Highlights app selection

`Highlights isn't installed where Bob Refs can find it`: use **Open in
Default App**, or **Choose Highlights…** in Settings › References to
point at the `.app`. **Use Default** clears the override path.

Confirm Settings › References shows the resolved name, version, and
path.

## 3. Accessibility for Tabs

`Bob Refs needs Accessibility access to see Highlights' tabs`: use
**Open Accessibility Settings** from the Tabs card. That recovery is
[Restore Mac Capture permissions](restore-mac-capture-permissions.md).

## 4. Tab-close failure

`Couldn't close “title” in Highlights` means Highlights did not close
that tab, so the row came back. Press Control-X to retry. If a save
sheet appears instead, Bob Refs hides so Highlights can show the sheet.
Finish or dismiss the sheet, then retry Control-X.

## 5. Scan failure

`Bob couldn't scan your library` is the error banner with `bob`'s
message and hint, plus **Scan Again** and **Copy Diagnostic**.

- A dirty vault (`refusing to modify dirty vault files`) clears once
  the named notes are committed. Inspect with
  [Resolve a vault-sync conflict copy](resolve-vault-conflict-copy.md)
  when a conflict copy exists.
- `scan_busy` means another scan held the writer lock past the wait.
  Wait for it to finish, then scan again.
- `This bob can't report scans yet` means this `bob` predates the JSON
  report. Update `bob` and press ⌘S again.
- A timeout (`ran longer than 5 minutes`) is easier to watch from
  Terminal:

  ```bash
  bob ref scan -w
  ```

  That command writes notes. Quitting mid-scan stops it; `bob`'s note
  writes are atomic per file, and the next scan or cron run finishes
  the job.

## 6. Reset open history

To clear recently-opened rows and frecency, use **Reset Open
History…** in Settings › References. Confirm the dialog before it
runs. That write is local to the Mac; it does not change vault notes.

## If it goes wrong

If Retry still shows the load card, Copy Diagnostic and `bob ref list`
from Terminal. If scan keeps reporting a dirty vault, commit or recover
those notes first; do not delete files under the vault to clear the
error. Tab-close retries stay on Control-X; do not force-quit
Highlights while a save sheet is up.

## Related

- [Bob Refs](../reference/mac-capture/refs.md)
- [Bob Refs inspector](../reference/mac-capture/refs-inspector.md)
- [Restore Mac Capture permissions](restore-mac-capture-permissions.md)
- [Recover a stuck reference job](recover-reference-job.md)
- [Resolve a vault-sync conflict copy](resolve-vault-conflict-copy.md)
- [Mac Capture diagnostics](../reference/mac-capture/diagnostics.md)
