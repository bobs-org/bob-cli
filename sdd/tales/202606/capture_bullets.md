---
create_time: 2026-06-19 09:11:25
status: wip
prompt: sdd/prompts/202606/capture_bullets.md
---
# Plan: Add bullet captures to `bob capture`

## Context

`bob capture` currently has one capture shape:

- parse normalized text into `{ body, route }`;
- format `- [ ] #task <body> [created::YYYY-MM-DD]`;
- write the line to `mac_inbox.md` or `<route>.md`;
- prefer a `Tasks` section, then fall back to the last top-level task block.

The requested change adds a second capture shape, ordinary Markdown bullets, selected by a terminal `#` or
`#<section-prefix>` marker. This should be a behavior extension, not a new subcommand or option. Existing task capture
behavior and the current `@route` contract should remain stable for callers that do not use the new marker.

Note: I attempted the required long-memory read for `cli_rules.md`, but `sase memory read long/cli_rules.md` was
rejected because the reader requires flat note names, and `sase memory read cli_rules.md` reported the note absent. This
plan therefore avoids adding CLI surface area and follows the existing local `capture` conventions visible in code and
tests.

## Desired behavior

1. Task captures without a terminal `#...` marker keep their existing behavior.
2. Bullet captures render as:

   ```md
   - <body> [created::YYYY-MM-DD]
   ```

3. The terminal marker is stripped from the captured body:
   - `Some bullet #Ideas` -> bullet routed to the first non-`Tasks` section whose title starts with `Ideas`.
   - `Some bullet #` -> bullet routed to the first non-`Tasks` section.
4. A terminal `@route` and a terminal `#...` may appear in either order:
   - `Some bullet @foo #`
   - `Some bullet # @foo`

   Both should capture the same bullet body into `foo.md`.

5. `--route NAME` should continue to force the note target and keep `@tokens` literal in the text, but it should still
   allow the terminal `#...` marker to select bullet mode.
6. Bullet placement must never target the `Tasks` section. It should use:
   - first matching non-`Tasks` section by heading-title prefix;
   - first non-`Tasks` section when the marker is bare `#`;
   - the zeroth section when no eligible section exists.
7. Within the target section:
   - if there is already at least one top-level ordinary bullet, insert after the last bullet block and its indented
     continuation lines;
   - otherwise insert immediately after the section header plus one blank line;
   - for the zeroth section, use the same "last ordinary bullet, otherwise section start" rule before the first real
     heading.

## Implementation approach

### 1. Split parsed capture kind from route

Replace `ParsedCaptureText { body, route }` with a shape that also records capture kind:

```rust
enum CaptureKind {
    Task,
    Bullet { section_prefix: Option<String> },
}

struct ParsedCaptureText {
    body: String,
    route: Option<String>,
    kind: CaptureKind,
}
```

Keep route normalization rules unchanged: route tokens still use the existing `is_route_token()` validation and are
lower-cased.

### 2. Add terminal control parsing

Add a small parser for terminal control tokens after whitespace normalization.

- Recognize a final token starting with `#` as the bullet marker.
  - `#` means `section_prefix: None`.
  - `#X` means `section_prefix: Some("X")`.
- When no `--route` is present, recognize one terminal `@route` token next to the bullet marker, in either order.
- Consume at most one bullet marker and at most one route marker; leave any earlier duplicate-looking tokens in the
  body.
- Validate body after stripping controls. If the remaining body is empty, return the existing usage-error style.
- If no bullet marker is present, delegate to the current auto-route parsing so normal task captures remain
  byte-for-byte compatible.
- If a bullet marker is present and no terminal route was consumed, preserve the existing leading `@route body` prefix
  behavior where possible.

This keeps the new grammar narrow and makes the new `#...` behavior explicit.

### 3. Generalize line formatting and result naming carefully

Add:

```rust
fn format_bullet_line(body: &str, created: &str) -> String
```

Internally rename local variables from `task_line` to a neutral `capture_line`/`line` where practical.

For JSON compatibility, keep the existing `task_line` field populated with the rendered line for both task and bullet
captures. Additive fields such as `kind: "task" | "bullet"` and/or `line` can be considered during implementation, but
existing callers should not lose `task_line`.

Human output should print the rendered line exactly as before, just with the bullet line when bullet mode is selected.

### 4. Route to separate placement algorithms by kind

Keep the current `insert_task_line()` path unchanged for `CaptureKind::Task`.

Add a new `insert_bullet_line(contents, bullet_line, section_prefix)` path that:

- reuses existing line-span, ATX-heading, frontmatter, and fenced-code helpers;
- identifies Markdown sections using the same heading parser that already ignores frontmatter and code fences;
- skips headings whose parsed title is exactly `Tasks`;
- matches target section title with `starts_with(prefix)` when a prefix is provided;
- treats bare `#` as a match on the first non-`Tasks` heading;
- falls back to the zeroth section when no heading matches.

Represent target section ranges explicitly, for example:

```rust
struct MarkdownSection {
    heading_end: Option<usize>,
    start_line: usize,
    end_line: usize,
    insertion_start: usize,
}
```

For ordinary headings, `insertion_start` is the heading line end. For the zeroth section, it is after YAML frontmatter
when present, otherwise byte index `0`.

### 5. Detect ordinary bullet blocks

Add a top-level ordinary-bullet detector distinct from task detection:

- match top-level lines beginning with `- `;
- exclude task checkbox lines such as `- [ ] ...`, `- [x] ...`, `- [/] ...`, and `- [B] ...`;
- ignore indented bullets as insertion anchors.

Reuse the existing block-end logic for continuation lines so insertion happens after nested bullet children and indented
continuation content.

When a target section has ordinary bullets, insert after the final ordinary bullet block. When it has none:

- heading section: insert at `heading_end` using helper text that produces exactly one blank line before the new bullet;
- zeroth section: insert at `insertion_start`, adding a trailing newline as needed without disturbing frontmatter.

### 6. Preserve file creation behavior

For missing target files:

- task captures create a one-line task file exactly as today;
- bullet captures create a one-line bullet file in the zeroth section.

Dry-run behavior should still avoid writes and report the same target and placement semantics.

## Tests

Add unit tests in `src/native/capture.rs` for:

- parsing `Some bullet #Ideas`;
- parsing `Some bullet #`;
- parsing `Some bullet @foo #` and `Some bullet # @foo` to identical body/route/kind;
- preserving forced route with literal `@tokens` while still consuming `#...`;
- rejecting marker-only bullet input such as `#`;
- formatting bullet lines with `[created::YYYY-MM-DD]`;
- inserting first bullet after a matched section header plus one blank line;
- inserting after the last ordinary bullet block and its indented continuation lines;
- skipping the `Tasks` section even when its title starts with the prefix;
- bare `#` selecting the first non-`Tasks` section;
- unmatched prefix falling back to the zeroth section;
- zeroth-section insertion after YAML frontmatter;
- ignoring headings in frontmatter and fenced code.

Add CLI tests in `tests/cli.rs` for:

- routed bullet capture into an existing note section by `#<prefix>`;
- equivalence of terminal marker ordering with `@route` and `#`;
- bare `#` selecting the first non-`Tasks` section in `mac_inbox.md`;
- JSON output for a bullet capture, including the rendered bullet line in the compatibility field.

Keep existing task-capture tests unchanged unless mechanical field additions require expected JSON additions.

## Documentation

Update:

- `src/native/capture.rs` command help: describe "task or bullet" capture and show one bullet example.
- `README.md` `bob capture` section: document terminal `#` / `#<section-prefix>` markers, bullet line format, and how
  they combine with terminal `@route`.
- `src/runner.rs` top-level examples only if it remains concise and useful; no new command option is needed.

## Verification

Run focused checks first:

```bash
cargo test capture
cargo test --test cli capture
```

If those pass, run the broader project check that is customary for this repo, likely:

```bash
cargo test
```

Also use one scratch-vault manual command for the end-to-end behavior:

```bash
BOB_DIR=<tmp-vault> BOB_NOW='2026-06-15' cargo run -- capture -- 'Some bullet #Ideas @foo'
```
