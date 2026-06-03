# Dataview Parity Fixture

This vault is a deterministic corpus for `bob dataview` native parity work.
It intentionally combines cases the current native engine can read with cases
reserved for later native parity phases.

The fixture covers:

- scalar, array, object, null, missing, date, datetime, duration, and link fields
- aliases, tags, subtags, inline fields, wikilinks, incoming links, and outgoing links
- folder, file, daily-note, task, nested-task, and task-metadata scenarios
- origin/`this` queries through `Origins/Origin.md`

Keep fixture data small and stable. Add new query expectations in
`tests/dataview_parity.rs` when expanding the native engine.
