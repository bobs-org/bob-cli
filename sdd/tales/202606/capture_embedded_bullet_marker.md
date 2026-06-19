---
create_time: 2026-06-19 14:17:38
status: wip
prompt: sdd/prompts/202606/capture_embedded_bullet_marker.md
---
# Plan: Require Bullet Markers On Capture Route Tokens

## Goal

Change `bob capture` bullet syntax so bullet mode is selected only by appending the section marker to the `@route`
token:

- `Some note. @foo#bar` captures a bullet into `foo.md`, targeting the `bar` section prefix.
- `@foo#bar Some note.` does the same, because leading `@route` syntax is already supported.
- `Some note. @foo#` captures a bare-marker bullet into `foo.md`.

The old standalone terminal marker forms should stop being accepted:

- `Some note. #bar @foo`
- `Some note. @foo #bar`
- `Some note. #bar`
- `Some note. #`

This is a syntax refinement for the existing `capture` command, not a new subcommand or option.

## Context Reviewed

- `src/native/capture.rs`
  - `parse_capture_text` currently normalizes text, peels a terminal standalone `#...` marker plus an optional terminal
    `@route`, then delegates to leading/trailing route parsing.
  - `CaptureKind::Bullet { section_prefix }` already drives bullet formatting and section placement.
  - `peel_terminal_controls` is the current syntax chokepoint and should be replaced or narrowed.
  - `parse_leading_route`, `parse_trailing_route`, and `is_route_token` encode the route grammar and route precedence.
- `tests/cli.rs`
  - Existing integration tests cover `#Ideas @foo`, `@foo #`, `# @foo`, bare `#` into the inbox, JSON output, and
    case-insensitive section matching.
- `README.md` and `src/native/capture.rs` long help
  - Both currently document the old standalone `#` / `#<section-prefix>` marker and state that route/marker order is
    flexible.
- Existing SDD plans
  - `sdd/tales/202606/capture_bullets.md` documents the original standalone marker design.
  - `sdd/tales/202606/capture_bullet_section_matching.md` documents the later section matching refinement.
- Project memory
  - CLI memory says help output should stay clear and complete when CLI behavior changes.
  - Obsidian memory confirms these captures are vault note workflows under `~/bob`.

## Desired Grammar

Keep the current route positions:

- Leading route: `@route body...`
- Trailing route: `body... @route`
- Leading route wins over any trailing route-looking body text, as today.

Extend those route tokens to optionally include a bullet suffix:

- `@route#prefix`
- `@route#`

Parse the route token as:

- route part: the text after `@` and before the first `#`, validated with the existing `is_route_token` rules and
  lower-cased exactly like current routes;
- optional bullet part: the text after the first `#`;
- empty bullet part means the existing bare-marker behavior (`section_prefix: None`);
- non-empty bullet part means `section_prefix: Some(...)`, preserving the user's prefix text and keeping
  case-insensitive matching in placement, as today.

Examples:

- `Some note. @foo#bar` -> body `Some note.`, route `foo`, kind `bullet`, section prefix `bar`.
- `@foo#bar Some note.` -> body `Some note.`, route `foo`, kind `bullet`, section prefix `bar`.
- `Some note. @Foo-Bar#R` -> route `foo-bar`, kind `bullet`, section prefix `R`.
- `Some note. @foo#` -> route `foo`, kind `bullet`, no section prefix.
- `Some note. @foo` -> unchanged task capture routed to `foo.md`.
- `@foo Some note.` -> unchanged task capture routed to `foo.md`.
- `Some note.` -> unchanged task capture to `mac_inbox.md`.

## Compatibility Decision

Reject legacy standalone bullet marker forms with a usage error instead of treating them as literal task text.

Reason: `Some note #bar @foo` currently creates a bullet. If the new parser simply ignored standalone `#bar`, the same
input would silently create a task containing `#bar`, which is a worse migration failure than a clear error.

The error should be specific enough to teach the new syntax, for example:

```text
bullet section markers must be appended to an @route token; use @foo#bar instead of #bar @foo
```

This keeps current users from accidentally creating the wrong item type while still freeing non-terminal `#tag` text to
remain normal task body text.

## Implementation Approach

1. Replace terminal-control peeling with route-token parsing.

   Introduce a small parsed route token representation:

   ```rust
   struct RouteToken {
       route: String,
       bullet: Option<Option<String>>,
   }
   ```

   Add a helper that accepts a token beginning with `@`, splits at the first `#` when present, validates only the route
   part with `is_route_token`, lower-cases the route, and records the optional bullet marker.

2. Preserve route precedence in `parse_capture_text`.

   For auto routing:
   - normalize whitespace as today;
   - if the first token is a valid route token and there is body text after it, use it and do not inspect later route
     tokens for routing, preserving "leading route wins";
   - otherwise, if the last token is a valid route token and there is body text before it, use it;
   - otherwise, fall back to an unrouted task, after legacy-marker validation.

   The selected route token decides capture kind:
   - no `#` suffix -> `CaptureKind::Task`;
   - `#` suffix present -> `CaptureKind::Bullet { section_prefix }`.

3. Handle missing body explicitly.

   Keep plain `@route` behavior compatible where possible, but treat control-looking route+bullet tokens without body as
   usage errors:
   - `@foo` can remain literal task text as it does today.
   - `@foo#bar` and `@foo#` should return the existing missing-text usage error because the token clearly expresses a
     routed bullet capture with no body.

4. Retire standalone bullet parsing.

   Remove or stop using `peel_terminal_controls` for new parsing. Add a legacy marker detector before returning a task
   for inputs that match the old control positions:
   - final token starts with `#`;
   - final token is a plain valid `@route` and the preceding token starts with `#`.

   Return the new usage error for those cases. Do not reject `#tag` when it appears in the middle of the body.

5. Decide `--route` behavior narrowly.

   Since the new syntax requires the `#...` marker to be appended to an `@route` marker, `--route NAME` should no longer
   honor standalone terminal `#...` markers. It should continue to force the target and keep `@tokens` literal, matching
   the existing purpose of `--route`.

   Under this plan:
   - `bob capture --route foo "Some note #bar"` returns the legacy marker usage error.
   - `bob capture --route foo "Some #tag note"` remains a routed task with literal body text.
   - If forced-route bullet capture becomes important later, add an explicit option or syntax rather than preserving the
     deprecated standalone marker only for `--route`.

6. Leave placement and rendering behavior alone.

   No changes are needed to:
   - `format_bullet_line`;
   - JSON result shape (`task_line` still carries the rendered line);
   - `insert_bullet_line`;
   - section prefix matching;
   - H1 fallback behavior;
   - dry-run and file creation mechanics.

7. Update docs and help.

   Revise:
   - `src/native/capture.rs` long help and examples;
   - `README.md` capture section.

   The docs should say that route tokens may be written as `@route#section-prefix` or `@route#`, and that standalone
   terminal `#...` markers are no longer valid. Examples should use `bob capture jot idea @notes#Ideas` and, if useful,
   `bob capture @notes#Ideas jot idea`.

## Tests

Update unit tests in `src/native/capture.rs`:

- Add parsing tests for `Some note @foo#bar`, `@foo#bar Some note`, `Some note @foo#`, and `@foo# Some note`.
- Keep route lower-casing coverage with a suffixed token such as `@Foo-Bar#R`.
- Add missing-body coverage for `@foo#bar` and `@foo#`.
- Replace old `terminal_marker_order_is_equivalent` expectations with legacy syntax errors.
- Update forced-route tests so standalone terminal `#...` is rejected and middle `#tag` text remains literal.
- Keep existing bullet formatting and bullet insertion tests unchanged.

Update CLI integration tests in `tests/cli.rs`:

- Change routed bullet examples from `#Ideas @foo` to `@foo#Ideas`.
- Add a leading-route bullet test for `@foo#Ideas Some bullet`.
- Change marker-order equivalence tests to compare leading `@foo#` and trailing `@foo#`, not old split marker order.
- Replace the bare inbox `#` test with a usage-error test, because bullet capture now requires an explicit note route.
- Add a legacy-form error test for `Some bullet #Ideas @foo` so the migration behavior is locked in.
- Keep JSON bullet output tests, but use `@foo#Ideas`.
- Preserve the existing section-placement integration test by changing only the input syntax from `@foo #r` to `@foo#r`.

Update docs/help tests if any expected snippets mention the old examples or grammar.

## Verification

Run focused checks first:

```bash
cargo fmt --check
cargo test capture
cargo test --test cli capture_bullet
```

Then run the broader repo checks:

```bash
cargo test
cargo clippy --all-targets
```

Manual smoke checks in a temporary vault:

```bash
tmp="$(mktemp -d)"
printf '# Foo\n## Ideas\nnotes\n' > "$tmp/foo.md"
BOB_NOW=2026-06-15 cargo run -- capture -b "$tmp" -- 'Some note. @foo#Ideas'
BOB_NOW=2026-06-15 cargo run -- capture -b "$tmp" -- '@foo#Ideas Another note.'
BOB_NOW=2026-06-15 cargo run -- capture -b "$tmp" -- 'Old syntax #Ideas @foo'
```

Expected results:

- the first two commands create ordinary bullets in `foo.md`;
- the old syntax command fails with the new usage error and does not write a task.

## Risks

- Silent migration mistakes: mitigated by rejecting old standalone marker positions instead of treating them as literal
  task text.
- Hashtag task text at the old terminal-control position remains unavailable without changing wording, which is
  consistent with current behavior because those inputs already triggered bullet mode.
- Parser precedence can drift if leading and trailing route handling are refactored too broadly. Keep the existing
  leading-route-wins rule and add tests around mixed route-looking tokens.
- `--route` users who depended on standalone bullet markers will need to switch to explicit `@route#prefix` auto-route
  syntax or request a separate forced-route bullet affordance. This is intentional under the new grammar.

## Success Criteria

- `Some note. @foo#bar` behaves like old `Some note. #bar @foo`.
- `@foo#bar Some note.` works as a leading routed bullet capture.
- Old standalone terminal marker forms fail clearly and do not create tasks.
- Normal task route syntax without `#` is unchanged.
- Bullet placement, JSON shape, dry-run behavior, and section matching stay unchanged apart from the new marker
  location.
- Help text, README docs, unit tests, and CLI integration tests all describe the new grammar.
