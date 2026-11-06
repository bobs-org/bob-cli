---
title: "Send email from Obsidian"
description: "Write a compose or reply note, preview its Gmail draft, and send it after an explicit confirmation."
tags:
  - CLI
  - macOS
---

# Send email from Obsidian

Write the message as a local note, check the exact Gmail draft preview, then
confirm. Every send goes through a Gmail draft plus a confirmation bound to
that preview; there is no direct-send command. The Bob Mail plugin's **New
email**, **Reply**, **Save draft**, and preview-modal **Send** gestures call
these same CLI steps; until they land, run the commands below.

## Before you start

Follow [Connect Google Mail](connect-google-mail.md), then add the compose
grant on the Mac that owns `gmail.writer`:

```bash
bob gmail login -s compose
bob gmail doctor -b /path/to/vault
```

Agent hosts stay read-only: the compose scope is granted only on the Mac.
`draft` and `send` refuse on other hosts with the writer pin. Confirm the
doctor `scopes` row reads `readonly+compose`.

## Write the note

Create a standalone message or reply to a saved thread. Both are local vault
writes and need no sign-in:

```bash
bob gmail new -t 'Ada <ada@example.com>' -s "Lease renewal"
bob gmail reply mail/2026/261105_lease_renewal.md
bob gmail reply mail/2026/261105_lease_renewal.md --all
```

A second reply refuses without changing the note. Write the body under the
H1 (or under the `## Reply` heading); anything under `## Notes` never enters
the outgoing message. Task checkboxes, block ids, inline fields, Dataview or
Tasks fences, Templater syntax, unsafe link destinations, remote images, and
embeds that name no declared attachment refuse with a kind and line number
instead of sending. See [Bob Gmail](../gmail.md) for the full refusal list
and the attachment rules.

## Preview the draft

```bash
bob gmail draft mail/2026/261105_lease_renewal.md
```

This creates or updates the Gmail draft, reads it back, and shows the stored
preview: account, actual From, To/Cc/Bcc, subject, reply target, exact text
body, signature mode, attachments with hashes, and flattened links. Readback
never falls back to a snippet; a missing MIME part fails instead of sending a
half-verified message. The preview ends with a `digest` covering every
content and envelope field. Any edit — to the note, the attachments, the
signature setting, or the remote draft — changes the digest.

## Confirm and send

Preview first, then confirm that exact preview:

```bash
bob gmail send mail/2026/261105_lease_renewal.md -d
bob gmail send mail/2026/261105_lease_renewal.md -c 0123abcd... -f json
```

`-d` always previews without sending. On a terminal, `send` without `-d`
shows the preview and asks `[y/N]`; anything but `y` keeps the draft. Without
a terminal, pass the digest from the preview with `-c`; a stale digest exits
2 with `confirmation_mismatch`, so re-preview instead of resending. There is
no force flag.

On success the compose note is promoted in place at its original path and a
reply send removes only the submitted Reply section. Sent messages render as
`[!mail-sent]` callouts; inbound mail stays `[!mail]`. Only body-free
metadata (note, draft id, digest, thread and message ids) reaches the journal
and error output.

If a previous send crashed or timed out, the next `send` recovers first: a
still-present draft returns to preview, a proven SENT copy finishes local
finalization only, and anything else reports `outcome_unknown` without
sending again. Inspect the draft and SENT folder, then retry `send` for a
fresh recovery. The undo countdown lives in the plugin; there is no CLI delay
setting.

## Try it

Agents never run these live steps.

1. Pick a test address you control.
2. On the Mac, run `bob gmail login -s compose` and confirm doctor is green.
3. `bob gmail new -t '<test address>' -s "Bob send test"`, write one line,
   then `bob gmail draft <note> -f json` and read the preview.
4. `bob gmail send <note> -d -f json`, check the digest, then confirm with
   `-c <digest>` and verify the message arrives.
5. Open the promoted note: the conversation carries the `[!mail-sent]` card
   and your `## Notes` section is untouched.
