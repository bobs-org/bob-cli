---
title: "File email from your phone"
description: "Forward a message to a Gmail plus address, apply the Bob label automatically, and pull it into the vault inbox."
tags:
  - CLI
---

# File email from your phone

This guide routes a message you forward from your phone into Bob's Gmail task
inbox. Bob reads the Gmail label and writes a mail note plus a task link; it
does not archive, mark read, or remove the label.

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

## 3. Pull the label into Bob

Preview what Bob will file, then pull it on the configured writer host:

```bash
bob gmail pull -d
bob gmail pull
```

Each new thread is saved as a mail note and linked from `gmail_inbox.md` by
default. Review the tasks there and route them as usual. Pulling again is
idempotent; existing tasks are not duplicated. The Gmail label stays in
place, so you can keep using it as a filing marker.

Change `gmail.pull.inbox` or `gmail.pull.label` in the Gmail config when you
want another task file or label. See [Bob Gmail](../gmail.md#pull) for the
full command and JSON contracts.
