---
title: "Fix Mac Capture when bob is not resolved"
description: "Resolve the bob executable, timeouts, stale target cache, and empty completion in Bob Mac Capture."
tags:
  - macOS
  - CLI
---

# Fix Mac Capture when bob is not resolved

This guide restores Bob Mac Capture when Settings cannot find `bob`, a
command times out, target completion is empty or stale, or wikilink
completion returns no candidates. The app never searches a login shell;
it uses Settings › Bob › Executable override or four default paths.

## Before you start

Install the CLI and Bob Mac Capture. Open the menu-bar item. These
commands are read-only:

```bash
which bob
echo "$BOB_DIR"
bob --version
```

Confirm `which bob` prints an absolute path and `bob --version` exits 0.

## 1. Read the unresolved state

When `bob` is missing, the menu-bar bullet becomes a `!`. The tooltip
reads `Bob Mac Capture — bob is not resolved. Check Settings.`, and the
menu gains **bob Not Resolved — Open Settings…**. Settings › Bob shows
**Resolved path** as `Not resolved` plus the underlying error.

Open Settings from that menu row, or choose **Settings** and look at the
**Bob** section. Confirm **Resolved path** and the error string before
changing anything.

## 2. Point Settings at the installed `bob`

Set **Executable override** to the absolute path from `which bob`, or
install `bob` at one of `~/.cargo/bin/bob`, `~/bin/bob`,
`/opt/homebrew/bin/bob`, or `/usr/local/bin/bob` and leave the override
empty. Relative paths are refused. Set **Vault path** to the vault
`BOB_DIR` names.

Choose **Recheck Bob** from the menu-bar item. Confirm **Resolved path**
matches `which bob` and the `!` glyph returns to the bullet-b.

## 3. Timeouts

Every `bob` invocation is bounded (20 seconds by default). A wedged
process is terminated instead of leaving the panel stuck. The error
names the timed-out command and never the captured text. Copy Diagnostic
next to the error copies that bounded message.

Retry the same draft after Recheck Bob. If the same command times out
from Terminal, fix that `bob` install first:

```bash
bob capture --dry-run --no-clip --format json 'test task'
```

## 4. Empty or stale target completion

Diagnostics reports `Target cache stale` with the scan error when
target or route completion is empty or stale. Fix the named cause (an
unreadable vault path is common), then reopen the panel so the refresh
retries.

Confirm the error is gone in Settings › Diagnostics and that `@` route
completion lists notes again.

## 5. Wikilink completion

An empty wikilink list with no status change means no note, heading, or
block matched the query. Try a shorter query or check the spelling. A
status message starting with `Link completion warning:` means `bob`
skipped one or more vault entries (an unreadable note or malformed
alias) and still returned the candidates it could build; the warning
names the problem. A process or transport failure uses the unresolved
or timeout paths above rather than a silent empty list.

## If it goes wrong

If Recheck Bob still shows `Not resolved`, the override is not an
absolute path, that file is not executable, or none of the four default
paths holds an executable `bob`. The resolver checks only that an
override is absolute and executable, or finds an executable in those
paths; it does not inspect code signatures. Paste the path from
`which bob` with no wrapping quotes in the field. App signing
requirements are separate from this diagnosis; see
[Install](../reference/mac-capture/install.md#signing). Capability
mismatches belong in
[Protocol](../reference/mac-capture/protocol.md); a `swift test` failure
with `no such module 'XCTest'` belongs in
[Build](../develop/mac-capture/build.md).

## Related

- [Mac Capture settings](../reference/mac-capture/settings.md)
- [Mac Capture install](../reference/mac-capture/install.md)
- [Mac Capture diagnostics](../reference/mac-capture/diagnostics.md)
- [Wikilink completion](../reference/mac-capture/wikilink-completion.md)
- [Recover a stuck Mac Capture panel](recover-mac-capture.md)
