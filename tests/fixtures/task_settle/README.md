# Task settle vectors

`vectors.json` is a versioned golden contract for the pure note-local settle
pass. Each text value stores logical lines, a line ending (`\n` or `\r\n`),
and whether the final line ending is present. Expected line maps use zero-based
physical line indices; removed lines are omitted. `warning_codes` lists the
grouping warnings in order. The cases cover status destinations and badge
refresh, future local schedules and their boundaries, note eligibility,
newline preservation, and every `GroupingSkipCode`.

Update bob-cli first, then copy: this directory is canonical. Keep
`plugins/bob-ledger-tools/scripts/fixtures/task_settle/` byte-identical by
copying these files after any change; never edit the plugin copy directly.
