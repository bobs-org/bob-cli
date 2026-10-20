---
title: Reader testing
description: Test whether Bob's documentation helps new readers choose a useful first path.
---

# Reader testing

Reader tests check whether someone can find useful Bob documentation without knowing
the site structure in advance. Test the live site and `llms.txt` as separate entry
points, and record the route each reader tries before they reach a page.

## Six-task protocol

Use five independent readers for each entry point. Give each reader only the live site
URL or only the `llms.txt` URL. Do not give them the expected door, page name, or the
other entry point. Present tasks one at a time, in the table's order, and do not explain
Bob or point to a page. Allow up to three minutes per task; do not offer hints. Stop a
task when the reader says they have the answer or asks for help.

Record the first door or `llms.txt` section the reader chooses, whether they reach the
intended page, elapsed time, and any short observation. The site doors map to the
published tabs as follows: Learn → Get started, Do → Guides, Understand → Concepts,
Look up → Reference. For `llms.txt`, use the matching section title. A task succeeds
when the reader reaches the intended page and can explain the requested goal or fact.

The pass bar is at least four of five readers choosing the expected door first for each
task. A miss is evidence to improve labels or homepage copy. Keep page locations and
the URL contract stable; reader tests do not authorize moving pages.

| Task | Expected door | Intended page |
| --- | --- | --- |
| Is Bob for me? | Learn | [Is Bob for me?](../start/is-bob-for-me.md) |
| Capture your first task | Learn | [Your first Bob day](../start/first-bob-day.md) |
| Sync the vault across machines | Do | [Sync the vault across machines](../guides/sync-vault-across-machines.md) |
| What is a lane? | Understand | [Tasks and lanes](../concepts/tasks-and-lanes.md) |
| Find an environment variable | Look up | [Configuration](../reference/configuration.md) |
| Fix “bob not resolved” in Mac Capture | Do | [Fix Mac Capture when bob is not resolved](../guides/fix-mac-capture-bob-resolution.md) |

## Results table

Keep one row per reader and task. Use a stable anonymous reader ID, and record separate
rounds for site-only and `llms.txt`-only readers.

| Reader | Entry point | Task | Expected door | First door or section | Reached intended page? | Time | Observation |
| --- | --- | --- | --- | --- | --- | --- | --- |
| R1 | Site / `llms.txt` |  |  |  |  |  |  |
