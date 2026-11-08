---
title: "File email from your phone"
description: "Forward a message to a Gmail plus address, apply the Bob label automatically, and sync it into the vault."
tags:
  - CLI
---

# File email from your phone

This guide routes a message you forward from your phone into Bob. Bob reads
the Gmail label and writes a mail note plus a task link in the area or
project named by a routing label, or in the Gmail inbox when none is
present; it does not archive, mark read, or remove the label.

## Before you start

Set up Gmail with the [Connect Google Mail](connect-google-mail.md) guide. Add
the label `bob` in Gmail, then confirm that `bob gmail doctor` passes on the
host configured as `gmail.writer`.

## 1. Make a phone-friendly address

Use Gmail's plus addressing to create an address such as
`user+bob@example.com`. Messages sent to it still arrive in `user@example.com`.
From your phone, forward a message you want to file to that address.

## 2. Label those messages automatically

In Gmail on the web, open **Settings → Filters and blocked addresses → Create
a new filter**. Enter your plus address in **To**, then choose **Create
filter** and select **Apply the label → bob**. If you also choose **Also
apply filter to matching conversations**, Gmail labels matching messages
already in the inbox. Save the filter.

The filter only applies the label. It does not forward mail elsewhere or
change read or archive state.

## 3. Sync the label into Bob

Preview what Bob will file, then sync it on the configured writer host:

```bash
bob gmail sync -d
bob gmail sync
```

Each new thread is saved as a mail note and linked from the area or project
named by a routing label, or from `gmail_inbox.md` when none or an invalid
one is present. Review the tasks there and route them as usual. Syncing
again is idempotent; existing tasks are not duplicated. The Gmail label
stays in place, so you can keep using it as a filing marker.

Change `gmail.sync.inbox` or `gmail.sync.label` in the Gmail config when you
want another inbox file or label. See [Bob Gmail](../gmail.md#sync) for the
full command and JSON contracts.
