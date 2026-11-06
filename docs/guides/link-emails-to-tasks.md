---
title: "Link emails to tasks"
description: "Capture a Gmail URL, reuse a saved conversation, and backfill old task links."
tags:
  - CLI
  - macOS
---

# Link emails to tasks

Paste a Gmail conversation URL into a task to keep the action beside its
context. Bob saves the conversation as a mail note and replaces the URL with
a readable link such as **✉️ Lease renewal · Dana**.

## Before you start

Follow [Connect Google Mail](connect-google-mail.md), then run
`bob gmail login` on the host that captures tasks. If `gmail.writer` is set,
automatic linking runs only on that host. Running login once after upgrading
also enables the local background-linking receipt for an existing grant.

## Capture the URL

Paste the Gmail URL into Bob Mac Capture, alone or after an action. From a
terminal, quote the whole draft:

```bash
bob capture 'Reply to Dana https://mail.google.com/mail/#all/1a0b9eb2cc26ab96'
```

Capture writes the task immediately. If that thread is already saved, the
task contains its final mail-note link. Otherwise the URL stays while a
background worker saves and links it. Closing the capture panel does not
stop that worker. A failure leaves the URL available to open or retry.

Use `bob capture -d -f json '…'` for the offline preview. Gmail account
selectors in the pasted URL do not override Bob's configured account.

## Paste or pick inside Obsidian

With the Bob Mail plugin enabled, run **Bob Mail: Link email…** to pick a
recent or searched thread; Bob saves it and inserts its link at the cursor.
Pasting one Gmail URL into a note does the same in place, and a failed save
leaves the URL as pasted. Without the plugin, run
`bob gmail save 'GMAIL_URL' -f json` and paste the returned `link`. Existing
Markdown links keep the words you chose for their label.

## Link older tasks

Preview a whole-vault backfill, inspect the diff, then apply it:

```bash
bob gmail link -a -d
bob gmail link -a
```

To retry one note, run `bob gmail link mac_inbox.md`. Unresolvable or deleted
threads remain URLs. Mail notes and generated/archive directories are
excluded from whole-vault backfill; see [Bob Gmail](../gmail.md) for details.

## Try it

1. Choose one non-sensitive conversation in your own Gmail account.
2. Capture its URL with an action. Confirm the task appears immediately and
   then becomes a labelled mail-note link.
3. Capture the same URL again. Confirm it links immediately, even offline.
4. Set `BOB_GMAIL_KICK=off` for a capture of another conversation. Confirm its
   URL stays, then retry with `bob gmail link mac_inbox.md`.

The exact preview fields and worker behavior are in
[Capture: Gmail links](../capture.md#gmail-links); the picker and paste
settings are in [Bob Mail](../reference/plugins/bob-mail.md).
