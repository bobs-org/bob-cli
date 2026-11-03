# Task Merge

aka merge, merged into

Task Merge folds one or more source tasks into a target task, one of its drawers, or a
note section through `bob task merge`, capture's `>`, or the configured `merged into`
relation. It cuts the selected source blocks and appends their notes to the destination,
retiring each source as a cancelled tombstone rather than copying and deleting it.
Dependent tasks and affected links are repaired, and the source records the target in
its `🪢 LINKS: merged into` line. See `docs/task-merge.md` for the contract and edge
cases. This task-outline operation is distinct from a [[merged-pomodoro]], which
combines work sessions.
