# Mail Close Card

Closing a Gmail Task asks first. No interactive gesture in Obsidian,
`bob capture`, or Bob Mac Capture completes or cancels a Gmail Task
without showing one calm confirmation card that says whose turn the
thread is on and what closing it will cost. A stray Enter, Alt+[,
or muscle-memory keypress keeps the task open.

The Rust model lives in `src/native/gmail/close_guard/`
(`predicate.rs`, `copy.rs`, `digest.rs`); the conformance vectors in
`tests/fixtures/mail_close/vectors.json` are authoritative and
bob-plugins copies them byte for byte. The JavaScript mirror lives in
the navigation-hotkeys and task-status-cycler fragments.

```text
╭──────────────────────────────────────────────────────────────╮
│  (✉!)  Cancel this email task?                              │
│        job.md · Ready → Cancelled                            │
│                                                              │
│   ┊ ✉️ Yogi - Senior SWE - Technical Screen · Derrick Blakely │
│   ↊ ↩ Derrick Blakely · 16h — waiting on your reply           │
│                                                              │
│   Derrick Blakely is waiting on your reply, and nothing      │
│   reopens this task until they write again. Bob's            │
│   archive-on-close can then clear it from your Gmail inbox.  │
│                                                              │
│   [Open in Gmail ⌘↵]        [Keep open]  [Cancel anyway  y]  │
│   y cancels · ⌘↵ opens the thread · any other key keeps it   │
╰──────────────────────────────────────────────────────────────╯
```

## Principles

- **Always, on every surface.** A Gmail Task never moves from an open
  status (`[ ]`, `[*]`, `[/]`, `[?]`) to `[x]`, `[X]`, or `[-]` through
  an interactive gesture without the card. This holds whosever turn it
  is on and whichever verb is used (Done or Cancel).
- **Muscle memory keeps the task.** Only a deliberate `y` (Obsidian)
  or `Y` (Mac) confirms. Enter, Ctrl+Enter, Alt+[, Space, Esc, `q`,
  Ctrl+[, a click outside, and every other key keep the task. The card
  swallows keys until the opening chord is released and ignores
  `event.repeat`, as the decay card already does.
- **Tell the truth about the thread.** The card leads with the human
  fact (who wrote last, how long ago, whether they wait on you) using
  the same glyphs as the mail marks (`mail-link-marks.md`): `↩ Ada · 2h`
  for their move, `✈` for an awaiting send. Unknown facts are omitted,
  never guessed.
- **Offer the better move.** The likeliest reason to pause is an owed
  reply, so the card offers ⌘↵ / Ctrl+↵ **Open in Gmail** (read-only;
  the mail note's `gmail_url`, exactly like the `⌘↵ Gmail` chip).
- **One card per gesture, all or nothing.** A counted batch, Task Link
  batch, Pomodoro cascade, or multi-verdict picker shows one card that
  lists only the email rows. `y` applies the whole gesture; keeping
  writes nothing from that step. A gesture prompts at most once: an
  upstream confirmation travels down as context, so downstream writers
  never ask again.
- **Not crying wolf.** Reopens, merges, automation, requirement lines,
  and tasks created by the same capture are not guarded (see
  Exemptions). Closing a non-email task stays byte-identical, and on
  the cycler's sync paths it stays synchronous.

## What a Gmail Task is

A task is a Gmail Task when its line carries the exact `#task` token
and it links a mail note either on its own line or in a direct
link-only child bullet — the same placement the watch scan uses
(`src/native/gmail/sync/scan.rs`; the guard shares its
`line_wikilinks`, `direct_children`, `link_only_bullet`, and
`link_target` helpers so both agree).

- Deeper descendants, prose child bullets, mentions elsewhere in a
  note, closed rows, frontmatter, and fenced code never qualify.
- Embeds (`![[…]]`), struck child bullets, and inline-code spans never
  qualify.
- Links resolve to note identity (an exact vault path, then a unique
  case-insensitive basename), never to the display label; an ambiguous
  basename or a missing note is not a Gmail Task.
- A resolved note is a mail note under the loose glossary rule:
  frontmatter `type` is `"[[mail]]"` or `gmail_thread_id` is a
  non-empty string. The strict mail-index rule (which also requires
  `content: untrusted-email` and hex ids) is never the gate, so bob
  and the plugins never disagree.

Turn facts come from the mail note's frontmatter: `title`, `who`
(first line, `::` folded to `:`), `turn` (`yours` / `theirs`),
`turn_since`, and `gmail_url`.

## What counts as a close

A close is an open → closed checkbox move on a Gmail Task row:
`[ ]`, `[*]`, `[/]`, or `[?]` to `[x]`, `[X]`, or `[-]`.
`x`/`X` is a Complete verb, `-` is Cancel. Anything else — a reopen,
any write that leaves a task open, an unknown status — is not a close
and is never guarded.

## Surfaces and keys

| Surface | Refusal / card | Confirm | Open thread | Keep |
| ------- | -------------- | ------- | ----------- | ---- |
| Obsidian (every nav- and cycler-owned close, incl. checkbox clicks in Live Preview and Reading view of the task's own note) | Mail Close Card modal | `y` | ⌘↵ / Ctrl+↵ opens the first row's `gmail_url` and keeps the task | any other key, Esc, `q`, Ctrl+[, click outside |
| `bob capture` | refuses the real run unless `-C`/`--mail-close DIGEST` equals the computed digest; dry-run and human output carry the `mail_close` payload | rerun with `--mail-close DIGEST` | the payload's `confirm` prints the exact rerun command | refusal writes nothing, exit 1 (`mail_close_confirmation_required` without a digest, `mail_close_confirmation_mismatch` when it differs) |
| Bob Mac Capture | preview banner plus a Mail Close card built from bob's refusal payload | `Y` re-dispatches with `--mail-close DIGEST` | ⌘↩ opens the first row's `gmail_url` and keeps the card | Return hints and stays, Escape / `q` / Ctrl+[ return to the unchanged draft |

Keeping inside a modal leaves the modal open with its input: the Task
Card reason stage keeps the typed reason, the successor picker keeps
staged verdicts, the relation stage keeps its selection. On a review-walk
landing, keeping writes nothing, journals nothing, does not advance, and
releases the gesture lock; confirming behaves exactly as today. Keeping
shows a 4-second Notice `✉ Kept open · {title}`.

## Copy

Every surface renders the same strings:

| Field | Rule |
| ----- | ---- |
| title | One target: `Complete this email task?` / `Cancel this email task?`. Batch: `Complete N tasks?` / `Cancel N tasks?`, or `Close N tasks?` when verbs mix. |
| subtitle | One target: `{path} · {Before} → {After}`, with status names Ready `[ ]`, Next `[*]`, In Progress `[/]`, Blocked `[?]`, Done `[x]`/`[X]`, Cancelled `[-]`. Batch: `N tasks · K wired to email`, where `K` is the distinct mail-note count. |
| row label | `✉️ {mail title} · {who}`; the task text (after the checkbox and `#task` token) when the mail title is missing. The `· {who}` tail is omitted when `who` is empty. |
| turn line | yours: `↩ {who} · {age} — waiting on your reply` (`who` omitted when unknown, like the reply badge). theirs: `✈ You wrote {Sat, Nov 7} — waiting on {who}` (date via `%a, %b %-d`; the `— waiting on {who}` tail is omitted when `who` is empty; `✈ Waiting on {who}` when the date is missing). Unknown turn: no line. Age and date formats are exactly the reply badge's (`now`, `Nm`, `Nh`, `Nd`) and the no-reply pill's. When a task links several mail notes, the first `yours` mail wins, otherwise the first mail. |
| tone | `warning` when any orphaned row's turn is yours; otherwise `calm`. |
| consequence | A row is _orphaned_ when no other open task still links its thread after this gesture (see Orphaned below). No orphaned rows: `Another open task still watches this thread.` (`these threads` for several). Orphaned and yours: `{who} is waiting on your reply, and nothing reopens this task until they write again.` (empty `who` reads `They are …`; several orphaned threads read `{K} threads are waiting on your reply, and nothing reopens these tasks until they write again.`). Orphaned and all theirs: `A reply from {who} reopens it.` (empty `who` reads `A reply reopens it.`; several read `A reply reopens them.`). Orphaned and otherwise unknown: `Bob stops watching this thread.` (`these threads` for several). Append ` Bob's archive-on-close can then clear it from your Gmail inbox.` (`them … inbox` for several) when `gmail.sync.archive` is on and any row is orphaned. |

Email rows sit in a subtle left-ruled block. The first line is the mail
link label as Bob writes it, ellipsized on one line. The second line is
the turn line. Batch rows add the note path in muted text. At most 5
rows, then `+N more email tasks`. Surface-specific key hints
(`y cancels · ⌘↵ opens the thread · any other key keeps it` in
Obsidian, `Y close · ⌘↩ open in Gmail · esc keep` on the Mac) stay
outside the shared vectors.

Card visuals: a Lucide icon in a softly tinted circle — `mail-warning`
on an orange tint when any orphaned row's turn is yours, `mail` on the
muted tint otherwise. Title semibold, subtitle muted, consequence one
short muted paragraph. Ghost **Open in Gmail** button with a ⌘↵ key
chip (only when a `gmail_url` exists), default-focused **Keep open**,
`mod-warning` **{Complete|Cancel|Close} anyway** with a `y` key chip.
Theme variables only, readable in light and dark. `role="alertdialog"`,
`aria-labelledby` on the title, `aria-describedby` on the consequence;
focus starts on Keep open.

## Orphaned

A row is orphaned when no other open task still links its thread after
this gesture. The count reuses the vault watch scan
(`scan_watching_tasks`, which walks the same placements the predicate
uses) and excludes the rows the gesture closes
(`other_open_linker_count`): zero remaining linkers means orphaned. A
note the strict index cannot see (loose-only frontmatter) has no
watchers, so it counts as orphaned — the conservative direction.

## Digest

The confirmation digest is 12 lowercase hex characters of SHA-256 over
the canonical, sorted `(path, line, before, after, mail note paths)`
rows. One line per target in byte order:

```text
{path}:{line}:{before}:{after}:{mail1},{mail2},…
```

Rows join with `\n` (no trailing newline); mails keep source order. An
empty mail list renders as an empty tail after the final colon; such a
row never guards, and the digest stays total so callers can hash first
and filter later.

## Prompt-once and fail-closed

A gesture prompts at most once. In Obsidian the confirmation travels as
`context.mailCloseConfirmed`; in `bob capture` it is the `--mail-close DIGEST`
rerun; on the Mac it is the `--mail-close DIGEST` re-dispatch. A digest
with nothing to guard is ignored.

Unknown facts stay unknown: missing `who`, `turn`, or `turn_since`
omit their span, and unresolved or ambiguous links are not Gmail
Tasks. When the nav guard API (v1 or later) is unavailable and the twin
says a target is a Gmail Task, the cycler writes nothing and shows
`✉ Not closed — confirming an email task needs Bob Navigation Hotkeys`.
Non-email closes are unaffected.

## Exemptions

Documented, not guarded:

- **Reopens**, and any write that leaves a task open.
- **Task merges.** Retirement tombstones are not a loss: the watch and
  archive follow the merge target.
- **Automation.** gcal lifecycle and refile closes, and reconcile
  settling off-clock Event Tasks.
- **Requirement lines.** They are never `#task` lines.
- **Tasks created and closed by the same capture.**

## Known limits

No interactive gesture can intercept these:

- Typing over the checkbox character by hand.
- Edits made on another device or outside Obsidian and bob.
- Checkbox clicks inside Tasks or Dataview query results that render
  another note's task.

## Vectors and verification

`tests/fixtures/mail_close/vectors.json` pins the contract. Two case
kinds: `predicate` (line, children, a link→path map, and a
path→frontmatter map; expect the Gmail Task verdict and the ordered
mail note paths) and `copy` (targets with verbs and before/after
symbols, mail facts, `now`, `other_open_linkers`, and `archive`;
expect title, subtitle, tone, rows, `more`, consequence, and digest).
They cover the own line, child bullets, nested note bullets, embeds and
struck links, loose versus strict frontmatter, unresolved and ambiguous
links, yours/theirs/unknown turns, a missing `who`, several mails on
one task, mixed verbs, a batch over five rows, orphaned versus not,
and archive on and off. A Rust test runs every case:

```bash
cargo test --lib gmail::close_guard
```

bob-plugins copies the fixture byte for byte and runs it against the
JS twin.
