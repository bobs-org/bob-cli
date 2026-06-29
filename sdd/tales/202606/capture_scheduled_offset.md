---
create_time: 2026-06-29 14:06:30
status: wip
prompt: sdd/prompts/202606/capture_scheduled_offset.md
---
# Plan: `s:<N>` scheduled-offset syntax for `bob capture`

## Repository

This feature lives entirely in the **`bob-cli`** Rust crate. The only source file that needs logic changes is
`src/native/capture.rs`; tests are added to `tests/cli.rs` (integration) and the `#[cfg(test)]` module inside
`src/native/capture.rs` (unit). No new dependency is needed — `chrono` is already a dependency
(`chrono = { version = "0.4", default-features = false, features = ["clock", "std"] }`).

## Goal

Let a user append a trailing `s:<N>` token to `bob capture` input to schedule the captured task `N` days from today. The
token is **stripped from the generated task** and replaced by a `[scheduled::YYYY-MM-DD]` inline property, where the
date is today + `N` days (`s:0` = today, `s:1` = tomorrow, `s:7` = a week out).

`s:<N>` must coexist with the existing trailing `@<note_file>` route token in **either order**:

```
bob capture buy milk s:1                 # → mac_inbox.md, scheduled tomorrow
bob capture buy milk s:2 @groceries      # → groceries.md, scheduled in 2 days
bob capture buy milk @groceries s:2      # → groceries.md, scheduled in 2 days  (same result)
bob capture @groceries buy milk s:3      # leading route + trailing schedule
```

The result must be **intuitive** (mirrors the familiar trailing-`@route` muscle memory and reads naturally),
**reliable** (only a terminal `s:<N>` is special; `s:`-looking text in the middle of a task body is left untouched), and
**consistent** with the existing capture output (a new bracketed inline property appended after `[created::]`, same
Dataview inline-field style the command already emits).

## Background: how capture parses today

`parse_capture_text(raw_text, forced_route, forced_section)` in `src/native/capture.rs` is the single choke point all
modes flow through (plain text, `--route`/`--section` forced modes, and stdin). It:

1. Normalizes whitespace (`normalize_task_text`) into `normalized`, then splits into `tokens: Vec<&str>`.
2. Handles `--section` and `--route` forced modes (body = the whole `normalized` string, `@tokens` kept literal).
3. Otherwise resolves an `@route` token: a **leading** route wins (`@foo body…`); else a **trailing** route routes the
   body that precedes it (`body… @foo`); else the capture is unrouted (→ `mac_inbox.md`).
4. Returns a `ParsedCaptureText { body, route: Option<String>, kind: CaptureKind }`.

The caller `capture()` then computes `created = current_date_string()` (which reads `bob_env::current_datetime()`,
honoring the `BOB_NOW`/`DATE` test overrides), formats the line via `format_task_line` / `format_bullet_line`
(`- [ ] #task {body} [created::{created}]`), writes it, and builds a `CaptureResult` (the struct serialized for
`-f json`). Route token validation lives in `parse_route_token` / `is_route_token`; malformed `@tokens` simply stay
literal, which is the precedent this feature follows for malformed `s:` tokens.

`s:<N>` is **inline input syntax** (like `@route`), **not** a new CLI flag — so it adds nothing to the `--help` option
list and raises no alphabetical-ordering concern from the CLI rules.

## Product behavior

### Recognized token

A trailing token is treated as a schedule offset when it is exactly `s:` followed by one or more ASCII digits (`s:0`,
`s:1`, `s:42`). Matching rules, chosen to mirror how `@route` already behaves:

- **Case-sensitive, lowercase `s:`** (per the requested syntax). `S:1` is not a schedule token; it stays literal.
- **Non-matching shapes stay literal** (no error), exactly like an invalid `@token`: `s:` (empty), `s:abc`, `s:-1`,
  `s:1.5`, `sx:1`, or digits that overflow `u64` are _not_ schedule tokens and remain part of the task body.
- **Out-of-range but well-formed** offsets (a valid `u64` whose date addition overflows the calendar, e.g.
  `s:9999999999`) produce a clear **usage error** rather than silently dropping, because the token _was_ a schedule
  request.

### Position — terminal only, either side of the route

Only the **end** of the input is scanned, in these positions:

- The **last** token is `s:<N>` → strip it. Covers `body s:1` and `body @route s:1`.
- The last token is an `@route` token **and** the **second-to-last** is `s:<N>` → strip the `s:<N>`, leave the route in
  place. Covers `body s:1 @route`.

This makes both `s:<N> @route` and `@route s:<N>` work, and also composes with a **leading** route (`@route body s:1` →
strip `s:1`, then the leading-route rule handles `@route`). A non-terminal `s:<N>` (e.g. `take s:1 pill before bed`) is
never special — same safety model the trailing-`@route` syntax already relies on. Exactly one schedule token is
extracted; a second `s:<N>` deeper in the body stays literal.

### Generated output

When a schedule offset `N` is present, append `[scheduled::YYYY-MM-DD]` **after** `[created::…]` on the generated line:

```
bob capture buy milk s:1
# (with today = 2026-06-15)
- [ ] #task buy milk [created::2026-06-15] [scheduled::2026-06-16]
```

- `created` and `scheduled` are derived from the **same** `bob_env::current_datetime()` call basis, so the `BOB_NOW`/
  `DATE` overrides keep tests deterministic and the two dates stay consistent.
- The property is appended uniformly to **both** task lines and bullet lines (`format_bullet_line`). Recommendation:
  apply it uniformly — the formatting path is shared, it matches the user's "add the property" intent, and
  `[scheduled::]` on a non-`#task` bullet is inert-but-harmless Dataview metadata. (Restricting to task mode only, if
  ever desired, is a one-line guard; called out here as the single open design choice.)
- `-f json` output gains a `scheduled` field (see Technical design §6); the human output already prints the full
  `task_line`, so the scheduled date is visible there with no extra formatting.

## Technical design

All changes are in `src/native/capture.rs` unless noted.

### 1. Carry the offset on `ParsedCaptureText`

Add `scheduled_offset: Option<u64>` to `ParsedCaptureText`. Parsing stays **pure** (no clock access): it records the
number of days; the actual date is computed later in `capture()` alongside `created`. Update the existing constructions
of `ParsedCaptureText` (the two forced-mode returns, the unrouted fallthrough, and `RouteToken::into_parsed`) to thread
the offset through. `RouteToken::into_parsed` takes the offset as a parameter (or the caller sets the field after) so
both the leading- and trailing-route paths attach it.

### 2. `parse_schedule_token` helper

Mirror `parse_route_token`:

```rust
/// Parse one whitespace-free token as a schedule offset (`s:<N>`), returning the
/// non-negative day count. Returns None when the token is not `s:` + ASCII digits
/// or the digits overflow u64 (left as literal body text, like an invalid @route).
fn parse_schedule_token(token: &str) -> Option<u64> {
    let digits = token.strip_prefix("s:")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()
}
```

### 3. `extract_trailing_schedule` — strip from the end region

A small mutator run **before** route resolution and before `reject_legacy_bullet_markers`, so the legacy-marker check
and the leading/trailing route logic operate on the already-reduced token list:

```rust
/// Remove a terminal `s:<N>` token (the last token, or the second-to-last when the
/// last token is an @route token) and return its offset. Pure; no clock access.
fn extract_trailing_schedule(tokens: &mut Vec<&str>) -> Option<u64> {
    if let Some(&last) = tokens.last()
        && let Some(n) = parse_schedule_token(last)
    {
        tokens.pop();
        return Some(n);
    }
    if tokens.len() >= 2
        && parse_route_token(tokens[tokens.len() - 1]).is_some()
        && let Some(n) = parse_schedule_token(tokens[tokens.len() - 2])
    {
        tokens.remove(tokens.len() - 2);
        return Some(n);
    }
    None
}
```

### 4. Wire extraction into `parse_capture_text`

At the top, after building `tokens`, extract the offset, then rebuild the body string so **every** downstream mode sees
the schedule token removed (note: `tokens` borrows `normalized`, so the reduced body must be a fresh `String`, not a
reassignment of `normalized`):

```rust
let normalized = normalize_task_text(raw_text);
if normalized.is_empty() { return Err(missing_text_error()); }
let mut tokens: Vec<&str> = normalized.split(' ').collect();
let scheduled_offset = extract_trailing_schedule(&mut tokens);
if tokens.is_empty() { return Err(missing_text_error()); } // input was only `s:<N>`
let body = tokens.join(" ");
```

Then:

- Replace the three `body: normalized` / final-`normalized` usages with `body`, and set `scheduled_offset` on each
  returned `ParsedCaptureText`.
- The leading-route (`tokens[1..].join(" ")`) and trailing-route (`rest.join(" ")`) paths already build their body from
  the reduced `tokens`, so they need only attach `scheduled_offset`.
- The empty-after-extraction guard prevents a `tokens[0]` panic and gives the same `missing_text_error` a bare `s:1`
  deserves.

Forced `--route`/`--section` modes thus also honor a trailing `s:<N>` (the token is stripped from `body` before the
literal-`@tokens` body is formed) — consistent and low-cost.

### 5. Compute the scheduled date in `capture()`

Add an env-aware, overflow-safe helper next to `current_date_string()`:

```rust
fn scheduled_date_string(offset_days: u64) -> Result<String, CaptureError> {
    let date = bob_env::current_datetime()
        .date()
        .checked_add_days(chrono::Days::new(offset_days))
        .ok_or_else(|| CaptureError::usage("scheduled offset is out of range"))?;
    Ok(format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day()))
}
```

In `capture()`, after `let created = current_date_string();`:

```rust
let scheduled = match parsed.scheduled_offset {
    Some(n) => Some(scheduled_date_string(n)?),
    None => None,
};
```

and pass `scheduled.as_deref()` into the formatters. (`chrono::Datelike` is already imported; add `chrono::Days`.)

### 6. Formatting and `CaptureResult`

- `format_task_line(body, created, scheduled: Option<&str>)` and `format_bullet_line(body, created, scheduled)` append
  `" [scheduled::{s}]"` after the `[created::…]` segment when `scheduled` is `Some`, and are unchanged otherwise.
- Add `scheduled: Option<String>` to `CaptureResult` (derives `Serialize`). Following the existing
  `route: Option<String>` precedent it serializes as `null` when absent — no `skip_serializing_if`, so the JSON shape is
  stable for consumers. Populate it from the computed `scheduled` in `capture()`.

### 7. Help text (CLI rules: keep `--help` excellent)

Update `build_cli()`:

- Add a `long_about` paragraph describing `s:<N>`: a trailing `s:<N>` (lowercase `s`, non-negative integer) schedules
  the task `N` days out via `[scheduled::YYYY-MM-DD]`, is removed from the task text, may sit before or after a trailing
  `@route`, and is only recognized at the very end of the input.
- Add `after_help` examples, e.g. `bob capture buy milk s:1` and `bob capture buy milk s:2 @groceries`.

No new `Arg` is added, so the option list and its alphabetical ordering are unchanged.

## Edge cases

- **`s:<N>` only, no body** (`bob capture s:1`) → `missing_text_error` (usage), via the post-extraction empty guard.
- **Both orders** (`s:1 @route` and `@route s:1`) → identical routed + scheduled result.
- **Leading route + trailing schedule** (`@route body s:1`) → strip `s:1`, leading route resolves; scheduled attached.
- **Non-terminal `s:<N>`** (`take s:1 pill`) → stays literal; no schedule, no error.
- **Two schedule tokens** (`buy milk s:1 s:2`) → only the terminal `s:2` is extracted; `s:1` stays literal. (Documented;
  not an error.)
- **Malformed schedule-ish tokens** (`s:`, `s:abc`, `s:-1`, `s:1.5`, `S:1`, u64-overflowing digits) → left literal, like
  an invalid `@token`.
- **Calendar overflow** for a valid-but-huge `u64` (`s:9999999999`) → usage error "scheduled offset is out of range".
- **`s:0`** → today's date (equals `created`); allowed.
- **Bullet mode** (`@notes#Ideas jot idea s:1`) → bullet line gains `[scheduled::…]` (uniform behavior; see Product
  behavior note).
- **`--route`/`--section` forced modes** → trailing `s:<N>` still stripped and applied; `@tokens` in the body remain
  literal as today.
- **Ambiguity with literal trailing `s:N` text** (e.g. a task that genuinely ends in `s:1`) → captured as a schedule.
  This is the same accepted trade-off as a task that genuinely ends in `@word` being routed; noted, not mitigated.

## Validation

From the crate root (the `justfile` wraps these as `just all` → `fmt` + `lint` + `test`):

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features` (clean — no new warnings)
- `cargo test` — existing suite stays green; new tests added:
  - **Integration (`tests/cli.rs`)**, using `.env("BOB_NOW", "2026-06-15 10:11:12")` for determinism, asserting exact
    file contents / JSON:
    - `s:1` unrouted → `mac_inbox.md` line `… [created::2026-06-15] [scheduled::2026-06-16]`.
    - `s:0` → scheduled equals created.
    - `buy milk s:2 @groceries` and `buy milk @groceries s:2` → both route to `groceries.md` with
      `[scheduled::2026-06-17]`.
    - Leading route `@groceries buy milk s:3` → routed + scheduled.
    - Non-terminal `s:1` stays literal in body.
    - `-f json` includes `"scheduled":"2026-06-16"` (and `null` when absent).
    - `--dry-run` reports the scheduled `task_line` without writing.
    - Bare `s:1` → exit code 2 (usage error).
  - **Unit (`#[cfg(test)]` in `capture.rs`)**:
    - `parse_schedule_token` accepts `s:0`/`s:1`/`s:42`; rejects `s:`/`s:abc`/`s:-1`/`S:1`/overflow.
    - `extract_trailing_schedule` strips last-position and second-to-last-before-route; leaves non-terminal tokens.
    - `parse_capture_text` returns the right `scheduled_offset` and `body` for the order-permutations and the
      forced-route mode; bare `s:1` → `missing_text_error`.
    - `format_task_line` / `format_bullet_line` append `[scheduled::…]` only when present, after `[created::]`.
- `git diff --check` (whitespace) and a quick `bob capture --help` read-through for the CLI-rules quality bar.

## Out of scope

- Negative/relative-past offsets, absolute dates (`s:2026-07-01`), or other Tasks properties (`due`, `start`,
  `priority`).
- A `--scheduled`/`-S` CLI flag — the request is explicitly the inline `s:<N>` syntax that mirrors `@route`.
- Leading or mid-input schedule tokens — recognition is intentionally terminal-only.
- Any change to routing, bullet placement, the `[created::]` stamp, or the `@route`/`--route`/`--section` semantics.
