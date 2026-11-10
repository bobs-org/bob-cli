# Private event note JSON fixtures

These are complete schema-1 responses from the synthetic `FakeGog` cases in
`tests/gcal_note_cli.rs`. The tests compare the normalized whole response,
including event identity, note text, effective fields, spans, preview, status,
warnings, and errors. No live Google account or personal event is used.

Regenerate them from the bob-cli checkout with:

```sh
BOB_UPDATE_GCAL_NOTE_FIXTURES=1 cargo test --test gcal_note_cli
```

The test helper normalizes only the rolling 90-day window timestamps and dates
to the fake clock's fixed 2026-10-30 window. Event timestamps, day-based preview
paths, and valid Google Calendar URLs use stable synthetic event data and stay
intact.
