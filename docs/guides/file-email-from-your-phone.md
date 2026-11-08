---
title: "File email from your phone"
description: "Label the original conversation with bob plus one routing label, then sync it into the vault."
tags:
  - CLI
---

# File email from your phone

This guide files the original conversation you choose on your phone into
Bob. Apply `bob` plus one `#/note` routing label to that conversation in
Gmail, then Bob saves it as a mail note and links one task from the area or
project named by the routing label, or from the Gmail inbox when none or an
invalid one is present. Bob never archives, marks read, or removes labels.

## Before you start

Set up Gmail with the [Connect Google Mail](connect-google-mail.md) guide. Add
the label `bob` in Gmail, then confirm that `bob gmail doctor` passes on the
host configured as `gmail.writer`.

## 1. Label the original conversation

On your phone, open the conversation you want to file and apply the `bob`
label plus one routing label that names its destination, such as `#/job` or
`+/job_2026_apply_29`. Prefix routing needs `gmail.sync.route_prefixes`
set to `#` and `+`:

```yaml
gmail:
  sync:
    route_prefixes: ["#", "+"]
```

The part after the slash names one area or project note, the way
`bob gmail save -P` does. Absent or `[]` is no-prefix mode: a user label
named after the note routes the thread, and a `#/job` label is ignored so
the thread files to the inbox.

Only the original conversation is watched. Removing the Gmail label later
changes nothing: enrollment is durable after the first sync, even when the
label is gone or the task is closed, moved, or deleted. Reopening the task
or linking its mail note restores the watch without needing the label again.

## 2. Sync the label into Bob

Preview what Bob will file, then sync it on the configured writer host:

```bash
bob gmail sync -d
bob gmail sync
```

Each new thread is saved as a mail note and linked from the area or project
named by its routing label, or from `gmail_inbox.md` when none or an invalid
one is present. A thread whose routing label does not resolve is still filed
to the inbox with a managed `GMAIL ROUTE` child that names the offending
label, the resolver's explanation, and its hint; the first such filing exits
1. Review the tasks there and route them as usual. Syncing again is
idempotent; existing tasks are not duplicated. The Gmail label stays in
place, so you can keep using it as a filing marker.

Change `gmail.sync.inbox` or `gmail.sync.label` in the Gmail config when you
want another inbox file or label. See [Bob Gmail](../gmail.md#sync) for the
full command and JSON contracts.

## Alternative: forward to a plus address

If your phone mail app cannot apply labels, forward the message to a
plus address such as `user+bob@example.com` and filter that address to
`bob` in Gmail on the web (**Settings → Filters and blocked addresses →
Create a new filter**, **To** your plus address, **Apply the label →
bob**). This files the forwarded copy as a new conversation. It does not
watch the original conversation; prefer labeling the original above when
you want replies to resurface its task.
